//! Error type for key slots and providers.
//!
//! Every variant is classified as either *retryable* (the backing store is
//! temporarily unavailable or locked; trying again later may succeed) or
//! *fatal* (the key material is wrong, missing or the file is corrupt; trying
//! again will not help). Use [`Error::is_retryable`].
//!
//! No variant ever carries key material.

use std::fmt;
use std::path::PathBuf;

use crate::provider::ProviderKind;
use crate::slots::SlotId;

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// One failed slot attempt inside [`Error::NoSlotUnlocked`].
#[derive(Debug)]
pub struct SlotAttempt {
  /// The slot that was tried.
  pub slot: SlotId,
  /// Its provider kind.
  pub kind: ProviderKind,
  /// Why it failed.
  pub error: Error,
}

/// Errors from key slot handling and key providers.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
  // ---- retryable -------------------------------------------------------
  /// The provider's backing store is locked (e.g. KWallet closed, keyring
  /// locked) and the provider is not allowed to prompt. Retryable.
  #[error("{0} key store is locked")]
  Locked(ProviderKind),
  /// The provider's backing store cannot be reached right now (no Secret
  /// Service on the bus, no default collection yet, session key lost on
  /// restart, ...). Retryable.
  #[error("{kind} key store unavailable: {reason}")]
  Unavailable {
    /// Provider kind.
    kind: ProviderKind,
    /// Human-readable reason (never contains secrets).
    reason: String,
  },
  /// A user prompt (unlock dialog) was dismissed. Retryable.
  #[error("{0} unlock prompt was dismissed")]
  Dismissed(ProviderKind),
  /// The provider needs a secret from the user (a passphrase, or a FIDO2
  /// PIN) and none was supplied. Retryable: prompt the user and try again
  /// with a provider constructed with the secret. See
  /// [`Error::needs_secret`].
  #[error("{0} needs a passphrase or PIN from the user")]
  NeedsSecret(ProviderKind),

  // ---- fatal for the slot ------------------------------------------------
  /// The key-encryption key did not authenticate the wrapped data key: wrong
  /// KEK, tampered slot, or slot moved to another file. Fatal.
  #[error("slot key does not decrypt the wrapped data key (wrong key or tampered slot)")]
  WrongKey,
  /// The PIN supplied for a FIDO2 security key was wrong. Not retryable as
  /// is, but the caller should re-prompt (the key decrements its PIN retry
  /// counter on every wrong PIN).
  #[error("{0}: wrong PIN")]
  PinInvalid(ProviderKind),
  /// The FIDO2 security key's PIN (or built-in user verification) is
  /// blocked: too many wrong attempts, or too many in a row without
  /// re-plugging the key. Fatal; the user has to re-plug or reset the key.
  #[error("{0}: PIN / user verification is blocked on the security key")]
  PinBlocked(ProviderKind),
  /// The provider has no secret for this slot (deleted from the keyring).
  /// Fatal for this slot.
  #[error("{0} has no secret for this slot")]
  SecretMissing(ProviderKind),
  /// Provider-side data is malformed (e.g. keyring secret has the wrong
  /// length, duplicate items). Fatal.
  #[error("{kind} key store returned unusable data: {reason}")]
  ProviderCorrupt {
    /// Provider kind.
    kind: ProviderKind,
    /// Reason (never contains secrets).
    reason: String,
  },
  /// Unexpected provider failure that is not known to be transient. Fatal.
  #[error("{kind} key store error: {reason}")]
  Provider {
    /// Provider kind.
    kind: ProviderKind,
    /// Reason (never contains secrets).
    reason: String,
  },
  /// The provider kind exists in the format but has no implementation yet.
  #[error("{0} provider is not implemented yet")]
  NotImplemented(ProviderKind),
  /// No provider instance of the slot's kind was passed in.
  #[error("no {0} provider available")]
  NoProvider(ProviderKind),
  /// Every slot failed. Retryable iff at least one attempt was retryable.
  #[error("no key slot could be unlocked ({})", summarize(.attempts))]
  NoSlotUnlocked {
    /// One entry per slot that was tried (or skipped for lack of provider).
    attempts: Vec<SlotAttempt>,
  },

  // ---- keyslots.json -----------------------------------------------------
  /// The keyslots file is malformed or fails validation.
  #[error("keyslots file {path}: {reason}")]
  Corrupt {
    /// File path.
    path: PathBuf,
    /// Reason.
    reason: String,
  },
  /// The keyslots file is larger than [`crate::MAX_FILE_SIZE`].
  #[error("keyslots file {path} is too large ({size} bytes)")]
  TooLarge {
    /// File path.
    path: PathBuf,
    /// Observed size.
    size: u64,
  },
  /// The keyslots file has a format version this build cannot read.
  #[error("keyslots file {path} has unsupported version {version}")]
  UnsupportedVersion {
    /// File path.
    path: PathBuf,
    /// Version found.
    version: u64,
  },
  /// `create_new` refused to overwrite an existing keyslots file.
  #[error("keyslots file {0} already exists")]
  AlreadyExists(PathBuf),
  /// The keyslots file does not exist.
  #[error("keyslots file {0} does not exist")]
  NotFound(PathBuf),
  /// No slot with that id.
  #[error("no such key slot {0}")]
  NoSuchSlot(SlotId),
  /// Refused to remove the last slot without `force`.
  #[error("refusing to remove the last key slot without force")]
  LastSlot,
  /// Too many slots.
  #[error("too many key slots (max {0})")]
  TooManySlots(usize),
  /// Some provider secrets could not be destroyed during wipe / slot removal.
  /// The keyslots file was still updated / deleted.
  #[error("could not destroy {} provider secret(s): {}", .failures.len(), summarize(.failures))]
  DestroyIncomplete {
    /// The slots whose provider secret is still present (or unknown).
    failures: Vec<SlotAttempt>,
  },
  /// Filesystem error.
  #[error("{context}: {source}")]
  Io {
    /// What was being done.
    context: String,
    /// Underlying error.
    #[source]
    source: std::io::Error,
  },
}

fn summarize(attempts: &[SlotAttempt]) -> String {
  struct S<'a>(&'a [SlotAttempt]);
  impl fmt::Display for S<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
      for (i, a) in self.0.iter().enumerate() {
        if i > 0 {
          f.write_str("; ")?;
        }
        write!(f, "slot {} ({}): {}", a.slot, a.kind, a.error)?;
      }
      Ok(())
    }
  }
  S(attempts).to_string()
}

impl Error {
  /// True if retrying later (e.g. after the wallet is opened) may succeed.
  ///
  /// For [`Error::NoSlotUnlocked`] this is true iff at least one slot failed
  /// for a retryable reason.
  pub fn is_retryable(&self) -> bool {
    match self {
      Error::Locked(_)
      | Error::Unavailable { .. }
      | Error::Dismissed(_)
      | Error::NeedsSecret(_) => true,
      Error::NoSlotUnlocked { attempts } => attempts.iter().any(|a| a.error.is_retryable()),
      _ => false,
    }
  }

  /// True if this is (or, for [`Error::NoSlotUnlocked`], contains) a
  /// [`Error::Locked`]: a wallet/keyring that will become usable once the user
  /// unlocks it. The daemon should wait for the store to unlock (see
  /// [`crate::SecretServiceProvider::wait_for_unlock`]) and retry.
  pub fn is_locked(&self) -> bool {
    match self {
      Error::Locked(_) => true,
      Error::NoSlotUnlocked { attempts } => attempts.iter().any(|a| a.error.is_locked()),
      _ => false,
    }
  }

  /// True if this is (or, for [`Error::NoSlotUnlocked`], contains) an
  /// [`Error::NeedsSecret`] or [`Error::PinInvalid`]: unlocking can proceed
  /// once the user types a passphrase / PIN. Use
  /// [`Error::secret_kinds`] to learn which prompt(s) to show.
  pub fn needs_secret(&self) -> bool {
    !self.secret_kinds().is_empty()
  }

  /// The provider kinds that asked for a passphrase / PIN
  /// ([`Error::NeedsSecret`]) or rejected the one given
  /// ([`Error::PinInvalid`]), deduplicated, in attempt order.
  pub fn secret_kinds(&self) -> Vec<ProviderKind> {
    let mut out = Vec::new();
    self.collect_secret_kinds(&mut out);
    out
  }

  fn collect_secret_kinds(&self, out: &mut Vec<ProviderKind>) {
    match self {
      Error::NeedsSecret(k) | Error::PinInvalid(k) => {
        if !out.contains(k) {
          out.push(*k);
        }
      }
      Error::NoSlotUnlocked { attempts } => {
        for a in attempts {
          a.error.collect_secret_kinds(out);
        }
      }
      _ => {}
    }
  }

  pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
    Error::Io { context: context.into(), source }
  }

  pub(crate) fn corrupt(path: &std::path::Path, reason: impl Into<String>) -> Self {
    Error::Corrupt { path: path.to_path_buf(), reason: reason.into() }
  }
}
