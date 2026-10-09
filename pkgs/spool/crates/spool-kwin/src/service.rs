//! The `dev.bcnelson.spool.Kwin` D-Bus interface the KWin script calls.

use std::time::Instant;

use tokio::sync::mpsc;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::{Connection, interface};

use crate::tracker::ActiveWindowTracker;
use crate::{BUS_NAME, EVENT_QUEUE, KWIN_BUS_NAME, KwinError, KwinEvent, OBJECT_PATH, validate};

/// Service options.
#[derive(Debug, Clone)]
pub struct ServiceConfig {
  /// Only accept calls whose sender currently owns `org.kde.KWin`. Default
  /// `true`. This is hygiene (a same-user process can still reach KWin or
  /// spoold in other ways), not a security boundary. Turn it off for manual
  /// testing with `busctl`.
  pub require_kwin_sender: bool,
}

impl Default for ServiceConfig {
  fn default() -> Self {
    Self { require_kwin_sender: true }
  }
}

struct KwinIface {
  tx: mpsc::Sender<KwinEvent>,
  tracker: ActiveWindowTracker,
  cfg: ServiceConfig,
}

impl KwinIface {
  async fn check_sender(&self, conn: &Connection, hdr: &Header<'_>) -> zbus::fdo::Result<()> {
    if !self.cfg.require_kwin_sender {
      return Ok(());
    }
    let Some(sender) = hdr.sender() else {
      return Err(zbus::fdo::Error::AccessDenied("no sender".into()));
    };
    let owner = name_owner(conn, KWIN_BUS_NAME).await;
    if owner.as_deref() == Some(sender.as_str()) {
      Ok(())
    } else {
      tracing::debug!(sender = %sender, "rejecting KWin call from a non-KWin sender");
      Err(zbus::fdo::Error::AccessDenied("only KWin may call this interface".into()))
    }
  }

  fn push(&self, ev: KwinEvent) {
    match self.tx.try_send(ev) {
      Ok(()) => {}
      Err(mpsc::error::TrySendError::Full(_)) => {
        tracing::warn!("kwin event queue full; dropping event")
      }
      Err(mpsc::error::TrySendError::Closed(_)) => tracing::debug!("kwin event receiver gone"),
    }
  }
}

fn invalid(e: validate::Invalid) -> zbus::fdo::Error {
  zbus::fdo::Error::InvalidArgs(e.to_string())
}

#[interface(name = "dev.bcnelson.spool.Kwin")]
impl KwinIface {
  /// Focus changed. `app_id` is the window's desktop file name (or resource
  /// class), `internal_id` its `QUuid`; both empty when nothing is active.
  #[zbus(name = "ActiveWindow")]
  async fn active_window(
    &self,
    app_id: String,
    internal_id: String,
    #[zbus(header)] hdr: Header<'_>,
    #[zbus(connection)] conn: &Connection,
  ) -> zbus::fdo::Result<()> {
    self.check_sender(conn, &hdr).await?;
    let app_id = validate::app_id(&app_id).map_err(invalid)?;
    let window_id = validate::window_id(&internal_id).map_err(invalid)?;
    let at = Instant::now();
    tracing::trace!(app_id = app_id.as_deref().unwrap_or(""), "active window");
    self.tracker.update(app_id.clone(), window_id.clone(), at);
    self.push(KwinEvent::ActiveWindow { app_id, window_id, at });
    Ok(())
  }

  /// The global shortcut fired. `x`/`y` is the cursor position in global
  /// logical coordinates; `app_id`/`internal_id` the window active at that
  /// moment.
  #[zbus(name = "Show")]
  async fn show(
    &self,
    x: i32,
    y: i32,
    app_id: String,
    internal_id: String,
    #[zbus(header)] hdr: Header<'_>,
    #[zbus(connection)] conn: &Connection,
  ) -> zbus::fdo::Result<()> {
    self.check_sender(conn, &hdr).await?;
    let app_id = validate::app_id(&app_id).map_err(invalid)?;
    let window_id = validate::window_id(&internal_id).map_err(invalid)?;
    let cursor = validate::cursor(x, y);
    tracing::debug!(
      app_id = app_id.as_deref().unwrap_or(""),
      has_cursor = cursor.is_some(),
      "kwin show"
    );
    self.push(KwinEvent::Show { cursor, app_id, window_id });
    Ok(())
  }
}

/// Current unique-name owner of `name`, if any.
pub(crate) async fn name_owner(conn: &Connection, name: &str) -> Option<String> {
  let reply = conn
    .call_method(
      Some("org.freedesktop.DBus"),
      "/org/freedesktop/DBus",
      Some("org.freedesktop.DBus"),
      "GetNameOwner",
      &(name,),
    )
    .await
    .ok()?;
  reply.body().deserialize::<String>().ok()
}

/// A running service: the exported object plus the owned bus name.
pub struct KwinService {
  conn: Connection,
  tracker: ActiveWindowTracker,
  stopped: bool,
}

impl KwinService {
  /// Exports the interface on `conn` and claims [`BUS_NAME`]
  /// (`DoNotQueue`: fails with [`KwinError::NameTaken`] if someone else has
  /// it, so a second spoold cannot silently steal the KWin script's calls).
  ///
  /// Start this **before** [`KwinScript::load`](crate::KwinScript::load) so
  /// the script's initial `ActiveWindow` report is not lost.
  pub async fn start(
    conn: &Connection,
    cfg: ServiceConfig,
  ) -> Result<(Self, mpsc::Receiver<KwinEvent>), KwinError> {
    let (tx, rx) = mpsc::channel(EVENT_QUEUE);
    let tracker = ActiveWindowTracker::new();
    let iface = KwinIface { tx, tracker: tracker.clone(), cfg };
    let server = conn.object_server();
    if !server.at(OBJECT_PATH, iface).await? {
      return Err(KwinError::ScriptLoad(format!(
        "object {OBJECT_PATH} already exported on this connection"
      )));
    }
    let flags = RequestNameFlags::DoNotQueue.into();
    match conn.request_name_with_flags(BUS_NAME, flags).await {
      Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {}
      Ok(_) | Err(zbus::Error::NameTaken) => {
        let _ = server.remove::<KwinIface, _>(OBJECT_PATH).await;
        return Err(KwinError::NameTaken(BUS_NAME));
      }
      Err(e) => {
        let _ = server.remove::<KwinIface, _>(OBJECT_PATH).await;
        return Err(e.into());
      }
    }
    tracing::info!("owning {BUS_NAME} on the session bus");
    Ok((Self { conn: conn.clone(), tracker, stopped: false }, rx))
  }

  /// The active-window tracker this service feeds.
  pub fn tracker(&self) -> ActiveWindowTracker {
    self.tracker.clone()
  }

  pub fn connection(&self) -> &Connection {
    &self.conn
  }

  /// Releases the name and removes the object. (Dropping the connection does
  /// the same implicitly.)
  pub async fn stop(mut self) -> Result<(), KwinError> {
    self.stopped = true;
    let _ = self.conn.release_name(BUS_NAME).await;
    let _ = self.conn.object_server().remove::<KwinIface, _>(OBJECT_PATH).await;
    Ok(())
  }
}

impl Drop for KwinService {
  fn drop(&mut self) {
    if self.stopped {
      return;
    }
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
      let conn = self.conn.clone();
      rt.spawn(async move {
        let _ = conn.release_name(BUS_NAME).await;
        let _ = conn.object_server().remove::<KwinIface, _>(OBJECT_PATH).await;
      });
    }
  }
}
