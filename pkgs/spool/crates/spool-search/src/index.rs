//! Opening, rebuilding, writing and searching the index.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustix::fs::{FlockOperation, RenameFlags};
use serde::{Deserialize, Serialize};
use spool_crypto::{DataKey, Label};
use tantivy::collector::TopDocs;
use tantivy::directory::RamDirectory;
use tantivy::schema::Facet;
use tantivy::{
  DateTime, DocId, Index, IndexReader, IndexSettings, IndexWriter, ReloadPolicy, Score,
  SegmentReader, TantivyDocument, Term,
};

use crate::directory::EncryptedDirectory;
use crate::error::{Error, Result};
use crate::query::{self, Filters, Hit, RankParams};
use crate::schema::{self, Fields, SCHEMA_VERSION};

/// Minimum delay between the first uncommitted change and the commit that
/// [`Indexer::commit_if_due`] makes.
pub const COMMIT_BATCH: Duration = Duration::from_millis(300);
/// Writer heap budget: Tantivy's minimum (15 MB) with one indexing thread.
const WRITER_BUDGET: usize = 15_000_000;
/// At most this many bytes of an item's text are indexed (cut on a char
/// boundary); the rest is not searchable.
pub const MAX_INDEXED_TEXT: usize = 1 << 20;

/// What the daemon feeds the index for one history item.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct IndexDoc {
  /// Store item id (must be >= 0).
  pub id: i64,
  /// The store's `change_seq` for this version of the item.
  pub change_seq: i64,
  /// Searchable text (not stored in the index).
  pub text: String,
  /// Offered MIME types.
  pub mime: Vec<String>,
  /// Source application, if known.
  pub app: Option<String>,
  /// User tags.
  pub tags: Vec<String>,
  /// Recency timestamp in ms since the Unix epoch (`created_at`, or
  /// `last_used_at` if the daemon prefers "recently used").
  pub created_at: i64,
  pub pinned: bool,
}

/// What an on-disk index must belong to. Stored in every commit payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexIdentity {
  pub schema_version: u32,
  /// The store database's UUID; an index built from another DB is rebuilt.
  pub db_uuid: String,
}

impl IndexIdentity {
  /// Identity with this crate's [`SCHEMA_VERSION`].
  pub fn new(db_uuid: impl Into<String>) -> Self {
    IndexIdentity { schema_version: SCHEMA_VERSION, db_uuid: db_uuid.into() }
  }
}

/// JSON commit payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct CommitPayload {
  last_change_seq: i64,
  schema_version: u32,
  db_uuid: String,
}

/// Why [`SearchIndex::open`] wants a rebuild.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebuildReason {
  /// No index directory / no `meta.json`.
  Missing,
  /// Authentication failed: wrong key, or files tampered with / swapped.
  DecryptFailure,
  /// Built with another [`SCHEMA_VERSION`] or schema.
  SchemaMismatch,
  /// Built from another database.
  DbMismatch,
  /// Decrypts but Tantivy cannot use it (or the payload is missing/bad).
  Corrupt,
}

/// Result of [`SearchIndex::open`].
#[derive(Debug)]
pub enum OpenOutcome {
  Ready(Box<Opened>),
  NeedsRebuild(RebuildReason),
}

/// An open index: the shareable search handle and the single writer.
#[derive(Debug)]
pub struct Opened {
  pub search: SearchIndex,
  pub indexer: Indexer,
  /// Highest `change_seq` covered by the last commit; replay store changes
  /// after it.
  pub last_change_seq: i64,
}

/// Searchable handle. Cheap to clone; `Send + Sync`.
#[derive(Clone)]
pub struct SearchIndex {
  inner: Arc<SearchInner>,
}

struct SearchInner {
  index: Index,
  reader: IndexReader,
  fields: Fields,
  rank: RwLock<RankParams>,
}

impl fmt::Debug for SearchIndex {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("SearchIndex").field("num_docs", &self.num_docs()).finish_non_exhaustive()
  }
}

/// `flock` on `<parent>/<name>.lock`, beside the index directory so it
/// survives the rebuild swap.
struct InstanceLock {
  _file: File,
}

impl InstanceLock {
  fn acquire(dir: &Path) -> Result<Self> {
    let path = sibling(dir, ".lock");
    if let Some(parent) = path.parent() {
      fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
      .read(true)
      .write(true)
      .create(true)
      .truncate(false)
      .mode(0o600)
      .open(&path)?;
    match rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive) {
      Ok(()) => Ok(InstanceLock { _file: file }),
      Err(rustix::io::Errno::WOULDBLOCK) => Err(Error::Locked),
      Err(e) => Err(Error::Io(e.into())),
    }
  }
}

fn sibling(dir: &Path, suffix: &str) -> PathBuf {
  let mut name = dir.file_name().unwrap_or_default().to_os_string();
  name.push(suffix);
  dir.with_file_name(name)
}

fn now_ms() -> i64 {
  SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

impl SearchIndex {
  /// Open the encrypted index at `dir` (e.g. `$STATE/spool/index`) with the
  /// `Label::Index` sub-key of `key`, checking it belongs to `expected`.
  ///
  /// Never panics on bad data: anything unusable is `NeedsRebuild`. `Err` is
  /// reserved for environment problems: [`Error::Locked`] (another writer)
  /// and I/O errors creating the lock file.
  pub fn open(dir: &Path, key: &DataKey, expected: &IndexIdentity) -> Result<OpenOutcome> {
    let lock = InstanceLock::acquire(dir)?;
    Ok(open_locked(dir, key, expected, lock))
  }

  /// Build a fresh index from `docs` in `<dir>.rebuild`, commit it with
  /// `last_change_seq`, and atomically swap it into `dir`
  /// (`renameat2(RENAME_EXCHANGE)`); the old index is deleted afterwards.
  ///
  /// Any previous [`Indexer`] for `dir` must be dropped first
  /// ([`Error::Locked`] otherwise).
  pub fn rebuild(
    dir: &Path,
    key: &DataKey,
    identity: &IndexIdentity,
    last_change_seq: i64,
    docs: impl IntoIterator<Item = IndexDoc>,
  ) -> Result<Opened> {
    let lock = InstanceLock::acquire(dir)?;
    let staging = sibling(dir, ".rebuild");
    if staging.exists() {
      fs::remove_dir_all(&staging)?;
    }
    {
      let edir = EncryptedDirectory::open(&staging, key.derive(Label::Index))?;
      let (schema, fields) = schema::build();
      let index = Index::create(edir, schema, IndexSettings::default())?;
      schema::register(&index);
      let mut writer: IndexWriter<TantivyDocument> =
        index.writer_with_num_threads(1, WRITER_BUDGET)?;
      for d in docs {
        writer.add_document(to_tantivy(&fields, &d)?)?;
      }
      let mut prepared = writer.prepare_commit()?;
      prepared.set_payload(&payload_json(last_change_seq, identity));
      prepared.commit()?;
      writer.wait_merging_threads()?;
    }
    let parent = dir.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if dir.exists() {
      rustix::fs::renameat_with(
        rustix::fs::CWD,
        &staging,
        rustix::fs::CWD,
        dir,
        RenameFlags::EXCHANGE,
      )
      .map_err(io::Error::from)?;
      File::open(parent)?.sync_all()?;
      // `staging` now holds the old index.
      fs::remove_dir_all(&staging)?;
    } else {
      fs::rename(&staging, dir)?;
    }
    File::open(parent)?.sync_all()?;
    match open_locked(dir, key, identity, lock) {
      OpenOutcome::Ready(o) => Ok(*o),
      OpenOutcome::NeedsRebuild(r) => {
        Err(Error::Io(io::Error::other(format!("freshly rebuilt index does not open: {r:?}"))))
      }
    }
  }

  /// Unencrypted in-memory index (pre-unlock session history). Never
  /// touches disk; gone when dropped.
  pub fn in_memory() -> Result<Opened> {
    let (schema, fields) = schema::build();
    let index = Index::create(RamDirectory::create(), schema, IndexSettings::default())?;
    schema::register(&index);
    let identity = IndexIdentity::new("");
    let mut opened = finish_open(index, fields, identity, 0, None)?;
    // An initial commit so the payload exists, as on disk.
    opened.indexer.commit()?;
    Ok(opened)
  }

  /// Search with the wall clock as "now". See [`search_at`](Self::search_at).
  pub fn search(
    &self,
    q: &str,
    filters: &Filters,
    limit: usize,
    offset: usize,
  ) -> Result<Vec<Hit>> {
    self.search_at(q, filters, limit, offset, now_ms())
  }

  /// Ranked hits for `q` with `filters`, skipping `offset` and returning at
  /// most `limit`. `now_ms` is the reference time for recency. An empty
  /// query matches everything (recency/pin order).
  pub fn search_at(
    &self,
    q: &str,
    filters: &Filters,
    limit: usize,
    offset: usize,
    now_ms: i64,
  ) -> Result<Vec<Hit>> {
    let f = self.inner.fields;
    let scoring = query::build(&self.inner.index, &f, q)?;
    let query = query::with_filters(&f, scoring, filters);
    if limit == 0 {
      return Ok(vec![]);
    }
    let rank = *self.inner.rank.read().unwrap_or_else(|e| e.into_inner());
    let collector =
      TopDocs::with_limit(limit).and_offset(offset).tweak_score(move |seg: &SegmentReader| {
        let ff = seg.fast_fields();
        let created = ff.date("created").ok();
        let pinned = ff.bool("pinned").ok();
        let id = ff.u64("id").ok();
        move |doc: DocId, bm25: Score| {
          let created_ms = created
            .as_ref()
            .and_then(|c| c.first(doc))
            .map_or(now_ms, DateTime::into_timestamp_millis);
          let is_pinned = pinned.as_ref().and_then(|c| c.first(doc)).unwrap_or(false);
          let id = id.as_ref().and_then(|c| c.first(doc)).unwrap_or(0);
          (rank.score(bm25, now_ms - created_ms, is_pinned), id)
        }
      });
    let searcher = self.inner.reader.searcher();
    let top = searcher.search(&query, &collector)?;
    Ok(
      top
        .into_iter()
        .map(|((score, id), _)| Hit { id: i64::try_from(id).unwrap_or(i64::MAX), score })
        .collect(),
    )
  }

  /// Replace the ranking parameters.
  pub fn set_rank_params(&self, p: RankParams) {
    *self.inner.rank.write().unwrap_or_else(|e| e.into_inner()) = p;
  }

  pub fn rank_params(&self) -> RankParams {
    *self.inner.rank.read().unwrap_or_else(|e| e.into_inner())
  }

  /// Live (committed, not deleted) documents.
  pub fn num_docs(&self) -> u64 {
    self.inner.reader.searcher().num_docs()
  }
}

fn payload_json(last_change_seq: i64, id: &IndexIdentity) -> String {
  serde_json::to_string(&CommitPayload {
    last_change_seq,
    schema_version: id.schema_version,
    db_uuid: id.db_uuid.clone(),
  })
  .expect("payload serializes")
}

fn open_locked(
  dir: &Path,
  key: &DataKey,
  expected: &IndexIdentity,
  lock: InstanceLock,
) -> OpenOutcome {
  if !dir.join("meta.json").exists() {
    return OpenOutcome::NeedsRebuild(RebuildReason::Missing);
  }
  let edir = match EncryptedDirectory::open(dir, key.derive(Label::Index)) {
    Ok(d) => d,
    Err(_) => return OpenOutcome::NeedsRebuild(RebuildReason::Corrupt),
  };
  let classify = |what: &str, e: &dyn fmt::Display| {
    let reason =
      if edir.auth_failures() > 0 { RebuildReason::DecryptFailure } else { RebuildReason::Corrupt };
    // Tantivy errors carry file names and structure, never indexed text.
    tracing::warn!(error = %e, ?reason, "search index unusable ({what})");
    OpenOutcome::NeedsRebuild(reason)
  };
  let index = match Index::open(edir.clone()) {
    Ok(i) => i,
    Err(e) => return classify("open", &e),
  };
  schema::register(&index);
  let (schema, fields) = schema::build();
  let metas = match index.load_metas() {
    Ok(m) => m,
    Err(e) => return classify("metas", &e),
  };
  let payload: CommitPayload = match metas.payload.as_deref().map(serde_json::from_str) {
    Some(Ok(p)) => p,
    _ => return OpenOutcome::NeedsRebuild(RebuildReason::Corrupt),
  };
  if payload.schema_version != expected.schema_version || index.schema() != schema {
    return OpenOutcome::NeedsRebuild(RebuildReason::SchemaMismatch);
  }
  if payload.db_uuid != expected.db_uuid {
    return OpenOutcome::NeedsRebuild(RebuildReason::DbMismatch);
  }
  // Authenticate every live file now (v1 decrypts whole files anyway), so
  // tampering is reported here instead of failing a later search.
  let segments = match index.searchable_segment_metas() {
    Ok(s) => s,
    Err(e) => return classify("segments", &e),
  };
  for seg in &segments {
    for file in seg.list_files() {
      if let Err(e) = tantivy::Directory::get_file_handle(&edir, &file) {
        // Delete bitsets etc. that a segment does not have are fine.
        if matches!(e, tantivy::directory::error::OpenReadError::FileDoesNotExist(_)) {
          continue;
        }
        return classify("segment file", &e);
      }
    }
  }
  match finish_open(index, fields, expected.clone(), payload.last_change_seq, Some(lock)) {
    Ok(o) => OpenOutcome::Ready(Box::new(o)),
    Err(e) => classify("reader", &e),
  }
}

fn finish_open(
  index: Index,
  fields: Fields,
  identity: IndexIdentity,
  last_change_seq: i64,
  lock: Option<InstanceLock>,
) -> Result<Opened> {
  let reader = index.reader_builder().reload_policy(ReloadPolicy::Manual).try_into()?;
  let writer: IndexWriter<TantivyDocument> = index.writer_with_num_threads(1, WRITER_BUDGET)?;
  let search = SearchIndex {
    inner: Arc::new(SearchInner {
      index,
      reader: reader.clone(),
      fields,
      rank: RwLock::new(RankParams::default()),
    }),
  };
  let indexer = Indexer {
    writer,
    reader,
    fields,
    identity,
    committed_seq: last_change_seq,
    pending_seq: last_change_seq,
    dirty_since: None,
    _lock: lock,
  };
  Ok(Opened { search, indexer, last_change_seq })
}

fn to_tantivy(f: &Fields, d: &IndexDoc) -> Result<TantivyDocument> {
  let id = u64::try_from(d.id).map_err(|_| Error::NegativeId)?;
  let mut doc = TantivyDocument::new();
  doc.add_u64(f.id, id);
  let text = truncate(&d.text, MAX_INDEXED_TEXT);
  doc.add_text(f.text, text);
  doc.add_text(f.text_tri, text);
  for m in &d.mime {
    doc.add_text(f.mime, m);
  }
  if let Some(app) = &d.app {
    doc.add_text(f.app, app);
  }
  for t in &d.tags {
    doc.add_facet(f.tag, Facet::from_path([t.as_str()]));
  }
  doc.add_date(f.created, DateTime::from_timestamp_millis(d.created_at));
  doc.add_bool(f.pinned, d.pinned);
  Ok(doc)
}

fn truncate(s: &str, max: usize) -> &str {
  if s.len() <= max {
    return s;
  }
  let mut end = max;
  while !s.is_char_boundary(end) {
    end -= 1;
  }
  &s[..end]
}

/// The single index writer (holds the directory lock).
pub struct Indexer {
  writer: IndexWriter<TantivyDocument>,
  reader: IndexReader,
  fields: Fields,
  identity: IndexIdentity,
  committed_seq: i64,
  pending_seq: i64,
  dirty_since: Option<Instant>,
  _lock: Option<InstanceLock>,
}

impl fmt::Debug for Indexer {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Indexer")
      .field("committed_seq", &self.committed_seq)
      .field("pending_seq", &self.pending_seq)
      .field("dirty", &self.dirty_since.is_some())
      .finish_non_exhaustive()
  }
}

impl Indexer {
  fn touch(&mut self, change_seq: i64) {
    self.pending_seq = self.pending_seq.max(change_seq);
    self.dirty_since.get_or_insert_with(Instant::now);
  }

  fn id_term(&self, id: u64) -> Term {
    Term::from_field_u64(self.fields.id, id)
  }

  /// Insert or replace the document for `doc.id` (visible after a commit).
  pub fn upsert(&mut self, doc: &IndexDoc) -> Result<()> {
    let tdoc = to_tantivy(&self.fields, doc)?;
    let id = u64::try_from(doc.id).map_err(|_| Error::NegativeId)?;
    self.writer.delete_term(self.id_term(id));
    self.writer.add_document(tdoc)?;
    self.touch(doc.change_seq);
    Ok(())
  }

  /// Remove item `id`; `change_seq` is the store change that deleted it (so
  /// the committed payload covers it).
  pub fn delete(&mut self, id: i64, change_seq: i64) -> Result<()> {
    let id = u64::try_from(id).map_err(|_| Error::NegativeId)?;
    self.writer.delete_term(self.id_term(id));
    self.touch(change_seq);
    Ok(())
  }

  /// Record that the store advanced to `change_seq` without anything to
  /// index (e.g. a change to a non-text item).
  pub fn note_change_seq(&mut self, change_seq: i64) {
    if change_seq > self.pending_seq {
      self.touch(change_seq);
    }
  }

  /// Uncommitted changes exist.
  pub fn has_pending(&self) -> bool {
    self.dirty_since.is_some()
  }

  /// When [`commit_if_due`](Self::commit_if_due) will next commit, if
  /// anything is pending (for scheduling a timer).
  pub fn commit_deadline(&self) -> Option<Instant> {
    self.dirty_since.map(|t| t + COMMIT_BATCH)
  }

  /// Commit if changes have been pending for at least [`COMMIT_BATCH`].
  /// Returns whether a commit happened.
  pub fn commit_if_due(&mut self) -> Result<bool> {
    match self.dirty_since {
      Some(t) if t.elapsed() >= COMMIT_BATCH => {
        self.commit()?;
        Ok(true)
      }
      _ => Ok(false),
    }
  }

  /// Commit now (payload: `last_change_seq`, schema version, DB uuid) and
  /// make the changes visible to searches. Also commits when nothing is
  /// pending (cheap; refreshes the payload).
  pub fn commit(&mut self) -> Result<()> {
    let mut prepared = self.writer.prepare_commit()?;
    prepared.set_payload(&payload_json(self.pending_seq, &self.identity));
    prepared.commit()?;
    self.committed_seq = self.pending_seq;
    self.dirty_since = None;
    self.reader.reload()?;
    Ok(())
  }

  /// Highest `change_seq` covered by the last commit.
  pub fn last_change_seq(&self) -> i64 {
    self.committed_seq
  }
}
