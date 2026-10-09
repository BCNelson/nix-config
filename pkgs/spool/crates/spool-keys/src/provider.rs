//! The [`KeyProvider`] trait, provider kinds and per-slot provider params.

use std::collections::BTreeMap;
use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::Result;
use crate::fido2::Fido2Params;
use crate::passphrase::PassphraseParams;
use crate::slots::SlotId;

/// A 256-bit key-encryption key (KEK) held by a provider. Zeroized on drop.
pub type Kek = Zeroizing<[u8; 32]>;

/// The kind of unlock method behind a slot. Serialized in kebab-case
/// (`"secret-service"`, `"session"`, `"passphrase"`, `"fido2-hmac"`, `"tpm2"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
  /// freedesktop Secret Service (KWallet 6, gnome-keyring, oo7-daemon).
  SecretService,
  /// In-memory only; never survives a process restart.
  Session,
  /// Passphrase + Argon2id ([`crate::PassphraseProvider`]).
  Passphrase,
  /// FIDO2 hmac-secret ([`crate::Fido2Provider`]).
  Fido2Hmac,
  /// TPM2 sealed key. Not implemented yet.
  Tpm2,
}

impl ProviderKind {
  /// The on-disk / wire name.
  pub fn as_str(self) -> &'static str {
    match self {
      ProviderKind::SecretService => "secret-service",
      ProviderKind::Session => "session",
      ProviderKind::Passphrase => "passphrase",
      ProviderKind::Fido2Hmac => "fido2-hmac",
      ProviderKind::Tpm2 => "tpm2",
    }
  }

  /// Whether this crate has a provider implementation for the kind.
  pub fn is_implemented(self) -> bool {
    matches!(
      self,
      ProviderKind::SecretService
        | ProviderKind::Session
        | ProviderKind::Passphrase
        | ProviderKind::Fido2Hmac
    )
  }
}

impl fmt::Display for ProviderKind {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.as_str())
  }
}

/// Provider-specific, non-secret parameters stored in a slot's `params`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SlotParams {
  /// `{"attributes": {"application": "spool", "slot": "<uuid>"}}`: the
  /// Secret Service lookup attributes of the item holding the KEK.
  SecretService {
    /// Lookup attributes (exactly `application` and `slot`).
    attributes: BTreeMap<String, String>,
  },
  /// `{}`.
  Session,
  /// `{"kdf":"argon2id","m_kib":262144,"t":3,"p":1,"salt":"<b64>"}`.
  Passphrase(PassphraseParams),
  /// `{"credential_id":"<b64>","salt":"<b64>","rp_id":"spool:clipboard","uv":false}`.
  Fido2Hmac(Fido2Params),
  /// Params of a kind this build does not implement, kept verbatim so the
  /// slot survives a load/save round trip. Always a JSON object.
  Opaque(serde_json::Value),
}

impl SlotParams {
  pub(crate) fn to_json(&self) -> serde_json::Value {
    match self {
      SlotParams::SecretService { attributes } => {
        serde_json::json!({ "attributes": attributes })
      }
      SlotParams::Session => serde_json::json!({}),
      SlotParams::Passphrase(p) => p.to_json(),
      SlotParams::Fido2Hmac(p) => p.to_json(),
      SlotParams::Opaque(v) => v.clone(),
    }
  }
}

/// An unlock method: owns a key-encryption key (KEK) per slot in some backing
/// store and can create, recover and destroy it.
///
/// This uses `#[async_trait]` rather than native `async fn` in traits because
/// [`crate::KeySlots::unlock_any`] takes `&[&dyn KeyProvider]`: native async
/// trait methods are not dyn-compatible (and could not promise `Send` futures
/// for use on the multi-threaded tokio runtime). The cost is one boxed future
/// per call, which is irrelevant for a handful of unlock operations.
#[async_trait]
pub trait KeyProvider: Send + Sync {
  /// Which kind of slot this provider serves.
  fn kind(&self) -> ProviderKind;

  /// Whether using this provider may require user interaction (a prompt,
  /// touching a token, typing a passphrase). `unlock_any` tries
  /// non-interactive providers first.
  fn interactive(&self) -> bool;

  /// Create a fresh KEK, persist it in the provider's backing store, return
  /// the slot params and the KEK.
  async fn enroll(&self, slot_id: &SlotId) -> Result<(SlotParams, Kek)>;

  /// Recover the KEK for an existing slot.
  async fn unlock(&self, slot_id: &SlotId, params: &SlotParams) -> Result<Kek>;

  /// Destroy the provider-side secret (for wipe / slot removal). Destroying a
  /// secret that is already gone is not an error.
  async fn destroy(&self, slot_id: &SlotId, params: &SlotParams) -> Result<()>;
}
