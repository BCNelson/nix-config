//! Picking an item: the picker seam ([`PickerLauncher`]) and auto-paste
//! ([`AutoPaste`]).
//!
//! # Flow (owned by the orchestrator)
//!
//! 1. `Show` (KWin script / portal hotkey, or `spoolctl show`) is rate
//!    limited, recorded as a [`ShowContext`] (cursor + [`PasteTarget`]: the
//!    window that was active when the shortcut fired) and handed to the
//!    [`PickerLauncher`].
//! 2. The picker answers with `Request::Select { id, mode }`
//!    ([`crate::orchestrator::Request::Select`]). The orchestrator loads the
//!    item, bumps `last_used_at`, publishes it as the clipboard selection and,
//!    for the paste modes, waits until the Wayland side reports
//!    `NewSelection { ours: true }` for **this** publish (publishes are
//!    counted; a foreign offer first aborts the paste and leaves the
//!    clipboard alone; no confirmation within [`ACK_TIMEOUT`] aborts too).
//!    (`spoolctl pick` instead gets the item back on its connection,
//!    [`SelectOutcome::Returned`].)
//! 3. Then [`PickerLauncher::hide`] (a no-op: the picker hid itself before
//!    sending `Select`; skipped if another `Show` came in meanwhile), and [`AutoPaste::paste_into`]: wait up
//!    to [`FOCUS_WAIT`] for focus to return to the target window, choose the
//!    chord for its app id ([`TerminalList`]) and press it.
//!
//! Never pastes without a target window id, without a focus source, without
//! a paste backend, or with `auto_paste = false`; the item then simply stays
//! on the clipboard ([`SelectOutcome::NotPasted`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use spool_compositor::ActiveWindowTracker;
use spool_core::config::Config;
use spool_paste::{PasteChord, PasteError, RemotePaster, TerminalList};
use spool_proto::ItemPreview;

use crate::keyflow::BoxFuture;

/// How long to wait for focus to come back to the paste target after the
/// picker closed.
pub const FOCUS_WAIT: Duration = Duration::from_millis(500);

/// How long to wait for the compositor to confirm our clipboard publish
/// before giving up on the paste.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(1);

/// The window a paste is meant for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteTarget {
  /// Opaque compositor window id (KWin `QUuid`, Hyprland address, ...).
  pub window_id: String,
  /// Its app id, for the paste chord (terminals use Ctrl+Shift+V).
  pub app_id: Option<String>,
}

/// Where a `Show` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowOrigin {
  /// Global shortcut (KWin script or GlobalShortcuts portal).
  Hotkey,
  /// `PublicReq::Show` on the socket (`spoolctl show`).
  Socket,
}

/// Everything the picker needs to open, recorded at `Show` time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowContext {
  pub origin: ShowOrigin,
  /// Global logical cursor position, if known.
  pub cursor: Option<(i32, i32)>,
  /// Paste target; `None` = copy only.
  pub target: Option<PasteTarget>,
  pub at: Instant,
}

/// Why opening the picker failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PickerError {
  /// No picker on this install (`spool-picker` not on spoold's PATH).
  #[error("picker not wired")]
  NotWired,
  /// The picker exists but cannot be shown (gave up restarting it, ...).
  #[error("picker: {0}")]
  Failed(String),
}

/// Whether history is locked, as the picker's unlock panel needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockView {
  /// Readable history (ready, session-only, plaintext).
  Unlocked,
  /// Locked (session store in use). `failed`: the background key flow gave
  /// up (`KeyState::Failed`), so only an interactive unlock can help.
  Locked { failed: bool },
}

/// The orchestrator's handle on the picker process. Called on the
/// orchestrator task: implementations must not block (spawn and return).
///
/// [`crate::picker::ResidentPicker`] is the real one (launched when
/// `spool-picker` is on PATH); [`UnwiredPicker`] stands in otherwise.
pub trait PickerLauncher: Send + 'static {
  /// Open (or raise) the picker.
  fn show(&mut self, ctx: &ShowContext) -> Result<(), PickerError>;
  /// Close the picker if it is visible (before auto-paste waits for focus
  /// to return). A picker that already hid itself is left alone.
  fn hide(&mut self);
  /// Whether [`PickerLauncher::item_stored`] is wanted (a picker exists).
  fn wants_items(&self) -> bool {
    false
  }
  /// A clipboard item was stored (inserted or bumped): `PickerEvt::NewItem`.
  fn item_stored(&mut self, _preview: ItemPreview) {}
  /// The key state changed (`Locked{prompts}` / `Unlocked` for the picker).
  fn lock_changed(&mut self, _lock: LockView) {}
}

/// Stand-in without a picker: logs and reports [`PickerError::NotWired`].
#[derive(Debug, Default)]
pub struct UnwiredPicker;

impl PickerLauncher for UnwiredPicker {
  fn show(&mut self, ctx: &ShowContext) -> Result<(), PickerError> {
    tracing::info!(
      origin = ?ctx.origin,
      has_cursor = ctx.cursor.is_some(),
      has_target = ctx.target.is_some(),
      "picker not wired; show request recorded"
    );
    Err(PickerError::NotWired)
  }

  fn hide(&mut self) {}
}

/// Presses a paste chord in the focused window.
pub trait PasteSink: Send + Sync + 'static {
  /// `fake-input` / `virtual-keyboard` (for `Status`).
  fn backend(&self) -> &'static str;
  fn paste(&self, chord: PasteChord) -> BoxFuture<'_, Result<(), PasteError>>;
}

/// The real sink: the `spool-paster` helper process (spoold itself is
/// non-dumpable, so KWin would never grant it fake input; see
/// [`spool_paste::remote`]), plus KWin's layout index (when on KWin) so
/// keycodes match the active layout.
pub struct HelperSink {
  pub paster: RemotePaster,
  pub layout_bus: Option<zbus::Connection>,
}

impl PasteSink for HelperSink {
  fn backend(&self) -> &'static str {
    match self.paster.backend() {
      spool_paste::PasteBackend::FakeInput => "fake-input",
      spool_paste::PasteBackend::VirtualKeyboard => "virtual-keyboard",
    }
  }

  fn paste(&self, chord: PasteChord) -> BoxFuture<'_, Result<(), PasteError>> {
    Box::pin(async move {
      let layout = match &self.layout_bus {
        Some(c) => spool_kwin::keyboard_layout_index(c).await,
        None => None,
      };
      self.paster.paste(chord, layout).await
    })
  }
}

/// Result of `Request::Select`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectOutcome {
  /// On the clipboard (`SelectMode::Copy`).
  Copied,
  /// On the clipboard and the chord was sent to the target window.
  Pasted { chord: PasteChord },
  /// On the clipboard (unless `ForeignOffer`), but not pasted.
  NotPasted(NoPaste),
  /// Returned to a `spoolctl pick` client instead (clipboard untouched).
  Returned,
}

/// Why an auto-paste did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoPaste {
  /// `auto_paste = false`.
  Disabled,
  /// No paste backend (no fake input / virtual keyboard).
  NoBackend,
  /// No focus source, so the target cannot be verified.
  NoFocusSource,
  /// The `Show` carried no target window.
  NoTarget,
  /// Focus did not return to the target within [`FOCUS_WAIT`].
  FocusTimeout,
  /// Another app took the clipboard before our publish was confirmed.
  ForeignOffer,
  /// The compositor did not confirm our publish within [`ACK_TIMEOUT`].
  NotConfirmed,
  /// A newer `Select` replaced this one.
  Superseded,
  /// The paste backend failed.
  Failed(String),
}

/// Errors of `Request::Select` (nothing was published).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectError {
  #[error("no such item")]
  NotFound,
  /// `PastePlain` on an item without text, or an item without data.
  #[error("item has nothing to paste in this mode")]
  NoData,
  #[error("cannot set the clipboard: {0}")]
  Unavailable(String),
  #[error("internal error: {0}")]
  Internal(String),
}

/// Auto-paste configuration and backends (cheap to clone into a task).
#[derive(Clone)]
pub struct AutoPaste {
  pub enabled: bool,
  pub sink: Option<Arc<dyn PasteSink>>,
  pub focus: Option<ActiveWindowTracker>,
  pub terminals: TerminalList,
  pub focus_wait: Duration,
}

impl std::fmt::Debug for AutoPaste {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("AutoPaste")
      .field("enabled", &self.enabled)
      .field("backend", &self.sink.as_ref().map(|s| s.backend()))
      .field("focus", &self.focus.is_some())
      .finish_non_exhaustive()
  }
}

impl AutoPaste {
  /// No backends (tests, or no desktop integration).
  pub fn unavailable(config: &Config) -> Self {
    Self {
      enabled: config.auto_paste,
      sink: None,
      focus: None,
      terminals: terminals(config),
      focus_wait: FOCUS_WAIT,
    }
  }

  /// Whether a paste could happen at all (target permitting).
  pub fn available(&self) -> bool {
    self.check().is_none()
  }

  /// The reason a paste cannot happen regardless of the target.
  pub fn check(&self) -> Option<NoPaste> {
    if !self.enabled {
      Some(NoPaste::Disabled)
    } else if self.sink.is_none() {
      Some(NoPaste::NoBackend)
    } else if self.focus.is_none() {
      Some(NoPaste::NoFocusSource)
    } else {
      None
    }
  }

  /// Steps after the clipboard holds the item: wait for focus to return to
  /// `target`, then press the chord for its app id.
  pub async fn paste_into(&self, target: &PasteTarget) -> SelectOutcome {
    if let Some(why) = self.check() {
      return SelectOutcome::NotPasted(why);
    }
    let (Some(sink), Some(focus)) = (&self.sink, &self.focus) else {
      return SelectOutcome::NotPasted(NoPaste::NoBackend);
    };
    if !focus.wait_for(&target.window_id, self.focus_wait).await {
      tracing::info!(
        wait_ms = self.focus_wait.as_millis() as u64,
        "focus did not return to the paste target; not pasting (the item is on the clipboard)"
      );
      return SelectOutcome::NotPasted(NoPaste::FocusTimeout);
    }
    let chord = self.terminals.chord_for(target.app_id.as_deref());
    tracing::debug!(app_id = target.app_id.as_deref().unwrap_or(""), ?chord, "auto-paste");
    match sink.paste(chord).await {
      Ok(()) => {
        tracing::info!(?chord, "auto-pasted");
        SelectOutcome::Pasted { chord }
      }
      Err(e) => {
        tracing::warn!("auto-paste failed: {e}; the item is on the clipboard");
        SelectOutcome::NotPasted(NoPaste::Failed(e.to_string()))
      }
    }
  }
}

/// `paste_terminals` (absent = built-in list) as a [`TerminalList`].
pub fn terminals(config: &Config) -> TerminalList {
  match &config.paste_terminals {
    None => TerminalList::default(),
    Some(ids) => TerminalList::from_terminals(ids.iter().map(String::as_str)),
  }
}

#[cfg(test)]
pub(crate) mod tests {
  use super::*;
  use std::sync::Mutex;

  /// Records chords instead of pressing them.
  #[derive(Default)]
  pub struct FakeSink {
    pub chords: Mutex<Vec<PasteChord>>,
    pub fail: bool,
  }

  impl PasteSink for FakeSink {
    fn backend(&self) -> &'static str {
      "fake"
    }

    fn paste(&self, chord: PasteChord) -> BoxFuture<'_, Result<(), PasteError>> {
      Box::pin(async move {
        if self.fail {
          return Err(PasteError::Wayland("broken".into()));
        }
        self.chords.lock().unwrap().push(chord);
        Ok(())
      })
    }
  }

  pub const W1: &str = "0c8f9a0e-3b9c-4d4c-8a8e-2f1d7a3b5c6d";
  pub const W2: &str = "11111111-2222-3333-4444-555555555555";

  fn ap(sink: &Arc<FakeSink>, tracker: &ActiveWindowTracker) -> AutoPaste {
    AutoPaste {
      sink: Some(sink.clone() as Arc<dyn PasteSink>),
      focus: Some(tracker.clone()),
      ..AutoPaste::unavailable(&Config::default())
    }
  }

  fn target(app: &str, w: &str) -> PasteTarget {
    PasteTarget { window_id: w.into(), app_id: Some(app.into()) }
  }

  #[test]
  fn terminal_config_mapping() {
    let t = terminals(&Config::default());
    assert_eq!(t.chord_for(Some("org.kde.konsole")), PasteChord::CtrlShiftV);
    assert_eq!(t.chord_for(Some("org.kde.kate")), PasteChord::CtrlV);
    let c = Config { paste_terminals: Some(vec!["org.kde.kate".into()]), ..Config::default() };
    let t = terminals(&c);
    assert_eq!(t.chord_for(Some("org.kde.kate")), PasteChord::CtrlShiftV);
    assert_eq!(t.chord_for(Some("org.kde.konsole")), PasteChord::CtrlV);
    let t = terminals(&Config { paste_terminals: Some(vec![]), ..Config::default() });
    assert_eq!(t.chord_for(Some("kitty")), PasteChord::CtrlV);
  }

  #[tokio::test(start_paused = true)]
  async fn chord_follows_target_app() {
    let sink = Arc::new(FakeSink::default());
    let tr = ActiveWindowTracker::new();
    let a = ap(&sink, &tr);
    tr.update(Some("org.kde.konsole".into()), Some(W1.into()), Instant::now());
    assert_eq!(
      a.paste_into(&target("org.kde.konsole", W1)).await,
      SelectOutcome::Pasted { chord: PasteChord::CtrlShiftV }
    );
    tr.update(Some("org.kde.kate".into()), Some(W2.into()), Instant::now());
    assert_eq!(
      a.paste_into(&target("org.kde.kate", W2)).await,
      SelectOutcome::Pasted { chord: PasteChord::CtrlV }
    );
    let t = PasteTarget { window_id: W2.into(), app_id: None };
    assert_eq!(a.paste_into(&t).await, SelectOutcome::Pasted { chord: PasteChord::CtrlV });
    assert_eq!(
      *sink.chords.lock().unwrap(),
      vec![PasteChord::CtrlShiftV, PasteChord::CtrlV, PasteChord::CtrlV]
    );
  }

  #[tokio::test(start_paused = true)]
  async fn focus_timeout_does_not_paste() {
    let sink = Arc::new(FakeSink::default());
    let tr = ActiveWindowTracker::new();
    tr.update(Some("spool-picker".into()), Some(W2.into()), Instant::now());
    let a = ap(&sink, &tr);
    let start = tokio::time::Instant::now();
    assert_eq!(
      a.paste_into(&target("org.kde.kate", W1)).await,
      SelectOutcome::NotPasted(NoPaste::FocusTimeout)
    );
    assert_eq!(start.elapsed(), FOCUS_WAIT);
    assert!(sink.chords.lock().unwrap().is_empty());
  }

  #[tokio::test(start_paused = true)]
  async fn focus_returning_late_still_pastes() {
    let sink = Arc::new(FakeSink::default());
    let tr = ActiveWindowTracker::new();
    tr.update(Some("spool-picker".into()), Some(W2.into()), Instant::now());
    let a = ap(&sink, &tr);
    let tr2 = tr.clone();
    tokio::spawn(async move {
      tokio::time::sleep(Duration::from_millis(200)).await;
      tr2.update(Some("org.kde.kate".into()), Some(W1.into()), Instant::now());
    });
    assert_eq!(
      a.paste_into(&target("org.kde.kate", W1)).await,
      SelectOutcome::Pasted { chord: PasteChord::CtrlV }
    );
  }

  #[tokio::test]
  async fn unavailable_backends_never_paste() {
    let sink = Arc::new(FakeSink::default());
    let tr = ActiveWindowTracker::new();
    tr.update(None, Some(W1.into()), Instant::now());
    let t = target("x", W1);
    let mut a = ap(&sink, &tr);
    a.enabled = false;
    assert_eq!(a.paste_into(&t).await, SelectOutcome::NotPasted(NoPaste::Disabled));
    let mut a = ap(&sink, &tr);
    a.sink = None;
    assert_eq!(a.paste_into(&t).await, SelectOutcome::NotPasted(NoPaste::NoBackend));
    let mut a = ap(&sink, &tr);
    a.focus = None;
    assert_eq!(a.paste_into(&t).await, SelectOutcome::NotPasted(NoPaste::NoFocusSource));
    assert!(sink.chords.lock().unwrap().is_empty());
    let failing = Arc::new(FakeSink { fail: true, ..Default::default() });
    assert!(matches!(
      ap(&failing, &tr).paste_into(&t).await,
      SelectOutcome::NotPasted(NoPaste::Failed(_))
    ));
  }
}
