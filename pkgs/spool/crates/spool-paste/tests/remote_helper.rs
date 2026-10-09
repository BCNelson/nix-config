//! The real `spool-paster` binary through `RemotePaster`, without a
//! compositor (runs in the Nix sandbox): its connect error comes back.

use spool_paste::{PasteConfig, PasteError, RemotePaster};

#[tokio::test]
async fn helper_reports_connect_errors() {
  let exe = std::path::PathBuf::from(env!("CARGO_BIN_EXE_spool-paster"));
  let cfg = PasteConfig { display: Some("/nonexistent/wayland-0".into()), ..Default::default() };
  match RemotePaster::spawn(&exe, cfg).await {
    Err(PasteError::Connect(_) | PasteError::Unsupported(_)) => {}
    other => panic!("unexpected {other:?}"),
  }
}

#[test]
fn helper_refuses_a_terminal_or_file_stdin() {
  let exe = env!("CARGO_BIN_EXE_spool-paster");
  let st = std::process::Command::new(exe).stdin(std::process::Stdio::null()).status().unwrap();
  assert_eq!(st.code(), Some(2));
  let st = std::process::Command::new(exe).arg("--bogus").status().unwrap();
  assert_eq!(st.code(), Some(2));
}
