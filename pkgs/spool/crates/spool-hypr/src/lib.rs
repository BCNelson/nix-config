//! Hyprland backend for Spool (M7).
//!
//! Talks to the running Hyprland over its two IPC sockets in
//! `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/` (older releases:
//! `/tmp/hypr/$HYPRLAND_INSTANCE_SIGNATURE/`):
//!
//! - `.socket.sock` (request/response, one request per connection):
//!   `j/cursorpos` -> `{"x":..,"y":..}` for picker placement and
//!   `j/activewindow` -> the focused client (`{}` when none);
//! - `.socket2.sock` (event stream, `EVENT>>DATA\n` lines): `activewindow`
//!   (`class,title`) immediately followed by `activewindowv2` (`address`
//!   without `0x`).
//!
//! [`HyprFocus`] turns the event stream into the shared
//! [`spool_compositor::CompositorEvent::ActiveWindow`] events (app id =
//! `class`, window id = `0x<address>`), and [`HyprIpc`] implements
//! [`spool_compositor::CursorProvider`].
//!
//! Window titles arrive on the event socket (`activewindow>>class,title`)
//! and inside `j/activewindow`; they are parsed past and never stored or
//! logged. Any process of the user can talk to these sockets (and could
//! spoof them only by replacing Hyprland's runtime dir), so the same trust
//! model as the KWin script applies: values influence attribution and the
//! auto-paste focus check only.
#![forbid(unsafe_code)]

pub mod ipc;
pub mod parse;

pub use ipc::{HyprFocus, HyprIpc};
pub use parse::{HyprEvent, HyprWindow};

/// Errors from this crate.
#[derive(Debug, thiserror::Error)]
pub enum HyprError {
  /// Not running under Hyprland (`HYPRLAND_INSTANCE_SIGNATURE` unset or no
  /// socket directory).
  #[error("Hyprland IPC is not available: {0}")]
  Unsupported(String),
  #[error("Hyprland IPC I/O: {0}")]
  Io(#[from] std::io::Error),
  #[error("Hyprland IPC timed out")]
  Timeout,
  #[error("unexpected Hyprland IPC reply: {0}")]
  Protocol(String),
}
