//! libfido2 backend (through `fido2-rs`). Only compiled with the `fido2`
//! feature; never exercised by the automated tests (see
//! `tests/fido2_hw.rs` for the opt-in hardware test).
//!
//! libfido2's debug output stays off: `fido_init(FIDO_DEBUG)` is never
//! called, so no CTAP traffic (which carries hmac salts and outputs) is
//! printed.

use fido2_rs::assertion::AssertRequest;
use fido2_rs::credentials::{CoseType, Credential, Extensions, Opt};
use fido2_rs::device::{Device, DeviceList};
use fido2_rs::error::Error as FidoRsError;
use rand::RngCore;
use zeroize::Zeroizing;

use super::{Authenticator, DevError, Fido2Device, SALT_LEN};
use crate::provider::Kek;

/// Upper bound on enumerated devices.
const MAX_DEVICES: usize = 16;

// libfido2 `fido/err.h` codes (stable ABI).
const FIDO_ERR_UNSUPPORTED_EXTENSION: i32 = 0x16;
const FIDO_ERR_INVALID_CREDENTIAL: i32 = 0x22;
const FIDO_ERR_OPERATION_DENIED: i32 = 0x27;
const FIDO_ERR_UNSUPPORTED_OPTION: i32 = 0x2b;
const FIDO_ERR_INVALID_OPTION: i32 = 0x2c;
const FIDO_ERR_KEEPALIVE_CANCEL: i32 = 0x2d;
const FIDO_ERR_NO_CREDENTIALS: i32 = 0x2e;
const FIDO_ERR_USER_ACTION_TIMEOUT: i32 = 0x2f;
const FIDO_ERR_PIN_INVALID: i32 = 0x31;
const FIDO_ERR_PIN_BLOCKED: i32 = 0x32;
const FIDO_ERR_PIN_AUTH_INVALID: i32 = 0x33;
const FIDO_ERR_PIN_AUTH_BLOCKED: i32 = 0x34;
const FIDO_ERR_PIN_NOT_SET: i32 = 0x35;
const FIDO_ERR_PIN_REQUIRED: i32 = 0x36;
const FIDO_ERR_ACTION_TIMEOUT: i32 = 0x3a;
const FIDO_ERR_UV_BLOCKED: i32 = 0x3c;
const FIDO_ERR_UV_INVALID: i32 = 0x3f;
const FIDO_ERR_TX: i32 = -1;
const FIDO_ERR_RX: i32 = -2;

pub(super) struct LibFido2;

fn map(e: FidoRsError) -> DevError {
  match e {
    FidoRsError::Fido(f) => map_code(f.code),
    FidoRsError::NulError(_) => DevError::Other("string contains a NUL byte".into()),
    FidoRsError::Openssl(_) => DevError::Other("libfido2 crypto error".into()),
    FidoRsError::Unsupported => DevError::Unsupported("operation not supported by the key".into()),
  }
}

pub(super) fn map_code(code: i32) -> DevError {
  match code {
    FIDO_ERR_NO_CREDENTIALS | FIDO_ERR_INVALID_CREDENTIAL => DevError::NoCredentials,
    FIDO_ERR_PIN_REQUIRED => DevError::PinRequired,
    FIDO_ERR_PIN_INVALID | FIDO_ERR_PIN_AUTH_INVALID | FIDO_ERR_UV_INVALID => DevError::PinInvalid,
    FIDO_ERR_PIN_BLOCKED | FIDO_ERR_PIN_AUTH_BLOCKED | FIDO_ERR_UV_BLOCKED => DevError::PinBlocked,
    FIDO_ERR_USER_ACTION_TIMEOUT | FIDO_ERR_ACTION_TIMEOUT => DevError::Timeout,
    FIDO_ERR_OPERATION_DENIED | FIDO_ERR_KEEPALIVE_CANCEL => DevError::Denied,
    FIDO_ERR_PIN_NOT_SET => DevError::Unsupported(
      "user verification requested but the security key has no PIN set".into(),
    ),
    FIDO_ERR_UNSUPPORTED_EXTENSION | FIDO_ERR_UNSUPPORTED_OPTION | FIDO_ERR_INVALID_OPTION => {
      DevError::Unsupported(format!(
        "security key does not support a required feature (hmac-secret / option), libfido2 code {code}"
      ))
    }
    FIDO_ERR_TX | FIDO_ERR_RX => {
      DevError::Gone(format!("security key I/O failed (libfido2 code {code})"))
    }
    _ => DevError::Other(format!("libfido2 error code {code}")),
  }
}

fn random_hash() -> [u8; 32] {
  let mut h = [0u8; 32];
  rand::rng().fill_bytes(&mut h);
  h
}

fn open(path: &str) -> Result<Device, DevError> {
  Device::open(path).map_err(|e| match map(e) {
    DevError::Other(r) => DevError::Gone(format!("cannot open {path}: {r}")),
    other => other,
  })
}

impl Authenticator for LibFido2 {
  fn devices(&self) -> Result<Vec<Fido2Device>, DevError> {
    let list = DeviceList::list_devices(MAX_DEVICES).map_err(|e| match map(e) {
      DevError::Other(r) => DevError::Gone(format!("cannot enumerate security keys: {r}")),
      other => other,
    })?;
    Ok(
      list
        .map(|d| Fido2Device {
          path: d.path.to_string_lossy().into_owned(),
          manufacturer: d.manufacturer.to_string_lossy().into_owned(),
          product: d.product.to_string_lossy().into_owned(),
        })
        .collect(),
    )
  }

  fn make_credential(
    &self,
    dev: &str,
    rp_id: &str,
    user_id: &[u8],
    uv: bool,
    pin: Option<&str>,
  ) -> Result<Vec<u8>, DevError> {
    let d = open(dev)?;
    let mut cred = Credential::new().map_err(map)?;
    cred.set_client_data_hash(random_hash()).map_err(map)?;
    cred.set_rp(rp_id, "Spool").map_err(map)?;
    cred.set_user(user_id, "spool", Some("Spool clipboard history"), None).map_err(map)?;
    cred.set_cose_type(CoseType::ES256).map_err(map)?;
    cred.set_extension(Extensions::HMAC_SECRET).map_err(map)?;
    cred.set_rk(Opt::False).map_err(map)?;
    cred.set_uv(if uv { Opt::True } else { Opt::Omit }).map_err(map)?;
    d.make_credential(&mut cred, pin).map_err(map)?;
    let id = cred.id().to_vec();
    if id.is_empty() {
      return Err(DevError::Other("security key returned an empty credential id".into()));
    }
    Ok(id)
  }

  fn has_credential(&self, dev: &str, rp_id: &str, credential_id: &[u8]) -> Result<bool, DevError> {
    let d = open(dev)?;
    let mut req = AssertRequest::new().map_err(map)?;
    req.set_rp(rp_id).map_err(map)?;
    req.set_client_data_hash(random_hash()).map_err(map)?;
    req.set_allow_credential(credential_id).map_err(map)?;
    req.set_up(Opt::False).map_err(map)?;
    match d.get_assertion(req, None) {
      Ok(_) => Ok(true),
      Err(e) => match map(e) {
        DevError::NoCredentials => Ok(false),
        other => Err(other),
      },
    }
  }

  fn hmac_secret(
    &self,
    dev: &str,
    rp_id: &str,
    credential_id: &[u8],
    salt: &[u8; SALT_LEN],
    uv: bool,
    pin: Option<&str>,
  ) -> Result<Kek, DevError> {
    let d = open(dev)?;
    let mut req = AssertRequest::new().map_err(map)?;
    req.set_rp(rp_id).map_err(map)?;
    req.set_client_data_hash(random_hash()).map_err(map)?;
    req.set_allow_credential(credential_id).map_err(map)?;
    req.set_extensions(Extensions::HMAC_SECRET).map_err(map)?;
    req.set_hmac_salt(salt).map_err(map)?;
    req.set_up(Opt::True).map_err(map)?;
    req.set_uv(if uv { Opt::True } else { Opt::Omit }).map_err(map)?;
    let asserts = d.get_assertion(req, pin).map_err(map)?;
    let a = asserts.iter().next().ok_or_else(|| DevError::Other("no assertion returned".into()))?;
    let out = a.hmac_secret();
    if out.len() != 32 {
      return Err(DevError::Unsupported(
        "security key returned no 32-byte hmac-secret (extension unsupported?)".into(),
      ));
    }
    let mut kek = Zeroizing::new([0u8; 32]);
    kek.copy_from_slice(out);
    Ok(kek)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn code_mapping() {
    assert_eq!(map_code(FIDO_ERR_NO_CREDENTIALS), DevError::NoCredentials);
    assert_eq!(map_code(FIDO_ERR_PIN_REQUIRED), DevError::PinRequired);
    assert_eq!(map_code(FIDO_ERR_PIN_INVALID), DevError::PinInvalid);
    assert_eq!(map_code(FIDO_ERR_PIN_AUTH_BLOCKED), DevError::PinBlocked);
    assert_eq!(map_code(FIDO_ERR_ACTION_TIMEOUT), DevError::Timeout);
    assert_eq!(map_code(FIDO_ERR_USER_ACTION_TIMEOUT), DevError::Timeout);
    assert_eq!(map_code(FIDO_ERR_KEEPALIVE_CANCEL), DevError::Denied);
    assert!(matches!(map_code(FIDO_ERR_UNSUPPORTED_EXTENSION), DevError::Unsupported(_)));
    assert!(matches!(map_code(FIDO_ERR_RX), DevError::Gone(_)));
    assert!(matches!(map_code(0x7f), DevError::Other(_)));
  }
}
