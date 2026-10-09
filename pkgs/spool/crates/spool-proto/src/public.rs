//! Public socket messages (`spoolctl`, KWin script, other local clients).

use serde::{Deserialize, Serialize};

/// Request from a public client to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublicReq {
  /// Show the picker. Without a picker installed:
  /// [`PublicResp::NotYetImplemented`].
  /// Rate-limited by the daemon.
  Show,
  /// Show the picker in "return" mode: the item the user picks is sent
  /// back on this connection ([`PublicResp::Picked`], its preferred
  /// representation, text first) instead of being pasted; Esc / focus loss
  /// answers [`PublicResp::Cancelled`]. The answer comes when the user is
  /// done (no time limit). Without a picker: [`PublicResp::NotYetImplemented`].
  /// Rate-limited by the daemon (shared with `Show`).
  Pick,
  /// Put `data` on the clipboard as `mime` (the daemon becomes the selection
  /// owner) and record it in history. Answers [`PublicResp::Ok`].
  /// `data.len()` is bounded by the frame limit and the configured caps.
  Copy { mime: String, data: Vec<u8> },
  /// Return the newest stored clipboard item's preferred representation
  /// (text first). Answers [`PublicResp::Current`] or [`PublicResp::Empty`].
  Current,
  /// Stop recording. `None` = until [`PublicReq::Resume`]; `Some(n)` = for
  /// `n` seconds. Answers [`PublicResp::Ok`].
  Pause { secs: Option<u32> },
  /// Resume recording. Answers [`PublicResp::Ok`] (also when not paused).
  Resume,
  /// Answers [`PublicResp::Status`].
  Status,
}

/// Response from the daemon to a public client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublicResp {
  Ok,
  Current {
    mime: String,
    data: Vec<u8>,
  },
  /// No stored clipboard item (or the newest is not servable).
  Empty,
  Status(StatusInfo),
  Error {
    code: ErrorCode,
    message: String,
  },
  /// The request is valid but its feature is not available in this daemon
  /// (`Show`/`Pick` without a picker).
  NotYetImplemented,
  /// Answer to [`PublicReq::Pick`]: the picked item's preferred
  /// representation. (Appended: postcard enum tags are positional.)
  Picked {
    mime: String,
    data: Vec<u8>,
  },
  /// Answer to [`PublicReq::Pick`]: the picker was closed without a choice
  /// (Esc, focus loss, replaced by another `Show`/`Pick`, picker restart).
  Cancelled,
}

/// Machine-readable error class for [`PublicResp::Error`]. `message` is for
/// humans and must never contain clipboard content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
  /// Malformed or semantically invalid request (e.g. empty mime).
  BadRequest,
  /// Payload exceeds a configured cap.
  TooLarge,
  /// Too many `Show`/`Pick` requests in a short window.
  RateLimited,
  /// Peer is not allowed (wrong uid, sandboxed client, ...).
  Forbidden,
  /// The Wayland side is not connected / compositor unsupported.
  Unavailable,
  /// Store is locked (encrypted, no key yet; M2+).
  Locked,
  /// Anything else; see `message` and the daemon journal.
  Internal,
}

/// Recording state reported by [`StatusInfo`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PauseState {
  Recording,
  /// Paused until this wall-clock time, in milliseconds since the Unix epoch.
  PausedUntil {
    unix_ms: u64,
  },
  /// Paused until an explicit `Resume`.
  PausedIndefinitely,
}

/// Daemon status snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusInfo {
  /// Daemon version (`CARGO_PKG_VERSION`).
  pub version: String,
  pub paused: PauseState,
  /// Number of stored items (all selections).
  pub item_count: u64,
  /// Human-readable compositor connection description, e.g.
  /// `"wayland-1 (ext_data_control_manager_v1 v1)"`; `None` while not
  /// (yet) connected.
  pub compositor: Option<String>,
  /// Whether the primary selection is being recorded.
  pub primary_enabled: bool,
  /// Whether history currently goes to the encrypted on-disk database
  /// (`false` while capturing into the in-memory session store).
  pub encrypted: bool,
  /// Whether the persistent store is unlocked and in use.
  pub unlocked: bool,
  /// Key/unlock state for humans and scripts (M2). One of `"locked"`
  /// (unlock in progress), `"waiting-for-wallet"`, `"unlocking"`,
  /// `"ready"`, `"session-only"` (`key_provider = "session"`: memory only),
  /// `"dev-plaintext"` (development override) or `"error: <reason>"`
  /// (capturing into memory only; see the daemon log). Appended last:
  /// postcard is positional.
  pub key_state: String,
  /// Desktop integration in use (M5/M7): how the picker is triggered, where
  /// focus / cursor come from, whether auto-paste can work. `None` until the
  /// daemon has probed the compositor. Appended last (postcard is
  /// positional).
  pub capabilities: Option<Capabilities>,
}

/// [`StatusInfo::capabilities`]. Plain strings so this crate does not depend
/// on the compositor crates; values are stable, lowercase identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
  /// Detected compositor family: `kwin`, `hyprland`, `sway`, `other`.
  pub compositor: String,
  /// Global shortcut source: `kwin-script`, `portal`, or `external` (the
  /// user binds `spoolctl show` themselves).
  pub hotkey: String,
  /// Active-window source: `kwin-script`, `hyprland-ipc`,
  /// `wlr-foreign-toplevel`, or `none` (auto-paste disabled).
  pub focus: String,
  /// Whether the cursor position is known when the picker opens.
  pub cursor: bool,
  /// Whether auto-paste is possible right now (enabled in config, a paste
  /// backend and a focus source are available).
  pub auto_paste: bool,
  /// Paste backend: `fake-input`, `virtual-keyboard`, or `none`.
  pub paste_backend: String,
}
