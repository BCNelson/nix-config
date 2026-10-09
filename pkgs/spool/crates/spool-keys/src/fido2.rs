//! The `fido2-hmac` provider: KEK = the FIDO2 `hmac-secret` output of a
//! security key for a per-slot salt.
//!
//! Slot params:
//!
//! ```json
//! {"credential_id":"<base64>","salt":"<base64 32 bytes>","rp_id":"spool:clipboard","uv":false}
//! ```
//!
//! * **Enroll** creates a *non-resident* (non-discoverable) credential for
//!   relying party [`DEFAULT_RP_ID`] with the `hmac-secret` extension on one
//!   security key, then asks that key for `hmac-secret(salt)` with a fresh
//!   random 32-byte salt. Nothing is stored on the key (it derives the
//!   credential from the id we keep), so `destroy` is a no-op. User presence
//!   (a touch) is always required; enrolling costs **two touches** (make
//!   credential, then get assertion), unlocking one.
//! * **Several keys**: each physical key gets its own slot (enroll once per
//!   key). With more than one key connected, enroll needs
//!   [`Fido2Provider::with_device_path`] (see [`Fido2Provider::devices`]).
//!   Unlock probes every connected key silently (`up=false`) for the slot's
//!   credential and only asks the key that has it for a touch.
//! * **PIN / user verification.** `uv` records whether the slot was enrolled
//!   with user verification ([`Fido2Provider::require_uv`], default off). It
//!   is part of the params because the key returns a *different* hmac-secret
//!   with and without UV, so unlock must repeat what enroll did: for `uv:
//!   false` slots the PIN is never sent (sending it would switch the key to
//!   its UV secret). A key with a PIN set may still need the PIN to *create*
//!   the credential (CTAP 2.0 keys without `makeCredUvNotRqd`); without one,
//!   enroll fails with the retryable [`Error::NeedsSecret`] and the caller
//!   should prompt for the PIN and retry with [`Fido2Provider::new`]`(Some(pin))`.
//!
//! Errors: no key connected, or none of the connected keys holds the slot's
//! credential -> retryable [`Error::Unavailable`]; touch timed out or the
//! request was cancelled/denied on the key -> retryable [`Error::Dismissed`];
//! PIN needed -> [`Error::NeedsSecret`]; wrong PIN -> [`Error::PinInvalid`]
//! (re-prompt); PIN blocked -> [`Error::PinBlocked`]; key without
//! `hmac-secret` -> [`Error::Provider`].
//!
//! Device I/O blocks (it waits for the touch), so it runs on tokio's
//! blocking pool. The key's own timeout applies (about 15–30 s on YubiKeys);
//! dropping the future does not cancel a request already waiting for a touch.
//!
//! Hardware backend: libfido2 (Yubico's reference implementation, via the
//! `fido2-rs` bindings, cargo feature `fido2`, on by default). It talks to
//! `/dev/hidraw*`, which needs the user to have access to security-key
//! hidraw nodes (systemd's `uaccess` tag for `ID_SECURITY_TOKEN` devices, or
//! libfido2's udev rules). Built without the feature, every device operation
//! fails with [`Error::Unavailable`] while slots still load and validate.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use rand::RngCore;
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::provider::{Kek, KeyProvider, ProviderKind, SlotParams};
use crate::slots::SlotId;

#[cfg(feature = "fido2")]
mod libfido2;

/// Relying party id of credentials created by Spool. Not a web origin:
/// CTAP accepts any string, and a non-domain one cannot collide with a
/// website's credentials.
pub const DEFAULT_RP_ID: &str = "spool:clipboard";
/// Salt length in bytes (CTAP `hmac-secret` salts are 32 bytes).
pub const SALT_LEN: usize = 32;
/// Maximum accepted credential id length (CTAP 2.1 bound).
pub const MAX_CREDENTIAL_ID_LEN: usize = 1023;
/// Maximum accepted relying party id length.
pub const MAX_RP_ID_LEN: usize = 253;

const KIND: ProviderKind = ProviderKind::Fido2Hmac;

/// Non-secret parameters of a FIDO2 slot.
///
/// Neither value is secret on its own (the key's internal secret is what
/// matters), but `Debug` omits the credential id and salt anyway.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Fido2Params {
  /// The (non-resident) credential id returned by the key at enroll.
  pub credential_id: Vec<u8>,
  /// Per-slot random `hmac-secret` salt.
  pub salt: [u8; SALT_LEN],
  /// Relying party id the credential was created for.
  pub rp_id: String,
  /// Whether enroll (and so unlock) used user verification (PIN / bio).
  pub uv: bool,
}

impl fmt::Debug for Fido2Params {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Fido2Params")
      .field("credential_id_len", &self.credential_id.len())
      .field("rp_id", &self.rp_id)
      .field("uv", &self.uv)
      .finish_non_exhaustive()
  }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Repr {
  credential_id: String,
  salt: String,
  rp_id: String,
  uv: bool,
}

impl Fido2Params {
  /// Check lengths and the relying party id.
  pub fn validate(&self) -> std::result::Result<(), String> {
    if self.credential_id.is_empty() || self.credential_id.len() > MAX_CREDENTIAL_ID_LEN {
      return Err(format!(
        "\"credential_id\" must be 1..={MAX_CREDENTIAL_ID_LEN} bytes, got {}",
        self.credential_id.len()
      ));
    }
    if self.rp_id.is_empty()
      || self.rp_id.len() > MAX_RP_ID_LEN
      || !self.rp_id.bytes().all(|b| b.is_ascii_graphic())
    {
      return Err(format!(
        "\"rp_id\" must be 1..={MAX_RP_ID_LEN} printable ASCII characters without spaces"
      ));
    }
    Ok(())
  }

  /// Strictly parse (and validate) the JSON form.
  pub(crate) fn from_json(v: serde_json::Value) -> std::result::Result<Self, String> {
    let r: Repr = serde_json::from_value(v).map_err(|e| e.to_string())?;
    let credential_id = B64
      .decode(r.credential_id.as_bytes())
      .map_err(|e| format!("bad base64 in \"credential_id\": {e}"))?;
    let salt = B64.decode(r.salt.as_bytes()).map_err(|e| format!("bad base64 in \"salt\": {e}"))?;
    let salt: [u8; SALT_LEN] = salt
      .try_into()
      .map_err(|s: Vec<u8>| format!("\"salt\" must be {SALT_LEN} bytes, got {}", s.len()))?;
    let p = Fido2Params { credential_id, salt, rp_id: r.rp_id, uv: r.uv };
    p.validate()?;
    Ok(p)
  }

  pub(crate) fn to_json(&self) -> serde_json::Value {
    serde_json::json!({
      "credential_id": B64.encode(&self.credential_id),
      "salt": B64.encode(self.salt),
      "rp_id": self.rp_id,
      "uv": self.uv,
    })
  }
}

/// A connected FIDO security key.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Fido2Device {
  /// OS path (`/dev/hidrawN`), for [`Fido2Provider::with_device_path`].
  pub path: String,
  /// USB manufacturer string.
  pub manufacturer: String,
  /// USB product string.
  pub product: String,
}

// ---- backend seam ---------------------------------------------------------

/// What a device operation can fail with, independent of the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(feature = "fido2"), allow(dead_code))] // only the fake / libfido2 build them
pub(crate) enum DevError {
  /// The key has no such credential (wrong key).
  NoCredentials,
  /// A PIN is needed and none was given.
  PinRequired,
  /// Wrong PIN / failed built-in UV.
  PinInvalid,
  /// PIN / UV blocked.
  PinBlocked,
  /// No touch in time.
  Timeout,
  /// Cancelled or denied on the key.
  Denied,
  /// The key lacks a needed feature (`hmac-secret`, an option, a PIN).
  Unsupported(String),
  /// The device went away / I/O failed.
  Gone(String),
  /// Anything else.
  Other(String),
}

/// The CTAP operations the provider needs. The real implementation is
/// libfido2; tests use a fake that never touches hidraw.
pub(crate) trait Authenticator: Send + Sync {
  /// Enumerate connected keys.
  fn devices(&self) -> std::result::Result<Vec<Fido2Device>, DevError>;
  /// Create a non-resident credential with `hmac-secret`; returns its id.
  fn make_credential(
    &self,
    dev: &str,
    rp_id: &str,
    user_id: &[u8],
    uv: bool,
    pin: Option<&str>,
  ) -> std::result::Result<Vec<u8>, DevError>;
  /// Silently (no touch) check whether `dev` knows the credential.
  /// `Ok(true)` may also mean "cannot tell".
  fn has_credential(
    &self,
    dev: &str,
    rp_id: &str,
    credential_id: &[u8],
  ) -> std::result::Result<bool, DevError>;
  /// Get an assertion with user presence and return `hmac-secret(salt)`.
  fn hmac_secret(
    &self,
    dev: &str,
    rp_id: &str,
    credential_id: &[u8],
    salt: &[u8; SALT_LEN],
    uv: bool,
    pin: Option<&str>,
  ) -> std::result::Result<Kek, DevError>;
}

/// Backend used when the crate is built without the `fido2` feature.
#[cfg(not(feature = "fido2"))]
struct NoBackend;

#[cfg(not(feature = "fido2"))]
impl Authenticator for NoBackend {
  fn devices(&self) -> std::result::Result<Vec<Fido2Device>, DevError> {
    Err(DevError::Gone("spool-keys was built without FIDO2 support".into()))
  }
  fn make_credential(
    &self,
    _: &str,
    _: &str,
    _: &[u8],
    _: bool,
    _: Option<&str>,
  ) -> std::result::Result<Vec<u8>, DevError> {
    self.devices().map(|_| Vec::new())
  }
  fn has_credential(&self, _: &str, _: &str, _: &[u8]) -> std::result::Result<bool, DevError> {
    self.devices().map(|_| false)
  }
  fn hmac_secret(
    &self,
    _: &str,
    _: &str,
    _: &[u8],
    _: &[u8; SALT_LEN],
    _: bool,
    _: Option<&str>,
  ) -> std::result::Result<Kek, DevError> {
    self.devices().map(|_| Zeroizing::new([0u8; 32]))
  }
}

fn default_backend() -> Arc<dyn Authenticator> {
  #[cfg(feature = "fido2")]
  {
    Arc::new(libfido2::LibFido2)
  }
  #[cfg(not(feature = "fido2"))]
  {
    Arc::new(NoBackend)
  }
}

/// Map a device error to the crate error.
pub(crate) fn map_err(e: DevError) -> Error {
  match e {
    DevError::NoCredentials => Error::Unavailable {
      kind: KIND,
      reason: "no connected security key holds this slot's credential".into(),
    },
    DevError::PinRequired => Error::NeedsSecret(KIND),
    DevError::PinInvalid => Error::PinInvalid(KIND),
    DevError::PinBlocked => Error::PinBlocked(KIND),
    DevError::Timeout | DevError::Denied => Error::Dismissed(KIND),
    DevError::Unsupported(reason) => Error::Provider { kind: KIND, reason },
    DevError::Gone(reason) => Error::Unavailable { kind: KIND, reason },
    DevError::Other(reason) => Error::Provider { kind: KIND, reason },
  }
}

// ---- provider ---------------------------------------------------------------

/// FIDO2 `hmac-secret` provider.
///
/// Holds the optional PIN the caller collected (zeroized on drop).
#[derive(Clone)]
pub struct Fido2Provider {
  pin: Option<Zeroizing<String>>,
  require_uv: bool,
  device_path: Option<String>,
  backend: Arc<dyn Authenticator>,
}

impl Fido2Provider {
  /// A provider using the system's security keys (libfido2). `pin` is used
  /// when a key demands one (and for user verification on `uv` slots).
  /// Constructing it does not touch any device.
  pub fn new(pin: Option<Zeroizing<String>>) -> Self {
    Self::with_backend(default_backend(), pin)
  }

  pub(crate) fn with_backend(
    backend: Arc<dyn Authenticator>,
    pin: Option<Zeroizing<String>>,
  ) -> Self {
    Fido2Provider { pin, require_uv: false, device_path: None, backend }
  }

  /// Enroll new slots with user verification (PIN or built-in biometrics) in
  /// addition to presence. Default `false`. Affects only `enroll`; unlock
  /// follows the slot's stored `uv`.
  pub fn require_uv(mut self, require_uv: bool) -> Self {
    self.require_uv = require_uv;
    self
  }

  /// Enroll on this device (a path from [`Fido2Provider::devices`]) instead
  /// of "the only connected key". Unlock ignores it and probes all keys.
  pub fn with_device_path(mut self, path: impl Into<String>) -> Self {
    self.device_path = Some(path.into());
    self
  }

  /// List connected security keys (no touch needed).
  pub async fn devices(&self) -> Result<Vec<Fido2Device>> {
    let backend = self.backend.clone();
    blocking(move || backend.devices().map_err(map_err)).await
  }

  fn pin(&self) -> Option<Zeroizing<String>> {
    self.pin.clone()
  }
}

impl fmt::Debug for Fido2Provider {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Fido2Provider")
      .field("has_pin", &self.pin.is_some())
      .field("require_uv", &self.require_uv)
      .field("device_path", &self.device_path)
      .finish_non_exhaustive()
  }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
  tokio::task::spawn_blocking(f)
    .await
    .map_err(|e| Error::Provider { kind: KIND, reason: format!("device task failed: {e}") })?
}

fn enroll_blocking(
  backend: &dyn Authenticator,
  device_path: Option<&str>,
  user_id: &[u8],
  uv: bool,
  pin: Option<&str>,
) -> Result<(Fido2Params, Kek)> {
  let devices = backend.devices().map_err(map_err)?;
  let dev = match device_path {
    Some(p) => devices.into_iter().find(|d| d.path == p).ok_or_else(|| Error::Unavailable {
      kind: KIND,
      reason: format!("security key {p} is not connected"),
    })?,
    None => {
      let n = devices.len();
      match <[Fido2Device; 1]>::try_from(devices) {
        Ok([d]) => d,
        Err(_) if n == 0 => {
          return Err(Error::Unavailable {
            kind: KIND,
            reason: "no FIDO2 security key connected".into(),
          });
        }
        Err(_) => {
          return Err(Error::Unavailable {
            kind: KIND,
            reason: format!("{n} security keys connected; connect only one or choose a device"),
          });
        }
      }
    }
  };
  let rp_id = DEFAULT_RP_ID;
  // Creating the credential may need the PIN even for a non-UV slot.
  let credential_id =
    backend.make_credential(&dev.path, rp_id, user_id, uv, pin).map_err(map_err)?;
  let mut salt = [0u8; SALT_LEN];
  rand::rng().fill_bytes(&mut salt);
  let params = Fido2Params { credential_id, salt, rp_id: rp_id.to_owned(), uv };
  params.validate().map_err(|reason| Error::Provider { kind: KIND, reason })?;
  let kek = backend
    .hmac_secret(&dev.path, rp_id, &params.credential_id, &salt, uv, if uv { pin } else { None })
    .map_err(map_err)?;
  Ok((params, kek))
}

fn unlock_blocking(
  backend: &dyn Authenticator,
  params: &Fido2Params,
  pin: Option<&str>,
) -> Result<Kek> {
  let devices = backend.devices().map_err(map_err)?;
  if devices.is_empty() {
    return Err(Error::Unavailable {
      kind: KIND,
      reason: "no FIDO2 security key connected".into(),
    });
  }
  // A non-UV slot must never send the PIN: that would make the key answer
  // with its UV secret and the KEK would be wrong.
  let pin = if params.uv { pin } else { None };
  let mut last = DevError::NoCredentials;
  for dev in &devices {
    match backend.has_credential(&dev.path, &params.rp_id, &params.credential_id) {
      Ok(false) | Err(DevError::NoCredentials) => continue,
      // Cannot ask this key (unplugged mid-way, I/O error): try the others.
      Err(e @ DevError::Gone(_)) => {
        last = e;
        continue;
      }
      // `Ok(true)`, or the probe is unsupported / wants a PIN: ask for real.
      Ok(true) | Err(_) => {}
    }
    match backend.hmac_secret(
      &dev.path,
      &params.rp_id,
      &params.credential_id,
      &params.salt,
      params.uv,
      pin,
    ) {
      Ok(kek) => return Ok(kek),
      Err(DevError::NoCredentials) => continue,
      Err(e @ DevError::Gone(_)) => last = e,
      // The user interacted with the key that holds the credential (or it
      // needs a PIN): report that rather than trying other keys.
      Err(e) => return Err(map_err(e)),
    }
  }
  Err(map_err(last))
}

#[async_trait]
impl KeyProvider for Fido2Provider {
  fn kind(&self) -> ProviderKind {
    KIND
  }

  fn interactive(&self) -> bool {
    true
  }

  async fn enroll(&self, slot_id: &SlotId) -> Result<(SlotParams, Kek)> {
    let backend = self.backend.clone();
    let device_path = self.device_path.clone();
    let user_id = *slot_id.0.as_bytes();
    let uv = self.require_uv;
    let pin = self.pin();
    let (params, kek) = blocking(move || {
      enroll_blocking(
        &*backend,
        device_path.as_deref(),
        &user_id,
        uv,
        pin.as_deref().map(|s| s.as_str()),
      )
    })
    .await?;
    Ok((SlotParams::Fido2Hmac(params), kek))
  }

  async fn unlock(&self, _slot_id: &SlotId, params: &SlotParams) -> Result<Kek> {
    let SlotParams::Fido2Hmac(params) = params else {
      return Err(Error::ProviderCorrupt {
        kind: KIND,
        reason: "slot params are not fido2-hmac params".into(),
      });
    };
    params.validate().map_err(|reason| Error::ProviderCorrupt { kind: KIND, reason })?;
    let backend = self.backend.clone();
    let params = params.clone();
    let pin = self.pin();
    blocking(move || unlock_blocking(&*backend, &params, pin.as_deref().map(|s| s.as_str()))).await
  }

  /// Non-resident credentials live nowhere but in the slot; nothing to do.
  async fn destroy(&self, _slot_id: &SlotId, _params: &SlotParams) -> Result<()> {
    Ok(())
  }
}

#[cfg(test)]
mod tests;
