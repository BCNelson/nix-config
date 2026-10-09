//! End-to-end tests against a **nested virtual KWin** on a **private** session
//! bus. Gated on `SPOOL_KWIN_TESTS=1`; run from the dev shell (needs
//! `kwin_wayland`, `dbus-daemon`, `wev`, `stdbuf` on PATH):
//!
//! ```sh
//! SPOOL_KWIN_TESTS=1 cargo test -p spool-paste --test nested_kwin
//! ```
//!
//! Isolation: every test starts its own `dbus-daemon` (address passed
//! explicitly; the inherited `DBUS_SESSION_BUS_ADDRESS` is never used and
//! children get a cleared environment), its own 0700 `XDG_RUNTIME_DIR`, and
//! a throwaway `HOME`/`XDG_CONFIG_HOME` (KWin writes kwinrc and
//! kglobalshortcutsrc there). The user's KWin and session bus are never
//! contacted.

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spool_kwin::{KwinEvent, KwinScript, KwinService, ServiceConfig};
use spool_paste::{PasteBackend, PasteChord, PasteConfig, PasteError, PasteHandle};
use tokio::sync::mpsc;

const SOCKET: &str = "wl-spool";
const TIMEOUT: Duration = Duration::from_secs(20);

fn enabled() -> bool {
  if std::env::var("SPOOL_KWIN_TESTS").as_deref() != Ok("1") {
    eprintln!("skipping: set SPOOL_KWIN_TESTS=1 (dev shell) to run nested-KWin tests");
    return false;
  }
  true
}

fn script_dir() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kwin-script")
}

fn which(bin: &str) -> PathBuf {
  std::env::var_os("PATH")
    .and_then(|p| std::env::split_paths(&p).map(|d| d.join(bin)).find(|p| p.is_file()))
    .unwrap_or_else(|| panic!("{bin} not on PATH (use the dev shell)"))
}

/// Kills the whole process group of each child on drop.
struct Group(Vec<Child>);

impl Drop for Group {
  fn drop(&mut self) {
    for c in &mut self.0 {
      let pgid = c.id() as i32;
      // SAFETY: plain kill(2) on a process group we created.
      unsafe {
        libc::kill(-pgid, libc::SIGTERM);
      }
    }
    std::thread::sleep(Duration::from_millis(100));
    for c in &mut self.0 {
      unsafe {
        libc::kill(-(c.id() as i32), libc::SIGKILL);
      }
      let _ = c.wait();
    }
  }
}

/// Field order matters: processes are killed before the directory is removed.
struct Env {
  procs: Group,
  bus: String,
  dir: tempfile::TempDir,
}

impl Env {
  fn runtime(&self) -> &Path {
    self.dir.path()
  }

  fn home(&self) -> PathBuf {
    self.dir.path().join("home")
  }

  fn display_path(&self) -> String {
    self.runtime().join(SOCKET).to_string_lossy().into_owned()
  }

  /// A cleared-environment command for this sandbox.
  fn cmd(&self, bin: &Path) -> Command {
    let home = self.home();
    let mut c = Command::new(bin);
    c.env_clear()
      .env("PATH", std::env::var_os("PATH").unwrap_or_default())
      .env("HOME", &home)
      .env("XDG_RUNTIME_DIR", self.runtime())
      .env("XDG_CONFIG_HOME", home.join(".config"))
      .env("XDG_DATA_HOME", home.join(".local/share"))
      .env("XDG_CACHE_HOME", home.join(".cache"))
      .env("XDG_STATE_HOME", home.join(".local/state"))
      .env("DBUS_SESSION_BUS_ADDRESS", &self.bus)
      .stdin(Stdio::null())
      .process_group(0);
    if let Some(d) = std::env::var_os("XDG_DATA_DIRS") {
      c.env("XDG_DATA_DIRS", d);
    }
    c
  }

  /// Private dbus-daemon only.
  fn bus_only() -> Env {
    // Short path: the Wayland socket path must fit in sun_path (108 bytes).
    let dir = tempfile::Builder::new().prefix("spk").tempdir_in("/tmp").expect("tempdir");
    std::fs::set_permissions(dir.path(), std::os::unix::fs::PermissionsExt::from_mode(0o700))
      .unwrap();
    std::fs::create_dir_all(dir.path().join("home/.config")).unwrap();
    let addr = format!("unix:path={}/bus", dir.path().display());
    let mut c = Command::new(which("dbus-daemon"));
    c.env_clear()
      .env("PATH", std::env::var_os("PATH").unwrap_or_default())
      .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
      .arg(format!("--address={addr}"))
      .stdin(Stdio::null())
      .stdout(Stdio::piped())
      .stderr(Stdio::null())
      .process_group(0);
    let mut child = c.spawn().expect("spawn dbus-daemon");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).expect("bus address");
    let bus = line.trim().to_owned();
    assert!(bus.starts_with("unix:path=/tmp/spk"), "unexpected private bus address {bus}");
    if let Ok(inherited) = std::env::var("DBUS_SESSION_BUS_ADDRESS") {
      assert_ne!(inherited, bus, "refusing to run against the inherited session bus");
    }
    Env { dir, bus, procs: Group(vec![child]) }
  }

  /// Private bus + nested virtual KWin. `no_permission_checks` sets
  /// `KWIN_WAYLAND_NO_PERMISSION_CHECKS=1` (grants fake_input to anyone).
  fn with_kwin(no_permission_checks: bool, kxkbrc: Option<&str>) -> Env {
    let mut env = Env::bus_only();
    if let Some(rc) = kxkbrc {
      std::fs::write(env.home().join(".config/kxkbrc"), rc).unwrap();
    }
    let mut c = env.cmd(&which("kwin_wayland"));
    c.args([
      "--virtual",
      "--no-lockscreen",
      "--width",
      "800",
      "--height",
      "600",
      "--socket",
      SOCKET,
    ])
    .env("QT_LOGGING_RULES", "kwin_*.debug=false")
    .stdout(Stdio::null())
    .stderr(std::fs::File::create(env.runtime().join("kwin.log")).unwrap());
    if no_permission_checks {
      c.env("KWIN_WAYLAND_NO_PERMISSION_CHECKS", "1");
    }
    env.procs.0.push(c.spawn().expect("spawn kwin_wayland"));
    let sock = env.runtime().join(SOCKET);
    let start = Instant::now();
    while !sock.exists() {
      assert!(
        start.elapsed() < TIMEOUT,
        "kwin did not create its socket; log:\n{}",
        env.kwin_log()
      );
      std::thread::sleep(Duration::from_millis(50));
    }
    env
  }

  fn kwin_log(&self) -> String {
    std::fs::read_to_string(self.runtime().join("kwin.log")).unwrap_or_default()
  }

  async fn conn(&self) -> zbus::Connection {
    zbus::connection::Builder::address(self.bus.as_str())
      .unwrap()
      .build()
      .await
      .expect("connect private bus")
  }

  /// Waits until KWin's scripting object answers.
  async fn wait_scripting(&self, conn: &zbus::Connection) {
    let start = Instant::now();
    loop {
      let r = conn
        .call_method(
          Some("org.kde.KWin"),
          "/Scripting",
          Some("org.kde.kwin.Scripting"),
          "isScriptLoaded",
          &("x",),
        )
        .await;
      if r.is_ok() {
        return;
      }
      assert!(
        start.elapsed() < TIMEOUT,
        "KWin scripting never appeared; log:\n{}",
        self.kwin_log()
      );
      tokio::time::sleep(Duration::from_millis(100)).await;
    }
  }

  /// Starts `wev` (logs key events) and returns its line-buffered output.
  fn spawn_wev(&mut self) -> Arc<Mutex<Vec<String>>> {
    let mut c = self.cmd(&which("stdbuf"));
    c.args(["-oL", which("wev").to_str().unwrap(), "-f", "wl_keyboard:key"])
      .env("WAYLAND_DISPLAY", SOCKET)
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
}

async fn next_event(
  rx: &mut mpsc::Receiver<KwinEvent>,
  pred: impl Fn(&KwinEvent) -> bool,
) -> KwinEvent {
  let deadline = tokio::time::Instant::now() + TIMEOUT;
  loop {
    let ev = tokio::time::timeout_at(deadline, rx.recv())
      .await
      .expect("timed out waiting for a KWin event")
      .expect("channel closed");
    if pred(&ev) {
      return ev;
    }
  }
}

/// `(xkb keycode, pressed)` pairs that wev logged, e.g.
/// `[21: wl_keyboard] key: serial: 10; time: 468720514; key: 37; state: 1 (pressed)`.
/// wev binds one `wl_keyboard` per seat capability event, so each key is
/// logged several times with the same serial; dedupe on it.
fn wev_keys(lines: &[String]) -> Vec<(u32, bool)> {
  let mut seen = std::collections::HashSet::new();
  lines
    .iter()
    .filter_map(|l| {
      let serial: u32 = l[l.find("serial: ")? + 8..].split(';').next()?.trim().parse().ok()?;
      let rest = &l[l.rfind("key: ")? + 5..];
      let (code, rest) = rest.split_once(';')?;
      let state = rest.trim().strip_prefix("state: ")?;
      let ev = (code.trim().parse().ok()?, state.starts_with('1'));
      seen.insert(serial).then_some(ev)
    })
    .collect()
}

async fn wait_keys(lines: &Arc<Mutex<Vec<String>>>, n: usize) -> Vec<(u32, bool)> {
  let start = Instant::now();
  loop {
    let keys = wev_keys(&lines.lock().unwrap());
    if keys.len() >= n || start.elapsed() > Duration::from_secs(5) {
      return keys;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
  }
}

fn xkb(evdev: u32) -> u32 {
  evdev + 8
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_reports_focus_shortcut_and_paste_reaches_window() {
  if !enabled() {
    return;
  }
  let mut env = Env::with_kwin(true, None);
  let conn = env.conn().await;
  env.wait_scripting(&conn).await;

  // Service first, then script (the script reports the initial window).
  let (svc, mut rx) =
    KwinService::start(&conn, ServiceConfig::default()).await.expect("claim name");
  let tracker = svc.tracker();

  // A second claimant is refused.
  let conn2 = env.conn().await;
  assert!(matches!(
    KwinService::start(&conn2, ServiceConfig::default()).await,
    Err(spool_kwin::KwinError::NameTaken(_))
  ));

  let script = KwinScript::load(&conn, &script_dir()).await.expect("load script");
  // Initial report: nothing active yet in an empty virtual session.
  let first = next_event(&mut rx, |e| matches!(e, KwinEvent::ActiveWindow { .. })).await;
  eprintln!("initial: {first:?}");

  // Calls not coming from KWin are rejected by the sender check.
  let forged = conn2
    .call_method(
      Some(spool_kwin::BUS_NAME),
      spool_kwin::OBJECT_PATH,
      Some(spool_kwin::INTERFACE),
      "Show",
      &(1i32, 2i32, "x", ""),
    )
    .await;
  assert!(forged.is_err(), "non-KWin caller must be rejected");

  // Map a window -> ActiveWindow with its app id and uuid.
  let wev = env.spawn_wev();
  let ev =
    next_event(&mut rx, |e| matches!(e, KwinEvent::ActiveWindow { window_id: Some(_), .. })).await;
  let KwinEvent::ActiveWindow { app_id, window_id: Some(wid), .. } = ev else { unreachable!() };
  eprintln!("active: app_id={app_id:?} window_id={wid}");
  assert_eq!(app_id.as_deref(), Some("wev"));
  assert!(tracker.is_active(&wid));
  assert!(tracker.wait_for(&wid, Duration::from_millis(500)).await);

  // Trigger the registered global shortcut through kglobalaccel on the
  // private bus (component "kwin", action "spool-show").
  conn
    .call_method(
      Some("org.kde.kglobalaccel"),
      "/component/kwin",
      Some("org.kde.kglobalaccel.Component"),
      "invokeShortcut",
      &("spool-show",),
    )
    .await
    .expect("invokeShortcut");
  let ev = next_event(&mut rx, |e| matches!(e, KwinEvent::Show { .. })).await;
  eprintln!("show: {ev:?}");
  let KwinEvent::Show { cursor, app_id, window_id } = ev else { unreachable!() };
  assert_eq!(window_id.as_deref(), Some(wid.as_str()));
  assert_eq!(app_id.as_deref(), Some("wev"));
  let (x, y) = cursor.expect("cursor");
  assert!((0..=800).contains(&x) && (0..=600).contains(&y), "cursor {x},{y}");

  // Layout index is readable from KWin.
  assert_eq!(spool_kwin::keyboard_layout_index(&conn).await, Some(0));

  // Paste Ctrl+V and Ctrl+Shift+V into the focused wev window.
  let paste =
    PasteHandle::spawn(PasteConfig { display: Some(env.display_path()), ..Default::default() })
      .expect("fake input");
  // M7: KWin keeps using fake input even though other backends exist.
  assert_eq!(paste.backend(), PasteBackend::FakeInput);
  assert!(tracker.wait_for(&wid, Duration::from_millis(500)).await);
  paste.paste(PasteChord::CtrlV, None).await.expect("paste");
  let keys = wait_keys(&wev, 4).await;
  if keys.is_empty() {
    eprintln!("wev output:\n{}", wev.lock().unwrap().join("\n"));
  }
  assert_eq!(
    keys,
    vec![(xkb(29), true), (xkb(47), true), (xkb(47), false), (xkb(29), false)],
    "wev saw {keys:?}"
  );
  wev.lock().unwrap().clear();
  paste.paste(PasteChord::CtrlShiftV, None).await.expect("paste");
  let keys = wait_keys(&wev, 6).await;
  assert_eq!(
    keys,
    vec![
      (xkb(29), true),
      (xkb(42), true),
      (xkb(47), true),
      (xkb(47), false),
      (xkb(42), false),
      (xkb(29), false)
    ],
    "wev saw {keys:?}"
  );

  // Reloading replaces the stale instance; unload removes it.
  drop(script); // spawns an async unload
  tokio::time::sleep(Duration::from_millis(200)).await;
  let script = KwinScript::load(&conn, &script_dir()).await.expect("reload script");
  let _ = KwinScript::load(&conn, &script_dir()).await.expect("replace loaded script");
  drop(script);
  tokio::time::sleep(Duration::from_millis(200)).await;
  let s = KwinScript::load(&conn, &script_dir()).await.expect("load again");
  assert!(s.unload().await.expect("unload"));
  tokio::time::sleep(Duration::from_millis(100)).await;
  let loaded: bool = conn
    .call_method(
      Some("org.kde.KWin"),
      "/Scripting",
      Some("org.kde.kwin.Scripting"),
      "isScriptLoaded",
      &("spool",),
    )
    .await
    .unwrap()
    .body()
    .deserialize()
    .unwrap();
  assert!(!loaded);
  svc.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paste_uses_compositor_keymap_dvorak() {
  if !enabled() {
    return;
  }
  let mut env =
    Env::with_kwin(true, Some("[Layout]\nUse=true\nLayoutList=us\nVariantList=dvorak\n"));
  let conn = env.conn().await;
  env.wait_scripting(&conn).await;
  let (svc, mut rx) = KwinService::start(&conn, ServiceConfig::default()).await.unwrap();
  let _script = KwinScript::load(&conn, &script_dir()).await.unwrap();
  let wev = env.spawn_wev();
  let ev =
    next_event(&mut rx, |e| matches!(e, KwinEvent::ActiveWindow { window_id: Some(_), .. })).await;
  let KwinEvent::ActiveWindow { window_id: Some(wid), .. } = ev else { unreachable!() };

  let paste =
    PasteHandle::spawn(PasteConfig { display: Some(env.display_path()), ..Default::default() })
      .unwrap();
  let k = paste.keycodes(None).await.unwrap();
  assert_eq!(k.v, 52, "Dvorak 'v' is on the QWERTY '.' key (evdev 52)");
  assert!(svc.tracker().wait_for(&wid, Duration::from_millis(500)).await);
  paste.paste(PasteChord::CtrlV, None).await.unwrap();
  let keys = wait_keys(&wev, 4).await;
  assert_eq!(
    keys,
    vec![(xkb(29), true), (xkb(52), true), (xkb(52), false), (xkb(29), false)],
    "wev saw {keys:?}"
  );
  let lines = wev.lock().unwrap().join("\n");
  assert!(lines.contains("sym: v "), "wev should resolve the key to 'v':\n{lines}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fake_input_needs_authorisation() {
  if !enabled() {
    return;
  }
  // Permission checks on and no desktop file for this test binary: KWin
  // hides the global.
  let env = Env::with_kwin(false, None);
  let conn = env.conn().await;
  env.wait_scripting(&conn).await;
  match PasteHandle::spawn(PasteConfig { display: Some(env.display_path()), ..Default::default() })
  {
    Err(PasteError::Unsupported(_)) => {}
    Err(e) => panic!("unexpected error {e}"),
    Ok(_) => panic!("fake input must not be granted without X-KDE-Wayland-Interfaces"),
  }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fake_input_granted_by_desktop_file() {
  if !enabled() {
    return;
  }
  // KWin matches canonical(/proc/<pid>/exe) against the desktop file's Exec;
  // the file must exist before KWin starts (KService database).
  let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
  let mut env = Env::bus_only();
  let apps = env.home().join(".local/share/applications");
  std::fs::create_dir_all(&apps).unwrap();
  std::fs::write(
    apps.join("dev.bcnelson.spool.test.desktop"),
    format!(
      "[Desktop Entry]\nType=Application\nName=spool test\nExec={}\nNoDisplay=true\nX-KDE-Wayland-Interfaces=org_kde_kwin_fake_input\n",
      exe.display()
    ),
  )
  .unwrap();
  let mut c = env.cmd(&which("kwin_wayland"));
  c.args(["--virtual", "--no-lockscreen", "--socket", SOCKET])
    .stdout(Stdio::null())
    .stderr(std::fs::File::create(env.runtime().join("kwin.log")).unwrap());
  env.procs.0.push(c.spawn().unwrap());
  let conn = env.conn().await;
  env.wait_scripting(&conn).await;
  match PasteHandle::spawn(PasteConfig { display: Some(env.display_path()), ..Default::default() })
  {
    Ok(_) => {}
    Err(e) => panic!("desktop file should grant fake input: {e}\nkwin log:\n{}", env.kwin_log()),
  }
}

#[tokio::test]
async fn not_kwin_is_unsupported() {
  if !enabled() {
    return;
  }
  let env = Env::bus_only();
  let conn = env.conn().await;
  assert!(!spool_kwin::kwin_available(&conn).await);
  match KwinScript::load(&conn, &script_dir()).await {
    Err(spool_kwin::KwinError::Unsupported) => {}
    other => panic!("expected Unsupported, got {other:?}"),
  }
  // The service itself works without KWin (name + object).
  let (svc, _rx) = KwinService::start(&conn, ServiceConfig::default()).await.unwrap();
  svc.stop().await.unwrap();
}
