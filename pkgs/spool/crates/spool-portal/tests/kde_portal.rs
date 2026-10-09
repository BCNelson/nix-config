//! spool-portal against the **real** portal stack, fully isolated:
//! private `dbus-daemon` (only the service files this test writes are
//! activatable) + nested virtual KWin (`kwin_wayland --virtual`, hosts
//! kglobalaccel) + `xdg-desktop-portal` + `xdg-desktop-portal-kde` (a
//! GlobalShortcuts backend). The shortcut is then fired through kglobalaccel
//! and must arrive as `Show`. Gated on `SPOOL_PORTAL_KDE_TESTS=1`; run from
//! the dev shell:
//!
//! ```sh
//! SPOOL_PORTAL_KDE_TESTS=1 cargo test -p spool-portal --test kde_portal -- --nocapture
//! ```
//!
//! Isolation: cleared environment, 0700 runtime dir/HOME under `/tmp`, the
//! bus config has no system/session service directories, and every client
//! connects by explicit address. The user's session bus, portal and KWin
//! are never contacted.

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use spool_compositor::{CompositorEvent, EventSink};
use spool_portal::{PortalConfig, PortalShortcuts};

const TIMEOUT: Duration = Duration::from_secs(30);
const SOCKET: &str = "wl-spool";

fn enabled() -> bool {
  if std::env::var("SPOOL_PORTAL_KDE_TESTS").as_deref() != Ok("1") {
    eprintln!("skipping: set SPOOL_PORTAL_KDE_TESTS=1 (dev shell) to run the real-portal test");
    return false;
  }
  true
}

fn which(bin: &str) -> PathBuf {
  std::env::var_os("PATH")
    .and_then(|p| std::env::split_paths(&p).map(|d| d.join(bin)).find(|p| p.is_file()))
    .unwrap_or_else(|| panic!("{bin} not on PATH (use the dev shell)"))
}

/// `libexec/<bin>` of a package whose `share` is on `XDG_DATA_DIRS`.
fn libexec(bin: &str) -> PathBuf {
  std::env::var_os("XDG_DATA_DIRS")
    .and_then(|p| {
      std::env::split_paths(&p)
        .filter_map(|d| d.parent().map(|p| p.join("libexec").join(bin)))
        .find(|p| p.is_file())
    })
    .unwrap_or_else(|| panic!("libexec/{bin} not found via XDG_DATA_DIRS (use the dev shell)"))
}

struct Group(Vec<Child>);

impl Drop for Group {
  fn drop(&mut self) {
    for c in &mut self.0 {
      let _ = Command::new("kill")
        .args(["-TERM", &format!("-{}", c.id())])
        .stderr(Stdio::null())
        .status();
    }
    std::thread::sleep(Duration::from_millis(200));
    for c in &mut self.0 {
      let _ = Command::new("kill")
        .args(["-KILL", &format!("-{}", c.id())])
        .stderr(Stdio::null())
        .status();
      let _ = c.wait();
    }
  }
}

struct Env {
  procs: Group,
  bus: String,
  dir: tempfile::TempDir,
}

impl Env {
  fn root(&self) -> &Path {
    self.dir.path()
  }

  fn log(&self, name: &str) -> String {
    std::fs::read_to_string(self.root().join(name)).unwrap_or_default()
  }

  fn logs(&self) -> String {
    ["bus.log", "kwin.log"].iter().map(|n| format!("== {n}\n{}", self.log(n))).collect()
  }

  fn start() -> Env {
    let dir = tempfile::Builder::new().prefix("spq").tempdir_in("/tmp").unwrap();
    let root = dir.path().to_owned();
    std::fs::set_permissions(&root, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let home = root.join("home");
    let apps = home.join(".local/share/applications");
    let services = root.join("services");
    for d in [&apps, &services, &home.join(".config/xdg-desktop-portal")] {
      std::fs::create_dir_all(d).unwrap();
    }
    // Like the desktop file the package ships; the portal Registry insists
    // on it, and GLib ignores entries whose Exec binary does not exist.
    std::fs::write(
      apps.join("dev.bcnelson.spool.daemon.desktop"),
      format!(
        "[Desktop Entry]\nType=Application\nName=Spool daemon\nExec={}\nNoDisplay=true\n",
        which("true").display()
      ),
    )
    .unwrap();
    std::fs::write(
      home.join(".config/xdg-desktop-portal/portals.conf"),
      "[preferred]\ndefault=kde\n",
    )
    .unwrap();
    let service = |name: &str, exec: PathBuf| {
      std::fs::write(
        services.join(format!("{name}.service")),
        format!("[D-BUS Service]\nName={name}\nExec={}\n", exec.display()),
      )
      .unwrap();
    };
    service("org.freedesktop.portal.Desktop", libexec("xdg-desktop-portal"));
    service("org.freedesktop.impl.portal.PermissionStore", libexec("xdg-permission-store"));
    service("org.freedesktop.impl.portal.desktop.kde", libexec("xdg-desktop-portal-kde"));
    let conf = root.join("bus.conf");
    std::fs::write(
      &conf,
      format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={root}/bus</listen>
  <auth>EXTERNAL</auth>
  <servicedir>{services}</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
        root = root.display(),
        services = services.display()
      ),
    )
    .unwrap();
    let mut env = Env { procs: Group(Vec::new()), bus: String::new(), dir };
    // The bus passes its environment to activated services (portal + KDE
    // backend), so it gets the sandbox environment.
    let mut c = env.cmd(&which("dbus-daemon"));
    c.args(["--nofork", "--nopidfile", "--print-address=1"])
      .arg(format!("--config-file={}", conf.display()))
      .stdout(Stdio::piped())
      .stderr(std::fs::File::create(root.join("bus.log")).unwrap());
    let mut child = c.spawn().expect("dbus-daemon");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    env.bus = line.trim().to_owned();
    assert!(env.bus.starts_with("unix:path=/tmp/spq"), "unexpected bus {}", env.bus);
    if let Ok(inherited) = std::env::var("DBUS_SESSION_BUS_ADDRESS") {
      assert_ne!(inherited, env.bus);
    }
    env.procs.0.push(child);

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
    .env("DBUS_SESSION_BUS_ADDRESS", &env.bus)
    .env("KWIN_WAYLAND_NO_PERMISSION_CHECKS", "1")
    .env_remove("WAYLAND_DISPLAY")
    .stdout(Stdio::null())
    .stderr(std::fs::File::create(root.join("kwin.log")).unwrap());
    env.procs.0.push(c.spawn().expect("kwin_wayland"));
    let start = Instant::now();
    while !root.join(SOCKET).exists() {
      assert!(start.elapsed() < TIMEOUT, "kwin did not start:\n{}", env.logs());
      std::thread::sleep(Duration::from_millis(50));
    }
    env
  }

  fn cmd(&self, bin: &Path) -> Command {
    let root = self.root();
    let home = root.join("home");
    let mut c = Command::new(bin);
    c.env_clear()
      .env("PATH", std::env::var_os("PATH").unwrap_or_default())
      .env("HOME", &home)
      .env("XDG_RUNTIME_DIR", root)
      .env("XDG_CONFIG_HOME", home.join(".config"))
      .env("XDG_DATA_HOME", home.join(".local/share"))
      .env("XDG_CACHE_HOME", home.join(".cache"))
      .env("XDG_STATE_HOME", home.join(".local/state"))
      .env("XDG_CURRENT_DESKTOP", "KDE")
      .env("WAYLAND_DISPLAY", SOCKET)
      .env("QT_QPA_PLATFORM", "wayland")
      .env("QT_LOGGING_RULES", "*.debug=false")
      .stdin(Stdio::null())
      .process_group(0);
    for k in ["XDG_DATA_DIRS", "QT_PLUGIN_PATH", "QML2_IMPORT_PATH", "LOCALE_ARCHIVE"] {
      if let Some(v) = std::env::var_os(k) {
        c.env(k, v);
      }
    }
    c
  }

  async fn conn(&self) -> zbus::Connection {
    zbus::connection::Builder::address(self.bus.as_str()).unwrap().build().await.unwrap()
  }
}

/// kglobalaccel component that owns `spool-show` (xdp-kde names it after
/// the app id).
async fn find_component(conn: &zbus::Connection) -> Option<zbus::zvariant::OwnedObjectPath> {
  let reply = conn
    .call_method(
      Some("org.kde.kglobalaccel"),
      "/kglobalaccel",
      Some("org.kde.KGlobalAccel"),
      "allComponents",
      &(),
    )
    .await
    .ok()?;
  let comps: Vec<zbus::zvariant::OwnedObjectPath> = reply.body().deserialize().ok()?;
  for c in comps {
    let names = conn
      .call_method(
        Some("org.kde.kglobalaccel"),
        c.as_str(),
        Some("org.kde.kglobalaccel.Component"),
        "shortcutNames",
        &(),
      )
      .await
      .ok()
      .and_then(|r| r.body().deserialize::<Vec<String>>().ok())
      .unwrap_or_default();
    if names.iter().any(|n| n == "spool-show") {
      return Some(c);
    }
  }
  None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_portal_binds_and_fires_show() {
  if !enabled() {
    return;
  }
  let env = Env::start();
  let conn = env.conn().await;
  let (sink, mut rx) = EventSink::new();
  sink.active_window(Some("org.kde.kate"), Some("w1"));
  let _ = rx.recv().await;

  // Registry first (what `start` does by default), asserted explicitly here.
  let reg =
    tokio::time::timeout(TIMEOUT, spool_portal::register_app_id(&conn, spool_portal::APP_ID))
      .await
      .expect("register timed out");
  assert!(matches!(reg, Ok(true)), "Registry.Register: {reg:?}\n{}", env.logs());
  let cfg = PortalConfig { app_id: None, ..Default::default() };
  // xdp-kde asks the user to confirm new shortcuts in a dialog; accept it
  // with Return through the nested KWin's fake input.
  let display = env.root().join(SOCKET);
  let confirm = tokio::task::spawn_blocking(move || {
    std::thread::sleep(Duration::from_secs(4));
    for _ in 0..3 {
      press_key(&display, KEY_ENTER);
      std::thread::sleep(Duration::from_secs(2));
    }
  });
  let ps = tokio::time::timeout(TIMEOUT, PortalShortcuts::start(&conn, cfg, sink.clone(), None))
    .await
    .unwrap_or_else(|_| panic!("portal bind timed out\n{}", env.logs()))
    .unwrap_or_else(|e| panic!("portal bind failed: {e}\n{}", env.logs()));
  confirm.abort();
  // xdp-kde assigned the preferred trigger (LOGO+v).
  assert_eq!(ps.trigger_description(), Some("Meta+V"));

  let comp = find_component(&conn).await.unwrap_or_else(|| panic!("no component\n{}", env.logs()));
  conn
    .call_method(
      Some("org.kde.kglobalaccel"),
      comp.as_str(),
      Some("org.kde.kglobalaccel.Component"),
      "invokeShortcut",
      &("spool-show",),
    )
    .await
    .expect("invokeShortcut");
  let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv())
    .await
    .unwrap_or_else(|_| panic!("no Show\n{}", env.logs()))
    .unwrap();
  assert_eq!(
    ev,
    CompositorEvent::Show {
      cursor: None,
      app_id: Some("org.kde.kate".into()),
      window_id: Some("w1".into())
    }
  );
  ps.stop().await;
}

const KEY_ENTER: u32 = 28;

/// Presses and releases one evdev key in the nested KWin.
fn press_key(display: &Path, code: u32) {
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
  wayland_client::delegate_noop!(S: ignore OrgKdeKwinFakeInput);
  let Ok(stream) = std::os::unix::net::UnixStream::connect(display) else { return };
  let conn = Connection::from_socket(stream).unwrap();
  let (globals, mut q) = registry_queue_init::<S>(&conn).unwrap();
  let fake: OrgKdeKwinFakeInput = globals.bind(&q.handle(), 4..=5, ()).expect("fake input");
  fake.authenticate("spool-test".into(), "confirm dialog".into());
  fake.keyboard_key(code, 1);
  q.roundtrip(&mut S).unwrap();
  std::thread::sleep(Duration::from_millis(30));
  fake.keyboard_key(code, 0);
  q.roundtrip(&mut S).unwrap();
}
