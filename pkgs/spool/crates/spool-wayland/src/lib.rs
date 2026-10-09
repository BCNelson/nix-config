//! Spool Wayland backend.
//!
//! Owns every Wayland object on one dedicated thread and talks to the rest of
//! the daemon through a command channel ([`WaylandHandle`]) and an event
//! channel (`crossbeam_channel::Receiver<WaylandEvent>`). Uses
//! `ext-data-control-v1` (`ext_data_control_manager_v1`).
//!
//! # Rules
//!
//! * Fetching from a source app and serving `send` requests never blocks the
//!   Wayland event loop: pipe I/O runs on helper threads (or non-blocking
//!   fds) with caps and deadlines. A slow or infinite source is cut off at
//!   the cap/timeout.
//! * Never log clipboard content; mimes and byte counts only.
//! * The daemon's own selections are tagged with the marker mime
//!   `application/x-spool-source;nonce=<nonce>` ([`marker_mime`]); the nonce
//!   is regenerated on every [`WaylandHandle::set_selection`]. A
//!   `NewSelection` whose offer carries the *current* marker has
//!   `ours = true`. The marker mime is never included in
//!   `NewSelection::mimes`.
//!
//! # Offer lifetime
//!
//! An [`OfferToken`] stays valid until the next `NewSelection` or
//! `SelectionCleared` for the same selection; fetching a stale token yields
//! [`FetchError::OfferGone`]. A fetch already in progress when the offer is
//! replaced may still complete (pipes stay open), or fail with `OfferGone`.
//!
//! # Threading and ordering
//!
//! * All protocol objects live on the `spool-wayland` thread. Each fetch runs
//!   on its own short-lived reader thread (at most [`MAX_CONCURRENT_FETCHES`]);
//!   each paste served from our source runs on a writer thread (at most
//!   [`MAX_CONCURRENT_SENDS`], each bounded by [`SEND_TIMEOUT`]). Writers rely
//!   on `SIGPIPE` being ignored (Rust's default for binaries).
//! * Selection events are delivered in compositor order. `Fetched` arrives
//!   whenever the reader finishes, so it may come after later
//!   `NewSelection`s; match it by token.
//! * On bind the compositor replays the current clipboard (and primary, if
//!   watched) state, so `Ready` is followed by a `NewSelection` or
//!   `SelectionCleared` for each watched selection.
//! * `ours` is true when the offer carries the marker of a source of ours
//!   that has not been cancelled yet (in practice the most recent
//!   `set_selection` for that selection; a just-replaced source's own
//!   selection event can still arrive, and also counts).
//! * Only the first `wl_seat` is followed (multi-seat is unsupported). A seat
//!   that appears after startup is picked up.
//! * A source that exits (or `wl-copy --clear`) yields `SelectionCleared`.
//!   A backgrounded `wl-copy` stays alive until something else takes the
//!   selection, so it produces a `NewSelection` for the new owner instead.

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

pub use spool_core::item::Selection;

mod event_loop;
mod pipes;

pub use event_loop::{MAX_CONCURRENT_FETCHES, MAX_CONCURRENT_SENDS, SEND_TIMEOUT};

/// Prefix of the marker mime the daemon adds to its own offers.
pub const MARKER_MIME_PREFIX: &str = "application/x-spool-source;nonce=";

/// Name of the required global, for error messages.
pub const DATA_CONTROL_GLOBAL: &str = "ext_data_control_manager_v1";

/// Marker mime for `nonce`.
pub fn marker_mime(nonce: &str) -> String {
  format!("{MARKER_MIME_PREFIX}{nonce}")
}

/// Whether `mime` is any Spool marker mime (current nonce or not).
pub fn is_marker_mime(mime: &str) -> bool {
  mime.starts_with(MARKER_MIME_PREFIX)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WaylandConfig {
  /// Wayland display name or absolute socket path; `None` = `$WAYLAND_DISPLAY`.
  pub display: Option<String>,
  /// Also watch (and allow setting) the primary selection. When `false`, no
  /// `Primary` events are emitted.
  pub watch_primary: bool,
}

/// Opaque handle to a data-control offer living on the Wayland thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OfferToken(u64);

impl OfferToken {
  /// For tests of consumers (fake event streams). Not meaningful to the
  /// Wayland thread unless it issued the value.
  #[doc(hidden)]
  pub const fn from_raw(v: u64) -> Self {
    Self(v)
  }

  pub const fn raw(self) -> u64 {
    self.0
  }
}

/// Information about the connected compositor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositorInfo {
  /// Display name used to connect (e.g. `wayland-1`).
  pub display: String,
  /// Bound version of `ext_data_control_manager_v1`.
  pub data_control_version: u32,
  /// Whether the data-control device supports the primary selection
  /// (`ext_data_control_device_v1.primary_selection` events can occur).
  pub primary_supported: bool,
}

/// Fetched representations: `(mime, bytes)` in requested order, omitting
/// mimes that exceeded `per_rep_cap` or that the source closed with an
/// error.
pub type FetchResult = Result<Vec<(String, Vec<u8>)>, FetchError>;

/// Event from the Wayland thread.
#[derive(Debug)]
pub enum WaylandEvent {
  /// Connected and bound; sent once, before any other event.
  Ready(CompositorInfo),
  /// A new selection was offered (by any client, including us).
  NewSelection {
    selection: Selection,
    /// Offered mimes in offer order, excluding marker mimes.
    mimes: Vec<String>,
    offer: OfferToken,
    /// The offer carries the marker with our *current* nonce.
    ours: bool,
  },
  /// The compositor set the selection to null (e.g. the source client
  /// exited or a client cleared it).
  SelectionCleared { selection: Selection },
  /// Completion of a [`WaylandHandle::fetch`]. Exactly one per fetch call.
  Fetched { offer: OfferToken, result: FetchResult },
  /// Unrecoverable; the thread has exited after sending this. Messages name
  /// the cause, e.g. "compositor does not advertise
  /// ext_data_control_manager_v1" or "connection to wayland-1 lost: ...".
  Fatal(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
  /// Overall deadline elapsed before all mimes were read.
  #[error("fetch timed out")]
  Timeout,
  /// The sum of fetched bytes exceeded `total_cap`.
  #[error("offer exceeds total size cap")]
  TooLarge,
  /// The offer token is unknown or was replaced.
  #[error("offer no longer valid")]
  OfferGone,
  #[error("i/o error: {0}")]
  Io(String),
}

#[derive(Debug, thiserror::Error)]
pub enum WaylandError {
  /// Could not connect to the display.
  #[error("cannot connect to wayland display: {0}")]
  Connect(String),
  /// A required global is missing (always names it, e.g.
  /// [`DATA_CONTROL_GLOBAL`]).
  #[error(
    "compositor does not advertise {0}; Spool needs ext-data-control-v1 \
     (KWin >= 6.5, wlroots >= 0.19 / sway >= 1.11)"
  )]
  MissingGlobal(&'static str),
  /// The Wayland thread has exited (after `Fatal` or `shutdown`).
  #[error("wayland thread is gone")]
  ThreadGone,
  #[error("i/o error: {0}")]
  Io(#[from] std::io::Error),
}

/// Command handle to the Wayland thread. Cheap to clone; all methods are
/// non-blocking (they enqueue a command and wake the thread).
#[derive(Debug, Clone)]
pub struct WaylandHandle {
  inner: Arc<HandleInner>,
}

#[derive(Debug)]
struct HandleInner {
  cmd: crossbeam_channel::Sender<event_loop::Cmd>,
  waker: Arc<event_loop::Waker>,
}

impl WaylandHandle {
  fn send(&self, cmd: event_loop::Cmd) -> Result<(), WaylandError> {
    self.inner.cmd.send(cmd).map_err(|_| WaylandError::ThreadGone)?;
    self.inner.waker.wake();
    Ok(())
  }
}

impl std::fmt::Debug for event_loop::Cmd {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    // Never print rep bytes.
    match self {
      Self::Fetch { offer, mimes, .. } => write!(f, "Fetch({offer:?}, {mimes:?})"),
      Self::SetSelection { selection, reps } => {
        write!(f, "SetSelection({selection:?}, {} reps)", reps.len())
      }
      Self::Shutdown => f.write_str("Shutdown"),
    }
  }
}

impl WaylandHandle {
  /// Read `mimes` (each offered by `offer`) concurrently or sequentially,
  /// stopping a mime at `per_rep_cap` bytes (that mime is omitted from the
  /// result), failing with `TooLarge` if the total exceeds `total_cap`, and
  /// with `Timeout` if not done within `timeout`. Result arrives as
  /// [`WaylandEvent::Fetched`].
  pub fn fetch(
    &self,
    offer: OfferToken,
    mimes: Vec<String>,
    per_rep_cap: usize,
    total_cap: usize,
    timeout: Duration,
  ) -> Result<(), WaylandError> {
    self.send(event_loop::Cmd::Fetch { offer, mimes, per_rep_cap, total_cap, timeout })
  }

  /// Become the owner of `selection`, offering each `reps` mime plus a fresh
  /// marker mime. `send` requests are served from helper threads with a
  /// write deadline; the event loop never blocks. Replaces any previous
  /// source we owned for that selection. Setting `Primary` when
  /// `watch_primary` is false or unsupported is a no-op (logged). Empty
  /// `reps` clears the selection instead (no marker; yields
  /// `SelectionCleared`). Marker mimes and duplicate mimes in `reps` are
  /// dropped. Requests for the marker mime get an empty body.
  pub fn set_selection(
    &self,
    selection: Selection,
    reps: Vec<(String, Arc<[u8]>)>,
  ) -> Result<(), WaylandError> {
    self.send(event_loop::Cmd::SetSelection { selection, reps })
  }

  /// Ask the thread to destroy all objects, disconnect and exit. Idempotent;
  /// the event channel is closed afterwards.
  pub fn shutdown(&self) {
    let _ = self.send(event_loop::Cmd::Shutdown);
  }
}

/// Connect to the compositor, bind `ext_data_control_manager_v1` and the
/// seat's data-control device, and start the Wayland thread.
///
/// Connection failure and a missing global are reported synchronously as
/// `Err` (the error names the missing global). After a successful return,
/// the first event is [`WaylandEvent::Ready`], followed by the initial
/// `NewSelection`/`SelectionCleared` state the compositor sends. Runtime
/// failures arrive as [`WaylandEvent::Fatal`].
pub fn spawn(
  cfg: WaylandConfig,
) -> Result<(WaylandHandle, crossbeam_channel::Receiver<WaylandEvent>), WaylandError> {
  let waker = event_loop::new_waker()?;
  let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
  let (ev_tx, ev_rx) = crossbeam_channel::unbounded();
  let (init_tx, init_rx) = crossbeam_channel::bounded(1);
  let channels = event_loop::Channels { cmd_rx, waker: waker.clone(), events: ev_tx };
  std::thread::Builder::new()
    .name("spool-wayland".into())
    .spawn(move || event_loop::run(cfg, channels, init_tx))?;
  match init_rx.recv() {
    Ok(Ok(())) => {}
    Ok(Err(e)) => return Err(e),
    Err(_) => return Err(WaylandError::ThreadGone),
  }
  Ok((WaylandHandle { inner: Arc::new(HandleInner { cmd: cmd_tx, waker }) }, ev_rx))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn marker() {
    let m = marker_mime("abc");
    assert_eq!(m, "application/x-spool-source;nonce=abc");
    assert!(is_marker_mime(&m));
    assert!(!is_marker_mime("text/plain"));
  }

  #[test]
  fn connect_failure_is_synchronous() {
    // Absolute path that cannot exist: never touches a real compositor.
    let cfg = WaylandConfig {
      display: Some("/nonexistent/spool-test/wayland-99".into()),
      watch_primary: false,
    };
    match spawn(cfg) {
      Err(WaylandError::Connect(msg)) => assert!(msg.contains("wayland-99"), "{msg}"),
      other => panic!("expected Connect error, got {:?}", other.map(|_| ())),
    }
  }

  #[test]
  fn missing_global_message_names_it() {
    let msg = WaylandError::MissingGlobal(DATA_CONTROL_GLOBAL).to_string();
    assert!(msg.contains("ext_data_control_manager_v1"), "{msg}");
    assert!(msg.contains("KWin"), "{msg}");
  }

  #[test]
  fn protocol_bindings_exist() {
    // Compile-time check that the staging ext-data-control bindings are
    // available with the pinned wayland-protocols features.
    use wayland_protocols::ext::data_control::v1::client::{
      ext_data_control_device_v1, ext_data_control_manager_v1, ext_data_control_offer_v1,
      ext_data_control_source_v1,
    };
    let _ = (
      std::any::type_name::<ext_data_control_manager_v1::ExtDataControlManagerV1>(),
      std::any::type_name::<ext_data_control_device_v1::ExtDataControlDeviceV1>(),
      std::any::type_name::<ext_data_control_offer_v1::ExtDataControlOfferV1>(),
      std::any::type_name::<ext_data_control_source_v1::ExtDataControlSourceV1>(),
    );
  }
}
