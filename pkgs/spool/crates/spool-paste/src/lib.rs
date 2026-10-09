//! Auto-paste for Spool on KWin (M5) and wlroots/Hyprland (M7).
//!
//! After the user picks an item, spoold publishes it as the clipboard
//! selection and then asks KWin to "press" the paste chord in the window that
//! was active when the picker opened. The chord goes through KWin's
//! `org_kde_kwin_fake_input` (v4+ `keyboard_key`, evdev keycodes) on a
//! dedicated Wayland connection. Only the chord is ever sent, never content.
//!
//! Keycodes are computed from the compositor's current xkb keymap (received
//! through `wl_keyboard.keymap`), so `v` is found wherever the layout puts it
//! (Dvorak, AZERTY, ...).
//!
//! KWin only exposes `org_kde_kwin_fake_input` to executables whose desktop
//! file lists it in `X-KDE-Wayland-Interfaces` (matched by the canonical path
//! of `/proc/<pid>/exe` against the desktop file's `Exec`), unless KWin runs
//! with `KWIN_WAYLAND_NO_PERMISSION_CHECKS=1`. Without that the global is
//! simply absent.
//!
//! # Other compositors (M7)
//!
//! When fake input is not advertised (v4+), [`Paster::connect`] falls back
//! to `zwp_virtual_keyboard_manager_v1` (sway and other wlroots compositors,
//! Hyprland): it creates one virtual keyboard on the seat, uploads the
//! seat's current keymap (from `wl_keyboard.keymap`; libxkbcommon's default
//! if the seat has no keyboard yet), and sends the same evdev keycodes,
//! resolved with [`KeyCodes::resolve`], plus explicit modifier state. The
//! keymap is re-read before every paste and re-uploaded if the layout
//! changed. Neither protocol -> [`PasteError::Unsupported`].
//! [`Paster::backend`] reports which one is in use.
//!
//! # Helper process
//!
//! spoold is non-dumpable, which hides its `/proc/<pid>/exe` from KWin's
//! permission check, so it pastes through the `spool-paster` helper binary
//! ([`remote`]; desktop file `dev.bcnelson.spool.paster.desktop`).
//!
//! [`toplevel::ToplevelTracker`] follows the active window through
//! `zwlr_foreign_toplevel_manager_v1` for compositors without a KWin script
//! or Hyprland IPC.
#![forbid(unsafe_code)]

pub mod chord;
pub mod handle;
pub mod keys;
pub mod paster;
pub mod remote;
pub mod toplevel;

pub use chord::{KeyEvent, PasteChord, TerminalList};
pub use handle::PasteHandle;
pub use keys::KeyCodes;
pub use paster::{PasteBackend, PasteConfig, Paster};
pub use remote::{PASTER_BIN, RemotePaster};
pub use toplevel::ToplevelTracker;

/// Errors from auto-paste.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PasteError {
  /// The compositor offers neither a usable `org_kde_kwin_fake_input` (not
  /// KWin, too old, or this executable is not authorised) nor
  /// `zwp_virtual_keyboard_manager_v1` (or refused it). spoold should just
  /// leave the item on the clipboard.
  #[error("fake input unsupported: {0}")]
  Unsupported(String),
  #[error("cannot connect to the Wayland display: {0}")]
  Connect(String),
  #[error("Wayland connection error: {0}")]
  Wayland(String),
  #[error("paste thread is gone")]
  ThreadGone,
}
