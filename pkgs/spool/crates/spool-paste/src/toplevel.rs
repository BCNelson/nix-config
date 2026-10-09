//! Active-window tracking through `zwlr_foreign_toplevel_manager_v1`
//! (sway and other wlroots compositors; Hyprland also offers it but its IPC
//! gives stable addresses, see `spool-hypr`).
//!
//! `ext-foreign-toplevel-list-v1` has no activation state, so the wlr
//! protocol is used: every toplevel handle reports `app_id` and a `state`
//! array; the handle whose committed (`done`) state contains `activated` is
//! the active window. The protocol has no stable window ids, so each handle
//! gets a per-tracker synthetic id `wlr-<n>` that is stable for the window's
//! lifetime. Titles are received (the protocol always sends them) but
//! dropped immediately; they are never stored or logged.
//!
//! The tracker runs its own Wayland connection on a dedicated thread and
//! reports changes through a callback, typically
//! `move |app, win| sink.active_window(app, win)` with a
//! `spool_compositor::EventSink`.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;

use rustix::event::{PollFd, PollFlags, poll};
use wayland_client::backend::ObjectId;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::{
  self as handle, ZwlrForeignToplevelHandleV1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::{
  self as manager, ZwlrForeignToplevelManagerV1,
};

use crate::PasteError;

/// Interface name of the wlr foreign toplevel manager.
pub const FOREIGN_TOPLEVEL_GLOBAL: &str = "zwlr_foreign_toplevel_manager_v1";
/// Longest app id kept; longer ones are treated as unknown.
const MAX_APP_ID: usize = 256;

/// Called with `(app_id, window_id)` of the new active window (`None`s when
/// nothing is active) on every change, from the tracker thread.
pub type OnChange = Box<dyn FnMut(Option<&str>, Option<&str>) + Send>;

#[derive(Default)]
struct Entry {
  id: String,
  app_id: Option<String>,
  pending_app_id: Option<Option<String>>,
  activated: bool,
  pending_activated: Option<bool>,
}

struct State {
  entries: HashMap<ObjectId, Entry>,
  next_id: u64,
  active: Option<ObjectId>,
  reported: Option<(Option<String>, Option<String>)>,
  on_change: OnChange,
  finished: bool,
}

impl State {
  fn report(&mut self) {
    let cur = match self.active.as_ref().and_then(|a| self.entries.get(a)) {
      Some(e) => (e.app_id.clone(), Some(e.id.clone())),
      None => (None, None),
    };
    if self.reported.as_ref() != Some(&cur) {
      (self.on_change)(cur.0.as_deref(), cur.1.as_deref());
      self.reported = Some(cur);
    }
  }
}

/// A running tracker. Dropping it stops the thread and closes its
/// connection.
pub struct ToplevelTracker {
  stop: Option<OwnedFd>,
  thread: Option<JoinHandle<()>>,
}

fn connect(display: Option<&str>) -> Result<Connection, PasteError> {
  crate::paster::connect_display(display)
}

impl ToplevelTracker {
  /// Connects (`display`: name or absolute socket path, `None` =
  /// `$WAYLAND_DISPLAY`), reports the initial active window through
  /// `on_change` before returning, then follows changes on a thread.
  /// [`PasteError::Unsupported`] if the compositor lacks the protocol.
  pub fn spawn(
    display: Option<String>,
    on_change: impl FnMut(Option<&str>, Option<&str>) + Send + 'static,
  ) -> Result<Self, PasteError> {
    let (stop_r, stop_w) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
      .map_err(|e| PasteError::Connect(format!("pipe: {e}")))?;
    let (ready_tx, ready_rx) = std_mpsc::sync_channel::<Result<(), PasteError>>(1);
    let on_change: OnChange = Box::new(on_change);
    let thread = std::thread::Builder::new()
      .name("spool-toplevel".into())
      .spawn(move || {
        let (conn, mut queue, mut state) = match setup(display.as_deref(), on_change) {
          Ok(v) => v,
          Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
          }
        };
        let _ = ready_tx.send(Ok(()));
        if let Err(e) = run(&conn, &mut queue, &mut state, &stop_r) {
          tracing::warn!(error = %e, "foreign-toplevel tracking stopped");
        }
      })
      .map_err(|e| PasteError::Connect(e.to_string()))?;
    match ready_rx.recv() {
      Ok(Ok(())) => Ok(Self { stop: Some(stop_w), thread: Some(thread) }),
      Ok(Err(e)) => {
        let _ = thread.join();
        Err(e)
      }
      Err(_) => Err(PasteError::ThreadGone),
    }
  }

  /// Whether the tracker thread is still running (it ends if the
  /// compositor goes away or stops the manager).
  pub fn is_running(&self) -> bool {
    self.thread.as_ref().is_some_and(|t| !t.is_finished())
  }
}

impl Drop for ToplevelTracker {
  fn drop(&mut self) {
    drop(self.stop.take()); // HUP on the read end wakes the thread
    if let Some(t) = self.thread.take() {
      let _ = t.join();
    }
  }
}

fn werr(e: impl std::fmt::Display) -> PasteError {
  PasteError::Wayland(e.to_string())
}

fn setup(
  display: Option<&str>,
  on_change: OnChange,
) -> Result<(Connection, EventQueue<State>, State), PasteError> {
  let conn = connect(display)?;
  let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(werr)?;
  let qh = queue.handle();
  let available =
    globals.contents().with_list(|l| l.iter().any(|g| g.interface == FOREIGN_TOPLEVEL_GLOBAL));
  if !available {
    return Err(PasteError::Unsupported(format!("{FOREIGN_TOPLEVEL_GLOBAL} not advertised")));
  }
  let _mgr: ZwlrForeignToplevelManagerV1 = globals.bind(&qh, 1..=3, ()).map_err(werr)?;
  let mut state = State {
    entries: HashMap::new(),
    next_id: 1,
    active: None,
    reported: None,
    on_change,
    finished: false,
  };
  // Handles, then their initial state + done.
  queue.roundtrip(&mut state).map_err(werr)?;
  queue.roundtrip(&mut state).map_err(werr)?;
  state.report();
  Ok((conn, queue, state))
}

fn run(
  conn: &Connection,
  queue: &mut EventQueue<State>,
  state: &mut State,
  stop: &OwnedFd,
) -> Result<(), PasteError> {
  loop {
    queue.dispatch_pending(state).map_err(werr)?;
    if state.finished {
      return Ok(());
    }
    conn.flush().map_err(werr)?;
    let Some(guard) = queue.prepare_read() else { continue };
    let (wl_ready, stopped) = {
      let wl_fd = guard.connection_fd();
      let mut fds = [PollFd::new(&wl_fd, PollFlags::IN), PollFd::new(stop, PollFlags::IN)];
      match poll(&mut fds, None) {
        Ok(_) => {}
        Err(rustix::io::Errno::INTR) => continue,
        Err(e) => return Err(PasteError::Wayland(format!("poll: {e}"))),
      }
      (!fds[0].revents().is_empty(), !fds[1].revents().is_empty())
    };
    if stopped {
      return Ok(());
    }
    if wl_ready {
      guard.read().map_err(werr)?;
    }
  }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
  fn event(
    _: &mut Self,
    _: &wl_registry::WlRegistry,
    _: wl_registry::Event,
    _: &GlobalListContents,
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
  }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
  fn event(
    state: &mut Self,
    _: &ZwlrForeignToplevelManagerV1,
    event: manager::Event,
    _: &(),
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
    match event {
      manager::Event::Toplevel { toplevel } => {
        let id = format!("wlr-{}", state.next_id);
        state.next_id += 1;
        state.entries.insert(toplevel.id(), Entry { id, ..Default::default() });
      }
      manager::Event::Finished => state.finished = true,
      _ => {}
    }
  }

  event_created_child!(State, ZwlrForeignToplevelManagerV1, [
    manager::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
  ]);
}

/// `state` array -> whether it contains `activated` (native-endian u32s).
fn has_activated(raw: &[u8]) -> bool {
  let activated = handle::State::Activated as u32;
  raw.chunks_exact(4).any(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]) == activated)
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
  fn event(
    state: &mut Self,
    h: &ZwlrForeignToplevelHandleV1,
    event: handle::Event,
    _: &(),
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
    let key = h.id();
    match event {
      handle::Event::AppId { app_id } => {
        if let Some(e) = state.entries.get_mut(&key) {
          let t = app_id.trim();
          let ok = !t.is_empty() && t.len() <= MAX_APP_ID && !t.chars().any(char::is_control);
          e.pending_app_id = Some(ok.then(|| t.to_owned()));
        }
      }
      handle::Event::State { state: raw } => {
        if let Some(e) = state.entries.get_mut(&key) {
          e.pending_activated = Some(has_activated(&raw));
        }
      }
      handle::Event::Done => {
        let Some(e) = state.entries.get_mut(&key) else { return };
        if let Some(a) = e.pending_app_id.take() {
          e.app_id = a;
        }
        if let Some(act) = e.pending_activated.take() {
          e.activated = act;
          if act {
            state.active = Some(key.clone());
          } else if state.active.as_ref() == Some(&key) {
            state.active = None;
          }
        }
        state.report();
      }
      handle::Event::Closed => {
        state.entries.remove(&key);
        if state.active.as_ref() == Some(&key) {
          state.active = None;
        }
        h.destroy();
        state.report();
      }
      // Title, outputs, parent: not needed (titles are never kept).
      _ => {}
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn activated_bit() {
    let enc = |v: &[u32]| v.iter().flat_map(|x| x.to_ne_bytes()).collect::<Vec<u8>>();
    assert!(has_activated(&enc(&[2])));
    assert!(has_activated(&enc(&[0, 2, 3])));
    assert!(!has_activated(&enc(&[0, 1, 3])));
    assert!(!has_activated(&[]));
    assert!(!has_activated(&[2, 0, 0])); // truncated
  }
}
