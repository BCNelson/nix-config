//! On-disk index: confidentiality, tamper detection, open/catch-up/rebuild.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use spool_crypto::DataKey;
use spool_search::{
  COMMIT_BATCH, Error, Filters, IndexIdentity, OpenOutcome, RebuildReason, SCHEMA_VERSION,
  SearchIndex,
};

fn ids(idx: &SearchIndex, q: &str) -> Vec<i64> {
  idx.search_at(q, &Filters::default(), 50, 0, NOW).unwrap().into_iter().map(|h| h.id).collect()
}

fn files(dir: &Path) -> Vec<PathBuf> {
  let mut v: Vec<PathBuf> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
  v.sort();
  v
}

/// Segment files are `<32 hex>.<ext>`.
fn segment_files(dir: &Path) -> Vec<PathBuf> {
  files(dir)
    .into_iter()
    .filter(|p| {
      let n = p.file_name().unwrap().to_string_lossy();
      n.len() > 33 && n.as_bytes()[32] == b'.' && n[..32].chars().all(|c| c.is_ascii_hexdigit())
    })
    .collect()
}

fn setup(n: i64) -> (tempfile::TempDir, PathBuf, DataKey) {
  let t = tempdir();
  let dir = t.path().join("index");
  let key = DataKey::generate();
  let docs = (1..=n).map(|i| doc(i, &format!("kubectl get pods x{i} zebramarkerqx")));
  let o = SearchIndex::rebuild(&dir, &key, &identity(), n, docs).unwrap();
  drop(o);
  (t, dir, key)
}

#[test]
fn no_plaintext_on_disk() {
  let t = tempdir();
  let dir = t.path().join("index");
  let key = DataKey::generate();
  let mut d = doc(1, "the secret is zebramarkerqx and Squeamishossifrage");
  d.app = Some("org.markerapp.Editor".into());
  d.tags = vec!["markertagzz".into()];
  d.mime = vec!["text/x-markermime".into()];
  let mut o = SearchIndex::rebuild(&dir, &key, &identity(), 1, [d]).unwrap();
  // Plus a few incremental commits and a delete, so several segments,
  // delete files and merges exist.
  for i in 2..40 {
    o.indexer.upsert(&doc(i, &format!("zebramarkerqx item {i} squeamishossifrage"))).unwrap();
    if i % 5 == 0 {
      o.indexer.commit().unwrap();
    }
  }
  o.indexer.delete(3, 50).unwrap();
  o.indexer.commit().unwrap();
  assert_eq!(ids(&o.search, "zebramarkerqx").len(), 38);
  assert_eq!(ids(&o.search, "squeamish").len(), 38);
  drop(o);

  let markers: [&[u8]; 6] = [
    b"zebramarkerqx",
    b"squeamishossifrage",
    b"markerapp",
    b"markertagzz",
    b"markermime",
    b"secret",
  ];
  let all = files(&dir);
  assert!(all.len() > 3, "{all:?}");
  for f in &all {
    let bytes = fs::read(f).unwrap();
    for m in markers {
      assert!(!bytes.windows(m.len()).any(|w| w.eq_ignore_ascii_case(m)), "{m:?} found in {f:?}");
    }
    // Tantivy's own metadata must not be readable either.
    assert!(!bytes.windows(8).any(|w| w == b"segments"), "meta in clear: {f:?}");
  }
  // Every file is 0600 or an empty lock file.
  use std::os::unix::fs::PermissionsExt;
  for f in &all {
    let m = fs::metadata(f).unwrap();
    assert_eq!(m.permissions().mode() & 0o077, 0, "{f:?}");
  }
}

#[test]
fn every_tampered_segment_file_needs_rebuild() {
  let (_t, dir, key) = setup(30);
  let segs = segment_files(&dir);
  assert!(!segs.is_empty());
  for f in &segs {
    let orig = fs::read(f).unwrap();
    let mut bad = orig.clone();
    let i = bad.len() / 2;
    bad[i] ^= 0x40;
    fs::write(f, &bad).unwrap();
    let r = needs(SearchIndex::open(&dir, &key, &identity()).unwrap());
    assert_eq!(r, RebuildReason::DecryptFailure, "{f:?}");
    // Truncation is caught as well (length check or tag).
    fs::write(f, &orig[..orig.len() - 1]).unwrap();
    let r = needs(SearchIndex::open(&dir, &key, &identity()).unwrap());
    assert!(matches!(r, RebuildReason::DecryptFailure | RebuildReason::Corrupt), "{f:?} {r:?}");
    fs::write(f, &orig).unwrap();
  }
  // Restored: opens fine again.
  let o = ready(SearchIndex::open(&dir, &key, &identity()).unwrap());
  assert_eq!(o.search.num_docs(), 30);
}

#[test]
fn swapped_segment_files_need_rebuild() {
  let (_t, dir, key) = setup(10);
  let segs = segment_files(&dir);
  assert!(segs.len() >= 2);
  let (a, b) = (&segs[0], &segs[1]);
  let tmp = dir.join("swap.tmp");
  fs::rename(a, &tmp).unwrap();
  fs::rename(b, a).unwrap();
  fs::rename(&tmp, b).unwrap();
  assert_eq!(
    needs(SearchIndex::open(&dir, &key, &identity()).unwrap()),
    RebuildReason::DecryptFailure
  );
}

#[test]
fn tampered_meta_and_wrong_key_need_rebuild() {
  let (_t, dir, key) = setup(5);
  let wrong = DataKey::generate();
  assert_eq!(
    needs(SearchIndex::open(&dir, &wrong, &identity()).unwrap()),
    RebuildReason::DecryptFailure
  );
  let meta = dir.join("meta.json");
  let orig = fs::read(&meta).unwrap();
  let mut bad = orig.clone();
  *bad.last_mut().unwrap() ^= 1;
  fs::write(&meta, &bad).unwrap();
  assert_eq!(
    needs(SearchIndex::open(&dir, &key, &identity()).unwrap()),
    RebuildReason::DecryptFailure
  );
  fs::write(&meta, b"").unwrap();
  assert!(matches!(
    needs(SearchIndex::open(&dir, &key, &identity()).unwrap()),
    RebuildReason::DecryptFailure | RebuildReason::Corrupt
  ));
  fs::write(&meta, &orig).unwrap();
  ready(SearchIndex::open(&dir, &key, &identity()).unwrap());
}

#[test]
fn identity_checks() {
  let (t, dir, key) = setup(3);
  assert_eq!(
    needs(SearchIndex::open(&dir, &key, &IndexIdentity::new("other-db")).unwrap()),
    RebuildReason::DbMismatch
  );
  let mut v2 = identity();
  v2.schema_version += 1;
  assert_eq!(needs(SearchIndex::open(&dir, &key, &v2).unwrap()), RebuildReason::SchemaMismatch);
  assert_eq!(
    needs(SearchIndex::open(&t.path().join("nope"), &key, &identity()).unwrap()),
    RebuildReason::Missing
  );
}

#[test]
fn schema_bump_rebuilds_with_substrings() {
  let (_t, dir, key) = setup(3);
  // An index written by an older schema version is not reused...
  let old = IndexIdentity { schema_version: SCHEMA_VERSION - 1, db_uuid: identity().db_uuid };
  let o = SearchIndex::rebuild(&dir, &key, &old, 3, [doc(1, "kubectl get pods")]).unwrap();
  drop(o);
  assert_eq!(
    needs(SearchIndex::open(&dir, &key, &identity()).unwrap()),
    RebuildReason::SchemaMismatch
  );
  // ...and the rebuild with the current version supports mid-word matches.
  let o = SearchIndex::rebuild(&dir, &key, &identity(), 3, [doc(1, "kubectl get pods")]).unwrap();
  assert_eq!(ids(&o.search, "ctl"), [1]);
  drop(o);
  let o = ready(SearchIndex::open(&dir, &key, &identity()).unwrap());
  assert_eq!(ids(&o.search, "bectl"), [1]);
}

#[test]
fn single_writer_lock() {
  let (_t, dir, key) = setup(3);
  let o = ready(SearchIndex::open(&dir, &key, &identity()).unwrap());
  assert!(matches!(SearchIndex::open(&dir, &key, &identity()), Err(Error::Locked)));
  assert!(matches!(SearchIndex::rebuild(&dir, &key, &identity(), 0, []), Err(Error::Locked)));
  drop(o);
  ready(SearchIndex::open(&dir, &key, &identity()).unwrap());
}

#[test]
fn catch_up_via_payload() {
  let (_t, dir, key) = setup(5);
  let mut o = ready(SearchIndex::open(&dir, &key, &identity()).unwrap());
  assert_eq!(o.last_change_seq, 5);
  let mut d = doc(6, "helmfile sync");
  d.change_seq = 9;
  o.indexer.upsert(&d).unwrap();
  assert!(o.indexer.has_pending());
  // Batching: not due immediately.
  assert!(!o.indexer.commit_if_due().unwrap());
  assert!(ids(&o.search, "helmfile").is_empty());
  std::thread::sleep(COMMIT_BATCH + Duration::from_millis(20));
  assert!(o.indexer.commit_if_due().unwrap());
  assert!(!o.indexer.has_pending());
  assert_eq!(ids(&o.search, "helmfile"), [6]);
  o.indexer.delete(2, 12).unwrap();
  // Uncommitted work is lost on drop; the payload still says 9.
  drop(o);
  let mut o = ready(SearchIndex::open(&dir, &key, &identity()).unwrap());
  assert_eq!(o.last_change_seq, 9);
  assert_eq!(ids(&o.search, "x2").len(), 1);
  o.indexer.delete(2, 12).unwrap();
  o.indexer.note_change_seq(15);
  o.indexer.commit().unwrap();
  assert_eq!(o.indexer.last_change_seq(), 15);
  drop(o);
  let o = ready(SearchIndex::open(&dir, &key, &identity()).unwrap());
  assert_eq!(o.last_change_seq, 15);
  assert!(ids(&o.search, "x2").is_empty());
  assert_eq!(o.search.num_docs(), 5);
}

#[test]
fn rebuild_swaps_atomically() {
  let (t, dir, key) = setup(5);
  let o = SearchIndex::rebuild(&dir, &key, &identity(), 100, [doc(77, "terraform plan")]).unwrap();
  assert_eq!(o.last_change_seq, 100);
  assert_eq!(ids(&o.search, "terraform"), [77]);
  assert!(ids(&o.search, "kubectl").is_empty());
  drop(o);
  let names: Vec<String> = fs::read_dir(t.path())
    .unwrap()
    .map(|e| e.unwrap().file_name().to_string_lossy().into())
    .collect();
  assert!(!names.iter().any(|n| n.contains("rebuild")), "{names:?}");
  let o = ready(SearchIndex::open(&dir, &key, &identity()).unwrap());
  assert_eq!(o.search.num_docs(), 1);
  // Rebuilding after corruption works too (the usual recovery path).
  drop(o);
  fs::write(dir.join("meta.json"), b"garbage").unwrap();
  assert!(matches!(
    SearchIndex::open(&dir, &key, &identity()).unwrap(),
    OpenOutcome::NeedsRebuild(_)
  ));
  let o = SearchIndex::rebuild(&dir, &key, &identity(), 1, [doc(1, "again")]).unwrap();
  assert_eq!(ids(&o.search, "again"), [1]);
}
