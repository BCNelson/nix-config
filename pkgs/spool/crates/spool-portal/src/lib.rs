//! The Spool hotkey through `org.freedesktop.portal.GlobalShortcuts` (M7).
//!
//! On compositors other than KWin (where the M5 KWin script owns the
//! shortcut), spoold binds one shortcut, id [`SHORTCUT_ID`], through the XDG
//! desktop portal and turns its `Activated` signal into the shared
//! [`CompositorEvent::Show`](spool_compositor::CompositorEvent) via an
//! [`EventSink`]: the target window comes from the sink's tracker and the
//! cursor from an optional [`CursorProvider`] (Hyprland IPC; `None`
//! elsewhere, which centres the picker).
//!
//! Backends: xdg-desktop-portal-hyprland and xdg-desktop-portal-kde
//! implement GlobalShortcuts; xdg-desktop-portal-wlr (sway) does **not** —
//! there the user binds `spoolctl show` in the compositor config and
//! [`PortalShortcuts::start`] returns [`PortalError::Unsupported`].
//!
//! # App id
//!
//! Portals key permissions and shortcut storage on the caller's app id. A
//! host (non-Flatpak) process has none unless it either runs in a systemd
//! scope/unit named after it (`app-dev.bcnelson.spool.daemon@...service`) or
//! calls `org.freedesktop.host.portal.Registry.Register` (added in
//! xdg-desktop-portal 1.19) **before any other portal call on that D-Bus
//! connection**.
//! [`PortalShortcuts::start`] does the latter with [`APP_ID`], which matches
//! the desktop file the package ships
//! (`share/applications/dev.bcnelson.spool.daemon.desktop`); ashpd exposes it
//! as `register_host_app_with_connection`. Give this crate a dedicated
//! connection so nothing else has talked to the portal on it first.
//!
//! xdg-desktop-portal (verified with 1.22.1) only accepts the registration
//! if it finds that desktop file in `$XDG_DATA_DIRS/applications`
//! (`Could not register app ID: App info not found for '...'` otherwise),
//! so the package must be installed into the user's profile, not just
//! referenced by the systemd unit. A failed registration (or an older portal
//! without the Registry, `UnknownMethod`) is logged and binding proceeds.
//!
//! # Trust
//!
//! `Activated` signals are accepted only from the portal's unique name (zbus
//! proxy match rules) and only for our session and shortcut id. A same-user
//! process could still call spoold's socket `Show` directly, so this is no
//! stronger than `spoolctl show`; spoold rate-limits both.
#![forbid(unsafe_code)]

use std::sync::Arc;

use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use ashpd::desktop::{CreateSessionOptions, ResponseError, Session};
use futures_util::StreamExt;
use spool_compositor::{CursorProvider, EventSink};
use tokio::task::JoinHandle;
use zbus::zvariant::OwnedObjectPath;

/// Shortcut id registered with the portal.
pub const SHORTCUT_ID: &str = "spool-show";
/// Human-readable description shown in the portal's dialog/settings.
pub const SHORTCUT_DESCRIPTION: &str = "Spool: show clipboard history";
/// Preferred trigger (Super+V) in the XDG shortcuts-spec syntax the portal
/// expects: modifiers `CTRL`, `ALT`, `SHIFT`, `NUM`, `LOGO` (= Super/Meta),
/// then an xkb keysym name.
pub const PREFERRED_TRIGGER: &str = "LOGO+v";
/// App id registered with `org.freedesktop.host.portal.Registry`; matches
/// the shipped `dev.bcnelson.spool.daemon.desktop`.
pub const APP_ID: &str = "dev.bcnelson.spool.daemon";

/// Errors from this crate.
#[derive(Debug, thiserror::Error)]
pub enum PortalError {
  /// No portal frontend, or its backend lacks GlobalShortcuts (sway's
  /// xdg-desktop-portal-wlr). spoold should rely on `spoolctl show`.
  #[error("GlobalShortcuts portal unavailable: {0}")]
  Unsupported(String),
  /// The user (or the backend) refused the binding.
  #[error("GlobalShortcuts binding was cancelled")]
  Cancelled,
  #[error("portal: {0}")]
  Portal(String),
}

impl From<ashpd::Error> for PortalError {
  fn from(e: ashpd::Error) -> Self {
    let missing = |z: &zbus::Error| match z {
      zbus::Error::MethodError(name, _, _) => is_missing_service(name.as_str()),
      zbus::Error::FDO(f) => matches!(
        **f,
        zbus::fdo::Error::ServiceUnknown(_)
          | zbus::fdo::Error::NameHasNoOwner(_)
          | zbus::fdo::Error::UnknownMethod(_)
          | zbus::fdo::Error::UnknownInterface(_)
          | zbus::fdo::Error::UnknownObject(_)
      ),
      _ => false,
    };
    match e {
      ashpd::Error::PortalNotFound(i) => Self::Unsupported(format!("{i} not provided")),
      ashpd::Error::Response(ResponseError::Cancelled) => Self::Cancelled,
      ashpd::Error::Zbus(ref z) | ashpd::Error::Portal(ashpd::PortalError::ZBus(ref z))
        if missing(z) =>
      {
        Self::Unsupported(e.to_string())
      }
      other => Self::Portal(other.to_string()),
    }
  }
}

/// Bus name of the portal frontend.
pub const PORTAL_BUS_NAME: &str = "org.freedesktop.portal.Desktop";

/// Whether the portal frontend is running or D-Bus-activatable on `conn`.
pub async fn portal_present(conn: &zbus::Connection) -> bool {
  let Ok(dbus) = zbus::fdo::DBusProxy::new(conn).await else { return false };
  let Ok(name) = zbus::names::BusName::try_from(PORTAL_BUS_NAME) else { return false };
  if dbus.name_has_owner(name).await.unwrap_or(false) {
    return true;
  }
  dbus
    .list_activatable_names()
    .await
    .map(|names| names.iter().any(|n| n.as_str() == PORTAL_BUS_NAME))
    .unwrap_or(false)
}

fn is_missing_service(err_name: &str) -> bool {
  matches!(
    err_name,
    "org.freedesktop.DBus.Error.ServiceUnknown"
      | "org.freedesktop.DBus.Error.NameHasNoOwner"
      | "org.freedesktop.DBus.Error.UnknownMethod"
      | "org.freedesktop.DBus.Error.UnknownInterface"
      | "org.freedesktop.DBus.Error.UnknownObject"
  )
}

/// Options for [`PortalShortcuts::start`].
#[derive(Debug, Clone)]
pub struct PortalConfig {
  /// App id to register as (`None`: skip `host.portal.Registry`, e.g. when
  /// spoold runs in an `app-<id>` systemd unit or a Flatpak).
  pub app_id: Option<String>,
  /// Preferred trigger in shortcuts-spec syntax (`None`: let the portal
  /// ask / leave it unassigned).
  pub preferred_trigger: Option<String>,
}

impl Default for PortalConfig {
  fn default() -> Self {
    Self { app_id: Some(APP_ID.into()), preferred_trigger: Some(PREFERRED_TRIGGER.into()) }
  }
}

impl PortalConfig {
  /// The single shortcut Spool binds.
  pub fn shortcut(&self) -> NewShortcut {
    NewShortcut::new(SHORTCUT_ID, SHORTCUT_DESCRIPTION)
      .preferred_trigger(self.preferred_trigger.as_deref())
  }
}

/// Registers `app_id` for this connection with
/// `org.freedesktop.host.portal.Registry`. Missing interface (portal older
/// than 1.19) is not an error; anything else is returned.
pub async fn register_app_id(conn: &zbus::Connection, app_id: &str) -> Result<bool, PortalError> {
  let id = ashpd::AppID::try_from(app_id)
    .map_err(|_| PortalError::Portal(format!("invalid app id {app_id:?}")))?;
  match ashpd::register_host_app_with_connection(conn.clone(), id).await {
    Ok(()) => Ok(true),
    Err(ashpd::Error::Zbus(zbus::Error::MethodError(name, _, _)))
      if is_missing_service(name.as_str()) =>
    {
      tracing::info!(error = %name, "host portal registry unavailable; portal will derive the app id itself");
      Ok(false)
    }
    Err(ashpd::Error::PortalNotFound(_)) => Ok(false),
    Err(e) => Err(e.into()),
  }
}

/// Object path a [`Session`] refers to (ashpd keeps the accessor private).
fn session_path(s: &Session<GlobalShortcuts>) -> Option<OwnedObjectPath> {
  use zbus::zvariant::{LE, serialized::Context, to_bytes};
  let ctxt = Context::new_dbus(LE, 0);
  let bytes = to_bytes(ctxt, s).ok()?;
  bytes.deserialize::<OwnedObjectPath>().ok().map(|(p, _)| p)
}

/// A bound portal shortcut feeding an [`EventSink`]. Dropping it stops the
/// listener (the portal session closes when the connection does);
/// [`stop`](Self::stop) closes the session explicitly.
pub struct PortalShortcuts {
  task: JoinHandle<()>,
  session: Session<GlobalShortcuts>,
  trigger: Option<String>,
}

impl PortalShortcuts {
  /// Registers the app id (if configured), creates a GlobalShortcuts
  /// session, binds [`SHORTCUT_ID`] and starts forwarding activations as
  /// `Show` events.
  ///
  /// `conn` must be the session bus the portal lives on, ideally dedicated
  /// to this (see the crate docs on the app id).
  pub async fn start(
    conn: &zbus::Connection,
    cfg: PortalConfig,
    sink: EventSink,
    cursor: Option<Arc<dyn CursorProvider>>,
  ) -> Result<Self, PortalError> {
    if !portal_present(conn).await {
      return Err(PortalError::Unsupported(format!("{PORTAL_BUS_NAME} is not on the bus")));
    }
    if let Some(id) = cfg.app_id.as_deref()
      && let Err(e) = register_app_id(conn, id).await
    {
      // e.g. xdg-desktop-portal >= 1.20 refuses ids without an installed
      // desktop file ("App info not found"). Binding still works with the
      // portal-derived (possibly empty) app id; shortcuts may then not be
      // remembered across sessions.
      tracing::warn!(error = %e, "could not register the app id with the portal; continuing");
    }
    let portal = GlobalShortcuts::with_connection(conn.clone()).await?;
    // Subscribe before binding so an early activation is not lost.
    let mut activated = portal.receive_activated().await?;
    let session = portal.create_session(CreateSessionOptions::default()).await?;
    let our_session = session_path(&session)
      .ok_or_else(|| PortalError::Portal("cannot read the session handle".into()))?;
    let bound = portal
      .bind_shortcuts(&session, &[cfg.shortcut()], None, BindShortcutsOptions::default())
      .await?
      .response()?;
    let Some(ours) = bound.shortcuts().iter().find(|s| s.id() == SHORTCUT_ID) else {
      let _ = session.close().await;
      return Err(PortalError::Cancelled);
    };
    let trigger = Some(ours.trigger_description().to_owned()).filter(|t| !t.is_empty());
    tracing::info!(trigger = trigger.as_deref().unwrap_or("(unassigned)"), "portal shortcut bound");

    let task = tokio::spawn(async move {
      while let Some(a) = activated.next().await {
        if a.shortcut_id() != SHORTCUT_ID || a.session_handle() != our_session.as_ref() {
          continue;
        }
        let pos = match &cursor {
          Some(c) => c.cursor().await,
          None => None,
        };
        sink.show(pos);
        if sink.is_closed() {
          break;
        }
      }
      tracing::debug!("portal shortcut listener ended");
    });
    Ok(Self { task, session, trigger })
  }

  /// What the portal says triggers the shortcut (e.g. `Super+V`), if
  /// assigned.
  pub fn trigger_description(&self) -> Option<&str> {
    self.trigger.as_deref()
  }

  /// Whether the listener is still running.
  pub fn is_running(&self) -> bool {
    !self.task.is_finished()
  }

  /// Closes the portal session and stops listening.
  pub async fn stop(self) {
    self.task.abort();
    let _ = self.session.close().await;
  }
}

impl Drop for PortalShortcuts {
  fn drop(&mut self) {
    self.task.abort();
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::HashMap;
  use zbus::zvariant::{LE, OwnedValue, serialized::Context, to_bytes};

  type Encoded = (String, HashMap<String, OwnedValue>);

  fn encode(s: &NewShortcut) -> Encoded {
    let bytes = to_bytes(Context::new_dbus(LE, 0), s).unwrap();
    bytes.deserialize::<Encoded>().unwrap().0
  }

  #[test]
  fn shortcut_request() {
    let (id, opts) = encode(&PortalConfig::default().shortcut());
    assert_eq!(id, "spool-show");
    assert_eq!(String::try_from(opts["description"].clone()).unwrap(), SHORTCUT_DESCRIPTION);
    assert_eq!(String::try_from(opts["preferred_trigger"].clone()).unwrap(), "LOGO+v");
    assert_eq!(opts.len(), 2);

    let none = PortalConfig { preferred_trigger: None, ..Default::default() };
    let (_, opts) = encode(&none.shortcut());
    assert!(!opts.contains_key("preferred_trigger"));
  }

  #[test]
  fn signature_matches_spec() {
    use zbus::zvariant::Type;
    // BindShortcuts takes a(sa{sv}).
    assert_eq!(NewShortcut::SIGNATURE.to_string(), "(sa{sv})");
  }

  #[test]
  fn defaults() {
    let c = PortalConfig::default();
    assert_eq!(c.app_id.as_deref(), Some("dev.bcnelson.spool.daemon"));
    assert!(ashpd::AppID::try_from(APP_ID).is_ok());
  }

  #[test]
  fn error_mapping() {
    let e: PortalError = ashpd::Error::Response(ResponseError::Cancelled).into();
    assert!(matches!(e, PortalError::Cancelled));
    assert!(is_missing_service("org.freedesktop.DBus.Error.ServiceUnknown"));
    assert!(!is_missing_service("org.freedesktop.portal.Error.NotAllowed"));
  }
}
