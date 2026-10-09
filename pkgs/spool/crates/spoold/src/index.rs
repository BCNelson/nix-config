//! Full-text search index wiring (spool-search) for the orchestrator.
//!
//! # Lifecycle (follows the store)
//!
//! - Session / locked / plaintext / session-only: an unencrypted in-memory
//!   index ([`SearchIndex::in_memory`]), back-filled from the store at start.
//! - After unlock (merge + store swap): the RAM index is dropped (its ids are
//!   the session store's) and `$STATE/index` is opened with the data key.
//!   `Ready { last_change_seq }` -> catch up from the store feed;
//!   `NeedsRebuild(reason)` (or an index *ahead* of the database) -> rebuild
//!   from the whole feed on a blocking thread, reporting [`IndexProgress`].
//!   Until an index is attached, search falls back to the recent list.
//!
//! # Feed
//!
//! The store thread ([`crate::orchestrator::StoreActor`]) owns a
//! [`FeedSink`]: after **every** store job it compares the store's
//! `max_change_seq` with the sink's cursor and sends the changed items and
//! tombstones to the [`IndexActor`] ([`pump`]). Inserts, bumps, deletes,
//! retention sweeps and merges are therefore indexed without any call-site
//! bookkeeping, in store order, and capture never waits on Tantivy.
//! Back-fills ([`backfill`]) run as small store jobs that send pages
//! straight from the store thread and attach the sink in the job that
//! reaches the head, so no change is missed or applied out of order.
//!
//! # Writer
//!
//! [`IndexActor`] owns the single [`Indexer`] on its own thread and commits
//! when [`Indexer::commit_deadline`] passes (batched, >= `COMMIT_BATCH`)
//! and at shutdown.
//!
//! Never logs indexed text or query strings.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self as std_mpsc, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::Context;
use spool_core::item::{ItemFlags, ItemId, Selection, TEXT_MIMES};
use spool_core::store::index_feed::{IndexItem, TaggedSummary};
use spool_core::store::to_millis;
use spool_crypto::DataKey;
use spool_proto::{ItemPreview, PreviewKind, QueryFilters};
use spool_search::{
  Filters, IndexDoc, IndexIdentity, Indexer, MAX_INDEXED_TEXT, OpenOutcome, Opened, SearchIndex,
};
use tokio::sync::mpsc;

use crate::orchestrator::{StoreCtx, StoreHandle};

/// Directory of the on-disk index inside the state directory.
pub const INDEX_DIR_NAME: &str = "index";
/// Items per back-fill / rebuild page (one store job each).
const PAGE: usize = 256;
/// Largest page a search returns.
pub const MAX_LIMIT: u32 = 500;
/// Candidates fetched per round while filtering search results.
const CANDIDATES: usize = 200;
/// Stop filtering after this many candidates (bounds a filter that matches
/// almost nothing).
const MAX_SCANNED: usize = 20_000;
/// Back-fill attempts before giving up on a failing store.
const BACKFILL_ATTEMPTS: u32 = 3;

/// Rebuild progress (for the picker's `PickerEvt::IndexProgress`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexProgress {
  pub done: u64,
  pub total: u64,
}

/// Where search currently stands (published on a `watch` channel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexStatus {
  /// No index attached yet (start, or switching after unlock): search
  /// lists recent history.
  Pending,
  /// Rebuilding the on-disk index; search lists recent history.
  Rebuilding(IndexProgress),
  /// The in-memory index is attached (it may still be catching up).
  Memory,
  /// The encrypted on-disk index is attached (it may still be catching up).
  Disk,
  /// Opening / rebuilding failed; search lists recent history.
  Unavailable,
}

/// A batch of store changes for the index, in store order: tombstones,
/// then upserts, then "covered up to `upto`".
#[derive(Debug, Default)]
pub struct Batch {
  pub deletes: Vec<(i64, i64)>,
  pub docs: Vec<IndexDoc>,
  pub upto: i64,
}

pub enum IndexMsg {
  Apply(Batch),
  /// Commit what is pending and exit.
  Stop,
}

/// The store thread's link to the current index writer.
pub struct FeedSink {
  tx: std_mpsc::Sender<IndexMsg>,
  /// Highest `change_seq` already sent.
  cursor: i64,
}

fn to_doc(i: IndexItem) -> IndexDoc {
  IndexDoc {
    id: i.id.0,
    change_seq: i.change_seq,
    text: i.text,
    mime: i.mimes,
    app: i.source_app,
    tags: i.tags,
    // Ranking recency: "recently used", like the recent list.
    created_at: i.last_used_at,
    pinned: i.pinned,
  }
}

/// Changes with `cursor < change_seq <= upto`, at most `limit` items (the
/// returned `upto` is lowered to the last item's seq when the page is full).
fn read_batch(
  store: &spool_core::store::Store,
  cursor: i64,
  target: i64,
  limit: usize,
) -> spool_core::Result<Batch> {
  let items = store.items_for_index(cursor, target, limit, MAX_INDEXED_TEXT)?;
  let upto = match items.last() {
    Some(last) if items.len() == limit => last.change_seq,
    _ => target,
  };
  let deletes =
    store.tombstones_after(cursor, upto)?.into_iter().map(|t| (t.id.0, t.change_seq)).collect();
  Ok(Batch { deletes, docs: items.into_iter().map(to_doc).collect(), upto })
}

/// Run after every store job: send what changed since the sink's cursor.
pub fn pump(ctx: &mut StoreCtx) {
  let StoreCtx { store, feed, .. } = ctx;
  let Some(sink) = feed.as_mut() else { return };
  let batch = store.max_change_seq().and_then(|target| {
    if target <= sink.cursor {
      Ok(None)
    } else {
      read_batch(store, sink.cursor, target, usize::MAX).map(Some)
    }
  });
  match batch {
    Ok(None) => {}
    Ok(Some(b)) => {
      let upto = b.upto;
      if sink.tx.send(IndexMsg::Apply(b)).is_err() {
        tracing::debug!("index writer gone; feed detached");
        *feed = None;
      } else {
        sink.cursor = upto;
      }
    }
    // Retried after the next store job.
    Err(e) => tracing::warn!("index feed: reading changes failed: {e}"),
  }
}

/// The single index writer on its own thread.
pub struct IndexActor {
  tx: std_mpsc::Sender<IndexMsg>,
  thread: Option<std::thread::JoinHandle<()>>,
  search: SearchIndex,
}

impl IndexActor {
  pub fn spawn(opened: Opened, what: &'static str) -> anyhow::Result<Self> {
    let Opened { search, indexer, .. } = opened;
    let (tx, rx) = std_mpsc::channel();
    let thread = std::thread::Builder::new()
      .name("spool-index".into())
      .spawn(move || writer_loop(indexer, rx, what))
      .context("spawning the index thread")?;
    Ok(Self { tx, thread: Some(thread), search })
  }

  pub fn search(&self) -> SearchIndex {
    self.search.clone()
  }

  /// The writer's channel (for [`backfill`]).
  pub fn sender(&self) -> std_mpsc::Sender<IndexMsg> {
    self.tx.clone()
  }

  /// Commit pending changes, stop the thread and wait for it.
  pub async fn shutdown(mut self) {
    let _ = self.tx.send(IndexMsg::Stop);
    if let Some(t) = self.thread.take() {
      let _ = tokio::task::spawn_blocking(move || t.join()).await;
    }
  }
}

impl Drop for IndexActor {
  fn drop(&mut self) {
    // The thread commits and exits; not joined here (`shutdown` does).
    let _ = self.tx.send(IndexMsg::Stop);
  }
}

fn writer_loop(mut ix: Indexer, rx: std_mpsc::Receiver<IndexMsg>, what: &'static str) {
  loop {
    let msg = match ix.commit_deadline() {
      Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
      None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
    };
    match msg {
      Ok(IndexMsg::Apply(b)) => apply(&mut ix, b),
      Err(RecvTimeoutError::Timeout) => {
        if let Err(e) = ix.commit_if_due() {
          tracing::warn!(index = what, "search index commit failed: {e}");
        }
      }
      Ok(IndexMsg::Stop) | Err(RecvTimeoutError::Disconnected) => {
        if ix.has_pending() {
          commit(&mut ix, what);
        }
        break;
      }
    }
  }
  tracing::debug!(index = what, "index thread exited");
}

fn commit(ix: &mut Indexer, what: &str) {
  if let Err(e) = ix.commit() {
    tracing::warn!(index = what, "search index commit failed: {e}");
  }
}

fn apply(ix: &mut Indexer, b: Batch) {
  for (id, seq) in b.deletes {
    if let Err(e) = ix.delete(id, seq) {
      tracing::warn!(id, "search index delete failed: {e}");
    }
  }
  for d in &b.docs {
    if let Err(e) = ix.upsert(d) {
      tracing::warn!(id = d.id, "search index update failed: {e}");
    }
  }
  ix.note_change_seq(b.upto);
}

/// Reports from background index work to the orchestrator.
pub enum IndexEvent {
  /// The on-disk index is open (or was rebuilt) and covers `cursor`.
  DiskReady {
    generation: u64,
    opened: Box<Opened>,
    cursor: i64,
  },
  /// Opening / rebuilding the on-disk index failed; search stays on the
  /// recent-list fallback.
  DiskFailed {
    generation: u64,
    error: String,
  },
  Progress {
    generation: u64,
    progress: IndexProgress,
  },
}

/// Feed `actor` from `cursor` to the head of the store in pages, then
/// attach it as the live feed (in the same store job, so nothing is missed).
/// Stops quietly when `generation` is no longer current.
pub async fn backfill(
  store: StoreHandle,
  actor_tx: std_mpsc::Sender<IndexMsg>,
  generation: u64,
  mut cursor: i64,
) {
  let mut sent = 0usize;
  let mut failures = 0;
  loop {
    let tx = actor_tx.clone();
    let step = store
      .call_ctx(move |ctx: &mut StoreCtx| -> spool_core::Result<Option<(i64, usize, bool)>> {
        if ctx.generation != generation {
          return Ok(None);
        }
        let target = ctx.store.max_change_seq()?;
        let b = read_batch(&ctx.store, cursor, target, PAGE)?;
        let (upto, n) = (b.upto, b.docs.len());
        if tx.send(IndexMsg::Apply(b)).is_err() {
          return Ok(None);
        }
        let done = upto >= target;
        if done {
          ctx.feed = Some(FeedSink { tx, cursor: upto });
        }
        Ok(Some((upto, n, done)))
      })
      .await;
    match step {
      Ok(Ok(Some((upto, n, done)))) => {
        sent += n;
        cursor = upto;
        if done {
          tracing::debug!(items = sent, "search index caught up");
          return;
        }
      }
      Ok(Ok(None)) => return,
      Ok(Err(e)) => {
        failures += 1;
        tracing::warn!("search index catch-up: {e}");
        if failures >= BACKFILL_ATTEMPTS {
          tracing::error!("search index catch-up gave up; search may miss items until restart");
          return;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
      }
      Err(_) => return, // store shut down
    }
  }
}

/// Open `<state>/index` with `key`, or rebuild it from the store; runs on a
/// blocking thread. Sends [`IndexEvent::Progress`] while rebuilding and
/// returns the opened index and the `change_seq` it covers.
pub fn open_or_rebuild(
  store: &StoreHandle,
  dir: &Path,
  key: &DataKey,
  identity: &IndexIdentity,
  generation: u64,
  events: &mpsc::UnboundedSender<IndexEvent>,
) -> anyhow::Result<(Opened, i64)> {
  let reason = match SearchIndex::open(dir, key, identity).context("opening the search index")? {
    OpenOutcome::Ready(opened) => {
      let head = store.call_blocking(|s| s.max_change_seq())??;
      if opened.last_change_seq <= head {
        tracing::info!(
          docs = opened.search.num_docs(),
          behind = head - opened.last_change_seq,
          "search index opened"
        );
        let cursor = opened.last_change_seq;
        return Ok((*opened, cursor));
      }
      // E.g. the database lost its last commits (power loss) while the
      // index kept them: change_seqs would be reused for other content.
      drop(opened);
      "index is ahead of the database".to_string()
    }
    OpenOutcome::NeedsRebuild(r) => format!("{r:?}"),
  };
  tracing::warn!(%reason, "search index needs a rebuild; rebuilding in the background");
  rebuild(store, dir, key, identity, generation, events)
}

/// Rebuild from a snapshot of the store (`change_seq <= T`, paged); returns
/// the index committed at `T`. Changes after `T` are caught up by the
/// caller's back-fill.
///
/// The rebuild itself is committed with `last_change_seq = 0` and only
/// re-committed at `T` once every page was read: an interrupted rebuild
/// (shutdown, store error, crash) therefore opens later as "covers 0" and is
/// caught up completely instead of silently missing items.
fn rebuild(
  store: &StoreHandle,
  dir: &Path,
  key: &DataKey,
  identity: &IndexIdentity,
  generation: u64,
  events: &mpsc::UnboundedSender<IndexEvent>,
) -> anyhow::Result<(Opened, i64)> {
  let (target, total) =
    store.call_blocking(|s| Ok::<_, spool_core::Error>((s.max_change_seq()?, s.count()?)))??;
  let progress = |done: u64| {
    let _ =
      events.send(IndexEvent::Progress { generation, progress: IndexProgress { done, total } });
  };
  progress(0);
  let started = Instant::now();
  let failed: Cell<Option<String>> = Cell::new(None);
  let mut cursor = 0i64;
  let mut done = 0u64;
  let mut page: std::vec::IntoIter<IndexItem> = Vec::new().into_iter();
  let mut finished = false;
  let docs = std::iter::from_fn(|| {
    loop {
      if let Some(item) = page.next() {
        return Some(to_doc(item));
      }
      if finished {
        return None;
      }
      let from = cursor;
      let r = store.call_ctx_blocking(move |ctx: &mut StoreCtx| {
        if ctx.generation != generation {
          return Ok(None);
        }
        ctx.store.items_for_index(from, target, PAGE, MAX_INDEXED_TEXT).map(Some)
      });
      match r {
        Ok(Ok(Some(items))) => {
          if items.len() < PAGE {
            finished = true;
          }
          if let Some(last) = items.last() {
            cursor = last.change_seq;
          }
          done += items.len() as u64;
          progress(done.min(total));
          page = items.into_iter();
        }
        Ok(Ok(None)) => {
          failed.set(Some("superseded".into()));
          return None;
        }
        Ok(Err(e)) => {
          failed.set(Some(format!("reading the store: {e}")));
          return None;
        }
        Err(e) => {
          failed.set(Some(e.to_string()));
          return None;
        }
      }
    }
  });
  let mut opened =
    SearchIndex::rebuild(dir, key, identity, 0, docs).context("rebuilding the search index")?;
  if let Some(why) = failed.take() {
    anyhow::bail!("search index rebuild interrupted: {why}");
  }
  opened.indexer.note_change_seq(target);
  opened.indexer.commit().context("committing the rebuilt search index")?;
  tracing::info!(docs = opened.search.num_docs(), elapsed = ?started.elapsed(), "search index rebuilt");
  Ok((opened, target))
}

/// Errors a search reports to the picker. Messages never contain the query.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SearchError {
  /// The query is rejected (too long, too many terms, bad syntax).
  #[error("{0}")]
  BadQuery(String),
  #[error("search failed: {0}")]
  Internal(String),
}

fn map_search_error(e: spool_search::Error) -> SearchError {
  use spool_search::Error as E;
  match e {
    E::QueryTooLong(_) | E::TooManyTerms(_) | E::UnsupportedSyntax(_) | E::InvalidQuery => {
      SearchError::BadQuery(e.to_string())
    }
    other => SearchError::Internal(other.to_string()),
  }
}

fn internal(e: impl std::fmt::Display) -> SearchError {
  SearchError::Internal(e.to_string())
}

/// The picker's list kind for a set of offered mimes.
pub fn preview_kind(mimes: &[String]) -> PreviewKind {
  let has = |f: &dyn Fn(&str) -> bool| mimes.iter().any(|m| f(m));
  if has(&|m| m == "text/uri-list" || m == "x-special/gnome-copied-files") {
    PreviewKind::Files
  } else if has(&|m| m.starts_with("image/")) {
    PreviewKind::Image
  } else if has(&|m| TEXT_MIMES.contains(&m)) {
    PreviewKind::Text
  } else if has(&|m| m == "text/html") {
    PreviewKind::Html
  } else {
    PreviewKind::Other
  }
}

pub(crate) fn to_preview(t: TaggedSummary) -> ItemPreview {
  let s = t.summary;
  ItemPreview {
    id: s.id.0,
    kind: preview_kind(&s.mimes),
    preview: s.preview.unwrap_or_default(),
    mimes: s.mimes,
    source_app: s.source_app,
    created_unix_ms: to_millis(s.created_at).max(0) as u64,
    last_used_unix_ms: to_millis(s.last_used_at).max(0) as u64,
    pinned: s.flags.contains(ItemFlags::PINNED),
    tags: t.tags,
    total_size: s.total_size,
  }
}

fn keep(f: &QueryFilters, t: &TaggedSummary) -> bool {
  let s = &t.summary;
  (f.include_primary || s.selection != Selection::Primary)
    && (!f.pinned_only || s.flags.contains(ItemFlags::PINNED))
    && f.source_app.as_ref().is_none_or(|app| s.source_app.as_ref() == Some(app))
    && (f.kinds.is_empty() || f.kinds.contains(&preview_kind(&s.mimes)))
    && f.tags.iter().all(|tag| t.tags.contains(tag))
}

/// Collects filtered results past `offset`, up to `limit`.
struct Pager<'a> {
  filters: &'a QueryFilters,
  skip: usize,
  limit: usize,
  out: Vec<ItemPreview>,
}

impl Pager<'_> {
  /// Feed candidates in order; `true` once the page is full.
  fn push(&mut self, page: Vec<TaggedSummary>) -> bool {
    for t in page {
      if !keep(self.filters, &t) {
        continue;
      }
      if self.skip > 0 {
        self.skip -= 1;
        continue;
      }
      self.out.push(to_preview(t));
      if self.out.len() >= self.limit {
        return true;
      }
    }
    false
  }
}

/// The picker's search: an empty query (or no index yet) lists history in
/// the store's recent order; otherwise ranked index hits, resolved against
/// the store in hit order (ids no longer stored are skipped). `filters`
/// that the index cannot apply are applied to the candidates.
pub async fn search(
  index: Option<SearchIndex>,
  store: StoreHandle,
  q: String,
  filters: QueryFilters,
  offset: u32,
  limit: u32,
) -> Result<Vec<ItemPreview>, SearchError> {
  let limit = limit.min(MAX_LIMIT) as usize;
  if limit == 0 {
    return Ok(vec![]);
  }
  let mut pager = Pager { filters: &filters, skip: offset as usize, limit, out: Vec::new() };
  let mut scanned = 0usize;
  match index.filter(|_| !q.trim().is_empty()) {
    None => loop {
      let from = scanned;
      let page = store
        .call(move |s| s.recent_page(from, CANDIDATES))
        .await
        .map_err(internal)?
        .map_err(internal)?;
      let n = page.len();
      scanned += n;
      if pager.push(page) || n < CANDIDATES || scanned >= MAX_SCANNED {
        break;
      }
    },
    Some(index) => {
      let sf = Filters {
        app: filters.source_app.clone(),
        pinned_only: filters.pinned_only,
        ..Filters::default()
      };
      let q = Arc::new(q);
      loop {
        let (idx, q, sf, from) = (index.clone(), q.clone(), sf.clone(), scanned);
        let hits = tokio::task::spawn_blocking(move || idx.search(&q, &sf, CANDIDATES, from))
          .await
          .map_err(internal)?
          .map_err(map_search_error)?;
        let n = hits.len();
        scanned += n;
        let ids: Vec<ItemId> = hits.iter().map(|h| ItemId(h.id)).collect();
        let page =
          store.call(move |s| s.summaries(&ids)).await.map_err(internal)?.map_err(internal)?;
        if pager.push(page) || n < CANDIDATES || scanned >= MAX_SCANNED {
          break;
        }
      }
    }
  }
  Ok(pager.out)
}

/// `<state>/index`.
pub fn index_dir(state: &Path) -> PathBuf {
  state.join(INDEX_DIR_NAME)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn kinds() {
    let k = |m: &[&str]| preview_kind(&m.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(k(&["text/plain;charset=utf-8", "TEXT"]), PreviewKind::Text);
    assert_eq!(k(&["text/html", "text/plain"]), PreviewKind::Text);
    assert_eq!(k(&["text/html"]), PreviewKind::Html);
    assert_eq!(k(&["image/png", "text/html"]), PreviewKind::Image);
    assert_eq!(k(&["text/uri-list", "text/plain"]), PreviewKind::Files);
    assert_eq!(k(&["application/x-foo"]), PreviewKind::Other);
  }

  #[test]
  fn search_errors_are_picker_visible_and_never_echo_the_query() {
    let e = map_search_error(spool_search::Error::QueryTooLong(300));
    assert!(matches!(e, SearchError::BadQuery(_)));
    assert!(matches!(
      map_search_error(spool_search::Error::InvalidQuery),
      SearchError::BadQuery(_)
    ));
    assert!(matches!(
      map_search_error(spool_search::Error::TooManyTerms(20)),
      SearchError::BadQuery(_)
    ));
    assert!(matches!(map_search_error(spool_search::Error::Locked), SearchError::Internal(_)));
  }
}
