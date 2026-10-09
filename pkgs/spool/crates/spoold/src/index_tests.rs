//! Search index lifecycle through the orchestrator, with fake key providers:
//! RAM index while locked, encrypted on-disk index after unlock, catch-up
//! after restart, deletes / retention, rebuilds (corrupt, schema mismatch,
//! missing), large blob-backed text, and no plaintext in the state dir.

use bytes::Bytes;
use spool_core::policy::Policy;
use spool_proto::{ItemPreview, QueryFilters};
use spool_search::{IndexDoc, IndexIdentity, SearchIndex};

use super::*;
use crate::index::{INDEX_DIR_NAME, IndexStatus, SearchError};

const T: Duration = Duration::from_secs(20);

impl H {
  async fn search_with(
    &self,
    q: &str,
    filters: QueryFilters,
    offset: u32,
    limit: u32,
  ) -> Result<Vec<ItemPreview>, SearchError> {
    let (reply, rx) = oneshot::channel();
    let req = Request::Search { q: q.into(), filters, offset, limit, reply };
    self.req.send(req).await.unwrap();
    rx.await.unwrap()
  }

  /// Previews of the search hits for `q`.
  async fn search(&self, q: &str) -> Vec<String> {
    let hits = self.search_with(q, QueryFilters::default(), 0, 50).await.unwrap();
    hits.into_iter().map(|p| p.preview).collect()
  }

  async fn wait_index(&self, want: IndexStatus) {
    let start = tokio::time::Instant::now();
    loop {
      let now = *self.index.borrow();
      if now == want {
        return;
      }
      assert!(start.elapsed() < T, "search index stuck in {now:?}, want {want:?}");
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
  }

  /// Poll until the hits for `q` satisfy `pred` and every hit contains
  /// `q` (so the recent-list fallback, which ignores `q`, never passes; the
  /// index also commits in batches).
  async fn wait_search(&self, q: &str, pred: impl Fn(&[String]) -> bool) -> Vec<String> {
    let start = tokio::time::Instant::now();
    let needle = q.to_lowercase();
    loop {
      let got = self.search(q).await;
      if pred(&got) && got.iter().all(|p| p.to_lowercase().contains(&needle)) {
        return got;
      }
      assert!(start.elapsed() < T, "search never matched; last hits: {got:?}");
      tokio::time::sleep(Duration::from_millis(20)).await;
    }
  }

  async fn wait_found(&self, q: &str, preview: &str) {
    self.wait_search(q, |h| h.iter().any(|p| p == preview)).await;
  }

  async fn wait_gone(&self, q: &str, preview: &str) {
    self.wait_search(q, |h| !h.iter().any(|p| p == preview)).await;
  }
}

async fn data_key(dir: &Path, provider: &Arc<FakeProvider>) -> DataKey {
  let slots = KeySlots::load(dir.join(spool_keys::FILE_NAME)).unwrap();
  let p: &dyn KeyProvider = &**provider;
  let (key, _) = slots.unlock_any(&[p]).await.unwrap();
  DataKey::from_bytes(key)
}

/// Every file under `dir` containing `needle`.
fn files_containing(dir: &Path, needle: &[u8]) -> Vec<std::path::PathBuf> {
  let mut hits = Vec::new();
  let mut stack = vec![dir.to_path_buf()];
  while let Some(d) = stack.pop() {
    for e in std::fs::read_dir(&d).unwrap().flatten() {
      let p = e.path();
      if e.file_type().unwrap().is_dir() {
        stack.push(p);
      } else if std::fs::read(&p).unwrap_or_default().windows(needle.len()).any(|w| w == needle) {
        hits.push(p);
      }
    }
  }
  hits
}

fn assert_no_plaintext(dir: &Path, needles: &[&str]) {
  for n in needles {
    let hits = files_containing(dir, n.as_bytes());
    assert!(hits.is_empty(), "plaintext {n:?} found in {hits:?}");
  }
}

#[tokio::test]
async fn ram_index_while_locked_then_encrypted_index_with_catch_up() {
  let d = tempfile::tempdir().unwrap();
  let provider = FakeProvider::new(true);
  let mut h = H::start(d.path(), &provider);
  h.wait_state(|s| s == "waiting-for-wallet").await;
  h.wait_index(IndexStatus::Memory).await;

  // Locked: captured into the session store and the RAM index.
  h.offer("kubectlmarker get pods").await;
  h.offer("zebramarker stripes").await;
  h.wait_found("kubectlmarker", "kubectlmarker get pods").await;
  h.wait_found("ctlmark", "kubectlmarker get pods").await; // mid-word substring
  assert_eq!(h.search("zebramarker").await, ["zebramarker stripes"]);
  // Empty query: the recent list.
  assert_eq!(h.search("").await, ["zebramarker stripes", "kubectlmarker get pods"]);
  assert!(!d.path().join(INDEX_DIR_NAME).exists(), "no on-disk index without a key");

  // Unlock: same results, now from the encrypted index.
  provider.unlock_now();
  h.wait_state(|s| s == "ready").await;
  h.wait_index(IndexStatus::Disk).await;
  h.wait_found("kubectlmarker", "kubectlmarker get pods").await;
  h.wait_found("zebramarker", "zebramarker stripes").await;
  assert!(d.path().join(INDEX_DIR_NAME).join("meta.json").is_file());
  h.offer("gammamarker after unlock").await;
  h.wait_found("gammamarker", "gammamarker after unlock").await;
  h.stop().await;

  // Changes the index has not seen (made while the daemon was down).
  let key = data_key(d.path(), &provider).await;
  {
    let mut store = Store::open_encrypted(d.path(), &key).unwrap();
    let policy = Policy::new(Config::default(), store.hash_key().unwrap()).unwrap();
    let item = policy
      .manual_item(
        Selection::Clipboard,
        UTF8,
        Bytes::from_static(b"offlinemarker added"),
        std::time::SystemTime::now(),
      )
      .unwrap();
    store.insert(item).unwrap();
    let zebra = store
      .recent(10)
      .unwrap()
      .into_iter()
      .find(|s| s.preview.as_deref() == Some("zebramarker stripes"))
      .unwrap();
    assert!(store.delete(zebra.id).unwrap());
  }

  // Restart: the index opens Ready and catches up (insert + delete).
  let h = H::start(d.path(), &provider);
  h.wait_state(|s| s == "ready").await;
  h.wait_index(IndexStatus::Disk).await;
  h.wait_found("offlinemarker", "offlinemarker added").await;
  h.wait_gone("zebramarker", "zebramarker stripes").await;
  h.wait_found("kubectlmarker", "kubectlmarker get pods").await;
  h.stop().await;

  assert_no_plaintext(
    d.path(),
    &["kubectlmarker", "zebramarker", "gammamarker", "offlinemarker", "stripes"],
  );
}

#[tokio::test]
async fn deletes_and_retention_leave_the_results() {
  let d = tempfile::tempdir().unwrap();
  let provider = FakeProvider::new(true);
  let config = Config { max_items: 2, ..Config::default() };
  let mut h = H::start_with(Some((d.path(), &provider)), config);
  h.wait_state(|s| s == "waiting-for-wallet").await;
  h.offer("retainmarker one").await;
  h.offer("retainmarker two").await;
  h.offer("retainmarker three").await;
  h.wait_search("retainmarker", |hits| hits.len() == 3).await;

  // Unlock: the merge is followed by a retention sweep (max 2 items).
  provider.unlock_now();
  h.wait_state(|s| s == "ready").await;
  h.wait_index(IndexStatus::Disk).await;
  let hits = h.wait_search("retainmarker", |hits| hits.len() == 2).await;
  assert!(!hits.iter().any(|p| p == "retainmarker one"), "{hits:?}");

  // Clearing the clipboard purges the previous copy.
  h.offer("").await;
  h.wait_gone("retainmarker", "retainmarker three").await;
  assert_eq!(h.search("retainmarker").await, ["retainmarker two"]);
  h.stop().await;
}

/// First run, two items, stopped.
async fn seeded(dir: &Path, provider: &Arc<FakeProvider>) {
  let mut h = H::start(dir, provider);
  h.wait_state(|s| s == "ready").await;
  h.wait_index(IndexStatus::Disk).await;
  h.offer("seedmarker alpha").await;
  h.offer("seedmarker beta").await;
  h.wait_search("seedmarker", |hits| hits.len() == 2).await;
  h.stop().await;
}

#[tokio::test]
async fn corrupt_index_is_rebuilt() {
  let d = tempfile::tempdir().unwrap();
  let provider = FakeProvider::new(false);
  seeded(d.path(), &provider).await;

  // Garbage over every index file (segments, meta.json, ...).
  let idx = d.path().join(INDEX_DIR_NAME);
  for e in std::fs::read_dir(&idx).unwrap().flatten() {
    let len = e.metadata().unwrap().len().max(64) as usize;
    std::fs::write(e.path(), vec![0x42u8; len]).unwrap();
  }

  let h = H::start(d.path(), &provider);
  h.wait_state(|s| s == "ready").await;
  h.wait_index(IndexStatus::Disk).await;
  h.wait_search("seedmarker", |hits| hits.len() == 2).await;
  h.stop().await;
  // The rebuilt index opens cleanly next time (no second rebuild needed).
  let key = data_key(d.path(), &provider).await;
  let uuid = Store::open_encrypted(d.path(), &key).unwrap().db_uuid().unwrap();
  match SearchIndex::open(&idx, &key, &IndexIdentity::new(uuid)).unwrap() {
    spool_search::OpenOutcome::Ready(o) => assert_eq!(o.search.num_docs(), 2),
    other => panic!("rebuilt index does not open: {other:?}"),
  }
}

#[tokio::test]
async fn schema_mismatch_is_rebuilt() {
  let d = tempfile::tempdir().unwrap();
  let provider = FakeProvider::new(false);
  seeded(d.path(), &provider).await;

  // An index of an older schema version for the same database, holding a
  // document the store does not have.
  let key = data_key(d.path(), &provider).await;
  let (uuid, head) = {
    let mut s = Store::open_encrypted(d.path(), &key).unwrap();
    (s.db_uuid().unwrap(), s.max_change_seq().unwrap())
  };
  let old = IndexIdentity { schema_version: 1, db_uuid: uuid };
  let stale =
    IndexDoc { id: 999, change_seq: head, text: "stalemarker".into(), ..IndexDoc::default() };
  drop(SearchIndex::rebuild(&d.path().join(INDEX_DIR_NAME), &key, &old, head, [stale]).unwrap());

  let h = H::start(d.path(), &provider);
  h.wait_state(|s| s == "ready").await;
  h.wait_index(IndexStatus::Disk).await;
  h.wait_search("seedmarker", |hits| hits.len() == 2).await;
  assert!(h.search("stalemarker").await.is_empty());
  h.stop().await;
}

#[tokio::test]
async fn large_text_in_a_blob_is_searchable() {
  let d = tempfile::tempdir().unwrap();
  let provider = FakeProvider::new(false);
  let mut h = H::start(d.path(), &provider);
  h.wait_state(|s| s == "ready").await;
  h.wait_index(IndexStatus::Disk).await;

  let mut big = String::from("blobneedlemarker ");
  let mut i = 0;
  while big.len() <= spool_core::store::INLINE_MAX + 64 * 1024 {
    big.push_str(&format!("filler line {i:08}\n"));
    i += 1;
  }
  h.offer(&big).await;
  let blobs: Vec<_> = std::fs::read_dir(d.path().join("blobs")).unwrap().flatten().collect();
  assert_eq!(blobs.len(), 1, "expected the text in a blob file");
  h.wait_search("blobneedlemarker", |h| h.len() == 1).await;
  let hits = h.search_with("blobneedlemarker", QueryFilters::default(), 0, 10).await.unwrap();
  assert_eq!(hits.len(), 1);
  assert_eq!(hits[0].kind, spool_proto::PreviewKind::Text);
  assert_eq!(hits[0].total_size, big.len() as u64);
  h.stop().await;

  // Missing index: rebuilt from the store, reading the blob.
  std::fs::remove_dir_all(d.path().join(INDEX_DIR_NAME)).unwrap();
  let h = H::start(d.path(), &provider);
  h.wait_state(|s| s == "ready").await;
  h.wait_index(IndexStatus::Disk).await;
  h.wait_search("blobneedlemarker", |h| h.len() == 1).await;
  h.stop().await;
  assert_no_plaintext(d.path(), &["blobneedlemarker", "filler line 00000042"]);
}

#[tokio::test]
async fn session_only_search_paging_filters_and_errors() {
  let mut h = H::start_with(None, Config::default());
  assert_eq!(h.status().await.key_state, "session-only");
  h.wait_index(IndexStatus::Memory).await;
  for i in 0..5 {
    h.offer(&format!("pagemarker item {i}")).await;
  }
  h.wait_search("pagemarker", |hits| hits.len() == 5).await;

  // Paging over the recent list and over hits.
  let page = |v: Vec<ItemPreview>| v.into_iter().map(|p| p.preview).collect::<Vec<_>>();
  let recent = page(h.search_with("", QueryFilters::default(), 1, 2).await.unwrap());
  assert_eq!(recent, ["pagemarker item 3", "pagemarker item 2"]);
  let all = page(h.search_with("pagemarker", QueryFilters::default(), 0, 50).await.unwrap());
  let tail = page(h.search_with("pagemarker", QueryFilters::default(), 3, 50).await.unwrap());
  assert_eq!(tail, all[3..]);
  assert!(h.search_with("pagemarker", QueryFilters::default(), 0, 0).await.unwrap().is_empty());

  // Filters the index cannot apply are applied to the candidates.
  let pinned = QueryFilters { pinned_only: true, ..QueryFilters::default() };
  assert!(h.search_with("pagemarker", pinned.clone(), 0, 50).await.unwrap().is_empty());
  assert!(h.search_with("", pinned, 0, 50).await.unwrap().is_empty());
  let images =
    QueryFilters { kinds: vec![spool_proto::PreviewKind::Image], ..QueryFilters::default() };
  assert!(h.search_with("pagemarker", images, 0, 50).await.unwrap().is_empty());
  let tagged = QueryFilters { tags: vec!["work".into()], ..QueryFilters::default() };
  assert!(h.search_with("", tagged, 0, 50).await.unwrap().is_empty());

  // Rejected queries come back as picker-visible errors (no query echo).
  let long = "x".repeat(300);
  let e = h.search_with(&long, QueryFilters::default(), 0, 10).await.unwrap_err();
  assert!(matches!(e, SearchError::BadQuery(_)), "{e:?}");
  assert!(!e.to_string().contains("xxxx"));
  let many = (0..20).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ");
  let e = h.search_with(&many, QueryFilters::default(), 0, 10).await.unwrap_err();
  assert!(matches!(e, SearchError::BadQuery(_)), "{e:?}");
  let e = h.search_with("app:[a TO b]", QueryFilters::default(), 0, 10).await.unwrap_err();
  assert!(matches!(e, SearchError::BadQuery(_)), "{e:?}");
  h.stop().await;
}
