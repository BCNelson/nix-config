//! Script channel: `spoold` <-> `spool-scripts` (the Lua script host).
//!
//! The host is a separate process (its own systemd user unit, not spoold's
//! child) that connects to `scripts.sock` next to the public socket
//! ([`crate::paths::script_socket_path`]). spoold accepts it only if the
//! peer's executable is the `spool-scripts` on its audited `PATH`.
//!
//! Handshake: the host sends [`Hello::script`] first, spoold answers with
//! its own and closes on a mismatch; then the host sends one
//! [`ScriptReq::Register`]. After that spoold sends [`ScriptEvt`]s and the
//! host answers each [`ScriptEvt::Filter`] with exactly one
//! [`ScriptReq::FilterDone`] carrying the same `seq`, in order.
//!
//! Nothing the host sends is ever logged by spoold as text: a [`Verdict`]
//! has no reason string, because a script's message may quote content.
//!
//! Enums and structs are positional (postcard): append, never insert.

use serde::{Deserialize, Serialize};

use crate::{Hello, WireItemId};

/// Script channel version carried in [`Hello::script`].
pub const SCRIPT_PROTO_VERSION: u16 = 2;

impl Hello {
  /// `Hello` for this build's [`SCRIPT_PROTO_VERSION`].
  pub const fn script() -> Self {
    Self { proto: SCRIPT_PROTO_VERSION }
  }

  /// Whether the peer speaks this build's script protocol.
  pub const fn is_script_compatible(&self) -> bool {
    self.proto == SCRIPT_PROTO_VERSION
  }
}

/// Which selection an item came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireSelection {
  Clipboard,
  Primary,
}

/// One representation with its bytes.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireRep {
  pub mime: String,
  pub data: Vec<u8>,
}

impl std::fmt::Debug for WireRep {
  /// Never prints the data (rule 3).
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("WireRep").field("mime", &self.mime).field("len", &self.data.len()).finish()
  }
}

/// An item as scripts see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptItem {
  /// `None` for a [`ScriptEvt::Filter`]: the item is not stored yet.
  pub id: Option<WireItemId>,
  pub selection: WireSelection,
  pub source_app: Option<String>,
  /// Milliseconds since the Unix epoch.
  pub created_at_ms: i64,
  pub tags: Vec<String>,
  /// Canonical representations (with data), canonical first.
  pub reps: Vec<WireRep>,
  /// `(alias, canonical)`: further mimes the item answers to with the data
  /// of a canonical representation (e.g. `UTF8_STRING`).
  pub aliases: Vec<(String, String)>,
}

/// Host -> spoold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptReq {
  /// Sent once, right after the handshake: what the loaded scripts handle.
  Register { filters: bool },
  /// Answer to [`ScriptEvt::Filter`] with the same `seq`.
  FilterDone { seq: u64, verdict: Verdict },
}

/// A filter's decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
  /// Store the item. `tags` are added; `reps = Some` replaces its content
  /// (spoold re-checks it like a manual copy: caps, secret patterns). With
  /// a rewrite, `original` keeps the copy as it came in as a hidden
  /// earlier version, and `by` names the scripts that rewrote it (file
  /// names, recorded as the version's note). (v2: `original`, `by`.)
  Keep { tags: Vec<String>, reps: Option<Vec<WireRep>>, original: bool, by: Vec<String> },
  /// Do not store the item.
  Drop,
}

impl Verdict {
  /// Keep unchanged.
  pub const fn keep() -> Self {
    Self::Keep { tags: Vec::new(), reps: None, original: true, by: Vec::new() }
  }
}

/// spoold -> host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptEvt {
  /// Run the filters on a copy the built-in policy would store. spoold
  /// stores the item unfiltered if no `FilterDone` arrives within
  /// `budget_ms`.
  Filter { seq: u64, budget_ms: u32, item: ScriptItem },
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{decode_payload, encode_frame};

  fn rt<T: Serialize + serde::de::DeserializeOwned>(v: &T) -> T {
    decode_payload(&encode_frame(v).unwrap()[4..]).unwrap()
  }

  fn item() -> ScriptItem {
    ScriptItem {
      id: None,
      selection: WireSelection::Clipboard,
      source_app: Some("org.kde.konsole".into()),
      created_at_ms: 1_700_000_000_000,
      tags: vec![],
      reps: vec![WireRep { mime: "text/plain;charset=utf-8".into(), data: b"hello".to_vec() }],
      aliases: vec![("UTF8_STRING".into(), "text/plain;charset=utf-8".into())],
    }
  }

  #[test]
  fn round_trips() {
    let evt = ScriptEvt::Filter { seq: 7, budget_ms: 250, item: item() };
    assert_eq!(rt(&evt), evt);
    for req in [
      ScriptReq::Register { filters: true },
      ScriptReq::FilterDone { seq: 7, verdict: Verdict::Drop },
      ScriptReq::FilterDone {
        seq: 8,
        verdict: Verdict::Keep {
          tags: vec!["work".into()],
          reps: Some(vec![WireRep { mime: "text/plain".into(), data: b"x".to_vec() }]),
          original: false,
          by: vec!["10-redact.lua".into()],
        },
      },
    ] {
      assert_eq!(rt(&req), req);
    }
  }

  #[test]
  fn script_hello_is_separate() {
    let h = rt(&Hello::script());
    assert!(h.is_script_compatible());
    assert_eq!(h.proto, SCRIPT_PROTO_VERSION);
  }

  #[test]
  fn rep_debug_hides_data() {
    let dbg = format!("{:?}", item());
    assert!(!dbg.contains("hello"), "{dbg}");
  }
}
