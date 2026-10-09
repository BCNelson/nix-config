//! M7 end-to-end tests against a **private headless sway** (wlroots):
//! virtual-keyboard paste into `wev` and foreign-toplevel focus tracking.
//! Gated on `SPOOL_WAYLAND_TESTS=1`; run from the dev shell (needs `sway`,
//! `swaymsg`, `wev`, `foot`, `stdbuf` on PATH):
//!
//! ```sh
//! SPOOL_WAYLAND_TESTS=1 cargo test -p spool-paste --test headless_sway
//! ```
//!
//! Isolation: each test starts its own sway with a cleared environment, a
//! fresh 0700 `XDG_RUNTIME_DIR` and `HOME` under `/tmp`, and connects only
//! to that sway's socket by absolute path. The user's session (Wayland
//! display, clipboard, D-Bus) is never contacted.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spool_compositor::{CompositorEvent, EventSink};
use spool_paste::{PasteBackend, PasteChord, PasteConfig, PasteHandle, Paster, ToplevelTracker};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

const TIMEOUT: Duration = Duration::from_secs(15);

fn enabled() -> bool {
  if std::env::var("SPOOL_WAYLAND_TESTS").as_deref() != Ok("1") {
    eprintln!("skipping: set SPOOL_WAYLAND_TESTS=1 (dev shell) to run headless-sway tests");
    return false;
  }
  true
}

fn which(bin: &str) -> PathBuf {
  std::env::var_os("PATH")
    .and_then(|p| std::env::split_paths(&p).map(|d| d.join(bin)).find(|p| p.is_file()))
    .unwrap_or_else(|| panic!("{bin} not on PATH (use the dev shell)"))
}

/// Kills each child's process group on drop.
struct Group(Vec<Child>);

impl Drop for Group {
  fn drop(&mut self) {
    for c in &mut self.0 {
      let _ = Command::new("kill")
        .args(["-TERM", &format!("-{}", c.id())])
        .stderr(Stdio::null())
        .status();
    }
    std::thread::sleep(Duration::from_millis(100));
    for c in &mut self.0 {
      let _ = c.kill();
      let _ = c.wait();
    }
  }
}

/// Field order: processes die before the directory goes.
struct Sway {
  procs: Group,
  display: PathBuf,
  ipc: PathBuf,
  dir: tempfile::TempDir,
}

impl Sway {
  fn start() -> Sway {
    let dir = tempfile::Builder::new().prefix("sps").tempdir_in("/tmp").expect("tempdir");
    std::fs::set_permissions(dir.path(), std::os::unix::fs::PermissionsExt::from_mode(0o700))
      .unwrap();
    std::fs::create_dir_all(dir.path().join("home")).unwrap();
    let mut me =
      Sway { procs: Group(Vec::new()), display: PathBuf::new(), ipc: PathBuf::new(), dir };
    let mut c = me.cmd(&which("sway"));
    c.args(["-c", "/dev/null"])
      .env("WLR_BACKENDS", "headless")
      .env("WLR_LIBINPUT_NO_DEVICES", "1")
      .env("WLR_RENDERER", "pixman")
      .stdout(Stdio::null())
      .stderr(std::fs::File::create(me.runtime().join("sway.log")).unwrap());
    me.procs.0.push(c.spawn().expect("spawn sway"));
    let start = Instant::now();
    loop {
      let mut display = None;
      let mut ipc = None;
      for e in std::fs::read_dir(me.runtime()).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("wayland-") && !name.ends_with(".lock") {
          display = Some(e.path());
        } else if name.starts_with("sway-ipc.") && name.ends_with(".sock") {
          ipc = Some(e.path());
        }
      }
      if let (Some(d), Some(i)) = (display, ipc)
        && UnixStream::connect(&d).is_ok()
      {
        me.display = d;
        me.ipc = i;
        return me;
      }
      assert!(start.elapsed() < TIMEOUT, "sway did not start; log:\n{}", me.log());
      std::thread::sleep(Duration::from_millis(50));
    }
  }

  fn runtime(&self) -> &Path {
    self.dir.path()
  }

  fn log(&self) -> String {
    std::fs::read_to_string(self.runtime().join("sway.log")).unwrap_or_default()
  }

  fn display_str(&self) -> String {
    self.display.to_string_lossy().into_owned()
  }

  fn cmd(&self, bin: &Path) -> Command {
    let home = self.runtime().join("home");
    let mut c = Command::new(bin);
    c.env_clear()
      .env("PATH", std::env::var_os("PATH").unwrap_or_default())
      .env("HOME", &home)
      .env("XDG_RUNTIME_DIR", self.runtime())
      .env("XDG_CONFIG_HOME", home.join(".config"))
      .stdin(Stdio::null())
      .process_group(0);
    for k in ["XDG_DATA_DIRS", "FONTCONFIG_FILE", "LOCALE_ARCHIVE"] {
      if let Some(v) = std::env::var_os(k) {
        c.env(k, v);
      }
    }
    if !self.display.as_os_str().is_empty() {
      c.env("WAYLAND_DISPLAY", &self.display).env("SWAYSOCK", &self.ipc);
    }
    c
  }

  fn swaymsg(&self, args: &[&str]) -> String {
    let out = self.cmd(&which("swaymsg")).args(["-s"]).arg(&self.ipc).args(args).output().unwrap();
    assert!(out.status.success(), "swaymsg {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
  }

  /// `wev` with line-buffered output.
  fn spawn_wev(&mut self) -> Arc<Mutex<Vec<String>>> {
    let mut c = self.cmd(&which("stdbuf"));
    c.args(["-oL", which("wev").to_str().unwrap(), "-f", "wl_keyboard:key"])
      .stdout(Stdio::piped())
      .stderr(Stdio::null());
    let mut child = c.spawn().expect("spawn wev");
    let out = child.stdout.take().unwrap();
    self.procs.0.push(child);
    let lines = Arc::new(Mutex::new(Vec::new()));
    let l2 = lines.clone();
    std::thread::spawn(move || {
      for line in BufReader::new(out).lines().map_while(Result::ok) {
        l2.lock().unwrap().push(line);
      }
    });
    lines
  }

  fn spawn_foot(&mut self, app_id: &str) -> usize {
    let mut c = self.cmd(&which("foot"));
    c.args(["--app-id", app_id, "sleep", "600"]).stdout(Stdio::null()).stderr(Stdio::null());
    self.procs.0.push(c.spawn().expect("spawn foot"));
    self.procs.0.len() - 1
  }

  fn kill_proc(&mut self, i: usize) {
    let c = &mut self.procs.0[i];
    let _ =
      Command::new("kill").args(["-TERM", &format!("-{}", c.id())]).stderr(Stdio::null()).status();
    let _ = c.wait();
  }
}

/// `(xkb keycode, pressed, keysym name)` that wev logged. Format:
/// `[14:     wl_keyboard] key: serial: 5; time: 0; key: 37; state: 1 (pressed)`
/// followed by `                       sym: Control_L    (65507), utf8: ''`.
/// wev may bind several `wl_keyboard`s; dedupe on the serial.
fn wev_keys(lines: &[String]) -> Vec<(u32, bool, String)> {
  let mut seen = std::collections::HashSet::new();
  let mut out = Vec::new();
  for (i, l) in lines.iter().enumerate() {
    let Some(p) = l.find("serial: ") else { continue };
    let Some(serial) = l[p + 8..].split(';').next().and_then(|s| s.trim().parse::<u32>().ok())
    else {
      continue;
    };
    let Some(k) = l.rfind("key: ") else { continue };
    let rest = &l[k + 5..];
    let Some((code, rest)) = rest.split_once(';') else { continue };
    let Some(state) = rest.trim().strip_prefix("state: ") else { continue };
    let sym = lines
      .get(i + 1)
      .and_then(|n| n.trim().strip_prefix("sym: "))
      .and_then(|s| s.split_whitespace().next())
      .unwrap_or("")
      .to_owned();
    if seen.insert(serial) {
      out.push((code.trim().parse().unwrap(), state.starts_with('1'), sym));
    }
  }
  out
}

fn wait_keys(lines: &Arc<Mutex<Vec<String>>>, n: usize) -> Vec<(u32, bool, String)> {
  let start = Instant::now();
  loop {
    let keys = wev_keys(&lines.lock().unwrap());
    // Wait for the `sym:` line that follows the last key line, too.
    let complete = keys.len() >= n && keys.iter().all(|k| !k.2.is_empty());
    if complete || start.elapsed() > Duration::from_secs(5) {
      return keys;
    }
    std::thread::sleep(Duration::from_millis(50));
  }
}

fn k(code: u32, pressed: bool, sym: &str) -> (u32, bool, String) {
  (code, pressed, sym.to_owned())
}

/// Follows focus with the foreign-toplevel tracker into an EventSink.
fn track(
  sway: &Sway,
) -> (ToplevelTracker, EventSink, tokio::sync::mpsc::Receiver<CompositorEvent>) {
  let (sink, rx) = EventSink::new();
  let s2 = sink.clone();
  let t =
    ToplevelTracker::spawn(Some(sway.display_str()), move |app, win| s2.active_window(app, win))
      .expect("foreign toplevel manager");
  (t, sink, rx)
}

async fn next_active(
  rx: &mut tokio::sync::mpsc::Receiver<CompositorEvent>,
  pred: impl Fn(Option<&str>, Option<&str>) -> bool,
) -> (Option<String>, Option<String>) {
  let deadline = tokio::time::Instant::now() + TIMEOUT;
  loop {
    let ev = tokio::time::timeout_at(deadline, rx.recv())
      .await
      .expect("timed out waiting for focus change")
      .expect("closed");
    if let CompositorEvent::ActiveWindow { app_id, window_id, .. } = ev
      && pred(app_id.as_deref(), window_id.as_deref())
    {
      return (app_id, window_id);
    }
  }
}

fn xkb(evdev: u32) -> u32 {
  evdev + 8
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn virtual_keyboard_paste_reaches_focused_window() {
  if !enabled() {
    return;
  }
  let mut sway = Sway::start();
  // The virtual keyboard comes first: it gives the seat its keyboard
  // capability, so wev binds a wl_keyboard when it starts.
  let paste =
    PasteHandle::spawn(PasteConfig { display: Some(sway.display_str()), ..Default::default() })
      .expect("virtual keyboard");
  assert_eq!(paste.backend(), PasteBackend::VirtualKeyboard);
  assert_eq!(paste.keycodes(None).await.unwrap().v, 47, "default keymap is US");
  let (_t, sink, mut rx) = track(&sway);
  let wev = sway.spawn_wev();
  let (_, win) = next_active(&mut rx, |a, _| a == Some("wev")).await;
  let win = win.unwrap();
  assert!(win.starts_with("wlr-"));
  assert!(sink.tracker().wait_for(&win, Duration::from_millis(500)).await);
  tokio::time::sleep(Duration::from_millis(200)).await; // wev's keyboard bound

  paste.paste(PasteChord::CtrlV, None).await.expect("paste");
  let keys = wait_keys(&wev, 4);
  assert_eq!(
    keys,
    vec![
      k(xkb(29), true, "Control_L"),
      k(xkb(47), true, "v"),
      k(xkb(47), false, "v"),
      k(xkb(29), false, "Control_L")
    ],
    "wev saw {keys:?}\n{}",
    wev.lock().unwrap().join("\n")
  );
  wev.lock().unwrap().clear();
  paste.paste(PasteChord::CtrlShiftV, None).await.expect("paste");
  let keys = wait_keys(&wev, 6);
  // With Shift held, level 2 of the v key is V.
  assert_eq!(
    keys,
    vec![
      k(xkb(29), true, "Control_L"),
      k(xkb(42), true, "Shift_L"),
      k(xkb(47), true, "V"),
      k(xkb(47), false, "V"),
      k(xkb(42), false, "Shift_L"),
      k(xkb(29), false, "Control_L")
    ],
    "wev saw {keys:?}"
  );
}

/// A "physical" keyboard stand-in: a virtual keyboard with a dvorak keymap
/// that becomes the seat's keyboard before spool connects.
struct DvorakKeyboard {
  _conn: Connection,
  _queue: EventQueue<Nop>,
  _kb: ZwpVirtualKeyboardV1,
}

struct Nop;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Nop {
  fn event(
    _: &mut Self,
    _: &wl_registry::WlRegistry,
    _: wl_registry::Event,
    _: &GlobalListContents,
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
  }
}
wayland_client::delegate_noop!(Nop: ignore wl_seat::WlSeat);
wayland_client::delegate_noop!(Nop: ignore ZwpVirtualKeyboardManagerV1);
wayland_client::delegate_noop!(Nop: ignore ZwpVirtualKeyboardV1);

impl DvorakKeyboard {
  fn create(display: &Path) -> Self {
    use std::io::Write;
    use std::os::fd::AsFd;
    let conn = Connection::from_socket(UnixStream::connect(display).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<Nop>(&conn).unwrap();
    let qh = queue.handle();
    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let mgr: ZwpVirtualKeyboardManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
    let kb = mgr.create_virtual_keyboard(&seat, &qh, ());
    let map = spool_paste::keys::keymap_from_names("us", "dvorak", "").expect("dvorak keymap");
    let text = map.get_as_string(xkbcommon::xkb::KEYMAP_FORMAT_TEXT_V1);
    let mut f = tempfile::tempfile().unwrap();
    f.write_all(text.as_bytes()).unwrap();
    f.write_all(&[0]).unwrap();
    kb.keymap(1, f.as_fd(), (text.len() + 1) as u32);
    queue.roundtrip(&mut Nop).unwrap();
    Self { _conn: conn, _queue: queue, _kb: kb }
  }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn virtual_keyboard_uses_seat_keymap_dvorak() {
  if !enabled() {
    return;
  }
  let mut sway = Sway::start();
  let _dvorak = DvorakKeyboard::create(&sway.display);
  let mut p =
    Paster::connect(&PasteConfig { display: Some(sway.display_str()), ..Default::default() })
      .expect("virtual keyboard");
  assert_eq!(p.backend(), PasteBackend::VirtualKeyboard);
  assert_eq!(p.keycodes(None).v, 52, "Dvorak 'v' is on the QWERTY '.' key (evdev 52)");
  let (_t, sink, mut rx) = track(&sway);
  let wev = sway.spawn_wev();
  let (_, win) = next_active(&mut rx, |a, _| a == Some("wev")).await;
  assert!(sink.tracker().wait_for(&win.unwrap(), Duration::from_millis(500)).await);
  tokio::time::sleep(Duration::from_millis(200)).await;
  tokio::task::block_in_place(|| p.paste(PasteChord::CtrlV, None)).expect("paste");
  let keys = wait_keys(&wev, 4);
  assert_eq!(
    keys,
    vec![
      k(xkb(29), true, "Control_L"),
      k(xkb(52), true, "v"),
      k(xkb(52), false, "v"),
      k(xkb(29), false, "Control_L")
    ],
    "wev saw {keys:?}"
  );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreign_toplevel_tracks_focus_between_two_windows() {
  if !enabled() {
    return;
  }
  let mut sway = Sway::start();
  let (tracker, sink, mut rx) = track(&sway);
  assert!(tracker.is_running());
  // Empty desktop: the initial report is "nothing active".
  assert_eq!(next_active(&mut rx, |_, _| true).await, (None, None));

  let a = sway.spawn_foot("spool-a");
  let (_, wa) = next_active(&mut rx, |a, _| a == Some("spool-a")).await;
  let _b = sway.spawn_foot("spool-b");
  let (_, wb) = next_active(&mut rx, |a, _| a == Some("spool-b")).await;
  assert_ne!(wa, wb);
  let t = sink.tracker();
  assert!(t.is_active(wb.as_deref().unwrap()));

  sway.swaymsg(&["[app_id=\"spool-a\"]", "focus"]);
  let (_, again) = next_active(&mut rx, |a, _| a == Some("spool-a")).await;
  assert_eq!(again, wa, "window id is stable for the window's lifetime");
  assert!(t.wait_for(wa.as_deref().unwrap(), Duration::from_millis(500)).await);
  assert_eq!(t.current_app_id().as_deref(), Some("spool-a"));

  // Closing the focused window moves focus to the other one (sway may
  // report "nothing active" in between).
  sway.kill_proc(a);
  let (_, win) = next_active(&mut rx, |a, _| a == Some("spool-b")).await;
  assert_eq!(win, wb);
  assert!(t.is_active(wb.as_deref().unwrap()));

  drop(tracker); // stops promptly
}

#[test]
fn unsupported_without_protocols() {
  // No compositor needed: a dead socket path is a connect error, not a
  // panic, for both entry points.
  let bogus = "/tmp/spool-no-such-socket".to_owned();
  assert!(matches!(
    Paster::connect(&PasteConfig { display: Some(bogus.clone()), ..Default::default() }),
    Err(spool_paste::PasteError::Connect(_))
  ));
  assert!(matches!(
    ToplevelTracker::spawn(Some(bogus), |_, _| {}),
    Err(spool_paste::PasteError::Connect(_))
  ));
}
