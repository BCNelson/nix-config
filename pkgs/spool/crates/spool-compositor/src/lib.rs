//! Compositor-neutral capability layer for Spool (M7).
//!
//! spoold needs three things from the compositor: a **hotkey** that opens the
//! picker (`Show`), the **active window** (source-app attribution and the
//! auto-paste focus check) plus, if available, the **cursor** position to
//! place the picker, and a way to **paste** (press Ctrl+V in the target). On
//! KWin all of these come from M5 (`spool-kwin`'s script + D-Bus service,
//! `spool-paste`'s fake input). Elsewhere:
//!
//! | Need | KWin | Hyprland | sway / other wlroots |
//! | --- | --- | --- | --- |
//! | hotkey -> `Show` | KWin script | GlobalShortcuts portal (`spool-portal`, xdg-desktop-portal-hyprland) | compositor binding running `spoolctl show` (xdg-desktop-portal-wlr has no GlobalShortcuts) |
//! | active window | KWin script | Hyprland IPC event socket (`spool-hypr`) | `zwlr_foreign_toplevel_manager_v1` (`spool_paste::toplevel`) |
//! | cursor | in the script's `Show` | Hyprland IPC `j/cursorpos` | none (centre the picker) |
//! | paste | `org_kde_kwin_fake_input` | `zwp_virtual_keyboard_v1` | `zwp_virtual_keyboard_v1` |
//!
//! Every backend produces the **same** daemon-facing values: a
//! [`CompositorEvent`] stream (identical to [`spool_kwin::KwinEvent`], so the
//! daemon has one code path) and an [`ActiveWindowTracker`]. Non-KWin
//! backends share one [`EventSink`]; on KWin the `KwinService` returns its own
//! receiver/tracker of the same types.
//!
//! This crate is deliberately tiny: types, the sink, env-based detection and
//! the [`CursorProvider`] trait. Backends live in `spool-portal`,
//! `spool-hypr` and `spool-paste`.
#![forbid(unsafe_code)]

use std::time::Instant;

use tokio::sync::mpsc;

pub use spool_kwin::{ActiveWindow, ActiveWindowTracker, EVENT_QUEUE, MAX_COORD, MAX_ID_LEN};

/// The daemon-facing event, shared by every backend. Same type as
/// [`spool_kwin::KwinEvent`]: `ActiveWindow { app_id, window_id, at }` on
/// focus changes and `Show { cursor, app_id, window_id }` when the hotkey
/// fires (`cursor: None` = centre the picker).
pub type CompositorEvent = spool_kwin::KwinEvent;

/// Which compositor the session runs, from the environment only (no
/// connection is made). Probing (is `org.kde.KWin` on the bus, is the
/// Wayland global advertised) still decides what is actually used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositorKind {
  Kwin,
  Hyprland,
  Sway,
  /// Some other compositor (wlroots-based or not); probe protocols.
  Other,
}

impl CompositorKind {
  /// Detects from the process environment.
  pub fn detect() -> Self {
    Self::detect_from(|k| std::env::var(k).ok())
  }

  /// Detects from an environment lookup function (tests).
  ///
  /// Compositor-specific variables win over `XDG_CURRENT_DESKTOP`:
  /// `HYPRLAND_INSTANCE_SIGNATURE`, `SWAYSOCK`, then `KDE_FULL_SESSION` /
  /// `XDG_CURRENT_DESKTOP` containing `KDE`.
  pub fn detect_from(env: impl Fn(&str) -> Option<String>) -> Self {
    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
    if set("HYPRLAND_INSTANCE_SIGNATURE") {
      return Self::Hyprland;
    }
    if set("SWAYSOCK") {
      return Self::Sway;
    }
    let desktops = env("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let has = |name: &str| desktops.split(':').any(|d| d.eq_ignore_ascii_case(name));
    if set("KDE_FULL_SESSION") || has("KDE") {
      Self::Kwin
    } else if has("Hyprland") {
      Self::Hyprland
    } else if has("sway") {
      Self::Sway
    } else {
      Self::Other
    }
  }
}

/// Where `Show` comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeySource {
  /// The KWin script's global shortcut (M5).
  KwinScript,
  /// `org.freedesktop.portal.GlobalShortcuts` (`spool-portal`).
  Portal,
  /// Nothing registered by Spool: the user binds `spoolctl show` in the
  /// compositor config (sway). The socket `Show` request then carries no
  /// target; spoold fills it from the tracker like [`EventSink::show`].
  External,
}

/// Where the active window comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusSource {
  KwinScript,
  /// Hyprland IPC event socket (`spool-hypr`).
  HyprlandIpc,
  /// `zwlr_foreign_toplevel_manager_v1` (`spool_paste::toplevel`).
  WlrForeignToplevel,
  /// Unknown: no attribution, auto-paste cannot verify focus (spoold should
  /// then paste after a fixed delay or not at all).
  None,
}

/// What the daemon ended up with; for logs and `spoolctl status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
  pub kind: CompositorKind,
  pub hotkey: HotkeySource,
  pub focus: FocusSource,
  /// Whether `Show` carries a cursor position.
  pub cursor: bool,
  /// Whether auto-paste is possible (`PasteHandle::spawn` succeeded).
  pub paste: bool,
}

/// On-demand cursor position (global logical coordinates). Hyprland
/// implements it over IPC; compositors without one simply have no provider.
#[async_trait::async_trait]
pub trait CursorProvider: Send + Sync {
  async fn cursor(&self) -> Option<(i32, i32)>;
}

/// Shared producer side for non-KWin backends: updates the tracker and
/// pushes [`CompositorEvent`]s, exactly like `spool_kwin::KwinService` does
/// for the KWin script. Cheap to clone; give one to every backend.
#[derive(Clone)]
pub struct EventSink {
  tx: mpsc::Sender<CompositorEvent>,
  tracker: ActiveWindowTracker,
}

/// Normalises an id from a compositor: trims, drops empty, rejects (-> `None`,
/// logged) over-long values or control characters. Never logs the value.
pub fn sanitize_id(raw: Option<&str>) -> Option<String> {
  let s = raw?.trim();
  if s.is_empty() {
    return None;
  }
  if s.len() > MAX_ID_LEN || s.chars().any(char::is_control) {
    tracing::debug!(len = s.len(), "dropping malformed id from the compositor");
    return None;
  }
  Some(s.to_owned())
}

impl EventSink {
  /// A sink plus the receiver spoold drains.
  pub fn new() -> (Self, mpsc::Receiver<CompositorEvent>) {
    let (tx, rx) = mpsc::channel(EVENT_QUEUE);
    (Self { tx, tracker: ActiveWindowTracker::new() }, rx)
  }

  /// The tracker this sink feeds (same type the KWin service returns).
  pub fn tracker(&self) -> ActiveWindowTracker {
    self.tracker.clone()
  }

  /// Focus moved. Ids are sanitised with [`sanitize_id`]; `None` = nothing
  /// active. Updates the tracker even if nobody drains the channel.
  pub fn active_window(&self, app_id: Option<&str>, window_id: Option<&str>) {
    let app_id = sanitize_id(app_id);
    let window_id = sanitize_id(window_id);
    let at = Instant::now();
    tracing::trace!(app_id = app_id.as_deref().unwrap_or(""), "active window");
    self.tracker.update(app_id.clone(), window_id.clone(), at);
    self.push(CompositorEvent::ActiveWindow { app_id, window_id, at });
  }

  /// The hotkey fired: `Show` targeted at the currently active window.
  /// Out-of-range cursor coordinates (beyond [`MAX_COORD`]) become `None`.
  pub fn show(&self, cursor: Option<(i32, i32)>) {
    let cursor = cursor.filter(|(x, y)| x.abs() <= MAX_COORD && y.abs() <= MAX_COORD);
    let (app_id, window_id) =
      self.tracker.current().map(|w| (w.app_id, w.window_id)).unwrap_or_default();
    tracing::debug!(
      app_id = app_id.as_deref().unwrap_or(""),
      has_cursor = cursor.is_some(),
      "show"
    );
    self.push(CompositorEvent::Show { cursor, app_id, window_id });
  }

  /// Whether the daemon dropped its receiver (backends may stop then).
  pub fn is_closed(&self) -> bool {
    self.tx.is_closed()
  }

  fn push(&self, ev: CompositorEvent) {
    match self.tx.try_send(ev) {
      Ok(()) => {}
      Err(mpsc::error::TrySendError::Full(_)) => {
        tracing::warn!("compositor event queue full; dropping event")
      }
      Err(mpsc::error::TrySendError::Closed(_)) => {
        tracing::debug!("compositor event receiver gone")
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::HashMap;

  fn kind(vars: &[(&str, &str)]) -> CompositorKind {
    let m: HashMap<String, String> =
      vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    CompositorKind::detect_from(|k| m.get(k).cloned())
  }

  #[test]
  fn detection() {
    assert_eq!(kind(&[]), CompositorKind::Other);
    assert_eq!(kind(&[("XDG_CURRENT_DESKTOP", "KDE")]), CompositorKind::Kwin);
    assert_eq!(kind(&[("KDE_FULL_SESSION", "true")]), CompositorKind::Kwin);
    assert_eq!(kind(&[("XDG_CURRENT_DESKTOP", "Hyprland")]), CompositorKind::Hyprland);
    assert_eq!(kind(&[("HYPRLAND_INSTANCE_SIGNATURE", "abc")]), CompositorKind::Hyprland);
    assert_eq!(kind(&[("SWAYSOCK", "/run/user/1/sway-ipc.sock")]), CompositorKind::Sway);
    assert_eq!(kind(&[("XDG_CURRENT_DESKTOP", "sway:wlroots")]), CompositorKind::Sway);
    // A nested Hyprland inside Plasma: the compositor-specific var wins.
    assert_eq!(
      kind(&[("XDG_CURRENT_DESKTOP", "KDE"), ("HYPRLAND_INSTANCE_SIGNATURE", "x")]),
      CompositorKind::Hyprland
    );
    assert_eq!(kind(&[("HYPRLAND_INSTANCE_SIGNATURE", "")]), CompositorKind::Other);
  }

  #[test]
  fn sanitize() {
    assert_eq!(sanitize_id(None), None);
    assert_eq!(sanitize_id(Some("  ")), None);
    assert_eq!(sanitize_id(Some(" foot ")).as_deref(), Some("foot"));
    assert_eq!(sanitize_id(Some("a\nb")), None);
    assert_eq!(sanitize_id(Some(&"x".repeat(MAX_ID_LEN + 1))), None);
  }

  #[tokio::test]
  async fn sink_feeds_tracker_and_channel() {
    let (sink, mut rx) = EventSink::new();
    let t = sink.tracker();
    sink.active_window(Some("foot"), Some("0xabc"));
    assert!(t.is_active("0xabc"));
    let CompositorEvent::ActiveWindow { app_id, window_id, .. } = rx.recv().await.unwrap() else {
      panic!("expected ActiveWindow")
    };
    assert_eq!((app_id.as_deref(), window_id.as_deref()), (Some("foot"), Some("0xabc")));

    sink.show(Some((10, 20)));
    assert_eq!(
      rx.recv().await.unwrap(),
      CompositorEvent::Show {
        cursor: Some((10, 20)),
        app_id: Some("foot".into()),
        window_id: Some("0xabc".into())
      }
    );
    sink.show(Some((MAX_COORD + 1, 0)));
    assert!(matches!(rx.recv().await.unwrap(), CompositorEvent::Show { cursor: None, .. }));

    sink.active_window(None, None);
    assert_eq!(t.current_app_id(), None);
    drop(rx);
    assert!(sink.is_closed());
    sink.show(None); // no panic when closed
  }
}
