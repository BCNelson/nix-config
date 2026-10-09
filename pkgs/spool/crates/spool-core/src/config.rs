//! TOML configuration (`$XDG_CONFIG_HOME/spool/config.toml`).
//!
//! Every field has a default, so an empty or missing file is valid. Unknown
//! keys are rejected so typos surface instead of silently doing nothing.
//!
//! ```toml
//! max_age_days = 14
//! max_items = 5000
//! primary_selection = false
//! excluded_apps = ["org.keepassxc.KeePassXC"]
//! max_rep_bytes = 8388608
//! max_item_bytes = 16777216
//! fetch_timeout_ms = 2000
//! mime_allowlist = ["text/plain;charset=utf-8", "text/plain", "image/*"]
//! key_provider = "secret-service"   # or "session" (never persisted)
//! auto_paste = true
//! paste_terminals = ["org.kde.konsole", "kitty"]   # replaces the built-in list
//!
//! [picker]
//! renderer = "software"   # or "gpu" (FemtoVG/EGL; falls back to software)
//! prerender = true        # keep the next frame drawn while hidden
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
  /// Unpinned items older than this (by `last_used_at`) are deleted.
  pub max_age_days: u32,
  /// Keep at most this many unpinned items (oldest by `last_used_at` go).
  pub max_items: u32,
  /// Record the primary selection (middle-click buffer).
  pub primary_selection: bool,
  /// Source app ids never recorded. Matched case-insensitively against
  /// `OfferInfo::source_app` (exact match): the app id of the window that
  /// was focused when the offer appeared (KWin script, Hyprland IPC or
  /// wlr-foreign-toplevel); `None` without a focus source.
  pub excluded_apps: Vec<String>,
  /// Max bytes of one representation; larger ones are skipped.
  pub max_rep_bytes: usize,
  /// Max total bytes of one item; larger items are dropped.
  pub max_item_bytes: usize,
  /// Overall deadline for fetching one offer from the source app.
  pub fetch_timeout_ms: u64,
  /// MIME types recorded. Exact match, or `type/*` wildcard. Text variants
  /// (see `item::TEXT_MIMES`) are collapsed by the policy regardless of
  /// which of them are listed, as long as at least one is.
  pub mime_allowlist: Vec<String>,
  /// Where the history data key comes from (M2). `secret-service` (default):
  /// a key slot in the Secret Service (KWallet / gnome-keyring), history is
  /// persisted encrypted. `session`: history lives only in memory and is
  /// lost when the daemon exits.
  pub key_provider: KeyProviderSetting,
  /// After an item is picked with "paste", press the paste chord in the
  /// window that was active when the picker opened (M5; needs KWin's fake
  /// input or a virtual keyboard, and a focus source). `false`: picking only
  /// puts the item on the clipboard.
  pub auto_paste: bool,
  /// App ids that paste with Ctrl+Shift+V (terminal emulators); everything
  /// else gets Ctrl+V. `None` (key absent) = the built-in list
  /// (`spool_paste::chord::DEFAULT_TERMINALS`); a list replaces it entirely
  /// (`[]` = no terminals). Matched case-insensitively, trailing `.desktop`
  /// ignored.
  pub paste_terminals: Option<Vec<String>>,
  /// The resident picker process (`[picker]` table).
  pub picker: PickerSettings,
}

/// [`Config::picker`]: how spoold starts `spool-picker`. Only policy: the
/// executable itself always comes from spoold's (audited) `PATH`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PickerSettings {
  /// `SPOOL_PICKER_RENDERER` for the picker. `gpu` falls back to software
  /// inside the picker when EGL is missing or a software GL; spoold also
  /// relaunches with `software` if a `gpu` picker dies before it is ready.
  pub renderer: PickerRenderer,
  /// `SPOOL_PICKER_PRERENDER`: keep the next frame drawn while hidden
  /// (fastest show; costs the frame buffers' memory). `false` frees them.
  pub prerender: bool,
}

impl Default for PickerSettings {
  fn default() -> Self {
    Self { renderer: PickerRenderer::Software, prerender: true }
  }
}

/// [`PickerSettings::renderer`]. Serialized in kebab-case.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PickerRenderer {
  /// Slint's software renderer into wl_shm buffers (default).
  #[default]
  Software,
  /// FemtoVG over EGL (picker builds with the `gpu` feature).
  Gpu,
}

impl PickerRenderer {
  /// The config-file / `SPOOL_PICKER_RENDERER` spelling.
  pub fn as_str(self) -> &'static str {
    match self {
      PickerRenderer::Software => "software",
      PickerRenderer::Gpu => "gpu",
    }
  }
}

/// [`Config::key_provider`]. Serialized in kebab-case.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeyProviderSetting {
  /// freedesktop Secret Service (KWallet 6, gnome-keyring).
  #[default]
  SecretService,
  /// Memory only; nothing is persisted.
  Session,
}

impl KeyProviderSetting {
  /// The config-file spelling.
  pub fn as_str(self) -> &'static str {
    match self {
      KeyProviderSetting::SecretService => "secret-service",
      KeyProviderSetting::Session => "session",
    }
  }
}

/// Default [`Config::mime_allowlist`].
///
/// Deliberately absent: `image/svg+xml` (XML/script attack surface for the
/// picker's renderer), `image/gif`, and the file-manager markers
/// `x-special/gnome-copied-files` / `application/x-kde-cutselection`
/// (re-pasting a stale "cut" marker later could move files). `text/html` is
/// stored alongside the plain text but never rendered.
pub const DEFAULT_MIME_ALLOWLIST: &[&str] = &[
  "text/plain;charset=utf-8",
  "text/plain",
  "UTF8_STRING",
  "TEXT",
  "STRING",
  "text/uri-list",
  "text/html",
  "image/png",
  "image/jpeg",
  "image/webp",
];

impl Default for Config {
  fn default() -> Self {
    Self {
      max_age_days: 14,
      max_items: 5000,
      primary_selection: false,
      excluded_apps: Vec::new(),
      max_rep_bytes: 8 * 1024 * 1024,
      max_item_bytes: 16 * 1024 * 1024,
      fetch_timeout_ms: 2000,
      mime_allowlist: DEFAULT_MIME_ALLOWLIST.iter().map(|s| s.to_string()).collect(),
      key_provider: KeyProviderSetting::SecretService,
      auto_paste: true,
      paste_terminals: None,
      picker: PickerSettings::default(),
    }
  }
}

impl Config {
  /// Parse TOML text. `origin` is only used in error messages.
  pub fn from_toml_str(s: &str, origin: &Path) -> Result<Self> {
    let cfg: Config = toml::from_str(s)
      .map_err(|e| Error::Config { path: origin.to_owned(), message: e.to_string() })?;
    cfg.validate().map_err(|message| Error::Config { path: origin.to_owned(), message })?;
    Ok(cfg)
  }

  /// Load from `path`; a missing file yields [`Config::default`].
  pub fn load(path: &Path) -> Result<Self> {
    match std::fs::read_to_string(path) {
      Ok(s) => Self::from_toml_str(&s, path),
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
      Err(e) => Err(e.into()),
    }
  }

  /// `$XDG_CONFIG_HOME/spool/config.toml` (falls back to `~/.config`).
  pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
      .filter(|v| !v.is_empty())
      .map(PathBuf::from)
      .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("spool").join("config.toml"))
  }

  fn validate(&self) -> std::result::Result<(), String> {
    if self.max_rep_bytes == 0 || self.max_item_bytes == 0 {
      return Err("size caps must be > 0".into());
    }
    if self.max_rep_bytes > self.max_item_bytes {
      return Err("max_rep_bytes must be <= max_item_bytes".into());
    }
    if self.fetch_timeout_ms == 0 {
      return Err("fetch_timeout_ms must be > 0".into());
    }
    if let Some(t) = &self.paste_terminals
      && let Some(bad) = t.iter().find(|id| id.trim().is_empty() || id.len() > 256)
    {
      return Err(format!("paste_terminals: invalid app id {bad:?}"));
    }
    Ok(())
  }

  pub fn max_age(&self) -> Duration {
    Duration::from_secs(u64::from(self.max_age_days) * 86_400)
  }

  pub fn fetch_timeout(&self) -> Duration {
    Duration::from_millis(self.fetch_timeout_ms)
  }

  /// Retention limits for [`crate::store::Store::retention_sweep`].
  pub fn retention(&self) -> RetentionLimits {
    RetentionLimits { max_age: self.max_age(), max_items: self.max_items }
  }
}

/// Limits applied by the hourly retention sweep. Pinned items are exempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionLimits {
  pub max_age: Duration,
  pub max_items: u32,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn empty_is_default() {
    assert_eq!(Config::from_toml_str("", Path::new("x")).unwrap(), Config::default());
  }

  #[test]
  fn partial_override() {
    let c =
      Config::from_toml_str("max_items = 10\nprimary_selection = true\n", Path::new("x")).unwrap();
    assert_eq!(c.max_items, 10);
    assert!(c.primary_selection);
    assert_eq!(c.max_age_days, 14);
  }

  #[test]
  fn key_provider_setting() {
    assert_eq!(Config::default().key_provider, KeyProviderSetting::SecretService);
    let c = Config::from_toml_str("key_provider = \"session\"", Path::new("x")).unwrap();
    assert_eq!(c.key_provider, KeyProviderSetting::Session);
    let c = Config::from_toml_str("key_provider = \"secret-service\"", Path::new("x")).unwrap();
    assert_eq!(c.key_provider, KeyProviderSetting::SecretService);
    assert!(Config::from_toml_str("key_provider = \"kwallet\"", Path::new("x")).is_err());
  }

  #[test]
  fn paste_settings() {
    let c = Config::default();
    assert!(c.auto_paste);
    assert_eq!(c.paste_terminals, None);
    let c = Config::from_toml_str(
      "auto_paste = false\npaste_terminals = [\"kitty\", \"foot\"]",
      Path::new("x"),
    )
    .unwrap();
    assert!(!c.auto_paste);
    assert_eq!(c.paste_terminals, Some(vec!["kitty".to_string(), "foot".to_string()]));
    let c = Config::from_toml_str("paste_terminals = []", Path::new("x")).unwrap();
    assert_eq!(c.paste_terminals, Some(vec![]));
    assert!(Config::from_toml_str("paste_terminals = [\"\"]", Path::new("x")).is_err());
  }

  #[test]
  fn picker_settings() {
    let c = Config::default();
    assert_eq!(c.picker, PickerSettings { renderer: PickerRenderer::Software, prerender: true });
    let c =
      Config::from_toml_str("[picker]\nrenderer = \"gpu\"\nprerender = false\n", Path::new("x"))
        .unwrap();
    assert_eq!(c.picker.renderer, PickerRenderer::Gpu);
    assert_eq!(c.picker.renderer.as_str(), "gpu");
    assert!(!c.picker.prerender);
    // A partial table keeps the other default.
    let c = Config::from_toml_str("[picker]\nprerender = false\n", Path::new("x")).unwrap();
    assert_eq!(c.picker.renderer, PickerRenderer::Software);
    assert!(Config::from_toml_str("[picker]\nrenderer = \"vulkan\"\n", Path::new("x")).is_err());
    assert!(Config::from_toml_str("[picker]\nexe = \"/tmp/x\"\n", Path::new("x")).is_err());
  }

  #[test]
  fn unknown_key_rejected() {
    assert!(Config::from_toml_str("max_itmes = 1", Path::new("x")).is_err());
  }

  #[test]
  fn invalid_caps_rejected() {
    assert!(
      Config::from_toml_str("max_rep_bytes = 10\nmax_item_bytes = 5", Path::new("x")).is_err()
    );
  }

  #[test]
  fn default_allowlist_matches_security_plan() {
    let c = Config::default();
    for m in [
      "text/plain;charset=utf-8",
      "text/plain",
      "UTF8_STRING",
      "TEXT",
      "STRING",
      "text/uri-list",
      "text/html",
      "image/png",
      "image/jpeg",
      "image/webp",
    ] {
      assert!(c.mime_allowlist.iter().any(|a| a == m), "{m} missing");
    }
    for m in [
      "image/svg+xml",
      "image/gif",
      "x-special/gnome-copied-files",
      "application/x-kde-cutselection",
    ] {
      assert!(!c.mime_allowlist.iter().any(|a| a == m), "{m} must not be allowlisted");
    }
    assert_eq!(c.mime_allowlist.len(), 10);
  }

  #[test]
  fn missing_file_is_default() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(Config::load(&d.path().join("nope.toml")).unwrap(), Config::default());
  }
}
