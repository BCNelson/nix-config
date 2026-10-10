//! Picker channel messages (protocol v3, see [`PICKER_PROTO_VERSION`]).
//!
//! The daemon spawns the picker and talks to it over a private socketpair, so
//! these never travel over the public socket. The picker channel has its own
//! version number, independent of [`crate::PROTO_VERSION`]: the first frame
//! in each direction is a [`Hello`] carrying [`PICKER_PROTO_VERSION`]
//! ([`Hello::picker`], checked with [`Hello::is_picker_compatible`]); the
//! picker speaks first and the daemon closes the channel after its own
//! `Hello` on a mismatch.
//!
//! # Request ids
//!
//! [`PickerReq::Query`], [`PickerReq::Thumb`] and the edits
//! ([`PickerReq::Pin`], [`PickerReq::Delete`], [`PickerReq::Tag`]) carry a
//! picker-chosen `seq` (one counter for all, strictly increasing, wraps never
//! in practice) that the daemon echoes in the answer ([`PickerEvt::Page`],
//! [`PickerEvt::Thumb`]) or in a [`PickerEvt::Error`]. Answers may arrive in
//! any order; the picker drops pages whose `seq` is not its latest query
//! (typing outran the search). Every `Query`/`Thumb` gets exactly one answer:
//! the page/thumb, or an `Error` with that `seq`. An edit is answered only
//! when it fails: an `Error` with its `seq`.
//!
//! # Unlock flow
//!
//! While the encrypted history is locked the daemon still serves pages (the
//! in-memory session items) and tells the picker what it can prompt for with
//! [`PickerEvt::Locked`] (sent after the handshake, and again whenever the
//! set of prompts changes, e.g. a FIDO2 key that turns out to need a PIN).
//! The picker answers with [`PickerReq::Unlock`]; the daemon replies
//! [`PickerEvt::Unlocked`] or [`PickerEvt::UnlockFailed`].
//!
//! # Secrets
//!
//! Passphrases and PINs travel as [`UnlockSecret`] (zeroized on drop,
//! redacted `Debug`). The encoded frame buffers themselves are ordinary
//! `Vec<u8>`s; the receiving side should wipe them after decoding if it can.

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{Hello, WireItemId};

/// Picker channel protocol version carried in the picker [`Hello`]. Bump on
/// any incompatible change to the types in this module. v1 was the id-less
/// M4 draft (never shipped to spoold); v3 added [`PickerReq::Edit`] and
/// `seq` to `Pin`, `Delete` and `Tag` so their errors can be matched.
pub const PICKER_PROTO_VERSION: u16 = 3;

impl Hello {
  /// `Hello` for the picker channel ([`PICKER_PROTO_VERSION`]).
  pub const fn picker() -> Self {
    Self { proto: PICKER_PROTO_VERSION }
  }

  /// Whether the peer speaks this build's picker protocol.
  pub const fn is_picker_compatible(&self) -> bool {
    self.proto == PICKER_PROTO_VERSION
  }
}

/// Picker -> daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PickerReq {
  /// The surface exists and the first frame (initial page) is pre-rendered:
  /// a `Show` from now on maps instantly. Sent once.
  Ready,
  /// Search/list history. Empty `q` = most recent first. Answered by
  /// [`PickerEvt::Page`] or [`PickerEvt::Error`] with the same `seq`.
  Query {
    seq: u32,
    q: String,
    filters: QueryFilters,
    offset: u32,
    limit: u32,
  },
  /// Request a thumbnail of item `id`'s representation `mime`. Answered by
  /// [`PickerEvt::Thumb`] or [`PickerEvt::Error`] with the same `seq`.
  Thumb {
    seq: u32,
    id: WireItemId,
    mime: String,
  },
  /// User chose item `id`. The picker has already hidden itself (a
  /// [`PickerReq::Hidden`]`{reason: Selected}` precedes this).
  Select {
    id: WireItemId,
    mode: SelectMode,
  },
  /// Pin / unpin. Answered only on failure (an [`PickerEvt::Error`] with
  /// this `seq`), like `Delete` and `Tag`.
  Pin {
    id: WireItemId,
    on: bool,
    seq: u32,
  },
  Delete {
    id: WireItemId,
    seq: u32,
  },
  /// Add (`on`) or remove a tag. `tag` must pass [`check_tag`]; the daemon
  /// stores its normalized (trimmed) form.
  Tag {
    id: WireItemId,
    tag: String,
    on: bool,
    seq: u32,
  },
  /// Unlock the encrypted history. `secret` is the passphrase
  /// ([`UnlockProvider::Passphrase`]), the FIDO2 PIN ([`UnlockProvider::Fido2`]
  /// after a [`UnlockPrompt::Fido2Pin`]), or `None` (KWallet; FIDO2 touch
  /// without PIN: "start waiting for a touch now", sent when the unlock panel
  /// becomes visible and on Retry; a request while one is already waiting
  /// should be ignored).
  Unlock {
    provider: UnlockProvider,
    secret: Option<UnlockSecret>,
  },
  /// The picker unmapped itself (or obeyed [`PickerEvt::Hide`]). Sent for
  /// every transition from shown to hidden, so the daemon always knows the
  /// picker's visibility.
  Hidden {
    reason: HideReason,
  },
  /// Edit item `id`'s representation `mime` in the configured external
  /// editor (Ctrl+E: the preferred representation; Ctrl+Shift+E: one the
  /// user chose). The picker has already hidden itself
  /// (`Hidden{reason: Selected}` precedes this) and never receives the
  /// content: the daemon writes it to a private temp file, starts the
  /// editor and stores accepted saves as a new item. Failures come back as
  /// [`PickerEvt::Error`] with `seq: None` (and as a desktop notification,
  /// since the picker is hidden). Appended in v3.
  Edit {
    id: WireItemId,
    mime: String,
  },
}

/// Daemon -> picker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PickerEvt {
  /// Make the picker visible. Fields come from the compositor backend when
  /// known.
  Show {
    cursor: Option<CursorPos>,
    /// Output (monitor) name, e.g. `"DP-1"`.
    output: Option<String>,
    /// Opaque id of the window that should receive a paste.
    target_window: Option<String>,
    /// The output's (fractional) scale if the daemon knows it (KWin does):
    /// the first frame on a never-seen output is then rendered at the right
    /// scale instead of being corrected after mapping.
    scale_hint: Option<f64>,
  },
  Hide,
  /// Answer to `Query{seq, offset, ..}`. `more`: further items exist past
  /// `offset + items.len()`.
  Page {
    seq: u32,
    offset: u32,
    items: Vec<ItemPreview>,
    more: bool,
  },
  /// Answer to `Thumb{seq, id, ..}` (PNG/JPEG/WebP bytes; empty if the item
  /// or representation is gone).
  Thumb {
    seq: u32,
    id: WireItemId,
    bytes: Vec<u8>,
  },
  /// A request failed (`seq` set) or a channel-level problem (`seq: None`).
  /// `message` is short, human-readable and never contains the query or any
  /// item content.
  Error {
    seq: Option<u32>,
    code: PickerErrorCode,
    message: String,
  },
  /// History is locked; the picker may offer these prompts (in preference
  /// order). Pages keep coming (session items only). An empty list means
  /// "locked, nothing the user can do here" (e.g. waiting for KWallet).
  Locked {
    providers: Vec<UnlockPrompt>,
  },
  /// History is unlocked (also the answer to a successful `Unlock`). The
  /// picker re-queries.
  Unlocked,
  /// An `Unlock` attempt failed. The picker keeps the panel and shows why.
  UnlockFailed {
    provider: UnlockProvider,
    reason: UnlockFailReason,
  },
  IndexProgress {
    done: u64,
    total: u64,
  },
  /// A new item was stored while the picker is open.
  NewItem {
    preview: ItemPreview,
  },
}

/// Why the picker hid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HideReason {
  /// The user pressed Esc.
  Esc,
  /// Keyboard focus moved elsewhere (click outside, Alt+Tab, ...).
  FocusLost,
  /// The user picked an item (a `Select` follows).
  Selected,
  /// The daemon sent [`PickerEvt::Hide`].
  Requested,
  /// The compositor closed the surface (its output went away).
  Closed,
}

/// [`PickerEvt::Error`] classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PickerErrorCode {
  /// The search syntax was rejected (unsupported operator, too long, too
  /// many terms).
  BadQuery,
  /// The item no longer exists.
  NotFound,
  /// The request needs unlocked history.
  Locked,
  /// Temporarily unavailable (index rebuilding, store busy).
  Unavailable,
  Internal,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryFilters {
  pub pinned_only: bool,
  pub kinds: Vec<PreviewKind>,
  pub tags: Vec<String>,
  pub source_app: Option<String>,
  /// Include primary-selection items.
  pub include_primary: bool,
}

/// How a picked item is delivered. Picker keys: Enter = `Paste`,
/// Shift+Enter = `Copy`, Ctrl+Enter = `PastePlain`; a click = `Paste`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectMode {
  /// Put on the clipboard only.
  Copy,
  /// Put on the clipboard and auto-paste into `target_window`.
  Paste,
  /// Like `Paste` but text/plain only.
  PastePlain,
}

/// Who performs an unlock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnlockProvider {
  /// The Secret Service / KWallet slot (no secret: the wallet prompts).
  KWallet,
  /// A passphrase slot (`secret` = the passphrase).
  Passphrase,
  /// A FIDO2 hmac-secret slot (`secret` = the PIN when one was asked for).
  Fido2,
}

/// What the unlock panel can ask the user for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnlockPrompt {
  /// A passphrase field.
  Passphrase,
  /// "Touch your security key" (with Retry).
  Fido2Touch,
  /// A security-key PIN field (the key needs user verification).
  Fido2Pin,
}

/// Why an `Unlock` failed (mapped from `spool_keys::Error` by the daemon).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnlockFailReason {
  /// Wrong passphrase (`WrongKey` on a passphrase slot): re-prompt.
  WrongSecret,
  /// Wrong FIDO2 PIN (`PinInvalid`): re-prompt the PIN.
  PinInvalid,
  /// PIN / user verification blocked on the key (`PinBlocked`): the user
  /// has to re-plug or reset it; no retry from the picker.
  PinBlocked,
  /// Touch timed out / denied, or the wallet prompt was dismissed
  /// (`Dismissed`): offer Retry.
  Dismissed,
  /// No (matching) security key connected, wallet unreachable
  /// (`Unavailable`, `Locked`): offer Retry.
  Unavailable,
  /// Anything else (corrupt slot, provider error, ...); details are in the
  /// daemon log.
  Other,
}

/// A passphrase or PIN on the wire: zeroized on drop, `Debug` prints
/// `UnlockSecret(<redacted>)`. Serialized as a plain string.
#[derive(Clone, PartialEq, Eq)]
pub struct UnlockSecret(Zeroizing<String>);

impl UnlockSecret {
  pub fn new(s: Zeroizing<String>) -> Self {
    Self(s)
  }

  pub fn expose(&self) -> &str {
    &self.0
  }

  /// Hand the secret to a key provider (e.g.
  /// `PassphraseProvider::new(secret.into_inner())`).
  pub fn into_inner(self) -> Zeroizing<String> {
    self.0
  }
}

impl std::fmt::Debug for UnlockSecret {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str("UnlockSecret(<redacted>)")
  }
}

impl Serialize for UnlockSecret {
  fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&self.0)
  }
}

impl<'de> Deserialize<'de> for UnlockSecret {
  fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
    String::deserialize(d).map(|s| Self(Zeroizing::new(s)))
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorPos {
  pub x: i32,
  pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewKind {
  Text,
  Html,
  Image,
  Files,
  Other,
}

/// Longest tag (in characters) a [`PickerReq::Tag`] may carry.
pub const MAX_TAG_CHARS: usize = 64;

/// Why a tag is not acceptable for [`PickerReq::Tag`] (see [`check_tag`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagError {
  /// Empty or only whitespace.
  Empty,
  /// More than [`MAX_TAG_CHARS`] characters.
  TooLong,
  /// Contains a control character or `/` (tags are search facets).
  BadChar,
}

impl TagError {
  /// Short user-facing text (never echoes the tag).
  pub const fn message(self) -> &'static str {
    match self {
      TagError::Empty => "A tag cannot be empty",
      TagError::TooLong => "A tag can be at most 64 characters",
      TagError::BadChar => "A tag cannot contain '/' or control characters",
    }
  }
}

/// The tag rule shared by daemon and picker: surrounding whitespace is
/// trimmed, then the tag must be 1..=[`MAX_TAG_CHARS`] characters with no
/// control characters and no `/`. Returns the normalized (trimmed) tag, which
/// is what gets stored; case is kept. The picker checks it before sending,
/// so the user sees why at once; the daemon checks it again and stores the
/// result.
pub fn check_tag(tag: &str) -> Result<&str, TagError> {
  let t = tag.trim();
  if t.is_empty() {
    Err(TagError::Empty)
  } else if t.chars().count() > MAX_TAG_CHARS {
    Err(TagError::TooLong)
  } else if t.chars().any(|c| c.is_control() || c == '/') {
    Err(TagError::BadChar)
  } else {
    Ok(t)
  }
}

/// One row in the picker list. `preview` is a short, already-sanitized
/// excerpt (never the full content).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemPreview {
  pub id: WireItemId,
  pub kind: PreviewKind,
  pub preview: String,
  pub mimes: Vec<String>,
  pub source_app: Option<String>,
  pub created_unix_ms: u64,
  pub last_used_unix_ms: u64,
  pub pinned: bool,
  pub tags: Vec<String>,
  pub total_size: u64,
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{decode_payload, encode_frame};

  fn rt<T: Serialize + for<'de> Deserialize<'de>>(v: &T) -> T {
    let frame = encode_frame(v).unwrap();
    decode_payload(&frame[4..]).unwrap()
  }

  fn item(id: i64) -> ItemPreview {
    ItemPreview {
      id,
      kind: PreviewKind::Image,
      preview: "héllo".into(),
      mimes: vec!["image/png".into()],
      source_app: Some("org.kde.konsole".into()),
      created_unix_ms: 1,
      last_used_unix_ms: 2,
      pinned: true,
      tags: vec!["t".into()],
      total_size: 42,
    }
  }

  fn secret(s: &str) -> Option<UnlockSecret> {
    Some(UnlockSecret::new(Zeroizing::new(s.into())))
  }

  #[test]
  fn requests_round_trip() {
    let reqs = vec![
      PickerReq::Ready,
      PickerReq::Query {
        seq: 7,
        q: "kubectl".into(),
        filters: QueryFilters {
          pinned_only: true,
          kinds: vec![PreviewKind::Text, PreviewKind::Files],
          tags: vec!["work".into()],
          source_app: Some("firefox".into()),
          include_primary: true,
        },
        offset: 50,
        limit: 50,
      },
      PickerReq::Thumb { seq: u32::MAX, id: 3, mime: "image/png".into() },
      PickerReq::Select { id: 1, mode: SelectMode::Copy },
      PickerReq::Select { id: 1, mode: SelectMode::Paste },
      PickerReq::Select { id: 1, mode: SelectMode::PastePlain },
      PickerReq::Pin { id: 2, on: true, seq: 12 },
      PickerReq::Delete { id: 2, seq: 13 },
      PickerReq::Tag { id: 2, tag: "x".into(), on: false, seq: 14 },
      PickerReq::Unlock { provider: UnlockProvider::Passphrase, secret: secret("correct horse") },
      PickerReq::Unlock { provider: UnlockProvider::Fido2, secret: secret("1234") },
      PickerReq::Unlock { provider: UnlockProvider::Fido2, secret: None },
      PickerReq::Unlock { provider: UnlockProvider::KWallet, secret: None },
      PickerReq::Edit { id: 4, mime: "text/html".into() },
    ];
    let reasons = [
      HideReason::Esc,
      HideReason::FocusLost,
      HideReason::Selected,
      HideReason::Requested,
      HideReason::Closed,
    ];
    for r in reqs.into_iter().chain(reasons.map(|reason| PickerReq::Hidden { reason })) {
      assert_eq!(rt(&r), r);
    }
  }

  #[test]
  fn events_round_trip() {
    let mut evts = vec![
      PickerEvt::Show {
        cursor: Some(CursorPos { x: -5, y: 1080 }),
        output: Some("DP-1".into()),
        target_window: Some("{abc}".into()),
        scale_hint: Some(1.25),
      },
      PickerEvt::Show { cursor: None, output: None, target_window: None, scale_hint: None },
      PickerEvt::Hide,
      PickerEvt::Page { seq: 9, offset: 0, items: vec![item(1), item(2)], more: true },
      PickerEvt::Page { seq: 10, offset: 100, items: vec![], more: false },
      PickerEvt::Thumb { seq: 11, id: 1, bytes: vec![0x89, b'P', b'N', b'G'] },
      PickerEvt::Error { seq: Some(9), code: PickerErrorCode::BadQuery, message: "bad".into() },
      PickerEvt::Error { seq: None, code: PickerErrorCode::Internal, message: String::new() },
      PickerEvt::Locked {
        providers: vec![UnlockPrompt::Passphrase, UnlockPrompt::Fido2Touch, UnlockPrompt::Fido2Pin],
      },
      PickerEvt::Locked { providers: vec![] },
      PickerEvt::Unlocked,
      PickerEvt::IndexProgress { done: 1, total: u64::MAX },
      PickerEvt::NewItem { preview: item(3) },
    ];
    for code in [PickerErrorCode::NotFound, PickerErrorCode::Locked, PickerErrorCode::Unavailable] {
      evts.push(PickerEvt::Error { seq: Some(1), code, message: "m".into() });
    }
    for reason in [
      UnlockFailReason::WrongSecret,
      UnlockFailReason::PinInvalid,
      UnlockFailReason::PinBlocked,
      UnlockFailReason::Dismissed,
      UnlockFailReason::Unavailable,
      UnlockFailReason::Other,
    ] {
      for provider in [UnlockProvider::KWallet, UnlockProvider::Passphrase, UnlockProvider::Fido2] {
        evts.push(PickerEvt::UnlockFailed { provider, reason });
      }
    }
    for e in evts {
      assert_eq!(rt(&e), e);
    }
  }

  #[test]
  fn picker_hello_is_separate_from_public() {
    let h = rt(&Hello::picker());
    assert!(h.is_picker_compatible());
    assert_eq!(h.proto, PICKER_PROTO_VERSION);
    assert!(!Hello { proto: 1 }.is_picker_compatible());
  }

  #[test]
  fn secret_is_redacted_in_debug_and_plain_on_the_wire() {
    let req = PickerReq::Unlock { provider: UnlockProvider::Passphrase, secret: secret("hunter2") };
    let dbg = format!("{req:?}");
    assert!(!dbg.contains("hunter2"), "{dbg}");
    assert!(dbg.contains("<redacted>"));
    // Same encoding as a plain Option<String>, so a non-Rust peer is simple.
    let plain = postcard::to_stdvec(&(2u8, Some("hunter2".to_string()))).unwrap();
    let ours = postcard::to_stdvec(&(2u8, secret("hunter2"))).unwrap();
    assert_eq!(plain, ours);
    let back: PickerReq = rt(&req);
    match back {
      PickerReq::Unlock { secret: Some(s), .. } => assert_eq!(s.expose(), "hunter2"),
      other => panic!("{other:?}"),
    }
  }

  #[test]
  fn tag_rules() {
    assert_eq!(check_tag("work"), Ok("work"));
    assert_eq!(check_tag("two words"), Ok("two words"));
    assert_eq!(check_tag("Ünïcode"), Ok("Ünïcode"), "case is kept");
    let max = "é".repeat(MAX_TAG_CHARS);
    assert_eq!(check_tag(&max), Ok(max.as_str()), "characters, not bytes");
    // Normalized: surrounding whitespace is trimmed before the checks.
    assert_eq!(check_tag("  pad\t"), Ok("pad"));
    let padded = format!(" {max} ");
    assert_eq!(check_tag(&padded), Ok(max.as_str()), "trimmed before the length check");
    assert_eq!(check_tag(""), Err(TagError::Empty));
    assert_eq!(check_tag(" \t "), Err(TagError::Empty));
    assert_eq!(check_tag(&"x".repeat(MAX_TAG_CHARS + 1)), Err(TagError::TooLong));
    assert_eq!(check_tag("a/b"), Err(TagError::BadChar));
    assert_eq!(check_tag("a\nb"), Err(TagError::BadChar));
    assert_eq!(check_tag("a\u{85}b"), Err(TagError::BadChar), "C1 control");
    for e in [TagError::Empty, TagError::TooLong, TagError::BadChar] {
      assert!(!e.message().is_empty());
    }
  }

  #[test]
  fn truncated_frames_are_errors_not_panics() {
    let frame =
      encode_frame(&PickerEvt::Page { seq: 1, offset: 0, items: vec![item(1)], more: false })
        .unwrap();
    for cut in 4..frame.len() - 1 {
      assert!(decode_payload::<PickerEvt>(&frame[4..cut]).is_err());
    }
  }
}
