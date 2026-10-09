//! Clipboard item model.

use std::fmt;
use std::time::SystemTime;

use bitflags::bitflags;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Primary key of `items` (SQLite rowid; always > 0). The inner value is the
/// same number that travels on the wire as `spool_proto::WireItemId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ItemId(pub i64);

impl fmt::Display for ItemId {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "#{}", self.0)
  }
}

/// Which Wayland selection an item came from / is published to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Selection {
  Clipboard,
  Primary,
}

impl Selection {
  /// Value stored in `items.selection`.
  pub const fn as_str(self) -> &'static str {
    match self {
      Selection::Clipboard => "clipboard",
      Selection::Primary => "primary",
    }
  }

  /// Inverse of [`Selection::as_str`].
  pub fn parse(s: &str) -> Option<Self> {
    match s {
      "clipboard" => Some(Selection::Clipboard),
      "primary" => Some(Selection::Primary),
      _ => None,
    }
  }
}

bitflags! {
  /// `items.flags`. Unknown bits are preserved (`from_bits_retain`).
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
  pub struct ItemFlags: u32 {
    /// Exempt from retention; shown first in the picker.
    const PINNED = 1 << 0;
    /// Stored but must not be re-published by keep-alive nor shown
    /// unmasked (reserved; M1 never stores sensitive content at all).
    const SENSITIVE = 1 << 1;
  }
}

/// One MIME representation of an item.
///
/// Alias representations (`alias_of = Some(canonical)`) carry **empty**
/// `data`; their bytes are those of the canonical representation with mime
/// `canonical` in the same item. Use [`Item::resolve`] to get the bytes for
/// any offered mime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Representation {
  pub mime: String,
  pub alias_of: Option<String>,
  pub data: Bytes,
}

impl Representation {
  pub fn new(mime: impl Into<String>, data: impl Into<Bytes>) -> Self {
    Self { mime: mime.into(), alias_of: None, data: data.into() }
  }

  pub fn alias(mime: impl Into<String>, canonical: impl Into<String>) -> Self {
    Self { mime: mime.into(), alias_of: Some(canonical.into()), data: Bytes::new() }
  }

  pub fn is_alias(&self) -> bool {
    self.alias_of.is_some()
  }
}

/// A 32-byte keyed BLAKE3 hash used for dedupe.
pub type ItemHash = [u8; 32];

/// Keyed BLAKE3 over a representation: `mime || 0x00 || data`. Callers pass
/// the item's canonical (first non-alias) representation.
pub fn dedupe_hash(key: &[u8; 32], canonical: &Representation) -> ItemHash {
  let mut h = blake3::Hasher::new_keyed(key);
  h.update(canonical.mime.as_bytes());
  h.update(&[0]);
  h.update(&canonical.data);
  *h.finalize().as_bytes()
}

/// First 8 hex chars of a hash; the only content-derived value that may be
/// logged.
pub fn hash_prefix(hash: &ItemHash) -> String {
  hash[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// An item ready to be inserted (output of `Policy::evaluate`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewItem {
  pub selection: Selection,
  pub source_app: Option<String>,
  pub created_at: SystemTime,
  pub flags: ItemFlags,
  pub hash: ItemHash,
  /// Short single-line sanitized excerpt for lists (<= 200 chars). Text
  /// items get a text excerpt; items without usable text get a bracketed
  /// summary of their first canonical representation, e.g.
  /// `[image/png 1234 bytes]`. Always `Some` for items built by the policy.
  pub preview: Option<String>,
  /// Canonical representations first, aliases after. Never empty.
  pub reps: Vec<Representation>,
}

impl NewItem {
  /// Sum of non-alias representation sizes (`items.total_size`).
  pub fn total_size(&self) -> u64 {
    self.reps.iter().filter(|r| !r.is_alias()).map(|r| r.data.len() as u64).sum()
  }
}

/// A stored item with all representations loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
  pub id: ItemId,
  pub change_seq: i64,
  pub created_at: SystemTime,
  pub last_used_at: SystemTime,
  pub selection: Selection,
  pub source_app: Option<String>,
  pub flags: ItemFlags,
  pub hash: ItemHash,
  pub preview: Option<String>,
  pub total_size: u64,
  pub reps: Vec<Representation>,
}

/// MIME types treated as text, in order of preference.
pub const TEXT_MIMES: &[&str] =
  &["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "TEXT", "STRING"];

impl Item {
  /// Bytes for `mime`, following an alias to its canonical representation.
  pub fn resolve(&self, mime: &str) -> Option<&Bytes> {
    let rep = self.reps.iter().find(|r| r.mime == mime)?;
    match &rep.alias_of {
      None => Some(&rep.data),
      Some(canon) => self.reps.iter().find(|r| &r.mime == canon && !r.is_alias()).map(|r| &r.data),
    }
  }

  /// All offered mimes (canonical + aliases), in stored order.
  pub fn mimes(&self) -> impl Iterator<Item = &str> {
    self.reps.iter().map(|r| r.mime.as_str())
  }

  /// Preferred representation for `spoolctl current`: first text mime from
  /// [`TEXT_MIMES`], else the first canonical representation.
  pub fn preferred(&self) -> Option<(&str, &Bytes)> {
    for m in TEXT_MIMES {
      if let Some(d) = self.resolve(m) {
        return Some((m, d));
      }
    }
    self.reps.iter().find(|r| !r.is_alias()).map(|r| (r.mime.as_str(), &r.data))
  }
}

/// Lightweight row for lists (no representation bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemSummary {
  pub id: ItemId,
  pub created_at: SystemTime,
  pub last_used_at: SystemTime,
  pub selection: Selection,
  pub source_app: Option<String>,
  pub flags: ItemFlags,
  pub preview: Option<String>,
  pub total_size: u64,
  pub mimes: Vec<String>,
}

#[cfg(test)]
mod tests {
  use super::*;

  fn item(reps: Vec<Representation>) -> Item {
    Item {
      id: ItemId(1),
      change_seq: 1,
      created_at: SystemTime::UNIX_EPOCH,
      last_used_at: SystemTime::UNIX_EPOCH,
      selection: Selection::Clipboard,
      source_app: None,
      flags: ItemFlags::empty(),
      hash: [0; 32],
      preview: None,
      total_size: 0,
      reps,
    }
  }

  #[test]
  fn selection_roundtrip() {
    for s in [Selection::Clipboard, Selection::Primary] {
      assert_eq!(Selection::parse(s.as_str()), Some(s));
    }
    assert_eq!(Selection::parse("bogus"), None);
  }

  #[test]
  fn resolve_alias_and_preferred() {
    let it = item(vec![
      Representation::new("image/png", &b"png"[..]),
      Representation::new("text/plain;charset=utf-8", &b"hello"[..]),
      Representation::alias("UTF8_STRING", "text/plain;charset=utf-8"),
    ]);
    assert_eq!(it.resolve("UTF8_STRING").unwrap().as_ref(), b"hello");
    assert_eq!(it.resolve("nope"), None);
    let (m, d) = it.preferred().unwrap();
    assert_eq!((m, d.as_ref()), ("text/plain;charset=utf-8", &b"hello"[..]));
  }

  #[test]
  fn hash_is_keyed_and_mime_sensitive() {
    let r = Representation::new("text/plain", &b"x"[..]);
    let a = dedupe_hash(&[1; 32], &r);
    assert_ne!(a, dedupe_hash(&[2; 32], &r));
    assert_ne!(a, dedupe_hash(&[1; 32], &Representation::new("text/html", &b"x"[..])));
    assert_eq!(hash_prefix(&a).len(), 8);
  }
}
