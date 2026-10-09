//! The resident picker: one wlr-layer-shell surface (Overlay), unmapped
//! between uses, with the next frame pre-rendered into a `wl_shm` buffer so a
//! `Show` is just margins + configure + attach + commit.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{
  ComponentHandle, Image, LogicalPosition, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer,
  SharedString, VecModel,
};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, FrameCallbackData};
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::output::{OutputHandler, OutputInfo, OutputState};
use smithay_client_toolkit::reexports::calloop::LoopHandle;
use smithay_client_toolkit::reexports::client::globals::GlobalList;
use smithay_client_toolkit::reexports::client::protocol::{
  wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface,
};
use smithay_client_toolkit::reexports::client::{Connection, Proxy, QueueHandle};
use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::{
  wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
  wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::{
  wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use smithay_client_toolkit::reexports::protocols_wlr::layer_shell::v1::client::{
  zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
  zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{
  KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
};
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_dispatch2, delegate_registry, registry_handlers};
use spool_proto::{
  CursorPos, HideReason, ItemPreview, PickerReq, PreviewKind, SelectMode, UnlockFailReason,
  UnlockPrompt, UnlockProvider, UnlockSecret, WireItemId,
};
use zeroize::Zeroizing;

use crate::ipc::{Channel, Correlator, Event};
use crate::model;
use crate::render::{Backend, FrameRenderer, Rect};
use crate::theme::{EMBEDDED_FAMILY, Theme};
use crate::ui::{PickerWindow, Row, Theme as UiTheme};

/// Logical picker size.
pub const WIDTH: u32 = 640;
pub const HEIGHT: u32 = 480;
/// Gap between the picker and the output edge / cursor.
const EDGE: i32 = 8;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(30);
const FOCUS_GRACE: Duration = Duration::from_millis(50);
/// Rows whose thumbnails survive a hide (≈ the first visible page).
const KEEP_THUMB_ROWS: usize = 12;
const MAX_THUMBS_IN_FLIGHT: usize = 8;
/// Thumbnail box in logical px (matches ui/picker.slint).
const THUMB_W: f64 = 60.0;
const THUMB_H: f64 = 38.0;

/// Actions queued by Slint callbacks, drained by the main loop (Slint
/// callbacks can't borrow the `App`).
#[derive(Debug)]
pub enum UiAction {
  Edited(String),
  Clicked(i32),
  Scrolled(i32, i32),
  UnlockRetry,
}

/// The unlock panel's state (mirrors the Slint properties).
#[derive(Debug, Default)]
struct LockUi {
  locked: bool,
  prompts: Vec<UnlockPrompt>,
  /// A secret was submitted; waiting for `Unlocked` / `UnlockFailed`.
  busy: bool,
  touch: Touch,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Touch {
  /// No touch row (no FIDO2 touch prompt, or the PIN is needed first).
  #[default]
  Off,
  /// Touch prompt offered, not started yet (starts on the next Show).
  Idle,
  /// `Unlock{Fido2}` sent; spinner.
  Waiting,
  /// The last attempt failed; Retry button.
  Failed,
}

/// User-facing text for an unlock failure (never echoes any secret).
pub fn unlock_error_text(provider: UnlockProvider, reason: UnlockFailReason) -> &'static str {
  use UnlockFailReason as R;
  use UnlockProvider as P;
  match (reason, provider) {
    (R::WrongSecret, P::Fido2) | (R::PinInvalid, _) => {
      "Wrong PIN. Try again (the key counts failed attempts)."
    }
    (R::WrongSecret, _) => "Wrong passphrase. Try again.",
    (R::PinBlocked, _) => "The security key's PIN is blocked. Re-plug or reset the key.",
    (R::Dismissed, P::Fido2) => "No touch detected.",
    (R::Dismissed, _) => "Unlock was cancelled.",
    (R::Unavailable, P::Fido2) => "No matching security key found. Plug it in and retry.",
    (R::Unavailable, P::KWallet) => "The wallet is not available.",
    (R::Unavailable, P::Passphrase) => "Unlocking is not available right now.",
    (R::Other, _) => "Unlock failed (details in the Spool log).",
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vis {
  Hidden,
  /// Show requested; waiting for the layer configure.
  AwaitConfigure,
  Shown,
}

enum ThumbState {
  Requested,
  Ready(Image),
  Failed,
}

/// The layer surface. Driven directly on the protocol objects (not sctk's
/// `LayerSurface`) because sctk acks every configure, and a configure that
/// was in flight when we unmapped must NOT be acked ("wrong configure
/// serial" protocol error).
struct Surf {
  wl: wl_surface::WlSurface,
  layer: ZwlrLayerSurfaceV1,
  viewport: Option<WpViewport>,
  frac: Option<WpFractionalScaleV1>,
  output: Option<wl_output::WlOutput>,
}

impl Drop for Surf {
  fn drop(&mut self) {
    if let Some(f) = &self.frac {
      f.destroy();
    }
    if let Some(v) = &self.viewport {
      v.destroy();
    }
    self.layer.destroy();
    self.wl.destroy();
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
  /// First Show of the process (cold start).
  Cold,
  Show,
  /// Keystroke -> list updated.
  Key,
}

/// Timing mark: an input event waiting for the frame that shows its effect.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(feature = "timing"), allow(dead_code))] // read by the `timing` feature only
pub struct Mark {
  pub kind: MarkKind,
  pub t_evt_ns: u64,
}

pub fn mono_ns() -> u64 {
  let t = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
  t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

/// Work for the thumbnail decoder thread.
pub struct ThumbJob {
  pub id: WireItemId,
  pub bytes: Vec<u8>,
  pub box_w: u32,
  pub box_h: u32,
}

pub struct ThumbDone {
  pub id: WireItemId,
  pub result: Result<image::RgbaImage, crate::thumb::ThumbError>,
}

pub struct App {
  conn: Connection,
  qh: QueueHandle<App>,
  registry_state: RegistryState,
  seat_state: SeatState,
  output_state: OutputState,
  compositor: CompositorState,
  layer_shell: ZwlrLayerShellV1,
  shm: Shm,
  pool: SlotPool,
  frac_mgr: Option<WpFractionalScaleManagerV1>,
  viewporter: Option<WpViewporter>,
  #[cfg(feature = "timing")]
  presentation: Option<smithay_client_toolkit::presentation_time::PresentationTimeState>,
  #[cfg(feature = "timing")]
  feedback_marks: HashMap<smithay_client_toolkit::reexports::client::backend::ObjectId, Vec<Mark>>,

  surf: Option<Surf>,
  bufs: [Option<Buffer>; 2],
  buf_size: (i32, i32),
  back: usize,
  last_drawn: Option<usize>,
  logical: (u32, u32),
  scale: f64,
  /// Last preferred scale per output name (for pre-rendering while hidden).
  scale_by_output: HashMap<String, f64>,
  current_output: Option<String>,
  vis: Vis,
  frame_pending: bool,
  retry_soon: bool,
  ever_shown: bool,
  /// Marks waiting for the next presented frame.
  pending_marks: Vec<Mark>,
  key_mark: Option<u64>,
  query_mark: Option<u64>,

  keyboard: Option<wl_keyboard::WlKeyboard>,
  pointer: Option<wl_pointer::WlPointer>,
  mods: Modifiers,
  /// Enter pressed on the list: `(raw keycode, mode)`. The item is selected
  /// (and the picker hidden) on that key's **release**, so the window that
  /// gets focus back never receives a stray Return release.
  enter_down: Option<(u32, SelectMode)>,
  loop_handle: LoopHandle<'static, App>,

  pub ui: PickerWindow,
  renderer: Backend,
  rows: Rc<VecModel<Row>>,
  entries: Vec<ItemPreview>,
  thumbs: HashMap<WireItemId, ThumbState>,
  thumb_jobs: std::sync::mpsc::Sender<ThumbJob>,
  pub actions: Rc<RefCell<Vec<UiAction>>>,
  chan: Channel,
  adapter: Correlator,
  lock: LockUi,
  /// An answer (page or error) to the first query arrived.
  first_answer: bool,
  ready_sent: bool,
  /// Focus the secret field after the next render (a field that just
  /// appeared only exists once Slint has laid it out).
  focus_secret_after_render: bool,
  /// GPU: one offscreen frame was drawn (shaders, glyph atlas and text
  /// layout warmed up) before `Ready`.
  #[cfg(feature = "gpu")]
  gpu_warmed: bool,
  query_text: String,
  search_deadline: Option<Instant>,
  /// Keyboard focus left the shown picker at this time (hide after
  /// `FOCUS_GRACE` unless it comes back).
  focus_lost_at: Option<Instant>,
  has_more: bool,
  visible: (usize, usize),
  index_progress: Option<(u64, u64)>,
  theme: Theme,
  /// Keep a rendered frame ready while hidden (`SPOOL_PICKER_PRERENDER=0`
  /// disables it: buffers are freed on hide and the first frame is rendered
  /// after Show).
  prerender: bool,
  pub exit: Option<String>,
}

#[allow(clippy::too_many_arguments)]
impl App {
  pub fn new(
    conn: Connection,
    globals: &GlobalList,
    qh: QueueHandle<App>,
    loop_handle: LoopHandle<'static, App>,
    ui: PickerWindow,
    renderer: Backend,
    chan: Channel,
    thumb_jobs: std::sync::mpsc::Sender<ThumbJob>,
  ) -> anyhow::Result<App> {
    let compositor =
      CompositorState::bind(globals, &qh).map_err(|e| anyhow::anyhow!("wl_compositor: {e}"))?;
    let layer_shell = globals
      .bind::<ZwlrLayerShellV1, App, NoEvents>(&qh, 1..=4, NoEvents)
      .map_err(|e| anyhow::anyhow!("zwlr_layer_shell_v1 not available: {e}"))?;
    let shm = Shm::bind(globals, &qh).map_err(|e| anyhow::anyhow!("wl_shm: {e}"))?;
    let pool = SlotPool::new((WIDTH * HEIGHT * 4 * 2) as usize, &shm)?;
    let frac_mgr =
      globals.bind::<WpFractionalScaleManagerV1, App, NoEvents>(&qh, 1..=1, NoEvents).ok();
    let viewporter = globals.bind::<WpViewporter, App, NoEvents>(&qh, 1..=1, NoEvents).ok();
    #[cfg(feature = "timing")]
    let presentation =
      Some(smithay_client_toolkit::presentation_time::PresentationTimeState::bind(globals, &qh));

    let renderer_is_gpu = renderer.is_gpu();
    let rows = Rc::new(VecModel::<Row>::default());
    ui.set_rows(ModelRc::from(rows.clone()));
    let actions: Rc<RefCell<Vec<UiAction>>> = Rc::default();
    {
      let a = actions.clone();
      ui.on_edited(move |t| a.borrow_mut().push(UiAction::Edited(t.to_string())));
      let a = actions.clone();
      ui.on_clicked(move |i| a.borrow_mut().push(UiAction::Clicked(i)));
      let a = actions.clone();
      ui.on_scrolled(move |f, l| a.borrow_mut().push(UiAction::Scrolled(f, l)));
      let a = actions.clone();
      ui.on_unlock_retry(move || a.borrow_mut().push(UiAction::UnlockRetry));
    }

    let mut app = App {
      registry_state: RegistryState::new(globals),
      seat_state: SeatState::new(globals, &qh),
      output_state: OutputState::new(globals, &qh),
      conn,
      qh,
      compositor,
      layer_shell,
      shm,
      pool,
      frac_mgr,
      viewporter,
      #[cfg(feature = "timing")]
      presentation,
      #[cfg(feature = "timing")]
      feedback_marks: HashMap::new(),
      surf: None,
      bufs: [None, None],
      buf_size: (0, 0),
      back: 0,
      last_drawn: None,
      logical: (WIDTH, HEIGHT),
      scale: 1.0,
      scale_by_output: HashMap::new(),
      current_output: None,
      vis: Vis::Hidden,
      frame_pending: false,
      retry_soon: false,
      ever_shown: false,
      pending_marks: Vec::new(),
      key_mark: None,
      enter_down: None,
      query_mark: None,
      keyboard: None,
      pointer: None,
      mods: Modifiers::default(),
      loop_handle,
      ui,
      renderer,
      rows,
      entries: Vec::new(),
      thumbs: HashMap::new(),
      thumb_jobs,
      actions,
      chan,
      adapter: Correlator::default(),
      lock: LockUi::default(),
      first_answer: false,
      ready_sent: false,
      focus_secret_after_render: false,
      #[cfg(feature = "gpu")]
      gpu_warmed: false,
      query_text: String::new(),
      search_deadline: None,
      focus_lost_at: None,
      has_more: false,
      visible: (0, KEEP_THUMB_ROWS),
      index_progress: None,
      theme: Theme::load(),
      // The GPU path cannot draw without mapping (see gpu.rs).
      prerender: !renderer_is_gpu
        && !matches!(
          std::env::var("SPOOL_PICKER_PRERENDER").as_deref(),
          Ok("0" | "false" | "no" | "off")
        ),
      exit: None,
    };
    app.apply_theme();
    app.apply_scale(1.0);
    app.ui.window().dispatch_event(WindowEvent::WindowActiveChanged(false));
    // Focus (and the blinking cursor) only while shown; see on_configure.
    app.ui.invoke_blur_search();
    let req = app.adapter.new_query("");
    app.chan.send(req);
    app.update_status();
    Ok(app)
  }

  /// After the initial roundtrips: create the (unmapped) layer surface on the
  /// first output and pre-render for its scale.
  pub fn init_surface(&mut self) {
    let first = self.output_state.outputs().next();
    let name = first.as_ref().and_then(|o| self.output_state.info(o)).and_then(|i| i.name.clone());
    let scale = first
      .as_ref()
      .and_then(|o| self.output_state.info(o))
      .map(|i| i.scale_factor as f64)
      .unwrap_or(1.0);
    self.create_surface(first);
    self.current_output = name;
    self.apply_scale(scale);
  }

  /// Drop the layer surface (GPU: the EGL surface goes first).
  fn drop_surface(&mut self) {
    #[cfg(feature = "gpu")]
    if let Backend::Gpu(g) = &self.renderer {
      g.egl.detach();
    }
    self.surf = None;
  }

  fn create_surface(&mut self, output: Option<wl_output::WlOutput>) {
    let wl = self.compositor.create_surface(&self.qh);
    let viewport = self.viewporter.as_ref().map(|v| v.get_viewport(&wl, &self.qh, NoEvents));
    let frac = self.frac_mgr.as_ref().map(|m| m.get_fractional_scale(&wl, &self.qh, FracData));
    let layer = self.layer_shell.get_layer_surface(
      &wl,
      output.as_ref(),
      zwlr_layer_shell_v1::Layer::Overlay,
      "spool-picker".into(),
      &self.qh,
      LayerData,
    );
    layer.set_anchor(Anchor::Top | Anchor::Left);
    layer.set_size(self.logical.0, self.logical.1);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    // No commit: the surface stays unmapped until the first Show.
    self.surf = Some(Surf { wl, layer, viewport, frac, output });
  }

  // ---------------------------------------------------------------- events

  pub fn on_daemon_event(&mut self, evt: spool_proto::PickerEvt) {
    match self.adapter.on_event(evt) {
      Event::Show { cursor, output, scale_hint } => self.show(cursor, output, scale_hint),
      Event::Hide => self.hide(HideReason::Requested),
      Event::Page { offset, items, more } => self.on_page(offset, items, more),
      Event::Thumb { id, bytes } => self.on_thumb_bytes(id, bytes),
      Event::ThumbFailed { id } => {
        if let Some(st) = self.thumbs.get_mut(&id) {
          *st = ThumbState::Failed;
          if let Some(i) = self.entries.iter().position(|e| e.id == id) {
            self.refresh_row(i);
          }
        }
        self.request_visible_thumbs();
      }
      Event::QueryFailed { code, message } => {
        // Keep the previous list; say why the new one did not come.
        tracing::debug!(?code, "query failed");
        self.first_answer = true;
        self.query_mark = None;
        self
          .set_notice(&format!("Search failed: {}", crate::sanitize::sanitize_line(&message, 120)));
      }
      Event::Notice { code, message } => {
        tracing::debug!(?code, "daemon notice");
        self.set_notice(&crate::sanitize::sanitize_line(&message, 160));
      }
      Event::NewItem(p) => self.on_new_item(p),
      Event::Locked(prompts) => self.on_locked(prompts),
      Event::Unlocked => self.on_unlocked(),
      Event::UnlockFailed { provider, reason } => self.on_unlock_failed(provider, reason),
      Event::IndexProgress { done, total } => {
        self.index_progress = if done >= total { None } else { Some((done, total)) };
        self.update_status();
      }
      Event::Stale => {}
    }
  }

  fn set_notice(&mut self, text: &str) {
    if self.ui.get_notice() != text {
      self.ui.set_notice(text.into());
    }
  }

  // ------------------------------------------------------------ unlock

  fn on_locked(&mut self, prompts: Vec<UnlockPrompt>) {
    let first = !self.lock.locked;
    self.lock.locked = true;
    self.lock.busy = false;
    let has = |p| prompts.contains(&p);
    let kind = if has(UnlockPrompt::Fido2Pin) {
      2
    } else if has(UnlockPrompt::Passphrase) {
      1
    } else {
      0
    };
    // A PIN prompt means the touch comes after the PIN.
    let touch_wanted = has(UnlockPrompt::Fido2Touch) && !has(UnlockPrompt::Fido2Pin);
    if !touch_wanted {
      self.lock.touch = Touch::Off;
    } else if self.lock.touch == Touch::Off {
      self.lock.touch = Touch::Idle;
    }
    let ui = &self.ui;
    ui.set_secret_busy(false);
    ui.set_secret_kind(kind);
    if kind == 0 {
      ui.set_secret_focused(false);
    }
    ui.set_unlock_error(if has(UnlockPrompt::Fido2Pin) {
      "Your security key needs its PIN.".into()
    } else {
      "".into()
    });
    ui.set_locked(true);
    self.lock.prompts = prompts;
    self.sync_touch_ui();
    if self.vis != Vis::Hidden {
      self.start_touch(false);
      self.focus_input();
    }
    if first {
      // Only session items are readable now.
      let req = self.adapter.new_query(&self.query_text.clone());
      self.chan.send(req);
    }
  }

  fn on_unlocked(&mut self) {
    self.lock = LockUi::default();
    let ui = &self.ui;
    ui.set_locked(false);
    ui.set_secret_kind(0);
    ui.set_secret_busy(false);
    ui.set_secret("".into());
    ui.set_secret_focused(false);
    ui.set_unlock_error("".into());
    self.sync_touch_ui();
    if self.vis != Vis::Hidden {
      self.ui.invoke_focus_search();
    }
    // The store just became readable: refresh.
    let req = self.adapter.new_query(&self.query_text.clone());
    self.chan.send(req);
  }

  fn on_unlock_failed(&mut self, provider: UnlockProvider, reason: UnlockFailReason) {
    if !self.lock.locked {
      return;
    }
    self.lock.busy = false;
    self.ui.set_secret_busy(false);
    self.ui.set_unlock_error(unlock_error_text(provider, reason).into());
    if provider == UnlockProvider::Fido2 {
      self.lock.touch = match reason {
        UnlockFailReason::Dismissed | UnlockFailReason::Unavailable | UnlockFailReason::Other
          if self.lock.prompts.contains(&UnlockPrompt::Fido2Touch) =>
        {
          Touch::Failed
        }
        _ => Touch::Off,
      };
      if reason == UnlockFailReason::PinBlocked && self.ui.get_secret_kind() == 2 {
        // No point asking for the PIN again; keep a passphrase field if any.
        let pass = self.lock.prompts.contains(&UnlockPrompt::Passphrase);
        self.ui.set_secret_kind(if pass { 1 } else { 0 });
        if !pass {
          self.ui.set_secret_focused(false);
        }
      }
    }
    self.sync_touch_ui();
    if self.vis != Vis::Hidden {
      self.focus_input();
    }
  }

  fn bump_secret_focus(&self) {
    self.ui.set_secret_focus_req(self.ui.get_secret_focus_req().wrapping_add(1));
  }

  fn sync_touch_ui(&self) {
    let (state, text) = match self.lock.touch {
      Touch::Off => (0, ""),
      Touch::Idle => (2, "Security key"),
      Touch::Waiting => (1, "Touch your security key"),
      Touch::Failed => (2, "Security key"),
    };
    self.ui.set_touch_state(state);
    self.ui.set_touch_text(text.into());
  }

  /// Start waiting for a FIDO2 touch (`Unlock{Fido2, None}`; the daemon
  /// ignores duplicates): on Show, when the prompt appears while shown, and
  /// on Retry. `after_failure`: also restart a failed attempt (Show, Retry);
  /// a `Locked` update while shown does not.
  fn start_touch(&mut self, after_failure: bool) {
    let ok = match self.lock.touch {
      Touch::Idle => true,
      Touch::Failed => after_failure,
      Touch::Off | Touch::Waiting => false,
    };
    if !self.lock.locked || !ok {
      return;
    }
    self.lock.touch = Touch::Waiting;
    self.ui.set_unlock_error("".into());
    self.sync_touch_ui();
    self.chan.send(PickerReq::Unlock { provider: UnlockProvider::Fido2, secret: None });
  }

  fn retry_touch(&mut self) {
    self.start_touch(true);
  }

  /// Take the typed passphrase/PIN out of the UI and send it.
  fn submit_secret(&mut self) {
    if !self.lock.locked || self.lock.busy {
      return;
    }
    let kind = self.ui.get_secret_kind();
    // The SharedString is Slint's (not wipeable); our copy is zeroized on
    // drop and the property is cleared right away.
    let typed = self.ui.get_secret();
    self.ui.set_secret("".into());
    if typed.is_empty() || kind == 0 {
      return;
    }
    let secret = Zeroizing::new(String::from(typed.as_str()));
    drop(typed);
    let provider = if kind == 2 { UnlockProvider::Fido2 } else { UnlockProvider::Passphrase };
    self.lock.busy = true;
    self.ui.set_secret_busy(true);
    self.ui.set_unlock_error("".into());
    if provider == UnlockProvider::Fido2 {
      self.lock.touch = Touch::Waiting;
      self.sync_touch_ui();
    }
    self.chan.send(PickerReq::Unlock { provider, secret: Some(UnlockSecret::new(secret)) });
  }

  /// Keyboard focus for a fresh Show / panel change: the secret field when
  /// there is one, else the search box.
  fn focus_input(&mut self) {
    if self.lock.locked && self.ui.get_secret_kind() != 0 {
      self.bump_secret_focus();
      self.focus_secret_after_render = true;
    } else {
      self.ui.invoke_focus_search();
    }
  }

  pub fn on_thumb_done(&mut self, done: ThumbDone) {
    let Some(state) = self.thumbs.get_mut(&done.id) else { return };
    match done.result {
      Ok(img) => {
        let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
          img.as_raw(),
          img.width(),
          img.height(),
        );
        *state = ThumbState::Ready(Image::from_rgba8(buf));
      }
      Err(e) => {
        tracing::debug!(id = done.id, error = %e, "thumbnail rejected");
        *state = ThumbState::Failed;
      }
    }
    if let Some(i) = self.entries.iter().position(|e| e.id == done.id) {
      self.refresh_row(i);
    }
  }

  pub fn drain_ui_actions(&mut self) {
    let actions: Vec<UiAction> = std::mem::take(&mut *self.actions.borrow_mut());
    for a in actions {
      match a {
        UiAction::Edited(t) => {
          self.query_text = t;
          self.search_deadline = Some(Instant::now() + SEARCH_DEBOUNCE);
        }
        UiAction::Clicked(i) => {
          if i >= 0 {
            self.set_current(i as usize);
            self.select(SelectMode::Paste);
          }
        }
        UiAction::Scrolled(first, last) => {
          self.visible = (first.max(0) as usize, last.max(0) as usize);
          self.request_visible_thumbs();
          self.maybe_load_more();
        }
        UiAction::UnlockRetry => self.retry_touch(),
      }
    }
  }

  /// Time until the next internal deadline, if any.
  pub fn next_deadline(&self) -> Option<Duration> {
    let now = Instant::now();
    let mut d = [self.search_deadline, self.focus_lost_at.map(|t| t + FOCUS_GRACE)]
      .into_iter()
      .flatten()
      .map(|t| t.saturating_duration_since(now))
      .min();
    if self.retry_soon {
      d = Some(d.map_or(Duration::from_millis(2), |x| x.min(Duration::from_millis(2))));
    }
    d
  }

  pub fn process_deadlines(&mut self) {
    if let Some(t) = self.focus_lost_at
      && t.elapsed() >= FOCUS_GRACE
    {
      self.focus_lost_at = None;
      if self.vis == Vis::Shown {
        self.hide(HideReason::FocusLost);
      }
    }
    if let Some(t) = self.search_deadline
      && Instant::now() >= t
    {
      self.search_deadline = None;
      let req = self.adapter.new_query(&self.query_text.clone());
      self.query_mark = self.key_mark.take();
      self.chan.send(req);
    }
  }

  // ------------------------------------------------------------ show/hide

  fn show(&mut self, cursor: Option<CursorPos>, output: Option<String>, hint: Option<f64>) {
    let t0 = mono_ns();
    if self.vis != Vis::Hidden {
      return;
    }
    let target = self.pick_output(cursor, output.as_deref());
    let target_out = target.as_ref().map(|(o, _)| o.clone());
    let need_new = match (&self.surf, &target_out) {
      (None, _) => true,
      (Some(s), Some(o)) => s.output.as_ref() != Some(o),
      (Some(_), None) => false,
    };
    if need_new {
      // A layer surface is bound to one output for life: switching monitors
      // is the one case where we recreate it.
      self.drop_surface();
      self.create_surface(target_out.clone());
    }
    let info = target.as_ref().map(|(_, i)| i.clone());
    let name = info.as_ref().and_then(|i| i.name.clone());
    if name.is_some() && name != self.current_output {
      self.current_output = name.clone();
    }
    // Scale guess: last preferred scale seen on this output, else its
    // integer wl_output scale. The compositor corrects it after mapping.
    let hint = hint.filter(|h| h.is_finite() && (0.5..=8.0).contains(h));
    let guess = hint
      .or_else(|| name.as_ref().and_then(|n| self.scale_by_output.get(n).copied()))
      .or_else(|| info.as_ref().map(|i| i.scale_factor as f64))
      .unwrap_or(self.scale);
    if (guess - self.scale).abs() > 1e-6 {
      self.apply_scale(guess);
    }

    let (w, h) = (self.logical.0 as i32, self.logical.1 as i32);
    let Some(s) = &self.surf else { return };
    match (info.as_ref().and_then(|i| i.logical_position.zip(i.logical_size)), cursor) {
      (Some(((ox, oy), (ow, oh))), c) => {
        let (left, top) = place(c.map(|c| (c.x - ox, c.y - oy)), (ow, oh), (w, h));
        s.layer.set_anchor(Anchor::Top | Anchor::Left);
        s.layer.set_margin(top, 0, 0, left);
      }
      _ => {
        // Output geometry unknown: let the compositor center us.
        s.layer.set_anchor(Anchor::empty());
        s.layer.set_margin(0, 0, 0, 0);
      }
    }
    s.layer.set_size(self.logical.0, self.logical.1);
    s.layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
    s.wl.commit();
    let _ = self.conn.flush();
    self.vis = Vis::AwaitConfigure;
    self.focus_lost_at = None;
    self.ui.set_shown(true);
    self.start_touch(true);
    let kind = if self.ever_shown { MarkKind::Show } else { MarkKind::Cold };
    self.ever_shown = true;
    self.pending_marks.push(Mark { kind, t_evt_ns: t0 });
  }

  fn pick_output(
    &self,
    cursor: Option<CursorPos>,
    name: Option<&str>,
  ) -> Option<(wl_output::WlOutput, OutputInfo)> {
    let all: Vec<_> = self
      .output_state
      .outputs()
      .filter_map(|o| self.output_state.info(&o).map(|i| (o, i)))
      .collect();
    if let Some(n) = name
      && let Some(hit) = all.iter().find(|(_, i)| i.name.as_deref() == Some(n))
    {
      return Some(hit.clone());
    }
    if let Some(c) = cursor
      && let Some(hit) = all.iter().find(|(_, i)| match (i.logical_position, i.logical_size) {
        (Some((x, y)), Some((w, h))) => c.x >= x && c.x < x + w && c.y >= y && c.y < y + h,
        _ => false,
      })
    {
      return Some(hit.clone());
    }
    // Keep the current surface's output, else the first.
    if let Some(cur) = self.surf.as_ref().and_then(|s| s.output.clone())
      && let Some(hit) = all.iter().find(|(o, _)| *o == cur)
    {
      return Some(hit.clone());
    }
    all.into_iter().next()
  }

  /// Unmap and reset for the next use (search cleared, top selected), then
  /// pre-render that state.
  pub fn hide(&mut self, reason: HideReason) {
    self.enter_down = None;
    if let Some(s) = &self.surf {
      // Only the null buffer: any other state change in this commit makes
      // the compositor send a configure that arrives after the unmap reset
      // it, and acking that serial is a protocol error ("wrong configure
      // serial"). Keyboard interactivity is set again on every Show.
      s.wl.attach(None, 0, 0);
      s.wl.commit();
      let _ = self.conn.flush();
    }
    let was_visible = self.vis != Vis::Hidden;
    self.vis = Vis::Hidden;
    self.frame_pending = false;
    self.focus_lost_at = None;
    self.pending_marks.clear();
    if !was_visible {
      return;
    }
    self.chan.send(PickerReq::Hidden { reason });
    self.ui.set_shown(false);
    self.ui.set_notice("".into());
    // Never keep a half-typed secret across uses.
    self.ui.set_secret("".into());
    self.release_modifiers();
    self.ui.window().dispatch_event(WindowEvent::WindowActiveChanged(false));
    // Move focus off the secret field (if any) too, then drop it.
    self.ui.invoke_focus_search();
    self.ui.invoke_blur_search();
    self.ui.set_secret_focused(false);
    self.search_deadline = None;
    if !self.query_text.is_empty() {
      self.query_text.clear();
      self.ui.invoke_clear_search();
      let req = self.adapter.new_query("");
      self.chan.send(req);
    }
    self.ui.set_current(0);
    self.ui.invoke_scroll_top();
    self.visible = (0, KEEP_THUMB_ROWS);
    self.drop_offscreen_thumbs();
    self.refresh_times();
    if !self.prerender {
      self.bufs = [None, None];
      self.last_drawn = None;
    }
    self.renderer.request_redraw();
  }

  fn drop_offscreen_thumbs(&mut self) {
    let keep: std::collections::HashSet<WireItemId> =
      self.entries.iter().take(KEEP_THUMB_ROWS).map(|e| e.id).collect();
    let drop: Vec<WireItemId> = self
      .thumbs
      .iter()
      .filter(|(id, st)| !keep.contains(id) && !matches!(st, ThumbState::Requested))
      .map(|(id, _)| *id)
      .collect();
    for id in drop {
      self.thumbs.remove(&id);
      if let Some(i) = self.entries.iter().position(|e| e.id == id) {
        self.refresh_row(i);
      }
    }
  }

  // ------------------------------------------------------------- list

  fn on_page(&mut self, offset: u32, items: Vec<ItemPreview>, more: bool) {
    self.has_more = more;
    self.first_answer = true;
    if offset == 0 {
      self.set_notice("");
      self.entries = items;
      let rows: Vec<Row> = (0..self.entries.len()).map(|i| self.make_row(i)).collect();
      self.rows.set_vec(rows);
      self.ui.set_current(0);
      self.ui.invoke_scroll_top();
      self.visible = (0, KEEP_THUMB_ROWS);
    } else if offset as usize == self.entries.len() {
      let start = self.entries.len();
      self.entries.extend(items);
      for i in start..self.entries.len() {
        let r = self.make_row(i);
        self.rows.push(r);
      }
    } else {
      return;
    }
    if let Some(t_key) = self.query_mark.take() {
      self.pending_marks.push(Mark { kind: MarkKind::Key, t_evt_ns: t_key });
    }
    self.request_visible_thumbs();
    self.update_status();
  }

  fn on_new_item(&mut self, p: ItemPreview) {
    if !self.query_text.is_empty() {
      return;
    }
    if let Some(i) = self.entries.iter().position(|e| e.id == p.id) {
      self.entries.remove(i);
      self.rows.remove(i);
    }
    self.entries.insert(0, p);
    let r = self.make_row(0);
    self.rows.insert(0, r);
    if self.vis == Vis::Shown {
      // Keep the selected item selected.
      let cur = self.ui.get_current();
      if cur > 0 || self.entries.len() > 1 {
        self.ui.set_current((cur + 1).min(self.entries.len() as i32 - 1));
      }
    } else {
      self.ui.set_current(0);
    }
    self.request_visible_thumbs();
    self.update_status();
  }

  fn make_row(&self, i: usize) -> Row {
    let p = &self.entries[i];
    let is_image = p.kind == PreviewKind::Image;
    let (thumb, has_thumb, failed) = match self.thumbs.get(&p.id) {
      Some(ThumbState::Ready(img)) => (img.clone(), true, false),
      Some(ThumbState::Failed) => (Image::default(), false, true),
      _ => (Image::default(), false, is_image && model::image_mime(p).is_none()),
    };
    Row {
      preview: SharedString::from(model::preview_text(p)),
      badge: model::badge(p).into(),
      time: model::relative_time(model::now_ms(), p.last_used_unix_ms.max(p.created_unix_ms))
        .into(),
      pinned: p.pinned,
      is_image,
      thumb,
      has_thumb,
      thumb_failed: failed,
    }
  }

  fn refresh_row(&mut self, i: usize) {
    if i < self.entries.len() {
      let r = self.make_row(i);
      self.rows.set_row_data(i, r);
    }
  }

  fn refresh_times(&mut self) {
    let now = model::now_ms();
    for i in 0..self.entries.len() {
      let p = &self.entries[i];
      let t: SharedString =
        model::relative_time(now, p.last_used_unix_ms.max(p.created_unix_ms)).into();
      if let Some(mut r) = self.rows.row_data(i)
        && r.time != t
      {
        r.time = t;
        self.rows.set_row_data(i, r);
      }
    }
  }

  fn update_status(&mut self) {
    let n = self.entries.len();
    let mut s = match (n, self.has_more) {
      (_, true) => format!("{n}+ items"),
      (1, false) => "1 item".to_string(),
      _ => format!("{n} items"),
    };
    if let Some((d, t)) = self.index_progress {
      s.push_str(&format!(" · indexing {d}/{t}"));
    }
    self.ui.set_status(s.into());
  }

  fn request_visible_thumbs(&mut self) {
    let (first, last) = self.visible;
    let last = last.min(self.entries.len());
    let mut jobs = Vec::new();
    for i in first.min(last)..last {
      if self.adapter.thumbs_in_flight() + jobs.len() >= MAX_THUMBS_IN_FLIGHT {
        break;
      }
      let p = &self.entries[i];
      if p.kind != PreviewKind::Image || self.thumbs.contains_key(&p.id) {
        continue;
      }
      if let Some(m) = model::image_mime(p) {
        jobs.push((p.id, m));
      }
    }
    for (id, m) in jobs {
      self.thumbs.insert(id, ThumbState::Requested);
      let req = self.adapter.thumb(id, m);
      self.chan.send(req);
    }
  }

  fn on_thumb_bytes(&mut self, id: WireItemId, bytes: Vec<u8>) {
    if !self.thumbs.contains_key(&id) {
      return;
    }
    let box_w = (THUMB_W * self.scale).ceil() as u32;
    let box_h = (THUMB_H * self.scale).ceil() as u32;
    let _ = self.thumb_jobs.send(ThumbJob { id, bytes, box_w, box_h });
    // More visible rows may be waiting for a free slot.
    self.request_visible_thumbs();
  }

  fn maybe_load_more(&mut self) {
    let len = self.entries.len();
    if self.has_more && !self.adapter.query_in_flight() && self.visible.1 + 10 >= len {
      let req = self.adapter.more(len as u32);
      self.chan.send(req);
    }
  }

  fn set_current(&mut self, i: usize) {
    if self.entries.is_empty() {
      return;
    }
    let i = i.min(self.entries.len() - 1);
    self.ui.set_current(i as i32);
    self.ui.invoke_ensure_visible();
    self.drain_ui_actions_scroll_only();
  }

  fn drain_ui_actions_scroll_only(&mut self) {
    // ensure-visible reports the visible range synchronously; handle it now
    // so thumbnails/paging follow keyboard navigation.
    let pending: Vec<UiAction> = std::mem::take(&mut *self.actions.borrow_mut());
    let mut rest = Vec::new();
    for a in pending {
      match a {
        UiAction::Scrolled(f, l) => {
          self.visible = (f.max(0) as usize, l.max(0) as usize);
        }
        other => rest.push(other),
      }
    }
    self.actions.borrow_mut().extend(rest);
    self.request_visible_thumbs();
    self.maybe_load_more();
  }

  fn current_entry(&self) -> Option<(usize, &ItemPreview)> {
    let i = self.ui.get_current();
    if i < 0 {
      return None;
    }
    self.entries.get(i as usize).map(|e| (i as usize, e))
  }

  fn select(&mut self, mode: SelectMode) {
    if let Some((_, e)) = self.current_entry() {
      let id = e.id;
      self.hide(HideReason::Selected);
      self.chan.send(PickerReq::Select { id, mode });
    }
  }

  fn toggle_pin(&mut self) {
    if let Some((i, e)) = self.current_entry() {
      let (id, on) = (e.id, !e.pinned);
      self.entries[i].pinned = on;
      self.refresh_row(i);
      self.chan.send(PickerReq::Pin { id, on });
    }
  }

  fn delete_current(&mut self) {
    if let Some((i, e)) = self.current_entry() {
      let id = e.id;
      self.entries.remove(i);
      self.rows.remove(i);
      self.thumbs.remove(&id);
      self.chan.send(PickerReq::Delete { id });
      if !self.entries.is_empty() {
        self.set_current(i.min(self.entries.len() - 1));
      }
      self.update_status();
    }
  }

  // ------------------------------------------------------------- keys

  fn on_key(&mut self, ev: KeyEvent, repeat: bool) {
    if self.vis != Vis::Shown && self.vis != Vis::AwaitConfigure {
      return;
    }
    let page = ((self.logical.1 as f32 - 90.0) / 46.0).floor().max(1.0) as i64;
    let cur = self.ui.get_current() as i64;
    let len = self.entries.len() as i64;
    let mut nav = |delta: i64| {
      if len > 0 {
        self.set_current((cur + delta).clamp(0, len - 1) as usize);
      }
    };
    match ev.keysym {
      Keysym::Escape => return self.hide(HideReason::Esc),
      Keysym::Return | Keysym::KP_Enter if !repeat => {
        if self.ui.get_secret_focused() && self.ui.get_secret_kind() != 0 {
          // Stays visible: the release is ours anyway.
          return self.submit_secret();
        }
        // Select on release (see `enter_down`); modifiers count as pressed.
        self.enter_down = Some((ev.raw_code, select_mode(&self.mods)));
        return;
      }
      Keysym::Up | Keysym::KP_Up => return nav(-1),
      Keysym::Down | Keysym::KP_Down => return nav(1),
      Keysym::Page_Up | Keysym::KP_Page_Up => return nav(-page),
      Keysym::Page_Down | Keysym::KP_Page_Down => return nav(page),
      Keysym::Delete | Keysym::KP_Delete if !repeat => return self.delete_current(),
      Keysym::p | Keysym::P if self.mods.ctrl && !repeat => return self.toggle_pin(),
      Keysym::r | Keysym::R if self.mods.ctrl && !repeat && self.lock.locked => {
        return self.retry_touch();
      }
      _ => {}
    }
    if let Some(text) = slint_text(ev.keysym, ev.utf8.as_deref(), &self.mods) {
      let is_edit = !is_modifier(ev.keysym);
      if is_edit {
        self.key_mark = Some(mono_ns());
      }
      let event = if repeat {
        WindowEvent::KeyPressRepeated { text: text.clone() }
      } else {
        WindowEvent::KeyPressed { text: text.clone() }
      };
      self.ui.window().dispatch_event(event);
      if repeat {
        self.ui.window().dispatch_event(WindowEvent::KeyReleased { text });
      }
    }
  }

  fn on_key_release(&mut self, ev: KeyEvent) {
    if let Some((code, mode)) = self.enter_down
      && code == ev.raw_code
    {
      self.enter_down = None;
      if self.vis == Vis::Shown || self.vis == Vis::AwaitConfigure {
        self.select(mode);
      }
      return;
    }
    if let Some(text) = slint_text(ev.keysym, None, &self.mods) {
      self.ui.window().dispatch_event(WindowEvent::KeyReleased { text });
    }
  }

  fn release_modifiers(&mut self) {
    for k in [Key::Shift, Key::Control, Key::Alt, Key::Meta] {
      self.ui.window().dispatch_event(WindowEvent::KeyReleased { text: k.into() });
    }
  }

  // ------------------------------------------------------------ theme

  pub fn reload_theme(&mut self) {
    let t = Theme::load();
    if t != self.theme {
      self.theme = t;
      self.apply_theme();
    }
  }

  fn apply_theme(&mut self) {
    let c = |r: crate::theme::Rgb| slint::Color::from_rgb_u8(r.0, r.1, r.2);
    let t = &self.theme;
    let g = self.ui.global::<UiTheme>();
    g.set_bg(c(t.bg));
    g.set_fg(c(t.fg));
    g.set_dim(c(t.dim));
    g.set_sel_bg(c(t.sel_bg));
    g.set_sel_fg(c(t.sel_fg));
    g.set_accent(c(t.accent));
    g.set_surface(c(t.bg.mix(t.fg, 0.07)));
    g.set_border(c(t.bg.mix(t.fg, 0.18)));
    g.set_font_family(t.font_family.as_deref().unwrap_or(EMBEDDED_FAMILY).into());
    g.set_font_size(t.font_px);
  }

  // ------------------------------------------------------------ scale

  fn apply_scale(&mut self, scale: f64) {
    tracing::debug!(scale, viewporter = self.viewporter.is_some(), "apply scale");
    self.scale = scale;
    let w = self.ui.window();
    w.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: scale as f32 });
    let (pw, ph) = self.phys_size();
    w.set_size(slint::PhysicalSize::new(pw as u32, ph as u32));
    self.renderer.invalidate();
  }

  fn phys_size(&self) -> (i32, i32) {
    let s = if self.viewporter.is_some() { self.scale } else { self.scale.round().max(1.0) };
    ((self.logical.0 as f64 * s).round() as i32, (self.logical.1 as f64 * s).round() as i32)
  }

  fn on_preferred_scale(&mut self, scale: f64) {
    tracing::debug!(scale, output = ?self.current_output, "preferred scale");
    if let Some(n) = &self.current_output {
      self.scale_by_output.insert(n.clone(), scale);
    }
    if (scale - self.scale).abs() > 1e-6 {
      tracing::debug!(scale, "preferred scale changed");
      self.apply_scale(scale);
    }
  }

  // ------------------------------------------------------------ render

  fn ensure_buffers(&mut self) -> anyhow::Result<()> {
    let (w, h) = self.phys_size();
    if self.buf_size == (w, h) && self.bufs.iter().all(Option::is_some) {
      return Ok(());
    }
    self.bufs = [None, None];
    self.last_drawn = None;
    for b in &mut self.bufs {
      let (buf, _) = self.pool.create_buffer(w, h, w * 4, wl_shm::Format::Argb8888)?;
      *b = Some(buf);
    }
    self.buf_size = (w, h);
    self.back = 0;
    self.renderer.invalidate();
    Ok(())
  }

  /// Render into the back buffer if Slint wants a redraw.
  fn render_back(&mut self) -> Render {
    if self.ensure_buffers().is_err() {
      return Render::Busy;
    }
    let idx = self.back;
    let width = self.buf_size.0 as usize;
    let Some(buf) = self.bufs[idx].as_ref() else { return Render::Busy };
    let Some(canvas) = buf.canvas(&mut self.pool) else { return Render::Busy };
    let t = Instant::now();
    #[allow(irrefutable_let_patterns)] // refutable with the `gpu` feature
    let Backend::Software(sw) = &mut self.renderer else { return Render::Busy };
    match sw.render(canvas, width) {
      Some(damage) => {
        tracing::debug!(
          us = t.elapsed().as_micros() as u64,
          rects = damage.len(),
          hidden = (self.vis == Vis::Hidden),
          "rendered"
        );
        self.back ^= 1;
        self.last_drawn = Some(idx);
        Render::Drew(idx, damage)
      }
      None => Render::Clean,
    }
  }

  pub fn maybe_render(&mut self) {
    self.retry_soon = false;
    #[cfg(feature = "gpu")]
    if self.renderer.is_gpu() {
      match self.vis {
        Vis::Shown if !self.frame_pending => {
          self.present_gpu(false);
        }
        Vis::Hidden => {
          if self.first_answer && !self.gpu_warmed {
            // Draw the first page into an offscreen texture (Slint's
            // snapshot path; nothing is presented): the costly first-frame
            // work then happens before Ready, not on the first Show.
            self.gpu_warmed = true;
            let t = Instant::now();
            match self.ui.window().take_snapshot() {
              Ok(_) => tracing::debug!(ms = t.elapsed().as_millis() as u64, "gpu warm-up frame"),
              Err(e) => tracing::warn!("gpu warm-up: {e}"),
            }
            self.renderer.request_redraw();
          }
          self.maybe_ready();
        }
        _ => {}
      }
      return;
    }
    match self.vis {
      Vis::AwaitConfigure => {}
      Vis::Shown if self.frame_pending => {}
      Vis::Shown => match self.render_back() {
        Render::Drew(idx, dmg) => {
          self.present(idx, Some(dmg));
          if std::mem::take(&mut self.focus_secret_after_render) {
            self.bump_secret_focus();
          }
        }
        Render::Busy => self.retry_soon = true,
        Render::Clean => {}
      },
      Vis::Hidden if !self.prerender => self.maybe_ready(),
      Vis::Hidden => {
        if let Render::Busy = self.render_back() {
          self.retry_soon = true;
        } else {
          self.maybe_ready();
        }
      }
    }
  }

  /// `Ready` once the first answer is in and (pre-render on) drawn.
  fn maybe_ready(&mut self) {
    if !self.ready_sent
      && self.first_answer
      && self.surf.is_some()
      && (self.last_drawn.is_some() || !self.prerender)
    {
      self.ready_sent = true;
      self.chan.send(PickerReq::Ready);
    }
  }

  fn on_configure(&mut self, size: (u32, u32)) {
    let w = if size.0 == 0 { self.logical.0 } else { size.0 };
    let h = if size.1 == 0 { self.logical.1 } else { size.1 };
    if (w, h) != self.logical {
      self.logical = (w, h);
      let s = self.scale;
      self.apply_scale(s);
    }
    if self.vis != Vis::AwaitConfigure {
      return;
    }
    #[cfg(feature = "gpu")]
    if self.renderer.is_gpu() {
      if !self.present_gpu(true) {
        // EGL could not draw: let the main loop retry.
        self.vis = Vis::Shown;
        self.retry_soon = true;
        return;
      }
      self.after_map();
      return;
    }
    // Map: newest rendering if anything changed, else the pre-rendered frame.
    match self.render_back() {
      Render::Drew(idx, _) => self.present(idx, None),
      Render::Clean if self.last_drawn.is_some() => {
        let idx = self.last_drawn.unwrap_or(0);
        self.present(idx, None);
      }
      _ => {
        // Nothing drawable yet (should not happen): render ASAP.
        self.vis = Vis::Shown;
        self.renderer.request_redraw();
        self.retry_soon = true;
        return;
      }
    }
    self.after_map();
  }

  /// Now that the first frame is on its way: activate (cursor blink),
  /// refresh relative times; those become the next frame.
  fn after_map(&mut self) {
    self.ui.window().dispatch_event(WindowEvent::WindowActiveChanged(true));
    self.focus_input();
    self.refresh_times();
    self.request_visible_thumbs();
  }

  /// GPU: render with FemtoVG and present through eglSwapBuffers (which
  /// commits). Surface state for that commit is set first. `force`: draw
  /// even if Slint has nothing new (mapping).
  #[cfg(feature = "gpu")]
  fn present_gpu(&mut self, force: bool) -> bool {
    let Backend::Gpu(g) = &self.renderer else { return false };
    let g = g.clone();
    let Some(s) = &self.surf else { return false };
    if force {
      slint::platform::WindowAdapter::request_redraw(&*g);
    }
    if !g.needs_redraw() {
      return false;
    }
    if let Err(e) = g.egl.attach(&s.wl) {
      tracing::warn!("EGL surface: {e:#}");
      return false;
    }
    let wl = s.wl.clone();
    if let Some(vp) = &s.viewport {
      vp.set_destination(self.logical.0 as i32, self.logical.1 as i32);
    } else {
      wl.set_buffer_scale(self.scale.round().max(1.0) as i32);
    }
    wl.frame(&self.qh, FrameCallbackData(wl.clone()));
    #[cfg(feature = "timing")]
    if !self.pending_marks.is_empty()
      && let Some(p) = &self.presentation
      && let Ok(fb) = p.feedback(&wl, &self.qh)
    {
      self.feedback_marks.insert(fb.id(), std::mem::take(&mut self.pending_marks));
    }
    #[cfg(not(feature = "timing"))]
    self.pending_marks.clear();
    let t = Instant::now();
    match g.draw_if_needed() {
      Ok(true) => {
        tracing::debug!(us = t.elapsed().as_micros() as u64, "rendered (gpu)");
        self.frame_pending = true;
        self.vis = Vis::Shown;
        let _ = self.conn.flush();
        true
      }
      Ok(false) => false,
      Err(e) => {
        tracing::warn!("gpu render: {e}");
        false
      }
    }
  }

  fn present(&mut self, idx: usize, damage: Option<Vec<Rect>>) {
    let Some(s) = &self.surf else { return };
    let Some(buf) = self.bufs[idx].as_ref() else { return };
    let wl = &s.wl;
    if buf.attach_to(wl).is_err() {
      // Still held by the compositor from an earlier commit; re-attaching
      // the same wl_buffer is fine.
      wl.attach(Some(buf.wl_buffer()), 0, 0);
    }
    if let Some(vp) = &s.viewport {
      vp.set_destination(self.logical.0 as i32, self.logical.1 as i32);
    } else {
      wl.set_buffer_scale(self.scale.round().max(1.0) as i32);
    }
    match damage {
      Some(rects) => {
        for r in rects {
          wl.damage_buffer(r.x, r.y, r.w, r.h);
        }
      }
      None => wl.damage_buffer(0, 0, i32::MAX, i32::MAX),
    }
    wl.frame(&self.qh, FrameCallbackData(wl.clone()));
    self.frame_pending = true;
    #[cfg(feature = "timing")]
    if !self.pending_marks.is_empty()
      && let Some(p) = &self.presentation
      && let Ok(fb) = p.feedback(wl, &self.qh)
    {
      self.feedback_marks.insert(fb.id(), std::mem::take(&mut self.pending_marks));
    }
    #[cfg(not(feature = "timing"))]
    self.pending_marks.clear();
    wl.commit();
    let _ = self.conn.flush();
    self.vis = Vis::Shown;
  }

  pub fn flush(&self) {
    let _ = self.conn.flush();
  }
}

enum Render {
  Drew(usize, Vec<Rect>),
  Clean,
  Busy,
}

/// Top-left of a `size` window near `cursor` (output-local), clamped to the
/// output with an `EDGE` gap; centered when there is no cursor.
pub fn place(cursor: Option<(i32, i32)>, output: (i32, i32), size: (i32, i32)) -> (i32, i32) {
  let axis = |c: Option<i32>, out: i32, sz: i32| -> i32 {
    if out < sz + 2 * EDGE {
      return ((out - sz) / 2).max(0);
    }
    match c {
      Some(c) => (c + EDGE).clamp(EDGE, out - sz - EDGE),
      None => (out - sz) / 2,
    }
  };
  (axis(cursor.map(|c| c.0), output.0, size.0), axis(cursor.map(|c| c.1), output.1, size.1))
}

/// Enter = paste, Shift+Enter = copy only, Ctrl+Enter = paste as plain text.
fn select_mode(m: &Modifiers) -> SelectMode {
  if m.shift {
    SelectMode::Copy
  } else if m.ctrl {
    SelectMode::PastePlain
  } else {
    SelectMode::Paste
  }
}

fn is_modifier(k: Keysym) -> bool {
  matches!(
    k,
    Keysym::Shift_L
      | Keysym::Shift_R
      | Keysym::Control_L
      | Keysym::Control_R
      | Keysym::Alt_L
      | Keysym::Alt_R
      | Keysym::Super_L
      | Keysym::Super_R
      | Keysym::ISO_Level3_Shift
      | Keysym::Caps_Lock
      | Keysym::Num_Lock
  )
}

/// Slint key text for a keysym (None = ignore).
fn slint_text(k: Keysym, utf8: Option<&str>, mods: &Modifiers) -> Option<SharedString> {
  let special = match k {
    Keysym::BackSpace => Some(Key::Backspace),
    Keysym::Tab => Some(Key::Tab),
    Keysym::ISO_Left_Tab => Some(Key::Backtab),
    Keysym::Left | Keysym::KP_Left => Some(Key::LeftArrow),
    Keysym::Right | Keysym::KP_Right => Some(Key::RightArrow),
    Keysym::Home | Keysym::KP_Home => Some(Key::Home),
    Keysym::End | Keysym::KP_End => Some(Key::End),
    Keysym::Insert => Some(Key::Insert),
    Keysym::Shift_L => Some(Key::Shift),
    Keysym::Shift_R => Some(Key::ShiftR),
    Keysym::Control_L => Some(Key::Control),
    Keysym::Control_R => Some(Key::ControlR),
    Keysym::Alt_L => Some(Key::Alt),
    Keysym::ISO_Level3_Shift | Keysym::Alt_R => Some(Key::AltGr),
    Keysym::Super_L => Some(Key::Meta),
    Keysym::Super_R => Some(Key::MetaR),
    Keysym::Caps_Lock | Keysym::Num_Lock => return None,
    _ => None,
  };
  if let Some(key) = special {
    return Some(key.into());
  }
  if !(mods.ctrl || mods.alt)
    && let Some(t) = utf8
    && !t.is_empty()
    && !t.chars().any(char::is_control)
  {
    return Some(t.into());
  }
  k.key_char().filter(|c| !c.is_control()).map(|c| SharedString::from(c.to_string().as_str()))
}

// ------------------------------------------------------------- sctk glue

/// User data for globals/objects without events we care about.
pub struct NoEvents;

impl<I: Proxy> Dispatch2<I, App> for NoEvents {
  fn event(&self, _: &mut App, _: &I, _: I::Event, _: &Connection, _: &QueueHandle<App>) {}
}

pub struct FracData;

impl Dispatch2<WpFractionalScaleV1, App> for FracData {
  fn event(
    &self,
    app: &mut App,
    _: &WpFractionalScaleV1,
    ev: wp_fractional_scale_v1::Event,
    _: &Connection,
    _: &QueueHandle<App>,
  ) {
    if let wp_fractional_scale_v1::Event::PreferredScale { scale } = ev {
      app.on_preferred_scale(scale as f64 / 120.0);
    }
  }
}

impl CompositorHandler for App {
  fn scale_factor_changed(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    _: &wl_surface::WlSurface,
    f: i32,
  ) {
    if self.frac_mgr.is_none() {
      self.on_preferred_scale(f as f64);
    }
  }

  fn transform_changed(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    _: &wl_surface::WlSurface,
    _: wl_output::Transform,
  ) {
  }

  fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
    self.frame_pending = false;
  }

  fn surface_enter(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    _: &wl_surface::WlSurface,
    _: &wl_output::WlOutput,
  ) {
  }

  fn surface_leave(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    _: &wl_surface::WlSurface,
    _: &wl_output::WlOutput,
  ) {
  }
}

impl OutputHandler for App {
  fn output_state(&mut self) -> &mut OutputState {
    &mut self.output_state
  }
  fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
  fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
  fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, o: wl_output::WlOutput) {
    if self.surf.as_ref().is_some_and(|s| s.output.as_ref() == Some(&o)) {
      self.hide(HideReason::Closed);
      self.drop_surface();
    }
  }
}

/// User data for our layer surface; see `Surf`.
pub struct LayerData;

impl Dispatch2<ZwlrLayerSurfaceV1, App> for LayerData {
  fn event(
    &self,
    app: &mut App,
    layer: &ZwlrLayerSurfaceV1,
    ev: zwlr_layer_surface_v1::Event,
    _: &Connection,
    _: &QueueHandle<App>,
  ) {
    if app.surf.as_ref().is_none_or(|s| &s.layer != layer) {
      return;
    }
    match ev {
      zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
        if app.vis == Vis::Hidden {
          // Sent before the compositor processed our unmap: stale, and
          // acking it is a protocol error.
          return;
        }
        layer.ack_configure(serial);
        app.on_configure((width, height));
      }
      zwlr_layer_surface_v1::Event::Closed => {
        // Compositor dropped our surface (e.g. its output went away).
        // Recreated on the next Show.
        if app.vis != Vis::Hidden {
          app.hide(HideReason::Closed);
        }
        app.vis = Vis::Hidden;
        app.frame_pending = false;
        app.drop_surface();
      }
      _ => {}
    }
  }
}

impl SeatHandler for App {
  fn seat_state(&mut self) -> &mut SeatState {
    &mut self.seat_state
  }
  fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
  fn new_capability(
    &mut self,
    _: &Connection,
    qh: &QueueHandle<Self>,
    seat: wl_seat::WlSeat,
    cap: Capability,
  ) {
    if cap == Capability::Keyboard && self.keyboard.is_none() {
      match self.seat_state.get_keyboard_with_repeat(
        qh,
        &seat,
        None,
        self.loop_handle.clone(),
        Box::new(|app: &mut App, _, ev| app.on_key(ev, true)),
      ) {
        Ok(k) => self.keyboard = Some(k),
        Err(e) => tracing::warn!(error = %e, "no keyboard"),
      }
    }
    if cap == Capability::Pointer && self.pointer.is_none() {
      self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
    }
  }
  fn remove_capability(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    _: wl_seat::WlSeat,
    cap: Capability,
  ) {
    if cap == Capability::Keyboard
      && let Some(k) = self.keyboard.take()
    {
      k.release();
    }
    if cap == Capability::Pointer
      && let Some(p) = self.pointer.take()
    {
      p.release();
    }
  }
  fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
  fn enter(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    _: &wl_keyboard::WlKeyboard,
    surface: &wl_surface::WlSurface,
    _: u32,
    _: &[u32],
    _: &[Keysym],
  ) {
    if self.surf.as_ref().is_some_and(|s| &s.wl == surface) {
      self.focus_lost_at = None;
    }
  }

  fn leave(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    _: &wl_keyboard::WlKeyboard,
    surface: &wl_surface::WlSurface,
    _: u32,
  ) {
    // Keys still held are released elsewhere: forget a pending Enter.
    self.enter_down = None;
    // Focus loss hides the picker, but only if focus does not come straight
    // back (sway sends leave+enter when the layer surface maps).
    if self.surf.as_ref().is_some_and(|s| &s.wl == surface) && self.vis == Vis::Shown {
      self.focus_lost_at = Some(Instant::now());
    }
  }

  fn press_key(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    _: &wl_keyboard::WlKeyboard,
    _: u32,
    ev: KeyEvent,
  ) {
    self.on_key(ev, false);
  }

  fn repeat_key(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    _: &wl_keyboard::WlKeyboard,
    _: u32,
    ev: KeyEvent,
  ) {
    self.on_key(ev, true);
  }

  fn release_key(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    _: &wl_keyboard::WlKeyboard,
    _: u32,
    ev: KeyEvent,
  ) {
    self.on_key_release(ev);
  }

  fn update_modifiers(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    _: &wl_keyboard::WlKeyboard,
    _: u32,
    m: Modifiers,
    _: RawModifiers,
    _: u32,
  ) {
    self.mods = m;
  }
}

impl PointerHandler for App {
  fn pointer_frame(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    _: &wl_pointer::WlPointer,
    events: &[PointerEvent],
  ) {
    if self.vis != Vis::Shown {
      return;
    }
    let Some(s) = &self.surf else { return };
    let ours = s.wl.clone();
    for e in events {
      if e.surface != ours {
        continue;
      }
      let position = LogicalPosition::new(e.position.0 as f32, e.position.1 as f32);
      let w = self.ui.window();
      match e.kind {
        PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
          w.dispatch_event(WindowEvent::PointerMoved { position })
        }
        PointerEventKind::Leave { .. } => w.dispatch_event(WindowEvent::PointerExited),
        PointerEventKind::Press { button, .. } | PointerEventKind::Release { button, .. } => {
          let button = match button {
            0x110 => PointerEventButton::Left,
            0x111 => PointerEventButton::Right,
            0x112 => PointerEventButton::Middle,
            _ => PointerEventButton::Other,
          };
          if matches!(e.kind, PointerEventKind::Press { .. }) {
            w.dispatch_event(WindowEvent::PointerPressed { position, button });
          } else {
            w.dispatch_event(WindowEvent::PointerReleased { position, button });
          }
        }
        PointerEventKind::Axis { horizontal, vertical, .. } => {
          let px = |a: smithay_client_toolkit::seat::pointer::AxisScroll| {
            if a.absolute != 0.0 { a.absolute } else { a.discrete as f64 * 15.0 }
          };
          w.dispatch_event(WindowEvent::PointerScrolled {
            position,
            delta_x: -px(horizontal) as f32,
            delta_y: -px(vertical) as f32,
          });
        }
      }
    }
  }
}

impl ShmHandler for App {
  fn shm_state(&mut self) -> &mut Shm {
    &mut self.shm
  }
}

#[cfg(feature = "timing")]
impl smithay_client_toolkit::presentation_time::PresentationTimeHandler for App {
  fn presentation_time_state(
    &mut self,
  ) -> &mut smithay_client_toolkit::presentation_time::PresentationTimeState {
    self.presentation.as_mut().expect("bound")
  }

  fn presented(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    fb: &smithay_client_toolkit::reexports::protocols::wp::presentation_time::client::wp_presentation_feedback::WpPresentationFeedback,
    _: &wl_surface::WlSurface,
    _: Vec<wl_output::WlOutput>,
    time: smithay_client_toolkit::presentation_time::PresentTime,
    _: u32,
    _: u64,
    _: smithay_client_toolkit::reexports::client::WEnum<
      smithay_client_toolkit::reexports::protocols::wp::presentation_time::client::wp_presentation_feedback::Kind,
    >,
  ) {
    let t_present = time.tv_sec * 1_000_000_000 + time.tv_nsec as u64;
    for m in self.feedback_marks.remove(&fb.id()).unwrap_or_default() {
      let kind = match m.kind {
        MarkKind::Cold => "cold",
        MarkKind::Show => "show",
        MarkKind::Key => "key",
      };
      // Timing only: no content. Parsed by examples/mock_daemon.rs.
      eprintln!(
        "SPOOL_TIMING kind={kind} clk={} t_evt_ns={} t_present_ns={t_present} delta_us={}",
        time.clk_id,
        m.t_evt_ns,
        t_present.saturating_sub(m.t_evt_ns) / 1000
      );
    }
  }

  fn discarded(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    fb: &smithay_client_toolkit::reexports::protocols::wp::presentation_time::client::wp_presentation_feedback::WpPresentationFeedback,
    _: &wl_surface::WlSurface,
  ) {
    for m in self.feedback_marks.remove(&fb.id()).unwrap_or_default() {
      eprintln!("SPOOL_TIMING kind=discarded t_evt_ns={}", m.t_evt_ns);
    }
  }
}

impl ProvidesRegistryState for App {
  fn registry(&mut self) -> &mut RegistryState {
    &mut self.registry_state
  }
  registry_handlers![OutputState, SeatState];
}

delegate_dispatch2!(App);
delegate_registry!(App);

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn placement_near_cursor_and_clamped() {
    let out = (1920, 1080);
    let sz = (640, 480);
    assert_eq!(place(Some((100, 100)), out, sz), (108, 108));
    // Right/bottom edges clamp.
    assert_eq!(place(Some((1900, 1070)), out, sz), (1920 - 640 - 8, 1080 - 480 - 8));
    // Negative (cursor on another output) clamps to the edge gap.
    assert_eq!(place(Some((-50, -50)), out, sz), (8, 8));
    // Centered without a cursor.
    assert_eq!(place(None, out, sz), (640, 300));
    // Output smaller than the picker: centered, never negative.
    assert_eq!(place(Some((10, 10)), (600, 400), sz), (0, 0));
  }

  #[test]
  fn enter_modes() {
    let m = |shift, ctrl| Modifiers { shift, ctrl, ..Default::default() };
    assert_eq!(select_mode(&m(false, false)), SelectMode::Paste);
    assert_eq!(select_mode(&m(true, false)), SelectMode::Copy);
    assert_eq!(select_mode(&m(false, true)), SelectMode::PastePlain);
    assert_eq!(select_mode(&m(true, true)), SelectMode::Copy);
  }

  #[test]
  fn unlock_messages_cover_every_reason() {
    use UnlockFailReason as R;
    for p in [UnlockProvider::KWallet, UnlockProvider::Passphrase, UnlockProvider::Fido2] {
      for r in
        [R::WrongSecret, R::PinInvalid, R::PinBlocked, R::Dismissed, R::Unavailable, R::Other]
      {
        assert!(!unlock_error_text(p, r).is_empty());
      }
    }
    assert!(unlock_error_text(UnlockProvider::Passphrase, R::WrongSecret).contains("passphrase"));
    assert!(unlock_error_text(UnlockProvider::Fido2, R::PinInvalid).contains("PIN"));
  }

  #[test]
  fn key_text_mapping() {
    let none = Modifiers::default();
    let ctrl = Modifiers { ctrl: true, ..Default::default() };
    assert_eq!(slint_text(Keysym::a, Some("a"), &none).as_deref(), Some("a"));
    // Ctrl+A arrives as a control char in utf8; Slint wants "a" + Control.
    assert_eq!(slint_text(Keysym::a, Some("\u{1}"), &ctrl).as_deref(), Some("a"));
    assert_eq!(slint_text(Keysym::BackSpace, Some("\u{8}"), &none), Some(Key::Backspace.into()));
    assert_eq!(slint_text(Keysym::Shift_L, None, &none), Some(Key::Shift.into()));
    assert_eq!(slint_text(Keysym::Caps_Lock, None, &none), None);
    // Composed text wins over the keysym.
    assert_eq!(slint_text(Keysym::dead_acute, Some("é"), &none).as_deref(), Some("é"));
  }
}
