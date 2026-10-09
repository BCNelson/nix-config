//! The `spool-keyctl` binary, non-interactive paths only. Every run gets a
//! private HOME / XDG dirs / socket path and no D-Bus session, so nothing
//! here can reach the user's real state, wallet or security keys.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use spool_keyctl::{DaemonLock, exit};

struct Env {
  tmp: tempfile::TempDir,
}

impl Env {
  fn new() -> Env {
    Env { tmp: tempfile::tempdir().unwrap() }
  }
  fn state(&self) -> PathBuf {
    self.tmp.path().join("state")
  }
  fn socket(&self) -> PathBuf {
    self.tmp.path().join("run/spool/sock")
  }
  fn run(&self, args: &[&str]) -> Output {
    let t = self.tmp.path();
    Command::new(env!("CARGO_BIN_EXE_spool-keyctl"))
      .args(args)
      .env_clear()
      .env("PATH", std::env::var_os("PATH").unwrap_or_default())
      .env("HOME", t.join("home"))
      .env("XDG_STATE_HOME", t.join("xdg-state"))
      .env("XDG_RUNTIME_DIR", t.join("run"))
      .env("SPOOL_STATE_DIR", self.state())
      .env("SPOOL_SOCKET", self.socket())
      .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", t.join("no-bus").display()))
      .output()
      .unwrap()
  }
}

fn text(o: &Output) -> String {
  format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

#[test]
fn usage_error_is_exit_2() {
  let e = Env::new();
  let o = e.run(&["frobnicate"]);
  assert_eq!(o.status.code(), Some(exit::USAGE), "{}", text(&o));
  let o = e.run(&["remove", "not-a-uuid"]);
  assert_eq!(o.status.code(), Some(exit::USAGE), "{}", text(&o));
  let o = e.run(&["--help"]);
  assert_eq!(o.status.code(), Some(exit::OK));
  assert!(text(&o).contains("Exit codes"));
}

#[test]
fn status_on_fresh_state() {
  let e = Env::new();
  let o = e.run(&["status"]);
  assert_eq!(o.status.code(), Some(0), "{}", text(&o));
  let out = String::from_utf8_lossy(&o.stdout);
  assert!(out.contains(&e.state().display().to_string()));
  assert!(out.contains("does not exist"));
  assert!(out.contains("spoold:       not running"));
  // `list` is an alias; neither creates anything.
  assert_eq!(e.run(&["list"]).status.code(), Some(0));
  assert!(!e.state().exists());
  assert!(!e.socket().parent().unwrap().exists());
}

#[test]
fn state_dir_flag_overrides_env() {
  let e = Env::new();
  let other = e.tmp.path().join("other");
  let o = e.run(&["--state-dir", other.to_str().unwrap(), "status"]);
  assert!(String::from_utf8_lossy(&o.stdout).contains(&other.display().to_string()));
}

fn hold_daemon_lock(socket: &Path) -> DaemonLock {
  DaemonLock::acquire(socket, None).unwrap()
}

#[test]
fn refuses_while_spoold_runs() {
  let e = Env::new();
  std::fs::create_dir(e.state()).unwrap();
  let _daemon = hold_daemon_lock(&e.socket());
  for args in [
    &["add", "secret-service"][..],
    &["add", "passphrase"],
    &["add", "fido2"],
    &["remove", "00000000-0000-4000-8000-000000000000"],
    &["rotate"],
    &["recover"],
    &["wipe"],
  ] {
    let o = e.run(args);
    assert_eq!(o.status.code(), Some(exit::DAEMON_RUNNING), "{args:?}: {}", text(&o));
    assert!(text(&o).contains("systemctl --user stop spool"));
  }
  // status still works and says so.
  let o = e.run(&["status"]);
  assert_eq!(o.status.code(), Some(0));
  assert!(String::from_utf8_lossy(&o.stdout).contains("spoold:       running"));
}

#[test]
fn add_without_keyslots_is_an_error() {
  let e = Env::new();
  std::fs::create_dir(e.state()).unwrap();
  let o = e.run(&["add", "secret-service"]);
  assert_eq!(o.status.code(), Some(exit::ERROR), "{}", text(&o));
  assert!(text(&o).contains("keyslots.json does not exist"));
  // The lock was released at exit: a "daemon" can start now.
  let _ = hold_daemon_lock(&e.socket());
}

#[test]
fn wipe_of_missing_state_dir_is_a_noop() {
  let e = Env::new();
  let o = e.run(&["wipe"]);
  assert_eq!(o.status.code(), Some(0), "{}", text(&o));
  assert!(text(&o).contains("nothing to wipe"));
}
