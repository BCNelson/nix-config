//! Loading the Spool KWin script into the running KWin.
//!
//! KWin 6 API (verified against KWin 6.7.5, `src/scripting/scripting.{h,cpp}`):
//!
//! - `org.kde.KWin` `/Scripting` `org.kde.kwin.Scripting`:
//!   `loadScript(s filePath, s pluginName) -> i` (the path is the **JS file**,
//!   not the package dir; returns `-1` if `pluginName` is already loaded),
//!   `unloadScript(s pluginName) -> b` (`deleteLater`, so the name stays
//!   "loaded" until KWin's event loop runs once more), `isScriptLoaded(s) -> b`,
//!   `start()` (runs every loaded-but-not-running script).
//! - Each loaded script is exported at `/Scripting/Script<id>` with
//!   `org.kde.kwin.Script.run()` (replies once the JS has been evaluated, or
//!   with `org.kde.kwin.Scripting.FileError` if the file cannot be read) and
//!   `stop()`. `id` is `scripts.size()` at load time, so it can collide with a
//!   still-registered object of another script after unloads; we therefore
//!   never call `run` on it and use `Scripting.start()` instead.
//! - A JS evaluation error makes KWin delete the script after it ran, so we
//!   confirm with `isScriptLoaded` afterwards.

use std::path::{Path, PathBuf};
use std::time::Duration;

use zbus::Connection;

use crate::service::name_owner;
use crate::{KWIN_BUS_NAME, KwinError, SCRIPT_PLUGIN_NAME};

const SCRIPTING_PATH: &str = "/Scripting";
const SCRIPTING_IFACE: &str = "org.kde.kwin.Scripting";

/// Path of the script entry point inside a KPackage dir.
pub fn main_js(script_dir: &Path) -> PathBuf {
  script_dir.join("contents/code/main.js")
}

/// A script loaded into KWin; unloaded by [`KwinScript::unload`] or (best
/// effort, needs a tokio runtime) on drop.
pub struct KwinScript {
  conn: Connection,
  id: i32,
  loaded: bool,
}

impl std::fmt::Debug for KwinScript {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("KwinScript").field("id", &self.id).field("loaded", &self.loaded).finish()
  }
}

async fn scripting_call<B, R>(conn: &Connection, method: &str, body: &B) -> Result<R, KwinError>
where
  B: serde::Serialize + zbus::zvariant::DynamicType,
  R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
  let reply = conn
    .call_method(Some(KWIN_BUS_NAME), SCRIPTING_PATH, Some(SCRIPTING_IFACE), method, body)
    .await?;
  Ok(reply.body().deserialize::<R>()?)
}

/// Whether KWin (with scripting) is reachable on `conn`.
pub async fn kwin_available(conn: &Connection) -> bool {
  name_owner(conn, KWIN_BUS_NAME).await.is_some()
}

impl KwinScript {
  /// Loads `<script_dir>/contents/code/main.js` as plugin `"spool"`,
  /// replacing a stale copy (e.g. from a crashed spoold), and runs it.
  ///
  /// Returns [`KwinError::Unsupported`] when `org.kde.KWin` has no owner or
  /// does not export `/Scripting` (not KWin): spoold should fall back to the
  /// GlobalShortcuts portal.
  pub async fn load(conn: &Connection, script_dir: &Path) -> Result<Self, KwinError> {
    let js = main_js(script_dir);
    let js = js.canonicalize().map_err(|_| KwinError::ScriptMissing(js.clone()))?;
    if !js.is_file() {
      return Err(KwinError::ScriptMissing(js));
    }
    let js_str = js.to_str().ok_or_else(|| KwinError::ScriptMissing(js.clone()))?.to_owned();

    if !kwin_available(conn).await {
      return Err(KwinError::Unsupported);
    }
    // Probe /Scripting; treat "no such object/method" as Unsupported.
    match scripting_call::<_, bool>(conn, "isScriptLoaded", &(SCRIPT_PLUGIN_NAME,)).await {
      Ok(true) => {
        tracing::info!("replacing a previously loaded KWin script");
        let _: bool = scripting_call(conn, "unloadScript", &(SCRIPT_PLUGIN_NAME,)).await?;
      }
      Ok(false) => {}
      Err(KwinError::DBus(zbus::Error::MethodError(name, _, _)))
        if name.as_str() == "org.freedesktop.DBus.Error.UnknownObject"
          || name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod"
          || name.as_str() == "org.freedesktop.DBus.Error.UnknownInterface"
          || name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" =>
      {
        return Err(KwinError::Unsupported);
      }
      Err(e) => return Err(e),
    }

    // unloadScript is deleteLater(): retry until the old instance is gone.
    let mut id = -1;
    for _ in 0..40 {
      id = scripting_call(conn, "loadScript", &(js_str.as_str(), SCRIPT_PLUGIN_NAME)).await?;
      if id >= 0 {
        break;
      }
      tokio::time::sleep(Duration::from_millis(25)).await;
    }
    if id < 0 {
      return Err(KwinError::ScriptLoad("loadScript returned -1 (already loaded)".into()));
    }
    let mut me = Self { conn: conn.clone(), id, loaded: true };

    // Start it with Scripting.start() (runs every loaded-but-not-running
    // script), never `/Scripting/Script<id>.run`: the id is KWin's script-list
    // length and collides after unloads, so that object can be another,
    // already-running script. On sierra-2 that left Meta+V unregistered after
    // a restart while a second script was loaded.
    let _: () = scripting_call(conn, "start", &()).await?;

    // A JS error deletes the script asynchronously after it ran.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let loaded: bool = scripting_call(conn, "isScriptLoaded", &(SCRIPT_PLUGIN_NAME,)).await?;
    if !loaded {
      me.loaded = false;
      return Err(KwinError::ScriptLoad(
        "script was unloaded right after running (JS error? see KWin log)".into(),
      ));
    }
    tracing::info!(id, "KWin script loaded");
    Ok(me)
  }

  /// KWin's id for this script instance.
  pub fn id(&self) -> i32 {
    self.id
  }

  /// Unloads the script. Returns whether KWin still had it.
  pub async fn unload(mut self) -> Result<bool, KwinError> {
    self.loaded = false;
    scripting_call(&self.conn, "unloadScript", &(SCRIPT_PLUGIN_NAME,)).await
  }
}

impl Drop for KwinScript {
  fn drop(&mut self) {
    if !self.loaded {
      return;
    }
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
      let conn = self.conn.clone();
      rt.spawn(async move {
        let _ = scripting_call::<_, bool>(&conn, "unloadScript", &(SCRIPT_PLUGIN_NAME,)).await;
      });
    }
  }
}

/// Index of the active keyboard layout from KWin (`org.kde.keyboard`
/// `/Layouts` `org.kde.KeyboardLayouts.getLayout`), for
/// `spool_paste` keycode resolution. `None` if unavailable.
pub async fn keyboard_layout_index(conn: &Connection) -> Option<u32> {
  let reply = conn
    .call_method(
      Some("org.kde.keyboard"),
      "/Layouts",
      Some("org.kde.KeyboardLayouts"),
      "getLayout",
      &(),
    )
    .await
    .ok()?;
  reply.body().deserialize::<u32>().ok()
}
