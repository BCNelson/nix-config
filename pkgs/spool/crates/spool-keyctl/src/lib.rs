//! `spool-keyctl`: offline, interactive management of Spool's encryption
//! key slots (`keyslots.json`).
//!
//! Key management is deliberately **not** on spoold's public socket (any
//! process of the same user can reach that). This tool runs only while
//! spoold is stopped (it takes spoold's socket and state-directory locks and
//! keeps them), and every command that changes slots first proves knowledge
//! of the data key by unlocking an existing slot interactively and opening
//! the history database with it.
//!
//! Commands: [`ops::status`], [`ops::add`], [`ops::remove`],
//! [`ops::rotate`], [`ops::recover`], [`ops::wipe`]. See the README section
//! "Key management (spool-keyctl)" for the user-facing description and
//! [`ops::rotate`] for the crash-safety design.
//!
//! Nothing here logs or prints key material, passphrases or PINs.

#![forbid(unsafe_code)]

pub mod lock;
pub mod ops;
pub mod paths;
pub mod providers;
pub mod ui;
mod unlock;

use std::path::PathBuf;

use spool_keys::SlotId;

pub use lock::{DaemonLock, LockError};
pub use providers::{Fido2Opts, Providers, SystemProviders};
pub use ui::{PromptError, TtyUi, Ui};

/// Exit codes of the binary.
pub mod exit {
  /// Success.
  pub const OK: i32 = 0;
  /// Any other error.
  pub const ERROR: i32 = 1;
  /// Command-line usage error (clap).
  pub const USAGE: i32 = 2;
  /// spoold is running (its lock is held).
  pub const DAEMON_RUNNING: i32 = 3;
  /// No existing key slot could be unlocked.
  pub const UNLOCK_FAILED: i32 = 4;
  /// The operation finished, but some provider secrets could not be
  /// destroyed (e.g. the wallet stayed locked); they are listed.
  pub const INCOMPLETE: i32 = 5;
  /// An interrupted rotation needs `spool-keyctl recover` first.
  pub const RECOVERY_NEEDED: i32 = 6;
  /// The user declined or aborted (Ctrl-C at a prompt, wrong confirmation).
  pub const ABORTED: i32 = 7;
}

/// Errors, each mapping to an exit code.
#[derive(Debug, thiserror::Error)]
pub enum KeyctlError {
  /// Lock problems (spoold running, or I/O).
  #[error(transparent)]
  Lock(#[from] LockError),
  /// No existing slot could be unlocked.
  #[error("{0}")]
  Unlock(String),
  /// The user aborted / declined.
  #[error("aborted")]
  Aborted,
  /// An interrupted rotation is pending.
  #[error("{0}")]
  RecoveryNeeded(String),
  /// Done, but some provider secrets survive.
  #[error("{0}")]
  Incomplete(String),
  /// Anything else.
  #[error(transparent)]
  Other(#[from] anyhow::Error),
}

impl KeyctlError {
  /// The process exit code for this error.
  pub fn exit_code(&self) -> i32 {
    match self {
      KeyctlError::Lock(LockError::Held(_)) => exit::DAEMON_RUNNING,
      KeyctlError::Lock(_) => exit::ERROR,
      KeyctlError::Unlock(_) => exit::UNLOCK_FAILED,
      KeyctlError::Aborted => exit::ABORTED,
      KeyctlError::RecoveryNeeded(_) => exit::RECOVERY_NEEDED,
      KeyctlError::Incomplete(_) => exit::INCOMPLETE,
      KeyctlError::Other(_) => exit::ERROR,
    }
  }
}

impl From<PromptError> for KeyctlError {
  fn from(e: PromptError) -> Self {
    match e {
      PromptError::Aborted => KeyctlError::Aborted,
      PromptError::Io(e) => KeyctlError::Other(e.into()),
    }
  }
}

impl From<spool_core::Error> for KeyctlError {
  fn from(e: spool_core::Error) -> Self {
    KeyctlError::Other(e.into())
  }
}

/// Map a spool-keys error: destroy failures are [`KeyctlError::Incomplete`].
pub(crate) fn keys_err(e: spool_keys::Error) -> KeyctlError {
  match e {
    e @ spool_keys::Error::DestroyIncomplete { .. } => KeyctlError::Incomplete(e.to_string()),
    e => KeyctlError::Other(e.into()),
  }
}

/// Simulated-crash points inside [`ops::rotate`] / [`ops::recover`] (tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashPoint {
  /// `.next` exists with the placeholder and the first new slot.
  AfterFirstEnroll,
  /// `.next` complete; database still under the old key.
  AfterNextWritten,
  /// Database re-keyed; `keyslots.json` still the old one.
  AfterRekey,
  /// `keyslots.json.old` written; `.next` not yet switched in.
  AfterOldCopied,
  /// `.next` renamed over `keyslots.json`; old secrets not yet destroyed.
  AfterSwitch,
}

/// Everything a command needs.
pub struct Ctx<'a> {
  /// State directory paths.
  pub paths: paths::StatePaths,
  /// spoold's socket path (its `.lock` sibling is the instance lock).
  pub socket: PathBuf,
  /// Terminal.
  pub ui: &'a dyn Ui,
  /// Provider factory.
  pub providers: &'a dyn Providers,
  /// Unlock only with this slot (`--unlock-slot`).
  pub unlock_slot: Option<SlotId>,
  crash_at: Option<CrashPoint>,
}

impl<'a> Ctx<'a> {
  /// A context for `state_dir`.
  pub fn new(
    state_dir: PathBuf,
    socket: PathBuf,
    ui: &'a dyn Ui,
    providers: &'a dyn Providers,
  ) -> Self {
    Ctx {
      paths: paths::StatePaths::new(state_dir),
      socket,
      ui,
      providers,
      unlock_slot: None,
      crash_at: None,
    }
  }

  /// Simulate a crash at `point` (tests only; `#[doc(hidden)]`). Has no
  /// effect unless a test sets it: the release binary never does.
  #[doc(hidden)]
  pub fn with_crash_at(mut self, point: CrashPoint) -> Self {
    self.crash_at = Some(point);
    self
  }

  pub(crate) fn crash_point(&self, point: CrashPoint) -> Result<(), KeyctlError> {
    if self.crash_at == Some(point) {
      return Err(KeyctlError::Other(anyhow::anyhow!("simulated crash at {point:?}")));
    }
    Ok(())
  }

  /// Take spoold's locks for the duration of a mutating command.
  pub fn lock(&self) -> Result<DaemonLock, KeyctlError> {
    let state = self.paths.dir.symlink_metadata().is_ok().then_some(self.paths.dir.as_path());
    Ok(DaemonLock::acquire(&self.socket, state)?)
  }
}

#[cfg(test)]
mod tests;
