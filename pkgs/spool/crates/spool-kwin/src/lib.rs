//! KWin integration for Spool (M5).
//!
//! - [`service`]: owns `dev.bcnelson.spool` on the session bus and exports
//!   `dev.bcnelson.spool.Kwin` at `/dev/bcnelson/spool`. The KWin script calls
//!   `ActiveWindow(s app_id, s internal_id)` on every focus change and
//!   `Show(i x, i y, s app_id, s internal_id)` when the user presses the
//!   global shortcut (Meta+V). Calls become [`KwinEvent`]s on a tokio
//!   channel.
//! - [`script`]: loads `kwin-script/contents/code/main.js` into the running
//!   KWin through `org.kde.kwin.Scripting` and unloads it again.
//! - [`tracker`]: [`ActiveWindowTracker`], the latest active window plus an
//!   async `wait_for(window_id, timeout)` used by auto-paste.
//!
//! # Trust model
//!
//! Any process of the same user can call methods on the session bus, so
//! nothing that arrives here is trusted for anything security relevant:
//!
//! - `Show` is equivalent to the public `spoolctl show` request (it opens the
//!   picker, which the user then drives); spoold must apply the same rate
//!   limit to it as to the socket request.
//! - `ActiveWindow` only influences source-app attribution and the auto-paste
//!   "is focus back on the original window" check. A forged event can at worst
//!   mis-attribute an item or make a paste go to (or be withheld from) the
//!   window the forger chose, which a same-user process could do anyway.
//!
//! By default the service additionally only accepts calls whose sender is the
//! current owner of `org.kde.KWin` (see [`ServiceConfig::require_kwin_sender`]);
//! that is hygiene, not a security boundary.
//!
//! Inputs are validated (length <= [`MAX_ID_LEN`], no control characters,
//! window ids look like `QUuid::toString()`, coordinates within
//! [`MAX_COORD`]) and are never logged verbatim beyond app ids. Window titles
//! are never sent by the script.
#![forbid(unsafe_code)]

pub mod script;
pub mod service;
pub mod tracker;
pub mod validate;

pub use script::{KwinScript, keyboard_layout_index, kwin_available};
pub use service::{KwinService, ServiceConfig};
pub use tracker::{ActiveWindow, ActiveWindowTracker};

use std::time::Instant;

/// Well-known bus name Spool owns on the session bus.
pub const BUS_NAME: &str = "dev.bcnelson.spool";
/// Object path of the KWin-facing interface.
pub const OBJECT_PATH: &str = "/dev/bcnelson/spool";
/// Interface name the KWin script calls.
pub const INTERFACE: &str = "dev.bcnelson.spool.Kwin";
/// Plugin name the script is registered under in KWin.
pub const SCRIPT_PLUGIN_NAME: &str = "spool";
/// Bus name of KWin itself.
pub const KWIN_BUS_NAME: &str = "org.kde.KWin";

/// Maximum byte length of an app id or window id.
pub const MAX_ID_LEN: usize = 256;
/// Largest accepted absolute cursor coordinate (logical pixels).
pub const MAX_COORD: i32 = 1 << 20;
/// Capacity of the event channel; events beyond it are dropped (logged).
pub const EVENT_QUEUE: usize = 64;

/// An event produced by the KWin script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KwinEvent {
  /// Focus moved. `None` fields mean "no active window" (or unknown).
  ActiveWindow { app_id: Option<String>, window_id: Option<String>, at: Instant },
  /// The global shortcut was pressed. `cursor` is `None` if KWin sent an
  /// out-of-range position. `app_id`/`window_id` describe the window that was
  /// active when the shortcut fired (the auto-paste target).
  Show { cursor: Option<(i32, i32)>, app_id: Option<String>, window_id: Option<String> },
}

/// Errors from this crate.
#[derive(Debug, thiserror::Error)]
pub enum KwinError {
  /// Not running under KWin (no `org.kde.KWin` on the bus, or no scripting).
  /// spoold should fall back to the GlobalShortcuts portal.
  #[error("KWin is not available on this session bus")]
  Unsupported,
  /// Another process already owns `dev.bcnelson.spool`.
  #[error("bus name {0} is already owned by another process")]
  NameTaken(&'static str),
  /// The script package is missing or unreadable.
  #[error("KWin script not found at {0}")]
  ScriptMissing(std::path::PathBuf),
  /// KWin refused or failed to load/run the script.
  #[error("KWin failed to load the script: {0}")]
  ScriptLoad(String),
  #[error("D-Bus: {0}")]
  DBus(#[from] zbus::Error),
  #[error("D-Bus: {0}")]
  Fdo(#[from] zbus::fdo::Error),
}
