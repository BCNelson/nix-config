//! Spool IPC protocol: message types and framing.
//!
//! # Wire format
//!
//! Every message is a *frame*: a 4-byte big-endian payload length (at most
//! [`MAX_FRAME`]) followed by a [`postcard`]-encoded payload.
//!
//! # Handshake
//!
//! The first frame in each direction is a [`Hello`]. The client sends
//! `Hello { proto: PROTO_VERSION }`; the server answers with its own
//! `Hello { proto: PROTO_VERSION }`. If the versions differ the server closes
//! the connection right after sending its `Hello`, and the client must report
//! a version mismatch to the user. postcard is not self-describing, so both
//! sides must agree on the message type of every frame; the handshake is
//! therefore a fixed `Hello` exchange, never an enum.
//!
//! # Channels
//!
//! * The **public** socket (`$XDG_RUNTIME_DIR/spool/sock`, see [`paths`])
//!   carries [`PublicReq`] (client -> daemon) and [`PublicResp`]
//!   (daemon -> client), strictly one response per request, in order.
//! * The **picker** channel (a private socketpair the daemon hands to the
//!   picker process it spawns) carries [`PickerReq`] (picker -> daemon) and
//!   [`PickerEvt`] (daemon -> picker). It has its own version,
//!   [`PICKER_PROTO_VERSION`], exchanged as [`Hello::picker`] (the picker
//!   speaks first); see [`picker`] for request ids and the unlock flow.
//!
//! Public and picker messages are deliberately separate enums so a public
//! client can never speak picker messages (e.g. `Unlock`) and vice versa.

#![forbid(unsafe_code)]

pub mod framing;
pub mod paths;
pub mod picker;
pub mod public;

pub use framing::{FrameError, MAX_FRAME, decode_payload, encode_frame, read_frame, write_frame};
#[cfg(feature = "tokio")]
pub use framing::{read_frame_async, write_frame_async};
pub use picker::{
  CursorPos, HideReason, ItemPreview, MAX_TAG_CHARS, PICKER_PROTO_VERSION, PickerErrorCode,
  PickerEvt, PickerReq, PreviewKind, QueryFilters, SelectMode, TagError, UnlockFailReason,
  UnlockPrompt, UnlockProvider, UnlockSecret, check_tag,
};
pub use public::{Capabilities, ErrorCode, PauseState, PublicReq, PublicResp, StatusInfo};

use serde::{Deserialize, Serialize};

/// Protocol version carried in [`Hello`]. Bump on any incompatible change to
/// any message type in this crate.
pub const PROTO_VERSION: u16 = 1;

/// Item identifier on the wire. Matches `spool_core::item::ItemId.0`
/// (an SQLite rowid; always positive).
pub type WireItemId = i64;

/// First frame on every connection, in both directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
  pub proto: u16,
}

impl Hello {
  /// `Hello` for this build's [`PROTO_VERSION`].
  pub const fn current() -> Self {
    Self { proto: PROTO_VERSION }
  }

  /// Whether the peer speaks the same protocol version as this build.
  pub const fn is_compatible(&self) -> bool {
    self.proto == PROTO_VERSION
  }
}
