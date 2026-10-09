//! Integration tests against a private, nested headless sway.
//!
//! These never touch the user's session: each test starts its own sway with
//! a fresh `XDG_RUNTIME_DIR` (mode 0700) and connects by absolute socket
//! path; `wl-copy`/`wl-paste` get that runtime dir and display explicitly,
//! with the inherited `WAYLAND_DISPLAY`/`DISPLAY` removed.
//!
//! They only run when `SPOOL_WAYLAND_TESTS=1` is set (and
//! `SPOOL_SKIP_WAYLAND_TESTS` is not), and skip with a message if `sway`,
//! `wl-copy` or `wl-paste` are missing, so `nix build .#spool` (sandbox, no
//! compositor) passes. Run them from the dev shell:
//!
//! ```sh
//! SPOOL_WAYLAND_TESTS=1 cargo test -p spool-wayland --test headless_sway
//! ```

use std::io::Write;
use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use spool_wayland::{
  FetchError, OfferToken, Selection, WaylandConfig, WaylandError, WaylandEvent, WaylandHandle,
  is_marker_mime, spawn,
};

const T: Duration = Duration::from_secs(10);
static OUT_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

// ---- harness ----------------------------------------------------------------

fn on_path(bin: &str) -> bool {
  std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

struct Sway {
  dir: tempfile::TempDir,
  socket: PathBuf,
  child: Child,
}

impl Sway {
  fn start() -> Option<Sway> {
    if std::env::var_os("SPOOL_SKIP_WAYLAND_TESTS").is_some()
      || std::env::var("SPOOL_WAYLAND_TESTS").as_deref() != Ok("1")
    {
      eprintln!("skipping: set SPOOL_WAYLAND_TESTS=1 to run headless-sway tests");
      return None;
    }
    for bin in ["sway", "wl-copy", "wl-paste"] {
      if !on_path(bin) {
        eprintln!("skipping: `{bin}` not on PATH (use the spool dev shell)");
        return None;
      }
    }
    let dir = tempfile::Builder::new().prefix("spool-wl-test").tempdir().unwrap();
    {
      use std::os::unix::fs::PermissionsExt;
      std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let log = std::fs::File::create(dir.path().join("sway.log")).unwrap();
    let child = Command::new("sway")
      .args(["-c", "/dev/null"])
      .env_remove("WAYLAND_DISPLAY")
      .env_remove("WAYLAND_SOCKET")
      .env_remove("DISPLAY")
      .env_remove("SWAYSOCK")
      .env_remove("I3SOCK")
      .env("XDG_RUNTIME_DIR", dir.path())
      .env("WLR_BACKENDS", "headless")
      .env("WLR_LIBINPUT_NO_DEVICES", "1")
      .env("WLR_RENDERER", "pixman")
      // nixpkgs' `sway` is a `dbus-run-session` wrapper; a process group of
      // its own lets kill() take dbus-daemon, sway and swaybg down too.
      .process_group(0)
      .stdin(Stdio::null())
      .stdout(log.try_clone().unwrap())
      .stderr(log)
      .spawn()
      .expect("spawn sway");
    let mut sway = Sway { dir, socket: PathBuf::new(), child };
    let start = Instant::now();
    loop {
      if let Some(s) = sway.find_socket() {
        sway.socket = s;
        break;
      }
      if let Ok(Some(st)) = sway.child.try_wait() {
        panic!("sway exited early ({st}); log:\n{}", sway.log());
      }
      assert!(start.elapsed() < T, "sway socket never appeared; log:\n{}", sway.log());
      std::thread::sleep(Duration::from_millis(20));
    }
    // The socket file exists before sway accepts connections; give it a beat.
    std::thread::sleep(Duration::from_millis(300));
    Some(sway)
  }

  fn find_socket(&self) -> Option<PathBuf> {
    std::fs::read_dir(self.dir.path()).ok()?.flatten().map(|e| e.path()).find(|p| {
      let n = p.file_name().unwrap().to_string_lossy();
      n.starts_with("wayland-") && !n.ends_with(".lock")
    })
  }

  fn log(&self) -> String {
    std::fs::read_to_string(self.dir.path().join("sway.log")).unwrap_or_default()
  }

  fn cfg(&self, watch_primary: bool) -> WaylandConfig {
    WaylandConfig { display: Some(self.socket.to_string_lossy().into_owned()), watch_primary }
  }

  fn cmd(&self, bin: &str) -> Command {
    let mut c = Command::new(bin);
    // Join sway's group: backgrounded wl-copy servers outlive the
    // compositor otherwise.
    c.process_group(self.child.id() as i32)
      .env_remove("DISPLAY")
      .env_remove("WAYLAND_SOCKET")
      .env("XDG_RUNTIME_DIR", self.dir.path())
      .env("WAYLAND_DISPLAY", self.socket.file_name().unwrap());
    c
  }

  /// File in the private runtime dir for a child's output. Children get
  /// files, not pipes: wl-copy forks a background server that inherits its
  /// stdio, so a pipe would never hit EOF.
  fn out_file(&self, tag: &str) -> (PathBuf, std::fs::File) {
    let n = OUT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let p = self.dir.path().join(format!("{tag}-{n}.out"));
    let f = std::fs::File::create(&p).unwrap();
    (p, f)
  }

  /// `wl-copy ARGS` with `data` on stdin; waits for the (forking) wl-copy
  /// parent to exit.
  fn wl_copy(&self, args: &[&str], data: &[u8]) {
    let (errp, errf) = self.out_file("wl-copy-err");
    let mut c = self.cmd("wl-copy");
    c.args(args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(errf);
    let mut child = c.spawn().unwrap();
    child.stdin.take().unwrap().write_all(data).unwrap();
    let st = wait_timeout(child, T);
    assert!(st.success(), "wl-copy failed: {}", std::fs::read_to_string(errp).unwrap_or_default());
  }

  /// `wl-copy --foreground ARGS`; returns the running child.
  fn wl_copy_fg(&self, args: &[&str], data: &[u8]) -> Child {
    let mut c = self.cmd("wl-copy");
    c.arg("--foreground")
      .args(args)
      .stdin(Stdio::piped())
      .stdout(Stdio::null())
      .stderr(Stdio::null());
    let mut child = c.spawn().unwrap();
    child.stdin.take().unwrap().write_all(data).unwrap();
    drop(child.stdin.take());
    child
  }

  fn wl_paste(&self, args: &[&str]) -> Vec<u8> {
    let (outp, outf) = self.out_file("wl-paste-out");
    let (errp, errf) = self.out_file("wl-paste-err");
    let mut c = self.cmd("wl-paste");
    c.args(args).stdin(Stdio::null()).stdout(outf).stderr(errf);
    let st = wait_timeout(c.spawn().unwrap(), T);
    assert!(st.success(), "wl-paste failed: {}", std::fs::read_to_string(errp).unwrap_or_default());
    std::fs::read(outp).unwrap()
  }

  fn kill(&mut self) {
    if let Some(pg) = rustix::process::Pid::from_raw(self.child.id() as i32) {
      let _ = rustix::process::kill_process_group(pg, rustix::process::Signal::KILL);
    }
    let _ = self.child.kill();
    let _ = self.child.wait();
  }
}

impl Drop for Sway {
  fn drop(&mut self) {
    self.kill();
  }
}

fn wait_timeout(mut child: Child, t: Duration) -> std::process::ExitStatus {
  let start = Instant::now();
  loop {
    if let Some(st) = child.try_wait().unwrap() {
      return st;
    }
    if start.elapsed() > t {
      let _ = child.kill();
      let _ = child.wait();
      panic!("child process timed out");
    }
    std::thread::sleep(Duration::from_millis(10));
  }
}

fn next(rx: &Receiver<WaylandEvent>, t: Duration) -> WaylandEvent {
  rx.recv_timeout(t).unwrap_or_else(|e| panic!("no wayland event within {t:?}: {e}"))
}

/// Next event matching `pred`; other events are returned in `skipped`.
fn wait_for(
  rx: &Receiver<WaylandEvent>,
  mut pred: impl FnMut(&WaylandEvent) -> bool,
) -> (WaylandEvent, Vec<WaylandEvent>) {
  let deadline = Instant::now() + T;
  let mut skipped = Vec::new();
  loop {
    let left = deadline.saturating_duration_since(Instant::now());
    let ev = next(rx, left.max(Duration::from_millis(1)));
    if pred(&ev) {
      return (ev, skipped);
    }
    skipped.push(ev);
  }
}

fn new_selection(
  rx: &Receiver<WaylandEvent>,
  sel: Selection,
) -> (Vec<String>, OfferToken, bool, Vec<WaylandEvent>) {
  let (ev, skipped) = wait_for(
    rx,
    |e| matches!(e, WaylandEvent::NewSelection { selection, .. } if *selection == sel),
  );
  let WaylandEvent::NewSelection { mimes, offer, ours, .. } = ev else { unreachable!() };
  (mimes, offer, ours, skipped)
}

fn fetched(rx: &Receiver<WaylandEvent>, tok: OfferToken) -> spool_wayland::FetchResult {
  let (ev, _) = wait_for(rx, |e| matches!(e, WaylandEvent::Fetched { offer, .. } if *offer == tok));
  let WaylandEvent::Fetched { result, .. } = ev else { unreachable!() };
  result
}

fn start(sway: &Sway, primary: bool) -> (WaylandHandle, Receiver<WaylandEvent>) {
  let (h, rx) = spawn(sway.cfg(primary)).expect("spawn spool-wayland");
  match next(&rx, T) {
    WaylandEvent::Ready(info) => {
      assert_eq!(info.data_control_version, 1);
      assert!(info.display.ends_with(sway.socket.file_name().unwrap().to_str().unwrap()));
    }
    other => panic!("first event not Ready: {other:?}"),
  }
  (h, rx)
}

const UTF8: &str = "text/plain;charset=utf-8";

// ---- tests ------------------------------------------------------------------

#[test]
fn wl_copy_text_is_seen_and_fetched() {
  let Some(sway) = Sway::start() else { return };
  let (h, rx) = start(&sway, false);
  sway.wl_copy(&[], b"hello spool");
  let (mimes, tok, ours, _) = new_selection(&rx, Selection::Clipboard);
  assert!(!ours);
  assert!(mimes.iter().any(|m| m == UTF8), "mimes: {mimes:?}");
  assert!(mimes.iter().any(|m| m == "text/plain"), "mimes: {mimes:?}");
  assert!(!mimes.iter().any(|m| is_marker_mime(m)));

  h.fetch(tok, vec![UTF8.into(), "text/plain".into()], 1 << 20, 4 << 20, T).unwrap();
  let reps = fetched(&rx, tok).expect("fetch ok");
  assert_eq!(
    reps,
    vec![
      (UTF8.to_string(), b"hello spool".to_vec()),
      ("text/plain".into(), b"hello spool".to_vec())
    ]
  );
  h.shutdown();
}

#[test]
fn large_payload_hits_caps() {
  let Some(sway) = Sway::start() else { return };
  let (h, rx) = start(&sway, false);
  let big = vec![b'x'; 3 << 20];
  sway.wl_copy(&["--type", "application/octet-stream"], &big);
  let (mimes, tok, _, _) = new_selection(&rx, Selection::Clipboard);
  assert_eq!(mimes, vec!["application/octet-stream".to_string()]);

  // Over the per-rep cap: omitted, not an error.
  h.fetch(tok, mimes.clone(), 1 << 20, 64 << 20, T).unwrap();
  assert_eq!(fetched(&rx, tok), Ok(vec![]));
  // Over the total cap: TooLarge.
  h.fetch(tok, mimes.clone(), 64 << 20, 1 << 20, T).unwrap();
  assert_eq!(fetched(&rx, tok), Err(FetchError::TooLarge));
  // Under both: the full payload.
  h.fetch(tok, mimes, 4 << 20, 4 << 20, T).unwrap();
  let reps = fetched(&rx, tok).unwrap();
  assert_eq!(reps[0].1.len(), big.len());
}

#[test]
fn never_closing_source_times_out_without_blocking() {
  let Some(sway) = Sway::start() else { return };
  let (h, rx) = start(&sway, false);
  let _evil = evil::Evil::start(&sway.socket, "text/plain");
  let (mimes, tok, ours, _) = new_selection(&rx, Selection::Clipboard);
  assert_eq!(mimes, vec!["text/plain".to_string()]);
  assert!(!ours);

  let started = Instant::now();
  h.fetch(tok, mimes, 1 << 20, 1 << 20, Duration::from_millis(1500)).unwrap();
  // While the fetch hangs on the evil source, the loop keeps going.
  sway.wl_copy(&[], b"next");
  let (_, tok2, _, skipped) = new_selection(&rx, Selection::Clipboard);
  assert!(
    !skipped.iter().any(|e| matches!(e, WaylandEvent::Fetched { .. })),
    "new selection should arrive before the timeout"
  );
  assert!(started.elapsed() < Duration::from_millis(1500));
  assert_eq!(fetched(&rx, tok), Err(FetchError::Timeout));
  assert!(started.elapsed() < Duration::from_secs(5));

  // The old token is now stale.
  h.fetch(tok, vec!["text/plain".into()], 1, 1, T).unwrap();
  assert_eq!(fetched(&rx, tok), Err(FetchError::OfferGone));
  // And the new one still works.
  h.fetch(tok2, vec![UTF8.into()], 1 << 20, 1 << 20, T).unwrap();
  assert_eq!(fetched(&rx, tok2).unwrap()[0].1, b"next");
}

#[test]
fn set_selection_serves_wl_paste_and_is_ours() {
  let Some(sway) = Sway::start() else { return };
  let (h, rx) = start(&sway, false);
  let data: Arc<[u8]> = Arc::from(&b"from spool"[..]);
  h.set_selection(
    Selection::Clipboard,
    vec![(UTF8.into(), data.clone()), ("text/plain".into(), data.clone())],
  )
  .unwrap();
  let (mimes, tok, ours, _) = new_selection(&rx, Selection::Clipboard);
  assert!(ours, "our own selection must come back with ours = true");
  assert_eq!(mimes, vec![UTF8.to_string(), "text/plain".to_string()]);

  let types = String::from_utf8(sway.wl_paste(&["--list-types"])).unwrap();
  let markers: Vec<&str> = types.lines().filter(|l| is_marker_mime(l)).collect();
  assert_eq!(markers.len(), 1, "types: {types}");
  let first_marker = markers[0].to_string();
  assert_eq!(sway.wl_paste(&["--no-newline", "--type", UTF8]), b"from spool");
  assert_eq!(sway.wl_paste(&["--no-newline", "--type", &first_marker]), b"");

  // We can fetch from our own offer too (served by our own source).
  h.fetch(tok, vec!["text/plain".into()], 1 << 20, 1 << 20, T).unwrap();
  assert_eq!(fetched(&rx, tok).unwrap()[0].1, b"from spool");

  // Nonce rotates per set_selection.
  h.set_selection(Selection::Clipboard, vec![(UTF8.into(), Arc::from(&b"two"[..]))]).unwrap();
  let (_, _, ours, _) = new_selection(&rx, Selection::Clipboard);
  assert!(ours);
  let types = String::from_utf8(sway.wl_paste(&["--list-types"])).unwrap();
  let marker2 = types.lines().find(|l| is_marker_mime(l)).unwrap();
  assert_ne!(marker2, first_marker);
  assert_eq!(sway.wl_paste(&["--no-newline"]), b"two");

  // Someone else copies: not ours.
  sway.wl_copy(&[], b"other");
  let (_, _, ours, _) = new_selection(&rx, Selection::Clipboard);
  assert!(!ours);

  // Empty reps clear the selection.
  h.set_selection(Selection::Clipboard, vec![]).unwrap();
  let (ev, _) = wait_for(&rx, |e| !matches!(e, WaylandEvent::Fetched { .. }));
  assert!(
    matches!(ev, WaylandEvent::SelectionCleared { selection: Selection::Clipboard }),
    "{ev:?}"
  );
}

#[test]
fn large_set_selection_round_trips() {
  let Some(sway) = Sway::start() else { return };
  let (h, rx) = start(&sway, false);
  let data: Vec<u8> = (0..(2u32 << 20)).map(|i| (i % 251) as u8).collect();
  h.set_selection(
    Selection::Clipboard,
    vec![("application/octet-stream".into(), data.clone().into())],
  )
  .unwrap();
  let _ = new_selection(&rx, Selection::Clipboard);
  assert_eq!(sway.wl_paste(&["--type", "application/octet-stream"]), data);
}

#[test]
fn selection_cleared_behaviour_with_wl_copy() {
  let Some(sway) = Sway::start() else { return };
  let (_h, rx) = start(&sway, false);

  // Killing the source client (foreground wl-copy) makes the compositor
  // send a null selection.
  let mut child = sway.wl_copy_fg(&[], b"short-lived");
  let _ = new_selection(&rx, Selection::Clipboard);
  child.kill().unwrap();
  child.wait().unwrap();
  let (ev, skipped) = wait_for(&rx, |e| !matches!(e, WaylandEvent::Fetched { .. }));
  assert!(skipped.is_empty());
  assert!(
    matches!(ev, WaylandEvent::SelectionCleared { selection: Selection::Clipboard }),
    "got {ev:?}"
  );

  // `wl-copy --clear` also yields a null selection.
  sway.wl_copy(&[], b"again");
  let _ = new_selection(&rx, Selection::Clipboard);
  sway.wl_copy(&["--clear"], b"");
  let (ev, _) = wait_for(&rx, |_| true);
  assert!(
    matches!(ev, WaylandEvent::SelectionCleared { selection: Selection::Clipboard }),
    "got {ev:?}"
  );
}

#[test]
fn primary_only_when_enabled() {
  let Some(sway) = Sway::start() else { return };
  // Disabled: primary changes are invisible.
  {
    let (h, rx) = start(&sway, false);
    sway.wl_copy(&["--primary"], b"prim");
    sway.wl_copy(&[], b"barrier");
    let (_, _, _, skipped) = new_selection(&rx, Selection::Clipboard);
    assert!(
      !skipped.iter().any(|e| matches!(
        e,
        WaylandEvent::NewSelection { selection: Selection::Primary, .. }
          | WaylandEvent::SelectionCleared { selection: Selection::Primary }
      )),
      "primary events leaked: {skipped:?}"
    );
    // set_selection(Primary) is a no-op.
    h.set_selection(Selection::Primary, vec![(UTF8.into(), Arc::from(&b"p"[..]))]).unwrap();
    assert_eq!(sway.wl_paste(&["--primary", "--no-newline"]), b"prim");
    h.shutdown();
  }
  // Enabled.
  let (h, rx) = start(&sway, true);
  // The compositor replays the current primary selection on bind.
  let (_, tok0, _, _) = new_selection(&rx, Selection::Primary);
  h.fetch(tok0, vec![UTF8.into()], 1 << 20, 1 << 20, T).unwrap();
  assert_eq!(fetched(&rx, tok0).unwrap()[0].1, b"prim");
  sway.wl_copy(&["--primary"], b"prim2");
  let (mimes, tok, ours, _) = new_selection(&rx, Selection::Primary);
  assert!(!ours);
  assert!(mimes.iter().any(|m| m == UTF8));
  h.fetch(tok, vec![UTF8.into()], 1 << 20, 1 << 20, T).unwrap();
  assert_eq!(fetched(&rx, tok).unwrap()[0].1, b"prim2");

  h.set_selection(Selection::Primary, vec![(UTF8.into(), Arc::from(&b"ours-p"[..]))]).unwrap();
  let (_, _, ours, _) = new_selection(&rx, Selection::Primary);
  assert!(ours);
  assert_eq!(sway.wl_paste(&["--primary", "--no-newline"]), b"ours-p");
}

#[test]
fn shutdown_closes_channel() {
  let Some(sway) = Sway::start() else { return };
  let (h, rx) = start(&sway, false);
  h.shutdown();
  h.shutdown(); // idempotent
  let deadline = Instant::now() + T;
  loop {
    match rx.recv_deadline(deadline) {
      Ok(_) => continue,
      Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
      Err(e) => panic!("channel not closed after shutdown: {e}"),
    }
  }
  assert!(matches!(
    h.fetch(OfferToken::from_raw(1), vec![], 1, 1, T),
    Err(WaylandError::ThreadGone)
  ));
}

#[test]
fn compositor_death_is_fatal() {
  let Some(mut sway) = Sway::start() else { return };
  let (_h, rx) = start(&sway, false);
  sway.kill();
  let (ev, _) = wait_for(&rx, |e| matches!(e, WaylandEvent::Fatal(_)));
  let WaylandEvent::Fatal(msg) = ev else { unreachable!() };
  assert!(msg.contains("connection to"), "{msg}");
  assert!(rx.recv_timeout(T).is_err(), "channel closes after Fatal");
}

// ---- a hostile source: offers a mime and never writes or closes -------------

mod evil {
  use super::*;
  use std::os::unix::net::UnixStream;
  use wayland_client::protocol::{wl_registry, wl_seat};
  use wayland_client::{Connection, Dispatch, QueueHandle, event_created_child};
  use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::ExtDataControlOfferV1,
    ext_data_control_source_v1::{self, ExtDataControlSourceV1},
  };

  #[derive(Default)]
  struct St {
    globals: Vec<(u32, String, u32)>,
    held: Vec<OwnedFd>,
  }

  pub struct Evil;

  impl Evil {
    /// Takes the clipboard on its own thread; the thread ends when the
    /// compositor goes away.
    pub fn start(socket: &Path, mime: &str) -> Evil {
      let conn = Connection::from_socket(UnixStream::connect(socket).unwrap()).unwrap();
      let mut q = conn.new_event_queue::<St>();
      let qh = q.handle();
      let reg = conn.display().get_registry(&qh, ());
      let mut st = St::default();
      q.roundtrip(&mut st).unwrap();
      let find = |n: &str| st.globals.iter().find(|g| g.1 == n).cloned().unwrap();
      let (mn, ..) = find("ext_data_control_manager_v1");
      let (sn, ..) = find("wl_seat");
      let mgr: ExtDataControlManagerV1 = reg.bind(mn, 1, &qh, ());
      let seat: wl_seat::WlSeat = reg.bind(sn, 1, &qh, ());
      let dev = mgr.get_data_device(&seat, &qh, ());
      let src = mgr.create_data_source(&qh, ());
      src.offer(mime.into());
      dev.set_selection(Some(&src));
      q.roundtrip(&mut st).unwrap();
      std::thread::spawn(move || while q.blocking_dispatch(&mut st).is_ok() {});
      Evil
    }
  }

  impl Dispatch<wl_registry::WlRegistry, ()> for St {
    fn event(
      st: &mut Self,
      _: &wl_registry::WlRegistry,
      e: wl_registry::Event,
      _: &(),
      _: &Connection,
      _: &QueueHandle<Self>,
    ) {
      if let wl_registry::Event::Global { name, interface, version } = e {
        st.globals.push((name, interface, version));
      }
    }
  }
  impl Dispatch<wl_seat::WlSeat, ()> for St {
    fn event(
      _: &mut Self,
      _: &wl_seat::WlSeat,
      _: wl_seat::Event,
      _: &(),
      _: &Connection,
      _: &QueueHandle<Self>,
    ) {
    }
  }
  impl Dispatch<ExtDataControlManagerV1, ()> for St {
    fn event(
      _: &mut Self,
      _: &ExtDataControlManagerV1,
      _: <ExtDataControlManagerV1 as wayland_client::Proxy>::Event,
      _: &(),
      _: &Connection,
      _: &QueueHandle<Self>,
    ) {
    }
  }
  impl Dispatch<ExtDataControlDeviceV1, ()> for St {
    fn event(
      _: &mut Self,
      _: &ExtDataControlDeviceV1,
      _: ext_data_control_device_v1::Event,
      _: &(),
      _: &Connection,
      _: &QueueHandle<Self>,
    ) {
    }
    event_created_child!(St, ExtDataControlDeviceV1, [
      ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
  }
  impl Dispatch<ExtDataControlOfferV1, ()> for St {
    fn event(
      _: &mut Self,
      _: &ExtDataControlOfferV1,
      _: <ExtDataControlOfferV1 as wayland_client::Proxy>::Event,
      _: &(),
      _: &Connection,
      _: &QueueHandle<Self>,
    ) {
    }
  }
  impl Dispatch<ExtDataControlSourceV1, ()> for St {
    fn event(
      st: &mut Self,
      _: &ExtDataControlSourceV1,
      e: ext_data_control_source_v1::Event,
      _: &(),
      _: &Connection,
      _: &QueueHandle<Self>,
    ) {
      if let ext_data_control_source_v1::Event::Send { fd, .. } = e {
        st.held.push(fd); // never write, never close
      }
    }
  }
}
