//! Sandbox-safe CLI tests for the real `spoolctl` binary (no daemon, no
//! Wayland).
//!
//! Having an integration test here also makes `cargo test --workspace` build
//! the `spoolctl` binary, which `spoold`'s end-to-end suite
//! (`crates/spoold/tests/e2e.rs`) runs.

use std::process::{Command, Stdio};

fn spoolctl() -> Command {
  let mut c = Command::new(env!("CARGO_BIN_EXE_spoolctl"));
  c.env_clear().stdin(Stdio::null());
  c
}

#[test]
fn no_daemon_is_exit_1() {
  let d = tempfile::tempdir().unwrap();
  let out = spoolctl().env("SPOOL_SOCKET", d.path().join("sock")).arg("status").output().unwrap();
  assert_eq!(out.status.code(), Some(1));
  let err = String::from_utf8_lossy(&out.stderr);
  assert!(err.contains("not running"), "{err}");
}

#[test]
fn usage_error_is_exit_2() {
  let out = spoolctl().args(["pause", "--for", "soon"]).output().unwrap();
  assert_eq!(out.status.code(), Some(2));
  let out = spoolctl().arg("frobnicate").output().unwrap();
  assert_eq!(out.status.code(), Some(2));
}

#[test]
fn relative_socket_override_is_rejected() {
  let out = spoolctl().env("SPOOL_SOCKET", "relative/sock").arg("status").output().unwrap();
  assert_eq!(out.status.code(), Some(1));
  assert!(String::from_utf8_lossy(&out.stderr).contains("absolute"));
}
