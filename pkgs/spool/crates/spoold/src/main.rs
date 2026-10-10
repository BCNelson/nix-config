//! `spoold`: Spool clipboard history daemon.
//!
//! Startup order matters for security:
//! 1. umask 077 and `PR_SET_DUMPABLE=0` before anything touches content.
//! 2. Logging (journald when available, never clipboard content).
//! 3. PATH audit (refuse to start on writable PATH dirs unless
//!    `SPOOL_INSECURE_PATH_OK=1`).
//! 4. Config, public socket (the single-instance guard), state directory
//!    (`$XDG_STATE_HOME/spool`, 0700) and its `state.lock` (shared with
//!    `spool-keyctl`, [`paths::lock_state_dir`]), in-memory session store (capture
//!    starts immediately), desktop integration ([`desktop`]: KWin script /
//!    portal shortcut, focus tracking, auto-paste), Wayland thread,
//!    orchestrator, the resident picker ([`picker`], if `spool-picker` is on
//!    the audited PATH), external-editor support ([`editor`]: stale edit
//!    sessions removed), then the key flow ([`keyflow`]) that unlocks the
//!    encrypted store in the background (the picker's unlock panel can
//!    unlock passphrase / FIDO2 slots through the same gate, [`unlock`]).
//!
//! SIGTERM/SIGINT shut down in bounded steps ([`shutdown`]).
//!
//! A plaintext `history.db` left at the production path by M1 development
//! builds is renamed to `history.db.m1-plaintext.bak` (with a warning) and a
//! fresh encrypted database is created. `SPOOL_DEV_PLAINTEXT=1` instead keeps
//! using a plaintext database (development only; logged loudly).

mod autopaste;
mod desktop;
mod editor;
mod index;
mod ipc;
#[cfg(test)]
mod ipc_tests;
mod keyflow;
mod logging;
mod orchestrator;
mod paths;
mod picker;
mod security;
mod shutdown;
#[cfg(feature = "test-hooks")]
mod test_hooks;
mod unlock;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use spool_core::config::{Config, KeyProviderSetting};
use spool_core::store::{DB_FILE_NAME, Store};

#[derive(Debug, Parser)]
#[command(name = "spoold", version, about = "Spool clipboard history daemon")]
struct Args {
  /// Config file (default: $XDG_CONFIG_HOME/spool/config.toml).
  #[arg(long, env = "SPOOL_CONFIG")]
  config: Option<PathBuf>,
  /// State directory holding the encrypted history and key slots
  /// (default: $XDG_STATE_HOME/spool).
  #[arg(long, env = "SPOOL_STATE_DIR")]
  state_dir: Option<PathBuf>,
  /// Log to stderr instead of journald.
  #[arg(long)]
  log_stderr: bool,
}

fn main() -> std::process::ExitCode {
  match real_main() {
    Ok(()) => std::process::ExitCode::SUCCESS,
    Err(e) => {
      tracing::error!("fatal: {e:#}");
      eprintln!("spoold: {e:#}");
      std::process::ExitCode::FAILURE
    }
  }
}

fn real_main() -> anyhow::Result<()> {
  security::set_private_umask();
  security::disable_dumpable().context("prctl(PR_SET_DUMPABLE, 0)")?;
  #[cfg(feature = "test-hooks")]
  if std::env::args().nth(1).as_deref() == Some("--probe-fake-input") {
    return test_hooks::probe_fake_input();
  }

  let args = Args::parse();
  logging::init(args.log_stderr)?;

  security::enforce_path_policy()?;

  let config_path = args.config.or_else(Config::default_path);
  let config = match &config_path {
    Some(p) => Config::load(p).with_context(|| format!("loading config {}", p.display()))?,
    None => Config::default(),
  };

  let state_dir = paths::state_dir(args.state_dir)?;
  let socket_path = spool_proto::paths::public_socket_path()?;
  let dev_plaintext = std::env::var("SPOOL_DEV_PLAINTEXT").as_deref() == Ok("1");

  let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
  let result = rt.block_on(run(config, state_dir, socket_path, dev_plaintext));
  // Dropping the runtime would wait for every blocking task (a key-flow
  // store open, an index rebuild, ...) without limit; they are all
  // crash-safe to abandon.
  shutdown::phase("stopping the runtime");
  rt.shutdown_timeout(RUNTIME_SHUTDOWN_WAIT);
  result
}

/// Longest wait for leftover blocking tasks when the runtime stops.
const RUNTIME_SHUTDOWN_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// The store the daemon starts on, and whether a key flow must follow.
fn initial_store(
  config: &Config,
  state_dir: &Path,
  dev_plaintext: bool,
) -> anyhow::Result<(Store, bool)> {
  paths::ensure_private_dir(state_dir)?;
  let db_path = state_dir.join(DB_FILE_NAME);
  if dev_plaintext {
    tracing::warn!(
      db = %db_path.display(),
      "SPOOL_DEV_PLAINTEXT=1: history is stored UNENCRYPTED. Development only; never use this \
       with real clipboard data"
    );
    let store = Store::open(&db_path, None)
      .with_context(|| format!("opening plaintext history database {}", db_path.display()))?;
    return Ok((store, false));
  }
  if let Some(backup) = paths::quarantine_plaintext_db(state_dir, DB_FILE_NAME)? {
    tracing::warn!(
      backup = %backup.display(),
      "found an unencrypted history database from a development build and moved it aside; it \
       is development data: delete it (and its -wal/-shm files). Starting a new encrypted history"
    );
  }
  let session_key = spool_crypto::DataKey::generate();
  let store = Store::open_session(&session_key).context("opening the in-memory session store")?;
  keyflow::quiet_sqlcipher(&store);
  let key_flow = match config.key_provider {
    KeyProviderSetting::SecretService => true,
    KeyProviderSetting::Session => {
      tracing::info!("key_provider = \"session\": clipboard history is kept in memory only");
      false
    }
  };
  Ok((store, key_flow))
}

async fn run(
  config: Config,
  state_dir: PathBuf,
  socket_path: PathBuf,
  dev_plaintext: bool,
) -> anyhow::Result<()> {
  use tokio::signal::unix::{SignalKind, signal};

  // Socket first: it is the single-instance guard, so a second daemon
  // never opens the database or connects to Wayland and fights the first
  // one.
  let ipc::BoundSocket { listener, guard } = ipc::bind(&socket_path)?;
  // The state directory's lock, shared with spool-keyctl: never touch the
  // history or key slots while they are being changed (held until exit).
  paths::ensure_private_dir(&state_dir)?;
  let _state_lock = paths::lock_state_dir(&state_dir)?;
  let (store, key_flow) = initial_store(&config, &state_dir, dev_plaintext)?;
  let (desktop, desktop_link) = desktop::start(&config).await;

  let (wayland, wl_events) = match spool_wayland::spawn(spool_wayland::WaylandConfig {
    display: None,
    watch_primary: config.primary_selection,
  }) {
    Ok(v) => v,
    Err(e) => {
      desktop.shutdown().await;
      return Err(e).context("starting the Wayland backend");
    }
  };

  let mut sigterm = signal(SignalKind::terminate())?;
  let mut sigint = signal(SignalKind::interrupt())?;

  let (req_tx, req_rx) = tokio::sync::mpsc::channel(orchestrator::REQUEST_QUEUE);
  let limiter = ipc::show_limiter();
  let picker_settings = config.picker.clone();
  let mut orch = match orchestrator::Orchestrator::new(config, store, wayland.clone()) {
    Ok(o) => o.with_desktop(desktop_link).with_show_limiter(limiter.clone()),
    Err(e) => {
      wayland.shutdown();
      desktop.shutdown().await;
      return Err(e);
    }
  };
  let gate = keyflow::OpenGate::default();
  let (key_task, key_tx) = if key_flow {
    let (key_tx, key_rx) = tokio::sync::mpsc::channel(8);
    orch = orch.with_key_flow(key_rx);
    let source: Arc<dyn keyflow::KeySource> = Arc::new(keyflow::SecretServiceSource::default());
    let task =
      tokio::spawn(keyflow::run_gated(source, state_dir.clone(), key_tx.clone(), gate.clone()));
    orch = orch.with_key_task(task.abort_handle());
    (Some(task), Some(key_tx))
  } else {
    (None, None)
  };
  // Wayland is up: start the resident picker (if installed).
  if let Some(exe) = picker::locate() {
    let deps = picker::HostDeps {
      requests: req_tx.clone(),
      index_status: orch.index_status(),
      unlock: key_tx.map(|key_tx| picker::UnlockCtx {
        dir: state_dir.clone(),
        key_tx,
        gate: gate.clone(),
        providers: Arc::new(unlock::SystemProviders),
      }),
    };
    let spawner = Box::new(picker::ProcessSpawner::new(exe, &picker_settings));
    orch =
      orch.with_picker(Box::new(picker::ResidentPicker::start(spawner, deps, &picker_settings)));
  }
  // "Edit in external editor": sessions under <socket dir>/edit.
  if let Some(deps) = editor::system_deps(&socket_path) {
    orch = orch.with_editor(deps);
  }
  let wl_rx = orchestrator::bridge_wayland_events(wl_events);
  let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
  let mut orch_task = tokio::spawn(orch.run(wl_rx, req_rx, async move {
    let _ = stop_rx.await;
  }));
  #[cfg(feature = "test-hooks")]
  let _test_hooks = test_hooks::start(req_tx.clone())?;
  let mut server = tokio::spawn(ipc::serve(listener, req_tx, limiter));

  let mut early: Option<anyhow::Error> = None;
  let result = tokio::select! {
    r = &mut orch_task => Some(r),
    r = &mut server => {
      early = Some(match r {
        Ok(Ok(())) => anyhow::anyhow!("socket server stopped"),
        Ok(Err(e)) => e,
        Err(e) => anyhow::anyhow!("socket server task: {e}"),
      });
      None
    }
    _ = sigterm.recv() => { tracing::info!("SIGTERM: shutting down"); None }
    _ = sigint.recv() => { tracing::info!("SIGINT: shutting down"); None }
  };
  // Bounded from here on: every step below limits its own waits, and the
  // watchdog ends the process if something still hangs.
  let watchdog = shutdown::Watchdog::arm(shutdown::HARD_DEADLINE);
  server.abort();
  // The key flow may be waiting on the Secret Service or opening the store
  // on a blocking thread; abandon it (nothing it does is needed any more,
  // and the blocking part is cut off by the runtime's shutdown timeout).
  if let Some(t) = &key_task {
    shutdown::phase("cancelling the key flow");
    t.abort();
  }
  let result = match result {
    Some(r) => r,
    None => {
      shutdown::phase("stopping the orchestrator");
      let _ = stop_tx.send(());
      orch_task.await
    }
  };

  desktop.shutdown().await;
  shutdown::phase("stopping the Wayland thread");
  wayland.shutdown();
  // Removes the socket, then releases the lock (the lock file itself stays;
  // it is harmless and avoids an unlink/lock race).
  drop(guard);

  let orch_result = result.map_err(|e| anyhow::anyhow!("orchestrator task: {e}"))?;
  if let Some(e) = early {
    return Err(e);
  }
  if orch_result.is_ok() {
    tracing::info!("spoold stopped");
  }
  watchdog.disarm();
  orch_result
}
