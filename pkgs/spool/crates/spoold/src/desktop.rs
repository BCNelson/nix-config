//! Desktop integration: where the picker shortcut, the active window, the
//! cursor and auto-paste come from (M5 KWin, M7 other compositors).
//!
//! Selection ([`start`], see INTERFACES.md "Compositor backends (M7)"):
//!
//! - **KWin** (`org.kde.KWin` on the session bus): [`KwinService`] owns
//!   `dev.bcnelson.spool` (calls only accepted from KWin's unique name), then
//!   [`KwinScript::load`] loads `kwin-script` (`$SPOOL_KWIN_SCRIPT_DIR`, else
//!   `<exe>/../../share/spool/kwin-script`). The script reports focus changes
//!   and the Meta+V shortcut (with the cursor). If the script cannot be
//!   loaded the service is stopped and the generic path below is used.
//! - **Other** compositors: focus from Hyprland IPC, else
//!   `zwlr_foreign_toplevel_manager_v1`, else none; the shortcut from the
//!   GlobalShortcuts portal (bound in the background: the portal may ask the
//!   user), else nothing (`HotkeySource::External`: the user binds
//!   `spoolctl show`); cursor from Hyprland IPC only.
//! - **Paste** through the `spool-paster` helper ([`spool_paste::remote`]:
//!   fake input on KWin, virtual keyboard elsewhere), found next to the
//!   spoold executable or on the (audited) PATH, started with a scrubbed
//!   environment; only with `auto_paste = true` and a focus source (without
//!   one the target cannot be verified, so nothing is pasted).
//!
//! Every source feeds the same `CompositorEvent` channel and
//! `ActiveWindowTracker`. Using the active window as an offer's source app
//! is a heuristic (the app that copies is nearly always the focused one).
//!
//! All of it is best effort: a missing session bus / KWin / portal / paste
//! protocol is logged and the daemon carries on with less integration.
//! Real-session KWin note: spoold is non-dumpable, so KWin cannot read its
//! `/proc/<pid>/exe` and never grants it fake input. The helper is granted
//! it through the installed `dev.bcnelson.spool.paster.desktop`
//! (`X-KDE-Wayland-Interfaces=org_kde_kwin_fake_input`, `Exec` = the
//! canonical path of `spool-paster`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use spool_compositor::{
  ActiveWindowTracker, Capabilities, CompositorEvent, CompositorKind, CursorProvider, EventSink,
  FocusSource, HotkeySource,
};
use spool_core::config::Config;
use spool_kwin::{KwinError, KwinScript, KwinService, ServiceConfig};
use spool_paste::{PasteConfig, PasteError, RemotePaster, ToplevelTracker};
use tokio::sync::{mpsc, watch};

use crate::autopaste::{AutoPaste, HelperSink, PasteSink};

/// Longest wait for one D-Bus step during startup / shutdown.
const BUS_STEP_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest wait for one cleanup step at shutdown.
const STOP_STEP_TIMEOUT: Duration = Duration::from_secs(2);

/// What the orchestrator takes from the desktop integration.
pub struct DesktopLink {
  /// Focus changes and `Show` requests from the compositor.
  pub events: mpsc::Receiver<CompositorEvent>,
  /// The active window (`None`: no focus source).
  pub focus: Option<ActiveWindowTracker>,
  pub autopaste: AutoPaste,
  /// For `StatusInfo::capabilities` (updated once the portal is bound).
  pub capabilities: watch::Receiver<spool_proto::Capabilities>,
}

/// The shortcut source in use. The seam the non-KWin fallbacks plug into.
enum Hotkey {
  Kwin {
    svc: KwinService,
    script: KwinScript,
  },
  /// Portal binding in progress / done (filled by a background task).
  Portal(Arc<Mutex<PortalSlot>>),
  External,
}

#[derive(Default)]
struct PortalSlot {
  task: Option<tokio::task::JoinHandle<()>>,
  shortcuts: Option<spool_portal::PortalShortcuts>,
}

/// Keeps the focus backends alive (each stops on drop).
#[allow(dead_code)] // held for their Drop
enum FocusKeep {
  Kwin,
  Hypr(spool_hypr::HyprFocus),
  Toplevel(ToplevelTracker),
  None,
}

/// Running desktop integration; [`Desktop::shutdown`] tears it down.
pub struct Desktop {
  hotkey: Hotkey,
  _focus: FocusKeep,
}

fn script_dir() -> Option<PathBuf> {
  if let Some(d) = std::env::var_os("SPOOL_KWIN_SCRIPT_DIR").filter(|d| !d.is_empty()) {
    return Some(PathBuf::from(d));
  }
  let exe = std::env::current_exe().ok()?;
  Some(exe.parent()?.parent()?.join("share/spool/kwin-script"))
}

async fn session_bus() -> Option<zbus::Connection> {
  match tokio::time::timeout(BUS_STEP_TIMEOUT, zbus::Connection::session()).await {
    Ok(Ok(c)) => Some(c),
    Ok(Err(e)) => {
      tracing::info!("no session bus ({e}); no KWin script or shortcut portal");
      None
    }
    Err(_) => {
      tracing::warn!("connecting to the session bus timed out; no KWin script or shortcut portal");
      None
    }
  }
}

/// KWin path: service + script. `Err(true)`: not usable, try the generic
/// path; `Err(false)`: another spoold owns the name, add nothing.
async fn start_kwin(
  conn: &zbus::Connection,
) -> Result<(KwinService, KwinScript, mpsc::Receiver<CompositorEvent>), bool> {
  let (svc, rx) = match KwinService::start(conn, ServiceConfig::default()).await {
    Ok(v) => v,
    Err(KwinError::NameTaken(name)) => {
      tracing::warn!(
        "{name} is owned by another process (a second spoold?); KWin integration disabled"
      );
      return Err(false);
    }
    Err(e) => {
      tracing::warn!("KWin service: {e}; trying the portal instead");
      return Err(true);
    }
  };
  let Some(dir) = script_dir() else {
    tracing::warn!("cannot locate the KWin script; trying the portal instead");
    let _ = svc.stop().await;
    return Err(true);
  };
  match tokio::time::timeout(BUS_STEP_TIMEOUT, KwinScript::load(conn, &dir)).await {
    Ok(Ok(script)) => Ok((svc, script, rx)),
    Ok(Err(e)) => {
      // Unsupported = not KWin after all. The M7 portal fallback takes over.
      tracing::warn!("KWin script not loaded ({e}); trying the portal instead");
      let _ = svc.stop().await;
      Err(true)
    }
    Err(_) => {
      tracing::warn!("loading the KWin script timed out; trying the portal instead");
      let _ = svc.stop().await;
      Err(true)
    }
  }
}

/// Generic focus source: Hyprland IPC, else wlr foreign-toplevel.
async fn start_focus(sink: &EventSink) -> (FocusKeep, FocusSource, Option<spool_hypr::HyprIpc>) {
  if let Ok(ipc) = spool_hypr::HyprIpc::from_env() {
    match spool_hypr::HyprFocus::spawn(ipc.clone(), sink.clone()).await {
      Ok(f) => return (FocusKeep::Hypr(f), FocusSource::HyprlandIpc, Some(ipc)),
      Err(e) => tracing::warn!("Hyprland IPC focus tracking: {e}"),
    }
  }
  let s = sink.clone();
  let spawned = tokio::task::spawn_blocking(move || {
    ToplevelTracker::spawn(None, move |a, w| s.active_window(a, w))
  })
  .await;
  match spawned {
    Ok(Ok(t)) => (FocusKeep::Toplevel(t), FocusSource::WlrForeignToplevel, None),
    Ok(Err(PasteError::Unsupported(_))) => {
      tracing::info!("no focus source (no KWin script, Hyprland IPC or wlr foreign-toplevel)");
      (FocusKeep::None, FocusSource::None, None)
    }
    Ok(Err(e)) => {
      tracing::warn!("foreign-toplevel focus tracking: {e}");
      (FocusKeep::None, FocusSource::None, None)
    }
    Err(e) => {
      tracing::warn!("foreign-toplevel task: {e}");
      (FocusKeep::None, FocusSource::None, None)
    }
  }
}

/// Bind the portal shortcut in the background on a dedicated connection.
fn start_portal(
  sink: EventSink,
  cursor: Option<Arc<dyn CursorProvider>>,
  caps: watch::Sender<spool_proto::Capabilities>,
) -> Arc<Mutex<PortalSlot>> {
  let slot = Arc::new(Mutex::new(PortalSlot::default()));
  let slot2 = slot.clone();
  let task = tokio::spawn(async move {
    let Some(conn) = session_bus().await else { return };
    let cfg = spool_portal::PortalConfig::default();
    match spool_portal::PortalShortcuts::start(&conn, cfg, sink, cursor).await {
      Ok(p) => {
        caps.send_modify(|c| c.hotkey = "portal".into());
        slot2.lock().unwrap_or_else(|p| p.into_inner()).shortcuts = Some(p);
      }
      Err(spool_portal::PortalError::Unsupported(why)) => tracing::info!(
        "no GlobalShortcuts portal ({why}); bind a shortcut to `spoolctl show` to open the picker"
      ),
      Err(e) => tracing::warn!(
        "GlobalShortcuts portal: {e}; bind a shortcut to `spoolctl show` to open the picker"
      ),
    }
  });
  slot.lock().unwrap_or_else(|p| p.into_inner()).task = Some(task);
  slot
}

async fn start_paste(config: &Config, layout_bus: Option<zbus::Connection>) -> Option<HelperSink> {
  if !config.auto_paste {
    tracing::info!("auto_paste = false: picking only sets the clipboard");
    return None;
  }
  let own = std::env::current_exe().ok();
  let Some(exe) = RemotePaster::locate(own.as_deref()) else {
    tracing::warn!(
      "auto-paste unavailable: {} not found next to spoold or on PATH",
      spool_paste::PASTER_BIN
    );
    return None;
  };
  match RemotePaster::spawn(&exe, PasteConfig::default()).await {
    Ok(paster) => Some(HelperSink { paster, layout_bus }),
    Err(PasteError::Unsupported(why)) => {
      tracing::info!(
        "auto-paste unavailable: {why} (on KWin, {} must be installed so KWin grants {} fake \
         input)",
        "dev.bcnelson.spool.paster.desktop",
        exe.display()
      );
      None
    }
    Err(e) => {
      tracing::warn!("auto-paste unavailable: {e}");
      None
    }
  }
}

fn kind_str(k: CompositorKind) -> &'static str {
  match k {
    CompositorKind::Kwin => "kwin",
    CompositorKind::Hyprland => "hyprland",
    CompositorKind::Sway => "sway",
    CompositorKind::Other => "other",
  }
}

fn hotkey_str(h: HotkeySource) -> &'static str {
  match h {
    HotkeySource::KwinScript => "kwin-script",
    HotkeySource::Portal => "portal",
    HotkeySource::External => "external",
  }
}

fn focus_str(f: FocusSource) -> &'static str {
  match f {
    FocusSource::KwinScript => "kwin-script",
    FocusSource::HyprlandIpc => "hyprland-ipc",
    FocusSource::WlrForeignToplevel => "wlr-foreign-toplevel",
    FocusSource::None => "none",
  }
}

/// Wire format of [`Capabilities`] (+ the paste backend).
pub fn capabilities_info(c: &Capabilities, paste_backend: &str) -> spool_proto::Capabilities {
  spool_proto::Capabilities {
    compositor: kind_str(c.kind).into(),
    hotkey: hotkey_str(c.hotkey).into(),
    focus: focus_str(c.focus).into(),
    cursor: c.cursor,
    auto_paste: c.paste,
    paste_backend: paste_backend.into(),
  }
}

/// Probe the session and start what is available. Never fails.
pub async fn start(config: &Config) -> (Desktop, DesktopLink) {
  let kind = CompositorKind::detect();
  let conn = session_bus().await;
  let kwin = match &conn {
    Some(c) if spool_kwin::kwin_available(c).await => start_kwin(c).await,
    _ => Err(true),
  };

  let (hotkey_handle, focus_keep, events, tracker, hotkey, focus, cursor, layout_bus, portal) =
    match kwin {
      Ok((svc, script, rx)) => {
        tracing::info!("KWin integration active (Meta+V via the KWin script)");
        let tracker = svc.tracker();
        (
          Hotkey::Kwin { svc, script },
          FocusKeep::Kwin,
          rx,
          Some(tracker),
          HotkeySource::KwinScript,
          FocusSource::KwinScript,
          true,
          conn.clone(),
          None,
        )
      }
      Err(generic) => {
        let (sink, rx) = EventSink::new();
        let (keep, focus, hypr) = start_focus(&sink).await;
        let tracker = (focus != FocusSource::None).then(|| sink.tracker());
        let cursor: Option<Arc<dyn CursorProvider>> =
          hypr.map(|i| Arc::new(i) as Arc<dyn CursorProvider>);
        let has_cursor = cursor.is_some();
        // Portal only if a session bus exists and the KWin name was not
        // already taken by another spoold.
        let portal = (generic && conn.is_some()).then_some((sink, cursor));
        (
          Hotkey::External,
          keep,
          rx,
          tracker,
          HotkeySource::External,
          focus,
          has_cursor,
          None,
          portal,
        )
      }
    };

  let sink = if tracker.is_some() {
    start_paste(config, layout_bus).await
  } else {
    if config.auto_paste {
      tracing::info!("auto-paste disabled: no focus source to verify the paste target");
    }
    None
  };
  let backend = sink.as_ref().map(|s| s.backend()).unwrap_or("none");
  let mut autopaste = AutoPaste::unavailable(config);
  autopaste.sink = sink.map(|s| Arc::new(s) as Arc<dyn PasteSink>);
  autopaste.focus = tracker.clone();
  let caps = Capabilities { kind, hotkey, focus, cursor, paste: autopaste.available() };
  let (caps_tx, caps_rx) = watch::channel(capabilities_info(&caps, backend));
  let hotkey_handle = match portal {
    Some((sink, cursor)) => Hotkey::Portal(start_portal(sink, cursor, caps_tx)),
    None => hotkey_handle,
  };
  tracing::info!(
    compositor = kind_str(kind),
    hotkey = hotkey_str(hotkey),
    focus = focus_str(focus),
    cursor,
    auto_paste = autopaste.available(),
    paste_backend = backend,
    "desktop integration"
  );
  (
    Desktop { hotkey: hotkey_handle, _focus: focus_keep },
    DesktopLink { events, focus: tracker, autopaste, capabilities: caps_rx },
  )
}

impl Desktop {
  /// Unload the KWin script, release the bus name, close the portal
  /// session. Each step is bounded.
  pub async fn shutdown(self) {
    match self.hotkey {
      Hotkey::Kwin { svc, script } => {
        crate::shutdown::phase("unloading the KWin script");
        match tokio::time::timeout(STOP_STEP_TIMEOUT, script.unload()).await {
          Ok(Ok(_)) => tracing::debug!("KWin script unloaded"),
          Ok(Err(e)) => tracing::warn!("unloading the KWin script: {e}"),
          Err(_) => tracing::warn!("unloading the KWin script timed out"),
        }
        crate::shutdown::phase("releasing the KWin service name");
        if tokio::time::timeout(STOP_STEP_TIMEOUT, svc.stop()).await.is_err() {
          tracing::warn!("releasing the KWin service name timed out");
        }
      }
      Hotkey::Portal(slot) => {
        crate::shutdown::phase("closing the portal session");
        let (task, shortcuts) = {
          let mut s = slot.lock().unwrap_or_else(|p| p.into_inner());
          (s.task.take(), s.shortcuts.take())
        };
        if let Some(t) = task {
          t.abort();
        }
        if let Some(p) = shortcuts
          && tokio::time::timeout(STOP_STEP_TIMEOUT, p.stop()).await.is_err()
        {
          tracing::warn!("closing the portal session timed out");
        }
      }
      Hotkey::External => {}
    }
  }
}
