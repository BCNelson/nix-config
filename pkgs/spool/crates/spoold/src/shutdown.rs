//! Bounded SIGTERM shutdown.
//!
//! Every step of the shutdown sequence is time-limited where it can wait on
//! something outside our control (index commit / rebuild, D-Bus peers,
//! blocking threads), and records its name with [`phase`], so a stuck
//! shutdown says what it was waiting for. As a last resort [`Watchdog`]
//! exits the process after [`HARD_DEADLINE`]: everything spoold keeps is
//! crash-safe (SQLite WAL with committed transactions, the search index
//! commits atomically and is caught up from the store at the next start), so
//! a forced exit loses at most uncommitted index work.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// After this long the watchdog ends the process. Below systemd's default
/// stop timeout (90 s) and the E2E suite's 10 s.
pub const HARD_DEADLINE: Duration = Duration::from_secs(8);

static PHASE: Mutex<&'static str> = Mutex::new("running");

/// Record (and log at debug) the shutdown step now in progress.
pub fn phase(name: &'static str) {
  tracing::debug!("shutdown: {name}");
  *PHASE.lock().unwrap_or_else(|p| p.into_inner()) = name;
}

/// The step last recorded with [`phase`].
pub fn current_phase() -> &'static str {
  *PHASE.lock().unwrap_or_else(|p| p.into_inner())
}

/// Exits the process if the shutdown is not finished in time.
pub struct Watchdog {
  done: Arc<AtomicBool>,
}

impl Watchdog {
  /// Starts a plain thread (it must work even if the runtime is wedged).
  pub fn arm(deadline: Duration) -> Self {
    let done = Arc::new(AtomicBool::new(false));
    let d = done.clone();
    let spawned =
      std::thread::Builder::new().name("spool-shutdown-watchdog".into()).spawn(move || {
        let step = Duration::from_millis(50);
        let mut waited = Duration::ZERO;
        while waited < deadline {
          if d.load(Ordering::Acquire) {
            return;
          }
          std::thread::sleep(step);
          waited += step;
        }
        if d.load(Ordering::Acquire) {
          return;
        }
        let msg = format!(
          "shutdown did not finish within {deadline:?} (stuck: {}); exiting anyway",
          current_phase()
        );
        tracing::error!("{msg}");
        eprintln!("spoold: {msg}");
        // SIGTERM was handled, state on disk is crash-safe: report success.
        std::process::exit(0);
      });
    if let Err(e) = spawned {
      tracing::warn!("cannot start the shutdown watchdog: {e}");
    }
    Self { done }
  }

  /// The shutdown finished; the watchdog thread exits quietly.
  pub fn disarm(&self) {
    self.done.store(true, Ordering::Release);
  }
}
