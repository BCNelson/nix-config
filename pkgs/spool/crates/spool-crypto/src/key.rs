//! [`DataKey`], HKDF [`Label`]s and derived [`SubKey`]s.

use std::fmt;

use hkdf::Hkdf;
use rand::TryRngCore;
use rand::rngs::OsRng;
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::mlock::Secret32;

/// Length of every key in this crate (256 bits).
pub const KEY_LEN: usize = 32;

/// Fill `out` from the OS CSPRNG (`getrandom(2)`).
///
/// # Panics
/// If the OS RNG fails, which on Linux only happens before the kernel pool
/// is initialised or under seccomp misconfiguration; continuing without
/// randomness would be worse than aborting.
pub(crate) fn os_random(out: &mut [u8]) {
  OsRng.try_fill_bytes(out).expect("OS random number generator failed");
}

/// HKDF-SHA256(ikm, salt) expanded with `info` straight into a locked secret.
pub(crate) fn hkdf32(ikm: &[u8; 32], salt: Option<&[u8]>, info: &[u8]) -> Secret32 {
  let mut out = Secret32::zeroed();
  Hkdf::<Sha256>::new(salt, ikm)
    .expand(info, out.bytes_mut())
    .expect("32 bytes is a valid HKDF-SHA256 output length");
  out
}

/// Fixed HKDF `info` labels. Changing a label string changes every derived
/// key, so the strings are part of the on-disk format (pinned by tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Label {
  /// `spool/db/v1`: SQLCipher raw key.
  Db,
  /// `spool/index/v1`: Tantivy encrypted directory (M3).
  Index,
  /// `spool/blob/v1`: large blob files (chunked AEAD).
  Blob,
  /// `spool/hash/v1`: keyed BLAKE3 dedupe key.
  Hash,
}

impl Label {
  pub const ALL: [Label; 4] = [Label::Db, Label::Index, Label::Blob, Label::Hash];

  /// The HKDF `info` string.
  pub const fn as_str(self) -> &'static str {
    match self {
      Label::Db => "spool/db/v1",
      Label::Index => "spool/index/v1",
      Label::Blob => "spool/blob/v1",
      Label::Hash => "spool/hash/v1",
    }
  }
}

/// The single random 256-bit key that encrypts Spool's history.
///
/// Zeroized on drop, best-effort mlocked, no `Clone` (use
/// [`DataKey::try_clone`] when a second copy is really needed), `Debug` is
/// redacted. [`DataKey::expose`] is the only way to read the bytes.
pub struct DataKey(Secret32);

impl DataKey {
  /// A fresh key from the OS CSPRNG.
  ///
  /// # Panics
  /// If the OS RNG fails (see module docs); never in practice on Linux.
  pub fn generate() -> Self {
    let mut s = Secret32::zeroed();
    os_random(s.bytes_mut());
    DataKey(s)
  }

  /// Wrap existing key bytes (e.g. unwrapped from a key slot). The argument
  /// is zeroized when it drops.
  pub fn from_bytes(bytes: Zeroizing<[u8; KEY_LEN]>) -> Self {
    let mut s = Secret32::zeroed();
    s.bytes_mut().copy_from_slice(&bytes[..]);
    DataKey(s)
  }

  /// The raw key bytes. This is the **only** way out of a `DataKey`; do not
  /// copy the result into anything that is not itself zeroized.
  pub fn expose(&self) -> &[u8; KEY_LEN] {
    self.0.bytes()
  }

  /// An explicit second copy (separately locked and zeroized). Infallible;
  /// named `try_clone` so copies of the master key stand out in review.
  pub fn try_clone(&self) -> Self {
    let mut s = Secret32::zeroed();
    s.bytes_mut().copy_from_slice(self.0.bytes());
    DataKey(s)
  }

  /// HKDF-SHA256 (no salt, `info` = [`Label::as_str`]) sub-key.
  pub fn derive(&self, label: Label) -> SubKey {
    SubKey { label, key: hkdf32(self.0.bytes(), None, label.as_str().as_bytes()) }
  }
}

impl Zeroize for DataKey {
  fn zeroize(&mut self) {
    self.0.zeroize();
  }
}

/// The inner `Secret32` zeroizes itself on drop.
impl ZeroizeOnDrop for DataKey {}

impl fmt::Debug for DataKey {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str("DataKey(<redacted>)")
  }
}

/// A 256-bit key derived from a [`DataKey`] for one purpose ([`Label`]).
/// Same hygiene as `DataKey`.
pub struct SubKey {
  label: Label,
  key: Secret32,
}

impl SubKey {
  /// Which label this key was derived for.
  pub fn label(&self) -> Label {
    self.label
  }

  /// The raw sub-key bytes (e.g. for `Store::open` or keyed BLAKE3). Do not
  /// copy them into anything that is not itself zeroized.
  pub fn expose(&self) -> &[u8; KEY_LEN] {
    self.key.bytes()
  }

  /// An explicit second copy; see [`DataKey::try_clone`].
  pub fn try_clone(&self) -> Self {
    let mut s = Secret32::zeroed();
    s.bytes_mut().copy_from_slice(self.key.bytes());
    SubKey { label: self.label, key: s }
  }

  /// Lowercase hex of the key: the body of SQLCipher's raw-key literal, used
  /// as `PRAGMA key = "x'<this>'"`. 64 characters, zeroized on drop.
  pub fn sqlcipher_pragma_hex(&self) -> Zeroizing<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = Zeroizing::new(String::with_capacity(2 * KEY_LEN));
    for b in self.key.bytes() {
      s.push(HEX[(b >> 4) as usize] as char);
      s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
  }
}

impl Zeroize for SubKey {
  fn zeroize(&mut self) {
    self.key.zeroize();
  }
}

impl ZeroizeOnDrop for SubKey {}

impl fmt::Debug for SubKey {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "SubKey({}, <redacted>)", self.label.as_str())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
  }

  fn zero_key() -> DataKey {
    DataKey::from_bytes(Zeroizing::new([0u8; 32]))
  }

  /// Pinned vectors: HKDF-SHA256(ikm = 32 zero bytes, salt = none, info =
  /// label, L = 32). Cross-checked against Python's `hmac`/`hashlib`. If this
  /// test fails, the on-disk key schedule changed: every existing database
  /// and blob would become unreadable.
  #[test]
  fn label_vectors_are_stable() {
    let k = zero_key();
    let expected = [
      (Label::Db, "322e3b1a92ee31a68a0bd0d42f0ed0269393493991a9246cc54c6a2477bcbf9c"),
      (Label::Index, "97fc4a8bf746d473b27f9e646b9b30e05ba4e56a525d98a64eef366ff28e2b8e"),
      (Label::Blob, "47ec4d45cd7d2980a7233a39d138f7b895498ffdc3dc84a450b7728a7d75d7a1"),
      (Label::Hash, "0343ec1cd27eff1771a6704d55f258b1a04303d2bfe7feaa66be8ab5afca0a9e"),
    ];
    for (label, want) in expected {
      assert_eq!(hex(k.derive(label).expose()), want, "{}", label.as_str());
    }
  }

  #[test]
  fn labels_strings_are_fixed() {
    let s: Vec<&str> = Label::ALL.iter().map(|l| l.as_str()).collect();
    assert_eq!(s, ["spool/db/v1", "spool/index/v1", "spool/blob/v1", "spool/hash/v1"]);
  }

  #[test]
  fn labels_produce_distinct_keys() {
    let k = DataKey::generate();
    let keys: Vec<[u8; 32]> = Label::ALL.iter().map(|l| *k.derive(*l).expose()).collect();
    for i in 0..keys.len() {
      assert_ne!(&keys[i], k.expose());
      for j in i + 1..keys.len() {
        assert_ne!(keys[i], keys[j]);
      }
    }
  }

  #[test]
  fn derive_is_deterministic_and_key_dependent() {
    let a = DataKey::generate();
    let b = a.try_clone();
    assert_eq!(a.expose(), b.expose());
    assert_eq!(a.derive(Label::Db).expose(), b.derive(Label::Db).expose());
    let c = DataKey::generate();
    assert_ne!(a.expose(), c.expose());
    assert_ne!(a.derive(Label::Db).expose(), c.derive(Label::Db).expose());
  }

  #[test]
  fn generate_is_random() {
    let a = DataKey::generate();
    let b = DataKey::generate();
    assert_ne!(a.expose(), b.expose());
    assert_ne!(a.expose(), &[0u8; 32]);
  }

  #[test]
  fn sqlcipher_hex_matches_bytes() {
    let k = zero_key().derive(Label::Db);
    let h = k.sqlcipher_pragma_hex();
    assert_eq!(h.len(), 64);
    assert_eq!(*h, hex(k.expose()));
    assert!(h.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
  }

  #[test]
  fn debug_is_redacted() {
    let k = DataKey::from_bytes(Zeroizing::new([0xab; 32]));
    let d = format!("{k:?} {:?}", k.derive(Label::Blob));
    assert!(!d.contains("ab"), "{d}");
    assert!(!d.contains("171"), "{d}");
    assert!(d.contains("redacted"));
  }

  #[test]
  fn zeroize_clears() {
    let mut k = DataKey::from_bytes(Zeroizing::new([0x5a; 32]));
    k.zeroize();
    assert_eq!(k.expose(), &[0u8; 32]);
  }
}
