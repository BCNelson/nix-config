//! End-to-end tests: the real `spoold` and `spoolctl` binaries against a
//! private, nested headless sway, driven with `wl-copy`/`wl-paste`.
//!
//! Gated: they only run when `SPOOL_WAYLAND_TESTS=1` (and
//! `SPOOL_SKIP_WAYLAND_TESTS` is unset) and `sway`, `wl-copy`, `wl-paste`
//! are on PATH; otherwise every test returns early with a message, so the
//! Nix sandbox build (`cargo test --workspace`) skips them. From the dev
//! shell:
//!
//! ```sh
//! SPOOL_WAYLAND_TESTS=1 cargo test -p spoold --test e2e
//! # copy-latency report (prints p50/p95/p99/max):
//! SPOOL_WAYLAND_TESTS=1 cargo test -p spoold --test e2e -- --ignored --nocapture copy_latency
//! ```
//!
//! Isolation: each test starts its own sway with a fresh 0700
//! `XDG_RUNTIME_DIR`; every child (sway, spoold, wl-copy, wl-paste,
//! spoolctl) runs with a cleared environment pointing only at that
//! directory, so nothing can reach the user's session, clipboard, config or
//! history. The socket is `$XDG_RUNTIME_DIR/spool/sock`, the DB and config
//! live in a separate tempdir. Drop guards kill every process, also on panic.
//!
//! PATH: the dev shell's PATH contains user-owned directories (e.g.
//! `~/.nix-profile/bin` resolving outside the store, `.direnv/bin`, the
//! per-user Flatpak exports), which spoold's PATH audit rightly rejects.
//! Instead of setting `SPOOL_INSECURE_PATH_OK=1`, spoold gets a PATH with
//! those entries filtered out; the dropped entries are printed once per test
//! run. (The picker tests put a symlink to the built `spool-picker` in a
//! temp dir on that PATH and therefore do set `SPOOL_INSECURE_PATH_OK=1`.)
//!
//! Key provider: tests run with `key_provider = "session"` (memory only)
//! unless they test persistence. Those (`*_secret_service`) additionally
//! need `dbus-daemon` and `gnome-keyring-daemon` on PATH (dev shell): each
//! starts a private `dbus-daemon` (listening only inside the test's data
//! dir, no service activation) and a throwaway `gnome-keyring-daemon` with a
//! fresh `login` keyring, and refuses to run if the bus address is not that
//! private one (same guard idea as
//! `crates/spool-keys/tests/secret-service-harness.sh`). The user's session
//! bus and keyring are never contacted.
//!
//! Observation: besides the public API (`status`, `current`), the tests wait
//! on spoold's own decision log lines (`--log-stderr`; they never contain
//! clipboard content) to know an offer was processed — e.g. that a secret
//! was dropped — without sleeping blindly.

#[path = "../../spoolctl/src/client.rs"]
#[allow(dead_code)]
mod client;

use std::fs::File;
use std::io::{BufRead, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use spool_proto::{PauseState, PublicReq, PublicResp, StatusInfo};

const T: Duration = Duration::from_secs(10);
const UTF8: &str = "text/plain;charset=utf-8";
const MARKER: &str = "application/x-spool-source";
const HINT: &str = "x-kde-passwordManagerHint";

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// Minimal valid 1x1 RGBA PNG.
const PNG: &[u8] = &[
  0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
  0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
  0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
  0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
  0x42, 0x60, 0x82,
];

// ---- gating -----------------------------------------------------------------

fn on_path(bin: &str) -> bool {
  std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

fn enabled() -> bool {
  if std::env::var_os("SPOOL_SKIP_WAYLAND_TESTS").is_some()
    || std::env::var("SPOOL_WAYLAND_TESTS").as_deref() != Ok("1")
  {
    eprintln!("skipping: set SPOOL_WAYLAND_TESTS=1 to run the end-to-end tests");
    return false;
  }
  for bin in ["sway", "wl-copy", "wl-paste"] {
    if !on_path(bin) {
      eprintln!("skipping: `{bin}` not on PATH (use the spool dev shell)");
      return false;
    }
  }
  true
}

/// PATH for spoold: the test PATH minus entries spoold's audit would reject
/// (not under /nix/store and not root-owned, or group/world-writable).
fn trusted_path() -> String {
  static REPORTED: std::sync::Once = std::sync::Once::new();
  let path = std::env::var_os("PATH").unwrap_or_default();
  let mut keep = Vec::new();
  let mut dropped = Vec::new();
  for dir in std::env::split_paths(&path) {
    let Ok(resolved) = std::fs::canonicalize(&dir) else { continue };
    let ok = resolved.starts_with("/nix/store")
      || std::fs::metadata(&resolved).is_ok_and(|m| m.uid() == 0 && m.mode() & 0o022 == 0);
    if ok && dir.is_absolute() {
      keep.push(dir);
    } else {
      dropped.push(dir);
    }
  }
  REPORTED.call_once(|| {
    eprintln!("e2e: PATH entries withheld from spoold (would fail its PATH audit): {dropped:?}");
  });
  std::env::join_paths(keep).unwrap().into_string().unwrap()
}

// ---- process helpers --------------------------------------------------------

fn wait_timeout(child: &mut Child, t: Duration) -> Option<ExitStatus> {
  let start = Instant::now();
  loop {
    if let Some(st) = child.try_wait().unwrap() {
      return Some(st);
    }
    if start.elapsed() > t {
      return None;
    }
    std::thread::sleep(Duration::from_millis(5));
  }
}

fn kill_child(child: &mut Child) {
  let _ = child.kill();
  let _ = child.wait();
}

fn sigterm(child: &Child) {
  // SAFETY: plain syscall on a pid we spawned and have not reaped yet.
  let r = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
  assert_eq!(r, 0, "kill(SIGTERM) failed");
}

/// SIGKILL any process of ours whose environment has
/// `XDG_RUNTIME_DIR=<rt>` (something that left the process group).
fn kill_stragglers(rt: &Path) {
  let want = format!("XDG_RUNTIME_DIR={}", rt.display()).into_bytes();
  let Ok(procs) = std::fs::read_dir("/proc") else { return };
  for e in procs.flatten() {
    let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
    let Ok(environ) = std::fs::read(e.path().join("environ")) else { continue };
    if environ.split(|b| *b == 0).any(|kv| kv == want.as_slice()) {
      // SAFETY: plain syscall.
      unsafe {
        libc::kill(pid, libc::SIGKILL);
      }
    }
  }
}

fn poll_until<T>(what: &str, t: Duration, mut f: impl FnMut() -> Option<T>) -> T {
  let start = Instant::now();
  loop {
    if let Some(v) = f() {
      return v;
    }
    assert!(start.elapsed() < t, "timed out after {t:?} waiting for {what}");
    std::thread::sleep(Duration::from_millis(5));
  }
}

// ---- the environment --------------------------------------------------------

/// A headless sway + (optionally) a spoold, in private directories.
struct Env {
  /// Private `XDG_RUNTIME_DIR` (0700): sway socket, spool socket, outputs.
  rt: tempfile::TempDir,
  /// DB, config, HOME, spoold logs.
  data: tempfile::TempDir,
  display: String,
  sway: Child,
  /// Process group of sway's wrapper; every child joins it.
  pgid: i32,
  spoold: Option<Child>,
  spoold_log: PathBuf,
  /// Foreground wl-copy processes and other children to reap on drop.
  extra: Vec<Child>,
  /// Address of the private D-Bus with a throwaway gnome-keyring, if any.
  dbus: Option<String>,
  /// Directory holding a `spool-picker` symlink, put first on spoold's
  /// PATH (with `SPOOL_INSECURE_PATH_OK=1`: the build dir is user-owned).
  picker_dir: Option<PathBuf>,
}

impl Env {
  /// Start sway and spoold with the default config (session key provider).
  fn start() -> Option<Env> {
    Self::start_with_config("")
  }

  /// `config` plus `key_provider = "session"` unless it sets a provider.
  fn start_with_config(config: &str) -> Option<Env> {
    let mut env = Self::sway()?;
    let config = if config.contains("key_provider") {
      config.to_owned()
    } else {
      format!("key_provider = \"session\"\n{config}")
    };
    std::fs::write(env.data.path().join("config.toml"), config).unwrap();
    env.start_spoold();
    Some(env)
  }

  /// Sway + private D-Bus + throwaway gnome-keyring (unlocked), and the
  /// default config (`secret-service`). spoold is not started yet.
  fn with_keyring() -> Option<Env> {
    if !enabled() {
      return None;
    }
    for bin in ["dbus-daemon", "gnome-keyring-daemon"] {
      if !on_path(bin) {
        eprintln!("skipping: `{bin}` not on PATH (use the spool dev shell)");
        return None;
      }
    }
    let mut env = Self::sway()?;
    std::fs::write(env.data.path().join("config.toml"), "").unwrap();
    env.start_keyring();
    Some(env)
  }

  fn start_keyring(&mut self) {
    let data = self.data.path().to_path_buf();
    let bus_dir = data.join("bus");
    let home = data.join("keyring-home");
    for d in [&bus_dir, &home] {
      std::fs::create_dir(d).unwrap();
      std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let conf = data.join("bus.conf");
    std::fs::write(
      &conf,
      format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
        bus_dir.display()
      ),
    )
    .unwrap();
    let mut bus = self
      .cmd("dbus-daemon")
      .arg(format!("--config-file={}", conf.display()))
      .args(["--nofork", "--print-address"])
      .stdin(Stdio::null())
      .stdout(Stdio::piped())
      .stderr(File::create(data.join("dbus.log")).unwrap())
      .spawn()
      .expect("spawn dbus-daemon");
    let mut addr = String::new();
    std::io::BufReader::new(bus.stdout.take().unwrap()).read_line(&mut addr).unwrap();
    self.extra.push(bus);
    let addr = addr.trim().to_owned();
    // Guard: only ever the private bus we just started.
    let parent = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
    assert!(!addr.is_empty(), "dbus-daemon printed no address");
    assert_ne!(addr, parent, "refusing to use the inherited (real) session bus");
    assert!(
      addr.starts_with("unix:") && addr.contains(&format!("{}/", bus_dir.display())),
      "refusing: bus {addr} is not inside {}",
      bus_dir.display()
    );
    let mut gk = self
      .cmd("gnome-keyring-daemon")
      .args(["--foreground", "--unlock", "--components=secrets"])
      .env("DBUS_SESSION_BUS_ADDRESS", &addr)
      .env("HOME", &home)
      .env("XDG_DATA_HOME", home.join("data"))
      .env("XDG_CONFIG_HOME", home.join("config"))
      .env("XDG_CACHE_HOME", home.join("cache"))
      .stdin(Stdio::piped())
      .stdout(Stdio::null())
      .stderr(File::create(data.join("gnome-keyring.log")).unwrap())
      .spawn()
      .expect("spawn gnome-keyring-daemon");
    gk.stdin.take().unwrap().write_all(KEYRING_PASSWORD.as_bytes()).unwrap();
    self.extra.push(gk);
    self.dbus = Some(addr);
    poll_until("gnome-keyring on the private bus", T, || {
      self
        .bus_call(|c| async move {
          let reply = c
            .call_method(
              Some("org.freedesktop.DBus"),
              "/org/freedesktop/DBus",
              Some("org.freedesktop.DBus"),
              "NameHasOwner",
              &("org.freedesktop.secrets"),
            )
            .await?;
          reply.body().deserialize::<bool>()
        })
        .ok()
        .filter(|owned| *owned)
    });
    poll_until("the default keyring", T, || self.keyring_locked().ok().filter(|l| !l));
  }

  /// Run `f` with a zbus connection to the private bus (never any other).
  fn bus_call<T, F, Fut>(&self, f: F) -> zbus::Result<T>
  where
    F: FnOnce(zbus::Connection) -> Fut,
    Fut: std::future::Future<Output = zbus::Result<T>>,
  {
    let addr = self.dbus.clone().expect("no private bus");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async move {
      let conn = zbus::connection::Builder::address(addr.as_str())?.build().await?;
      f(conn).await
    })
  }

  /// Object path of the keyring behind the `default` alias.
  fn default_collection(&self) -> zbus::Result<zbus::zvariant::OwnedObjectPath> {
    self.bus_call(|c| async move {
      let reply = c
        .call_method(
          Some("org.freedesktop.secrets"),
          "/org/freedesktop/secrets",
          Some("org.freedesktop.Secret.Service"),
          "ReadAlias",
          &("default"),
        )
        .await?;
      let path: zbus::zvariant::OwnedObjectPath = reply.body().deserialize()?;
      if path.as_str() == "/" {
        return Err(zbus::Error::Failure("no default collection yet".into()));
      }
      Ok(path)
    })
  }

  fn keyring_locked(&self) -> zbus::Result<bool> {
    let coll = self.default_collection()?;
    self.bus_call(|c| async move {
      let reply = c
        .call_method(
          Some("org.freedesktop.secrets"),
          coll.as_str(),
          Some("org.freedesktop.DBus.Properties"),
          "Get",
          &("org.freedesktop.Secret.Collection", "Locked"),
        )
        .await?;
      let v: zbus::zvariant::OwnedValue = reply.body().deserialize()?;
      Ok(bool::try_from(v)?)
    })
  }

  fn lock_keyring(&self) {
    let coll = self.default_collection().unwrap();
    self
      .bus_call(|c| async move {
        c.call_method(
          Some("org.freedesktop.secrets"),
          "/org/freedesktop/secrets",
          Some("org.freedesktop.Secret.Service"),
          "Lock",
          &(vec![coll]),
        )
        .await
        .map(|_| ())
      })
      .unwrap();
    assert!(self.keyring_locked().unwrap(), "keyring did not lock");
  }

  /// Unlock without a prompt via gnome-keyring's private
  /// `UnlockWithMasterPassword` (plain transfer session), like the
  /// spool-keys Secret Service test.
  fn unlock_keyring(&self) {
    let coll = self.default_collection().unwrap();
    self
      .bus_call(|c| async move {
        let reply = c
          .call_method(
            Some("org.freedesktop.secrets"),
            "/org/freedesktop/secrets",
            Some("org.freedesktop.Secret.Service"),
            "OpenSession",
            &("plain", zbus::zvariant::Value::from("")),
          )
          .await?;
        let (_out, session): (zbus::zvariant::OwnedValue, zbus::zvariant::OwnedObjectPath) =
          reply.body().deserialize()?;
        let secret =
          (session, Vec::<u8>::new(), KEYRING_PASSWORD.as_bytes().to_vec(), "text/plain");
        c.call_method(
          Some("org.freedesktop.secrets"),
          "/org/freedesktop/secrets",
          Some("org.gnome.keyring.InternalUnsupportedGuiltRiddenInterface"),
          "UnlockWithMasterPassword",
          &(coll, secret),
        )
        .await
        .map(|_| ())
      })
      .unwrap();
    assert!(!self.keyring_locked().unwrap(), "keyring did not unlock");
  }

  fn state_dir(&self) -> PathBuf {
    self.data.path().join("state")
  }

  fn sway() -> Option<Env> {
    if !enabled() {
      return None;
    }
    let rt = tempfile::Builder::new().prefix("spool-e2e-rt").tempdir().unwrap();
    std::fs::set_permissions(rt.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let data = tempfile::Builder::new().prefix("spool-e2e-data").tempdir().unwrap();
    std::fs::set_permissions(data.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let log = File::create(data.path().join("sway.log")).unwrap();
    let sway = Command::new("sway")
      .args(["-c", "/dev/null"])
      .env_clear()
      .env("PATH", std::env::var_os("PATH").unwrap_or_default())
      .env("HOME", data.path())
      .env("XDG_RUNTIME_DIR", rt.path())
      .env("WLR_BACKENDS", "headless")
      .env("WLR_LIBINPUT_NO_DEVICES", "1")
      .env("WLR_RENDERER", "pixman")
      // nixpkgs' `sway` is a `dbus-run-session` wrapper: its own process
      // group lets Drop kill dbus-daemon, sway and swaybg too.
      .process_group(0)
      .stdin(Stdio::null())
      .stdout(log.try_clone().unwrap())
      .stderr(log)
      .spawn()
      .expect("spawn sway");
    let spoold_log = data.path().join("spoold.log");
    let pgid = sway.id() as i32;
    let mut env = Env {
      rt,
      data,
      display: String::new(),
      sway,
      pgid,
      spoold: None,
      spoold_log,
      extra: Vec::new(),
      dbus: None,
      picker_dir: None,
    };
    let start = Instant::now();
    env.display = loop {
      let found = std::fs::read_dir(env.rt.path()).unwrap().flatten().find_map(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        (n.starts_with("wayland-") && !n.ends_with(".lock")).then_some(n)
      });
      if let Some(n) = found {
        break n;
      }
      if let Ok(Some(st)) = env.sway.try_wait() {
        panic!("sway exited early ({st}); log:\n{}", env.sway_log());
      }
      assert!(start.elapsed() < T, "sway socket never appeared; log:\n{}", env.sway_log());
      std::thread::sleep(Duration::from_millis(20));
    };
    // The socket exists slightly before sway accepts clients; wait until a
    // client round-trip works.
    poll_until("sway to accept clients", T, || {
      let out = env.cmd("wl-paste").arg("--list-types").output().ok()?;
      // Exit 1 + "Nothing is copied" also proves the connection works.
      (out.status.success() || String::from_utf8_lossy(&out.stderr).contains("Nothing"))
        .then_some(())
    });
    Some(env)
  }

  fn sway_log(&self) -> String {
    std::fs::read_to_string(self.data.path().join("sway.log")).unwrap_or_default()
  }

  fn socket(&self) -> PathBuf {
    self.rt.path().join("spool").join("sock")
  }

  /// A command with a cleared environment pointing only at the private
  /// directories, in sway's process group (so backgrounded wl-copy servers
  /// die with it).
  fn cmd(&self, bin: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(bin);
    c.process_group(self.pgid)
      .env_clear()
      .env("PATH", std::env::var_os("PATH").unwrap_or_default())
      .env("HOME", self.data.path())
      .env("XDG_RUNTIME_DIR", self.rt.path())
      .env("WAYLAND_DISPLAY", &self.display)
      .env("SPOOL_SOCKET", self.socket());
    c
  }

  fn spoold_cmd(&self, log: &Path) -> Command {
    let log = File::create(log).unwrap();
    let mut c = self.cmd(env!("CARGO_BIN_EXE_spoold"));
    let path = match &self.picker_dir {
      Some(d) => format!("{}:{}", d.display(), trusted_path()),
      None => trusted_path(),
    };
    c.arg("--log-stderr")
      .env("PATH", path)
      .env("SPOOL_STATE_DIR", self.state_dir())
      .env("SPOOL_CONFIG", self.data.path().join("config.toml"))
      .env("RUST_LOG", "info,spoold=debug")
      .env("NO_COLOR", "1")
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(log);
    if let Some(addr) = &self.dbus {
      c.env("DBUS_SESSION_BUS_ADDRESS", addr);
    }
    if self.picker_dir.is_some() {
      // The picker symlink lives in a user-owned temp dir.
      c.env("SPOOL_INSECURE_PATH_OK", "1")
        .env("XDG_CONFIG_HOME", self.data.path().join("xdg-config"))
        .env("LANG", "C.UTF-8");
    }
    // Only a `--features test-hooks` build of spoold reads it.
    c.env("SPOOL_TEST_HOOK_SOCKET", self.hook_socket());
    c
  }

  fn hook_socket(&self) -> PathBuf {
    self.data.path().join("hook.sock")
  }

  /// Previews of spoold's search hits for `q`, through the test-only hook
  /// socket (the same in-daemon search API the picker uses; search is
  /// deliberately not on the public socket).
  #[cfg(feature = "test-hooks")]
  fn search(&self, q: &str) -> Vec<String> {
    let mut s = std::os::unix::net::UnixStream::connect(self.hook_socket()).unwrap();
    spool_proto::write_frame(&mut s, &(q.to_string(), 50u32)).unwrap();
    let r: Result<Vec<spool_proto::ItemPreview>, String> = spool_proto::read_frame(&mut s).unwrap();
    r.unwrap().into_iter().map(|p| p.preview).collect()
  }

  /// Wait until searching `q` finds an item whose preview starts with
  /// `prefix`, and only items whose previews start with `q` (so the
  /// recent-list fallback used while the index opens never passes; the
  /// index also commits in batches).
  #[cfg(feature = "test-hooks")]
  fn expect_search(&self, q: &str, prefix: &str) {
    poll_until("search to find the item", KEY_T, || {
      let hits = self.search(q);
      (hits.iter().any(|p| p.starts_with(prefix)) && hits.iter().all(|p| p.starts_with(q)))
        .then_some(())
    });
  }

  #[cfg(not(feature = "test-hooks"))]
  fn expect_search(&self, _q: &str, _prefix: &str) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| eprintln!("search checks skipped: run with `--features test-hooks`"));
  }

  fn start_spoold(&mut self) {
    let child = self.spoold_cmd(&self.spoold_log).spawn().expect("spawn spoold");
    self.spoold = Some(child);
    // Ready + the compositor's initial state replay processed.
    poll_until("spoold to come up", T, || {
      if let Some(st) = self.spoold.as_mut().unwrap().try_wait().unwrap() {
        panic!("spoold exited early ({st}); log:\n{}", self.log());
      }
      self.try_status().filter(|s| s.compositor.is_some())
    });
    // ... and the compositor's replay of the current selection handled:
    // empty clipboard, or an existing offer picked up.
    poll_until("spoold to process the initial selection", T, || {
      let log = self.log();
      ["nothing to keep alive", "new offer"].iter().any(|n| log.contains(n)).then_some(())
    });
  }

  /// SIGTERM spoold, check it exits 0 and removes its socket.
  fn stop_spoold(&mut self) {
    let mut d = self.spoold.take().unwrap();
    sigterm(&d);
    let st = wait_timeout(&mut d, T).unwrap_or_else(|| {
      kill_child(&mut d);
      panic!("spoold ignored SIGTERM; log:\n{}", self.log())
    });
    assert_eq!(st.code(), Some(0), "log:\n{}", self.log());
    assert!(!self.socket().exists(), "socket left behind");
  }

  /// Poll `Status` until `key_state` satisfies `pred`.
  fn wait_key_state(&self, t: Duration, pred: impl Fn(&str) -> bool) -> StatusInfo {
    let mut last = String::new();
    let start = Instant::now();
    loop {
      let st = self.status();
      if pred(&st.key_state) {
        return st;
      }
      if st.key_state != last {
        eprintln!("[{:>7.3?}] key_state = {}", start.elapsed(), st.key_state);
        last = st.key_state.clone();
      }
      assert!(start.elapsed() < t, "stuck in key_state {last:?}; log:\n{}", self.log());
      std::thread::sleep(Duration::from_millis(20));
    }
  }

  fn log(&self) -> String {
    std::fs::read_to_string(&self.spoold_log).unwrap_or_default()
  }

  fn log_count(&self, needle: &str) -> usize {
    self.log().matches(needle).count()
  }

  /// Wait until spoold's log contains `needle` at least `n` times.
  fn wait_log(&self, needle: &str, n: usize) {
    let start = Instant::now();
    while self.log_count(needle) < n {
      assert!(
        start.elapsed() < T,
        "timed out waiting for {n}x {needle:?} in the spoold log:\n{}",
        self.log()
      );
      std::thread::sleep(Duration::from_millis(5));
    }
  }

  /// Run `action`, then wait for one more `needle` in the log.
  fn expect_log(&self, needle: &str, action: impl FnOnce()) {
    let n = self.log_count(needle);
    action();
    self.wait_log(needle, n + 1);
  }

  // -- public API (in-process client, same code as spoolctl) --

  fn request(&self, req: PublicReq) -> anyhow::Result<PublicResp> {
    client::Client::connect_to(&self.socket())?.request(&req)
  }

  fn try_status(&self) -> Option<StatusInfo> {
    match self.request(PublicReq::Status) {
      Ok(PublicResp::Status(s)) => Some(s),
      _ => None,
    }
  }

  fn status(&self) -> StatusInfo {
    match self.request(PublicReq::Status).unwrap() {
      PublicResp::Status(s) => s,
      other => panic!("unexpected {other:?}"),
    }
  }

  fn count(&self) -> u64 {
    self.status().item_count
  }

  fn current(&self) -> Option<(String, Vec<u8>)> {
    match self.request(PublicReq::Current).unwrap() {
      PublicResp::Current { mime, data } => Some((mime, data)),
      PublicResp::Empty => None,
      other => panic!("unexpected {other:?}"),
    }
  }

  // -- the spoolctl binary --

  fn spoolctl(&self, args: &[&str], stdin: &[u8]) -> (ExitStatus, Vec<u8>, String) {
    let bin = Path::new(env!("CARGO_BIN_EXE_spoold")).with_file_name("spoolctl");
    assert!(
      bin.is_file(),
      "{} not built; run `cargo test --workspace` (or `cargo build -p spoolctl`)",
      bin.display()
    );
    let (outp, outf) = self.out_file("spoolctl-out");
    let (errp, errf) = self.out_file("spoolctl-err");
    let mut child =
      self.cmd(&bin).args(args).stdin(Stdio::piped()).stdout(outf).stderr(errf).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let st = wait_timeout(&mut child, T).unwrap_or_else(|| {
      kill_child(&mut child);
      panic!("spoolctl {args:?} timed out")
    });
    (st, std::fs::read(outp).unwrap(), std::fs::read_to_string(errp).unwrap())
  }

  fn spoolctl_ok(&self, args: &[&str], stdin: &[u8]) -> Vec<u8> {
    let (st, out, err) = self.spoolctl(args, stdin);
    assert!(st.success(), "spoolctl {args:?} failed ({st}): {err}");
    out
  }

  // -- wl-clipboard --

  /// File for a child's output. Files, not pipes: a backgrounded wl-copy
  /// inherits its stdio, so a pipe would never reach EOF.
  fn out_file(&self, tag: &str) -> (PathBuf, File) {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let p = self.data.path().join(format!("{tag}-{n}.out"));
    let f = File::create(&p).unwrap();
    (p, f)
  }

  /// `wl-copy ARGS < data`; returns once the wl-copy parent has exited
  /// (selection set, server forked into the background).
  fn wl_copy(&self, args: &[&str], data: &[u8]) {
    let (errp, errf) = self.out_file("wl-copy-err");
    let mut child = self
      .cmd("wl-copy")
      .args(args)
      .stdin(Stdio::piped())
      .stdout(Stdio::null())
      .stderr(errf)
      .spawn()
      .unwrap();
    child.stdin.take().unwrap().write_all(data).unwrap();
    let st = wait_timeout(&mut child, T).unwrap_or_else(|| {
      kill_child(&mut child);
      panic!("wl-copy timed out")
    });
    assert!(st.success(), "wl-copy failed: {}", std::fs::read_to_string(errp).unwrap_or_default());
  }

  /// `wl-copy --foreground ARGS < data`; the child is returned (and also
  /// reaped on drop if the caller leaks it).
  fn wl_copy_fg(&mut self, args: &[&str], data: &[u8]) -> usize {
    let mut child = self
      .cmd("wl-copy")
      .arg("--foreground")
      .args(args)
      .stdin(Stdio::piped())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .spawn()
      .unwrap();
    child.stdin.take().unwrap().write_all(data).unwrap();
    self.extra.push(child);
    self.extra.len() - 1
  }

  fn kill_extra(&mut self, idx: usize) {
    kill_child(&mut self.extra[idx]);
  }

  /// `wl-paste ARGS`: `Ok(stdout)` on success, `Err(stderr)` otherwise.
  fn wl_paste(&self, args: &[&str]) -> Result<Vec<u8>, String> {
    let (outp, outf) = self.out_file("wl-paste-out");
    let (errp, errf) = self.out_file("wl-paste-err");
    let mut child = self
      .cmd("wl-paste")
      .args(args)
      .stdin(Stdio::null())
      .stdout(outf)
      .stderr(errf)
      .spawn()
      .unwrap();
    let st = wait_timeout(&mut child, T).unwrap_or_else(|| {
      kill_child(&mut child);
      panic!("wl-paste timed out")
    });
    if st.success() {
      Ok(std::fs::read(outp).unwrap())
    } else {
      Err(std::fs::read_to_string(errp).unwrap_or_default())
    }
  }

  /// Copy text with wl-copy and wait until spoold stored it as a new item.
  fn copy_stored(&self, text: &str) {
    let before = self.count();
    self.expect_log("stored (inserted)", || self.wl_copy(&[], text.as_bytes()));
    assert_eq!(self.count(), before + 1);
  }

  /// A second Wayland client in the test process (spool-wayland), for
  /// offers wl-copy cannot make (several mimes at once).
  fn test_client(&self) -> TestClient {
    let socket = self.rt.path().join(&self.display);
    let (handle, events) = spool_wayland::spawn(spool_wayland::WaylandConfig {
      display: Some(socket.to_string_lossy().into_owned()),
      watch_primary: false,
    })
    .expect("test client connects to the private sway");
    TestClient { handle, _events: events }
  }
}

/// The resident picker, real `spool-picker` binary (built next to spoold).
impl Env {
  /// Put the built `spool-picker` on spoold's PATH (before `start_spoold`).
  /// `false` (test skipped) if it is not built.
  fn enable_picker(&mut self) -> bool {
    let bin = Path::new(env!("CARGO_BIN_EXE_spoold")).with_file_name("spool-picker");
    if !bin.is_file() {
      eprintln!("skipping: {} not built (cargo build -p spool-picker)", bin.display());
      return false;
    }
    for tool in ["grim", "wtype"] {
      if !on_path(tool) {
        eprintln!("skipping: `{tool}` not on PATH (use the spool dev shell)");
        return false;
      }
    }
    let dir = self.data.path().join("picker-bin");
    std::fs::create_dir(&dir).unwrap();
    std::os::unix::fs::symlink(&bin, dir.join("spool-picker")).unwrap();
    std::fs::create_dir(self.data.path().join("xdg-config")).unwrap();
    self.picker_dir = Some(dir);
    true
  }

  /// The output's pixels right now (a grim PNG; same pixels, same bytes).
  fn grab(&self) -> Vec<u8> {
    let p = self.data.path().join("probe.png");
    let st = self.cmd("grim").arg(&p).stdout(Stdio::null()).stderr(Stdio::null()).status();
    assert!(st.unwrap().success(), "grim failed");
    std::fs::read(p).unwrap()
  }

  /// Wait until the screen differs from `before` (the picker mapped or
  /// unmapped; release builds of the picker have no other "presented"
  /// signal), then a moment for the frame to settle.
  fn wait_screen_change(&self, before: &[u8]) {
    poll_until("the screen to change (picker shown / hidden)", T, || {
      (self.grab() != before).then_some(())
    });
    std::thread::sleep(Duration::from_millis(200));
  }

  /// `wtype ARGS` (its virtual keyboard types into the focused surface).
  fn wtype(&self, args: &[&str]) {
    let st = self.cmd("wtype").args(args).stdout(Stdio::null()).stderr(Stdio::null()).status();
    assert!(st.unwrap().success(), "wtype {args:?} failed");
  }

  /// Screenshot of the headless output; copied to `$SPOOL_E2E_SHOTS` (if
  /// set) for a human to look at.
  fn screenshot(&self, name: &str) -> PathBuf {
    let p = self.data.path().join(format!("{name}.png"));
    let st = self.cmd("grim").arg(&p).stdout(Stdio::null()).stderr(Stdio::null()).status();
    assert!(st.unwrap().success(), "grim failed");
    let len = std::fs::metadata(&p).unwrap().len();
    assert!(len > 1000, "screenshot is suspiciously small ({len} bytes)");
    if let Some(dir) = std::env::var_os("SPOOL_E2E_SHOTS") {
      let _ = std::fs::copy(&p, Path::new(&dir).join(format!("{name}.png")));
    }
    p
  }

  /// `wev` (a focused toplevel logging key events); returns its lines.
  fn spawn_wev(&mut self) -> Arc<std::sync::Mutex<Vec<String>>> {
    let mut child = self
      .cmd("stdbuf")
      .args(["-oL", "wev", "-f", "wl_keyboard:key", "-f", "wl_keyboard:enter"])
      .stdin(Stdio::null())
      .stdout(Stdio::piped())
      .stderr(Stdio::null())
      .spawn()
      .expect("spawn wev");
    let out = child.stdout.take().unwrap();
    self.extra.push(child);
    let lines = Arc::new(std::sync::Mutex::new(Vec::new()));
    let l2 = lines.clone();
    std::thread::spawn(move || {
      for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
        l2.lock().unwrap().push(line);
      }
    });
    poll_until("wev to get keyboard focus", T, || {
      lines.lock().unwrap().iter().any(|l| l.contains("enter")).then_some(())
    });
    lines
  }
}

impl Drop for Env {
  fn drop(&mut self) {
    if let Some(c) = self.spoold.as_mut() {
      kill_child(c);
    }
    for c in &mut self.extra {
      kill_child(c);
    }
    // The whole group: sway's wrapper, dbus-daemon, sway, swaybg and
    // backgrounded wl-copy servers (which outlive their compositor).
    // SAFETY: plain syscall; the group is ours (created by process_group(0)).
    unsafe {
      libc::killpg(self.pgid, libc::SIGKILL);
    }
    kill_child(&mut self.sway);
    kill_stragglers(self.rt.path());
    if std::thread::panicking() {
      eprintln!("---- spoold log ----\n{}\n---- sway log ----\n{}", self.log(), self.sway_log());
    }
  }
}

struct TestClient {
  handle: spool_wayland::WaylandHandle,
  // Kept so the Wayland thread's event sends never fail; never read.
  _events: crossbeam_channel::Receiver<spool_wayland::WaylandEvent>,
}

impl TestClient {
  fn offer(&self, reps: &[(&str, &[u8])]) {
    let reps = reps.iter().map(|(m, d)| (m.to_string(), Arc::<[u8]>::from(*d))).collect();
    self.handle.set_selection(spool_wayland::Selection::Clipboard, reps).unwrap();
  }
}

impl Drop for TestClient {
  fn drop(&mut self) {
    self.handle.shutdown();
  }
}

fn text(c: Option<(String, Vec<u8>)>) -> String {
  let (_, d) = c.expect("current item");
  String::from_utf8(d).unwrap()
}

const KEYRING_PASSWORD: &str = "spool-e2e-keyring-password";

/// Timeout for key-state transitions: keyring calls and the first encrypted
/// open (fsyncs) can take seconds on a loaded machine, and `wait_for_unlock`
/// may fall back to polling (backoff up to 30 s) when the keyring does not
/// signal the unlock.
const KEY_T: Duration = Duration::from_secs(60);

/// Every file under `dir` (recursively) that contains `needle`.
fn files_containing(dir: &Path, needle: &[u8]) -> Vec<PathBuf> {
  let mut hits = Vec::new();
  let mut stack = vec![dir.to_path_buf()];
  while let Some(d) = stack.pop() {
    for e in std::fs::read_dir(&d).unwrap().flatten() {
      let p = e.path();
      let ft = e.file_type().unwrap();
      if ft.is_dir() {
        stack.push(p);
      } else if ft.is_file() {
        let data = std::fs::read(&p).unwrap_or_default();
        if data.windows(needle.len()).any(|w| w == needle) {
          hits.push(p);
        }
      }
    }
  }
  hits
}

/// A marker unique to this test run (letters and digits only: must not
/// look like a secret to the ingest policy).
fn unique_marker(tag: &str) -> String {
  let nanos =
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
  format!("spoolmarker {tag} {} {nanos}", std::process::id())
}

// Fake secrets: syntactically valid, never real.
fn fake_github_token() -> String {
  // `ghp_` + 36 alphanumerics.
  "ghp_aB3dE6gH9jK2mN5pQ8sT1vW4yZ7bC0eF3hJ6".to_string()
}

const FAKE_PEM: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\n\
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\n\
-----END OPENSSH PRIVATE KEY-----\n";

// ---- scenarios --------------------------------------------------------------

#[test]
fn text_copy_is_stored_and_deduped() {
  let Some(env) = Env::start() else { return };
  assert_eq!(env.count(), 0);
  assert_eq!(env.current(), None);
  let (st, _, _) = env.spoolctl(&["current"], b"");
  assert_eq!(st.code(), Some(3), "empty history is exit 3");

  env.copy_stored("hello spool");
  assert_eq!(env.spoolctl_ok(&["current"], b""), b"hello spool");
  let json = String::from_utf8(env.spoolctl_ok(&["status", "--json"], b"")).unwrap();
  assert!(json.contains("\"item_count\":1"), "{json}");
  assert!(json.contains("ext_data_control_manager_v1"), "{json}");

  // Same text again: dedupe bump, no new item.
  env.expect_log("stored (bumped)", || env.wl_copy(&[], b"hello spool"));
  assert_eq!(env.count(), 1);

  env.copy_stored("second");
  assert_eq!(env.count(), 2);
  assert_eq!(text(env.current()), "second");
}

#[test]
fn secrets_are_not_stored_nor_kept_alive() {
  let Some(mut env) = Env::start() else { return };
  env.copy_stored("harmless");

  for secret in [fake_github_token(), FAKE_PEM.to_string()] {
    env.expect_log("offer dropped", || env.wl_copy(&[], secret.as_bytes()));
    assert_eq!(env.count(), 1);
    assert_eq!(text(env.current()), "harmless");
  }

  // A secret whose source exits: keep-alive must not re-publish it (nor
  // resurrect the older item over it).
  let token = fake_github_token();
  let n = env.log_count("offer dropped");
  let src = env.wl_copy_fg(&[], token.as_bytes());
  env.wait_log("offer dropped", n + 1);
  let n = env.log_count("nothing to keep alive");
  env.kill_extra(src);
  env.wait_log("nothing to keep alive", n + 1);
  assert_eq!(env.log_count("keep-alive: re-published"), 0);
  let pasted = env.wl_paste(&["-n"]);
  assert!(pasted.is_err(), "clipboard must be empty after the secret's source exited");
  assert_eq!(env.count(), 1);
}

#[test]
fn password_manager_hint_is_honoured() {
  let Some(env) = Env::start() else { return };
  let client = env.test_client();

  // Control: a hint that is not "secret" is stored normally.
  client.offer(&[(HINT, b"public"), (UTF8, b"not a password")]);
  env.wait_log("stored (inserted)", 1);
  assert_eq!(env.count(), 1);

  // KeePassXC-style secret hint: never stored.
  env.expect_log("PasswordManagerHint", || {
    client.offer(&[(HINT, b"secret"), (UTF8, b"correct horse battery staple")]);
  });
  assert_eq!(env.count(), 1);
  assert_eq!(text(env.current()), "not a password");
}

#[test]
fn clearing_the_clipboard_purges_the_previous_copy() {
  let Some(env) = Env::start() else { return };
  env.copy_stored("keep me");
  env.copy_stored("oops, my password in plain text");
  assert_eq!(env.count(), 2);

  // An empty copy within CLEAR_WINDOW purges the previous item.
  env.expect_log("purged previous item", || env.wl_copy(&[], b""));
  assert_eq!(env.count(), 1);
  assert_eq!(text(env.current()), "keep me");
}

#[test]
fn keep_alive_republishes_when_the_source_exits() {
  let Some(mut env) = Env::start() else { return };
  let before = env.log_count("stored (inserted)");
  let src = env.wl_copy_fg(&[], b"hello");
  env.wait_log("stored (inserted)", before + 1);

  let n = env.log_count("keep-alive: re-published");
  env.kill_extra(src);
  env.wait_log("keep-alive: re-published", n + 1);
  assert_eq!(env.wl_paste(&["-n"]).unwrap(), b"hello");
  let types = String::from_utf8(env.wl_paste(&["--list-types"]).unwrap()).unwrap();
  assert!(types.lines().any(|l| l.starts_with(MARKER)), "marker missing from {types:?}");
  assert!(types.lines().any(|l| l == UTF8), "{types:?}");
  // Our own re-publish is not recorded again.
  assert_eq!(env.count(), 1);
}

#[test]
fn spoolctl_copy_publishes_and_stores() {
  let Some(env) = Env::start() else { return };
  env.spoolctl_ok(&["copy"], b"from spoolctl\n");
  assert_eq!(env.wl_paste(&["-n"]).unwrap(), b"from spoolctl\n");
  assert_eq!(env.count(), 1);
  assert_eq!(text(env.current()), "from spoolctl\n");
  let types = String::from_utf8(env.wl_paste(&["--list-types"]).unwrap()).unwrap();
  assert!(types.lines().any(|l| l == "UTF8_STRING"), "{types:?}");

  // A secret: on the clipboard (the user asked), but not stored.
  let token = fake_github_token();
  env.expect_log("copy of a secret", || {
    env.spoolctl_ok(&["copy"], token.as_bytes());
  });
  assert_eq!(env.wl_paste(&["-n"]).unwrap(), token.as_bytes());
  assert_eq!(env.wl_paste(&["-n", "--type", "UTF8_STRING"]).unwrap(), token.as_bytes());
  assert_eq!(env.count(), 1);
  assert_eq!(text(env.current()), "from spoolctl\n");

  // A paste reader that goes away mid-transfer (EPIPE on spoold's writer)
  // must not hurt the daemon (SIGPIPE stays ignored).
  let big = vec![b'x'; 4 * 1024 * 1024];
  env.spoolctl_ok(&["copy"], &big);
  let mut reader = env
    .cmd("wl-paste")
    .args(["-n"])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
    .unwrap();
  drop(reader.stdout.take());
  let _ = wait_timeout(&mut reader, T);
  kill_child(&mut reader);
  assert_eq!(env.wl_paste(&["-n"]).unwrap().len(), big.len());
  assert_eq!(env.count(), 2);
}

#[test]
fn pause_and_resume() {
  let Some(env) = Env::start() else { return };
  env.spoolctl_ok(&["pause"], b"");
  assert_eq!(env.status().paused, PauseState::PausedIndefinitely);
  env.expect_log("offer skipped", || env.wl_copy(&[], b"while paused"));
  assert_eq!(env.count(), 0);

  env.spoolctl_ok(&["pause", "--for", "60"], b"");
  assert!(matches!(env.status().paused, PauseState::PausedUntil { .. }));
  let human = String::from_utf8(env.spoolctl_ok(&["status"], b"")).unwrap();
  assert!(human.contains("s left"), "{human}");

  env.spoolctl_ok(&["resume"], b"");
  assert_eq!(env.status().paused, PauseState::Recording);
  env.copy_stored("after resume");
  assert_eq!(text(env.current()), "after resume");
}

#[test]
fn primary_selection_ignored_by_default() {
  let Some(env) = Env::start() else { return };
  assert!(!env.status().primary_enabled);
  // Primary first, then a clipboard copy as a barrier (compositor order).
  env.wl_copy(&["--primary"], b"middle click");
  env.copy_stored("ctrl c");
  assert_eq!(env.count(), 1);
  assert_eq!(text(env.current()), "ctrl c");
}

#[test]
fn primary_selection_recorded_when_enabled() {
  let Some(env) = Env::start_with_config("primary_selection = true\n") else { return };
  assert!(env.status().primary_enabled);
  env.expect_log("stored (inserted)", || env.wl_copy(&["--primary"], b"middle click"));
  assert_eq!(env.count(), 1);
  // `current` is the clipboard only.
  assert_eq!(env.current(), None);
}

#[test]
fn second_daemon_refuses_to_start() {
  let Some(env) = Env::start() else { return };
  let log = env.data.path().join("spoold-2.log");
  let mut second = env.spoold_cmd(&log).spawn().unwrap();
  let st = wait_timeout(&mut second, T).unwrap_or_else(|| {
    kill_child(&mut second);
    panic!("second spoold kept running")
  });
  assert!(!st.success());
  let err = std::fs::read_to_string(&log).unwrap();
  assert!(err.contains("already"), "{err}");
  // The first one is unaffected.
  assert!(env.socket().exists());
  env.copy_stored("still works");
}

#[test]
fn spool_keyctl_refuses_while_spoold_runs() {
  let Some(env) = Env::start() else { return };
  let keyctl = Path::new(env!("CARGO_BIN_EXE_spoold")).with_file_name("spool-keyctl");
  if !keyctl.is_file() {
    eprintln!("skipping: build spool-keyctl first (cargo build -p spool-keyctl)");
    return;
  }
  // Another socket path: only the state-directory lock can tell.
  let out = env
    .cmd(&keyctl)
    .args(["add", "secret-service"])
    .env("SPOOL_STATE_DIR", env.state_dir())
    .env("SPOOL_SOCKET", env.data.path().join("other.sock"))
    .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
    .stdin(Stdio::null())
    .output()
    .unwrap();
  let err = String::from_utf8_lossy(&out.stderr);
  assert_eq!(out.status.code(), Some(3), "{err}");
  assert!(err.contains("state.lock"), "{err}");
  // spoold is unaffected.
  env.copy_stored("still works");
}

#[test]
fn sigterm_exits_cleanly() {
  let Some(mut env) = Env::start() else { return };
  let st = env.status();
  assert_eq!(st.key_state, "session-only");
  assert!(!st.encrypted && !st.unlocked);
  // From spoold itself, so the clipboard is empty once it exits (a wl-copy
  // source would still offer it to the next spoold).
  env.spoolctl_ok(&["copy"], b"before shutdown");
  assert_eq!(env.count(), 1);
  env.stop_spoold();

  // key_provider = "session": nothing is persisted, nothing on disk.
  env.start_spoold();
  assert!(env.wl_paste(&["-n"]).is_err(), "clipboard should be empty");
  assert_eq!(env.count(), 0);
  assert!(!env.state_dir().join("history.db").exists());
  assert!(!env.state_dir().join("keyslots.json").exists());
}

/// M5/M7 desktop integration on sway: no session bus (so no KWin script or
/// portal: `spoolctl show` is the hotkey), focus from wlr foreign-toplevel,
/// auto-paste through the `spool-paster` helper's virtual keyboard (when
/// built next to spoold).
#[test]
fn desktop_capabilities_on_sway() {
  let Some(env) = Env::start() else { return };
  let caps = env.status().capabilities.expect("capabilities");
  assert_eq!(caps.hotkey, "external", "{caps:?}");
  assert_eq!(caps.focus, "wlr-foreign-toplevel", "{caps:?}");
  assert!(!caps.cursor);
  let helper = Path::new(env!("CARGO_BIN_EXE_spoold")).with_file_name("spool-paster");
  if helper.is_file() {
    assert_eq!(caps.paste_backend, "virtual-keyboard", "{caps:?}");
    assert!(caps.auto_paste);
  } else {
    eprintln!("spool-paster not built next to spoold; auto-paste not checked");
    assert_eq!(caps.paste_backend, "none");
  }
  // No spool-picker on spoold's PATH: Show is accepted by the rate limiter
  // and answered as not available.
  let r = env.request(PublicReq::Show).unwrap();
  assert_eq!(r, PublicResp::NotYetImplemented);
}

// ---- persistence (Secret Service) --------------------------------------------

/// Encrypted persistence end to end: first run creates the key slot in the
/// (throwaway) keyring; small and > 1 MiB (blob file) items survive a
/// restart; keep-alive serves the blob through wl-paste; nothing on disk
/// contains the copied plaintext.
#[test]
fn history_persists_encrypted_with_secret_service() {
  let Some(mut env) = Env::with_keyring() else { return };
  env.start_spoold();
  let st = env.wait_key_state(KEY_T, |s| s == "ready");
  assert!(st.encrypted && st.unlocked);
  let state = env.state_dir();
  assert_eq!(std::fs::metadata(&state).unwrap().mode() & 0o777, 0o700);
  assert!(state.join("keyslots.json").is_file());
  assert!(state.join("history.db").is_file());

  let small = unique_marker("small");
  env.copy_stored(&small);

  // > INLINE_MAX (1 MiB): goes to an encrypted blob file.
  let big_marker = unique_marker("blob");
  let mut big = String::new();
  let mut i = 0;
  while big.len() < 1536 * 1024 {
    big.push_str(&format!("{big_marker} line {i:08}\n"));
    i += 1;
  }
  let before = env.log_count("stored (inserted)");
  let src = env.wl_copy_fg(&[], big.as_bytes());
  env.wait_log("stored (inserted)", before + 1);
  assert_eq!(env.count(), 2);
  let blobs: Vec<_> = std::fs::read_dir(state.join("blobs")).unwrap().flatten().collect();
  assert_eq!(blobs.len(), 1, "expected one blob file");

  // Keep-alive re-publishes the large item from its blob file.
  let n = env.log_count("keep-alive: re-published");
  env.kill_extra(src);
  env.wait_log("keep-alive: re-published", n + 1);
  assert!(env.wl_paste(&["-n"]).unwrap() == big.as_bytes(), "pasted blob differs");

  // Both are searchable (the large one through its blob file).
  env.expect_search(&small, &small);
  env.expect_search(&big_marker, &big_marker);

  // No plaintext anywhere in the state dir (DB, WAL, blobs, slots, the
  // search index).
  for needle in [small.as_bytes(), big_marker.as_bytes()] {
    let hits = files_containing(&state, needle);
    assert!(hits.is_empty(), "plaintext marker found in {hits:?}");
  }

  // Restart: the history comes back from the encrypted store.
  env.stop_spoold();
  for needle in [small.as_bytes(), big_marker.as_bytes()] {
    let hits = files_containing(&state, needle);
    assert!(hits.is_empty(), "plaintext marker found after shutdown in {hits:?}");
  }
  env.start_spoold();
  let st = env.wait_key_state(KEY_T, |s| s == "ready");
  assert_eq!(st.item_count, 2);
  assert!(env.spoolctl_ok(&["current"], b"") == big.as_bytes(), "current is not the blob");
  let json = String::from_utf8(env.spoolctl_ok(&["status", "--json"], b"")).unwrap();
  assert!(
    json.contains("\"encrypted\":true") && json.contains("\"key_state\":\"ready\""),
    "{json}"
  );
  // Search works after the restart, from the encrypted on-disk index.
  env.expect_search(&small, &small);
  env.expect_search(&big_marker, &big_marker);
  #[cfg(feature = "test-hooks")]
  assert!(state.join("index").join("meta.json").is_file());
  env.stop_spoold();
  for needle in [small.as_bytes(), big_marker.as_bytes()] {
    let hits = files_containing(&state, needle);
    assert!(hits.is_empty(), "plaintext marker found after the second run in {hits:?}");
  }
}

/// The keyring is locked when spoold starts (first run): copies are
/// captured into the in-memory session store; once the keyring is unlocked
/// the key slot is created, the session is merged and persisted.
#[test]
fn locked_keyring_at_start_merges_after_unlock_secret_service() {
  let Some(mut env) = Env::with_keyring() else { return };
  env.lock_keyring();
  env.start_spoold();
  let st = env.wait_key_state(KEY_T, |s| s == "waiting-for-wallet");
  assert!(!st.encrypted && !st.unlocked);
  env.copy_stored("captured while locked 1");
  env.copy_stored("captured while locked 2");
  assert!(!env.status().encrypted);
  assert!(!env.state_dir().join("keyslots.json").exists());
  assert_eq!(env.log_count("key store not available yet"), 1, "logged once");

  env.expect_search("captured while locked", "captured while locked 2");

  env.unlock_keyring();
  // wait_for_unlock wakes on CollectionChanged (falls back to polling).
  let st = env.wait_key_state(KEY_T, |s| s == "ready");
  env.expect_search("captured while locked", "captured while locked 1");
  assert!(st.encrypted && st.unlocked);
  assert_eq!(st.item_count, 2);
  assert_eq!(text(env.current()), "captured while locked 2");
  assert!(env.state_dir().join("keyslots.json").is_file());
  // Capture continues into the encrypted store.
  env.copy_stored("after unlock");
  assert_eq!(env.log_count("key store not available yet"), 1);

  env.stop_spoold();
  env.start_spoold();
  let st = env.wait_key_state(KEY_T, |s| s == "ready");
  assert_eq!(st.item_count, 3);
  assert_eq!(text(env.current()), "after unlock");
}

/// SIGTERM while the key flow waits for a locked keyring (Secret Service
/// calls / `wait_for_unlock` in flight) and the RAM index is live: the
/// shutdown is bounded and prompt.
#[test]
fn sigterm_while_waiting_for_keyring_is_prompt() {
  let Some(mut env) = Env::with_keyring() else { return };
  env.lock_keyring();
  env.start_spoold();
  env.wait_key_state(KEY_T, |s| s == "waiting-for-wallet");
  env.copy_stored("captured while locked");
  let start = Instant::now();
  env.stop_spoold();
  assert!(start.elapsed() < Duration::from_secs(5), "shutdown took {:?}", start.elapsed());
}

#[test]
fn images_round_trip_and_svg_is_refused() {
  let Some(env) = Env::start() else { return };
  env.expect_log("stored (inserted)", || env.wl_copy(&["--type", "image/png"], PNG));
  assert_eq!(env.count(), 1);
  assert_eq!(env.current(), Some(("image/png".into(), PNG.to_vec())));
  // spoolctl writes the raw bytes when stdout is not a terminal.
  assert_eq!(env.spoolctl_ok(&["current"], b""), PNG);

  // SVG is not allowlisted. An SVG-only offer is skipped outright.
  let svg: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;
  let client = env.test_client();
  env.expect_log("offer skipped", || client.offer(&[("image/svg+xml", svg)]));
  assert_eq!(env.count(), 1);
  // wl-copy additionally offers XML as text/plain: only the text survives
  // (never the image/svg+xml representation).
  env.expect_log("stored (inserted)", || env.wl_copy(&["--type", "image/svg+xml"], svg));
  let last = env.log().lines().rev().find(|l| l.contains("stored (inserted)")).unwrap().to_owned();
  assert!(!last.contains("svg"), "{last}");
  assert_eq!(env.current(), Some((UTF8.into(), svg.to_vec())));
}

// ---- latency ----------------------------------------------------------------

// ---- resident picker (real spool-picker) ---------------------------------------

/// Show from the CLI, the list on screen, Down + Enter: the second item is
/// on the clipboard and pasted (virtual keyboard, Ctrl+V) into the
/// window that was focused at Show time.
#[test]
fn picker_shows_history_and_pastes() {
  let Some(mut env) = Env::sway() else { return };
  std::fs::write(env.data.path().join("config.toml"), "key_provider = \"session\"\n").unwrap();
  if !env.enable_picker() {
    return;
  }
  env.start_spoold();
  env.wait_log("picker ready", 1);
  for t in ["picker e2e alpha", "picker e2e beta", "picker e2e gamma"] {
    env.copy_stored(t);
  }
  let wev = env.spawn_wev();
  std::thread::sleep(Duration::from_millis(300));
  let before = env.grab();
  env.spoolctl_ok(&["show"], b"");
  env.wait_screen_change(&before);
  env.screenshot("picker-shown");
  wev.lock().unwrap().clear();
  // Second row = "beta" (newest first). Enter is held for a moment: the
  // picker selects (and hides) on the release, so wev, which gets the
  // focus back, never sees a Return press or release.
  env.expect_log("selected", || {
    env.wtype(&["-k", "Down", "-P", "Return", "-s", "300", "-p", "Return"])
  });
  poll_until("beta on the clipboard", T, || {
    (env.wl_paste(&["-n"]).ok()? == b"picker e2e beta").then_some(())
  });
  let helper = Path::new(env!("CARGO_BIN_EXE_spoold")).with_file_name("spool-paster");
  if helper.is_file() {
    env.wait_log("auto-pasted", 1);
    let start = Instant::now();
    loop {
      let l = wev.lock().unwrap().join("\n");
      // Keycodes (evdev + 8): wev decodes them with the keymap of the last
      // keyboard sway announced (wtype's), so the syms are meaningless here.
      if l.contains("key: 37; state: 1") && l.contains("key: 55; state: 1") {
        break;
      }
      assert!(start.elapsed() < T, "no Ctrl+V in wev; it saw:\n{l}\nlog:\n{}", env.log());
      std::thread::sleep(Duration::from_millis(50));
    }
  } else {
    eprintln!("spool-paster not built next to spoold; paste not checked");
  }
  env.screenshot("picker-after-paste");
  // No Return reached wev: neither as a key event nor in the pressed-keys
  // list of its `enter` (wtype uploads its own keymap, so match the sym
  // wev decodes, not a keycode).
  let l = wev.lock().unwrap().join("\n");
  assert!(!l.contains("sym: Return"), "a Return key reached wev:\n{l}");
  let log = env.log();
  assert!(!log.contains("picker e2e"), "content leaked into the log");
  env.stop_spoold();
}

/// `spoolctl pick`: the chosen item comes back on stdout (clipboard
/// untouched); Esc is exit 3.
#[test]
fn spoolctl_pick_returns_the_item() {
  let Some(mut env) = Env::sway() else { return };
  std::fs::write(env.data.path().join("config.toml"), "key_provider = \"session\"\n").unwrap();
  if !env.enable_picker() {
    return;
  }
  env.start_spoold();
  env.wait_log("picker ready", 1);
  env.copy_stored("pick e2e one");
  env.copy_stored("pick e2e two");
  let bin = Path::new(env!("CARGO_BIN_EXE_spoold")).with_file_name("spoolctl");
  let pick = |keys: &[&str]| -> (ExitStatus, Vec<u8>) {
    let (outp, outf) = env.out_file("pick-out");
    let before = env.grab();
    let mut child =
      env.cmd(&bin).args(["pick", "--raw"]).stdin(Stdio::null()).stdout(outf).spawn().unwrap();
    env.wait_screen_change(&before);
    env.wtype(keys);
    let st = wait_timeout(&mut child, T).unwrap_or_else(|| {
      kill_child(&mut child);
      panic!("spoolctl pick did not return; log:\n{}", env.log())
    });
    (st, std::fs::read(outp).unwrap())
  };
  let (st, out) = pick(&["-k", "Down", "-k", "Return"]);
  assert!(st.success(), "{st}");
  assert_eq!(out, b"pick e2e one");
  // Pick leaves the clipboard alone (it only bumps the item's last use).
  assert_eq!(env.wl_paste(&["-n"]).unwrap(), b"pick e2e two");
  let (st, out) = pick(&["-k", "Escape"]);
  assert_eq!(st.code(), Some(3));
  assert!(out.is_empty());
  env.stop_spoold();
}

/// Keyring locked at start: the picker lists the session items under an
/// unlock panel; unlocking the keyring (another route) removes the panel.
#[test]
fn picker_unlock_panel_follows_the_keyring_secret_service() {
  let Some(mut env) = Env::with_keyring() else { return };
  if !env.enable_picker() {
    return;
  }
  // First run creates the slot; restart with the keyring locked.
  env.start_spoold();
  env.wait_key_state(KEY_T, |s| s == "ready");
  env.copy_stored("persisted before the restart");
  env.stop_spoold();
  env.lock_keyring();
  env.start_spoold();
  env.wait_key_state(KEY_T, |s| s == "waiting-for-wallet");
  env.wait_log("picker ready", 1);
  env.wait_log("picker: locked", 1);
  env.copy_stored("captured while locked");
  let before = env.grab();
  env.spoolctl_ok(&["show"], b"");
  env.wait_screen_change(&before);
  env.screenshot("picker-locked");
  env.unlock_keyring();
  env.wait_key_state(KEY_T, |s| s == "ready");
  env.wait_log("picker: unlocked", 1);
  // The picker re-queries on Unlocked: give it a frame.
  std::thread::sleep(Duration::from_millis(400));
  env.screenshot("picker-unlocked");
  env.wtype(&["-k", "Escape"]);
  env.stop_spoold();
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
  let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
  sorted[idx]
}

fn report(label: &str, mut v: Vec<Duration>) {
  v.sort();
  let ms = |d: Duration| d.as_secs_f64() * 1000.0;
  eprintln!(
    "{label:<44} n={} p50={:.2}ms p95={:.2}ms p99={:.2}ms max={:.2}ms",
    v.len(),
    ms(percentile(&v, 0.50)),
    ms(percentile(&v, 0.95)),
    ms(percentile(&v, 0.99)),
    ms(*v.last().unwrap()),
  );
}

/// Copy -> stored latency over 50 distinct copies, observed by polling
/// `Status.item_count` every 1 ms. Ignored by default; run with
/// `-- --ignored --nocapture copy_latency`.
#[test]
#[ignore = "latency report; run explicitly with --ignored --nocapture"]
fn copy_latency() {
  const N: usize = 50;
  let Some(env) = Env::start() else { return };
  let wait_count = |want: u64| {
    let start = Instant::now();
    loop {
      if env.count() >= want {
        return Instant::now();
      }
      assert!(start.elapsed() < T, "item {want} never stored");
      std::thread::sleep(Duration::from_millis(1));
    }
  };

  // wl-copy: t0 = spawn, t1 = wl-copy parent exited (selection is set).
  let (mut from_spawn, mut from_set) = (Vec::new(), Vec::new());
  for i in 0..N {
    let want = env.count() + 1;
    let t0 = Instant::now();
    env.wl_copy(&[], format!("latency sample {i} {:?}", t0).as_bytes());
    let t1 = Instant::now();
    let done = wait_count(want);
    from_spawn.push(done - t0);
    from_set.push(done.saturating_duration_since(t1));
  }

  // In-process source: t0 = set_selection call (no process spawn at all).
  let client = env.test_client();
  let mut direct = Vec::new();
  for i in 0..N {
    let want = env.count() + 1;
    let body = format!("direct sample {i}");
    let t0 = Instant::now();
    client.offer(&[(UTF8, body.as_bytes())]);
    direct.push(wait_count(want) - t0);
  }

  eprintln!("copy -> stored latency (status polled every 1 ms):");
  report("wl-copy spawn -> stored", from_spawn);
  report("wl-copy exited (selection set) -> stored", from_set);
  report("in-process set_selection -> stored", direct);
}
