//! The Wayland thread: connection setup, protocol dispatch and the poll loop.

use std::collections::HashSet;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use rustix::event::{PollFd, PollFlags, poll};
use rustix::io::Errno;
use rustix::pipe::{PipeFlags, pipe_with};
use wayland_client::backend::WaylandError as BackendError;
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child};
use wayland_protocols::ext::data_control::v1::client::{
  ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
  ext_data_control_manager_v1::ExtDataControlManagerV1,
  ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
  ext_data_control_source_v1::{self, ExtDataControlSourceV1},
};

use crate::{
  CompositorInfo, DATA_CONTROL_GLOBAL, FetchError, FetchResult, OfferToken, Selection,
  WaylandConfig, WaylandError, WaylandEvent, is_marker_mime, marker_mime, pipes,
};

/// Deadline for serving one `send` request to a paste client.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum concurrent `send` writer threads; further requests are refused
/// (fd closed, i.e. the paster sees an empty body).
pub const MAX_CONCURRENT_SENDS: usize = 32;
/// Maximum concurrent fetch reader threads; further fetches fail with `Io`.
pub const MAX_CONCURRENT_FETCHES: usize = 16;
/// Mimes beyond this many per offer are ignored (hostile sources).
const MAX_MIMES_PER_OFFER: usize = 256;
/// Mime strings longer than this are ignored.
const MAX_MIME_LEN: usize = 1024;

pub(crate) enum Cmd {
  Fetch {
    offer: OfferToken,
    mimes: Vec<String>,
    per_rep_cap: usize,
    total_cap: usize,
    timeout: Duration,
  },
  SetSelection {
    selection: Selection,
    reps: Vec<(String, Arc<[u8]>)>,
  },
  Shutdown,
}

/// Cross-thread wakeup (an eventfd) for the poll loop.
#[derive(Debug)]
pub(crate) struct Waker(OwnedFd);

impl Waker {
  fn new() -> std::io::Result<Self> {
    use rustix::event::{EventfdFlags, eventfd};
    Ok(Self(eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?))
  }

  pub(crate) fn wake(&self) {
    // EAGAIN only if the counter is saturated, which still means "readable".
    let _ = rustix::io::write(&self.0, &1u64.to_ne_bytes());
  }

  fn drain(&self) {
    let mut buf = [0u8; 8];
    let _ = rustix::io::read(&self.0, &mut buf[..]);
  }
}

// ---- object user data -------------------------------------------------------

#[derive(Default)]
pub(crate) struct OfferData {
  mimes: Mutex<Vec<String>>,
}

pub(crate) struct SourceData {
  selection: Selection,
  nonce: String,
  reps: Vec<(String, Arc<[u8]>)>,
}

struct Current {
  token: OfferToken,
  offer: ExtDataControlOfferV1,
}

fn idx(sel: Selection) -> usize {
  match sel {
    Selection::Clipboard => 0,
    Selection::Primary => 1,
  }
}

// ---- state ----------------------------------------------------------------

pub(crate) struct State {
  events: Sender<WaylandEvent>,
  watch_primary: bool,
  primary_supported: bool,
  globals: Vec<(u32, String, u32)>,
  setup_done: bool,
  manager: Option<ExtDataControlManagerV1>,
  seat: Option<(u32, wl_seat::WlSeat)>,
  device: Option<ExtDataControlDeviceV1>,
  current: [Option<Current>; 2],
  next_token: u64,
  sources: Vec<ExtDataControlSourceV1>,
  /// Nonces of our sources that have not been cancelled, per selection.
  live_nonces: [HashSet<String>; 2],
  active_sends: Arc<AtomicUsize>,
  active_fetches: Arc<AtomicUsize>,
}

impl State {
  fn emit(&self, ev: WaylandEvent) {
    // A dropped receiver just means nobody listens any more.
    let _ = self.events.send(ev);
  }

  fn bind_seat(&mut self, registry: &wl_registry::WlRegistry, qh: &QueueHandle<State>) {
    if self.seat.is_some() {
      return;
    }
    let Some((name, _, version)) = self.globals.iter().find(|(_, i, _)| i == "wl_seat").cloned()
    else {
      tracing::warn!("no wl_seat yet; waiting for one to appear");
      return;
    };
    let seat: wl_seat::WlSeat = registry.bind(name, version.min(5), qh, ());
    if self.globals.iter().filter(|(_, i, _)| i == "wl_seat").count() > 1 {
      tracing::warn!("multiple seats; Spool only follows the first one");
    }
    if let Some(manager) = &self.manager {
      self.device = Some(manager.get_data_device(&seat, qh, ()));
      tracing::debug!(seat = name, "data-control device created");
    }
    self.seat = Some((name, seat));
  }

  fn drop_device(&mut self) {
    for c in self.current.iter_mut().filter_map(Option::take) {
      c.offer.destroy();
    }
    if let Some(d) = self.device.take() {
      d.destroy();
    }
  }

  fn on_selection(&mut self, sel: Selection, offer: Option<ExtDataControlOfferV1>) {
    if let Some(prev) = self.current[idx(sel)].take() {
      if offer.as_ref() != Some(&prev.offer) {
        prev.offer.destroy();
      }
    }
    let Some(offer) = offer else {
      tracing::debug!(selection = sel.as_str(), "selection cleared");
      self.emit(WaylandEvent::SelectionCleared { selection: sel });
      return;
    };
    let all = offer
      .data::<OfferData>()
      .map(|d| std::mem::take(&mut *d.mimes.lock().unwrap_or_else(|e| e.into_inner())))
      .unwrap_or_default();
    let mut ours = false;
    let mut mimes = Vec::with_capacity(all.len());
    for m in all {
      if is_marker_mime(&m) {
        let nonce = &m[crate::MARKER_MIME_PREFIX.len()..];
        ours |= self.live_nonces[idx(sel)].contains(nonce);
      } else if !mimes.contains(&m) {
        mimes.push(m);
      }
    }
    self.next_token += 1;
    let token = OfferToken(self.next_token);
    tracing::debug!(
      selection = sel.as_str(),
      token = token.0,
      n_mimes = mimes.len(),
      ours,
      "new selection"
    );
    self.current[idx(sel)] = Some(Current { token, offer });
    self.emit(WaylandEvent::NewSelection { selection: sel, mimes, offer: token, ours });
  }

  fn find_offer(&self, token: OfferToken) -> Option<&ExtDataControlOfferV1> {
    self.current.iter().flatten().find(|c| c.token == token).map(|c| &c.offer)
  }
}

// ---- dispatch -------------------------------------------------------------

impl Dispatch<wl_registry::WlRegistry, ()> for State {
  fn event(
    state: &mut Self,
    registry: &wl_registry::WlRegistry,
    event: wl_registry::Event,
    _: &(),
    _: &Connection,
    qh: &QueueHandle<Self>,
  ) {
    match event {
      wl_registry::Event::Global { name, interface, version } => {
        let is_seat = interface == "wl_seat";
        state.globals.push((name, interface, version));
        if is_seat && state.setup_done && state.seat.is_none() {
          tracing::info!("wl_seat appeared");
          state.bind_seat(registry, qh);
        }
      }
      wl_registry::Event::GlobalRemove { name } => {
        state.globals.retain(|(n, _, _)| *n != name);
        if state.seat.as_ref().is_some_and(|(n, _)| *n == name) {
          tracing::warn!("our wl_seat was removed; clipboard unavailable until a seat appears");
          state.drop_device();
          if let Some((_, seat)) = state.seat.take() {
            if seat.version() >= 5 {
              seat.release();
            }
          }
          if state.setup_done {
            state.bind_seat(registry, qh);
          }
        }
      }
      _ => {}
    }
  }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
  fn event(
    _: &mut Self,
    _: &wl_seat::WlSeat,
    _: wl_seat::Event,
    _: &(),
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
  }
}

impl Dispatch<ExtDataControlManagerV1, ()> for State {
  fn event(
    _: &mut Self,
    _: &ExtDataControlManagerV1,
    _: <ExtDataControlManagerV1 as Proxy>::Event,
    _: &(),
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
  }
}

impl Dispatch<ExtDataControlDeviceV1, ()> for State {
  fn event(
    state: &mut Self,
    device: &ExtDataControlDeviceV1,
    event: ext_data_control_device_v1::Event,
    _: &(),
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
    use ext_data_control_device_v1::Event;
    if state.device.as_ref() != Some(device) {
      return;
    }
    match event {
      Event::DataOffer { .. } => {} // mimes arrive on the offer itself
      Event::Selection { id } => state.on_selection(Selection::Clipboard, id),
      Event::PrimarySelection { id } => {
        if state.watch_primary {
          state.on_selection(Selection::Primary, id);
        } else if let Some(o) = id {
          o.destroy();
        }
      }
      Event::Finished => {
        tracing::warn!("data-control device finished; clipboard unavailable");
        state.drop_device();
      }
      _ => {}
    }
  }

  event_created_child!(State, ExtDataControlDeviceV1, [
    ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, OfferData::default()),
  ]);
}

impl Dispatch<ExtDataControlOfferV1, OfferData> for State {
  fn event(
    _: &mut Self,
    _: &ExtDataControlOfferV1,
    event: ext_data_control_offer_v1::Event,
    data: &OfferData,
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
    if let ext_data_control_offer_v1::Event::Offer { mime_type } = event {
      let mut mimes = data.mimes.lock().unwrap_or_else(|e| e.into_inner());
      if mimes.len() < MAX_MIMES_PER_OFFER && mime_type.len() <= MAX_MIME_LEN {
        mimes.push(mime_type);
      }
    }
  }
}

impl Dispatch<ExtDataControlSourceV1, SourceData> for State {
  fn event(
    state: &mut Self,
    source: &ExtDataControlSourceV1,
    event: ext_data_control_source_v1::Event,
    data: &SourceData,
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
    use ext_data_control_source_v1::Event;
    match event {
      Event::Send { mime_type, fd } => {
        if is_marker_mime(&mime_type) {
          return; // empty body: dropping fd closes it
        }
        let Some((_, bytes)) = data.reps.iter().find(|(m, _)| *m == mime_type) else {
          tracing::debug!(mime = %mime_type, "send for a mime we did not offer");
          return;
        };
        spawn_sender(state, mime_type, fd, bytes.clone());
      }
      Event::Cancelled => {
        tracing::debug!(selection = data.selection.as_str(), "our source was cancelled");
        state.live_nonces[idx(data.selection)].remove(&data.nonce);
        state.sources.retain(|s| s != source);
        source.destroy();
      }
      _ => {}
    }
  }
}

fn spawn_sender(state: &State, mime: String, fd: OwnedFd, bytes: Arc<[u8]>) {
  let active = state.active_sends.clone();
  if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONCURRENT_SENDS {
    active.fetch_sub(1, Ordering::SeqCst);
    tracing::warn!(mime = %mime, "too many concurrent paste requests; refusing one");
    return;
  }
  let spawned = std::thread::Builder::new().name("spool-wl-send".into()).spawn({
    let active = active.clone();
    move || {
      tracing::debug!(mime = %mime, len = bytes.len(), "serving paste");
      pipes::write_all(fd, &bytes, Instant::now() + SEND_TIMEOUT);
      active.fetch_sub(1, Ordering::SeqCst);
    }
  });
  if let Err(e) = spawned {
    active.fetch_sub(1, Ordering::SeqCst);
    tracing::warn!(error = %e, "cannot spawn paste writer");
  }
}

// ---- thread ---------------------------------------------------------------

pub(crate) struct Channels {
  pub cmd_rx: Receiver<Cmd>,
  pub waker: Arc<Waker>,
  pub events: Sender<WaylandEvent>,
}

pub(crate) fn new_waker() -> std::io::Result<Arc<Waker>> {
  Ok(Arc::new(Waker::new()?))
}

fn connect(display: Option<&str>) -> Result<(String, Connection), WaylandError> {
  let name = match display {
    Some(d) => d.to_owned(),
    None => std::env::var("WAYLAND_DISPLAY")
      .ok()
      .filter(|s| !s.is_empty())
      .ok_or_else(|| WaylandError::Connect("WAYLAND_DISPLAY is not set".into()))?,
  };
  let path = PathBuf::from(&name);
  let path = if path.is_absolute() {
    path
  } else {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
      .map(PathBuf::from)
      .filter(|p| p.is_absolute())
      .ok_or_else(|| WaylandError::Connect("XDG_RUNTIME_DIR is not set".into()))?;
    dir.join(&name)
  };
  let stream = UnixStream::connect(&path)
    .map_err(|e| WaylandError::Connect(format!("{}: {e}", path.display())))?;
  let conn = Connection::from_socket(stream).map_err(|e| WaylandError::Connect(e.to_string()))?;
  Ok((name, conn))
}

fn dispatch_err(e: impl std::fmt::Display) -> WaylandError {
  WaylandError::Connect(format!("initial roundtrip failed: {e}"))
}

struct Setup {
  conn: Connection,
  queue: EventQueue<State>,
  state: State,
  info: CompositorInfo,
}

fn setup(cfg: &WaylandConfig, events: Sender<WaylandEvent>) -> Result<Setup, WaylandError> {
  let (display, conn) = connect(cfg.display.as_deref())?;
  let mut queue = conn.new_event_queue::<State>();
  let qh = queue.handle();
  let registry = conn.display().get_registry(&qh, ());
  let mut state = State {
    events,
    watch_primary: cfg.watch_primary,
    primary_supported: false,
    globals: Vec::new(),
    setup_done: false,
    manager: None,
    seat: None,
    device: None,
    current: [None, None],
    next_token: 0,
    sources: Vec::new(),
    live_nonces: [HashSet::new(), HashSet::new()],
    active_sends: Arc::new(AtomicUsize::new(0)),
    active_fetches: Arc::new(AtomicUsize::new(0)),
  };
  queue.roundtrip(&mut state).map_err(dispatch_err)?;

  let Some((name, _, version)) =
    state.globals.iter().find(|(_, i, _)| i == DATA_CONTROL_GLOBAL).cloned()
  else {
    return Err(WaylandError::MissingGlobal(DATA_CONTROL_GLOBAL));
  };
  let version = version.min(1);
  state.manager = Some(registry.bind(name, version, &qh, ()));
  // ext-data-control-v1 always carries primary_selection; whether the
  // compositor ever sends it depends on it implementing primary selection
  // at all, which a primary-selection manager global indicates.
  state.primary_supported = state.globals.iter().any(|(_, i, _)| {
    i == "zwp_primary_selection_device_manager_v1" || i == "gtk_primary_selection_device_manager"
  });
  state.bind_seat(&registry, &qh);
  state.setup_done = true;
  conn.flush().map_err(dispatch_err)?;

  let info = CompositorInfo {
    display,
    data_control_version: version,
    primary_supported: state.primary_supported,
  };
  Ok(Setup { conn, queue, state, info })
}

/// Thread body. Reports setup success/failure through `init` before entering
/// the loop.
pub(crate) fn run(cfg: WaylandConfig, ch: Channels, init: Sender<Result<(), WaylandError>>) {
  let Setup { conn, mut queue, mut state, info } = match setup(&cfg, ch.events.clone()) {
    Ok(s) => s,
    Err(e) => {
      let _ = init.send(Err(e));
      return;
    }
  };
  tracing::info!(
    display = %info.display,
    version = info.data_control_version,
    primary_supported = info.primary_supported,
    "connected to compositor"
  );
  let display = info.display.clone();
  state.emit(WaylandEvent::Ready(info));
  let _ = init.send(Ok(()));
  drop(init);

  let (done_tx, done_rx) = crossbeam_channel::unbounded::<(OfferToken, FetchResult)>();
  let mut lp = Loop { conn, qh: queue.handle(), waker: ch.waker, done_tx };

  match lp.run(&mut queue, &mut state, &ch.cmd_rx, &done_rx) {
    Ok(()) => {
      tracing::info!("wayland thread shutting down");
      for c in state.current.iter_mut().filter_map(Option::take) {
        c.offer.destroy();
      }
      for s in state.sources.drain(..) {
        s.destroy();
      }
      if let Some(d) = state.device.take() {
        d.destroy();
      }
      if let Some(m) = state.manager.take() {
        m.destroy();
      }
      if let Some((_, seat)) = state.seat.take() {
        if seat.version() >= 5 {
          seat.release();
        }
      }
      let _ = lp.conn.flush();
    }
    Err(msg) => {
      let msg = format!("connection to {display} lost: {msg}");
      tracing::error!("{msg}");
      state.emit(WaylandEvent::Fatal(msg));
    }
  }
}

struct Loop {
  conn: Connection,
  qh: QueueHandle<State>,
  waker: Arc<Waker>,
  done_tx: Sender<(OfferToken, FetchResult)>,
}

enum Flow {
  Continue,
  Exit,
}

impl Loop {
  fn run(
    &mut self,
    queue: &mut EventQueue<State>,
    state: &mut State,
    cmd_rx: &Receiver<Cmd>,
    done_rx: &Receiver<(OfferToken, FetchResult)>,
  ) -> Result<(), String> {
    loop {
      // Wayland events first so commands see the freshest offers.
      queue.dispatch_pending(state).map_err(|e| e.to_string())?;

      while let Ok((offer, result)) = done_rx.try_recv() {
        state.emit(WaylandEvent::Fetched { offer, result });
      }
      loop {
        match cmd_rx.try_recv() {
          Ok(cmd) => {
            if let Flow::Exit = self.handle(state, cmd) {
              return Ok(());
            }
          }
          Err(TryRecvError::Empty) => break,
          // Every WaylandHandle is gone: nobody can talk to us any more.
          Err(TryRecvError::Disconnected) => return Ok(()),
        }
      }
      // Requests made by command handling may have produced nothing to
      // dispatch, but they must reach the compositor.
      let want_write = match self.conn.flush() {
        Ok(()) => false,
        Err(BackendError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => true,
        Err(e) => return Err(e.to_string()),
      };

      let Some(guard) = queue.prepare_read() else {
        continue; // events already queued: dispatch them
      };
      let (conn_ready, wake_ready) = {
        let mut flags = PollFlags::IN;
        if want_write {
          flags |= PollFlags::OUT;
        }
        let mut fds = [
          PollFd::from_borrowed_fd(guard.connection_fd(), flags),
          PollFd::new(&self.waker.0, PollFlags::IN),
        ];
        match poll(&mut fds, None) {
          Ok(_) => {}
          Err(Errno::INTR) => continue,
          Err(e) => return Err(format!("poll: {e}")),
        }
        let c = fds[0].revents();
        (
          c.intersects(PollFlags::IN | PollFlags::ERR | PollFlags::HUP),
          fds[1].revents().contains(PollFlags::IN),
        )
      };
      if conn_ready {
        match guard.read() {
          Ok(_) => {}
          Err(BackendError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
          Err(e) => return Err(e.to_string()),
        }
      } else {
        drop(guard);
      }
      if wake_ready {
        self.waker.drain();
      }
    }
  }

  fn handle(&mut self, state: &mut State, cmd: Cmd) -> Flow {
    match cmd {
      Cmd::Shutdown => return Flow::Exit,
      Cmd::Fetch { offer, mimes, per_rep_cap, total_cap, timeout } => {
        self.fetch(state, offer, mimes, per_rep_cap, total_cap, timeout)
      }
      Cmd::SetSelection { selection, reps } => self.set_selection(state, selection, reps),
    }
    Flow::Continue
  }

  fn fetch(
    &mut self,
    state: &mut State,
    token: OfferToken,
    mimes: Vec<String>,
    per_rep_cap: usize,
    total_cap: usize,
    timeout: Duration,
  ) {
    let deadline = Instant::now() + timeout;
    let Some(offer) = state.find_offer(token) else {
      tracing::debug!(token = token.0, "fetch of stale offer");
      state.emit(WaylandEvent::Fetched { offer: token, result: Err(FetchError::OfferGone) });
      return;
    };
    let active = state.active_fetches.clone();
    if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONCURRENT_FETCHES {
      active.fetch_sub(1, Ordering::SeqCst);
      let err = FetchError::Io("too many concurrent fetches".into());
      state.emit(WaylandEvent::Fetched { offer: token, result: Err(err) });
      return;
    }

    let mut seen = HashSet::new();
    let mut reads = Vec::with_capacity(mimes.len());
    for mime in mimes {
      if is_marker_mime(&mime) || !seen.insert(mime.clone()) {
        continue;
      }
      let (r, w) = match pipe_with(PipeFlags::CLOEXEC) {
        Ok(p) => p,
        Err(e) => {
          active.fetch_sub(1, Ordering::SeqCst);
          let err = FetchError::Io(format!("pipe: {e}"));
          state.emit(WaylandEvent::Fetched { offer: token, result: Err(err) });
          return;
        }
      };
      // The backend dup()s the fd when serialising, so closing our write end
      // right away is safe and required (the source's close is our EOF).
      offer.receive(mime.clone(), w.as_fd());
      drop(w);
      reads.push((mime, r));
    }
    if let Err(e) = self.conn.flush() {
      // WouldBlock: the main loop retries; anything else surfaces there too.
      tracing::debug!(error = %e, "flush after receive");
    }
    tracing::debug!(token = token.0, n_mimes = reads.len(), "fetch started");

    let done_tx = self.done_tx.clone();
    let waker = self.waker.clone();
    let spawned = std::thread::Builder::new().name("spool-wl-fetch".into()).spawn({
      let active = active.clone();
      move || {
        let result = pipes::read_reps(reads, per_rep_cap, total_cap, deadline);
        match &result {
          Ok(reps) => tracing::debug!(
            token = token.0,
            n_reps = reps.len(),
            bytes = reps.iter().map(|(_, b)| b.len()).sum::<usize>(),
            "fetch done"
          ),
          Err(e) => tracing::debug!(token = token.0, error = %e, "fetch failed"),
        }
        active.fetch_sub(1, Ordering::SeqCst);
        if done_tx.send((token, result)).is_ok() {
          waker.wake();
        }
      }
    });
    if let Err(e) = spawned {
      active.fetch_sub(1, Ordering::SeqCst);
      let err = FetchError::Io(format!("spawn: {e}"));
      state.emit(WaylandEvent::Fetched { offer: token, result: Err(err) });
    }
  }

  fn set_selection(&mut self, state: &mut State, sel: Selection, reps: Vec<(String, Arc<[u8]>)>) {
    if sel == Selection::Primary && !(state.watch_primary && state.primary_supported) {
      tracing::info!("ignoring set_selection(primary): primary selection disabled or unsupported");
      return;
    }
    let (Some(device), Some(manager)) = (&state.device, &state.manager) else {
      tracing::warn!("set_selection without a data-control device (no seat?)");
      return;
    };
    let mut seen = HashSet::new();
    let reps: Vec<_> =
      reps.into_iter().filter(|(m, _)| !is_marker_mime(m) && seen.insert(m.clone())).collect();
    if reps.is_empty() {
      tracing::debug!(selection = sel.as_str(), "set_selection with no reps: clearing");
      match sel {
        Selection::Clipboard => device.set_selection(None),
        Selection::Primary => device.set_primary_selection(None),
      }
      return;
    }
    let nonce = new_nonce();
    let marker = marker_mime(&nonce);
    let mimes: Vec<String> = reps.iter().map(|(m, _)| m.clone()).collect();
    let n_bytes: usize = reps.iter().map(|(_, b)| b.len()).sum();
    let source = manager
      .create_data_source(&self.qh, SourceData { selection: sel, nonce: nonce.clone(), reps });
    for m in mimes.iter() {
      source.offer(m.clone());
    }
    source.offer(marker);
    match sel {
      Selection::Clipboard => device.set_selection(Some(&source)),
      Selection::Primary => device.set_primary_selection(Some(&source)),
    }
    tracing::debug!(
      selection = sel.as_str(),
      n_mimes = mimes.len(),
      bytes = n_bytes,
      "took selection"
    );
    state.live_nonces[idx(sel)].insert(nonce);
    state.sources.push(source);
  }
}

fn new_nonce() -> String {
  use rand::Rng;
  format!("{:032x}", rand::rng().random::<u128>())
}
