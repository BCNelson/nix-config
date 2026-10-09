//! Development harness standing in for spoold's picker channel (feature
//! `dev`, never shipped).
//!
//! Creates the socketpair, execs `spool-picker` with the picker end on fd 3
//! (`SPOOL_PICKER_FD=3`), speaks picker protocol v2, answers `Query`/`Thumb`
//! from fake data and prints what the picker sends (never secrets).
//!
//! Answers are deliberately **out of order**: requests arriving within
//! 40 ms (`MOCK_REORDER_MS`) are answered newest first, so stale pages and swapped
//! thumbnails exercise the picker's `seq` matching (`REORDER n` lines count
//! the swaps). A query with an odd number of `"` gets `Error{BadQuery}`.
//!
//! Fake unlock (passphrase `open sesame`, FIDO2 PIN `1234`; 3 wrong PINs
//! block the PIN). While locked only the session items (ids 1-3 and `new`
//! ones) are listed. Commands on stdin:
//!
//! ```text
//! show [X Y [OUTPUT [SCALE]]]  PickerEvt::Show (cursor in global logical coords)
//! hide                  PickerEvt::Hide
//! new TEXT              PickerEvt::NewItem
//! progress DONE TOTAL   PickerEvt::IndexProgress
//! notice TEXT           PickerEvt::Error{seq: None}
//! lock pass|fido|fidopin|both|none   PickerEvt::Locked with those prompts
//! unlock                PickerEvt::Unlocked
//! touch                 the pending FIDO2 touch succeeds (fidopin: asks for the PIN)
//! timeout | nokey       the pending FIDO2 touch fails (Dismissed / Unavailable)
//! bench N [X Y [GAP]]  N × (Show, wait for presented frame, Hide, sleep GAP
//!                       ms, default 60); p50/p99
//! stats                 percentiles of all SPOOL_TIMING lines so far
//! quit
//! ```
//!
//! Run ONLY against a nested headless compositor (see README.md):
//! `cargo run -p spool-picker --features dev --example mock_daemon [-- --show]`

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use spool_proto::{
  CursorPos, Hello, ItemPreview, PickerErrorCode, PickerEvt, PickerReq, PreviewKind,
  UnlockFailReason, UnlockPrompt, UnlockProvider, read_frame, write_frame,
};

/// Requests arriving this close together are answered newest first
/// (`MOCK_REORDER_MS` overrides; 0 = answer immediately, in order, for
/// latency benches).
fn reorder_window() -> Duration {
  Duration::from_millis(
    std::env::var("MOCK_REORDER_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(40),
  )
}
const PASSPHRASE: &str = "open sesame";
const PIN: &str = "1234";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockMode {
  Unlocked,
  Pass,
  Fido,
  FidoPin,
  Both,
  /// Locked, nothing to prompt (waiting for the wallet).
  Wait,
}

impl LockMode {
  fn prompts(self) -> Vec<UnlockPrompt> {
    match self {
      LockMode::Unlocked | LockMode::Wait => vec![],
      LockMode::Pass => vec![UnlockPrompt::Passphrase],
      LockMode::Fido | LockMode::FidoPin => vec![UnlockPrompt::Fido2Touch],
      LockMode::Both => vec![UnlockPrompt::Passphrase, UnlockPrompt::Fido2Touch],
    }
  }
}

struct LockState {
  mode: LockMode,
  /// A FIDO2 touch is "pending" (waiting for `touch`/`timeout`/`nokey`).
  touch_pending: bool,
  /// fidopin: the touch asked for the PIN.
  pin_asked: bool,
  bad_pins: u32,
}

fn is_session_item(id: i64) -> bool {
  id <= 3 || id > 10_000
}

fn mono_ns() -> u64 {
  let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
  // SAFETY: valid out-pointer.
  unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
  ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

fn now_ms() -> u64 {
  SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

struct Item {
  preview: ItemPreview,
  image: Option<Vec<u8>>,
}

fn text_item(id: i64, age_s: u64, text: &str) -> Item {
  Item {
    preview: ItemPreview {
      id,
      kind: PreviewKind::Text,
      preview: text.chars().take(500).collect(),
      mimes: vec!["text/plain;charset=utf-8".into(), "text/plain".into()],
      source_app: Some("mock".into()),
      created_unix_ms: now_ms() - age_s * 1000,
      last_used_unix_ms: now_ms() - age_s * 1000,
      pinned: false,
      tags: vec![],
      total_size: text.len() as u64,
    },
    image: None,
  }
}

fn image_item(id: i64, age_s: u64, mime: &str, bytes: Vec<u8>) -> Item {
  Item {
    preview: ItemPreview {
      id,
      kind: PreviewKind::Image,
      preview: String::new(),
      mimes: vec![mime.into()],
      source_app: Some("mock".into()),
      created_unix_ms: now_ms() - age_s * 1000,
      last_used_unix_ms: now_ms() - age_s * 1000,
      pinned: false,
      tags: vec![],
      total_size: bytes.len() as u64,
    },
    image: Some(bytes),
  }
}

fn gradient_png(w: u32, h: u32) -> Vec<u8> {
  let img = image::RgbaImage::from_fn(w, h, |x, y| {
    let r = (x * 255 / w) as u8;
    let g = (y * 255 / h) as u8;
    let b = if (x / 20 + y / 20) % 2 == 0 { 220 } else { 60 };
    image::Rgba([r, g, b, 255])
  });
  let mut out = Vec::new();
  image::DynamicImage::ImageRgba8(img)
    .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
    .unwrap();
  out
}

/// PNG whose header claims 60000×60000 (decompression-bomb shape).
fn bomb_png() -> Vec<u8> {
  let mut p = gradient_png(1, 1);
  // IHDR data starts at byte 16; patch width/height and fix the CRC.
  p[16..20].copy_from_slice(&60_000u32.to_be_bytes());
  p[20..24].copy_from_slice(&60_000u32.to_be_bytes());
  let crc = {
    let data = &p[12..29];
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
      c ^= b as u32;
      for _ in 0..8 {
        c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
      }
    }
    !c
  };
  p[29..33].copy_from_slice(&crc.to_be_bytes());
  p
}

fn fake_items() -> Vec<Item> {
  let mut v = vec![
    text_item(1, 3, "Hello from the Spool mock daemon"),
    text_item(
      2,
      40,
      &"Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. "
        .repeat(30),
    ),
    text_item(3, 70, "invoice\u{202E}fdp.exe"),
    text_item(4, 130, "admin\u{2066}\u{202E}gnp.exe\u{2069} \u{200B}hidden\u{200D}joiner\u{FEFF}"),
    image_item(5, 200, "image/png", gradient_png(320, 200)),
    text_item(6, 400, "line one\n\tline two\r\nline three\u{1b}[31m red\u{1b}[0m"),
    text_item(7, 900, "שלום עולם — مرحبا بالعالم — mixed RTL text"),
    image_item(8, 1800, "image/png", bomb_png()),
    text_item(9, 4000, "pass\u{200B}word\u{E0041}\u{E0042} (tag chars)"),
    text_item(10, 9000, "Ünïcödé façade — 東京 — emoji 😀"),
  ];
  v[0].preview.pinned = true;
  if let Ok(j) = {
    let img = image::RgbImage::from_fn(200, 300, |x, y| {
      image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    });
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img)
      .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg)
      .map(|_| out)
  } {
    v.insert(6, image_item(11, 600, "image/jpeg", j));
  }
  for n in 0..140 {
    v.push(text_item(100 + n, 20_000 + n as u64 * 3600, &format!("Filler item #{n}")));
  }
  v
}

#[derive(Default)]
struct Timings {
  lines: Vec<(String, u64, u64, u64)>, // kind, t_evt, t_present, delta_us
}

/// A Show presentation (the first one of the process is reported as "cold").
fn is_show(kind: &str) -> bool {
  kind == "show" || kind == "cold"
}

fn pct(v: &mut [u64], p: f64) -> u64 {
  if v.is_empty() {
    return 0;
  }
  v.sort_unstable();
  let i = ((v.len() as f64 - 1.0) * p).round() as usize;
  v[i]
}

fn summarize(t: &Timings, kind: &str) -> String {
  let mut v: Vec<u64> = t.lines.iter().filter(|l| l.0 == kind).map(|l| l.3).collect();
  if v.is_empty() {
    return format!("{kind}: no samples");
  }
  let n = v.len();
  let p50 = pct(&mut v, 0.5);
  let p99 = pct(&mut v, 0.99);
  let max = *v.iter().max().unwrap();
  let min = *v.iter().min().unwrap();
  format!(
    "{kind}: n={n} min={:.2}ms p50={:.2}ms p99={:.2}ms max={:.2}ms",
    min as f64 / 1000.0,
    p50 as f64 / 1000.0,
    p99 as f64 / 1000.0,
    max as f64 / 1000.0
  )
}

fn main() {
  let args: Vec<String> = std::env::args().collect();
  let show_on_start = args.iter().any(|a| a == "--show");
  let picker = std::env::var("SPOOL_PICKER_BIN").unwrap_or_else(|_| {
    let exe = std::env::current_exe().unwrap();
    // target/<profile>/examples/mock_daemon -> target/<profile>/spool-picker
    exe.parent().unwrap().parent().unwrap().join("spool-picker").to_string_lossy().into_owned()
  });

  let (ours, theirs) = UnixStream::pair().expect("socketpair");
  let theirs_fd: OwnedFd = theirs.into();
  let raw = theirs_fd.as_raw_fd();
  let mut cmd = Command::new(&picker);
  cmd.env("SPOOL_PICKER_FD", "3").stderr(Stdio::piped()).stdout(Stdio::null()).stdin(Stdio::null());
  // SAFETY: only async-signal-safe libc calls between fork and exec.
  unsafe {
    cmd.pre_exec(move || {
      if raw == 3 {
        if libc::fcntl(3, libc::F_SETFD, 0) < 0 {
          return Err(std::io::Error::last_os_error());
        }
      } else if libc::dup2(raw, 3) < 0 {
        return Err(std::io::Error::last_os_error());
      }
      Ok(())
    });
  }
  let t_spawn = mono_ns();
  let mut child = cmd.spawn().unwrap_or_else(|e| panic!("spawn {picker}: {e}"));
  drop(theirs_fd);
  eprintln!("mock: spawned {picker} pid {}", child.id());

  let timings = Arc::new((Mutex::new(Timings::default()), Condvar::new()));
  {
    let stderr = child.stderr.take().unwrap();
    let timings = timings.clone();
    std::thread::spawn(move || {
      for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        if let Some(rest) = line.strip_prefix("SPOOL_TIMING ") {
          let kv: HashMap<&str, &str> = rest.split(' ').filter_map(|p| p.split_once('=')).collect();
          let kind = kv.get("kind").copied().unwrap_or("?").to_string();
          let t_evt = kv.get("t_evt_ns").and_then(|v| v.parse().ok()).unwrap_or(0u64);
          let t_present = kv.get("t_present_ns").and_then(|v| v.parse().ok()).unwrap_or(0u64);
          let delta = kv.get("delta_us").and_then(|v| v.parse().ok()).unwrap_or(0u64);
          if kind == "cold" {
            println!(
              "COLD exec->present {:.2}ms (show->present {:.2}ms)",
              t_present.saturating_sub(t_spawn) as f64 / 1e6,
              delta as f64 / 1000.0
            );
          }
          println!("TIMING {kind} {:.2}ms", delta as f64 / 1000.0);
          let (m, cv) = &*timings;
          m.lock().unwrap().lines.push((kind, t_evt, t_present, delta));
          cv.notify_all();
        } else {
          eprintln!("picker: {line}");
        }
      }
    });
  }

  // Handshake: the picker speaks first.
  let mut r = ours.try_clone().unwrap();
  let hello: Hello = read_frame(&mut r).expect("picker Hello");
  assert!(hello.is_picker_compatible(), "picker proto {}", hello.proto);
  let w = Arc::new(Mutex::new(ours));
  write_frame(&mut *w.lock().unwrap(), &Hello::picker()).unwrap();
  println!("READY");

  let items = Arc::new(Mutex::new(fake_items()));
  let lock = Arc::new(Mutex::new(LockState {
    mode: LockMode::Unlocked,
    touch_pending: false,
    pin_asked: false,
    bad_pins: 0,
  }));
  let send_evt = {
    let w = w.clone();
    move |evt: &PickerEvt| {
      let mut g = w.lock().unwrap();
      if write_frame(&mut *g, evt).is_err() {
        std::process::exit(0);
      }
    }
  };
  // Reader -> queue; the answering thread batches what arrives within
  // REORDER_WINDOW and answers it newest first.
  let (req_tx, req_rx) = std::sync::mpsc::channel::<PickerReq>();
  std::thread::spawn(move || {
    loop {
      let req: PickerReq = match read_frame(&mut r) {
        Ok(r) => r,
        Err(e) => {
          eprintln!("mock: picker channel closed: {e}");
          std::process::exit(0);
        }
      };
      if req_tx.send(req).is_err() {
        return;
      }
    }
  });
  {
    let items = items.clone();
    let lock = lock.clone();
    let send_evt = send_evt.clone();
    let window = reorder_window();
    std::thread::spawn(move || {
      while let Ok(first) = req_rx.recv() {
        let mut batch = vec![first];
        let until = std::time::Instant::now() + window;
        while let Ok(more) =
          req_rx.recv_timeout(until.saturating_duration_since(std::time::Instant::now()))
        {
          batch.push(more);
        }
        let answered: Vec<PickerEvt> =
          batch.into_iter().filter_map(|req| answer(req, &items, &lock)).collect();
        let swaps = answered.len().saturating_sub(1);
        if swaps > 0 {
          println!("REORDER {}", answered.len());
        }
        for evt in answered.iter().rev() {
          send_evt(evt);
        }
        let _ = std::io::stdout().flush();
      }
    });
  }

  let send = |evt: PickerEvt| send_evt(&evt);
  let show = |x: Option<(i32, i32)>, out: Option<String>| PickerEvt::Show {
    cursor: x.map(|(x, y)| CursorPos { x, y }),
    output: out,
    target_window: None,
    scale_hint: None,
  };
  if show_on_start {
    send(show(None, None));
  }

  let mut next_id = 10_000;
  for line in std::io::stdin().lock().lines().map_while(Result::ok) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    match parts.as_slice() {
      ["show"] => send(show(None, None)),
      ["show", x, y] => send(show(Some((x.parse().unwrap_or(0), y.parse().unwrap_or(0))), None)),
      ["show", x, y, o] => {
        send(show(Some((x.parse().unwrap_or(0), y.parse().unwrap_or(0))), Some(o.to_string())))
      }
      ["show", x, y, o, sc] => send(PickerEvt::Show {
        cursor: Some(CursorPos { x: x.parse().unwrap_or(0), y: y.parse().unwrap_or(0) }),
        output: Some(o.to_string()),
        target_window: None,
        scale_hint: sc.parse().ok(),
      }),
      ["notice", ..] => send(PickerEvt::Error {
        seq: None,
        code: PickerErrorCode::Unavailable,
        message: line.trim_start().strip_prefix("notice").unwrap_or("").trim().to_string(),
      }),
      ["lock", m] => {
        let mode = match *m {
          "pass" => LockMode::Pass,
          "fido" => LockMode::Fido,
          "fidopin" => LockMode::FidoPin,
          "both" => LockMode::Both,
          _ => LockMode::Wait,
        };
        let mut l = lock.lock().unwrap();
        *l = LockState { mode, touch_pending: false, pin_asked: false, bad_pins: 0 };
        println!("LOCKED {mode:?}");
        send(PickerEvt::Locked { providers: mode.prompts() });
      }
      ["unlock"] => {
        lock.lock().unwrap().mode = LockMode::Unlocked;
        send(PickerEvt::Unlocked);
      }
      ["touch"] | ["timeout"] | ["nokey"] => {
        let mut l = lock.lock().unwrap();
        if !l.touch_pending {
          println!("FIDO no touch pending");
        } else {
          l.touch_pending = false;
          let evt = match parts[0] {
            "touch" if l.mode == LockMode::FidoPin && !l.pin_asked => {
              // NeedsSecret: the key wants its PIN.
              l.pin_asked = true;
              PickerEvt::Locked {
                providers: vec![UnlockPrompt::Fido2Touch, UnlockPrompt::Fido2Pin],
              }
            }
            "touch" => {
              l.mode = LockMode::Unlocked;
              PickerEvt::Unlocked
            }
            "timeout" => PickerEvt::UnlockFailed {
              provider: UnlockProvider::Fido2,
              reason: UnlockFailReason::Dismissed,
            },
            _ => PickerEvt::UnlockFailed {
              provider: UnlockProvider::Fido2,
              reason: UnlockFailReason::Unavailable,
            },
          };
          println!("FIDO {} -> {}", parts[0], evt_name(&evt));
          drop(l);
          send(evt);
        }
      }
      ["hide"] => send(PickerEvt::Hide),
      ["new", ..] => {
        next_id += 1;
        let text = line.trim_start().strip_prefix("new").unwrap_or("").trim().to_string();
        let it = text_item(next_id, 0, &text);
        let preview = it.preview.clone();
        items.lock().unwrap().insert(0, it);
        send(PickerEvt::NewItem { preview });
      }
      ["progress", d, t] => send(PickerEvt::IndexProgress {
        done: d.parse().unwrap_or(0),
        total: t.parse().unwrap_or(0),
      }),
      ["bench", n, rest @ ..] => {
        let n: usize = n.parse().unwrap_or(10);
        let cursor = match rest {
          [x, y, ..] => Some((x.parse().unwrap_or(0), y.parse().unwrap_or(0))),
          _ => None,
        };
        // Optional 3rd arg: ms between Hide and the next Show (default 60).
        let gap: u64 = rest.get(2).and_then(|g| g.parse().ok()).unwrap_or(60);
        let (m, cv) = &*timings;
        let start_count = m.lock().unwrap().lines.iter().filter(|l| is_show(&l.0)).count();
        let mut e2e = Vec::new();
        for i in 0..n {
          let t_send = mono_ns();
          send(show(cursor, None));
          let g = m.lock().unwrap();
          let want = start_count + i + 1;
          let (g, to) = cv
            .wait_timeout_while(g, Duration::from_secs(3), |t| {
              t.lines.iter().filter(|l| is_show(&l.0)).count() < want
            })
            .unwrap();
          if let Some(l) = g.lines.iter().rev().find(|l| is_show(&l.0)) {
            e2e.push(l.2.saturating_sub(t_send) / 1000);
          }
          drop(g);
          if to.timed_out() {
            println!("bench: show {i} timed out");
          }
          std::thread::sleep(Duration::from_millis(30));
          send(PickerEvt::Hide);
          // Give the picker time to re-render the hidden frame.
          std::thread::sleep(Duration::from_millis(gap));
        }
        let t = m.lock().unwrap();
        let mut v: Vec<u64> =
          t.lines.iter().filter(|l| is_show(&l.0)).skip(start_count).map(|l| l.3).collect();
        let n = v.len();
        println!(
          "BENCH show->presented n={n} p50={:.2}ms p99={:.2}ms max={:.2}ms",
          pct(&mut v, 0.5) as f64 / 1000.0,
          pct(&mut v, 0.99) as f64 / 1000.0,
          v.iter().max().copied().unwrap_or(0) as f64 / 1000.0
        );
        let n = e2e.len();
        println!(
          "BENCH daemon-send->presented n={n} p50={:.2}ms p99={:.2}ms max={:.2}ms",
          pct(&mut e2e, 0.5) as f64 / 1000.0,
          pct(&mut e2e, 0.99) as f64 / 1000.0,
          e2e.iter().max().copied().unwrap_or(0) as f64 / 1000.0
        );
      }
      ["stats"] => {
        let t = timings.0.lock().unwrap();
        for k in ["cold", "show", "key"] {
          println!("STATS {}", summarize(&t, k));
        }
      }
      ["quit"] => break,
      [] => {}
      _ => eprintln!("mock: unknown command"),
    }
    let _ = std::io::stdout().flush();
  }
  drop(w);
  let _ = child.kill();
  let _ = child.wait();
}

fn evt_name(e: &PickerEvt) -> String {
  match e {
    PickerEvt::Unlocked => "Unlocked".into(),
    PickerEvt::UnlockFailed { reason, .. } => format!("UnlockFailed({reason:?})"),
    PickerEvt::Locked { providers } => format!("Locked({providers:?})"),
    other => format!("{other:?}").chars().take(40).collect(),
  }
}

/// One request -> at most one answer. Never prints secrets or query text.
fn answer(req: PickerReq, items: &Mutex<Vec<Item>>, lock: &Mutex<LockState>) -> Option<PickerEvt> {
  let mut items = items.lock().unwrap();
  let locked = lock.lock().unwrap().mode != LockMode::Unlocked;
  match req {
    PickerReq::Ready => {
      println!("READY_PICKER");
      None
    }
    PickerReq::Hidden { reason } => {
      println!("HIDDEN reason={reason:?}");
      None
    }
    PickerReq::Query { seq, q, offset, limit, .. } => {
      if q.matches('"').count() % 2 == 1 {
        println!("QUERY seq={seq} -> BadQuery");
        return Some(PickerEvt::Error {
          seq: Some(seq),
          code: PickerErrorCode::BadQuery,
          message: "unbalanced quote".into(),
        });
      }
      let ql = q.to_lowercase();
      let matching: Vec<&Item> = items
        .iter()
        .filter(|i| !locked || is_session_item(i.preview.id))
        .filter(|i| ql.is_empty() || i.preview.preview.to_lowercase().contains(&ql))
        .collect();
      let page: Vec<ItemPreview> = matching
        .iter()
        .skip(offset as usize)
        .take(limit as usize)
        .map(|i| i.preview.clone())
        .collect();
      let more = matching.len() > offset as usize + page.len();
      println!(
        "QUERY seq={seq} q_chars={} offset={offset} limit={limit} -> {} more={more}",
        q.chars().count(),
        page.len()
      );
      Some(PickerEvt::Page { seq, offset, items: page, more })
    }
    PickerReq::Thumb { seq, id, mime } => {
      let bytes = items.iter().find(|i| i.preview.id == id).and_then(|i| i.image.clone());
      match bytes {
        Some(bytes) => {
          println!("THUMB seq={seq} id={id} mime={mime} bytes={}", bytes.len());
          Some(PickerEvt::Thumb { seq, id, bytes })
        }
        None => {
          println!("THUMB seq={seq} id={id} -> NotFound");
          Some(PickerEvt::Error {
            seq: Some(seq),
            code: PickerErrorCode::NotFound,
            message: "item is gone".into(),
          })
        }
      }
    }
    PickerReq::Select { id, mode } => {
      println!("SELECT id={id} mode={mode:?}");
      None
    }
    PickerReq::Pin { id, on } => {
      if let Some(i) = items.iter_mut().find(|i| i.preview.id == id) {
        i.preview.pinned = on;
      }
      println!("PIN id={id} on={on}");
      None
    }
    PickerReq::Delete { id } => {
      items.retain(|i| i.preview.id != id);
      println!("DELETE id={id}");
      None
    }
    PickerReq::Tag { id, on, .. } => {
      println!("TAG id={id} on={on}");
      None
    }
    PickerReq::Unlock { provider, secret } => {
      drop(items);
      let mut l = lock.lock().unwrap();
      // `secret` is never printed; only whether there was one.
      println!(
        "UNLOCK provider={provider:?} secret={}",
        if secret.is_some() { "<redacted>" } else { "none" }
      );
      if l.mode == LockMode::Unlocked {
        return Some(PickerEvt::Unlocked);
      }
      let fail = |reason| Some(PickerEvt::UnlockFailed { provider, reason });
      match (provider, secret) {
        (UnlockProvider::Passphrase, Some(s)) => {
          if matches!(l.mode, LockMode::Pass | LockMode::Both) && s.expose() == PASSPHRASE {
            l.mode = LockMode::Unlocked;
            Some(PickerEvt::Unlocked)
          } else {
            fail(UnlockFailReason::WrongSecret)
          }
        }
        (UnlockProvider::Fido2, None) => {
          if l.touch_pending {
            println!("FIDO already waiting (ignored)");
            None
          } else {
            l.touch_pending = true;
            println!("FIDO WAITING");
            None
          }
        }
        (UnlockProvider::Fido2, Some(pin)) => {
          if l.bad_pins >= 3 {
            fail(UnlockFailReason::PinBlocked)
          } else if pin.expose() == PIN {
            // The real flow waits for a touch here; the fake succeeds.
            l.mode = LockMode::Unlocked;
            Some(PickerEvt::Unlocked)
          } else {
            l.bad_pins += 1;
            if l.bad_pins >= 3 {
              fail(UnlockFailReason::PinBlocked)
            } else {
              fail(UnlockFailReason::PinInvalid)
            }
          }
        }
        _ => fail(UnlockFailReason::Other),
      }
    }
  }
}
