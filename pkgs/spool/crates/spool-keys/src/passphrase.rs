//! The `passphrase` provider: KEK = Argon2id(passphrase, per-slot salt).
//!
//! Slot params:
//!
//! ```json
//! {"kdf":"argon2id","m_kib":262144,"t":3,"p":1,"salt":"<base64 32 bytes>"}
//! ```
//!
//! * **KDF.** Argon2id v1.3, 32-byte output, a fresh random 32-byte salt per
//!   slot. New slots use [`ARGON2_M_KIB`] (256 MiB), [`ARGON2_T`] (3 passes),
//!   [`ARGON2_P`] (1 lane). One derivation costs roughly 0.4 s (measured on
//!   a current desktop, optimised build) to 1.5 s (older / low-power CPUs)
//!   and 256 MiB of RAM, many times more in a debug build, so it runs on
//!   tokio's blocking pool (`spawn_blocking`) and never stalls the runtime.
//!   A dropped future does not stop a derivation that already started.
//! * **Downgrade protection.** Params are parsed strictly (unknown fields,
//!   non-`argon2id` KDFs, salts that are not 32 bytes are rejected) and must
//!   be at least [`MIN_M_KIB`] (64 MiB) and [`MIN_T`] (2 passes), so a
//!   tampered `keyslots.json` cannot make the daemon derive with weak
//!   params. Upper bounds ([`MAX_M_KIB`], [`MAX_T`], [`MAX_P`]) stop a
//!   tampered file from exhausting memory / CPU instead.
//! * **Unicode.** The passphrase is NFC-normalized before hashing, so the
//!   same passphrase typed through different input methods (precomposed
//!   `é` vs `e` + combining acute) yields the same key. No other folding
//!   (case, width, whitespace) is done.
//! * **Wrong passphrase.** The provider cannot tell: it derives *a* KEK and
//!   unwrapping the data key then fails with [`Error::WrongKey`]. That is
//!   fatal for the attempt, but for a passphrase slot the daemon should
//!   simply re-prompt.
//! * **No passphrase.** [`PassphraseProvider::without_passphrase`] makes
//!   `enroll`/`unlock` fail with the retryable [`Error::NeedsSecret`] (so
//!   [`crate::KeySlots::unlock_any`] can report "a passphrase would unlock
//!   this") and is enough for `destroy` (a no-op: nothing lives outside the
//!   slot, removing the slot removes the secret).

use std::fmt;

use argon2::{Algorithm, Argon2, Params, Version};
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use rand::RngCore;
use serde::Deserialize;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::provider::{Kek, KeyProvider, ProviderKind, SlotParams};
use crate::slots::SlotId;

/// Argon2id memory cost for new slots, in KiB (256 MiB).
pub const ARGON2_M_KIB: u32 = 256 * 1024;
/// Argon2id passes for new slots.
pub const ARGON2_T: u32 = 3;
/// Argon2id lanes for new slots.
pub const ARGON2_P: u32 = 1;
/// Minimum accepted memory cost, in KiB (64 MiB).
pub const MIN_M_KIB: u32 = 64 * 1024;
/// Minimum accepted passes.
pub const MIN_T: u32 = 2;
/// Maximum accepted memory cost, in KiB (4 GiB).
pub const MAX_M_KIB: u32 = 4 * 1024 * 1024;
/// Maximum accepted passes.
pub const MAX_T: u32 = 64;
/// Maximum accepted lanes.
pub const MAX_P: u32 = 16;
/// Salt length in bytes.
pub const SALT_LEN: usize = 32;
/// The only accepted `kdf` value.
pub const KDF_NAME: &str = "argon2id";

/// Non-secret parameters of a passphrase slot.
///
/// The salt is not secret, but `Debug` still omits it (this crate never
/// formats salts, PINs, passphrases or KEKs).
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PassphraseParams {
  /// Argon2id memory cost in KiB.
  pub m_kib: u32,
  /// Argon2id passes.
  pub t: u32,
  /// Argon2id lanes.
  pub p: u32,
  /// Per-slot random salt.
  pub salt: [u8; SALT_LEN],
}

impl fmt::Debug for PassphraseParams {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("PassphraseParams")
      .field("kdf", &KDF_NAME)
      .field("m_kib", &self.m_kib)
      .field("t", &self.t)
      .field("p", &self.p)
      .finish_non_exhaustive()
  }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Repr {
  kdf: String,
  m_kib: u32,
  t: u32,
  p: u32,
  salt: String,
}

#[cfg(test)]
thread_local! {
  /// Tests that need fast round trips through `KeySlots` lower the floor on
  /// their own thread; everything else (including the downgrade tests) sees
  /// the real floor.
  static RELAXED_FLOOR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Lower the param floor on the current thread (tests only).
#[cfg(test)]
pub(crate) fn relax_floor_for_tests() {
  RELAXED_FLOOR.with(|c| c.set(true));
}

fn floor() -> (u32, u32) {
  #[cfg(test)]
  if RELAXED_FLOOR.with(|c| c.get()) {
    return (Params::MIN_M_COST, 1);
  }
  (MIN_M_KIB, MIN_T)
}

impl PassphraseParams {
  /// Check bounds (floor and ceiling) of the KDF params.
  pub fn validate(&self) -> std::result::Result<(), String> {
    let (min_m, min_t) = floor();
    if self.m_kib < min_m {
      return Err(format!("m_kib {} is below the minimum {min_m}", self.m_kib));
    }
    if self.m_kib > MAX_M_KIB {
      return Err(format!("m_kib {} is above the maximum {MAX_M_KIB}", self.m_kib));
    }
    if self.t < min_t {
      return Err(format!("t {} is below the minimum {min_t}", self.t));
    }
    if self.t > MAX_T {
      return Err(format!("t {} is above the maximum {MAX_T}", self.t));
    }
    if self.p == 0 || self.p > MAX_P {
      return Err(format!("p {} is outside 1..={MAX_P}", self.p));
    }
    if self.m_kib < 8 * self.p {
      return Err(format!("m_kib {} is below 8 * p", self.m_kib));
    }
    Ok(())
  }

  /// Strictly parse (and validate) the JSON form.
  pub(crate) fn from_json(v: serde_json::Value) -> std::result::Result<Self, String> {
    let r: Repr = serde_json::from_value(v).map_err(|e| e.to_string())?;
    if r.kdf != KDF_NAME {
      return Err(format!("unsupported kdf {:?} (only {KDF_NAME:?})", r.kdf));
    }
    let salt = B64.decode(r.salt.as_bytes()).map_err(|e| format!("bad base64 in \"salt\": {e}"))?;
    let salt: [u8; SALT_LEN] = salt
      .try_into()
      .map_err(|s: Vec<u8>| format!("\"salt\" must be {SALT_LEN} bytes, got {}", s.len()))?;
    let p = PassphraseParams { m_kib: r.m_kib, t: r.t, p: r.p, salt };
    p.validate()?;
    Ok(p)
  }

  pub(crate) fn to_json(&self) -> serde_json::Value {
    serde_json::json!({
      "kdf": KDF_NAME,
      "m_kib": self.m_kib,
      "t": self.t,
      "p": self.p,
      "salt": B64.encode(self.salt),
    })
  }
}

/// NFC-normalize into a zeroizing buffer. The buffer is pre-sized so it
/// normally never reallocates; if it must grow, the old buffer is zeroized.
fn nfc(s: &str) -> Zeroizing<String> {
  let mut out = Zeroizing::new(String::with_capacity(s.len() * 3 + 16));
  for c in s.nfc() {
    if out.len() + c.len_utf8() > out.capacity() {
      let mut bigger = Zeroizing::new(String::with_capacity(out.capacity() * 2));
      bigger.push_str(&out);
      out = bigger;
    }
    out.push(c);
  }
  out
}

/// Argon2id(NFC(passphrase), salt) -> 32 bytes. Blocking and expensive.
fn derive(passphrase: &str, params: &PassphraseParams) -> Result<Kek> {
  let provider_err = |e: argon2::Error| Error::Provider {
    kind: ProviderKind::Passphrase,
    reason: format!("argon2id: {e}"),
  };
  let a2 = Argon2::new(
    Algorithm::Argon2id,
    Version::V0x13,
    Params::new(params.m_kib, params.t, params.p, Some(32)).map_err(provider_err)?,
  );
  let pw = nfc(passphrase);
  let mut kek = Zeroizing::new([0u8; 32]);
  a2.hash_password_into(pw.as_bytes(), &params.salt, kek.as_mut()).map_err(provider_err)?;
  Ok(kek)
}

async fn derive_blocking(passphrase: Zeroizing<String>, params: PassphraseParams) -> Result<Kek> {
  tokio::task::spawn_blocking(move || derive(&passphrase, &params)).await.map_err(|e| {
    Error::Provider { kind: ProviderKind::Passphrase, reason: format!("KDF task failed: {e}") }
  })?
}

/// Passphrase provider. Holds the passphrase (zeroized on drop) that the
/// caller collected, e.g. from the picker's unlock prompt.
#[derive(Clone)]
pub struct PassphraseProvider {
  passphrase: Option<Zeroizing<String>>,
  m_kib: u32,
  t: u32,
  p: u32,
}

impl PassphraseProvider {
  /// A provider that enrolls / unlocks with `passphrase`. New slots use the
  /// default cost ([`ARGON2_M_KIB`], [`ARGON2_T`], [`ARGON2_P`]); unlocking
  /// uses whatever (validated) params the slot stores.
  pub fn new(passphrase: Zeroizing<String>) -> Self {
    PassphraseProvider {
      passphrase: Some(passphrase),
      m_kib: ARGON2_M_KIB,
      t: ARGON2_T,
      p: ARGON2_P,
    }
  }

  /// A provider with no passphrase: `enroll` / `unlock` return
  /// [`Error::NeedsSecret`]; `destroy` works (it is a no-op).
  pub fn without_passphrase() -> Self {
    PassphraseProvider { passphrase: None, m_kib: ARGON2_M_KIB, t: ARGON2_T, p: ARGON2_P }
  }

  /// Test-only: enroll with cheap KDF params. They still have to pass
  /// [`PassphraseParams::validate`], so tests must call
  /// `relax_floor_for_tests` on their thread first.
  #[cfg(test)]
  pub(crate) fn with_test_cost(passphrase: Zeroizing<String>, m_kib: u32, t: u32, p: u32) -> Self {
    PassphraseProvider { passphrase: Some(passphrase), m_kib, t, p }
  }

  fn passphrase(&self) -> Result<Zeroizing<String>> {
    self.passphrase.clone().ok_or(Error::NeedsSecret(ProviderKind::Passphrase))
  }
}

impl fmt::Debug for PassphraseProvider {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("PassphraseProvider")
      .field("has_passphrase", &self.passphrase.is_some())
      .field("m_kib", &self.m_kib)
      .field("t", &self.t)
      .field("p", &self.p)
      .finish()
  }
}

#[async_trait]
impl KeyProvider for PassphraseProvider {
  fn kind(&self) -> ProviderKind {
    ProviderKind::Passphrase
  }

  fn interactive(&self) -> bool {
    true
  }

  async fn enroll(&self, _slot_id: &SlotId) -> Result<(SlotParams, Kek)> {
    let pw = self.passphrase()?;
    if pw.is_empty() {
      return Err(Error::Provider {
        kind: ProviderKind::Passphrase,
        reason: "refusing to enroll an empty passphrase".into(),
      });
    }
    let mut salt = [0u8; SALT_LEN];
    rand::rng().fill_bytes(&mut salt);
    let params = PassphraseParams { m_kib: self.m_kib, t: self.t, p: self.p, salt };
    params
      .validate()
      .map_err(|reason| Error::Provider { kind: ProviderKind::Passphrase, reason })?;
    let kek = derive_blocking(pw, params.clone()).await?;
    Ok((SlotParams::Passphrase(params), kek))
  }

  async fn unlock(&self, _slot_id: &SlotId, params: &SlotParams) -> Result<Kek> {
    let SlotParams::Passphrase(params) = params else {
      return Err(Error::ProviderCorrupt {
        kind: ProviderKind::Passphrase,
        reason: "slot params are not passphrase params".into(),
      });
    };
    // Also checked on load; re-check in case params were built in memory.
    params
      .validate()
      .map_err(|reason| Error::ProviderCorrupt { kind: ProviderKind::Passphrase, reason })?;
    let pw = self.passphrase()?;
    derive_blocking(pw, params.clone()).await
  }

  async fn destroy(&self, _slot_id: &SlotId, _params: &SlotParams) -> Result<()> {
    Ok(())
  }
}

#[cfg(test)]
mod tests;
