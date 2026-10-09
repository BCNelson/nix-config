//! spoold against a **nested virtual KWin** on a **private** session bus:
//! the KWin script is loaded, `invokeShortcut("spool-show")` reaches the
//! orchestrator, and a `Select { Paste }` (through the test-hooks socket,
//! exactly what the picker sends) pastes into a `wev` window.
//!
//! Gated on `SPOOL_KWIN_TESTS=1`; needs `--features test-hooks` and the
//! dev shell (`kwin_wayland`, `dbus-daemon`, `wev`, `stdbuf`), plus the
//! `spool-paster` helper next to `spoold`
//! (`cargo build -p spool-paste --bin spool-paster`):
//!
//! ```sh
//! SPOOL_KWIN_TESTS=1 cargo test -p spoold --features test-hooks --test nested_kwin
//! ```
//!
//! Isolation (never the real session): own `dbus-daemon` (its address is
//! checked against the inherited one), own 0700 `XDG_RUNTIME_DIR` and
//! `HOME`, cleared environments, every process in its own group and killed
//! on drop.
//!
//! Fake-input authorisation: KWin grants `org_kde_kwin_fake_input` to a
//! client whose `/proc/<pid>/exe` matches the `Exec` of a desktop file with
//! `X-KDE-Wayland-Interfaces=org_kde_kwin_fake_input`. spoold is
//! non-dumpable (it holds the data key), so KWin cannot read its
//! `/proc/<pid>/exe`; the paste connection therefore lives in the dumpable,
//! secret-free `spool-paster` helper, and the desktop file names the helper.
//! `paste_with_permission_checks` proves that path with KWin's permission
//! checks ON; the real session depends on the installed
//! `dev.bcnelson.spool.paster.desktop` (package `share/applications`).
#![cfg(feature = "test-hooks")]

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[path = "../../spoolctl/src/client.rs"]
#[allow(dead_code)]
mod client;

const SOCKET: &str = "wl-spool";
const T: Duration = Duration::from_secs(20);
/// KWin's first start with a new desktop file rebuilds the KService cache
/// (seen taking > 20 s on a loaded machine).
const KWIN_T: Duration = Duration::from_secs(60);

/// One nested KWin at a time (they are heavy and compete for the CPU).
static SERIAL: Mutex<()> = Mutex::new(());

fn enabled() -> bool {
  if std::env::var("SPOOL_KWIN_TESTS").as_deref() != Ok("1") {
    eprintln!("skipping: set SPOOL_KWIN_TESTS=1 (dev shell) to run nested-KWin tests");
    return false;
  }
  true
}

fn which(bin: &str) -> PathBuf {
  std::env::var_os("PATH")
    .and_then(|p| std::env::split_paths(&p).map(|d| d.join(bin)).find(|p| p.is_file()))
    .unwrap_or_else(|| panic!("{bin} not on PATH (use the dev shell)"))
}

/// PATH for spoold: entries its audit accepts (under /nix/store, or
/// root-owned and not group/world-writable).
fn trusted_path() -> String {
  use std::os::unix::fs::MetadataExt;
  let path = std::env::var_os("PATH").unwrap_or_default();
  let keep: Vec<PathBuf> = std::env::split_paths(&path)
    .filter(|dir| {
      let Ok(resolved) = std::fs::canonicalize(dir) else { return false };
      dir.is_absolute()
        && (resolved.starts_with("/nix/store")
          || std::fs::metadata(&resolved).is_ok_and(|m| m.uid() == 0 && m.mode() & 0o022 == 0))
    })
    .collect();
  std::env::join_paths(keep).unwrap().into_string().unwrap()
}

fn spoold_exe() -> PathBuf {
  PathBuf::from(env!("CARGO_BIN_EXE_spoold")).canonicalize().unwrap()
}

fn paster_exe() -> PathBuf {
  let p = spoold_exe().with_file_name("spool-paster");
  assert!(p.is_file(), "{} missing: cargo build -p spool-paste --bin spool-paster", p.display());
  p
}

fn poll_until<R>(what: &str, t: Duration, mut f: impl FnMut() -> Option<R>) -> R {
  let start = Instant::now();
  loop {
    if let Some(r) = f() {
      return r;
    }
    assert!(start.elapsed() < t, "timed out waiting for {what}");
    std::thread::sleep(Duration::from_millis(50));
  }
}

/// Kills each child's process group on drop.
struct Group(Vec<Child>);

impl Drop for Group {
  fn drop(&mut self) {
    for c in self.0.iter_mut().rev() {
      // SAFETY: kill(2) on a process group we created.
      unsafe {
        libc::kill(-(c.id() as i32), libc::SIGTERM);
      }
    }
    std::thread::sleep(Duration::from_millis(200));
    for c in &mut self.0 {
      unsafe {
        libc::kill(-(c.id() as i32), libc::SIGKILL);
      }
      let _ = c.wait();
    }
  }
}

/// Field order: processes die before the directory goes.
struct Env {
  procs: Group,
  spoold: Option<Child>,
  bus: String,
  dir: tempfile::TempDir,
  /// Directory with a `spool-picker` symlink, first on spoold's PATH.
  picker_dir: Option<PathBuf>,
}

impl Env {
  fn rt(&self) -> &Path {
    self.dir.path()
  }

  fn home(&self) -> PathBuf {
    self.dir.path().join("home")
  }

  fn cmd(&self, bin: &Path) -> Command {
    let home = self.home();
    let mut c = Command::new(bin);
    c.env_clear()
      .env("PATH", std::env::var_os("PATH").unwrap_or_default())
      .env("HOME", &home)
      .env("XDG_RUNTIME_DIR", self.rt())
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

  /// Private bus, a desktop file granting fake input to `grant` (if any),
  /// then nested KWin.
  fn start(no_permission_checks: bool, grant: Option<&Path>) -> Env {
    Self::start_with(no_permission_checks, grant, &[])
  }

  /// [`Env::start`] plus desktop files granting `(exe, interfaces)` (test
  /// tools such as wtype, which needs `zwp_virtual_keyboard_manager_v1`).
  fn start_with(no_permission_checks: bool, grant: Option<&Path>, extra: &[(&Path, &str)]) -> Env {
    let dir = tempfile::Builder::new().prefix("spk").tempdir_in("/tmp").expect("tempdir");
    std::fs::set_permissions(dir.path(), std::os::unix::fs::PermissionsExt::from_mode(0o700))
      .unwrap();
    for d in ["home/.config", "home/.local/share/applications", "data"] {
      std::fs::create_dir_all(dir.path().join(d)).unwrap();
    }
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
    let mut env = Env { procs: Group(vec![child]), spoold: None, bus, dir, picker_dir: None };

    if let Some(exe) = grant {
      // Must exist before KWin starts (KService database).
      std::fs::write(
        env.home().join(".local/share/applications/dev.bcnelson.spool.paster.desktop"),
        format!(
          "[Desktop Entry]\nType=Application\nName=Spool paste helper\nExec={}\nNoDisplay=true\nX-KDE-Wayland-Interfaces=org_kde_kwin_fake_input\n",
          exe.display()
        ),
      )
      .unwrap();
    }
    for (i, (exe, ifaces)) in extra.iter().enumerate() {
      std::fs::write(
        env.home().join(format!(".local/share/applications/test.spool.tool{i}.desktop")),
        format!(
          "[Desktop Entry]\nType=Application\nName=test tool {i}\nExec={}\nNoDisplay=true\nX-KDE-Wayland-Interfaces={ifaces}\n",
          exe.display()
        ),
      )
      .unwrap();
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
    .stderr(std::fs::File::create(env.rt().join("kwin.log")).unwrap());
    if no_permission_checks {
      c.env("KWIN_WAYLAND_NO_PERMISSION_CHECKS", "1");
    }
    env.procs.0.push(c.spawn().expect("spawn kwin_wayland"));
    let sock = env.rt().join(SOCKET);
    poll_until("kwin's socket", KWIN_T, || sock.exists().then_some(()));
    env.wait_scripting();
    env
  }

  fn kwin_log(&self) -> String {
    std::fs::read_to_string(self.rt().join("kwin.log")).unwrap_or_default()
  }

  fn busctl(&self, args: &[&str]) -> bool {
    self
      .cmd(&which("dbus-send"))
      .arg(format!("--bus={}", self.bus))
      .args(["--print-reply", "--type=method_call"])
      .args(args)
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .status()
      .is_ok_and(|s| s.success())
  }

  fn wait_scripting(&self) {
    let start = Instant::now();
    while !self.busctl(&[
      "--dest=org.kde.KWin",
      "/Scripting",
      "org.kde.kwin.Scripting.isScriptLoaded",
      "string:x",
    ]) {
      assert!(start.elapsed() < KWIN_T, "KWin scripting never appeared; log:\n{}", self.kwin_log());
      std::thread::sleep(Duration::from_millis(100));
    }
  }

  fn socket(&self) -> PathBuf {
    self.rt().join("spool/sock")
  }

  fn ctl_socket(&self) -> PathBuf {
    self.dir.path().join("data/ctl.sock")
  }

  fn log(&self) -> String {
    std::fs::read_to_string(self.dir.path().join("data/spoold.log")).unwrap_or_default()
  }

  fn start_spoold(&mut self) {
    let cfg = self.dir.path().join("data/config.toml");
    std::fs::write(&cfg, "key_provider = \"session\"\n").unwrap();
    let mut c = self.cmd(&spoold_exe());
    let path = match &self.picker_dir {
      // The symlink dir is user-owned: development override.
      Some(d) => {
        c.env("SPOOL_INSECURE_PATH_OK", "1");
        format!("{}:{}", d.display(), trusted_path())
      }
      None => trusted_path(),
    };
    c.arg("--log-stderr")
      .env("PATH", path)
      .env("WAYLAND_DISPLAY", SOCKET)
      .env("SPOOL_SOCKET", self.socket())
      .env("SPOOL_STATE_DIR", self.dir.path().join("data/state"))
      .env("SPOOL_CONFIG", &cfg)
      .env("SPOOL_KWIN_SCRIPT_DIR", Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kwin-script"))
      .env("SPOOL_TEST_HOOK_CTL_SOCKET", self.ctl_socket())
      .env("RUST_LOG", "info,spoold=debug")
      .env("NO_COLOR", "1")
      .stdout(Stdio::null())
      .stderr(std::fs::File::create(self.dir.path().join("data/spoold.log")).unwrap());
    self.spoold = Some(c.spawn().expect("spawn spoold"));
    poll_until("spoold's desktop integration", T, || {
      if let Some(st) = self.spoold.as_mut().unwrap().try_wait().unwrap() {
        panic!("spoold exited early ({st}); log:\n{}", self.log());
      }
      self.log().lines().find(|l| l.contains("desktop integration")).map(str::to_owned)
    });
  }

  /// SIGTERM spoold and check it exits 0 and unloads the script.
  fn stop_spoold(&mut self) {
    let mut d = self.spoold.take().unwrap();
    unsafe {
      libc::kill(d.id() as i32, libc::SIGTERM);
    }
    let st = poll_until("spoold to exit", Duration::from_secs(10), || d.try_wait().unwrap());
    assert_eq!(st.code(), Some(0), "log:\n{}", self.log());
  }

  fn script_loaded(&self) -> bool {
    let out = self
      .cmd(&which("dbus-send"))
      .arg(format!("--bus={}", self.bus))
      .args(["--print-reply", "--type=method_call", "--dest=org.kde.KWin", "/Scripting"])
      .args(["org.kde.kwin.Scripting.isScriptLoaded", "string:spool"])
      .output()
      .unwrap();
    String::from_utf8_lossy(&out.stdout).contains("boolean true")
  }

  fn invoke_shortcut(&self) {
    assert!(
      self.busctl(&[
        "--dest=org.kde.kglobalaccel",
        "/component/kwin",
        "org.kde.kglobalaccel.Component.invokeShortcut",
        "string:spool-show",
      ]),
      "invokeShortcut failed"
    );
  }

  fn copy(&self, text: &str) {
    let req = spool_proto::PublicReq::Copy {
      mime: "text/plain;charset=utf-8".into(),
      data: text.as_bytes().to_vec(),
    };
    let r = client::Client::connect_to(&self.socket()).unwrap().request(&req).unwrap();
    assert_eq!(r, spool_proto::PublicResp::Ok);
  }

  /// `Request::Select` for the newest item via the test-hooks socket.
  fn select_latest(&self, mode: u8) -> String {
    let mut s = std::os::unix::net::UnixStream::connect(self.ctl_socket()).unwrap();
    spool_proto::write_frame(&mut s, &mode).unwrap();
    let r: Result<String, String> = spool_proto::read_frame(&mut s).unwrap();
    r.unwrap_or_else(|e| panic!("select failed: {e}; log:\n{}", self.log()))
  }

  /// `wev` logging key and focus events; returns its output lines.
  fn spawn_wev(&mut self) -> Arc<Mutex<Vec<String>>> {
    let mut c = self.cmd(&which("stdbuf"));
    c.args([
      "-oL",
      which("wev").to_str().unwrap(),
      "-f",
      "wl_keyboard:key",
      "-f",
      "wl_keyboard:enter",
      "-f",
      "wl_keyboard:leave",
    ])
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
    poll_until("wev to get keyboard focus", T, || {
      lines.lock().unwrap().iter().any(|l| l.contains("enter")).then_some(())
    });
    lines
  }
}

impl Env {
  /// Put the built `spool-picker` on spoold's PATH (before `start_spoold`).
  fn enable_picker(&mut self) {
    let bin = spoold_exe().with_file_name("spool-picker");
    assert!(bin.is_file(), "{} missing: cargo build -p spool-picker", bin.display());
    let dir = self.dir.path().join("data/picker-bin");
    std::fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(&bin, dir.join("spool-picker")).unwrap();
    self.picker_dir = Some(dir);
  }

  /// Press and release evdev `keys` (one after the other) through KWin's
  /// fake input, from this (dumpable, desktop-file-granted) test process.
  /// KWin 6.7 has no `zwp_virtual_keyboard_v1`, so wtype cannot be used.
  fn press_keys(&self, keys: &[u32]) {
    fake_keys::press(&self.rt().join(SOCKET), keys);
  }

  /// Screenshot through KWin's ScreenShot2 (spectacle, authorised by its
  /// installed desktop file), copied to `$SPOOL_E2E_SHOTS`. `None` if
  /// spectacle is missing or KWin refuses it.
  fn screenshot(&self, name: &str) -> Option<PathBuf> {
    let spectacle = std::env::var_os("PATH")
      .and_then(|p| std::env::split_paths(&p).map(|d| d.join("spectacle")).find(|p| p.is_file()))
      .or_else(|| Some(PathBuf::from("/run/current-system/sw/bin/spectacle")))
      .filter(|p| p.is_file())?;
    let out = self.dir.path().join(format!("data/{name}.png"));
    let mut c = self.cmd(&spectacle);
    c.args(["-b", "-n", "-f", "-o"])
      .arg(&out)
      .env("WAYLAND_DISPLAY", SOCKET)
      .env("QT_QPA_PLATFORM", "wayland")
      .stdout(Stdio::null())
      .stderr(std::fs::File::create(self.dir.path().join("data/spectacle.log")).unwrap());
    let mut child = c.spawn().ok()?;
    let start = Instant::now();
    let st = loop {
      if let Some(st) = child.try_wait().ok()? {
        break st;
      }
      if start.elapsed() > Duration::from_secs(20) {
        let _ = child.kill();
        let _ = child.wait();
        return None;
      }
      std::thread::sleep(Duration::from_millis(100));
    };
    if !st.success() || !out.is_file() {
      return None;
    }
    if let Some(dir) = std::env::var_os("SPOOL_E2E_SHOTS") {
      let _ = std::fs::copy(&out, Path::new(&dir).join(format!("{name}.png")));
    }
    Some(out)
  }
}

/// `(xkb keycode, pressed)` from wev's `key:` lines, deduplicated on serial.
fn wev_keys(lines: &[String]) -> Vec<(u32, bool)> {
  let mut seen = std::collections::HashSet::new();
  lines
    .iter()
    .filter(|l| l.contains("] key:"))
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

fn wait_keys(lines: &Arc<Mutex<Vec<String>>>, n: usize) -> Vec<(u32, bool)> {
  let start = Instant::now();
  loop {
    let keys = wev_keys(&lines.lock().unwrap());
    if keys.len() >= n || start.elapsed() > Duration::from_secs(5) {
      return keys;
    }
    std::thread::sleep(Duration::from_millis(50));
  }
}

const CTRL_V: [(u32, bool); 4] = [(29 + 8, true), (47 + 8, true), (47 + 8, false), (29 + 8, false)];

/// The whole M5 flow: script loaded, shortcut -> orchestrator, Select{Paste}
/// -> Ctrl+V in wev, clean SIGTERM unloads the script.
fn show_and_paste(env: &mut Env) {
  env.start_spoold();
  let line = env.log().lines().find(|l| l.contains("desktop integration")).unwrap().to_owned();
  assert!(line.contains("hotkey=\"kwin-script\"") || line.contains("hotkey=kwin-script"), "{line}");
  assert!(line.contains("auto_paste=true"), "{line}\nlog:\n{}", env.log());
  assert!(line.contains("fake-input"), "{line}");
  eprintln!("{line}");
  assert!(env.script_loaded(), "KWin script not loaded; log:\n{}", env.log());

  let wev = env.spawn_wev();
  env.copy("pasted by spool");
  env.invoke_shortcut();
  poll_until("the shortcut to reach the orchestrator", T, || {
    env.log().lines().find(|l| l.contains("show requested (shortcut)")).map(str::to_owned)
  });
  let shown = env.log();
  assert!(shown.contains("has_target=true"), "log:\n{shown}");
  assert!(shown.contains("picker not wired"), "log:\n{shown}");

  wev.lock().unwrap().clear();
  let out = env.select_latest(1);
  assert_eq!(out, "Ok(Pasted { chord: CtrlV })", "log:\n{}", env.log());
  let keys = wait_keys(&wev, 4);
  assert_eq!(keys, CTRL_V.to_vec(), "wev saw {keys:?}");

  // The target is consumed by the paste: a second paste has none.
  assert_eq!(env.select_latest(1), "Ok(NotPasted(NoTarget))");

  env.stop_spoold();
  assert!(!env.script_loaded(), "script still loaded after SIGTERM");
  let log = env.log();
  assert!(!log.contains("pasted by spool"), "content leaked into the log");
}

#[test]
fn paste_with_permission_checks() {
  if !enabled() {
    return;
  }
  let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
  // As in a real session: KWin checks permissions; the desktop file grants
  // fake input to the helper only.
  let mut env = Env::start(false, Some(&paster_exe()));
  show_and_paste(&mut env);
}

#[test]
fn paste_without_permission_checks() {
  if !enabled() {
    return;
  }
  let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
  let mut env = Env::start(true, None);
  show_and_paste(&mut env);
}

#[test]
fn no_grant_means_no_auto_paste() {
  if !enabled() {
    return;
  }
  let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
  // Permission checks on and no desktop file: the shortcut still works,
  // auto-paste is reported unavailable and Select only copies.
  let mut env = Env::start(false, None);
  env.start_spoold();
  let line = env.log().lines().find(|l| l.contains("desktop integration")).unwrap().to_owned();
  assert!(line.contains("auto_paste=false"), "{line}");
  let _wev = env.spawn_wev();
  env.copy("x");
  env.invoke_shortcut();
  poll_until("the shortcut", T, || env.log().contains("show requested (shortcut)").then_some(()));
  assert_eq!(env.select_latest(1), "Ok(NotPasted(NoBackend))");
  env.stop_spoold();
}

#[test]
fn spoold_itself_is_not_granted_fake_input() {
  if !enabled() {
    return;
  }
  let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
  // Documents why the helper exists: a desktop file naming spoold does not
  // help, KWin cannot read a non-dumpable process's /proc/<pid>/exe.
  let mut env = Env::start(false, Some(&spoold_exe()));
  let probe = env.dir.path().join("data/probe.log");
  let mut c = env.cmd(&spoold_exe());
  c.arg("--probe-fake-input")
    .env("WAYLAND_DISPLAY", SOCKET)
    .env("PATH", trusted_path())
    .stdout(Stdio::null())
    .stderr(std::fs::File::create(&probe).unwrap());
  let st = c.status().unwrap();
  let out = std::fs::read_to_string(&probe).unwrap_or_default();
  assert!(!st.success(), "non-dumpable spoold must not get fake input: {out}");
  assert!(out.contains("unsupported"), "{out}");
  eprintln!("probe said: {}", out.trim());
  let _ = env.spoold.take();
}

/// The resident picker in nested KWin with permission checks ON: Meta+V
/// (`invokeShortcut`) opens the real `spool-picker`, Enter picks the newest
/// item, and Ctrl+V reaches wev through `spool-paster` (desktop-file grant).
/// Enter is pressed through fake input from the test process.
#[test]
fn picker_shortcut_enter_pastes_with_permission_checks() {
  if !enabled() {
    return;
  }
  let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
  // Keys reach the picker through fake input from this test binary, granted
  // like the paste helper (its real exe).
  let me = std::env::current_exe().unwrap().canonicalize().unwrap();
  let mut env = Env::start_with(false, Some(&paster_exe()), &[(&me, "org_kde_kwin_fake_input")]);
  env.enable_picker();
  env.start_spoold();
  poll_until("the picker to be ready", T, || env.log().contains("picker ready").then_some(()));
  let line = env.log().lines().find(|l| l.contains("desktop integration")).unwrap().to_owned();
  assert!(line.contains("fake-input"), "{line}");

  let wev = env.spawn_wev();
  env.copy("older kwin item");
  env.copy("pasted by the picker");
  env.invoke_shortcut();
  poll_until("the shortcut to reach the orchestrator", T, || {
    env.log().contains("show requested (shortcut)").then_some(())
  });
  // The picker maps and takes the keyboard: wev loses focus.
  poll_until("wev to lose keyboard focus", T, || {
    wev.lock().unwrap().iter().any(|l| l.contains("leave")).then_some(())
  });
  std::thread::sleep(Duration::from_millis(300));
  match env.screenshot("kwin-picker-shown") {
    Some(p) => eprintln!("screenshot: {}", p.display()),
    None => eprintln!("no screenshot (spectacle unavailable or refused); relying on focus events"),
  }
  wev.lock().unwrap().clear();
  env.press_keys(&[KEY_ENTER]);
  poll_until("the picker to report Hidden{Selected}", T, || {
    env.log().contains("picker hidden reason=Selected").then_some(())
  });
  poll_until("the auto-paste", T, || env.log().contains("auto-pasted").then_some(()));
  // The picker hides on Enter's press, so wev also gets the release.
  let enter_up = (KEY_ENTER + 8, false);
  let keys: Vec<_> = wait_keys(&wev, 5).into_iter().filter(|k| *k != enter_up).collect();
  assert_eq!(keys, CTRL_V.to_vec(), "wev saw {keys:?}; log:\n{}", env.log());
  env.stop_spoold();
  let log = env.log();
  assert!(!log.contains("pasted by the picker"), "content leaked into the log");
}

const KEY_ENTER: u32 = 28;

/// A minimal `org_kde_kwin_fake_input` client.
mod fake_keys {
  use std::os::unix::net::UnixStream;
  use std::path::Path;
  use std::time::Duration;

  use wayland_client::globals::{GlobalListContents, registry_queue_init};
  use wayland_client::protocol::wl_registry;
  use wayland_client::{Connection, Dispatch, QueueHandle};
  use wayland_protocols_plasma::fake_input::client::org_kde_kwin_fake_input::OrgKdeKwinFakeInput;

  struct S;

  impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for S {
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

  impl Dispatch<OrgKdeKwinFakeInput, ()> for S {
    fn event(
      _: &mut Self,
      _: &OrgKdeKwinFakeInput,
      _: <OrgKdeKwinFakeInput as wayland_client::Proxy>::Event,
      _: &(),
      _: &Connection,
      _: &QueueHandle<Self>,
    ) {
    }
  }

  pub fn press(socket: &Path, keys: &[u32]) {
    let conn = Connection::from_socket(UnixStream::connect(socket).expect("kwin socket")).unwrap();
    let (globals, mut queue) = registry_queue_init::<S>(&conn).unwrap();
    let qh = queue.handle();
    let fake: OrgKdeKwinFakeInput = globals
      .bind(&qh, 4..=5, ())
      .expect("KWin did not grant org_kde_kwin_fake_input to the test binary");
    fake.authenticate("spool tests".into(), "press keys in the picker".into());
    queue.roundtrip(&mut S).unwrap();
    for &k in keys {
      fake.keyboard_key(k, 1);
      queue.roundtrip(&mut S).unwrap();
      std::thread::sleep(Duration::from_millis(20));
      fake.keyboard_key(k, 0);
      queue.roundtrip(&mut S).unwrap();
      std::thread::sleep(Duration::from_millis(20));
    }
  }
}
