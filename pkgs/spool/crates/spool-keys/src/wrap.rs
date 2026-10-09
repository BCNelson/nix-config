//! Sealing the data key under a KEK with XChaCha20-Poly1305.
//!
//! `wrapped = nonce (24 random bytes) || ciphertext (32) || tag (16)`,
//! 72 bytes total, base64 (standard alphabet, padded) in `keyslots.json`.
//!
//! AAD = `"spool-keyslot-v1"` (16 ASCII bytes) `|| slot_id (16 raw UUID bytes)
//! || data_key_id (16 raw UUID bytes)`. Binding the slot id stops a wrapped
//! value from being swapped into another slot; binding the data key id stops a
//! slot from being replayed into a different keyslots file / after rotation.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::slots::{DataKeyId, SlotId};

/// AAD domain separator.
pub const AAD_PREFIX: &[u8; 16] = b"spool-keyslot-v1";
pub(crate) const NONCE_LEN: usize = 24;
pub(crate) const TAG_LEN: usize = 16;
/// Length of a decoded `wrapped` value.
pub const WRAPPED_LEN: usize = NONCE_LEN + 32 + TAG_LEN;

/// The AAD for a slot.
pub fn aad(slot: &SlotId, data_key_id: &DataKeyId) -> [u8; 48] {
  let mut out = [0u8; 48];
  out[..16].copy_from_slice(AAD_PREFIX);
  out[16..32].copy_from_slice(slot.0.as_bytes());
  out[32..].copy_from_slice(data_key_id.0.as_bytes());
  out
}

fn cipher(kek: &[u8; 32]) -> XChaCha20Poly1305 {
  // Length is statically 32, so this cannot fail.
  XChaCha20Poly1305::new_from_slice(kek).expect("32-byte key")
}

/// Seal `data_key` under `kek`. Returns `nonce || ciphertext || tag`.
pub(crate) fn seal(kek: &[u8; 32], data_key: &[u8; 32], aad: &[u8]) -> Vec<u8> {
  let mut nonce = [0u8; NONCE_LEN];
  rand::rng().fill_bytes(&mut nonce);
  let ct = cipher(kek)
    .encrypt(&XNonce::from(nonce), Payload { msg: data_key, aad })
    .expect("XChaCha20-Poly1305 encryption of 32 bytes cannot fail");
  let mut out = Vec::with_capacity(WRAPPED_LEN);
  out.extend_from_slice(&nonce);
  out.extend_from_slice(&ct);
  debug_assert_eq!(out.len(), WRAPPED_LEN);
  out
}

/// Open a wrapped data key. Any authentication failure is [`Error::WrongKey`].
pub(crate) fn open(kek: &[u8; 32], wrapped: &[u8], aad: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
  if wrapped.len() != WRAPPED_LEN {
    return Err(Error::WrongKey);
  }
  let (nonce, ct) = wrapped.split_at(NONCE_LEN);
  let nonce: [u8; NONCE_LEN] = nonce.try_into().expect("split at NONCE_LEN");
  let pt = Zeroizing::new(
    cipher(kek)
      .decrypt(&XNonce::from(nonce), Payload { msg: ct, aad })
      .map_err(|_| Error::WrongKey)?,
  );
  let mut key = Zeroizing::new([0u8; 32]);
  if pt.len() != 32 {
    return Err(Error::WrongKey);
  }
  key.copy_from_slice(&pt);
  Ok(key)
}

/// A fresh random 256-bit key.
pub(crate) fn random_key() -> Zeroizing<[u8; 32]> {
  let mut k = Zeroizing::new([0u8; 32]);
  rand::rng().fill_bytes(k.as_mut());
  k
}

#[cfg(test)]
mod tests {
  use super::*;

  fn ids() -> (SlotId, DataKeyId) {
    (SlotId::new(), DataKeyId::new())
  }

  #[test]
  fn round_trip() {
    let (s, d) = ids();
    let kek = random_key();
    let dk = random_key();
    let w = seal(&kek, &dk, &aad(&s, &d));
    assert_eq!(w.len(), WRAPPED_LEN);
    assert_eq!(*open(&kek, &w, &aad(&s, &d)).unwrap(), *dk);
  }

  #[test]
  fn nonce_is_random() {
    let (s, d) = ids();
    let kek = random_key();
    let dk = random_key();
    assert_ne!(seal(&kek, &dk, &aad(&s, &d)), seal(&kek, &dk, &aad(&s, &d)));
  }

  #[test]
  fn aad_layout() {
    let (s, d) = ids();
    let a = aad(&s, &d);
    assert_eq!(&a[..16], b"spool-keyslot-v1");
    assert_eq!(&a[16..32], s.0.as_bytes());
    assert_eq!(&a[32..], d.0.as_bytes());
  }

  #[test]
  fn aad_binds_slot_and_data_key_id() {
    let (s, d) = ids();
    let kek = random_key();
    let dk = random_key();
    let w = seal(&kek, &dk, &aad(&s, &d));
    assert!(matches!(open(&kek, &w, &aad(&SlotId::new(), &d)), Err(Error::WrongKey)));
    assert!(matches!(open(&kek, &w, &aad(&s, &DataKeyId::new())), Err(Error::WrongKey)));
  }

  #[test]
  fn wrong_kek_and_tamper() {
    let (s, d) = ids();
    let kek = random_key();
    let dk = random_key();
    let a = aad(&s, &d);
    let w = seal(&kek, &dk, &a);
    assert!(matches!(open(&random_key(), &w, &a), Err(Error::WrongKey)));
    for i in [0, 23, 24, 55, 56, 71] {
      let mut t = w.clone();
      t[i] ^= 1;
      assert!(matches!(open(&kek, &t, &a), Err(Error::WrongKey)), "byte {i}");
    }
    assert!(matches!(open(&kek, &w[..71], &a), Err(Error::WrongKey)));
  }
}
