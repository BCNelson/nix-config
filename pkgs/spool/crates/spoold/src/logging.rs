//! Logging setup. Rule: NEVER log clipboard content — only hash prefixes
//! (`spool_core::item::hash_prefix`), byte lengths and MIME types.

use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

/// Journald when reachable and `to_stderr` is false, else stderr. Filter
/// from `RUST_LOG`, default `info`.
pub fn init(to_stderr: bool) -> anyhow::Result<()> {
  let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
  let registry = tracing_subscriber::registry().with(filter);
  if !to_stderr && let Ok(journald) = tracing_journald::layer() {
    registry.with(journald.with_syslog_identifier("spoold".into())).init();
    return Ok(());
  }
  registry.with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr)).init();
  Ok(())
}
