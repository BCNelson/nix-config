//! Small-blob AEAD (key slots and similar): XChaCha20-Poly1305 with a random
//! 24-byte nonce.
//!
//! Layout: `nonce (24) || ciphertext || tag (16)`. Random 192-bit nonces are
//! safe to use with one key for any realistic number of messages.

use chacha20poly1305::aead::{AeadInOut, KeyInit};
use chacha20poly1305::{Key, Tag, XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::key::os_random;

/// XChaCha20 nonce length.
pub const NONCE_LEN: usize = 24;
/// Poly1305 tag length.
pub const TAG_LEN: usize = 16;
/// Bytes `seal` adds to the plaintext.
pub const OVERHEAD: usize = NONCE_LEN + TAG_LEN;

/// Encrypt `plaintext` under `key`, binding `aad`. Output is
/// `nonce || ciphertext || tag` (`plaintext.len() + OVERHEAD` bytes).
///
/// # Panics
/// If the OS RNG fails (never in practice on Linux).
pub fn seal(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
  let mut out = Vec::with_capacity(plaintext.len() + OVERHEAD);
  let mut nonce = [0u8; NONCE_LEN];
  os_random(&mut nonce);
  out.extend_from_slice(&nonce);
  out.extend_from_slice(plaintext);
  let cipher = XChaCha20Poly1305::new(&Key::from(*key));
  let tag = cipher
    .encrypt_inout_detached(&XNonce::from(nonce), aad, out[NONCE_LEN..].as_mut().into())
    .expect("XChaCha20-Poly1305 accepts any in-memory length");
  out.extend_from_slice(&tag);
  out
}

/// Decrypt and authenticate a blob produced by [`seal`] with the same `key`
/// and `aad`.
pub fn open(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
  if sealed.len() < OVERHEAD {
    return Err(Error::Truncated);
  }
  let (nonce, rest) = sealed.split_at(NONCE_LEN);
  let (ct, tag) = rest.split_at(rest.len() - TAG_LEN);
  let mut buf = Zeroizing::new(ct.to_vec());
  let nonce = XNonce::try_from(nonce).expect("24 bytes");
  let tag = Tag::try_from(tag).expect("16 bytes");
  XChaCha20Poly1305::new(&Key::from(*key))
    .decrypt_inout_detached(&nonce, aad, buf.as_mut_slice().into(), &tag)
    .map_err(|_| Error::Auth)?;
  Ok(buf)
}

#[cfg(test)]
mod tests {
  use super::*;

  const K: [u8; 32] = [7; 32];

  #[test]
  fn round_trip() {
    for len in [0usize, 1, 31, 32, 33, 1000] {
      let pt: Vec<u8> = (0..len).map(|i| i as u8).collect();
      let s = seal(&K, b"aad", &pt);
      assert_eq!(s.len(), len + OVERHEAD);
      assert_eq!(&open(&K, b"aad", &s).unwrap()[..], &pt[..]);
    }
  }

  #[test]
  fn nonces_are_random() {
    let a = seal(&K, b"", b"same");
    let b = seal(&K, b"", b"same");
    assert_ne!(a, b);
    assert_ne!(a[..NONCE_LEN], b[..NONCE_LEN]);
  }

  #[test]
  fn wrong_key_or_aad_fails() {
    let s = seal(&K, b"slot-1", b"secret");
    assert!(matches!(open(&[8; 32], b"slot-1", &s), Err(Error::Auth)));
    assert!(matches!(open(&K, b"slot-2", &s), Err(Error::Auth)));
    assert!(matches!(open(&K, b"", &s), Err(Error::Auth)));
  }

  #[test]
  fn every_bit_flip_fails() {
    let s = seal(&K, b"a", b"0123456789");
    for i in 0..s.len() {
      for bit in 0..8 {
        let mut t = s.clone();
        t[i] ^= 1 << bit;
        assert!(open(&K, b"a", &t).is_err(), "byte {i} bit {bit}");
      }
    }
  }

  #[test]
  fn truncation_fails() {
    let s = seal(&K, b"", b"hello");
    for n in 0..s.len() {
      assert!(open(&K, b"", &s[..n]).is_err(), "len {n}");
    }
    assert!(matches!(open(&K, b"", &s[..OVERHEAD - 1]), Err(Error::Truncated)));
    let mut longer = s.clone();
    longer.push(0);
    assert!(open(&K, b"", &longer).is_err());
  }
}
