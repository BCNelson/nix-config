//! Orchestrator: the single task that owns the store, the policy state and
//! the Wayland handle, and serializes everything that touches them.
//!
//! Flow per Wayland event:
//! * `NewSelection { ours: true }` -> ignore (our own re-publish/copy).
//! * `NewSelection { ours: false }` -> `Policy::plan` -> maybe hint fetch ->
//!   `WaylandHandle::fetch` -> on `Fetched`, `Policy::evaluate` -> act on the
//!   `Decision` (`Store::insert` / `Store::delete`) -> report back to
//!   `PolicyState` (`record_stored` / `record_dropped` / `record_purged`).
//!   Only the newest pending offer per selection matters; stale `Fetched`
//!   results are discarded.
//! * `SelectionCleared` -> keep-alive: re-publish
//!   `PolicyState::keepalive_candidate` via `WaylandHandle::set_selection`
//!   unless missing or `SENSITIVE`.
//! * `Fatal` -> return `Err` (daemon exits; systemd restarts it).
//!
//! Hourly: `Store::retention_sweep(now, config.retention())`.
//!
//! # Stores and the key state (M2)
//!
//! The daemon starts on an in-memory session store ([`KeyState::Locked`])
//! and captures immediately while [`crate::keyflow`] obtains the data key in
//! its own task ([`KeyEvent`]s: `Waiting` -> `WaitingForWallet`, `Unlocking`,
//! `Opened`, `Failed`). On `Opened` the orchestrator compiles a new `Policy`
//! with the persistent hash key, then runs one store-actor job that merges
//! the session into the encrypted store and swaps it in (so no store
//! operation can interleave), remaps the item ids `PolicyState` holds
//! (keep-alive / clear candidates) through `MergeReport::ids`, and is then
//! [`KeyState::Ready`]. A failed merge keeps the session store (and the
//! old policy) and reports [`KeyState::Failed`].
//!
//! The `Store` is synchronous (rusqlite), so it lives on a dedicated thread
//! ([`StoreActor`]); the orchestrator sends it closures and awaits the
//! results, so it never blocks a runtime worker. The Wayland side is behind
//! the small [`WaylandSide`] trait so tests can drive the orchestrator with
//! a fake.
//!
//! # Desktop integration and picking (M5)
//!
//! With [`Orchestrator::with_desktop`] the orchestrator also drains the
//! compositor's events ([`crate::desktop`]): `ActiveWindow` keeps the
//! focus tracker current (the tracker is fed by the backend itself; the
//! active window's app id becomes `OfferInfo::source_app` of new offers, a
//! heuristic that makes `excluded_apps` work), `Show` (rate limited with the
//! socket's `Show`/`Pick` limiter) records a [`ShowContext`] and goes to the
//! [`PickerLauncher`]. `Request::Select` publishes an item and runs the
//! auto-paste flow described in [`crate::autopaste`].
//!
//! Never log clipboard content: only hash prefixes, lengths, mimes and
//! decision reasons. App ids only at debug level; never window titles.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context;
use bytes::Bytes;
use spool_compositor::{ActiveWindowTracker, CompositorEvent};
use spool_core::config::Config;
use spool_core::item::ItemId;
use spool_core::item::{ItemFlags, NewItem, Representation, Selection, TEXT_MIMES, hash_prefix};
use spool_core::policy::{
  Decision, DropReason, FetchPlan, FetchSpec, OfferInfo, PASSWORD_MANAGER_HINT, PauseUntil, Policy,
  PolicyState,
};
use spool_core::store::{InsertOutcome, MergeReport, Store, StoreKind};
use spool_crypto::DataKey;
use spool_proto::{
  ErrorCode, HideReason, ItemPreview, PauseState, PickerErrorCode, PublicReq, PublicResp,
  QueryFilters, SelectMode, StatusInfo,
};
use spool_search::{IndexIdentity, Opened, SearchIndex};
use spool_wayland::{
  DATA_CONTROL_GLOBAL, FetchError, FetchResult, OfferToken, WaylandError, WaylandEvent,
  WaylandHandle,
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::autopaste::{
  ACK_TIMEOUT, AutoPaste, LockView, NoPaste, PasteTarget, PickerError, PickerLauncher, SelectError,
  SelectOutcome, ShowContext, ShowOrigin, UnwiredPicker,
};
use crate::desktop::DesktopLink;
use crate::index::{self, IndexActor, IndexEvent, IndexStatus, SearchError};
use crate::ipc::RateLimiter;
use crate::keyflow::{KeyEvent, OpenedStore};
use crate::security::PeerCred;

/// Bound of the IPC -> orchestrator request queue.
pub const REQUEST_QUEUE: usize = 64;

/// Interval between retention sweeps.
pub const RETENTION_INTERVAL: Duration = Duration::from_secs(3600);

/// Longest wait at shutdown for a background index open / rebuild. An
/// interrupted rebuild is crash-safe (it is redone / caught up next start).
const INDEX_SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

/// Longest wait at shutdown for the index writer's final commit (skipped
/// work is caught up from the store at the next start).
const INDEX_COMMIT_WAIT: Duration = Duration::from_secs(3);

/// Longest wait at shutdown for the store thread to close the database
/// (committed transactions are durable in the WAL either way).
const STORE_CLOSE_WAIT: Duration = Duration::from_secs(4);

/// Size cap for the password-manager hint fetch (`secret` is 6 bytes).
const HINT_CAP: usize = 1024;

/// Longest MIME type accepted from `Copy`.
const MAX_MIME_LEN: usize = 255;

/// Reply channel of [`Request::Search`].
pub type SearchReply = oneshot::Sender<Result<Vec<ItemPreview>, SearchError>>;

/// Reply channel of [`Request::Select`].
pub type SelectReply = oneshot::Sender<Result<SelectOutcome, SelectError>>;

/// Shared `Show`/`Pick` rate limiter (socket and hotkey).
pub type ShowLimiter = Arc<Mutex<RateLimiter>>;

/// Reply of the picker's item requests: an error code plus a short message
/// for the picker's status line (never content).
pub type PickerReply<T> = oneshot::Sender<Result<T, (PickerErrorCode, &'static str)>>;

/// Mime types a thumbnail may be served for (the picker never gets the full
/// text of an item, only previews and images).
pub const THUMB_MIMES: &[&str] = &["image/png", "image/jpeg", "image/webp"];

/// Longest tag accepted from the picker.
pub const MAX_TAG_LEN: usize = 64;

/// Item edits from the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOp {
  Pin(bool),
  Delete,
  Tag { tag: String, on: bool },
}

/// A request to the orchestrator.
pub enum Request {
  /// From the public socket. The IPC layer has already done the handshake,
  /// peer checks and `Show`/`Pick` rate limiting.
  Public { req: PublicReq, peer: PeerCred, reply: oneshot::Sender<PublicResp> },
  /// History search for the picker (see [`index::search`]). Only the picker
  /// channel may send this; it is never reachable from the public socket
  /// (only the picker may read history).
  Search { q: String, filters: QueryFilters, offset: u32, limit: u32, reply: SearchReply },
  /// The user picked item `id` in the picker. Only the picker channel (and
  /// test hooks) may send this. `Copy`: publish as the clipboard selection
  /// and bump `last_used_at`. `Paste` / `PastePlain` (text reps only):
  /// additionally auto-paste into the target of the last `Show` (consumed),
  /// see [`crate::autopaste`]. The reply comes once the flow is finished
  /// (after the paste, or why it did not happen).
  /// While a `spoolctl pick` waits, the item goes back to it instead
  /// ([`PublicResp::Picked`], [`SelectOutcome::Returned`]).
  Select { id: ItemId, mode: SelectMode, reply: SelectReply },
  /// Thumbnail source bytes of item `id`'s `mime` (picker channel only;
  /// [`THUMB_MIMES`], at most `MAX_THUMB_BYTES`).
  Thumb { id: ItemId, mime: String, reply: PickerReply<Vec<u8>> },
  /// Pin / delete / tag from the picker (the index follows via the store's
  /// change feed).
  Edit { id: ItemId, op: EditOp, reply: PickerReply<()> },
  /// The picker hid itself (`Hidden{reason}`): cancels a pending
  /// `spoolctl pick` unless `Selected` (a `Select` follows).
  PickerHidden { reason: HideReason },
}

impl std::fmt::Debug for Request {
  // Never prints the query or clipboard data.
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Request::Public { req, peer, .. } => f
        .debug_struct("Public")
        .field("req", &crate::ipc::req_name(req))
        .field("peer", peer)
        .finish_non_exhaustive(),
      Request::Search { offset, limit, .. } => f
        .debug_struct("Search")
        .field("offset", offset)
        .field("limit", limit)
        .finish_non_exhaustive(),
      Request::Select { id, mode, .. } => {
        f.debug_struct("Select").field("id", id).field("mode", mode).finish_non_exhaustive()
      }
      Request::Thumb { id, mime, .. } => {
        f.debug_struct("Thumb").field("id", id).field("mime", mime).finish_non_exhaustive()
      }
      Request::Edit { id, op, .. } => {
        let op = match op {
          EditOp::Pin(on) => format!("Pin({on})"),
          EditOp::Delete => "Delete".into(),
          EditOp::Tag { on, .. } => format!("Tag(on: {on})"),
        };
        f.debug_struct("Edit").field("id", id).field("op", &op).finish_non_exhaustive()
      }
      Request::PickerHidden { reason } => {
        f.debug_struct("PickerHidden").field("reason", reason).finish()
      }
    }
  }
}

/// The orchestrator's view of the Wayland thread (implemented by
/// [`WaylandHandle`]; tests use a fake).
pub trait WaylandSide: Send + 'static {
  fn fetch(
    &self,
    offer: OfferToken,
    mimes: Vec<String>,
    per_rep_cap: usize,
    total_cap: usize,
    timeout: Duration,
  ) -> Result<(), WaylandError>;

  fn set_selection(
    &self,
    selection: Selection,
    reps: Vec<(String, Arc<[u8]>)>,
  ) -> Result<(), WaylandError>;
}

impl WaylandSide for WaylandHandle {
  fn fetch(
    &self,
    offer: OfferToken,
    mimes: Vec<String>,
    per_rep_cap: usize,
    total_cap: usize,
    timeout: Duration,
  ) -> Result<(), WaylandError> {
    WaylandHandle::fetch(self, offer, mimes, per_rep_cap, total_cap, timeout)
  }

  fn set_selection(
    &self,
    selection: Selection,
    reps: Vec<(String, Arc<[u8]>)>,
  ) -> Result<(), WaylandError> {
    WaylandHandle::set_selection(self, selection, reps)
  }
}

/// What the store thread owns: the store, plus the search-index feed that
/// runs after every job ([`index::pump`]).
pub struct StoreCtx {
  pub store: Store,
  /// Live feed into the current index writer, if attached.
  pub feed: Option<index::FeedSink>,
  /// Index generation the store currently belongs to; back-fill jobs of an
  /// older generation stop.
  pub generation: u64,
}

type StoreJob = Box<dyn FnOnce(&mut StoreCtx) + Send>;

enum StoreMsg {
  Job(StoreJob),
  Stop,
}

/// Cloneable handle that submits jobs to the [`StoreActor`] thread.
#[derive(Clone)]
pub struct StoreHandle {
  tx: std::sync::mpsc::Sender<StoreMsg>,
}

impl StoreHandle {
  fn submit(&self, job: StoreJob) -> anyhow::Result<()> {
    self.tx.send(StoreMsg::Job(job)).map_err(|_| anyhow::anyhow!("store thread is gone"))
  }

  /// Run `f` on the store thread and await its result.
  pub async fn call<T, F>(&self, f: F) -> anyhow::Result<T>
  where
    T: Send + 'static,
    F: FnOnce(&mut Store) -> T + Send + 'static,
  {
    self.call_ctx(move |c: &mut StoreCtx| f(&mut c.store)).await
  }

  /// Like [`call`](Self::call), with access to the index feed state.
  pub async fn call_ctx<T, F>(&self, f: F) -> anyhow::Result<T>
  where
    T: Send + 'static,
    F: FnOnce(&mut StoreCtx) -> T + Send + 'static,
  {
    let (rtx, rrx) = oneshot::channel();
    self.submit(Box::new(move |c| {
      let _ = rtx.send(f(c));
    }))?;
    rrx
      .await
      .map_err(|_| anyhow::anyhow!("store thread dropped the request (shut down or panicked)"))
  }

  /// [`call`](Self::call) from a blocking (non-runtime) thread.
  pub fn call_blocking<T, F>(&self, f: F) -> anyhow::Result<T>
  where
    T: Send + 'static,
    F: FnOnce(&mut Store) -> T + Send + 'static,
  {
    self.call_ctx_blocking(move |c: &mut StoreCtx| f(&mut c.store))
  }

  /// [`call_ctx`](Self::call_ctx) from a blocking (non-runtime) thread.
  pub fn call_ctx_blocking<T, F>(&self, f: F) -> anyhow::Result<T>
  where
    T: Send + 'static,
    F: FnOnce(&mut StoreCtx) -> T + Send + 'static,
  {
    let (rtx, rrx) = std::sync::mpsc::sync_channel(1);
    self.submit(Box::new(move |c| {
      let _ = rtx.send(f(c));
    }))?;
    rrx
      .recv()
      .map_err(|_| anyhow::anyhow!("store thread dropped the request (shut down or panicked)"))
  }
}

/// Owns the [`Store`] on a dedicated thread; jobs run in submission order.
/// After every job the search-index feed is pumped ([`index::pump`]).
pub struct StoreActor {
  handle: StoreHandle,
  thread: Option<std::thread::JoinHandle<()>>,
}

impl StoreActor {
  pub fn spawn(store: Store) -> anyhow::Result<Self> {
    let (tx, rx) = std::sync::mpsc::channel::<StoreMsg>();
    let thread = std::thread::Builder::new()
      .name("spool-store".into())
      .spawn(move || {
        let mut ctx = StoreCtx { store, feed: None, generation: 0 };
        // `Stop` (or every handle gone) ends the thread even while other
        // handles (searches, back-fills) still exist; they get errors.
        while let Ok(StoreMsg::Job(job)) = rx.recv() {
          job(&mut ctx);
          index::pump(&mut ctx);
        }
        // Dropping the connection checkpoints/flushes the WAL.
        drop(ctx);
        tracing::debug!("store thread exited");
      })
      .context("spawning store thread")?;
    Ok(Self { handle: StoreHandle { tx }, thread: Some(thread) })
  }

  /// A cloneable handle (for tasks that outlive one orchestrator step).
  pub fn handle(&self) -> StoreHandle {
    self.handle.clone()
  }

  /// Run `f` on the store thread and await its result.
  pub async fn call<T, F>(&self, f: F) -> anyhow::Result<T>
  where
    T: Send + 'static,
    F: FnOnce(&mut Store) -> T + Send + 'static,
  {
    self.handle.call(f).await
  }

  /// Run `f` with the feed state on the store thread.
  pub async fn call_ctx<T, F>(&self, f: F) -> anyhow::Result<T>
  where
    T: Send + 'static,
    F: FnOnce(&mut StoreCtx) -> T + Send + 'static,
  {
    self.handle.call_ctx(f).await
  }

  /// Stop after the queued jobs, close the DB and join.
  pub async fn shutdown(mut self) {
    let _ = self.handle.tx.send(StoreMsg::Stop);
    if let Some(t) = self.thread.take() {
      let _ = tokio::task::spawn_blocking(move || t.join()).await;
    }
  }
}

impl Drop for StoreActor {
  fn drop(&mut self) {
    // Ends the thread; we do not join here (may be in async context).
    // `shutdown` is the orderly path.
    let _ = self.handle.tx.send(StoreMsg::Stop);
  }
}

/// Where history goes and how far unlocking got. Reported in
/// `StatusInfo::key_state` (see [`KeyState::describe`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyState {
  /// Session store; the key flow has not reported yet.
  Locked,
  /// Session store; the key store (wallet) is locked or unavailable.
  WaitingForWallet,
  /// Session store; a key was obtained, opening / merging.
  Unlocking,
  /// Encrypted persistent store in use.
  Ready,
  /// `key_provider = "session"`: memory only, by configuration.
  SessionOnly,
  /// Plaintext store (`SPOOL_DEV_PLAINTEXT=1` or tests).
  DevPlaintext,
  /// Gave up unlocking; session store (memory only) until restart.
  Failed(String),
}

impl KeyState {
  /// The `StatusInfo::key_state` string.
  pub fn describe(&self) -> String {
    match self {
      KeyState::Locked => "locked".into(),
      KeyState::WaitingForWallet => "waiting-for-wallet".into(),
      KeyState::Unlocking => "unlocking".into(),
      KeyState::Ready => "ready".into(),
      KeyState::SessionOnly => "session-only".into(),
      KeyState::DevPlaintext => "dev-plaintext".into(),
      KeyState::Failed(why) => format!("error: {why}"),
    }
  }

  /// Initial state for a store that needs no key flow.
  fn for_store(kind: StoreKind) -> Self {
    match kind {
      StoreKind::Encrypted => KeyState::Ready,
      StoreKind::Session => KeyState::SessionOnly,
      StoreKind::Plain => KeyState::DevPlaintext,
    }
  }
}

/// A fetch in flight for the newest offer of a selection.
#[derive(Debug)]
struct Pending {
  offer: OfferToken,
  info: OfferInfo,
  stage: Stage,
}

#[derive(Debug)]
enum Stage {
  Hint,
  Fetch(FetchSpec),
}

/// The search index the orchestrator currently feeds and queries.
struct IndexState {
  /// Bumped on every switch (RAM -> disk, rekey); stale background work
  /// compares against it.
  generation: u64,
  actor: Option<IndexActor>,
  /// Searchable index whose ids are the current store's; `None` while
  /// switching / rebuilding (search falls back to the recent list).
  search: Option<SearchIndex>,
  /// Data key + state dir of the encrypted store (on-disk index).
  disk: Option<(Arc<DataKey>, PathBuf)>,
  status: watch::Sender<IndexStatus>,
  events_tx: mpsc::UnboundedSender<IndexEvent>,
  events_rx: Option<mpsc::UnboundedReceiver<IndexEvent>>,
  /// The background open / rebuild of the on-disk index, if running.
  opening: Option<tokio::task::JoinHandle<()>>,
}

/// A paste waiting for the compositor to confirm our publish.
struct PendingPaste {
  /// Clipboard publish number that must be confirmed.
  publish: u64,
  target: PasteTarget,
  reply: SelectReply,
  deadline: tokio::time::Instant,
  /// `show_seq` when the paste started: a newer `Show` means the picker
  /// is open again and must not be hidden by this paste.
  show_seq: u64,
}

pub struct Orchestrator<W: WaylandSide = WaylandHandle> {
  config: Config,
  index: IndexState,
  policy: Policy,
  state: PolicyState,
  store: StoreActor,
  store_kind: StoreKind,
  key_state: KeyState,
  key_events: Option<mpsc::Receiver<KeyEvent>>,
  wayland: W,
  compositor: Option<String>,
  primary_supported: Option<bool>,
  pending: HashMap<Selection, Pending>,
  /// Compositor events (focus, `Show`), if a desktop integration exists.
  desktop_events: Option<mpsc::Receiver<CompositorEvent>>,
  /// Active window (source-app attribution, paste targets).
  focus: Option<ActiveWindowTracker>,
  autopaste: AutoPaste,
  capabilities: Option<watch::Receiver<spool_proto::Capabilities>>,
  picker: Box<dyn PickerLauncher>,
  show_limiter: ShowLimiter,
  /// The last `Show` (cursor + paste target), consumed by a paste.
  show_ctx: Option<ShowContext>,
  /// Clipboard publishes made / confirmed by `NewSelection { ours: true }`.
  clip_published: u64,
  clip_confirmed: u64,
  pending_paste: Option<PendingPaste>,
  /// Shows made so far (see `PendingPaste::show_seq`).
  show_seq: u64,
  /// A `spoolctl pick` waiting for the user's choice.
  pick: Option<oneshot::Sender<PublicResp>>,
  /// The background key flow, stopped once history is ready.
  key_task: Option<tokio::task::AbortHandle>,
  /// The lock state last reported to the picker.
  last_lock: Option<LockView>,
}

impl<W: WaylandSide> Orchestrator<W> {
  /// Build the orchestrator: compiles the `Policy` with `store.hash_key()`,
  /// creates a fresh `PolicyState`, moves the store onto its thread.
  pub fn new(config: Config, mut store: Store, wayland: W) -> anyhow::Result<Self> {
    let key = store.hash_key().context("reading the dedupe hash key")?;
    let policy = Policy::new(config.clone(), key).context("compiling the ingest policy")?;
    let store_kind = store.kind();
    let config_for_paste = config.clone();
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    Ok(Self {
      index: IndexState {
        generation: 0,
        actor: None,
        search: None,
        disk: None,
        status: watch::channel(IndexStatus::Pending).0,
        events_tx,
        events_rx: Some(events_rx),
        opening: None,
      },
      config,
      policy,
      state: PolicyState::new(),
      store: StoreActor::spawn(store)?,
      store_kind,
      key_state: KeyState::for_store(store_kind),
      key_events: None,
      wayland,
      compositor: None,
      primary_supported: None,
      pending: HashMap::new(),
      desktop_events: None,
      focus: None,
      autopaste: AutoPaste::unavailable(&config_for_paste),
      capabilities: None,
      picker: Box::new(UnwiredPicker),
      show_limiter: crate::ipc::show_limiter(),
      show_ctx: None,
      clip_published: 0,
      clip_confirmed: 0,
      pending_paste: None,
      show_seq: 0,
      pick: None,
      key_task: None,
      last_lock: None,
    })
  }

  /// Attach the desktop integration (compositor events, focus tracker,
  /// auto-paste backends, capabilities for `Status`).
  pub fn with_desktop(mut self, link: DesktopLink) -> Self {
    self.desktop_events = Some(link.events);
    self.focus = link.focus;
    self.autopaste = link.autopaste;
    self.capabilities = Some(link.capabilities);
    self
  }

  /// Replace the picker launcher (default: [`UnwiredPicker`]).
  pub fn with_picker(mut self, picker: Box<dyn PickerLauncher>) -> Self {
    self.picker = picker;
    self
  }

  /// Share the socket's `Show`/`Pick` rate limiter, so hotkey and socket
  /// `Show`s draw from the same budget.
  pub fn with_show_limiter(mut self, limiter: ShowLimiter) -> Self {
    self.show_limiter = limiter;
    self
  }

  /// Attach a key flow ([`crate::keyflow::run`]): the state becomes
  /// [`KeyState::Locked`] and `events` drive the switch to the encrypted
  /// store.
  pub fn with_key_flow(mut self, events: mpsc::Receiver<KeyEvent>) -> Self {
    self.key_state = KeyState::Locked;
    self.key_events = Some(events);
    self
  }

  /// The key flow task: aborted once history is ready by any route (it
  /// may still be waiting for the wallet after a picker unlock).
  pub fn with_key_task(mut self, task: tokio::task::AbortHandle) -> Self {
    self.key_task = Some(task);
    self
  }

  /// Current key state.
  #[cfg(test)]
  pub fn key_state(&self) -> &KeyState {
    &self.key_state
  }

  /// Search index status, including rebuild progress for the picker's
  /// `IndexProgress` events.
  pub fn index_status(&self) -> watch::Receiver<IndexStatus> {
    self.index.status.subscribe()
  }

  /// Run until the Wayland side reports `Fatal`, its channel closes, the
  /// request channel closes, or `shutdown` resolves. The store is flushed
  /// and closed before returning.
  pub async fn run(
    mut self,
    mut wl_events: mpsc::UnboundedReceiver<WaylandEvent>,
    mut requests: mpsc::Receiver<Request>,
    shutdown: impl Future<Output = ()>,
  ) -> anyhow::Result<()> {
    let mut sweep = tokio::time::interval(RETENTION_INTERVAL);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tokio::pin!(shutdown);
    let mut key_events = self.key_events.take();
    let mut index_events = self.index.events_rx.take().expect("run() called once");
    let mut desktop_events = self.desktop_events.take();
    if self.index.actor.is_none() {
      self.start_ram_index().await;
    }
    self.notify_lock();
    let result = loop {
      let ack_deadline = self.pending_paste.as_ref().map(|p| p.deadline);
      // Biased: Wayland events before requests, so a request sent after an
      // event observes its effect.
      tokio::select! {
        biased;
        () = &mut shutdown => break Ok(()),
        ev = wl_events.recv() => match ev {
          Some(ev) => {
            if let Err(e) = self.handle_wayland(ev).await {
              break Err(e);
            }
          }
          None => break Err(anyhow::anyhow!("wayland thread exited unexpectedly")),
        },
        Some(ev) = recv_key_event(&mut key_events) => {
          self.handle_key_event(ev).await;
          self.notify_lock();
        }
        Some(ev) = index_events.recv() => self.handle_index_event(ev).await,
        Some(ev) = recv_opt(&mut desktop_events) => self.handle_desktop(ev),
        () = sleep_until_opt(ack_deadline) => self.paste_not_confirmed(),
        req = requests.recv() => match req {
          Some(Request::Public { req: PublicReq::Pick, peer, reply }) => {
            tracing::debug!(pid = ?peer.pid, req = "Pick", "public request");
            self.start_pick(reply);
          }
          Some(Request::Public { req, peer, reply }) => {
            tracing::debug!(pid = ?peer.pid, req = crate::ipc::req_name(&req), "public request");
            let resp = self.handle_public(req).await;
            let _ = reply.send(resp);
          }
          Some(Request::Search { q, filters, offset, limit, reply }) => {
            self.spawn_search(q, filters, offset, limit, reply);
          }
          Some(Request::Select { id, mode, reply }) => self.handle_select(id, mode, reply).await,
          Some(Request::Thumb { id, mime, reply }) => self.spawn_thumb(id, mime, reply),
          Some(Request::Edit { id, op, reply }) => {
            let r = self.handle_edit(id, op).await;
            let _ = reply.send(r);
          }
          Some(Request::PickerHidden { reason }) => self.picker_hidden(reason),
          None => break Ok(()),
        },
        _ = sweep.tick() => self.retention_sweep().await,
      }
    };
    if let Some(p) = self.pending_paste.take() {
      let _ = p.reply.send(Ok(SelectOutcome::NotPasted(NoPaste::Superseded)));
    }
    if let Some(pick) = self.pick.take() {
      let _ = pick.send(PublicResp::Cancelled);
    }
    // Store first (no more feed), then the index writer commits and exits.
    // Every wait is bounded (see `crate::shutdown`): a loaded machine must
    // not turn SIGTERM into SIGKILL.
    crate::shutdown::phase("closing the store");
    if tokio::time::timeout(STORE_CLOSE_WAIT, self.store.shutdown()).await.is_err() {
      tracing::warn!("store thread still closing the database at shutdown; not waiting");
    } else {
      tracing::debug!("shutdown: store closed");
    }
    if let Some(actor) = self.index.actor.take() {
      crate::shutdown::phase("committing the search index");
      if tokio::time::timeout(INDEX_COMMIT_WAIT, actor.shutdown()).await.is_err() {
        tracing::warn!("search index commit still running at shutdown; not waiting");
      } else {
        tracing::debug!("shutdown: search index committed");
      }
    }
    // A background open / rebuild stops at its next store call (the store
    // is gone; an interrupted rebuild is caught up from scratch next time).
    // Give it a moment to release its index lock, but do not wait for a
    // slow open (it authenticates every file) or rebuild.
    if let Some(t) = self.index.opening.take() {
      crate::shutdown::phase("waiting for the search index open / rebuild");
      if tokio::time::timeout(INDEX_SHUTDOWN_WAIT, t).await.is_err() {
        tracing::warn!("search index open / rebuild still running at shutdown; not waiting");
      }
    }
    result
  }

  /// Answer a search on its own task, so a slow query never stalls capture.
  fn spawn_search(
    &self,
    q: String,
    filters: QueryFilters,
    offset: u32,
    limit: u32,
    reply: SearchReply,
  ) {
    let index = self.index.search.clone();
    let store = self.store.handle();
    tokio::spawn(async move {
      let _ = reply.send(index::search(index, store, q, filters, offset, limit).await);
    });
  }

  /// Start (or restart) on an unencrypted in-memory index fed from the
  /// current store: session / locked / plaintext / session-only.
  async fn start_ram_index(&mut self) {
    let generation = self.index.generation + 1;
    match SearchIndex::in_memory() {
      Ok(opened) => self.attach_index(generation, opened, 0, "memory").await,
      Err(e) => tracing::error!("creating the in-memory search index failed: {e}"),
    }
  }

  /// Make `opened` the current index (generation `generation`): spawn its
  /// writer, catch it up from `cursor` and attach the live feed.
  async fn attach_index(
    &mut self,
    generation: u64,
    opened: Opened,
    cursor: i64,
    what: &'static str,
  ) {
    self.detach_index(generation).await;
    let actor = match IndexActor::spawn(opened, what) {
      Ok(a) => a,
      Err(e) => return tracing::error!("search index ({what}): {e:#}"),
    };
    self.index.search = Some(actor.search());
    tokio::spawn(index::backfill(self.store.handle(), actor.sender(), generation, cursor));
    self.index.actor = Some(actor);
    self.index.status.send_replace(if what == "disk" {
      IndexStatus::Disk
    } else {
      IndexStatus::Memory
    });
  }

  /// Stop feeding and searching the current index (moving to `generation`).
  async fn detach_index(&mut self, generation: u64) {
    self.index.generation = generation;
    self.index.search = None;
    self.index.status.send_replace(IndexStatus::Pending);
    if let Err(e) = self
      .store
      .call_ctx(move |c: &mut StoreCtx| {
        c.feed = None;
        c.generation = generation;
      })
      .await
    {
      tracing::error!("detaching the search index: {e}");
    }
    if let Some(old) = self.index.actor.take() {
      old.shutdown().await;
    }
  }

  /// Open (or rebuild) `$STATE/index` for the encrypted store, in the
  /// background; the result arrives as an [`IndexEvent`].
  async fn start_disk_index(&mut self) {
    let Some((key, state)) = self.index.disk.clone() else { return };
    let generation = self.index.generation;
    let uuid = match self.store.call(|s| s.db_uuid()).await {
      Ok(Ok(u)) => u,
      Ok(Err(e)) => return tracing::error!("search index: reading the database id: {e}"),
      Err(e) => return tracing::error!("search index: {e}"),
    };
    let identity = IndexIdentity::new(uuid);
    let store = self.store.handle();
    let events = self.index.events_tx.clone();
    self.index.opening = Some(tokio::task::spawn_blocking(move || {
      let dir = index::index_dir(&state);
      let ev = match index::open_or_rebuild(&store, &dir, &key, &identity, generation, &events) {
        Ok((opened, cursor)) => {
          IndexEvent::DiskReady { generation, opened: Box::new(opened), cursor }
        }
        Err(e) => IndexEvent::DiskFailed { generation, error: format!("{e:#}") },
      };
      let _ = events.send(ev);
    }));
  }

  async fn handle_index_event(&mut self, ev: IndexEvent) {
    match ev {
      IndexEvent::Progress { generation, progress } if generation == self.index.generation => {
        self.index.status.send_replace(IndexStatus::Rebuilding(progress));
      }
      IndexEvent::DiskReady { generation, opened, cursor }
        if generation == self.index.generation =>
      {
        self.attach_index(generation, *opened, cursor, "disk").await;
      }
      IndexEvent::DiskFailed { generation, error } if generation == self.index.generation => {
        tracing::error!(
          "search index unavailable: {error}; the picker lists recent history only until restart"
        );
        self.index.status.send_replace(IndexStatus::Unavailable);
      }
      _ => tracing::debug!("stale search index event ignored"),
    }
  }

  /// Handle one report from the key flow.
  pub async fn handle_key_event(&mut self, ev: KeyEvent) {
    match ev {
      KeyEvent::Waiting { reason } => {
        tracing::debug!(%reason, "key flow waiting for the key store");
        self.key_state = KeyState::WaitingForWallet;
      }
      KeyEvent::Unlocking => self.key_state = KeyState::Unlocking,
      KeyEvent::Failed(why) => self.key_state = KeyState::Failed(why),
      // A second route (key flow vs. picker unlock) arriving late; the
      // `OpenGate` normally prevents this.
      KeyEvent::Opened(_) if self.key_state == KeyState::Ready => {
        tracing::debug!("history already unlocked; extra store ignored");
      }
      KeyEvent::Opened(opened) => self.switch_to(*opened).await,
    }
    if self.key_state == KeyState::Ready
      && let Some(t) = self.key_task.take()
    {
      // E.g. still waiting for the wallet after a passphrase unlock.
      t.abort();
    }
  }

  /// The picker's view of the key state.
  fn lock_view(&self) -> LockView {
    match &self.key_state {
      KeyState::Ready | KeyState::SessionOnly | KeyState::DevPlaintext => LockView::Unlocked,
      KeyState::Failed(_) => LockView::Locked { failed: true },
      KeyState::Locked | KeyState::WaitingForWallet | KeyState::Unlocking => {
        LockView::Locked { failed: false }
      }
    }
  }

  /// Tell the picker about a lock state change.
  fn notify_lock(&mut self) {
    let v = self.lock_view();
    if self.last_lock != Some(v) {
      self.last_lock = Some(v);
      self.picker.lock_changed(v);
    }
  }

  /// Merge the session store into `opened` and make it the store, as one
  /// store-actor job; then recompile the policy and remap ids.
  async fn switch_to(&mut self, opened: OpenedStore) {
    self.key_state = KeyState::Unlocking;
    let OpenedStore { store, hash_key, data_key, dir } = opened;
    let policy = match Policy::new(self.config.clone(), *hash_key) {
      Ok(p) => p,
      Err(e) => {
        let why = format!("compiling the ingest policy: {e}");
        tracing::error!("{why}; staying on the in-memory session store");
        self.key_state = KeyState::Failed(why);
        return;
      }
    };
    let generation = self.index.generation + 1;
    let res = self
      .store
      .call_ctx(move |ctx: &mut StoreCtx| -> spool_core::Result<(MergeReport, StoreKind)> {
        let mut persistent = store;
        let report = persistent.merge_from_session(&ctx.store)?;
        let kind = persistent.kind();
        // Only after a successful merge: the session store (and the
        // clipboard history it holds in memory) is dropped here. The RAM
        // index's ids are the session's, so its feed goes in the same job.
        drop(std::mem::replace(&mut ctx.store, persistent));
        ctx.feed = None;
        ctx.generation = generation;
        Ok((report, kind))
      })
      .await;
    let (report, kind) = match res {
      Ok(Ok(v)) => v,
      Ok(Err(e)) => {
        let why = format!("merging the in-memory history into the encrypted store: {e}");
        tracing::error!("{why}; staying on the in-memory session store");
        self.key_state = KeyState::Failed(why);
        return;
      }
      Err(e) => {
        tracing::error!("store switch: {e}");
        self.key_state = KeyState::Failed(e.to_string());
        return;
      }
    };
    self.policy = policy;
    let ids: HashMap<ItemId, ItemId> =
      report.ids.iter().map(|(session, outcome)| (*session, outcome.id())).collect();
    self.state.remap_ids(|id| ids.get(&id).copied());
    self.store_kind = kind;
    self.key_state = KeyState::Ready;
    // Search: drop the RAM index, open (or rebuild) the encrypted one.
    self.detach_index(generation).await;
    self.index.disk = Some((Arc::new(data_key), dir));
    self.start_disk_index().await;
    tracing::info!(
      inserted = report.inserted,
      bumped = report.bumped,
      blob_files = report.blob_files,
      "switched to the encrypted history"
    );
    // The merge may have pushed the persistent history over its limits.
    self.retention_sweep().await;
  }

  async fn retention_sweep(&mut self) {
    let limits = self.config.retention();
    match self.store.call(move |s| s.retention_sweep(SystemTime::now(), limits)).await {
      Ok(Ok(0)) => tracing::debug!("retention sweep: nothing to delete"),
      Ok(Ok(n)) => tracing::info!(deleted = n, "retention sweep"),
      Ok(Err(e)) => tracing::error!("retention sweep failed: {e}"),
      Err(e) => tracing::error!("retention sweep: {e}"),
    }
  }

  /// Handle one Wayland event. `Err` only for fatal conditions.
  pub async fn handle_wayland(&mut self, ev: WaylandEvent) -> anyhow::Result<()> {
    match ev {
      WaylandEvent::Ready(info) => {
        tracing::info!(
          display = %info.display,
          version = info.data_control_version,
          primary = info.primary_supported,
          "connected to compositor"
        );
        if self.config.primary_selection && !info.primary_supported {
          tracing::warn!("primary_selection is enabled but the compositor does not support it");
        }
        self.compositor =
          Some(format!("{} ({DATA_CONTROL_GLOBAL} v{})", info.display, info.data_control_version));
        self.primary_supported = Some(info.primary_supported);
      }
      WaylandEvent::NewSelection { selection, mimes, offer, ours } => {
        if ours {
          tracing::debug!(sel = selection.as_str(), "own selection; ignored");
          self.pending.remove(&selection);
          if selection == Selection::Clipboard {
            self.clipboard_confirmed();
          }
          return Ok(());
        }
        if selection == Selection::Clipboard
          && let Some(p) = self.pending_paste.take()
        {
          tracing::info!(
            "another app took the clipboard before our publish was confirmed; not pasting"
          );
          let _ = p.reply.send(Ok(SelectOutcome::NotPasted(NoPaste::ForeignOffer)));
        }
        // Heuristic: the app that copies is nearly always the focused one.
        let source_app = self.focus.as_ref().and_then(|f| f.current_app_id());
        let info = OfferInfo { selection, mimes, source_app, at: SystemTime::now() };
        tracing::debug!(
          sel = selection.as_str(),
          mimes = ?info.mimes,
          app_id = info.source_app.as_deref().unwrap_or(""),
          "new offer"
        );
        let plan = self.policy.plan(&mut self.state, &info);
        self.start_plan(offer, info, plan);
      }
      WaylandEvent::SelectionCleared { selection } => {
        self.pending.remove(&selection);
        self.keep_alive(selection).await;
      }
      WaylandEvent::Fetched { offer, result } => self.on_fetched(offer, result).await,
      WaylandEvent::Fatal(msg) => anyhow::bail!("wayland: {msg}"),
    }
    Ok(())
  }

  /// Act on a phase-1 plan for `offer`.
  fn start_plan(&mut self, offer: OfferToken, info: OfferInfo, plan: FetchPlan) {
    let sel = info.selection;
    let (mimes, per_rep, total, timeout, stage) = match plan {
      FetchPlan::Skip(reason) => {
        tracing::info!(sel = sel.as_str(), ?reason, "offer skipped");
        self.pending.remove(&sel);
        self.state.record_dropped(sel);
        return;
      }
      FetchPlan::CheckHintFirst { timeout } => {
        (vec![PASSWORD_MANAGER_HINT.to_owned()], HINT_CAP, HINT_CAP, timeout, Stage::Hint)
      }
      FetchPlan::Fetch(spec) => {
        (spec.mimes.clone(), spec.per_rep_cap, spec.total_cap, spec.timeout, Stage::Fetch(spec))
      }
    };
    match self.wayland.fetch(offer, mimes, per_rep, total, timeout) {
      Ok(()) => {
        self.pending.insert(sel, Pending { offer, info, stage });
      }
      Err(e) => {
        tracing::warn!(sel = sel.as_str(), "fetch request failed: {e}");
        self.pending.remove(&sel);
        self.state.record_dropped(sel);
      }
    }
  }

  async fn on_fetched(&mut self, offer: OfferToken, result: FetchResult) {
    let Some(sel) = self.pending.iter().find(|(_, p)| p.offer == offer).map(|(s, _)| *s) else {
      tracing::debug!(offer = offer.raw(), "stale fetch result discarded");
      return;
    };
    let Some(Pending { offer, info, stage }) = self.pending.remove(&sel) else { return };
    match stage {
      Stage::Hint => self.after_hint(offer, info, result),
      Stage::Fetch(spec) => self.after_fetch(info, spec, result).await,
    }
  }

  fn after_hint(&mut self, offer: OfferToken, info: OfferInfo, result: FetchResult) {
    let sel = info.selection;
    let reps = match result {
      Ok(r) => r,
      Err(e) => {
        // Fail closed: we could not prove the hint is not "secret".
        tracing::info!(sel = sel.as_str(), "password-manager hint fetch failed ({e}); dropped");
        self.state.record_dropped(sel);
        return;
      }
    };
    let secret = reps
      .iter()
      .find(|(m, _)| m == PASSWORD_MANAGER_HINT)
      .is_some_and(|(_, d)| self.policy.hint_is_secret(d));
    if secret {
      tracing::info!(sel = sel.as_str(), reason = "PasswordManagerHint", "offer dropped");
      self.state.record_dropped(sel);
      return;
    }
    let plan = match self.policy.plan_after_hint(&mut self.state, &info) {
      // Contract: never CheckHintFirst again; treat it as a drop if it is.
      FetchPlan::CheckHintFirst { .. } => {
        tracing::error!("policy returned CheckHintFirst after the hint; dropping");
        self.state.record_dropped(sel);
        return;
      }
      p => p,
    };
    self.start_plan(offer, info, plan);
  }

  async fn after_fetch(&mut self, info: OfferInfo, spec: FetchSpec, result: FetchResult) {
    let sel = info.selection;
    let fetched = match result {
      Ok(r) => r,
      Err(e) => {
        let reason = match e {
          FetchError::TooLarge => "TooLarge",
          FetchError::Timeout => "FetchTimeout",
          FetchError::OfferGone => "OfferGone",
          FetchError::Io(_) => "FetchIo",
        };
        tracing::info!(sel = sel.as_str(), reason, "offer dropped ({e})");
        self.state.record_dropped(sel);
        return;
      }
    };
    let sizes: Vec<(String, usize)> = fetched.iter().map(|(m, d)| (m.clone(), d.len())).collect();
    let reps = fetched.into_iter().map(|(m, d)| Representation::new(m, d)).collect();
    match self.policy.evaluate(&mut self.state, &info, &spec, reps) {
      Decision::Store(item) => {
        self.store_item(item, info.at).await;
      }
      Decision::Drop(reason) => {
        tracing::info!(sel = sel.as_str(), ?reason, reps = ?sizes, "offer dropped");
        self.state.record_dropped(sel);
      }
      Decision::PurgePrevious { previous } => {
        match self.store.call(move |s| s.delete(previous)).await {
          Ok(Ok(found)) => {
            tracing::info!(sel = sel.as_str(), id = %previous, found, "selection cleared; purged previous item");
          }
          Ok(Err(e)) => tracing::error!(id = %previous, "purge failed: {e}"),
          Err(e) => tracing::error!(id = %previous, "purge: {e}"),
        }
        self.state.record_purged(sel);
      }
    }
  }

  /// Insert `item` and record it in the policy state. Returns the outcome.
  async fn store_item(&mut self, item: NewItem, at: SystemTime) -> Option<InsertOutcome> {
    let sel = item.selection;
    let hp = hash_prefix(&item.hash);
    let size = item.total_size();
    let mimes: Vec<String> = item.reps.iter().map(|r| r.mime.clone()).collect();
    // The picker keeps its list warm while hidden: push every stored
    // clipboard item (read in the same store job as the insert).
    let want_preview = sel == Selection::Clipboard && self.picker.wants_items();
    let res = self
      .store
      .call(move |s| {
        let outcome = s.insert(item)?;
        let preview = if want_preview {
          s.summaries(&[outcome.id()]).ok().and_then(|mut v| v.pop())
        } else {
          None
        };
        Ok::<_, spool_core::Error>((outcome, preview))
      })
      .await;
    match res {
      Ok(Ok((outcome, preview))) => {
        if let Some(p) = preview {
          self.picker.item_stored(index::to_preview(p));
        }
        let (kind, id) = match outcome {
          InsertOutcome::Inserted(id) => ("inserted", id),
          InsertOutcome::Bumped(id) => ("bumped", id),
        };
        tracing::info!(sel = sel.as_str(), %id, hash = %hp, bytes = size, ?mimes, "stored ({kind})");
        self.state.record_stored(sel, id, at);
        Some(outcome)
      }
      Ok(Err(e)) => {
        tracing::error!(sel = sel.as_str(), hash = %hp, "store insert failed: {e}");
        self.state.record_dropped(sel);
        None
      }
      Err(e) => {
        tracing::error!(sel = sel.as_str(), hash = %hp, "store insert: {e}");
        self.state.record_dropped(sel);
        None
      }
    }
  }

  /// Re-publish the keep-alive candidate after the compositor cleared
  /// `selection` (source app exited).
  async fn keep_alive(&mut self, selection: Selection) {
    let Some(id) = self.state.keepalive_candidate(selection) else {
      tracing::debug!(sel = selection.as_str(), "selection cleared; nothing to keep alive");
      return;
    };
    let item = match self.store.call(move |s| s.get(id)).await {
      Ok(Ok(Some(item))) => item,
      Ok(Ok(None)) => {
        tracing::debug!(sel = selection.as_str(), %id, "keep-alive candidate no longer stored");
        return;
      }
      Ok(Err(e)) => return tracing::error!(%id, "keep-alive lookup failed: {e}"),
      Err(e) => return tracing::error!(%id, "keep-alive lookup: {e}"),
    };
    if item.flags.contains(ItemFlags::SENSITIVE) {
      tracing::debug!(%id, "keep-alive skipped: sensitive item");
      return;
    }
    let reps = publish_reps(&item.reps);
    if reps.is_empty() {
      return;
    }
    match self.publish(selection, reps) {
      Ok(()) => {
        tracing::info!(sel = selection.as_str(), %id, hash = %hash_prefix(&item.hash), "keep-alive: re-published")
      }
      Err(e) => {
        tracing::warn!(sel = selection.as_str(), %id, "keep-alive set_selection failed: {e}")
      }
    }
  }

  fn pause_state(&mut self, now: SystemTime) -> PauseState {
    match self.state.pause_state(now) {
      None => PauseState::Recording,
      Some(PauseUntil::Indefinite) => PauseState::PausedIndefinitely,
      Some(PauseUntil::Until(t)) => PauseState::PausedUntil {
        unix_ms: t
          .duration_since(SystemTime::UNIX_EPOCH)
          .map(|d| d.as_millis() as u64)
          .unwrap_or(0),
      },
    }
  }

  /// Handle one public request.
  pub async fn handle_public(&mut self, req: PublicReq) -> PublicResp {
    let now = SystemTime::now();
    match req {
      PublicReq::Show => {
        // `spoolctl show` carries no target: use the active window (the
        // terminal it ran in, or whatever the user's shortcut fired over).
        let target = self
          .focus
          .as_ref()
          .and_then(|f| f.current())
          .and_then(|w| w.window_id.map(|window_id| PasteTarget { window_id, app_id: w.app_id }));
        self.cancel_pick();
        match self.show(ShowOrigin::Socket, None, target) {
          Ok(()) => PublicResp::Ok,
          Err(PickerError::NotWired) => PublicResp::NotYetImplemented,
          Err(e) => PublicResp::Error { code: ErrorCode::Unavailable, message: e.to_string() },
        }
      }
      // Answered later, from the run loop (`start_pick`).
      PublicReq::Pick => internal("Pick is handled by the request loop".into()),
      PublicReq::Copy { mime, data } => self.copy(mime, data, now).await,
      PublicReq::Current => match self.store.call(|s| s.latest(Selection::Clipboard)).await {
        Ok(Ok(Some(item))) => match item.preferred() {
          Some((mime, data)) => PublicResp::Current { mime: mime.to_owned(), data: data.to_vec() },
          None => PublicResp::Empty,
        },
        Ok(Ok(None)) => PublicResp::Empty,
        Ok(Err(e)) => internal(format!("store: {e}")),
        Err(e) => internal(e.to_string()),
      },
      PublicReq::Pause { secs } => {
        self.state.pause(now, secs.map(|s| Duration::from_secs(u64::from(s))));
        tracing::info!(secs = ?secs, "recording paused");
        PublicResp::Ok
      }
      PublicReq::Resume => {
        self.state.resume();
        tracing::info!("recording resumed");
        PublicResp::Ok
      }
      PublicReq::Status => {
        let item_count = match self.store.call(|s| s.count()).await {
          Ok(Ok(n)) => n,
          Ok(Err(e)) => return internal(format!("store: {e}")),
          Err(e) => return internal(e.to_string()),
        };
        PublicResp::Status(StatusInfo {
          version: env!("CARGO_PKG_VERSION").to_owned(),
          paused: self.pause_state(now),
          item_count,
          compositor: self.compositor.clone(),
          primary_enabled: self.config.primary_selection && self.primary_supported.unwrap_or(true),
          encrypted: self.store_kind == StoreKind::Encrypted,
          unlocked: matches!(self.key_state, KeyState::Ready | KeyState::DevPlaintext),
          key_state: self.key_state.describe(),
          capabilities: self.capabilities.as_ref().map(|c| c.borrow().clone()),
        })
      }
    }
  }

  /// `set_selection`, counting clipboard publishes for paste confirmation.
  fn publish(
    &mut self,
    selection: Selection,
    reps: Vec<(String, Arc<[u8]>)>,
  ) -> Result<(), WaylandError> {
    self.wayland.set_selection(selection, reps)?;
    if selection == Selection::Clipboard {
      self.clip_published += 1;
    }
    Ok(())
  }

  /// `NewSelection { ours: true }` on the clipboard: one more publish is
  /// live. Proceeds with a paste waiting for it.
  fn clipboard_confirmed(&mut self) {
    // Clamped: after a timeout resync a late confirmation must not count
    // for a future publish.
    self.clip_confirmed = (self.clip_confirmed + 1).min(self.clip_published);
    if self.pending_paste.as_ref().is_some_and(|p| self.clip_confirmed >= p.publish) {
      let p = self.pending_paste.take().expect("checked");
      // The picker hid itself before `Select` (`Hidden{Selected}`); this
      // only matters for a picker that did not. Never hide one a newer
      // `Show` opened meanwhile.
      if p.show_seq == self.show_seq {
        self.picker.hide();
      }
      let autopaste = self.autopaste.clone();
      tokio::spawn(async move {
        let out = autopaste.paste_into(&p.target).await;
        let _ = p.reply.send(Ok(out));
      });
    }
  }

  /// The compositor never confirmed the publish a paste waits for.
  fn paste_not_confirmed(&mut self) {
    let Some(p) = self.pending_paste.take() else { return };
    tracing::warn!(
      timeout_ms = ACK_TIMEOUT.as_millis() as u64,
      "the compositor did not confirm the clipboard publish; not pasting"
    );
    // Resync, so one lost confirmation does not stall every later paste.
    self.clip_confirmed = self.clip_published;
    let _ = p.reply.send(Ok(SelectOutcome::NotPasted(NoPaste::NotConfirmed)));
  }

  /// One event from the desktop integration.
  pub fn handle_desktop(&mut self, ev: CompositorEvent) {
    match ev {
      // The tracker was already updated by the backend.
      CompositorEvent::ActiveWindow { app_id, .. } => {
        tracing::trace!(app_id = app_id.as_deref().unwrap_or(""), "focus changed");
      }
      CompositorEvent::Show { cursor, app_id, window_id } => {
        let allowed =
          self.show_limiter.lock().unwrap_or_else(|p| p.into_inner()).allow(Instant::now());
        if !allowed {
          tracing::info!("show shortcut rate limited");
          return;
        }
        tracing::info!(
          has_cursor = cursor.is_some(),
          has_target = window_id.is_some(),
          "show requested (shortcut)"
        );
        let target = window_id.map(|window_id| PasteTarget { window_id, app_id });
        self.cancel_pick();
        match self.show(ShowOrigin::Hotkey, cursor, target) {
          Ok(()) | Err(PickerError::NotWired) => {}
          Err(e) => tracing::warn!("{e}"),
        }
      }
    }
  }

  /// Record the show context and open the picker.
  fn show(
    &mut self,
    origin: ShowOrigin,
    cursor: Option<(i32, i32)>,
    target: Option<PasteTarget>,
  ) -> Result<(), PickerError> {
    let ctx = ShowContext { origin, cursor, target, at: Instant::now() };
    self.show_ctx = Some(ctx.clone());
    self.show_seq += 1;
    self.picker.show(&ctx)
  }

  /// `PublicReq::Pick`: open the picker in "return" mode; the reply waits
  /// for the user (rate limited by the socket layer like `Show`).
  fn start_pick(&mut self, reply: oneshot::Sender<PublicResp>) {
    self.cancel_pick();
    match self.show(ShowOrigin::Socket, None, None) {
      Ok(()) => {
        tracing::info!("pick requested (socket)");
        self.pick = Some(reply);
      }
      Err(PickerError::NotWired) => {
        let _ = reply.send(PublicResp::NotYetImplemented);
      }
      Err(e) => {
        let _ =
          reply.send(PublicResp::Error { code: ErrorCode::Unavailable, message: e.to_string() });
      }
    }
  }

  /// A pending `spoolctl pick` loses (another Show / Pick, Esc, ...).
  fn cancel_pick(&mut self) {
    if let Some(p) = self.pick.take() {
      tracing::debug!("pending pick cancelled");
      let _ = p.send(PublicResp::Cancelled);
    }
  }

  fn picker_hidden(&mut self, reason: HideReason) {
    if reason != HideReason::Selected {
      self.cancel_pick();
    }
  }

  /// Thumbnail bytes on their own task (a blob read can take a moment).
  fn spawn_thumb(&self, id: ItemId, mime: String, reply: PickerReply<Vec<u8>>) {
    let store = self.store.handle();
    tokio::spawn(async move {
      let r = if !THUMB_MIMES.contains(&mime.as_str()) {
        Err((PickerErrorCode::NotFound, "no thumbnail for this type"))
      } else {
        match store.call(move |s| s.get(id)).await {
          Ok(Ok(Some(item))) => match item.resolve(&mime) {
            Some(d) if d.len() > crate::picker::MAX_THUMB_BYTES => {
              Err((PickerErrorCode::Unavailable, "image too large to preview"))
            }
            Some(d) => Ok(d.to_vec()),
            None => Err((PickerErrorCode::NotFound, "image is gone")),
          },
          Ok(Ok(None)) => Err((PickerErrorCode::NotFound, "item is gone")),
          Ok(Err(e)) => {
            tracing::warn!(%id, "thumbnail: {e}");
            Err((PickerErrorCode::Internal, "could not read the image"))
          }
          Err(e) => {
            tracing::warn!(%id, "thumbnail: {e}");
            Err((PickerErrorCode::Unavailable, "store unavailable"))
          }
        }
      };
      let _ = reply.send(r);
    });
  }

  /// Pin / delete / tag (inline: keeps the picker's edits in order).
  async fn handle_edit(
    &mut self,
    id: ItemId,
    op: EditOp,
  ) -> Result<(), (PickerErrorCode, &'static str)> {
    if let EditOp::Tag { tag, .. } = &op
      && (tag.trim().is_empty()
        || tag.chars().count() > MAX_TAG_LEN
        || tag.chars().any(|c| c.is_control() || c == '/'))
    {
      return Err((PickerErrorCode::BadQuery, "invalid tag"));
    }
    let what = match &op {
      EditOp::Pin(true) => "pinned",
      EditOp::Pin(false) => "unpinned",
      EditOp::Delete => "deleted",
      EditOp::Tag { on: true, .. } => "tagged",
      EditOp::Tag { on: false, .. } => "untagged",
    };
    let r = self
      .store
      .call(move |s| match op {
        EditOp::Pin(on) => s.set_pinned(id, on),
        EditOp::Delete => s.delete(id),
        EditOp::Tag { tag, on } => s.set_tag(id, &tag, on),
      })
      .await;
    match r {
      Ok(Ok(true)) => {
        tracing::info!(%id, "item {what} from the picker");
        Ok(())
      }
      Ok(Ok(false)) => Err((PickerErrorCode::NotFound, "That item no longer exists")),
      Ok(Err(e)) => {
        tracing::error!(%id, "picker edit ({what}) failed: {e}");
        Err((PickerErrorCode::Internal, "Could not change the item"))
      }
      Err(e) => {
        tracing::error!(%id, "picker edit: {e}");
        Err((PickerErrorCode::Unavailable, "store unavailable"))
      }
    }
  }

  /// `Select` while a `spoolctl pick` waits: return the item to it.
  async fn select_return(
    &mut self,
    id: ItemId,
    mode: SelectMode,
    pick: oneshot::Sender<PublicResp>,
    reply: SelectReply,
  ) {
    let now = SystemTime::now();
    let item = match self.store.call(move |s| s.get(id)).await {
      Ok(Ok(Some(item))) => item,
      Ok(Ok(None)) => {
        let _ = pick.send(PublicResp::Error {
          code: ErrorCode::BadRequest,
          message: "the picked item no longer exists".into(),
        });
        let _ = reply.send(Err(SelectError::NotFound));
        return;
      }
      Ok(Err(e)) => {
        let _ = pick.send(internal(format!("store: {e}")));
        let _ = reply.send(Err(SelectError::Internal(format!("store: {e}"))));
        return;
      }
      Err(e) => {
        let _ = pick.send(internal(e.to_string()));
        let _ = reply.send(Err(SelectError::Internal(e.to_string())));
        return;
      }
    };
    let rep = if mode == SelectMode::PastePlain {
      TEXT_MIMES.iter().find_map(|m| item.resolve(m).map(|d| ((*m).to_owned(), d.to_vec())))
    } else {
      item.preferred().map(|(m, d)| (m.to_owned(), d.to_vec()))
    };
    let Some((mime, data)) = rep else {
      let _ = pick.send(PublicResp::Error {
        code: ErrorCode::BadRequest,
        message: "the picked item has nothing to return in this mode".into(),
      });
      let _ = reply.send(Err(SelectError::NoData));
      return;
    };
    match self.store.call(move |s| s.touch(id, now)).await {
      Ok(Ok(_)) => {}
      Ok(Err(e)) => tracing::warn!(%id, "bumping last_used_at failed: {e}"),
      Err(e) => tracing::warn!(%id, "bumping last_used_at: {e}"),
    }
    tracing::info!(%id, hash = %hash_prefix(&item.hash), bytes = data.len(), %mime, "picked (returned)");
    let _ = pick.send(PublicResp::Picked { mime, data });
    let _ = reply.send(Ok(SelectOutcome::Returned));
  }

  /// `Request::Select` (see [`Request::Select`] and [`crate::autopaste`]).
  async fn handle_select(&mut self, id: ItemId, mode: SelectMode, reply: SelectReply) {
    // `spoolctl pick` waiting (and still connected): the item goes there.
    if let Some(pick) = self.pick.take() {
      if !pick.is_closed() {
        return self.select_return(id, mode, pick, reply).await;
      }
      tracing::debug!("pick client went away; selecting normally");
    }
    let now = SystemTime::now();
    let item = match self.store.call(move |s| s.get(id)).await {
      Ok(Ok(Some(item))) => item,
      Ok(Ok(None)) => {
        let _ = reply.send(Err(SelectError::NotFound));
        return;
      }
      Ok(Err(e)) => {
        let _ = reply.send(Err(SelectError::Internal(format!("store: {e}"))));
        return;
      }
      Err(e) => {
        let _ = reply.send(Err(SelectError::Internal(e.to_string())));
        return;
      }
    };
    let mut reps = publish_reps(&item.reps);
    if mode == SelectMode::PastePlain {
      reps.retain(|(m, _)| TEXT_MIMES.iter().any(|t| t.eq_ignore_ascii_case(m)));
    }
    if reps.is_empty() {
      let _ = reply.send(Err(SelectError::NoData));
      return;
    }
    match self.store.call(move |s| s.touch(id, now)).await {
      Ok(Ok(_)) => {}
      Ok(Err(e)) => tracing::warn!(%id, "bumping last_used_at failed: {e}"),
      Err(e) => tracing::warn!(%id, "bumping last_used_at: {e}"),
    }
    if let Err(e) = self.publish(Selection::Clipboard, reps) {
      let _ = reply.send(Err(SelectError::Unavailable(e.to_string())));
      return;
    }
    // The clipboard now holds a history item we publish ourselves:
    // keep-alive has nothing to do for it, and a later clear by another app
    // must not purge it from history (clear detection is for items that
    // were just captured), so the policy sees neither.
    self.pending.remove(&Selection::Clipboard);
    self.state.record_dropped(Selection::Clipboard);
    tracing::info!(%id, hash = %hash_prefix(&item.hash), ?mode, "selected");
    if let Some(p) = self.pending_paste.take() {
      let _ = p.reply.send(Ok(SelectOutcome::NotPasted(NoPaste::Superseded)));
    }
    if mode == SelectMode::Copy {
      let _ = reply.send(Ok(SelectOutcome::Copied));
      return;
    }
    // A paste consumes the target of the last Show.
    let target = self.show_ctx.take().and_then(|c| c.target);
    let why = self.autopaste.check().or(target.is_none().then_some(NoPaste::NoTarget));
    if let Some(why) = why {
      tracing::info!(reason = ?why, "not auto-pasting; the item is on the clipboard");
      self.picker.hide();
      let _ = reply.send(Ok(SelectOutcome::NotPasted(why)));
      return;
    }
    self.pending_paste = Some(PendingPaste {
      publish: self.clip_published,
      target: target.expect("checked"),
      reply,
      deadline: tokio::time::Instant::now() + ACK_TIMEOUT,
      show_seq: self.show_seq,
    });
  }

  async fn copy(&mut self, mime: String, data: Vec<u8>, now: SystemTime) -> PublicResp {
    if let Err(why) = validate_mime(&mime) {
      return PublicResp::Error { code: ErrorCode::BadRequest, message: why.into() };
    }
    let len = data.len();
    let data = Bytes::from(data);
    let reps = match self.policy.manual_item(Selection::Clipboard, &mime, data.clone(), now) {
      Ok(item) => {
        let reps = publish_reps(&item.reps);
        // Paused means "do not record": still put it on the clipboard (the
        // user asked for that), but do not store it.
        if self.state.is_paused(now) {
          tracing::info!(bytes = len, %mime, "copy while paused: published, not stored");
          self.state.record_dropped(Selection::Clipboard);
        } else if self.store_item(item, now).await.is_none() {
          return internal("could not store the item".into());
        }
        reps
      }
      // The user explicitly asked to copy it, so it goes on the clipboard,
      // but a secret is never stored, and `record_dropped` makes sure
      // keep-alive never re-publishes it (or an older item) later.
      Err(DropReason::Secret(kind)) => {
        tracing::info!(?kind, bytes = len, %mime, "copy of a secret: published, not stored");
        self.state.record_dropped(Selection::Clipboard);
        manual_publish_reps(&mime, data)
      }
      Err(reason) => {
        tracing::info!(?reason, bytes = len, %mime, "copy rejected");
        let code = match reason {
          DropReason::TooLarge => ErrorCode::TooLarge,
          _ => ErrorCode::BadRequest,
        };
        return PublicResp::Error { code, message: format!("copy rejected: {reason:?}") };
      }
    };
    match self.publish(Selection::Clipboard, reps) {
      Ok(()) => PublicResp::Ok,
      Err(e) => PublicResp::Error {
        code: ErrorCode::Unavailable,
        message: format!("cannot set the clipboard: {e}"),
      },
    }
  }
}

fn internal(message: String) -> PublicResp {
  PublicResp::Error { code: ErrorCode::Internal, message }
}

fn validate_mime(mime: &str) -> Result<(), &'static str> {
  if mime.is_empty() {
    return Err("mime type is empty");
  }
  if mime.len() > MAX_MIME_LEN {
    return Err("mime type is too long");
  }
  if mime.chars().any(|c| c.is_control() || c.is_whitespace()) {
    return Err("mime type contains whitespace or control characters");
  }
  if spool_wayland::is_marker_mime(mime) {
    return Err("mime type is reserved");
  }
  Ok(())
}

/// `(mime, bytes)` for publishing data that `Policy::manual_item` refused
/// to build an item for (a secret): `mime`, plus the other text variants
/// when `mime` is one, all sharing the same bytes (mirrors `manual_item`'s
/// aliasing so pasting apps see the same offer either way).
fn manual_publish_reps(mime: &str, data: Bytes) -> Vec<(String, Arc<[u8]>)> {
  let bytes: Arc<[u8]> = Arc::from(data.as_ref());
  let mut reps = vec![(mime.to_owned(), bytes.clone())];
  if TEXT_MIMES.iter().any(|t| t.eq_ignore_ascii_case(mime)) {
    reps.extend(
      TEXT_MIMES
        .iter()
        .filter(|t| !t.eq_ignore_ascii_case(mime))
        .map(|t| ((*t).to_owned(), bytes.clone())),
    );
  }
  reps
}

/// `(mime, bytes)` for `set_selection`, resolving aliases to their canonical
/// bytes (shared, not copied).
fn publish_reps(reps: &[Representation]) -> Vec<(String, Arc<[u8]>)> {
  let canon: HashMap<&str, Arc<[u8]>> = reps
    .iter()
    .filter(|r| !r.is_alias())
    .map(|r| (r.mime.as_str(), Arc::<[u8]>::from(r.data.as_ref())))
    .collect();
  reps
    .iter()
    .filter_map(|r| {
      let key = r.alias_of.as_deref().unwrap_or(&r.mime);
      canon.get(key).map(|d| (r.mime.clone(), d.clone()))
    })
    .collect()
}

/// Next item of an optional channel; pending forever without one (or once
/// it closed).
async fn recv_opt<T>(rx: &mut Option<mpsc::Receiver<T>>) -> Option<T> {
  let Some(r) = rx.as_mut() else { return std::future::pending().await };
  match r.recv().await {
    Some(ev) => Some(ev),
    None => {
      *rx = None;
      std::future::pending().await
    }
  }
}

/// Sleeps until `at`; pending forever for `None`.
async fn sleep_until_opt(at: Option<tokio::time::Instant>) {
  match at {
    Some(at) => tokio::time::sleep_until(at).await,
    None => std::future::pending().await,
  }
}

/// Next key event; pending forever once there is no (more) key flow.
async fn recv_key_event(rx: &mut Option<mpsc::Receiver<KeyEvent>>) -> Option<KeyEvent> {
  let Some(r) = rx.as_mut() else { return std::future::pending().await };
  match r.recv().await {
    Some(ev) => Some(ev),
    None => {
      *rx = None;
      std::future::pending().await
    }
  }
}

/// Forward the Wayland thread's crossbeam channel into tokio on a small
/// dedicated std thread (exits when either side closes).
pub fn bridge_wayland_events(
  rx: crossbeam_channel::Receiver<WaylandEvent>,
) -> mpsc::UnboundedReceiver<WaylandEvent> {
  let (tx, out) = mpsc::unbounded_channel();
  std::thread::Builder::new()
    .name("spool-wl-bridge".into())
    .spawn(move || {
      while let Ok(ev) = rx.recv() {
        if tx.send(ev).is_err() {
          break;
        }
      }
    })
    .expect("spawn wayland bridge thread");
  out
}

#[cfg(test)]
#[path = "keyflow_tests.rs"]
mod keyflow_tests;

#[cfg(test)]
#[path = "select_tests.rs"]
mod select_tests;

#[cfg(test)]
#[path = "picker_tests.rs"]
mod picker_tests;

#[cfg(test)]
mod tests {
  use super::*;
  use spool_wayland::CompositorInfo;

  #[derive(Debug)]
  enum Call {
    Fetch { offer: OfferToken, mimes: Vec<String> },
    Set { selection: Selection, reps: Vec<(String, Vec<u8>)> },
  }

  struct FakeWayland {
    calls: mpsc::UnboundedSender<Call>,
  }

  impl WaylandSide for FakeWayland {
    fn fetch(
      &self,
      offer: OfferToken,
      mimes: Vec<String>,
      _per_rep_cap: usize,
      _total_cap: usize,
      _timeout: Duration,
    ) -> Result<(), WaylandError> {
      let _ = self.calls.send(Call::Fetch { offer, mimes });
      Ok(())
    }

    fn set_selection(
      &self,
      selection: Selection,
      reps: Vec<(String, Arc<[u8]>)>,
    ) -> Result<(), WaylandError> {
      let reps = reps.into_iter().map(|(m, d)| (m, d.to_vec())).collect();
      let _ = self.calls.send(Call::Set { selection, reps });
      Ok(())
    }
  }

  #[test]
  fn publish_reps_resolves_aliases() {
    let reps = vec![
      Representation::new("text/plain;charset=utf-8", &b"hi"[..]),
      Representation::alias("UTF8_STRING", "text/plain;charset=utf-8"),
      Representation::alias("dangling", "nope"),
    ];
    let out = publish_reps(&reps);
    assert_eq!(out.len(), 2);
    assert_eq!(out[1].0, "UTF8_STRING");
    assert_eq!(&*out[1].1, b"hi");
  }

  #[test]
  fn mime_validation() {
    assert!(validate_mime("text/plain;charset=utf-8").is_ok());
    assert!(validate_mime("").is_err());
    assert!(validate_mime("text/plain\n").is_err());
    assert!(validate_mime("a b").is_err());
    assert!(validate_mime(&spool_wayland::marker_mime("x")).is_err());
    assert!(validate_mime(&"x".repeat(300)).is_err());
  }

  struct Harness {
    wl: mpsc::UnboundedSender<WaylandEvent>,
    req: mpsc::Sender<Request>,
    calls: mpsc::UnboundedReceiver<Call>,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
  }

  impl Harness {
    fn start() -> Self {
      let (ctx, crx) = mpsc::unbounded_channel();
      let store = Store::open_in_memory().unwrap();
      let orch = Orchestrator::new(Config::default(), store, FakeWayland { calls: ctx }).unwrap();
      let (wl_tx, wl_rx) = mpsc::unbounded_channel();
      let (req_tx, req_rx) = mpsc::channel(REQUEST_QUEUE);
      let (stop_tx, stop_rx) = oneshot::channel::<()>();
      let task = tokio::spawn(orch.run(wl_rx, req_rx, async move {
        let _ = stop_rx.await;
      }));
      Self { wl: wl_tx, req: req_tx, calls: crx, stop: Some(stop_tx), task }
    }

    async fn ask(&self, req: PublicReq) -> PublicResp {
      let (reply, rx) = oneshot::channel();
      let peer = PeerCred { pid: Some(1), uid: 0, gid: 0 };
      self.req.send(Request::Public { req, peer, reply }).await.unwrap();
      rx.await.unwrap()
    }

    async fn next_call(&mut self) -> Call {
      tokio::time::timeout(Duration::from_secs(5), self.calls.recv())
        .await
        .expect("timed out waiting for a wayland call")
        .unwrap()
    }

    /// Round-trip a Status request so all earlier events are processed.
    async fn sync(&self) -> StatusInfo {
      match self.ask(PublicReq::Status).await {
        PublicResp::Status(s) => s,
        other => panic!("unexpected {other:?}"),
      }
    }

    fn no_calls(&mut self) {
      assert!(self.calls.try_recv().is_err(), "unexpected wayland call");
    }

    fn offer(&self, n: u64, mimes: &[&str], ours: bool) {
      self
        .wl
        .send(WaylandEvent::NewSelection {
          selection: Selection::Clipboard,
          mimes: mimes.iter().map(|s| s.to_string()).collect(),
          offer: OfferToken::from_raw(n),
          ours,
        })
        .unwrap();
    }

    fn fetched(&self, n: u64, result: FetchResult) {
      self.wl.send(WaylandEvent::Fetched { offer: OfferToken::from_raw(n), result }).unwrap();
    }
  }

  const UTF8: &str = "text/plain;charset=utf-8";

  #[tokio::test]
  async fn orchestrator_flow() {
    let mut h = Harness::start();
    h.wl
      .send(WaylandEvent::Ready(CompositorInfo {
        display: "wayland-test".into(),
        data_control_version: 1,
        primary_supported: true,
      }))
      .unwrap();
    let st = h.sync().await;
    assert_eq!(st.compositor.as_deref(), Some("wayland-test (ext_data_control_manager_v1 v1)"));
    assert_eq!(st.item_count, 0);
    assert_eq!(st.paused, PauseState::Recording);
    assert_eq!(h.ask(PublicReq::Current).await, PublicResp::Empty);

    // A foreign text offer: fetch one canonical text mime, store it.
    h.offer(1, &[UTF8, "UTF8_STRING", "TEXT"], false);
    let Call::Fetch { offer, mimes } = h.next_call().await else { panic!("expected fetch") };
    assert_eq!(offer.raw(), 1);
    assert_eq!(mimes, vec![UTF8.to_string()]);
    h.fetched(1, Ok(vec![(UTF8.into(), b"hello world".to_vec())]));
    assert_eq!(h.sync().await.item_count, 1);
    assert_eq!(
      h.ask(PublicReq::Current).await,
      PublicResp::Current { mime: UTF8.into(), data: b"hello world".to_vec() }
    );

    // Stale fetch result for an unknown offer is ignored.
    h.fetched(99, Ok(vec![(UTF8.into(), b"stale".to_vec())]));
    assert_eq!(h.sync().await.item_count, 1);

    // Source app exits -> keep-alive re-publishes, aliases included.
    h.wl.send(WaylandEvent::SelectionCleared { selection: Selection::Clipboard }).unwrap();
    let Call::Set { selection, reps } = h.next_call().await else { panic!("expected set") };
    assert_eq!(selection, Selection::Clipboard);
    assert!(reps.iter().any(|(m, d)| m == UTF8 && d == b"hello world"));
    assert!(reps.iter().all(|(_, d)| d == b"hello world"));
    // Our own re-publish comes back marked as ours: ignored.
    h.offer(2, &[UTF8], true);
    h.sync().await;
    h.no_calls();

    // A secret is fetched but dropped, and is then never kept alive.
    h.offer(3, &[UTF8], false);
    let Call::Fetch { .. } = h.next_call().await else { panic!("expected fetch") };
    h.fetched(3, Ok(vec![(UTF8.into(), b"ghp_0123456789abcdefghijklmnopqrstuvwxyz".to_vec())]));
    assert_eq!(h.sync().await.item_count, 1);
    h.wl.send(WaylandEvent::SelectionCleared { selection: Selection::Clipboard }).unwrap();
    h.sync().await;
    h.no_calls();

    // Password-manager hint: hint fetched first, "secret" -> dropped.
    h.offer(4, &[UTF8, PASSWORD_MANAGER_HINT], false);
    let Call::Fetch { mimes, .. } = h.next_call().await else { panic!("expected fetch") };
    assert_eq!(mimes, vec![PASSWORD_MANAGER_HINT.to_string()]);
    h.fetched(4, Ok(vec![(PASSWORD_MANAGER_HINT.into(), b"secret".to_vec())]));
    h.sync().await;
    h.no_calls();

    // Paused: offers are not even fetched.
    assert_eq!(h.ask(PublicReq::Pause { secs: None }).await, PublicResp::Ok);
    assert_eq!(h.sync().await.paused, PauseState::PausedIndefinitely);
    h.offer(5, &[UTF8], false);
    h.sync().await;
    h.no_calls();
    assert_eq!(h.ask(PublicReq::Pause { secs: Some(60) }).await, PublicResp::Ok);
    assert!(matches!(h.sync().await.paused, PauseState::PausedUntil { .. }));
    assert_eq!(h.ask(PublicReq::Resume).await, PublicResp::Ok);
    assert_eq!(h.sync().await.paused, PauseState::Recording);

    // Manual copy: stored and published.
    let resp = h.ask(PublicReq::Copy { mime: UTF8.into(), data: b"from cli".to_vec() }).await;
    assert_eq!(resp, PublicResp::Ok);
    let Call::Set { reps, .. } = h.next_call().await else { panic!("expected set") };
    assert!(reps.iter().any(|(m, d)| m == UTF8 && d == b"from cli"));
    assert_eq!(h.sync().await.item_count, 2);
    assert_eq!(
      h.ask(PublicReq::Current).await,
      PublicResp::Current { mime: UTF8.into(), data: b"from cli".to_vec() }
    );
    // Manual copy of a secret: published (the user asked), never stored,
    // and never kept alive afterwards.
    let secret = b"ghp_0123456789abcdefghijklmnopqrstuvwxyz".to_vec();
    let resp = h.ask(PublicReq::Copy { mime: UTF8.into(), data: secret.clone() }).await;
    assert_eq!(resp, PublicResp::Ok);
    let Call::Set { reps, .. } = h.next_call().await else { panic!("expected set") };
    assert!(reps.iter().any(|(m, d)| m == UTF8 && *d == secret));
    assert!(reps.iter().any(|(m, d)| m == "UTF8_STRING" && *d == secret));
    assert_eq!(h.sync().await.item_count, 2);
    assert_eq!(
      h.ask(PublicReq::Current).await,
      PublicResp::Current { mime: UTF8.into(), data: b"from cli".to_vec() }
    );
    h.wl.send(WaylandEvent::SelectionCleared { selection: Selection::Clipboard }).unwrap();
    h.sync().await;
    h.no_calls();

    let bad = h.ask(PublicReq::Copy { mime: String::new(), data: b"x".to_vec() }).await;
    assert!(matches!(bad, PublicResp::Error { code: ErrorCode::BadRequest, .. }));

    // No picker on this install.
    assert_eq!(h.ask(PublicReq::Show).await, PublicResp::NotYetImplemented);
    assert_eq!(h.ask(PublicReq::Pick).await, PublicResp::NotYetImplemented);

    // Fatal ends the run with an error.
    h.wl.send(WaylandEvent::Fatal("connection lost".into())).unwrap();
    let r = h.task.await.unwrap();
    assert!(r.unwrap_err().to_string().contains("connection lost"));
  }

  /// SIGTERM must not wait for a slow background index open / rebuild
  /// (the E2E "spoold ignored SIGTERM" flake on a loaded machine).
  #[tokio::test]
  async fn shutdown_does_not_wait_for_a_stuck_index_open() {
    let (ctx, _crx) = mpsc::unbounded_channel();
    let store = Store::open_in_memory().unwrap();
    let mut orch = Orchestrator::new(Config::default(), store, FakeWayland { calls: ctx }).unwrap();
    orch.index.opening = Some(tokio::spawn(std::future::pending::<()>()));
    let (_wl_tx, wl_rx) = mpsc::unbounded_channel();
    let (_req_tx, req_rx) = mpsc::channel(REQUEST_QUEUE);
    let start = std::time::Instant::now();
    let r = orch.run(wl_rx, req_rx, async {}).await;
    assert!(r.is_ok());
    let took = start.elapsed();
    assert!(
      took >= INDEX_SHUTDOWN_WAIT && took < INDEX_SHUTDOWN_WAIT + Duration::from_secs(3),
      "{took:?}"
    );
  }

  #[tokio::test]
  async fn orchestrator_shutdown_and_channel_close() {
    let mut h = Harness::start();
    let _ = h.stop.take().unwrap().send(());
    assert!(h.task.await.unwrap().is_ok());

    // Wayland channel closing is an error (daemon must exit non-zero).
    let h = Harness::start();
    drop(h.wl);
    assert!(h.task.await.unwrap().is_err());
  }
}
