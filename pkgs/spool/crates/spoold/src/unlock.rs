//! Interactive unlock from the picker's unlock panel (picker protocol v2,
//! `PickerReq::Unlock`).
//!
//! The background key flow ([`crate::keyflow`]) only ever uses the
//! non-prompting Secret Service provider. Passphrase and FIDO2 slots need the
//! user: the picker sends the passphrase / PIN (or "start waiting for a
//! touch"), [`attempt`] tries every slot of that provider's kind and opens
//! the store through the shared [`OpenGate`], and the host
//! ([`crate::picker`]) hands the result to the orchestrator as
//! `KeyEvent::Opened`, i.e. exactly the key flow's merge + switch path.
//!
//! Error mapping (INTERFACES.md "Picker channel"): `WrongKey` ->
//! `WrongSecret`; `PinInvalid`, `PinBlocked`, `Dismissed` -> themselves;
//! `Unavailable` / `Locked` / `NoProvider` -> `Unavailable`;
//! `NeedsSecret(Fido2Hmac)` -> no failure, the prompt set gains
//! `Fido2Pin`; several slots -> the most specific reason; anything else ->
//! `Other` (details only in the log).
//!
//! Secrets live in `Zeroizing` buffers inside the providers and are never
//! logged or formatted.

use std::path::Path;

use spool_crypto::Zeroizing;
use spool_keys::{
  Fido2Provider, KeyProvider, KeySlots, PassphraseProvider, ProviderKind, SecretServiceProvider,
  SlotParams,
};
use spool_proto::{UnlockFailReason, UnlockPrompt};

use crate::keyflow::{GateError, OpenGate, OpenedStore};

/// Builds the key providers an unlock uses (tests substitute fakes).
pub trait UnlockProviders: Send + Sync + 'static {
  fn passphrase(&self, passphrase: Zeroizing<String>) -> Box<dyn KeyProvider>;
  fn fido2(&self, pin: Option<Zeroizing<String>>) -> Box<dyn KeyProvider>;
  /// The Secret Service slot, retried on the user's request (may prompt:
  /// the user asked for it).
  fn secret_service(&self) -> Box<dyn KeyProvider>;
}

/// The real providers (Argon2id passphrase, libfido2, oo7).
#[derive(Debug, Default)]
pub struct SystemProviders;

impl UnlockProviders for SystemProviders {
  fn passphrase(&self, passphrase: Zeroizing<String>) -> Box<dyn KeyProvider> {
    Box::new(PassphraseProvider::new(passphrase))
  }

  fn fido2(&self, pin: Option<Zeroizing<String>>) -> Box<dyn KeyProvider> {
    Box::new(Fido2Provider::new(pin))
  }

  fn secret_service(&self) -> Box<dyn KeyProvider> {
    Box::new(SecretServiceProvider::new(true))
  }
}

/// Result of one [`attempt`].
#[derive(Debug)]
pub enum Attempt {
  /// The store is open; hand it to the orchestrator.
  Opened(Box<OpenedStore>),
  /// Opened already (by the key flow or another attempt): nothing to do.
  AlreadyOpen,
  /// A FIDO2 key wants its PIN: re-prompt with `Fido2Pin`, not a failure.
  NeedsPin,
  Failed(UnlockFailReason),
}

/// One slot's outcome, as the picker sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mapped {
  NeedsPin,
  Fail(UnlockFailReason),
}

/// Map a provider / slot error (see the module docs).
pub fn map_error(e: &spool_keys::Error) -> Mapped {
  use spool_keys::Error as E;
  match e {
    E::WrongKey => Mapped::Fail(UnlockFailReason::WrongSecret),
    E::PinInvalid(_) => Mapped::Fail(UnlockFailReason::PinInvalid),
    E::PinBlocked(_) => Mapped::Fail(UnlockFailReason::PinBlocked),
    E::Dismissed(_) => Mapped::Fail(UnlockFailReason::Dismissed),
    E::Unavailable { .. } | E::Locked(_) | E::NoProvider(_) => {
      Mapped::Fail(UnlockFailReason::Unavailable)
    }
    E::NeedsSecret(ProviderKind::Fido2Hmac) => Mapped::NeedsPin,
    // A passphrase slot asked without a passphrase: treat as wrong.
    E::NeedsSecret(ProviderKind::Passphrase) => Mapped::Fail(UnlockFailReason::WrongSecret),
    E::NoSlotUnlocked { attempts } => attempts
      .iter()
      .map(|a| map_error(&a.error))
      .max_by_key(|m| specificity(*m))
      .unwrap_or(Mapped::Fail(UnlockFailReason::Other)),
    _ => Mapped::Fail(UnlockFailReason::Other),
  }
}

/// Higher = more useful to show the user when several slots failed.
fn specificity(m: Mapped) -> u8 {
  match m {
    Mapped::NeedsPin => 7,
    Mapped::Fail(UnlockFailReason::PinBlocked) => 6,
    Mapped::Fail(UnlockFailReason::PinInvalid) => 5,
    Mapped::Fail(UnlockFailReason::WrongSecret) => 4,
    Mapped::Fail(UnlockFailReason::Dismissed) => 3,
    Mapped::Fail(UnlockFailReason::Unavailable) => 2,
    Mapped::Fail(UnlockFailReason::Other) => 1,
  }
}

/// Try every slot of `provider`'s kind in `dir/keyslots.json`, then open
/// the store through `gate`. Never logs the secret (the providers hold it).
pub async fn attempt(dir: &Path, provider: &dyn KeyProvider, gate: &OpenGate) -> Attempt {
  let kind = provider.kind();
  if crate::keyflow::interrupted_rotation(dir) {
    tracing::warn!(provider = %kind, "unlock refused: {}", crate::keyflow::INTERRUPTED_ROTATION);
    return Attempt::Failed(UnlockFailReason::Unavailable);
  }
  let path = dir.join(spool_keys::FILE_NAME);
  let slots = match KeySlots::load(&path) {
    Ok(s) => s,
    Err(e) => {
      tracing::warn!(provider = %kind, "unlock: cannot read {}: {e}", path.display());
      return Attempt::Failed(match e {
        spool_keys::Error::NotFound(_) => UnlockFailReason::Unavailable,
        _ => UnlockFailReason::Other,
      });
    }
  };
  let mut best: Option<Mapped> = None;
  let mut tried = 0usize;
  for slot in slots.slots().iter().filter(|s| s.kind() == kind) {
    tried += 1;
    let m = match slots.unlock_slot(&slot.id(), provider).await {
      Ok(key) => match gate.open(dir, key).await {
        Ok(opened) => {
          tracing::info!(slot = %slot.id(), provider = %kind, "history unlocked from the picker");
          return Attempt::Opened(Box::new(opened));
        }
        Err(GateError::AlreadyOpen) => return Attempt::AlreadyOpen,
        Err(GateError::Store(spool_core::Error::WrongKey)) => {
          tracing::warn!(slot = %slot.id(), "key slot yields a key that does not open the history");
          Mapped::Fail(UnlockFailReason::WrongSecret)
        }
        Err(GateError::Store(e)) => {
          tracing::error!(slot = %slot.id(), "unlock: opening the encrypted history: {e}");
          Mapped::Fail(UnlockFailReason::Other)
        }
      },
      Err(e) => {
        let m = map_error(&e);
        // Expected outcomes at info, the rest at warn; never the secret.
        match m {
          Mapped::Fail(UnlockFailReason::Other) => {
            tracing::warn!(slot = %slot.id(), provider = %kind, "unlock failed: {e}")
          }
          _ => tracing::info!(slot = %slot.id(), provider = %kind, "unlock did not succeed: {e}"),
        }
        m
      }
    };
    if best.is_none_or(|b| specificity(m) > specificity(b)) {
      best = Some(m);
    }
  }
  if tried == 0 {
    tracing::info!(provider = %kind, "unlock: no key slot of this kind");
    return Attempt::Failed(UnlockFailReason::Unavailable);
  }
  match best {
    Some(Mapped::NeedsPin) => Attempt::NeedsPin,
    Some(Mapped::Fail(r)) => Attempt::Failed(r),
    None => Attempt::Failed(UnlockFailReason::Other),
  }
}

/// The unlock prompts `dir/keyslots.json` allows: `Passphrase` for a
/// passphrase slot, `Fido2Touch` for a FIDO2 slot, plus `Fido2Pin` when a
/// FIDO2 slot uses user verification or a key asked for its PIN
/// (`need_pin`). Empty = nothing to prompt for (e.g. waiting for KWallet,
/// or an interrupted key rotation that `spool-keyctl recover` must finish).
pub fn prompts(dir: &Path, need_pin: bool) -> Vec<UnlockPrompt> {
  if crate::keyflow::interrupted_rotation(dir) {
    return Vec::new();
  }
  let Ok(slots) = KeySlots::load(dir.join(spool_keys::FILE_NAME)) else { return Vec::new() };
  let mut pass = false;
  let mut fido = false;
  let mut uv = false;
  for s in slots.slots() {
    match (s.kind(), s.params()) {
      (ProviderKind::Passphrase, _) => pass = true,
      (ProviderKind::Fido2Hmac, SlotParams::Fido2Hmac(p)) => {
        fido = true;
        uv |= p.uv;
      }
      (ProviderKind::Fido2Hmac, _) => fido = true,
      _ => {}
    }
  }
  let mut out = Vec::new();
  if pass {
    out.push(UnlockPrompt::Passphrase);
  }
  if fido {
    out.push(UnlockPrompt::Fido2Touch);
    if need_pin || uv {
      out.push(UnlockPrompt::Fido2Pin);
    }
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn error_mapping() {
    use spool_keys::Error as E;
    let k = ProviderKind::Fido2Hmac;
    let f = Mapped::Fail;
    assert_eq!(map_error(&E::WrongKey), f(UnlockFailReason::WrongSecret));
    assert_eq!(map_error(&E::PinInvalid(k)), f(UnlockFailReason::PinInvalid));
    assert_eq!(map_error(&E::PinBlocked(k)), f(UnlockFailReason::PinBlocked));
    assert_eq!(map_error(&E::Dismissed(k)), f(UnlockFailReason::Dismissed));
    assert_eq!(map_error(&E::Locked(k)), f(UnlockFailReason::Unavailable));
    assert_eq!(map_error(&E::NoProvider(k)), f(UnlockFailReason::Unavailable));
    assert_eq!(
      map_error(&E::Unavailable { kind: k, reason: "no key".into() }),
      f(UnlockFailReason::Unavailable)
    );
    assert_eq!(map_error(&E::NeedsSecret(k)), Mapped::NeedsPin);
    assert_eq!(map_error(&E::SecretMissing(k)), f(UnlockFailReason::Other));
    let slot = spool_keys::SlotId::new();
    let nsu = E::NoSlotUnlocked {
      attempts: vec![
        spool_keys::SlotAttempt {
          slot,
          kind: k,
          error: E::Unavailable { kind: k, reason: "x".into() },
        },
        spool_keys::SlotAttempt { slot, kind: k, error: E::PinInvalid(k) },
        spool_keys::SlotAttempt { slot, kind: k, error: E::Dismissed(k) },
      ],
    };
    assert_eq!(map_error(&nsu), f(UnlockFailReason::PinInvalid));
    assert_eq!(map_error(&E::NoSlotUnlocked { attempts: vec![] }), f(UnlockFailReason::Other));
  }
}
