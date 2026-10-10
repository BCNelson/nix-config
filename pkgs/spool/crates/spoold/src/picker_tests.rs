//! Picker protocol v3 through the real [`ResidentPicker`] host and the
//! orchestrator, with a fake picker on an in-process socketpair (the
//! [`Spawner`] seam), a fake Wayland side, focus tracker and paste sink,
//! and fake unlock providers over a real `keyslots.json` + encrypted store.

use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::Path;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use spool_core::config::{PickerRenderer, PickerSettings};
use spool_crypto::{DataKey, Zeroizing};
use spool_keys::{Kek, KeyProvider, KeySlots, ProviderKind, SlotId, SlotParams};
use spool_paste::PasteChord;
use spool_proto::{
  CursorPos, Hello, PickerEvt, PickerReq, PreviewKind, UnlockFailReason, UnlockPrompt,
  UnlockProvider, UnlockSecret, read_frame_async, write_frame_async,
};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Notify;

use super::*;
use crate::autopaste::PasteSink;
use crate::autopaste::tests::{FakeSink, W1};
use crate::keyflow::OpenGate;
use crate::picker::{HostDeps, ResidentPicker, Spawned, Spawner, UnlockCtx};
use crate::unlock::UnlockProviders;

const UTF8: &str = "text/plain;charset=utf-8";
const T: Duration = Duration::from_secs(10);

// ---- fakes ---------------------------------------------------------------

#[derive(Debug)]
enum Call {
  Fetch,
  Set(Vec<(String, Vec<u8>)>),
}

struct FakeWayland(mpsc::UnboundedSender<Call>);

impl WaylandSide for FakeWayland {
  fn fetch(
    &self,
    _offer: OfferToken,
    _mimes: Vec<String>,
    _per_rep_cap: usize,
    _total_cap: usize,
    _timeout: Duration,
  ) -> Result<(), WaylandError> {
    let _ = self.0.send(Call::Fetch);
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

/// Hands the picker end of every "launch" to the test.
struct FakeSpawner(mpsc::UnboundedSender<(StdUnixStream, PickerRenderer)>);

impl Spawner for FakeSpawner {
  fn spawn(&mut self, renderer: PickerRenderer) -> std::io::Result<Spawned> {
    let (ours, theirs) = StdUnixStream::pair()?;
    let _ = self.0.send((theirs, renderer));
    Ok(Spawned { sock: ours, child: None })
  }
}

/// The test's picker: speaks v3 like `spool-picker`.
struct FakePicker {
  rd: OwnedReadHalf,
  wr: OwnedWriteHalf,
  renderer: PickerRenderer,
}

impl FakePicker {
  async fn launch(
    conns: &mut mpsc::UnboundedReceiver<(StdUnixStream, PickerRenderer)>,
    proto: u16,
  ) -> Self {
    let (sock, renderer) = Self::accept(conns).await;
    Self::handshake(sock, renderer, proto).await
  }

  /// Wait for the host to launch a picker.
  async fn accept(
    conns: &mut mpsc::UnboundedReceiver<(StdUnixStream, PickerRenderer)>,
  ) -> (StdUnixStream, PickerRenderer) {
    tokio::time::timeout(T, conns.recv()).await.expect("picker launch").expect("spawner gone")
  }

  async fn handshake(sock: StdUnixStream, renderer: PickerRenderer, proto: u16) -> Self {
    sock.set_nonblocking(true).unwrap();
    let (mut rd, mut wr) = tokio::net::UnixStream::from_std(sock).unwrap().into_split();
    write_frame_async(&mut wr, &Hello { proto }).await.unwrap();
    let theirs: Hello = read_frame_async(&mut rd).await.unwrap();
    assert!(theirs.is_picker_compatible());
    Self { rd, wr, renderer }
  }

  async fn connect(conns: &mut mpsc::UnboundedReceiver<(StdUnixStream, PickerRenderer)>) -> Self {
    Self::launch(conns, spool_proto::PICKER_PROTO_VERSION).await
  }

  async fn send(&mut self, req: PickerReq) {
    write_frame_async(&mut self.wr, &req).await.unwrap();
  }

  async fn recv(&mut self) -> PickerEvt {
    tokio::time::timeout(T, read_frame_async(&mut self.rd))
      .await
      .expect("timed out waiting for a picker event")
      .expect("picker channel")
  }

  /// Next event matching `f`, skipping the others.
  async fn expect<R>(&mut self, what: &str, mut f: impl FnMut(&PickerEvt) -> Option<R>) -> R {
    let start = std::time::Instant::now();
    loop {
      assert!(start.elapsed() < T, "timed out waiting for {what}");
      let e = self.recv().await;
      if let Some(r) = f(&e) {
        return r;
      }
    }
  }

  async fn query(&mut self, seq: u32, q: &str, offset: u32, limit: u32) {
    self
      .send(PickerReq::Query { seq, q: q.into(), filters: QueryFilters::default(), offset, limit })
      .await;
  }

  async fn page(&mut self, seq: u32) -> (Vec<ItemPreview>, bool) {
    self
      .expect("page", |e| match e {
        PickerEvt::Page { seq: s, items, more, .. } if *s == seq => Some((items.clone(), *more)),
        PickerEvt::Error { seq: Some(s), code, .. } if *s == seq => {
          panic!("query failed: {code:?}")
        }
        _ => None,
      })
      .await
  }

  /// Nothing (but noise) arrives for a moment.
  async fn quiet(&mut self, what: &str) {
    while let Ok(r) = tokio::time::timeout(Duration::from_millis(300), self.recv()).await {
      assert!(matches!(r, PickerEvt::IndexProgress { .. }), "unexpected {r:?} ({what})");
    }
  }
}

// ---- harness -----------------------------------------------------------------

struct PH {
  wl: mpsc::UnboundedSender<WaylandEvent>,
  req: mpsc::Sender<Request>,
  desk: mpsc::Sender<CompositorEvent>,
  calls: mpsc::UnboundedReceiver<Call>,
  conns: mpsc::UnboundedReceiver<(StdUnixStream, PickerRenderer)>,
  tracker: ActiveWindowTracker,
  sink: Arc<FakeSink>,
  key_tx: mpsc::Sender<KeyEvent>,
  _stop: oneshot::Sender<()>,
  offers: u64,
}

struct Setup {
  store: Store,
  settings: PickerSettings,
  /// Locked start (key flow) with these unlock deps.
  unlock: Option<(PathBuf, Arc<dyn UnlockProviders>)>,
}

impl Default for Setup {
  fn default() -> Self {
    Self {
      store: Store::open_in_memory().unwrap(),
      settings: PickerSettings::default(),
      unlock: None,
    }
  }
}

impl PH {
  fn start(s: Setup) -> Self {
    let (ctx, crx) = mpsc::unbounded_channel();
    let tracker = ActiveWindowTracker::new();
    let sink = Arc::new(FakeSink::default());
    let (desk, events) = mpsc::channel(16);
    let config = Config::default();
    let mut autopaste = AutoPaste::unavailable(&config);
    autopaste.sink = Some(sink.clone() as Arc<dyn PasteSink>);
    autopaste.focus = Some(tracker.clone());
    let caps = spool_proto::Capabilities {
      compositor: "kwin".into(),
      hotkey: "kwin-script".into(),
      focus: "kwin-script".into(),
      cursor: true,
      auto_paste: true,
      paste_backend: "fake".into(),
    };
    let link = DesktopLink {
      events,
      focus: Some(tracker.clone()),
      autopaste,
      capabilities: watch::channel(caps).1,
    };
    let (req_tx, req_rx) = mpsc::channel(REQUEST_QUEUE);
    let (key_tx, key_rx) = mpsc::channel(8);
    let mut orch = Orchestrator::new(config, s.store, FakeWayland(ctx)).unwrap().with_desktop(link);
    let unlock = match s.unlock {
      Some((dir, providers)) => {
        orch = orch.with_key_flow(key_rx);
        Some(UnlockCtx { dir, key_tx: key_tx.clone(), gate: OpenGate::default(), providers })
      }
      None => None,
    };
    let (spawn_tx, conns) = mpsc::unbounded_channel();
    let deps = HostDeps { requests: req_tx.clone(), index_status: orch.index_status(), unlock };
    let picker = ResidentPicker::start(Box::new(FakeSpawner(spawn_tx)), deps, &s.settings);
    let orch = orch.with_picker(Box::new(picker));
    let (wl_tx, wl_rx) = mpsc::unbounded_channel();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    tokio::spawn(orch.run(wl_rx, req_rx, async move {
      let _ = stop_rx.await;
    }));
    Self {
      wl: wl_tx,
      req: req_tx,
      desk,
      calls: crx,
      conns,
      tracker,
      sink,
      key_tx,
      _stop: stop_tx,
      offers: 0,
    }
  }

  async fn picker(&mut self) -> FakePicker {
    FakePicker::connect(&mut self.conns).await
  }

  async fn ask(&self, req: PublicReq) -> PublicResp {
    let (reply, rx) = oneshot::channel();
    let peer = PeerCred { pid: Some(1), uid: 0, gid: 0 };
    self.req.send(Request::Public { req, peer, reply }).await.unwrap();
    tokio::time::timeout(T, rx).await.expect("reply").unwrap()
  }

  /// Send a request whose answer comes later (`Pick`).
  async fn ask_later(&self, req: PublicReq) -> oneshot::Receiver<PublicResp> {
    let (reply, rx) = oneshot::channel();
    let peer = PeerCred { pid: Some(1), uid: 0, gid: 0 };
    self.req.send(Request::Public { req, peer, reply }).await.unwrap();
    rx
  }

  async fn status(&self) -> StatusInfo {
    match self.ask(PublicReq::Status).await {
      PublicResp::Status(s) => s,
      other => panic!("unexpected {other:?}"),
    }
  }

  async fn next_call(&mut self) -> Call {
    tokio::time::timeout(T, self.calls.recv()).await.expect("wayland call").unwrap()
  }

  /// `spoolctl copy` + the compositor's confirmation.
  async fn copy(&mut self, mime: &str, data: &[u8]) {
    let r = self.ask(PublicReq::Copy { mime: mime.into(), data: data.to_vec() }).await;
    assert_eq!(r, PublicResp::Ok);
    let Call::Set(_) = self.next_call().await else { panic!("expected set") };
    self.confirm();
  }

  /// Our own publish comes back from the compositor.
  fn confirm(&mut self) {
    self.offers += 1;
    self
      .wl
      .send(WaylandEvent::NewSelection {
        selection: Selection::Clipboard,
        mimes: vec![UTF8.into()],
        offer: OfferToken::from_raw(self.offers),
        ours: true,
      })
      .unwrap();
  }
}

fn text(p: &ItemPreview) -> &str {
  &p.preview
}

// ---- tests: browsing ---------------------------------------------------------------

#[tokio::test]
async fn query_pages_more_and_errors_by_seq() {
  let mut h = PH::start(Setup::default());
  let mut p = h.picker().await;
  for i in 0..5 {
    h.copy(UTF8, format!("item {i}").as_bytes()).await;
  }
  p.query(1, "", 0, 2).await;
  let (items, more) = p.page(1).await;
  assert_eq!(items.iter().map(text).collect::<Vec<_>>(), ["item 4", "item 3"]);
  assert!(more);
  p.query(2, "", 4, 2).await;
  let (items, more) = p.page(2).await;
  assert_eq!(items.iter().map(text).collect::<Vec<_>>(), ["item 0"]);
  assert!(!more);
  // Exactly one answer per seq, even with several in flight, and a bad
  // query is an Error with its seq that never echoes the query.
  let long = "secretword ".repeat(40);
  p.query(3, &long, 0, 50).await;
  p.query(4, "", 0, 500).await;
  p.query(5, "", 0, 0).await;
  let mut answered = std::collections::HashMap::new();
  while answered.len() < 3 {
    match p.recv().await {
      PickerEvt::Page { seq, items, more, .. } => {
        assert!(answered.insert(seq, format!("page {} {more}", items.len())).is_none());
      }
      PickerEvt::Error { seq: Some(seq), code, message } => {
        assert!(!message.contains("secretword"), "{message}");
        assert!(answered.insert(seq, format!("{code:?}")).is_none());
      }
      PickerEvt::NewItem { .. } | PickerEvt::IndexProgress { .. } => {}
      other => panic!("unexpected {other:?}"),
    }
  }
  assert_eq!(answered[&3], "BadQuery");
  assert_eq!(answered[&4], "page 5 false");
  assert_eq!(answered[&5], "page 0 true", "nothing asked for, but more exist");
  p.quiet("after the answers").await;
}

#[tokio::test]
async fn new_items_are_pushed() {
  let mut h = PH::start(Setup::default());
  let mut p = h.picker().await;
  h.copy(UTF8, b"fresh").await;
  let preview = p
    .expect("NewItem", |e| match e {
      PickerEvt::NewItem { preview } => Some(preview.clone()),
      _ => None,
    })
    .await;
  assert_eq!(preview.preview, "fresh");
  assert_eq!(preview.kind, PreviewKind::Text);
  // The same text again is a bump: pushed again (moves to the top).
  h.copy(UTF8, b"fresh").await;
  let again = p
    .expect("NewItem", |e| match e {
      PickerEvt::NewItem { preview } => Some(preview.clone()),
      _ => None,
    })
    .await;
  assert_eq!(again.id, preview.id);
}

/// 1x1 PNG.
const PNG: &[u8] = &[
  0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
  0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
  0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
  0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
  0x42, 0x60, 0x82,
];

#[tokio::test]
async fn thumbnails_only_for_images() {
  let mut h = PH::start(Setup::default());
  let mut p = h.picker().await;
  h.copy("image/png", PNG).await;
  h.copy(UTF8, b"some text").await;
  p.query(1, "", 0, 10).await;
  let (items, _) = p.page(1).await;
  let img = items.iter().find(|i| i.kind == PreviewKind::Image).unwrap().id;
  let txt = items.iter().find(|i| i.kind == PreviewKind::Text).unwrap().id;
  p.send(PickerReq::Thumb { seq: 2, id: img, mime: "image/png".into() }).await;
  p.send(PickerReq::Thumb { seq: 3, id: txt, mime: UTF8.into() }).await;
  p.send(PickerReq::Thumb { seq: 4, id: 9999, mime: "image/png".into() }).await;
  let mut got = std::collections::HashMap::new();
  while got.len() < 3 {
    match p.recv().await {
      PickerEvt::Thumb { seq, id, bytes } => {
        assert_eq!(id, img);
        assert_eq!(bytes, PNG);
        got.insert(seq, "thumb".to_string());
      }
      PickerEvt::Error { seq: Some(seq), code, .. } => {
        got.insert(seq, format!("{code:?}"));
      }
      _ => {}
    }
  }
  assert_eq!(got[&2], "thumb");
  assert_eq!(got[&3], "NotFound", "text is never sent as a thumbnail");
  assert_eq!(got[&4], "NotFound");
}

#[tokio::test]
async fn pin_tag_delete() {
  let mut h = PH::start(Setup::default());
  let mut p = h.picker().await;
  h.copy(UTF8, b"older").await;
  h.copy(UTF8, b"newer").await;
  p.query(1, "", 0, 10).await;
  let (items, _) = p.page(1).await;
  let older = items[1].id;
  p.send(PickerReq::Pin { id: older, on: true, seq: 101 }).await;
  p.send(PickerReq::Tag { id: older, tag: "work".into(), on: true, seq: 102 }).await;
  p.query(2, "", 0, 10).await;
  let (items, _) = p.page(2).await;
  assert_eq!(items[0].id, older, "pinned first");
  assert!(items[0].pinned);
  assert_eq!(items[0].tags, ["work"]);
  // Tag filter + index: the tag search finds it once indexed.
  let filters = QueryFilters { tags: vec!["work".into()], ..QueryFilters::default() };
  p.send(PickerReq::Query { seq: 3, q: String::new(), filters, offset: 0, limit: 10 }).await;
  let (items, _) = p.page(3).await;
  assert_eq!(items.len(), 1);
  // Invalid tag and deleting twice: errors carrying the edit's seq.
  p.send(PickerReq::Tag { id: older, tag: "a/b".into(), on: true, seq: 103 }).await;
  let code = p
    .expect("tag error", |e| match e {
      PickerEvt::Error { seq: Some(103), code, .. } => Some(*code),
      PickerEvt::Error { seq, .. } => panic!("error for seq {seq:?}"),
      _ => None,
    })
    .await;
  assert_eq!(code, PickerErrorCode::BadQuery);
  p.send(PickerReq::Delete { id: older, seq: 104 }).await;
  p.query(4, "", 0, 10).await;
  let (items, _) = p.page(4).await;
  assert_eq!(items.iter().map(text).collect::<Vec<_>>(), ["newer"]);
  p.send(PickerReq::Delete { id: older, seq: 105 }).await;
  p.send(PickerReq::Pin { id: older, on: false, seq: 106 }).await;
  p.send(PickerReq::Tag { id: older, tag: "x".into(), on: true, seq: 107 }).await;
  let mut failed = std::collections::BTreeMap::new();
  while failed.len() < 3 {
    match p.recv().await {
      PickerEvt::Error { seq: Some(seq), code, .. } => {
        assert!(failed.insert(seq, code).is_none(), "one answer per failed edit");
      }
      PickerEvt::Error { seq: None, .. } => panic!("an edit error without its seq"),
      _ => {}
    }
  }
  assert_eq!(
    failed.into_iter().collect::<Vec<_>>(),
    [
      (105, PickerErrorCode::NotFound),
      (106, PickerErrorCode::NotFound),
      (107, PickerErrorCode::NotFound)
    ]
  );
  p.quiet("successful edits are not answered").await;
}

#[tokio::test]
async fn tags_show_in_pages_and_pushed_items() {
  let mut h = PH::start(Setup::default());
  let mut p = h.picker().await;
  for text in ["tagged", "other"] {
    h.copy(UTF8, text.as_bytes()).await;
    p.expect("NewItem", |e| {
      matches!(e, PickerEvt::NewItem { preview } if preview.preview == text).then_some(())
    })
    .await;
  }
  p.query(1, "", 0, 10).await;
  let (items, _) = p.page(1).await;
  let id = items.iter().find(|i| i.preview == "tagged").unwrap().id;
  assert!(items.iter().all(|i| i.tags.is_empty()));
  // What the picker's editor sends: add two (one twice), remove one.
  for (seq, (tag, on)) in
    [("work", true), ("two words", true), ("work", true), ("todo", true)].into_iter().enumerate()
  {
    p.send(PickerReq::Tag { id, tag: tag.into(), on, seq: 10 + seq as u32 }).await;
  }
  // Removal goes by the normalized tag too.
  p.send(PickerReq::Tag { id, tag: " todo ".into(), on: false, seq: 20 }).await;
  // Removing a tag the item doesn't have is not an error either.
  p.send(PickerReq::Tag { id, tag: "never".into(), on: false, seq: 21 }).await;
  p.query(2, "", 0, 10).await;
  let (items, _) = p.page(2).await;
  let it = items.iter().find(|i| i.id == id).unwrap();
  let mut tags = it.tags.clone();
  tags.sort();
  assert_eq!(tags, ["two words", "work"]);
  assert!(items.iter().filter(|i| i.id != id).all(|i| i.tags.is_empty()));
  // `tag:` search (index) and the tag filter carry the tags too.
  let start = std::time::Instant::now();
  let found = loop {
    p.query(3, "tag:work", 0, 10).await;
    let (items, _) = p.page(3).await;
    if !items.is_empty() || start.elapsed() > T {
      break items;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
  };
  assert_eq!(found.iter().map(|i| i.id).collect::<Vec<_>>(), [id]);
  assert!(found[0].tags.contains(&"work".to_string()));
  p.query(4, "tag:\"two words\"", 0, 10).await;
  let (items, _) = p.page(4).await;
  assert_eq!(items.iter().map(|i| i.id).collect::<Vec<_>>(), [id], "quoted tag search");
  // A bump of the tagged item pushes a preview with its tags.
  h.copy(UTF8, b"tagged").await;
  let pushed = p
    .expect("NewItem for the bump", |e| match e {
      PickerEvt::NewItem { preview } if preview.id == id => Some(preview.clone()),
      _ => None,
    })
    .await;
  assert!(pushed.tags.contains(&"work".to_string()), "{:?}", pushed.tags);
  p.quiet("after the tag edits").await;
}

#[tokio::test]
async fn daemon_and_picker_agree_on_tag_rules() {
  let mut h = PH::start(Setup::default());
  let mut p = h.picker().await;
  h.copy(UTF8, b"x").await;
  p.expect("NewItem", |e| matches!(e, PickerEvt::NewItem { .. }).then_some(())).await;
  p.query(1, "", 0, 10).await;
  let (items, _) = p.page(1).await;
  let id = items[0].id;
  let long = "y".repeat(spool_proto::MAX_TAG_CHARS + 1);
  let max = "z".repeat(spool_proto::MAX_TAG_CHARS);
  let padded = format!("  {max}\t");
  let tags = [
    "",
    "  ",
    "a/b",
    "a\nb",
    "a\u{85}b",
    long.as_str(),
    "ok",
    max.as_str(),
    " pad ",
    &padded,
    "Ok",
  ];
  for (seq, tag) in tags.into_iter().enumerate() {
    let seq = seq as u32;
    let accepted = spool_proto::check_tag(tag).is_ok();
    p.send(PickerReq::Tag { id, tag: tag.into(), on: true, seq }).await;
    if accepted {
      p.quiet("valid tag").await;
    } else {
      let (code, message) = p
        .expect("invalid tag", |e| match e {
          PickerEvt::Error { seq: Some(s), code, message } if *s == seq => {
            Some((*code, message.clone()))
          }
          _ => None,
        })
        .await;
      assert_eq!(code, PickerErrorCode::BadQuery);
      assert!(!message.contains("a/b") && !message.contains("yyy"), "{message}");
    }
  }
  // Exactly the accepted ones were stored, trimmed (the padded max-length
  // tag is the same tag as `max`), case kept.
  p.query(100, "", 0, 10).await;
  let (items, _) = p.page(100).await;
  let mut stored = items[0].tags.clone();
  stored.sort();
  assert_eq!(stored, ["Ok", "ok", "pad", max.as_str()]);
}

// ---- tests: select / paste / pick ------------------------------------------------

#[tokio::test]
async fn select_copy_and_paste() {
  let mut h = PH::start(Setup::default());
  let mut p = h.picker().await;
  h.copy(UTF8, b"first").await;
  h.copy(UTF8, b"second").await;
  p.query(1, "", 0, 10).await;
  let (items, _) = p.page(1).await;
  let first = items.iter().find(|i| i.preview == "first").unwrap().id;

  // Copy: published, nothing pasted.
  p.send(PickerReq::Select { id: first, mode: SelectMode::Copy }).await;
  let Call::Set(reps) = h.next_call().await else { panic!("expected set") };
  assert!(reps.iter().any(|(m, d)| m == UTF8 && d == b"first"));
  h.confirm();

  // Hotkey Show over kate -> the picker gets Show with the target.
  h.tracker.update(Some("org.kde.kate".into()), Some(W1.into()), std::time::Instant::now());
  h.desk
    .send(CompositorEvent::Show {
      cursor: Some((10, 20)),
      app_id: Some("org.kde.kate".into()),
      window_id: Some(W1.into()),
    })
    .await
    .unwrap();
  let (cursor, target) = p
    .expect("Show", |e| match e {
      PickerEvt::Show { cursor, target_window, .. } => Some((*cursor, target_window.clone())),
      _ => None,
    })
    .await;
  assert_eq!(cursor, Some(CursorPos { x: 10, y: 20 }));
  assert_eq!(target.as_deref(), Some(W1));
  // Enter: the picker hides itself, then selects.
  p.send(PickerReq::Hidden { reason: HideReason::Selected }).await;
  p.send(PickerReq::Select { id: first, mode: SelectMode::Paste }).await;
  let Call::Set(reps) = h.next_call().await else { panic!("expected set") };
  assert!(reps.iter().any(|(m, d)| m == UTF8 && d == b"first"));
  assert!(h.sink.chords.lock().unwrap().is_empty(), "not before the compositor confirms");
  h.confirm();
  let start = std::time::Instant::now();
  while h.sink.chords.lock().unwrap().is_empty() {
    assert!(start.elapsed() < T, "never pasted");
    tokio::time::sleep(Duration::from_millis(10)).await;
  }
  assert_eq!(*h.sink.chords.lock().unwrap(), [PasteChord::CtrlV]);
  // The picker hid itself: no Hide is sent (no double-hide).
  p.quiet("after the paste").await;
}

#[tokio::test]
async fn pick_returns_the_item_or_cancels() {
  let mut h = PH::start(Setup::default());
  let mut p = h.picker().await;
  h.copy(UTF8, b"returned text").await;
  p.query(1, "", 0, 10).await;
  let (items, _) = p.page(1).await;
  let id = items[0].id;

  let rx = h.ask_later(PublicReq::Pick).await;
  let target = p
    .expect("Show", |e| match e {
      PickerEvt::Show { target_window, .. } => Some(target_window.clone()),
      _ => None,
    })
    .await;
  assert_eq!(target, None, "pick never pastes");
  p.send(PickerReq::Hidden { reason: HideReason::Selected }).await;
  p.send(PickerReq::Select { id, mode: SelectMode::Paste }).await;
  let r = tokio::time::timeout(T, rx).await.unwrap().unwrap();
  assert_eq!(r, PublicResp::Picked { mime: UTF8.into(), data: b"returned text".to_vec() });
  assert!(h.calls.try_recv().is_err(), "pick leaves the clipboard alone");

  // Esc cancels.
  let rx = h.ask_later(PublicReq::Pick).await;
  p.expect("Show", |e| matches!(e, PickerEvt::Show { .. }).then_some(())).await;
  p.send(PickerReq::Hidden { reason: HideReason::Esc }).await;
  assert_eq!(tokio::time::timeout(T, rx).await.unwrap().unwrap(), PublicResp::Cancelled);

  // A hotkey Show replaces a waiting pick.
  let rx = h.ask_later(PublicReq::Pick).await;
  p.expect("Show", |e| matches!(e, PickerEvt::Show { .. }).then_some(())).await;
  assert_eq!(h.ask(PublicReq::Show).await, PublicResp::Ok);
  assert_eq!(tokio::time::timeout(T, rx).await.unwrap().unwrap(), PublicResp::Cancelled);
  // ... and the next Select is a normal one (clipboard).
  p.send(PickerReq::Hidden { reason: HideReason::Selected }).await;
  p.send(PickerReq::Select { id, mode: SelectMode::Copy }).await;
  let Call::Set(_) = h.next_call().await else { panic!("expected set") };
}

// ---- tests: lifecycle ----------------------------------------------------------

#[tokio::test]
async fn restart_after_crash_delivers_a_pending_show_and_gpu_falls_back() {
  let settings = PickerSettings { renderer: PickerRenderer::Gpu, prerender: true };
  let mut h = PH::start(Setup { settings, ..Setup::default() });
  let p = h.picker().await;
  assert_eq!(p.renderer, PickerRenderer::Gpu);
  // Dies before Ready: relaunched with software.
  drop(p);
  let (sock, renderer) = FakePicker::accept(&mut h.conns).await;
  assert_eq!(renderer, PickerRenderer::Software);
  // A Show made before the new picker said hello arrives after the
  // handshake.
  assert_eq!(h.ask(PublicReq::Show).await, PublicResp::Ok);
  let mut p = FakePicker::handshake(sock, renderer, spool_proto::PICKER_PROTO_VERSION).await;
  p.expect("pending Show", |e| matches!(e, PickerEvt::Show { .. }).then_some(())).await;
  p.send(PickerReq::Ready).await;
  // A visible picker dying cancels a pick (Hidden{Closed}).
  let rx = h.ask_later(PublicReq::Pick).await;
  p.expect("Show", |e| matches!(e, PickerEvt::Show { .. }).then_some(())).await;
  drop(p);
  assert_eq!(tokio::time::timeout(T, rx).await.unwrap().unwrap(), PublicResp::Cancelled);
  let p = h.picker().await;
  assert_eq!(p.renderer, PickerRenderer::Software);
}

#[tokio::test]
async fn incompatible_picker_is_not_restarted() {
  let mut h = PH::start(Setup::default());
  let (sock, _) = tokio::time::timeout(T, h.conns.recv()).await.unwrap().unwrap();
  sock.set_nonblocking(true).unwrap();
  let (mut rd, mut wr) = tokio::net::UnixStream::from_std(sock).unwrap().into_split();
  write_frame_async(&mut wr, &Hello { proto: 1 }).await.unwrap();
  let theirs: Hello = read_frame_async(&mut rd).await.unwrap();
  assert!(theirs.is_picker_compatible());
  // Closed after the Hello.
  assert!(matches!(
    read_frame_async::<_, PickerEvt>(&mut rd).await,
    Err(spool_proto::FrameError::Eof)
  ));
  let start = std::time::Instant::now();
  loop {
    match h.ask(PublicReq::Show).await {
      PublicResp::Error { code: ErrorCode::Unavailable, .. } => break,
      PublicResp::Ok => assert!(start.elapsed() < T, "show still accepted"),
      other => panic!("{other:?}"),
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
  }
  assert!(tokio::time::timeout(Duration::from_millis(500), h.conns.recv()).await.is_err());
}

// ---- tests: unlock ---------------------------------------------------------------

const PASS: &str = "open sesame";
const PIN: &str = "1234";
const KEK_PASS: [u8; 32] = [0x11; 32];
const KEK_FIDO: [u8; 32] = [0x22; 32];
const KEK_WALLET: [u8; 32] = [0x33; 32];
const B64_32_ZERO: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

/// Enrolls a slot with a fixed KEK as an (unimplemented-kind) `tpm2` slot;
/// the test rewrites the kind and params in the JSON afterwards.
struct Enroll([u8; 32]);

#[async_trait]
impl KeyProvider for Enroll {
  fn kind(&self) -> ProviderKind {
    ProviderKind::Tpm2
  }
  fn interactive(&self) -> bool {
    false
  }
  async fn enroll(&self, _: &SlotId) -> spool_keys::Result<(SlotParams, Kek)> {
    Ok((SlotParams::Opaque(serde_json::json!({})), Zeroizing::new(self.0)))
  }
  async fn unlock(&self, _: &SlotId, _: &SlotParams) -> spool_keys::Result<Kek> {
    unreachable!()
  }
  async fn destroy(&self, _: &SlotId, _: &SlotParams) -> spool_keys::Result<()> {
    Ok(())
  }
}

/// `keyslots.json` with a passphrase slot, a FIDO2 slot (`uv`) and a
/// Secret Service slot, plus an encrypted history holding `existing`.
/// Returns the data key.
async fn fixture(dir: &Path, uv: bool, existing: &str) -> Zeroizing<[u8; 32]> {
  let path = dir.join(spool_keys::FILE_NAME);
  let (mut slots, dk) = KeySlots::create_new(&path, &Enroll(KEK_PASS)).await.unwrap();
  slots.add_slot(&Enroll(KEK_FIDO), &dk).await.unwrap();
  slots.add_slot(&Enroll(KEK_WALLET), &dk).await.unwrap();
  let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
  let s = v["slots"].as_array_mut().unwrap();
  s[0]["provider"] = "passphrase".into();
  s[0]["params"] =
    serde_json::json!({"kdf": "argon2id", "m_kib": 65536, "t": 2, "p": 1, "salt": B64_32_ZERO});
  s[1]["provider"] = "fido2-hmac".into();
  s[1]["params"] = serde_json::json!({"credential_id": "AQID", "salt": B64_32_ZERO, "rp_id": "spool:clipboard", "uv": uv});
  let wallet_id = s[2]["id"].as_str().unwrap().to_owned();
  s[2]["provider"] = "secret-service".into();
  s[2]["params"] = serde_json::json!({"attributes": {"application": "spool", "slot": wallet_id}});
  std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
  let key = DataKey::from_bytes(Zeroizing::new(*dk));
  let mut store = Store::open_encrypted(dir, &key).unwrap();
  let policy =
    spool_core::policy::Policy::new(Config::default(), store.hash_key().unwrap()).unwrap();
  let item = policy
    .manual_item(
      Selection::Clipboard,
      UTF8,
      Bytes::from(existing.as_bytes().to_vec()),
      SystemTime::now(),
    )
    .unwrap();
  store.insert(item).unwrap();
  dk
}

struct FakePass(Zeroizing<String>);

#[async_trait]
impl KeyProvider for FakePass {
  fn kind(&self) -> ProviderKind {
    ProviderKind::Passphrase
  }
  fn interactive(&self) -> bool {
    true
  }
  async fn enroll(&self, _: &SlotId) -> spool_keys::Result<(SlotParams, Kek)> {
    unreachable!()
  }
  async fn unlock(&self, _: &SlotId, _: &SlotParams) -> spool_keys::Result<Kek> {
    // A wrong passphrase derives a wrong KEK -> the slot does not open.
    Ok(Zeroizing::new(if *self.0 == PASS { KEK_PASS } else { [0x99; 32] }))
  }
  async fn destroy(&self, _: &SlotId, _: &SlotParams) -> spool_keys::Result<()> {
    Ok(())
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fido {
  /// Touch works without a PIN.
  Touch,
  /// The key needs its PIN (`PIN`); others are `PinInvalid`.
  NeedsPin,
  Blocked,
  /// Touch timeout.
  Timeout,
}

struct FidoScript {
  mode: StdMutex<Fido>,
  calls: AtomicUsize,
  /// PINs the FIDO2 attempts were made with (`None` = touch only).
  pins: StdMutex<Vec<Option<String>>>,
  /// Explicit wallet unlocks (`UnlockProviders::secret_service`, the
  /// prompting provider) and whether the wallet "dialog" is dismissed.
  wallet_requests: AtomicUsize,
  wallet_dismissed: StdMutex<bool>,
  /// Holds every attempt until released (a pending touch).
  hold: StdMutex<bool>,
  release: Notify,
}

impl FidoScript {
  fn new(mode: Fido) -> Arc<Self> {
    Arc::new(Self {
      mode: StdMutex::new(mode),
      calls: AtomicUsize::new(0),
      pins: StdMutex::default(),
      wallet_requests: AtomicUsize::new(0),
      wallet_dismissed: StdMutex::new(false),
      hold: StdMutex::new(false),
      release: Notify::new(),
    })
  }
}

struct FakeFido(Arc<FidoScript>, Option<Zeroizing<String>>);

#[async_trait]
impl KeyProvider for FakeFido {
  fn kind(&self) -> ProviderKind {
    ProviderKind::Fido2Hmac
  }
  fn interactive(&self) -> bool {
    true
  }
  async fn enroll(&self, _: &SlotId) -> spool_keys::Result<(SlotParams, Kek)> {
    unreachable!()
  }
  async fn unlock(&self, _: &SlotId, _: &SlotParams) -> spool_keys::Result<Kek> {
    self.0.calls.fetch_add(1, Ordering::SeqCst);
    self.0.pins.lock().unwrap().push(self.1.as_ref().map(|p| p.to_string()));
    loop {
      let notified = self.0.release.notified();
      if !*self.0.hold.lock().unwrap() {
        break;
      }
      notified.await;
    }
    let k = ProviderKind::Fido2Hmac;
    let mode = *self.0.mode.lock().unwrap();
    match (mode, self.1.as_deref().map(String::as_str)) {
      (Fido::Touch, _) => Ok(Zeroizing::new(KEK_FIDO)),
      (Fido::NeedsPin, None) => Err(spool_keys::Error::NeedsSecret(k)),
      (Fido::NeedsPin, Some(PIN)) => Ok(Zeroizing::new(KEK_FIDO)),
      (Fido::NeedsPin, Some(_)) => Err(spool_keys::Error::PinInvalid(k)),
      (Fido::Blocked, _) => Err(spool_keys::Error::PinBlocked(k)),
      (Fido::Timeout, _) => Err(spool_keys::Error::Dismissed(k)),
    }
  }
  async fn destroy(&self, _: &SlotId, _: &SlotParams) -> spool_keys::Result<()> {
    Ok(())
  }
}

/// The prompting Secret Service provider (explicit `Unlock{KWallet}`): the
/// "wallet dialog" either opens the wallet or is dismissed.
struct FakeWallet(Arc<FidoScript>);

#[async_trait]
impl KeyProvider for FakeWallet {
  fn kind(&self) -> ProviderKind {
    ProviderKind::SecretService
  }
  fn interactive(&self) -> bool {
    true
  }
  async fn enroll(&self, _: &SlotId) -> spool_keys::Result<(SlotParams, Kek)> {
    unreachable!()
  }
  async fn unlock(&self, _: &SlotId, _: &SlotParams) -> spool_keys::Result<Kek> {
    if *self.0.wallet_dismissed.lock().unwrap() {
      return Err(spool_keys::Error::Dismissed(ProviderKind::SecretService));
    }
    Ok(Zeroizing::new(KEK_WALLET))
  }
  async fn destroy(&self, _: &SlotId, _: &SlotParams) -> spool_keys::Result<()> {
    Ok(())
  }
}

struct FakeProviders(Arc<FidoScript>);

impl UnlockProviders for FakeProviders {
  fn passphrase(&self, p: Zeroizing<String>) -> Box<dyn KeyProvider> {
    Box::new(FakePass(p))
  }
  fn fido2(&self, pin: Option<Zeroizing<String>>) -> Box<dyn KeyProvider> {
    Box::new(FakeFido(self.0.clone(), pin))
  }
  fn secret_service(&self) -> Box<dyn KeyProvider> {
    self.0.wallet_requests.fetch_add(1, Ordering::SeqCst);
    Box::new(FakeWallet(self.0.clone()))
  }
}

fn secret(s: &str) -> Option<UnlockSecret> {
  Some(UnlockSecret::new(Zeroizing::new(s.into())))
}

async fn locked(fido: Fido, uv: bool) -> (PH, FakePicker, Arc<FidoScript>, tempfile::TempDir) {
  let dir = tempfile::tempdir().unwrap();
  fixture(dir.path(), uv, "from the encrypted history").await;
  let script = FidoScript::new(fido);
  let session = Store::open_session(&DataKey::generate()).unwrap();
  let providers: Arc<dyn UnlockProviders> = Arc::new(FakeProviders(script.clone()));
  let mut h = PH::start(Setup {
    store: session,
    unlock: Some((dir.path().to_owned(), providers)),
    ..Setup::default()
  });
  let p = h.picker().await;
  (h, p, script, dir)
}

async fn expect_locked(p: &mut FakePicker) -> Vec<UnlockPrompt> {
  p.expect("Locked", |e| match e {
    PickerEvt::Locked { providers } => Some(providers.clone()),
    _ => None,
  })
  .await
}

async fn expect_failed(p: &mut FakePicker) -> (UnlockProvider, UnlockFailReason) {
  p.expect("UnlockFailed", |e| match e {
    PickerEvt::UnlockFailed { provider, reason } => Some((*provider, *reason)),
    PickerEvt::Unlocked => panic!("unexpectedly unlocked"),
    _ => None,
  })
  .await
}

async fn expect_unlocked(h: &PH, p: &mut FakePicker) {
  p.expect("Unlocked", |e| matches!(e, PickerEvt::Unlocked).then_some(())).await;
  assert_eq!(h.status().await.key_state, "ready");
  // The merged encrypted history is what the picker now lists.
  p.query(100, "", 0, 10).await;
  let (items, _) = p.page(100).await;
  assert!(items.iter().any(|i| i.preview == "from the encrypted history"), "{items:?}");
}

#[tokio::test]
async fn unlock_with_passphrase_wrong_then_right() {
  let (h, mut p, _, _dir) = locked(Fido::Touch, false).await;
  assert_eq!(expect_locked(&mut p).await, [UnlockPrompt::Passphrase, UnlockPrompt::Fido2Touch]);
  // Locked: pages come from the session store.
  p.query(1, "", 0, 10).await;
  assert!(p.page(1).await.0.is_empty());
  p.send(PickerReq::Unlock { provider: UnlockProvider::Passphrase, secret: secret("nope") }).await;
  assert_eq!(
    expect_failed(&mut p).await,
    (UnlockProvider::Passphrase, UnlockFailReason::WrongSecret)
  );
  assert_eq!(h.status().await.key_state, "locked");
  p.send(PickerReq::Unlock { provider: UnlockProvider::Passphrase, secret: secret(PASS) }).await;
  expect_unlocked(&h, &mut p).await;
  // Further unlocks just say Unlocked.
  p.send(PickerReq::Unlock { provider: UnlockProvider::Passphrase, secret: secret("x") }).await;
  p.expect("Unlocked", |e| matches!(e, PickerEvt::Unlocked).then_some(())).await;
}

#[tokio::test]
async fn unlock_with_a_fido2_touch_dedupes_requests() {
  let (h, mut p, script, _dir) = locked(Fido::Touch, false).await;
  expect_locked(&mut p).await;
  *script.hold.lock().unwrap() = true;
  p.send(PickerReq::Unlock { provider: UnlockProvider::Fido2, secret: None }).await;
  let start = std::time::Instant::now();
  while script.calls.load(Ordering::SeqCst) == 0 {
    assert!(start.elapsed() < T, "the touch request never started");
    tokio::time::sleep(Duration::from_millis(10)).await;
  }
  p.send(PickerReq::Unlock { provider: UnlockProvider::Fido2, secret: None }).await;
  p.quiet("while the touch is pending").await;
  assert_eq!(script.calls.load(Ordering::SeqCst), 1, "one touch request at a time");
  *script.hold.lock().unwrap() = false;
  script.release.notify_waiters();
  expect_unlocked(&h, &mut p).await;
}

#[tokio::test]
async fn unlock_fido2_asks_for_the_pin() {
  let (h, mut p, script, _dir) = locked(Fido::NeedsPin, false).await;
  expect_locked(&mut p).await;
  p.send(PickerReq::Unlock { provider: UnlockProvider::Fido2, secret: None }).await;
  // NeedsSecret is not a failure: the prompt set gains the PIN field.
  assert_eq!(
    expect_locked(&mut p).await,
    [UnlockPrompt::Passphrase, UnlockPrompt::Fido2Touch, UnlockPrompt::Fido2Pin]
  );
  p.send(PickerReq::Unlock { provider: UnlockProvider::Fido2, secret: secret("0000") }).await;
  assert_eq!(expect_failed(&mut p).await, (UnlockProvider::Fido2, UnlockFailReason::PinInvalid));
  p.send(PickerReq::Unlock { provider: UnlockProvider::Fido2, secret: secret(PIN) }).await;
  expect_unlocked(&h, &mut p).await;
  assert_eq!(script.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn unlock_fido2_blocked_timeout_and_uv_slot() {
  let (h, mut p, script, _dir) = locked(Fido::Blocked, true).await;
  // A `uv` slot prompts for the PIN up front.
  assert_eq!(
    expect_locked(&mut p).await,
    [UnlockPrompt::Passphrase, UnlockPrompt::Fido2Touch, UnlockPrompt::Fido2Pin]
  );
  p.send(PickerReq::Unlock { provider: UnlockProvider::Fido2, secret: secret(PIN) }).await;
  assert_eq!(expect_failed(&mut p).await, (UnlockProvider::Fido2, UnlockFailReason::PinBlocked));
  *script.mode.lock().unwrap() = Fido::Timeout;
  p.send(PickerReq::Unlock { provider: UnlockProvider::Fido2, secret: None }).await;
  assert_eq!(expect_failed(&mut p).await, (UnlockProvider::Fido2, UnlockFailReason::Dismissed));
  assert_eq!(h.status().await.key_state, "locked");
}

#[tokio::test]
async fn unlocked_by_another_route_tells_the_picker() {
  let dir = tempfile::tempdir().unwrap();
  let dk = fixture(dir.path(), false, "from the encrypted history").await;
  let script = FidoScript::new(Fido::Touch);
  let mut h = PH::start(Setup {
    store: Store::open_session(&DataKey::generate()).unwrap(),
    unlock: Some((dir.path().to_owned(), Arc::new(FakeProviders(script)))),
    ..Setup::default()
  });
  let mut p = h.picker().await;
  expect_locked(&mut p).await;
  // The wallet opened by itself: the key flow delivers the store.
  let key = DataKey::from_bytes(Zeroizing::new(*dk));
  let mut store = Store::open_encrypted(dir.path(), &key).unwrap();
  let hash_key = Zeroizing::new(store.hash_key().unwrap());
  let opened =
    crate::keyflow::OpenedStore { store, hash_key, data_key: key, dir: dir.path().into() };
  h.key_tx.send(KeyEvent::Opened(Box::new(opened))).await.unwrap();
  expect_unlocked(&h, &mut p).await;
  // A picker that connects now is not told it is locked.
  drop(p);
  let mut p = h.picker().await;
  p.query(1, "", 0, 10).await;
  p.page(1).await;
  p.quiet("after reconnecting").await;
}

#[tokio::test]
async fn pin_sent_during_a_running_touch_is_queued() {
  let (h, mut p, script, _dir) = locked(Fido::NeedsPin, false).await;
  expect_locked(&mut p).await;
  *script.hold.lock().unwrap() = true;
  p.send(PickerReq::Unlock { provider: UnlockProvider::Fido2, secret: None }).await;
  let start = std::time::Instant::now();
  while script.calls.load(Ordering::SeqCst) == 0 {
    assert!(start.elapsed() < T, "the touch request never started");
    tokio::time::sleep(Duration::from_millis(10)).await;
  }
  // The PIN arrives while the touch attempt is still waiting: queued, not
  // a second concurrent attempt.
  p.send(PickerReq::Unlock { provider: UnlockProvider::Fido2, secret: secret(PIN) }).await;
  p.quiet("while the touch is pending").await;
  assert_eq!(script.calls.load(Ordering::SeqCst), 1, "one attempt at a time");
  *script.hold.lock().unwrap() = false;
  script.release.notify_waiters();
  // The touch attempt ends with NeedsSecret (prompt set gains the PIN),
  // then the queued PIN runs and opens the history.
  assert_eq!(
    expect_locked(&mut p).await,
    [UnlockPrompt::Passphrase, UnlockPrompt::Fido2Touch, UnlockPrompt::Fido2Pin]
  );
  expect_unlocked(&h, &mut p).await;
  assert_eq!(*script.pins.lock().unwrap(), [None, Some(PIN.to_owned())]);
}

#[tokio::test]
async fn explicit_kwallet_unlock_retries_with_the_prompting_provider() {
  let (h, mut p, script, _dir) = locked(Fido::Touch, false).await;
  expect_locked(&mut p).await;
  // The user dismisses the wallet dialog: reported, still locked.
  *script.wallet_dismissed.lock().unwrap() = true;
  p.send(PickerReq::Unlock { provider: UnlockProvider::KWallet, secret: None }).await;
  assert_eq!(expect_failed(&mut p).await, (UnlockProvider::KWallet, UnlockFailReason::Dismissed));
  assert_eq!(h.status().await.key_state, "locked");
  assert_eq!(script.wallet_requests.load(Ordering::SeqCst), 1);
  // Retry: the wallet opens.
  *script.wallet_dismissed.lock().unwrap() = false;
  p.send(PickerReq::Unlock { provider: UnlockProvider::KWallet, secret: None }).await;
  expect_unlocked(&h, &mut p).await;
  assert_eq!(script.wallet_requests.load(Ordering::SeqCst), 2);
  assert_eq!(script.calls.load(Ordering::SeqCst), 0, "no FIDO2 attempt involved");
}

#[tokio::test]
async fn show_while_the_picker_is_down_is_delivered_after_relaunch() {
  let mut h = PH::start(Setup::default());
  let mut p = h.picker().await;
  p.send(PickerReq::Ready).await;
  p.quiet("after Ready").await;
  // The picker dies; the host waits out its restart backoff before
  // spawning a new one.
  drop(p);
  tokio::time::sleep(Duration::from_millis(50)).await;
  assert_eq!(h.ask(PublicReq::Show).await, PublicResp::Ok);
  assert!(h.conns.try_recv().is_err(), "relaunched before the backoff");
  let mut p = h.picker().await;
  p.expect("Show from the down time", |e| matches!(e, PickerEvt::Show { .. }).then_some(())).await;
}

#[tokio::test]
async fn interrupted_rotation_refuses_picker_unlocks() {
  let (h, mut p, script, dir) = locked(Fido::Touch, false).await;
  expect_locked(&mut p).await;
  let slots = dir.path().join(spool_keys::FILE_NAME);
  std::fs::copy(&slots, dir.path().join(format!("{}.next", spool_keys::FILE_NAME))).unwrap();
  // Prompts are read from keyslots.json: none while a rotation is pending.
  assert!(crate::unlock::prompts(dir.path(), true).is_empty());
  p.send(PickerReq::Unlock { provider: UnlockProvider::Passphrase, secret: secret(PASS) }).await;
  assert_eq!(
    expect_failed(&mut p).await,
    (UnlockProvider::Passphrase, UnlockFailReason::Unavailable)
  );
  p.send(PickerReq::Unlock { provider: UnlockProvider::Fido2, secret: None }).await;
  assert_eq!(expect_failed(&mut p).await, (UnlockProvider::Fido2, UnlockFailReason::Unavailable));
  assert_eq!(script.calls.load(Ordering::SeqCst), 0, "no slot was tried");
  assert_eq!(h.status().await.key_state, "locked");
}
