//! Desktop events, `Show` and `Request::Select` / auto-paste through the
//! orchestrator, with a fake Wayland side, fake picker and fake paste sink.

use std::sync::Mutex as StdMutex;

use spool_paste::PasteChord;

use super::*;
use crate::autopaste::tests::{FakeSink, W1, W2};
use crate::autopaste::{FOCUS_WAIT, PasteSink};
use crate::ipc::{RATE_BURST, RATE_WINDOW};

const UTF8: &str = "text/plain;charset=utf-8";

#[derive(Debug)]
enum Call {
  Fetch(OfferToken),
  Set(Vec<(String, Vec<u8>)>),
}

struct FakeWayland(mpsc::UnboundedSender<Call>);

impl WaylandSide for FakeWayland {
  fn fetch(
    &self,
    offer: OfferToken,
    _mimes: Vec<String>,
    _per_rep_cap: usize,
    _total_cap: usize,
    _timeout: Duration,
  ) -> Result<(), WaylandError> {
    let _ = self.0.send(Call::Fetch(offer));
    Ok(())
  }

  fn set_selection(
    &self,
    _selection: Selection,
    reps: Vec<(String, Arc<[u8]>)>,
  ) -> Result<(), WaylandError> {
    let _ = self.0.send(Call::Set(reps.into_iter().map(|(m, d)| (m, d.to_vec())).collect()));
    Ok(())
  }
}

#[derive(Default)]
struct PickerLog {
  shows: Vec<ShowContext>,
  hides: usize,
}

struct FakePicker(Arc<StdMutex<PickerLog>>);

impl PickerLauncher for FakePicker {
  fn show(&mut self, ctx: &ShowContext) -> Result<(), PickerError> {
    self.0.lock().unwrap().shows.push(ctx.clone());
    Ok(())
  }

  fn hide(&mut self) {
    self.0.lock().unwrap().hides += 1;
  }
}

struct H {
  wl: mpsc::UnboundedSender<WaylandEvent>,
  req: mpsc::Sender<Request>,
  desk: mpsc::Sender<CompositorEvent>,
  calls: mpsc::UnboundedReceiver<Call>,
  tracker: ActiveWindowTracker,
  sink: Arc<FakeSink>,
  picker: Arc<StdMutex<PickerLog>>,
  _stop: oneshot::Sender<()>,
  offers: u64,
}

struct Opts {
  config: Config,
  sink: bool,
  focus: bool,
  limiter: Option<ShowLimiter>,
}

impl Default for Opts {
  fn default() -> Self {
    Self { config: Config::default(), sink: true, focus: true, limiter: None }
  }
}

impl H {
  fn start(o: Opts) -> Self {
    let (ctx, crx) = mpsc::unbounded_channel();
    let store = Store::open_in_memory().unwrap();
    let tracker = ActiveWindowTracker::new();
    let sink = Arc::new(FakeSink::default());
    let (desk, events) = mpsc::channel(16);
    let mut autopaste = AutoPaste::unavailable(&o.config);
    if o.sink {
      autopaste.sink = Some(sink.clone() as Arc<dyn PasteSink>);
    }
    let focus = o.focus.then(|| tracker.clone());
    autopaste.focus = focus.clone();
    let caps = spool_proto::Capabilities {
      compositor: "kwin".into(),
      hotkey: "kwin-script".into(),
      focus: "kwin-script".into(),
      cursor: true,
      auto_paste: autopaste.available(),
      paste_backend: "fake".into(),
    };
    let link = DesktopLink { events, focus, autopaste, capabilities: watch::channel(caps).1 };
    let picker = Arc::new(StdMutex::new(PickerLog::default()));
    let mut orch = Orchestrator::new(o.config, store, FakeWayland(ctx))
      .unwrap()
      .with_desktop(link)
      .with_picker(Box::new(FakePicker(picker.clone())));
    if let Some(l) = o.limiter {
      orch = orch.with_show_limiter(l);
    }
    let (wl_tx, wl_rx) = mpsc::unbounded_channel();
    let (req_tx, req_rx) = mpsc::channel(REQUEST_QUEUE);
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    tokio::spawn(orch.run(wl_rx, req_rx, async move {
      let _ = stop_rx.await;
    }));
    Self {
      wl: wl_tx,
      req: req_tx,
      desk,
      calls: crx,
      tracker,
      sink,
      picker,
      _stop: stop_tx,
      offers: 100,
    }
  }

  async fn ask(&self, req: PublicReq) -> PublicResp {
    let (reply, rx) = oneshot::channel();
    let peer = PeerCred { pid: Some(1), uid: 0, gid: 0 };
    self.req.send(Request::Public { req, peer, reply }).await.unwrap();
    rx.await.unwrap()
  }

  async fn sync(&self) -> StatusInfo {
    match self.ask(PublicReq::Status).await {
      PublicResp::Status(s) => s,
      other => panic!("unexpected {other:?}"),
    }
  }

  async fn next_call(&mut self) -> Call {
    tokio::time::timeout(Duration::from_secs(5), self.calls.recv())
      .await
      .expect("timed out waiting for a wayland call")
      .unwrap()
  }

  fn no_calls(&mut self) {
    assert!(self.calls.try_recv().is_err(), "unexpected wayland call");
  }

  fn offer(&mut self, ours: bool) -> u64 {
    self.offers += 1;
    self
      .wl
      .send(WaylandEvent::NewSelection {
        selection: Selection::Clipboard,
        mimes: vec![UTF8.into()],
        offer: OfferToken::from_raw(self.offers),
        ours,
      })
      .unwrap();
    self.offers
  }

  /// `spoolctl copy`, plus the compositor's confirmation; returns the id.
  async fn copy(&mut self, text: &str) -> ItemId {
    let r = self.ask(PublicReq::Copy { mime: UTF8.into(), data: text.as_bytes().to_vec() }).await;
    assert_eq!(r, PublicResp::Ok);
    let Call::Set(_) = self.next_call().await else { panic!("expected set") };
    self.offer(true);
    let (reply, rx) = oneshot::channel();
    let q = Request::Search {
      q: String::new(),
      filters: QueryFilters::default(),
      offset: 0,
      limit: 1,
      reply,
    };
    self.req.send(q).await.unwrap();
    ItemId(rx.await.unwrap().unwrap()[0].id)
  }

  fn focus(&self, app: &str, w: &str) {
    self.tracker.update(Some(app.into()), Some(w.into()), std::time::Instant::now());
  }

  async fn show(&self, app: &str, w: &str) {
    self
      .desk
      .send(CompositorEvent::Show {
        cursor: Some((10, 20)),
        app_id: Some(app.into()),
        window_id: Some(w.into()),
      })
      .await
      .unwrap();
    self.sync().await;
  }

  async fn select(&self, id: ItemId, mode: SelectMode) -> oneshot::Receiver<SelectResult> {
    let (reply, rx) = oneshot::channel();
    self.req.send(Request::Select { id, mode, reply }).await.unwrap();
    rx
  }

  fn chords(&self) -> Vec<PasteChord> {
    self.sink.chords.lock().unwrap().clone()
  }
}

type SelectResult = Result<SelectOutcome, SelectError>;

async fn outcome(rx: oneshot::Receiver<SelectResult>) -> SelectResult {
  tokio::time::timeout(Duration::from_secs(5), rx).await.expect("select reply").unwrap()
}

#[tokio::test]
async fn active_window_becomes_source_app() {
  let config =
    Config { excluded_apps: vec!["org.keepassxc.KeePassXC".into()], ..Config::default() };
  let mut h = H::start(Opts { config, ..Opts::default() });
  // An excluded app is focused: its offer is skipped (never fetched).
  h.focus("org.keepassxc.KeePassXC", W1);
  h.desk
    .send(CompositorEvent::ActiveWindow {
      app_id: Some("org.keepassxc.KeePassXC".into()),
      window_id: Some(W1.into()),
      at: std::time::Instant::now(),
    })
    .await
    .unwrap();
  h.offer(false);
  h.sync().await;
  h.no_calls();
  // Another app: fetched and stored with that source app.
  h.focus("org.kde.kate", W2);
  let n = h.offer(false);
  let Call::Fetch(offer) = h.next_call().await else { panic!("expected fetch") };
  assert_eq!(offer.raw(), n);
  h.wl
    .send(WaylandEvent::Fetched { offer, result: Ok(vec![(UTF8.into(), b"hi".to_vec())]) })
    .unwrap();
  assert_eq!(h.sync().await.item_count, 1);
  let (reply, rx) = oneshot::channel();
  let q = Request::Search {
    q: String::new(),
    filters: QueryFilters::default(),
    offset: 0,
    limit: 5,
    reply,
  };
  h.req.send(q).await.unwrap();
  let items = rx.await.unwrap().unwrap();
  assert_eq!(items[0].source_app.as_deref(), Some("org.kde.kate"));
}

#[tokio::test]
async fn no_focus_source_means_no_source_app() {
  let config =
    Config { excluded_apps: vec!["org.keepassxc.KeePassXC".into()], ..Config::default() };
  let mut h = H::start(Opts { config, focus: false, ..Opts::default() });
  h.focus("org.keepassxc.KeePassXC", W1);
  h.offer(false);
  let Call::Fetch(_) = h.next_call().await else { panic!("expected fetch") };
}

#[tokio::test]
async fn show_is_rate_limited_and_shares_the_socket_budget() {
  let limiter = crate::ipc::show_limiter();
  let h = H::start(Opts { limiter: Some(limiter.clone()), ..Opts::default() });
  for _ in 0..RATE_BURST + 3 {
    h.show("org.kde.kate", W1).await;
  }
  {
    let log = h.picker.lock().unwrap();
    assert_eq!(log.shows.len(), RATE_BURST as usize);
    let s = &log.shows[0];
    assert_eq!(s.origin, ShowOrigin::Hotkey);
    assert_eq!(s.cursor, Some((10, 20)));
    assert_eq!(
      s.target,
      Some(PasteTarget { window_id: W1.into(), app_id: Some("org.kde.kate".into()) })
    );
  }
  // The socket server draws from the same bucket: it is empty now.
  assert!(!limiter.lock().unwrap().allow(std::time::Instant::now()));
  // After the window the budget is back.
  tokio::time::sleep(RATE_WINDOW / RATE_BURST + Duration::from_millis(50)).await;
  h.show("org.kde.kate", W1).await;
  assert_eq!(h.picker.lock().unwrap().shows.len(), RATE_BURST as usize + 1);
}

#[tokio::test]
async fn socket_show_targets_the_active_window() {
  let h = H::start(Opts::default());
  h.focus("org.kde.konsole", W2);
  assert_eq!(h.ask(PublicReq::Show).await, PublicResp::Ok);
  let log = h.picker.lock().unwrap();
  assert_eq!(log.shows[0].origin, ShowOrigin::Socket);
  assert_eq!(log.shows[0].cursor, None);
  assert_eq!(
    log.shows[0].target,
    Some(PasteTarget { window_id: W2.into(), app_id: Some("org.kde.konsole".into()) })
  );
}

#[tokio::test]
async fn paste_happy_path_and_terminal_chord() {
  let mut h = H::start(Opts::default());
  let id = h.copy("hello").await;
  let before = h.sync().await.item_count;

  // Kate: Ctrl+V, only after our publish is confirmed.
  h.focus("org.kde.kate", W1);
  h.show("org.kde.kate", W1).await;
  let mut rx = h.select(id, SelectMode::Paste).await;
  let Call::Set(reps) = h.next_call().await else { panic!("expected set") };
  assert!(reps.iter().any(|(m, d)| m == UTF8 && d == b"hello"));
  h.sync().await;
  assert!(rx.try_recv().is_err(), "must wait for the compositor's confirmation");
  assert!(h.chords().is_empty());
  h.offer(true);
  assert_eq!(outcome(rx).await, Ok(SelectOutcome::Pasted { chord: PasteChord::CtrlV }));
  assert_eq!(h.chords(), vec![PasteChord::CtrlV]);
  assert_eq!(h.picker.lock().unwrap().hides, 1);
  assert_eq!(h.sync().await.item_count, before, "select never adds items");

  // Konsole: Ctrl+Shift+V.
  h.focus("org.kde.konsole", W2);
  h.show("org.kde.konsole", W2).await;
  let rx = h.select(id, SelectMode::PastePlain).await;
  let Call::Set(_) = h.next_call().await else { panic!("expected set") };
  h.offer(true);
  assert_eq!(outcome(rx).await, Ok(SelectOutcome::Pasted { chord: PasteChord::CtrlShiftV }));

  // The target was consumed: another paste without a new Show has none.
  let rx = h.select(id, SelectMode::Paste).await;
  let Call::Set(_) = h.next_call().await else { panic!("expected set") };
  assert_eq!(outcome(rx).await, Ok(SelectOutcome::NotPasted(NoPaste::NoTarget)));
  assert_eq!(h.chords(), vec![PasteChord::CtrlV, PasteChord::CtrlShiftV]);
}

#[tokio::test]
async fn foreign_offer_before_confirmation_aborts() {
  let mut h = H::start(Opts::default());
  let id = h.copy("hello").await;
  h.focus("org.kde.kate", W1);
  h.show("org.kde.kate", W1).await;
  let rx = h.select(id, SelectMode::Paste).await;
  let Call::Set(_) = h.next_call().await else { panic!("expected set") };
  // Another app copies before our publish shows up.
  h.offer(false);
  assert_eq!(outcome(rx).await, Ok(SelectOutcome::NotPasted(NoPaste::ForeignOffer)));
  // The foreign offer is captured as usual; nothing re-publishes over it.
  let Call::Fetch(_) = h.next_call().await else { panic!("expected fetch") };
  h.offer(true); // our (late) publish
  h.sync().await;
  h.no_calls();
  assert!(h.chords().is_empty());
}

#[tokio::test]
async fn older_confirmation_does_not_count() {
  let mut h = H::start(Opts::default());
  let id = h.copy("one").await;
  // A keep-alive / copy publish whose confirmation is still in flight ...
  let r = h.ask(PublicReq::Copy { mime: UTF8.into(), data: b"two".to_vec() }).await;
  assert_eq!(r, PublicResp::Ok);
  let Call::Set(_) = h.next_call().await else { panic!("expected set") };
  h.focus("org.kde.kate", W1);
  h.show("org.kde.kate", W1).await;
  let mut rx = h.select(id, SelectMode::Paste).await;
  let Call::Set(_) = h.next_call().await else { panic!("expected set") };
  // ... its confirmation is not ours.
  h.offer(true);
  h.sync().await;
  assert!(rx.try_recv().is_err());
  h.offer(true);
  assert_eq!(outcome(rx).await, Ok(SelectOutcome::Pasted { chord: PasteChord::CtrlV }));
}

#[tokio::test]
async fn unconfirmed_publish_times_out_and_resyncs() {
  let mut h = H::start(Opts::default());
  let id = h.copy("hello").await;
  h.focus("org.kde.kate", W1);
  h.show("org.kde.kate", W1).await;
  let rx = h.select(id, SelectMode::Paste).await;
  let Call::Set(_) = h.next_call().await else { panic!("expected set") };
  // No confirmation at all.
  assert_eq!(outcome(rx).await, Ok(SelectOutcome::NotPasted(NoPaste::NotConfirmed)));
  // The lost confirmation does not poison the next paste.
  h.show("org.kde.kate", W1).await;
  let rx = h.select(id, SelectMode::Paste).await;
  let Call::Set(_) = h.next_call().await else { panic!("expected set") };
  h.offer(true);
  assert_eq!(outcome(rx).await, Ok(SelectOutcome::Pasted { chord: PasteChord::CtrlV }));
}

#[tokio::test]
async fn focus_wait_timeout_does_not_paste() {
  let mut h = H::start(Opts::default());
  let id = h.copy("hello").await;
  h.focus("org.kde.kate", W1);
  h.show("org.kde.kate", W1).await;
  // The picker (or anything else) keeps focus.
  h.focus("spool-picker", W2);
  let rx = h.select(id, SelectMode::Paste).await;
  let Call::Set(_) = h.next_call().await else { panic!("expected set") };
  let start = std::time::Instant::now();
  h.offer(true);
  assert_eq!(outcome(rx).await, Ok(SelectOutcome::NotPasted(NoPaste::FocusTimeout)));
  assert!(start.elapsed() >= FOCUS_WAIT);
  assert!(h.chords().is_empty());
}

#[tokio::test]
async fn no_target_no_backend_no_focus_never_paste() {
  // No Show at all -> no target.
  let mut h = H::start(Opts::default());
  let id = h.copy("hello").await;
  h.focus("org.kde.kate", W1);
  let rx = h.select(id, SelectMode::Paste).await;
  let Call::Set(_) = h.next_call().await else { panic!("expected set") };
  assert_eq!(outcome(rx).await, Ok(SelectOutcome::NotPasted(NoPaste::NoTarget)));
  // A Show without a window id (nothing focused) -> no target either.
  h.desk.send(CompositorEvent::Show { cursor: None, app_id: None, window_id: None }).await.unwrap();
  h.sync().await;
  let rx = h.select(id, SelectMode::Paste).await;
  let Call::Set(_) = h.next_call().await else { panic!("expected set") };
  assert_eq!(outcome(rx).await, Ok(SelectOutcome::NotPasted(NoPaste::NoTarget)));
  assert!(h.chords().is_empty());

  // No paste backend (PasteHandle::spawn failed).
  let mut h = H::start(Opts { sink: false, ..Opts::default() });
  let id = h.copy("hello").await;
  h.focus("org.kde.kate", W1);
  h.show("org.kde.kate", W1).await;
  let rx = h.select(id, SelectMode::Paste).await;
  let Call::Set(_) = h.next_call().await else { panic!("expected set") };
  assert_eq!(outcome(rx).await, Ok(SelectOutcome::NotPasted(NoPaste::NoBackend)));

  // No focus source.
  let mut h = H::start(Opts { focus: false, ..Opts::default() });
  let id = h.copy("hello").await;
  h.show("org.kde.kate", W1).await;
  let rx = h.select(id, SelectMode::Paste).await;
  let Call::Set(_) = h.next_call().await else { panic!("expected set") };
  assert_eq!(outcome(rx).await, Ok(SelectOutcome::NotPasted(NoPaste::NoFocusSource)));

  // auto_paste = false.
  let config = Config { auto_paste: false, ..Config::default() };
  let mut h = H::start(Opts { config, ..Opts::default() });
  let id = h.copy("hello").await;
  h.focus("org.kde.kate", W1);
  h.show("org.kde.kate", W1).await;
  let rx = h.select(id, SelectMode::Paste).await;
  let Call::Set(_) = h.next_call().await else { panic!("expected set") };
  assert_eq!(outcome(rx).await, Ok(SelectOutcome::NotPasted(NoPaste::Disabled)));
  assert!(h.chords().is_empty());
}

#[tokio::test]
async fn copy_mode_touches_and_publishes() {
  let mut h = H::start(Opts::default());
  let id = h.copy("hello").await;
  let used = |h: &H| {
    let (reply, rx) = oneshot::channel();
    let q = Request::Search {
      q: String::new(),
      filters: QueryFilters::default(),
      offset: 0,
      limit: 1,
      reply,
    };
    let req = h.req.clone();
    async move {
      req.send(q).await.unwrap();
      rx.await.unwrap().unwrap()[0].last_used_unix_ms
    }
  };
  let before = used(&h).await;
  tokio::time::sleep(Duration::from_millis(5)).await;
  let rx = h.select(id, SelectMode::Copy).await;
  assert_eq!(outcome(rx).await, Ok(SelectOutcome::Copied));
  let Call::Set(reps) = h.next_call().await else { panic!("expected set") };
  assert!(reps.iter().any(|(m, d)| m == UTF8 && d == b"hello"));
  assert!(used(&h).await > before, "last_used_at bumped");
  assert!(h.chords().is_empty());

  let rx = h.select(ItemId(9999), SelectMode::Copy).await;
  assert_eq!(outcome(rx).await, Err(SelectError::NotFound));
  h.sync().await;
  h.no_calls();
}

#[tokio::test]
async fn status_reports_capabilities() {
  let h = H::start(Opts::default());
  let caps = h.sync().await.capabilities.expect("capabilities");
  assert_eq!(caps.hotkey, "kwin-script");
  assert!(caps.auto_paste);
}
