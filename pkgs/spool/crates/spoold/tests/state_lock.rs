//! `spoold` refuses to start while its state directory is locked (by
//! `spool-keyctl`, which takes `<state>/state.lock` for every mutating
//! command), even with a socket path of its own. No compositor needed: the
//! lock is checked before Wayland or the session bus are touched, so this
//! runs everywhere, including the Nix sandbox. Private temp dirs only.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn spoold_refuses_a_locked_state_dir() {
  let tmp = tempfile::tempdir().unwrap();
  let rt = tmp.path().join("rt");
  let state = tmp.path().join("state");
  for d in [&rt, &state] {
    std::fs::create_dir(d).unwrap();
    std::fs::set_permissions(d, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
  }
  // What spool-keyctl's `DaemonLock` holds.
  let lock = std::fs::OpenOptions::new()
    .create(true)
    .truncate(false)
    .write(true)
    .open(state.join(spool_core::store::STATE_LOCK_FILE_NAME))
    .unwrap();
  rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();

  let mut child = Command::new(env!("CARGO_BIN_EXE_spoold"))
    .arg("--log-stderr")
    .env_clear()
    .env("PATH", "/nonexistent")
    .env("HOME", tmp.path())
    .env("XDG_RUNTIME_DIR", &rt)
    .env("XDG_CONFIG_HOME", tmp.path().join("config"))
    .env("SPOOL_STATE_DIR", &state)
    .env("SPOOL_SOCKET", rt.join("spool").join("sock"))
    // No Wayland display and no session bus: must never get that far.
    .env("WAYLAND_DISPLAY", "wayland-nonexistent")
    .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
  let start = Instant::now();
  let status = loop {
    if let Some(st) = child.try_wait().unwrap() {
      break st;
    }
    if start.elapsed() > Duration::from_secs(20) {
      let _ = child.kill();
      panic!("spoold kept running with a locked state directory");
    }
    std::thread::sleep(Duration::from_millis(20));
  };
  let mut err = String::new();
  std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut err).unwrap();
  assert!(!status.success());
  assert!(err.contains("is in use"), "{err}");
  // Nothing was created in the state directory besides the lock.
  let names: Vec<_> = std::fs::read_dir(&state).unwrap().map(|e| e.unwrap().file_name()).collect();
  assert_eq!(names, [spool_core::store::STATE_LOCK_FILE_NAME], "{names:?}");

  // Released: spoold gets past the lock (and then fails on the missing
  // compositor, which is fine here).
  drop(lock);
  let out = Command::new(env!("CARGO_BIN_EXE_spoold"))
    .arg("--log-stderr")
    .env_clear()
    .env("PATH", "/nonexistent")
    .env("HOME", tmp.path())
    .env("XDG_RUNTIME_DIR", &rt)
    .env("XDG_CONFIG_HOME", tmp.path().join("config"))
    .env("SPOOL_STATE_DIR", &state)
    .env("SPOOL_SOCKET", rt.join("spool").join("sock"))
    .env("WAYLAND_DISPLAY", "wayland-nonexistent")
    .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .output()
    .unwrap();
  let err = String::from_utf8_lossy(&out.stderr);
  assert!(!err.contains("is in use"), "{err}");
}
