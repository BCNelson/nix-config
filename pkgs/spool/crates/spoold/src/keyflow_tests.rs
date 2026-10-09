//! Orchestrator + key flow tests with fake key providers: capture while
//! locked, merge on unlock (order, keep-alive / clear candidates remapped),
//! waiting without spinning, fatal errors, first run.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use spool_core::store::{DB_FILE_NAME, Store};
use spool_crypto::{DataKey, Zeroizing};
use spool_keys::{Kek, KeyProvider, KeySlots, ProviderKind, SlotId, SlotParams};
use spool_wayland::CompositorInfo;
use tokio::sync::Notify;

use super::*;
use crate::keyflow::{self, BoxFuture, KeySource};

const UTF8: &str = "text/plain;charset=utf-8";

// ---- fake key provider -------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
  /// Behaves (unless `locked`).
  Ok,
  /// Unlock returns a KEK that does not unwrap the slot.
  WrongKek,
  /// Unlock fails with a fatal provider error.
  Fatal,
}

/// In-memory provider whose KEKs survive "daemon restarts" (shared `Arc`).
struct FakeProvider {
  keks: Mutex<HashMap<SlotId, [u8; 32]>>,
  mode: Mutex<Mode>,
  locked: AtomicBool,
  notify: Notify,
  enrolls: AtomicUsize,
  unlocks: AtomicUsize,
  waits: AtomicUsize,
}

impl FakeProvider {
  fn new(locked: bool) -> Arc<Self> {
    Arc::new(Self {
      keks: Mutex::default(),
      mode: Mutex::new(Mode::Ok),
      locked: AtomicBool::new(locked),
      notify: Notify::new(),
      enrolls: AtomicUsize::new(0),
      unlocks: AtomicUsize::new(0),
      waits: AtomicUsize::new(0),
    })
  }

  fn set_mode(&self, m: Mode) {
    *self.mode.lock().unwrap() = m;
  }

  fn lock(&self) {
    self.locked.store(true, Ordering::SeqCst);
  }

  fn unlock_now(&self) {
    self.locked.store(false, Ordering::SeqCst);
    self.notify.notify_one();
  }

  fn calls(&self) -> usize {
    self.enrolls.load(Ordering::SeqCst) + self.unlocks.load(Ordering::SeqCst)
  }

  fn check(&self) -> spool_keys::Result<Mode> {
    if self.locked.load(Ordering::SeqCst) {
      return Err(spool_keys::Error::Locked(ProviderKind::Session));
    }
    let mode = *self.mode.lock().unwrap();
    if mode == Mode::Fatal {
      return Err(spool_keys::Error::ProviderCorrupt {
        kind: ProviderKind::Session,
        reason: "fake fatal error".into(),
      });
    }
    Ok(mode)
  }
}

#[async_trait]
impl KeyProvider for FakeProvider {
  fn kind(&self) -> ProviderKind {
    ProviderKind::Session
  }

  fn interactive(&self) -> bool {
    false
  }

  async fn enroll(&self, slot_id: &SlotId) -> spool_keys::Result<(SlotParams, Kek)> {
    self.enrolls.fetch_add(1, Ordering::SeqCst);
    self.check()?;
    let kek = *DataKey::generate().expose();
    self.keks.lock().unwrap().insert(*slot_id, kek);
    Ok((SlotParams::Session, Zeroizing::new(kek)))
  }

  async fn unlock(&self, slot_id: &SlotId, _params: &SlotParams) -> spool_keys::Result<Kek> {
    self.unlocks.fetch_add(1, Ordering::SeqCst);
    if self.check()? == Mode::WrongKek {
      return Ok(Zeroizing::new([0x5a; 32]));
    }
    let kek = self.keks.lock().unwrap().get(slot_id).copied();
    kek.map(Zeroizing::new).ok_or(spool_keys::Error::SecretMissing(ProviderKind::Session))
  }

  async fn destroy(&self, slot_id: &SlotId, _params: &SlotParams) -> spool_keys::Result<()> {
    self.keks.lock().unwrap().remove(slot_id);
    Ok(())
  }
}

struct Source(Arc<FakeProvider>);

impl KeySource for Source {
  fn provider(&self) -> &dyn KeyProvider {
    &*self.0
  }

  fn wait_for_unlock(&self) -> BoxFuture<'_, spool_keys::Result<()>> {
    Box::pin(async move {
      self.0.waits.fetch_add(1, Ordering::SeqCst);
      // `notify_one` stores a permit, so an unlock between the check and
      // `notified()` is not lost.
      while self.0.locked.load(Ordering::SeqCst) {
        self.0.notify.notified().await;
      }
      Ok(())
    })
  }
}

// ---- harness -----------------------------------------------------------------

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
  stop: Option<oneshot::Sender<()>>,
  task: tokio::task::JoinHandle<anyhow::Result<()>>,
  key_task: tokio::task::JoinHandle<()>,
  next_offer: u64,
  index: tokio::sync::watch::Receiver<crate::index::IndexStatus>,
}

impl H {
  /// Session store + key flow against `dir` with `provider`.
  fn start(dir: &Path, provider: &Arc<FakeProvider>) -> H {
    Self::start_with(Some((dir, provider)), Config::default())
  }

  /// Session store, plus a key flow when `key` is given (else the
  /// session-only state).
  fn start_with(key: Option<(&Path, &Arc<FakeProvider>)>, config: Config) -> H {
    let (ctx, calls) = mpsc::unbounded_channel();
    let store = Store::open_session(&DataKey::generate()).unwrap();
    keyflow::quiet_sqlcipher(&store);
    let mut orch = Orchestrator::new(config, store, FakeWayland(ctx)).unwrap();
    let key_task = match key {
      Some((dir, provider)) => {
        let (key_tx, key_rx) = mpsc::channel(8);
        orch = orch.with_key_flow(key_rx);
        assert_eq!(orch.key_state(), &KeyState::Locked);
        tokio::spawn(keyflow::run(Arc::new(Source(provider.clone())), dir.to_path_buf(), key_tx))
      }
      None => tokio::spawn(async {}),
    };
    let index = orch.index_status();
    let (wl, wl_rx) = mpsc::unbounded_channel();
    let (req, req_rx) = mpsc::channel(REQUEST_QUEUE);
    let (stop, stop_rx) = oneshot::channel::<()>();
    let task = tokio::spawn(orch.run(wl_rx, req_rx, async move {
      let _ = stop_rx.await;
    }));
    wl.send(WaylandEvent::Ready(CompositorInfo {
      display: "wayland-test".into(),
      data_control_version: 1,
      primary_supported: false,
    }))
    .unwrap();
    H { wl, req, calls, stop: Some(stop), task, key_task, next_offer: 1, index }
  }

  async fn ask(&self, req: PublicReq) -> PublicResp {
    let (reply, rx) = oneshot::channel();
    let peer = PeerCred { pid: Some(1), uid: 0, gid: 0 };
    self.req.send(Request::Public { req, peer, reply }).await.unwrap();
    rx.await.unwrap()
  }

  async fn status(&self) -> StatusInfo {
    match self.ask(PublicReq::Status).await {
      PublicResp::Status(s) => s,
      other => panic!("unexpected {other:?}"),
    }
  }

  async fn wait_state(&self, pred: impl Fn(&str) -> bool) -> StatusInfo {
    let start = tokio::time::Instant::now();
    loop {
      let st = self.status().await;
      if pred(&st.key_state) {
        return st;
      }
      assert!(start.elapsed() < Duration::from_secs(10), "stuck in key state {:?}", st.key_state);
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
  }

  async fn next_call(&mut self) -> Call {
    tokio::time::timeout(Duration::from_secs(5), self.calls.recv())
      .await
      .expect("timed out waiting for a wayland call")
      .unwrap()
  }

  /// A foreign clipboard offer of `text` (empty = clear), fetched.
  async fn offer(&mut self, text: &str) {
    let n = self.next_offer;
    self.next_offer += 1;
    self
      .wl
      .send(WaylandEvent::NewSelection {
        selection: Selection::Clipboard,
        mimes: vec![UTF8.into()],
        offer: OfferToken::from_raw(n),
        ours: false,
      })
      .unwrap();
    let Call::Fetch(offer) = self.next_call().await else { panic!("expected a fetch") };
    assert_eq!(offer.raw(), n);
    self
      .wl
      .send(WaylandEvent::Fetched {
        offer: OfferToken::from_raw(n),
        result: Ok(vec![(UTF8.into(), text.as_bytes().to_vec())]),
      })
      .unwrap();
    self.status().await; // processed
  }

  async fn current(&self) -> Option<String> {
    match self.ask(PublicReq::Current).await {
      PublicResp::Current { data, .. } => Some(String::from_utf8(data).unwrap()),
      PublicResp::Empty => None,
      other => panic!("unexpected {other:?}"),
    }
  }

  async fn stop(mut self) {
    let _ = self.stop.take().unwrap().send(());
    self.task.await.unwrap().unwrap();
    self.key_task.abort();
  }
}

/// Newest-first previews of the persistent store in `dir`.
async fn persisted_previews(dir: &Path, provider: &Arc<FakeProvider>) -> Vec<String> {
  let slots = KeySlots::load(dir.join(spool_keys::FILE_NAME)).unwrap();
  let p: &dyn KeyProvider = &**provider;
  let (key, _) = slots.unlock_any(&[p]).await.unwrap();
  let store = Store::open_encrypted(dir, &DataKey::from_bytes(key)).unwrap();
  store.recent(100).unwrap().into_iter().map(|s| s.preview.unwrap_or_default()).collect()
}

fn is_error(s: &str) -> bool {
  s.starts_with("error: ")
}

// ---- tests -------------------------------------------------------------------

#[tokio::test]
async fn first_run_creates_slots_and_store() {
  let d = tempfile::tempdir().unwrap();
  let provider = FakeProvider::new(false);
  let mut h = H::start(d.path(), &provider);
  let st = h.wait_state(|s| s == "ready").await;
  assert!(st.encrypted && st.unlocked);
  assert_eq!(provider.enrolls.load(Ordering::SeqCst), 1);
  assert!(d.path().join(spool_keys::FILE_NAME).is_file());
  assert!(d.path().join(DB_FILE_NAME).is_file());
  h.offer("persisted").await;
  assert_eq!(h.status().await.item_count, 1);
  h.stop().await;
  assert_eq!(persisted_previews(d.path(), &provider).await, ["persisted"]);
}

#[tokio::test]
async fn capture_while_locked_merges_on_unlock() {
  let d = tempfile::tempdir().unwrap();
  let provider = FakeProvider::new(false);

  // Run 1: existing persistent history (ids 1, 2).
  let mut h = H::start(d.path(), &provider);
  h.wait_state(|s| s == "ready").await;
  h.offer("old a").await;
  h.offer("old b").await;
  h.stop().await;

  // Run 2: the wallet is locked at start.
  provider.lock();
  let mut h = H::start(d.path(), &provider);
  let st = h.wait_state(|s| s == "waiting-for-wallet").await;
  assert!(!st.encrypted && !st.unlocked);
  assert_eq!(st.item_count, 0, "session store starts empty");
  h.offer("new 1").await;
  h.offer("new 2").await;
  h.offer("new 3").await;
  let st = h.status().await;
  assert_eq!((st.item_count, st.encrypted), (3, false));
  assert_eq!(h.current().await.as_deref(), Some("new 3"));

  // Locked: one attempt, then it waits (no retry loop).
  let calls = provider.calls();
  tokio::time::sleep(Duration::from_millis(300)).await;
  assert_eq!(provider.calls(), calls, "key flow must not spin while the wallet is locked");
  assert_eq!(provider.waits.load(Ordering::SeqCst), 1);
  assert_eq!(h.status().await.key_state, "waiting-for-wallet");

  // Unlock: session items merged into the persistent store.
  provider.unlock_now();
  let st = h.wait_state(|s| s == "ready").await;
  assert!(st.encrypted && st.unlocked);
  assert_eq!(st.item_count, 5);
  assert_eq!(h.current().await.as_deref(), Some("new 3"));

  // Keep-alive candidate was remapped (session id 3 is persistent id 5;
  // persistent id 3 would be "new 1").
  h.wl.send(WaylandEvent::SelectionCleared { selection: Selection::Clipboard }).unwrap();
  let Call::Set(reps) = h.next_call().await else { panic!("expected keep-alive") };
  assert!(reps.iter().any(|(m, d)| m == UTF8 && d == b"new 3"), "{reps:?}");

  // Clear detection also uses the remapped id: purges "new 3".
  h.offer("").await;
  assert_eq!(h.status().await.item_count, 4);
  assert_eq!(h.current().await.as_deref(), Some("new 2"));

  // New captures now go to the encrypted store.
  h.offer("after unlock").await;
  assert_eq!(h.status().await.item_count, 5);
  h.stop().await;

  assert_eq!(
    persisted_previews(d.path(), &provider).await,
    ["after unlock", "new 2", "new 1", "old b", "old a"]
  );
}

#[tokio::test]
async fn first_run_with_locked_wallet_waits_then_creates() {
  let d = tempfile::tempdir().unwrap();
  let provider = FakeProvider::new(true);
  let mut h = H::start(d.path(), &provider);
  h.wait_state(|s| s == "waiting-for-wallet").await;
  assert!(!d.path().join(spool_keys::FILE_NAME).exists());
  h.offer("while locked").await;
  tokio::time::sleep(Duration::from_millis(200)).await;
  assert_eq!(provider.enrolls.load(Ordering::SeqCst), 1);
  provider.unlock_now();
  let st = h.wait_state(|s| s == "ready").await;
  assert_eq!((st.item_count, st.encrypted), (1, true));
  assert_eq!(provider.enrolls.load(Ordering::SeqCst), 2);
  h.stop().await;
  assert_eq!(persisted_previews(d.path(), &provider).await, ["while locked"]);
}

#[tokio::test]
async fn fatal_errors_stay_on_the_session_store() {
  let d = tempfile::tempdir().unwrap();
  let provider = FakeProvider::new(false);
  let mut h = H::start(d.path(), &provider);
  h.wait_state(|s| s == "ready").await;
  h.offer("persisted").await;
  h.stop().await;

  for (mode, needle) in [(Mode::Fatal, "fake fatal error"), (Mode::WrongKek, "wrong key")] {
    provider.set_mode(mode);
    let mut h = H::start(d.path(), &provider);
    let st = h.wait_state(is_error).await;
    assert!(st.key_state.contains(needle), "{mode:?}: {}", st.key_state);
    assert!(!st.encrypted && !st.unlocked);
    // Capture still works, in memory.
    h.offer("in memory").await;
    assert_eq!(h.status().await.item_count, 1);
    assert_eq!(h.current().await.as_deref(), Some("in memory"));
    let calls = provider.calls();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(provider.calls(), calls, "no retries after a fatal error");
    h.stop().await;
  }

  // Nothing was wiped.
  provider.set_mode(Mode::Ok);
  assert_eq!(persisted_previews(d.path(), &provider).await, ["persisted"]);
}

#[tokio::test]
async fn slot_with_a_foreign_data_key_is_fatal() {
  let d = tempfile::tempdir().unwrap();
  let provider = FakeProvider::new(false);
  let h = H::start(d.path(), &provider);
  h.wait_state(|s| s == "ready").await;
  h.stop().await;

  // A keyslots file for a different data key: the slot unwraps fine, but
  // the key does not open the history (keyslots has no key check value).
  let path = d.path().join(spool_keys::FILE_NAME);
  let saved = std::fs::read(&path).unwrap();
  std::fs::remove_file(&path).unwrap();
  KeySlots::create_new(&path, &*provider).await.unwrap();
  let h = H::start(d.path(), &provider);
  let st = h.wait_state(is_error).await;
  assert!(st.key_state.contains("does not open the history"), "{}", st.key_state);
  h.stop().await;

  // An existing history without key slots is not replaced by a new one.
  std::fs::remove_file(&path).unwrap();
  let h = H::start(d.path(), &provider);
  let st = h.wait_state(is_error).await;
  assert!(st.key_state.contains("keyslots.json is missing"), "{}", st.key_state);
  assert!(!path.exists());
  h.stop().await;

  std::fs::write(&path, saved).unwrap();
  assert!(persisted_previews(d.path(), &provider).await.is_empty());
}

#[tokio::test]
async fn interrupted_rotation_stays_session_only() {
  let d = tempfile::tempdir().unwrap();
  let provider = FakeProvider::new(false);
  let mut h = H::start(d.path(), &provider);
  h.wait_state(|s| s == "ready").await;
  h.offer("persisted").await;
  h.stop().await;

  let slots = d.path().join(spool_keys::FILE_NAME);
  for suffix in [".next", ".old"] {
    let marker = d.path().join(format!("{}{suffix}", spool_keys::FILE_NAME));
    std::fs::copy(&slots, &marker).unwrap();
    let calls = provider.calls();
    let mut h = H::start(d.path(), &provider);
    let st = h.wait_state(is_error).await;
    assert_eq!(st.key_state, format!("error: {}", keyflow::INTERRUPTED_ROTATION), "{suffix}");
    assert!(!st.encrypted && !st.unlocked);
    assert_eq!(provider.calls(), calls, "{suffix}: no slot unlocked or created");
    // Capture still works, in memory.
    h.offer("in memory").await;
    assert_eq!(h.status().await.item_count, 1);
    h.stop().await;
    std::fs::remove_file(&marker).unwrap();
  }

  // First run with only a staged `.next` (keyslots.json gone): nothing is
  // created either.
  let saved = std::fs::read(&slots).unwrap();
  let next = d.path().join(format!("{}.next", spool_keys::FILE_NAME));
  std::fs::rename(&slots, &next).unwrap();
  let calls = provider.calls();
  let h = H::start(d.path(), &provider);
  let st = h.wait_state(is_error).await;
  assert!(st.key_state.contains("spool-keyctl recover"), "{}", st.key_state);
  assert_eq!(provider.calls(), calls);
  assert!(!slots.exists(), "no new keyslots.json");
  h.stop().await;

  std::fs::remove_file(&next).unwrap();
  std::fs::write(&slots, saved).unwrap();
  assert_eq!(persisted_previews(d.path(), &provider).await, ["persisted"]);
}

#[tokio::test]
async fn session_only_and_dev_plaintext_states() {
  let (ctx, _calls) = mpsc::unbounded_channel();
  let store = Store::open_session(&DataKey::generate()).unwrap();
  let orch = Orchestrator::new(Config::default(), store, FakeWayland(ctx)).unwrap();
  assert_eq!(orch.key_state(), &KeyState::SessionOnly);
  assert_eq!(KeyState::SessionOnly.describe(), "session-only");
  let (ctx, _calls) = mpsc::unbounded_channel();
  let orch =
    Orchestrator::new(Config::default(), Store::open_in_memory().unwrap(), FakeWayland(ctx))
      .unwrap();
  assert_eq!(orch.key_state(), &KeyState::DevPlaintext);
  assert_eq!(KeyState::Failed("x".into()).describe(), "error: x");
}

#[path = "index_tests.rs"]
mod index_tests;
