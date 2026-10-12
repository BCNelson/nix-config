//! Script filters through the orchestrator: a real `scripts.sock` server
//! (the test process is the "host", so the exe check passes with its own
//! executable) and a fake Wayland side.

use std::path::Path;

use spool_proto::{FrameError, Hello, ScriptEvt, ScriptReq, read_frame_async, write_frame_async};
use tokio::net::UnixStream;

use super::*;
use crate::scripts::{ExeCheck, ScriptServer};

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

struct H {
  wl: mpsc::UnboundedSender<WaylandEvent>,
  req: mpsc::Sender<Request>,
  calls: mpsc::UnboundedReceiver<Call>,
  socket: PathBuf,
  connected: Box<dyn Fn() -> bool + Send>,
  offers: u64,
  _dir: tempfile::TempDir,
  _stop: oneshot::Sender<()>,
}

fn own_exe() -> ExeCheck {
  ExeCheck::Exact(std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap())
}

impl H {
  fn start(budget: Duration, check: ExeCheck) -> Self {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("scripts.sock");
    let listener = crate::scripts::bind(&socket).unwrap();
    let (link, server): (Scripts, ScriptServer) = Scripts::new(budget, check);
    let connected = server.connected_probe();
    tokio::spawn(crate::scripts::serve(listener, server));
    let (ctx, crx) = mpsc::unbounded_channel();
    let orch =
      Orchestrator::new(Config::default(), Store::open_in_memory().unwrap(), FakeWayland(ctx))
        .unwrap()
        .with_scripts(link);
    let (wl_tx, wl_rx) = mpsc::unbounded_channel();
    let (req_tx, req_rx) = mpsc::channel(REQUEST_QUEUE);
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    tokio::spawn(orch.run(wl_rx, req_rx, async move {
      let _ = stop_rx.await;
    }));
    Self {
      wl: wl_tx,
      req: req_tx,
      calls: crx,
      socket,
      connected: Box::new(connected),
      offers: 100,
      _dir: dir,
      _stop: stop_tx,
    }
  }

  async fn host(&self) -> Host {
    let host = Host::connect(&self.socket).await.expect("handshake");
    for _ in 0..200 {
      if (self.connected)() {
        return host;
      }
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("host never registered");
  }

  async fn next_call(&mut self) -> Call {
    tokio::time::timeout(Duration::from_secs(5), self.calls.recv())
      .await
      .expect("timed out waiting for a wayland call")
      .unwrap()
  }

  /// A foreign copy of `text`, fetched.
  async fn copy(&mut self, text: &str) {
    self.offers += 1;
    self
      .wl
      .send(WaylandEvent::NewSelection {
        selection: Selection::Clipboard,
        mimes: vec![UTF8.into()],
        offer: OfferToken::from_raw(self.offers),
        ours: false,
      })
      .unwrap();
    let Call::Fetch(offer) = self.next_call().await else { panic!("expected fetch") };
    self
      .wl
      .send(WaylandEvent::Fetched {
        offer,
        result: Ok(vec![(UTF8.into(), text.as_bytes().to_vec())]),
      })
      .unwrap();
  }

  /// Stored items, newest first (also a barrier: earlier events are done).
  async fn items(&self) -> Vec<ItemPreview> {
    let (reply, rx) = oneshot::channel();
    let q = Request::Search {
      q: String::new(),
      filters: QueryFilters::default(),
      offset: 0,
      limit: 10,
      reply,
    };
    self.req.send(q).await.unwrap();
    rx.await.unwrap().unwrap()
  }

  async fn ask(&self, req: PublicReq) -> PublicResp {
    let (reply, rx) = oneshot::channel();
    let peer = PeerCred { pid: Some(1), uid: 0, gid: 0 };
    self.req.send(Request::Public { req, peer, reply }).await.unwrap();
    rx.await.unwrap()
  }

  async fn versions(&self, id: i64) -> (i64, Vec<spool_proto::VersionPreview>) {
    let (reply, rx) = oneshot::channel();
    self.req.send(Request::Versions { id: ItemId(id), reply }).await.unwrap();
    let (cur, v) = rx.await.unwrap().unwrap();
    (cur.0, v)
  }

  async fn previews(&self) -> Vec<String> {
    self.items().await.into_iter().map(|i| i.preview).collect()
  }

  /// Poll until `n` items are stored.
  async fn wait_items(&self, n: usize) -> Vec<ItemPreview> {
    for _ in 0..400 {
      let items = self.items().await;
      if items.len() >= n {
        return items;
      }
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("expected {n} items, have {:?}", self.previews().await);
  }
}

struct Host(UnixStream);

impl Host {
  async fn connect(path: &Path) -> Result<Self, FrameError> {
    let mut s = UnixStream::connect(path).await?;
    write_frame_async(&mut s, &Hello::script()).await?;
    let hello: Hello = read_frame_async(&mut s).await?;
    assert!(hello.is_script_compatible());
    write_frame_async(&mut s, &ScriptReq::Register { filters: true }).await?;
    Ok(Self(s))
  }

  async fn next(&mut self) -> (u64, ScriptItem) {
    let evt: ScriptEvt =
      tokio::time::timeout(Duration::from_secs(5), read_frame_async(&mut self.0))
        .await
        .expect("timed out waiting for a filter request")
        .unwrap();
    let ScriptEvt::Filter { seq, budget_ms, item } = evt;
    assert!(budget_ms > 0);
    (seq, item)
  }

  async fn answer(&mut self, seq: u64, verdict: Verdict) {
    write_frame_async(&mut self.0, &ScriptReq::FilterDone { seq, verdict }).await.unwrap();
  }
}

fn text_rep(s: &str) -> WireRep {
  WireRep { mime: UTF8.into(), data: s.as_bytes().to_vec() }
}

const LONG: Duration = Duration::from_secs(10);

#[tokio::test]
async fn no_host_stores_at_once() {
  let mut h = H::start(LONG, own_exe());
  h.copy("plain").await;
  assert_eq!(h.previews().await, ["plain"]);
}

#[tokio::test]
async fn host_sees_the_copy_and_can_drop_it() {
  let mut h = H::start(LONG, own_exe());
  let mut host = h.host().await;
  h.copy("drop me").await;
  let (seq, item) = host.next().await;
  assert_eq!(item.reps, [text_rep("drop me")]);
  assert!(item.id.is_none());
  assert!(h.items().await.is_empty(), "held while the host decides");
  host.answer(seq, Verdict::Drop).await;
  h.copy("keep me").await;
  let (seq, _) = host.next().await;
  host.answer(seq, Verdict::keep()).await;
  assert_eq!(h.wait_items(1).await.into_iter().map(|i| i.preview).collect::<Vec<_>>(), ["keep me"]);
}

#[tokio::test]
async fn rewrite_and_tags_are_stored() {
  let mut h = H::start(LONG, own_exe());
  let mut host = h.host().await;
  h.copy("https://x.com/?utm_source=a").await;
  let (seq, _) = host.next().await;
  let verdict = Verdict::Keep {
    tags: vec!["links".into(), "bad/tag".into()],
    reps: Some(vec![text_rep("https://x.com/")]),
    original: true,
    by: vec!["10-urls.lua".into(), "../evil".into()],
  };
  host.answer(seq, verdict).await;
  let items = h.wait_items(1).await;
  assert_eq!(items.len(), 1, "the original is not listed");
  assert_eq!(items[0].preview, "https://x.com/");
  assert_eq!(items[0].tags, ["links"], "invalid tags are ignored");
  assert!(items[0].mimes.iter().any(|m| m == "UTF8_STRING"), "text aliases added back");
  assert_eq!(items[0].versions, 1);
  // The original, via the picker's Versions request.
  let (cur, versions) = h.versions(items[0].id).await;
  assert_eq!(cur, items[0].id);
  assert_eq!(versions.len(), 2);
  assert_eq!(versions[0].item.preview, "https://x.com/?utm_source=a");
  assert_eq!(versions[0].revision, None);
  assert!(!versions[0].current);
  assert_eq!(versions[1].revision.as_deref(), Some("script:10-urls.lua"), "bad names dropped");
  assert!(versions[1].current);
  // ... and via the public socket (the newest item's versions).
  let PublicResp::Versions(v) = h.ask(PublicReq::Versions).await else { panic!() };
  assert_eq!(
    v.iter().map(|v| v.preview.as_str()).collect::<Vec<_>>(),
    ["https://x.com/?utm_source=a", "https://x.com/"]
  );
  let PublicResp::Current { data, .. } = h.ask(PublicReq::CurrentVersion { index: 0 }).await else {
    panic!()
  };
  assert_eq!(data, b"https://x.com/?utm_source=a");
  assert_eq!(h.ask(PublicReq::CurrentVersion { index: 2 }).await, PublicResp::Empty);
}

#[tokio::test]
async fn redacting_rewrite_keeps_no_original() {
  let mut h = H::start(LONG, own_exe());
  let mut host = h.host().await;
  h.copy("token=abc123").await;
  let (seq, _) = host.next().await;
  let verdict = Verdict::Keep {
    tags: vec![],
    reps: Some(vec![text_rep("token=[redacted]")]),
    original: false,
    by: vec!["05-redact.lua".into()],
  };
  host.answer(seq, verdict).await;
  let items = h.wait_items(1).await;
  assert_eq!((items[0].preview.as_str(), items[0].versions), ("token=[redacted]", 0));
  let (_, versions) = h.versions(items[0].id).await;
  assert_eq!(versions.len(), 1);
}

#[tokio::test]
async fn rewrite_into_a_secret_is_dropped() {
  let mut h = H::start(LONG, own_exe());
  let mut host = h.host().await;
  h.copy("harmless").await;
  let (seq, _) = host.next().await;
  let token = format!("ghp_{}", "a".repeat(36));
  let verdict =
    Verdict::Keep { tags: vec![], reps: Some(vec![text_rep(&token)]), original: true, by: vec![] };
  host.answer(seq, verdict).await;
  h.copy("after").await;
  let (seq, _) = host.next().await;
  host.answer(seq, Verdict::keep()).await;
  assert_eq!(h.wait_items(1).await.into_iter().map(|i| i.preview).collect::<Vec<_>>(), ["after"]);
}

#[tokio::test]
async fn silent_host_times_out_to_unfiltered() {
  let mut h = H::start(Duration::from_millis(50), own_exe());
  let mut host = h.host().await;
  h.copy("slow").await;
  let (seq, _) = host.next().await;
  assert_eq!(h.wait_items(1).await[0].preview, "slow");
  // A late verdict changes nothing.
  host.answer(seq, Verdict::Drop).await;
  h.copy("next").await;
  let _ = host.next().await;
  assert_eq!(h.wait_items(2).await.len(), 2);
}

#[tokio::test]
async fn host_disconnect_finishes_waiting_copies() {
  let mut h = H::start(LONG, own_exe());
  let mut host = h.host().await;
  h.copy("one").await;
  h.copy("two").await;
  let _ = host.next().await;
  let _ = host.next().await;
  drop(host);
  assert_eq!(
    h.wait_items(2).await.into_iter().map(|i| i.preview).collect::<Vec<_>>(),
    ["two", "one"],
    "stored in copy order"
  );
}

#[tokio::test]
async fn copies_finish_in_order() {
  let mut h = H::start(LONG, own_exe());
  let mut host = h.host().await;
  h.copy("first").await;
  h.copy("second").await;
  let (a, _) = host.next().await;
  let (b, _) = host.next().await;
  // Answer the second first: it still waits for the first.
  host.answer(b, Verdict::keep()).await;
  tokio::time::sleep(Duration::from_millis(50)).await;
  assert!(h.items().await.is_empty());
  host.answer(a, Verdict::keep()).await;
  assert_eq!(
    h.wait_items(2).await.into_iter().map(|i| i.preview).collect::<Vec<_>>(),
    ["second", "first"]
  );
}

#[tokio::test]
async fn keep_alive_waits_for_the_parked_copy() {
  let mut h = H::start(LONG, own_exe());
  let mut host = h.host().await;
  h.copy("survivor").await;
  let (seq, _) = host.next().await;
  // The source app exits while the copy is with the filters.
  h.wl.send(WaylandEvent::SelectionCleared { selection: Selection::Clipboard }).unwrap();
  h.items().await;
  assert!(h.calls.try_recv().is_err(), "keep-alive must wait for the verdict");
  host.answer(seq, Verdict::keep()).await;
  let Call::Set(reps) = h.next_call().await else { panic!("expected keep-alive publish") };
  assert!(reps.iter().any(|(m, d)| m == UTF8 && d == b"survivor"));
}

#[tokio::test]
async fn other_executables_are_rejected() {
  let h = H::start(LONG, ExeCheck::Exact(PathBuf::from("/nix/store/x-spool/bin/spool-scripts")));
  assert!(Host::connect(&h.socket).await.is_err(), "closed before the Hello");
  assert!(!(h.connected)());
}
