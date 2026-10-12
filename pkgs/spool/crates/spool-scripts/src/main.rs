//! `spool-scripts`: Spool's Lua script host.
//!
//! Loads `*.lua` from `--scripts` (a `/nix/store` directory built by the
//! home-manager module), connects to spoold's `scripts.sock` and answers
//! its `Filter` requests with the filter chain's verdict ([`engine`]). Runs
//! as its own user unit, outside spoold's sandbox; reconnects with backoff
//! whenever spoold is not there.
//!
//! The protocol and spoold's side are in `spool_proto::script` and
//! `spoold::scripts`; the design is SCRIPTING.md.

mod api;
mod engine;

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use spool_proto::{FrameError, Hello, ScriptEvt, ScriptReq, Verdict, read_frame, write_frame};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

use crate::engine::{Engine, INSECURE_SCRIPTS_ENV};

/// Reconnect backoff bounds.
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

#[derive(Debug, Parser)]
#[command(name = "spool-scripts", version, about = "Spool's Lua script host")]
struct Args {
  /// Directory of `*.lua` scripts, loaded in name order (must be in
  /// /nix/store unless SPOOL_INSECURE_SCRIPTS_OK=1).
  #[arg(long, env = "SPOOL_SCRIPTS_DIR")]
  scripts: PathBuf,
  /// Load the scripts, report, and exit (non-zero if any failed).
  #[arg(long)]
  check: bool,
  /// Run the filters on TEXT (a text copy) without spoold, print what they
  /// decide, and exit. For writing scripts; prints the content.
  #[arg(long = "try", value_name = "TEXT")]
  try_text: Option<String>,
  /// Source app id for --try.
  #[arg(long, requires = "try_text")]
  source_app: Option<String>,
  /// Log to stderr instead of journald.
  #[arg(long)]
  log_stderr: bool,
}

fn main() -> std::process::ExitCode {
  match real_main() {
    Ok(code) => code,
    Err(e) => {
      tracing::error!("fatal: {e:#}");
      eprintln!("spool-scripts: {e:#}");
      std::process::ExitCode::FAILURE
    }
  }
}

fn real_main() -> anyhow::Result<std::process::ExitCode> {
  let args = Args::parse();
  init_logging(args.log_stderr || args.check || args.try_text.is_some());
  let debug = std::env::var_os("SPOOL_SCRIPTS_DEBUG").is_some_and(|v| v == "1");
  let insecure_ok = std::env::var_os(INSECURE_SCRIPTS_ENV).is_some_and(|v| v == "1");
  if insecure_ok {
    tracing::warn!(
      "{INSECURE_SCRIPTS_ENV}=1: loading scripts from outside /nix/store (development only)"
    );
  }

  let mut engine =
    Engine::new(debug).map_err(|e| anyhow::anyhow!("creating the Lua state: {e}"))?;
  let report = engine.load_dir(&args.scripts, insecure_ok)?;
  tracing::info!(
    loaded = report.loaded.len(),
    failed = report.failed.len(),
    filters = engine.filter_count(),
    "scripts loaded"
  );
  if let Some(text) = args.try_text {
    print!("{}", try_filters(&engine, text, args.source_app));
    return Ok(std::process::ExitCode::SUCCESS);
  }
  if args.check {
    return Ok(if report.failed.is_empty() {
      std::process::ExitCode::SUCCESS
    } else {
      std::process::ExitCode::FAILURE
    });
  }

  let socket = spool_proto::paths::script_socket_path()?;
  let mut backoff = BACKOFF_MIN;
  loop {
    match UnixStream::connect(&socket) {
      Ok(stream) => {
        backoff = BACKOFF_MIN;
        match session(stream, &engine) {
          Ok(()) => tracing::info!("spoold closed the connection"),
          Err(e) => tracing::warn!("connection to spoold: {e:#}"),
        }
      }
      Err(e) => tracing::debug!(path = %socket.display(), "spoold not reachable: {e}"),
    }
    std::thread::sleep(backoff);
    backoff = (backoff * 2).min(BACKOFF_MAX);
  }
}

/// `--try`: the chain's verdict on a text copy, for humans.
fn try_filters(engine: &Engine, text: String, source_app: Option<String>) -> String {
  let item = spool_proto::ScriptItem {
    id: None,
    selection: spool_proto::WireSelection::Clipboard,
    source_app,
    created_at_ms: 0,
    tags: Vec::new(),
    reps: vec![spool_proto::WireRep { mime: engine::TEXT_MIME.into(), data: text.into_bytes() }],
    aliases: Vec::new(),
  };
  match engine.filter(&item, Duration::from_secs(2)) {
    Verdict::Drop => "drop\n".into(),
    Verdict::Keep { tags, reps, original, by } => {
      let mut out = String::from("keep\n");
      if !tags.is_empty() {
        out += &format!("tags: {}\n", tags.join(", "));
      }
      match reps {
        None => out += "unchanged\n",
        Some(reps) => {
          out += &format!(
            "rewritten by: {} (original {})\n",
            by.join(", "),
            if original { "kept" } else { "not kept" }
          );
          for r in reps {
            out += &format!("{}: {}\n", r.mime, String::from_utf8_lossy(&r.data));
          }
        }
      }
      out
    }
  }
}

/// One connection: handshake, `Register`, then answer filters until EOF.
fn session(mut stream: UnixStream, engine: &Engine) -> anyhow::Result<()> {
  write_frame(&mut stream, &Hello::script())?;
  let hello: Hello = read_frame(&mut stream).context("reading spoold's Hello")?;
  if !hello.is_script_compatible() {
    anyhow::bail!(
      "spoold speaks script protocol v{}, this host v{} (mismatched packages?)",
      hello.proto,
      spool_proto::SCRIPT_PROTO_VERSION
    );
  }
  let filters = engine.filter_count() > 0;
  write_frame(&mut stream, &ScriptReq::Register { filters })?;
  tracing::info!(filters, "connected to spoold");
  loop {
    let evt: ScriptEvt = match read_frame(&mut stream) {
      Ok(e) => e,
      Err(FrameError::Eof) => return Ok(()),
      Err(e) => return Err(e.into()),
    };
    match evt {
      ScriptEvt::Filter { seq, budget_ms, item } => {
        let verdict = engine.filter(&item, Duration::from_millis(u64::from(budget_ms)));
        write_frame(&mut stream, &ScriptReq::FilterDone { seq, verdict })?;
      }
    }
  }
}

/// Journald when reachable and `to_stderr` is false, else stderr. Filter
/// from `RUST_LOG`, default `info`.
fn init_logging(to_stderr: bool) {
  let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
  let registry = tracing_subscriber::registry().with(filter);
  if !to_stderr && let Ok(journald) = tracing_journald::layer() {
    registry.with(journald.with_syslog_identifier("spool-scripts".into())).init();
    return;
  }
  registry.with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr)).init();
}
