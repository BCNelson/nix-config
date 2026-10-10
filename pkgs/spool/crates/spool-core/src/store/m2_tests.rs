//! M2 store tests: encryption, blob files, session merge, rekey.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use spool_crypto::{DataKey, Label};

use super::rekey::RekeyStep;
use super::*;
use crate::item::{ItemFlags, Representation, dedupe_hash};

fn t(secs: u64) -> SystemTime {
  SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + secs)
}

fn hash_key_of(k: &DataKey) -> [u8; 32] {
  *k.derive(Label::Hash).expose()
}

fn text_item(hk: &[u8; 32], sel: Selection, text: &str, at: SystemTime) -> NewItem {
  let canon = Representation::new("text/plain;charset=utf-8", text.as_bytes().to_vec());
  NewItem {
    selection: sel,
    source_app: Some("org.kde.kate".into()),
    created_at: at,
    flags: ItemFlags::empty(),
    hash: dedupe_hash(hk, &canon),
    preview: Some(text.chars().take(200).collect()),
    reps: vec![canon, Representation::alias("UTF8_STRING", "text/plain;charset=utf-8")],
  }
}

fn image_item(hk: &[u8; 32], len: usize, fill: u8, at: SystemTime) -> NewItem {
  let data: Vec<u8> = (0..len).map(|i| fill ^ (i % 251) as u8).collect();
  let canon = Representation::new("image/png", data);
  NewItem {
    selection: Selection::Clipboard,
    source_app: None,
    created_at: at,
    flags: ItemFlags::empty(),
    hash: dedupe_hash(hk, &canon),
    preview: Some(format!("[image/png {len} bytes]")),
    reps: vec![canon],
  }
}

fn blob_names(dir: &Path) -> Vec<String> {
  let mut v: Vec<String> = std::fs::read_dir(dir.join(BLOB_DIR_NAME))
    .map(|rd| rd.map(|e| e.unwrap().file_name().into_string().unwrap()).collect())
    .unwrap_or_default();
  v.sort();
  v
}

fn referenced_blobs(s: &Store) -> Vec<String> {
  let mut st =
    s.conn().prepare("SELECT blob_file FROM reps WHERE blob_file IS NOT NULL ORDER BY 1").unwrap();
  st.query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap()
}

fn assert_err<T: std::fmt::Debug>(r: Result<T>, pred: impl Fn(&Error) -> bool, what: &str) {
  match r {
    Err(e) if pred(&e) => {}
    other => panic!("{what}: unexpected {other:?}"),
  }
}

fn flip(path: &Path, offset: u64) {
  use std::io::{Read, Seek, SeekFrom, Write};
  let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path).unwrap();
  f.seek(SeekFrom::Start(offset)).unwrap();
  let mut b = [0u8; 1];
  f.read_exact(&mut b).unwrap();
  f.seek(SeekFrom::Start(offset)).unwrap();
  f.write_all(&[b[0] ^ 0x5a]).unwrap();
}

#[test]
fn encrypted_roundtrip_with_reopen() {
  let d = tempfile::tempdir().unwrap();
  let k = DataKey::generate();
  let hk = hash_key_of(&k);
  let (id, seq) = {
    let mut s = Store::open_encrypted(d.path(), &k).unwrap();
    assert_eq!(s.kind(), StoreKind::Encrypted);
    assert_eq!(s.hash_key().unwrap(), hk, "hash key = Label::Hash sub-key");
    let id = s.insert(text_item(&hk, Selection::Clipboard, "persist me", t(0))).unwrap().id();
    let mode: String = s.conn().query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    assert_eq!(mode, "wal");
    let sync: i64 = s.conn().query_row("PRAGMA synchronous", [], |r| r.get(0)).unwrap();
    assert_eq!(sync, 1, "synchronous=NORMAL");
    let temp: i64 = s.conn().query_row("PRAGMA temp_store", [], |r| r.get(0)).unwrap();
    assert_eq!(temp, 2, "temp_store=MEMORY");
    let ms: String = s.conn().query_row("PRAGMA cipher_memory_security", [], |r| r.get(0)).unwrap();
    assert_eq!(ms, "1");
    (id, s.get(id).unwrap().unwrap().change_seq)
  };
  let mut s = Store::open_encrypted(d.path(), &k).unwrap();
  let it = s.get(id).unwrap().unwrap();
  assert_eq!(it.resolve("UTF8_STRING").unwrap().as_ref(), b"persist me");
  assert_eq!(s.hash_key().unwrap(), hk);
  // Dedupe still works across reopen (hash key is stable).
  assert_eq!(
    s.insert(text_item(&hk, Selection::Clipboard, "persist me", t(1))).unwrap(),
    InsertOutcome::Bumped(id)
  );
  assert!(s.get(id).unwrap().unwrap().change_seq > seq);
  let kcv = blobs::read_kcv(&d.path().join(KCV_FILE_NAME)).unwrap();
  assert_eq!(kcv, vec![blobs::kcv_of(&k.derive(Label::Db))]);
}

fn open_and_read_all(dir: &Path, k: &DataKey) -> Result<()> {
  let s = Store::open_encrypted(dir, k)?;
  for r in s.recent(100_000)? {
    s.get(r.id)?;
  }
  Ok(())
}

#[test]
fn wrong_key_vs_corrupt() {
  let d = tempfile::tempdir().unwrap();
  let k = DataKey::generate();
  let hk = hash_key_of(&k);
  {
    let mut s = Store::open_encrypted(d.path(), &k).unwrap();
    for i in 0..200 {
      s.insert(text_item(
        &hk,
        Selection::Clipboard,
        &format!("item {i} {}", "x".repeat(500)),
        t(i),
      ))
      .unwrap();
    }
  } // close checkpoints the WAL into history.db
  let db = d.path().join(DB_FILE_NAME);
  assert!(!d.path().join(format!("{DB_FILE_NAME}-wal")).exists());
  assert!(std::fs::metadata(&db).unwrap().len() > 8 * 4096);

  assert_err(
    Store::open_encrypted(d.path(), &DataKey::generate()),
    |e| matches!(e, Error::WrongKey),
    "wrong key",
  );
  assert!(matches!(Store::open(&db, None), Err(Error::WrongKey)));
  Store::open_encrypted(d.path(), &k).unwrap();

  // Damage a later page: never reported as a wrong key.
  let pristine = std::fs::read(&db).unwrap();
  flip(&db, 5 * 4096 + 1000);
  assert_err(open_and_read_all(d.path(), &k), |e| matches!(e, Error::Corrupt(_)), "later page");

  // Damage page 1: the key check value says the key is right.
  std::fs::write(&db, &pristine).unwrap();
  flip(&db, 2000);
  assert_err(
    Store::open_encrypted(d.path(), &k),
    |e| matches!(e, Error::Corrupt(_)),
    "damaged page 1",
  );
  assert_err(
    Store::open_encrypted(d.path(), &DataKey::generate()),
    |e| matches!(e, Error::WrongKey),
    "damaged page 1, other key",
  );

  // Impossible length.
  std::fs::write(&db, &pristine[..pristine.len() - 100]).unwrap();
  assert_err(open_and_read_all(d.path(), &k), |e| matches!(e, Error::Corrupt(_)), "truncated");

  // Plaintext database where an encrypted one is expected.
  let p = tempfile::tempdir().unwrap();
  drop(Store::open(&p.path().join(DB_FILE_NAME), None).unwrap());
  assert_err(
    Store::open_encrypted(p.path(), &k),
    |e| matches!(e, Error::NotEncrypted),
    "plaintext db",
  );
}

#[test]
fn blob_files_only_above_inline_max() {
  let d = tempfile::tempdir().unwrap();
  let k = DataKey::generate();
  let hk = hash_key_of(&k);
  let mut s = Store::open_encrypted(d.path(), &k).unwrap();
  let small = s.insert(image_item(&hk, INLINE_MAX, 1, t(0))).unwrap().id();
  assert!(blob_names(d.path()).is_empty(), "exactly INLINE_MAX stays inline");
  let big_item = image_item(&hk, INLINE_MAX + 1, 2, t(1));
  let big = s.insert(big_item.clone()).unwrap().id();
  let names = blob_names(d.path());
  assert_eq!(names.len(), 1);
  assert!(blobs::is_blob_name(&names[0]));
  assert_eq!(referenced_blobs(&s), names);
  let (inline, size): (Option<Vec<u8>>, i64) = s
    .conn()
    .query_row("SELECT inline, size FROM reps WHERE item_id = ?1", params![big.0], |r| {
      Ok((r.get(0)?, r.get(1)?))
    })
    .unwrap();
  assert_eq!((inline, size), (None, (INLINE_MAX + 1) as i64));
  assert_eq!(s.get(big).unwrap().unwrap().reps, big_item.reps);
  assert_eq!(s.latest(Selection::Clipboard).unwrap().unwrap().reps, big_item.reps);
  assert_eq!(s.get(small).unwrap().unwrap().reps[0].data.len(), INLINE_MAX);
  // A dedupe bump writes no file.
  assert_eq!(s.insert(big_item.clone()).unwrap(), InsertOutcome::Bumped(big));
  assert_eq!(blob_names(d.path()).len(), 1);

  // Delete removes the file.
  assert!(s.delete(big).unwrap());
  assert!(blob_names(d.path()).is_empty());

  // Retention removes files.
  let old = s.insert(image_item(&hk, INLINE_MAX + 10, 3, t(2))).unwrap().id();
  let keep = s.insert(image_item(&hk, INLINE_MAX + 20, 4, t(30 * 86_400))).unwrap().id();
  assert_eq!(blob_names(d.path()).len(), 2);
  let limits = RetentionLimits { max_age: Duration::from_secs(14 * 86_400), max_items: 100 };
  assert_eq!(s.retention_sweep(t(31 * 86_400), limits).unwrap(), 2, "old + small");
  assert_eq!(s.get(old).unwrap(), None);
  assert_eq!(referenced_blobs(&s), blob_names(d.path()));
  assert_eq!(blob_names(d.path()).len(), 1);
  assert!(s.get(keep).unwrap().is_some());
  drop(s);

  // Orphan GC at open: unreferenced blob names and seal_file temp files go;
  // unknown names stay.
  let bdir = d.path().join(BLOB_DIR_NAME);
  let orphan = format!("{}.bin", "ab".repeat(16));
  std::fs::write(bdir.join(&orphan), b"junk").unwrap();
  let tmp = format!(".{orphan}.tmp-0123456789abcdef");
  std::fs::write(bdir.join(&tmp), b"junk").unwrap();
  std::fs::write(bdir.join("README"), b"keep").unwrap();
  let mut s = Store::open_encrypted(d.path(), &k).unwrap();
  let mut names = blob_names(d.path());
  names.retain(|n| n != "README");
  assert_eq!(names, referenced_blobs(&s));
  assert!(bdir.join("README").exists());
  assert_eq!(s.gc_orphan_blobs().unwrap(), 0);
  assert!(s.get(keep).unwrap().is_some());
}

#[test]
fn plain_and_session_keep_large_reps_inline() {
  let d = tempfile::tempdir().unwrap();
  let mut p = Store::open(&d.path().join("plain.db"), None).unwrap();
  assert_eq!(p.kind(), StoreKind::Plain);
  let hk = p.hash_key().unwrap();
  let id = p.insert(image_item(&hk, INLINE_MAX + 1, 5, t(0))).unwrap().id();
  assert!(referenced_blobs(&p).is_empty());
  assert_eq!(p.get(id).unwrap().unwrap().reps[0].data.len(), INLINE_MAX + 1);
  assert!(!d.path().join(BLOB_DIR_NAME).exists());
  assert_eq!(p.gc_orphan_blobs().unwrap(), 0);
  assert_err(p.rekey(&DataKey::generate()), |e| matches!(e, Error::Unsupported(_)), "plain rekey");

  let sk = DataKey::generate();
  let mut s = Store::open_session(&sk).unwrap();
  assert_eq!(s.kind(), StoreKind::Session);
  assert_eq!(s.hash_key().unwrap(), hash_key_of(&sk));
  let id = s.insert(image_item(&hash_key_of(&sk), INLINE_MAX + 1, 5, t(0))).unwrap().id();
  assert!(referenced_blobs(&s).is_empty());
  assert_eq!(s.get(id).unwrap().unwrap().reps[0].data.len(), INLINE_MAX + 1);
}

#[test]
fn tampered_or_missing_blob_is_an_error_not_garbage() {
  let d = tempfile::tempdir().unwrap();
  let k = DataKey::generate();
  let hk = hash_key_of(&k);
  let mut s = Store::open_encrypted(d.path(), &k).unwrap();
  let a = s.insert(image_item(&hk, INLINE_MAX + 4096, 6, t(0))).unwrap().id();
  let b = s.insert(image_item(&hk, INLINE_MAX + 4096, 7, t(1))).unwrap().id();
  let files = referenced_blobs(&s);
  let file_of = |id: ItemId| -> String {
    s.conn()
      .query_row("SELECT blob_file FROM reps WHERE item_id = ?1", params![id.0], |r| r.get(0))
      .unwrap()
  };
  let (fa, fb) = (file_of(a), file_of(b));
  assert_eq!(files.len(), 2);
  let bdir = d.path().join(BLOB_DIR_NAME);
  flip(&bdir.join(&fa), 70_000);
  match s.get(a) {
    Err(Error::Blob { item, file, source: spool_crypto::Error::Auth }) => {
      assert_eq!((item, file), (a.0, fa.clone()));
    }
    other => panic!("tampered: {other:?}"),
  }
  // Swapping files between names fails too (logical path = file name).
  std::fs::copy(bdir.join(&fb), bdir.join(&fa)).unwrap();
  assert!(matches!(s.get(a), Err(Error::Blob { .. })));
  std::fs::remove_file(bdir.join(&fb)).unwrap();
  assert!(matches!(s.get(b), Err(Error::Blob { .. })));
  // The items are still listed and deletable.
  let ids: Vec<ItemId> = s.recent(10).unwrap().into_iter().map(|r| r.id).collect();
  assert_eq!(ids, vec![b, a]);
  assert!(s.delete(a).unwrap());
  assert!(s.delete(b).unwrap());
  assert!(blob_names(d.path()).is_empty());
}

#[test]
fn session_merge_preserves_order_dedupes_and_spills() {
  let d = tempfile::tempdir().unwrap();
  let k = DataKey::generate();
  let hk = hash_key_of(&k);
  let mut p = Store::open_encrypted(d.path(), &k).unwrap();
  let pa = p.insert(text_item(&hk, Selection::Clipboard, "a", t(0))).unwrap().id();
  let seq_before = p.get(pa).unwrap().unwrap().change_seq;

  let sk = DataKey::generate();
  let shk = hash_key_of(&sk);
  let mut s = Store::open_session(&sk).unwrap();
  // Oldest first: "a" (dupe of the newest persistent item), "b", big image, primary "p".
  let sa = s.insert(text_item(&shk, Selection::Clipboard, "a", t(5))).unwrap().id();
  let sb = s.insert(text_item(&shk, Selection::Clipboard, "b", t(6))).unwrap().id();
  let img = image_item(&shk, INLINE_MAX + 100, 9, t(7));
  let si = s.insert(img.clone()).unwrap().id();
  let sp = s.insert(text_item(&shk, Selection::Primary, "p", t(8))).unwrap().id();
  s.touch(sb, t(9)).unwrap(); // b is now the newest
  s.conn().execute("INSERT INTO tags(item_id, tag) VALUES (?1, 'work')", params![sb.0]).unwrap();
  s.conn().execute("UPDATE items SET flags = 1 WHERE id = ?1", params![sp.0]).unwrap();

  let report = p.merge_from_session(&s).unwrap();
  assert_eq!((report.inserted, report.bumped, report.blob_files), (3, 1, 1));
  let order: Vec<ItemId> = report.ids.iter().map(|(sid, _)| *sid).collect();
  assert_eq!(order, vec![sa, si, sp, sb], "merged oldest first by last_used_at");
  assert_eq!(report.ids[0].1, InsertOutcome::Bumped(pa));

  // Order and timestamps preserved; pinned p first.
  let recent = p.recent(10).unwrap();
  let previews: Vec<&str> = recent.iter().map(|r| r.preview.as_deref().unwrap()).collect();
  assert_eq!(previews[0], "p");
  assert_eq!(&previews[1..], &["b", &format!("[image/png {} bytes]", INLINE_MAX + 100), "a"]);
  let b = p.get(recent[1].id).unwrap().unwrap();
  assert_eq!((b.created_at, b.last_used_at), (t(6), t(9)));
  assert!(b.change_seq > seq_before);
  let tags: Vec<String> = {
    let mut st = p.conn().prepare("SELECT tag FROM tags WHERE item_id = ?1").unwrap();
    st.query_map(params![b.id.0], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap()
  };
  assert_eq!(tags, vec!["work".to_string()]);
  let a = p.get(pa).unwrap().unwrap();
  assert_eq!((a.created_at, a.last_used_at), (t(0), t(5)));
  // Re-hashed with the persistent hash key.
  for r in &recent {
    let it = p.get(r.id).unwrap().unwrap();
    assert_eq!(it.hash, dedupe_hash(&hk, &it.reps[0]));
  }
  // The big rep moved to a file and reads back.
  assert_eq!(blob_names(d.path()).len(), 1);
  let ii = p.get(recent[2].id).unwrap().unwrap();
  assert_eq!(ii.reps, img.reps);
  assert_eq!(p.count().unwrap(), 4);
  // The session is untouched.
  assert_eq!(s.count().unwrap(), 4);
}

/// Setup for the rekey tests: text + two blob items + a primary item.
fn rekey_fixture(dir: &Path, k: &DataKey) -> Vec<(ItemId, Vec<Representation>)> {
  let hk = hash_key_of(k);
  let mut s = Store::open_encrypted(dir, k).unwrap();
  let items = vec![
    text_item(&hk, Selection::Clipboard, "rekey text", t(0)),
    image_item(&hk, INLINE_MAX + 1, 11, t(1)),
    image_item(&hk, INLINE_MAX + 4096, 12, t(2)),
    text_item(&hk, Selection::Primary, "rekey primary", t(3)),
  ];
  items.into_iter().map(|ni| (s.insert(ni.clone()).unwrap().id(), ni.reps)).collect()
}

fn assert_store_consistent(
  dir: &Path,
  s: &mut Store,
  k: &DataKey,
  items: &[(ItemId, Vec<Representation>)],
) {
  let hk = hash_key_of(k);
  assert_eq!(s.hash_key().unwrap(), hk);
  for (id, reps) in items {
    let it = s.get(*id).unwrap().unwrap();
    assert_eq!(&it.reps, reps);
    assert_eq!(it.hash, dedupe_hash(&hk, &it.reps[0]), "hash under the current key");
  }
  assert_eq!(blob_names(dir), referenced_blobs(s), "no stray blob files");
  assert_eq!(referenced_blobs(s).len(), 2);
  let marker: Option<String> = get_meta(s.conn(), META_REKEY_STATE).unwrap();
  assert_eq!(marker, None);
  let pending: i64 =
    s.conn().query_row("SELECT count(*) FROM rekey_blobs", [], |r| r.get(0)).unwrap();
  assert_eq!(pending, 0);
  let kcv = blobs::read_kcv(&dir.join(KCV_FILE_NAME)).unwrap();
  assert_eq!(kcv, vec![blobs::kcv_of(&k.derive(Label::Db))]);
}

#[test]
fn rekey_roundtrip_including_blobs_and_hashes() {
  let d = tempfile::tempdir().unwrap();
  let k1 = DataKey::generate();
  let k2 = DataKey::generate();
  let items = rekey_fixture(d.path(), &k1);
  let mut s = Store::open_encrypted(d.path(), &k1).unwrap();
  let before = blob_names(d.path());
  s.rekey(&k1).unwrap(); // same key: no-op
  assert_eq!(blob_names(d.path()), before);
  s.rekey(&k2).unwrap();
  let after = blob_names(d.path());
  assert_eq!(after.len(), 2);
  assert!(after.iter().all(|n| !before.contains(n)), "blob files renamed");
  assert_store_consistent(d.path(), &mut s, &k2, &items);
  // Dedupe works with the new hash key.
  let hk2 = hash_key_of(&k2);
  assert!(matches!(
    s.insert(text_item(&hk2, Selection::Primary, "rekey primary", t(10))).unwrap(),
    InsertOutcome::Bumped(_)
  ));
  drop(s);
  assert_err(Store::open_encrypted(d.path(), &k1), |e| matches!(e, Error::WrongKey), "old key");
  let mut s = Store::open_encrypted(d.path(), &k2).unwrap();
  assert_store_consistent(d.path(), &mut s, &k2, &items);
  // Old blob files under the old key cannot be read with the new one.
  assert!(before.iter().all(|n| !d.path().join(BLOB_DIR_NAME).join(n).exists()));
}

#[test]
fn interrupted_rekey_recovers_with_whichever_key_opens() {
  use RekeyStep::*;
  for (step, committed) in [
    (AfterMarker, false),
    (AfterBlobsWritten, false),
    (AfterSwap, false),
    (AfterPragmaRekey, true),
    (AfterOldBlobsDeleted, true),
  ] {
    let d = tempfile::tempdir().unwrap();
    let k1 = DataKey::generate();
    let k2 = DataKey::generate();
    let items = rekey_fixture(d.path(), &k1);
    let s = Store::open_encrypted(d.path(), &k1).unwrap();
    s.fail_at.set(Some(step));
    let mut s = s;
    assert!(s.rekey(&k2).is_err(), "{step:?}");
    drop(s); // "crash"

    let (good, bad) = if committed { (&k2, &k1) } else { (&k1, &k2) };
    assert_err(
      Store::open_encrypted(d.path(), bad),
      |e| matches!(e, Error::WrongKey),
      &format!("{step:?}: other key"),
    );
    let mut s = Store::open_encrypted(d.path(), good)
      .unwrap_or_else(|e| panic!("{step:?}: recovery open failed: {e}"));
    assert_store_consistent(d.path(), &mut s, good, &items);
    if !committed {
      // The rekey can simply be retried.
      s.rekey(&k2).unwrap();
      assert_store_consistent(d.path(), &mut s, &k2, &items);
    }
  }
}

/// Every regular file under `dir`, recursively.
fn all_files(dir: &Path) -> Vec<PathBuf> {
  let mut out = Vec::new();
  for e in std::fs::read_dir(dir).unwrap() {
    let p = e.unwrap().path();
    if p.is_dir() {
      out.extend(all_files(&p));
    } else {
      out.push(p);
    }
  }
  out
}

fn scan_for(dir: &Path, needles: &[&[u8]]) -> Vec<String> {
  let mut hits = Vec::new();
  for f in all_files(dir) {
    let data = std::fs::read(&f).unwrap();
    for n in needles {
      if data.windows(n.len()).any(|w| w == *n) {
        hits.push(format!("{} contains {:?}", f.display(), String::from_utf8_lossy(n)));
      }
    }
  }
  hits
}

/// Open SQLite temp files show up in /proc/self/fd as deleted `etilqs_*`.
fn sqlite_temp_fds() -> Vec<String> {
  std::fs::read_dir("/proc/self/fd")
    .map(|rd| {
      rd.filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .map(|p| p.display().to_string())
        .filter(|p| p.contains("etilqs_"))
        .collect()
    })
    .unwrap_or_default()
}

#[test]
fn no_plaintext_on_disk_when_encrypted() {
  const TEXT_MARKER: &[u8] = b"SPOOL-PLAINTEXT-MARKER-7f3a9c";
  const BLOB_MARKER: &[u8] = b"SPOOL-BLOB-MARKER-41d2e8";
  let d = tempfile::tempdir().unwrap();
  let k = DataKey::generate();
  let hk = hash_key_of(&k);
  let mut s = Store::open_encrypted(d.path(), &k).unwrap();
  let text = format!("secret {} text", String::from_utf8_lossy(TEXT_MARKER));
  let mut ni = text_item(&hk, Selection::Clipboard, &text, t(0));
  ni.source_app = Some(String::from_utf8_lossy(TEXT_MARKER).into_owned());
  s.insert(ni).unwrap();
  let mut big = Vec::new();
  while big.len() <= INLINE_MAX + 10_000 {
    big.extend_from_slice(BLOB_MARKER);
    big.extend_from_slice(b" filler ");
  }
  let canon = Representation::new("text/html", big);
  let html = NewItem {
    selection: Selection::Clipboard,
    source_app: None,
    created_at: t(1),
    flags: ItemFlags::empty(),
    hash: dedupe_hash(&hk, &canon),
    preview: Some(String::from_utf8_lossy(TEXT_MARKER).into_owned()),
    reps: vec![canon],
  };
  s.insert(html).unwrap();
  assert_eq!(blob_names(d.path()).len(), 1);

  // Temp b-trees stay in memory: a temp table and a big sort open no
  // SQLite temp file.
  let temp: i64 = s.conn().query_row("PRAGMA temp_store", [], |r| r.get(0)).unwrap();
  assert_eq!(temp, 2);
  s.conn()
    .execute_batch(
      "CREATE TEMP TABLE scratch AS SELECT preview || randomblob(2000) AS v FROM items;
       INSERT INTO scratch SELECT v FROM scratch; INSERT INTO scratch SELECT v FROM scratch;",
    )
    .unwrap();
  let n: i64 = s
    .conn()
    .query_row("SELECT count(*) FROM (SELECT v FROM scratch ORDER BY v DESC)", [], |r| r.get(0))
    .unwrap();
  assert_eq!(n, 8);
  assert!(sqlite_temp_fds().is_empty(), "{:?}", sqlite_temp_fds());

  // While open (WAL + SHM present) ...
  assert!(d.path().join(format!("{DB_FILE_NAME}-wal")).exists());
  let hits = scan_for(d.path(), &[TEXT_MARKER, BLOB_MARKER]);
  assert!(hits.is_empty(), "{hits:?}");
  // ... and after close.
  drop(s);
  let hits = scan_for(d.path(), &[TEXT_MARKER, BLOB_MARKER]);
  assert!(hits.is_empty(), "{hits:?}");
  // Sanity: the scanner does find plaintext.
  std::fs::write(d.path().join("probe"), TEXT_MARKER).unwrap();
  assert_eq!(scan_for(d.path(), &[TEXT_MARKER]).len(), 1);
}

#[test]
fn wipe_removes_everything() {
  let d = tempfile::tempdir().unwrap();
  let k = DataKey::generate();
  let hk = hash_key_of(&k);
  let mut s = Store::open_encrypted(d.path(), &k).unwrap();
  s.insert(image_item(&hk, INLINE_MAX + 1, 1, t(0))).unwrap();
  // WAL and SHM exist while open; wipe after close.
  drop(s);
  std::fs::write(d.path().join(format!("{DB_FILE_NAME}-wal")), b"x").unwrap();
  std::fs::write(d.path().join(format!("{DB_FILE_NAME}-shm")), b"x").unwrap();
  Store::wipe(d.path()).unwrap();
  assert!(all_files(d.path()).is_empty(), "{:?}", all_files(d.path()));
  assert!(!d.path().join(BLOB_DIR_NAME).exists());
  Store::wipe(d.path()).unwrap(); // idempotent
  // A fresh store can be created in its place with any key.
  Store::open_encrypted(d.path(), &DataKey::generate()).unwrap();
}

/// `meta.change_seq`, and a check that every writer keeps it strictly
/// increasing, equal to the highest seq in `items`/`tombstones`, and never
/// reused.
struct SeqWatch {
  last: i64,
}

impl SeqWatch {
  fn counter(s: &Store) -> i64 {
    get_meta::<i64>(s.conn(), META_CHANGE_SEQ).unwrap().unwrap_or(0)
  }

  /// `advanced`: the operation allocated at least one seq.
  fn check(&mut self, s: &Store, advanced: bool, what: &str) {
    let now = Self::counter(s);
    if advanced {
      assert!(now > self.last, "{what}: counter {now} <= {}", self.last);
    } else {
      assert_eq!(now, self.last, "{what}: counter moved");
    }
    assert_eq!(now, max_stored_change_seq(s.conn()).unwrap(), "{what}: counter != data max");
    let (total, distinct): (i64, i64) = s
      .conn()
      .query_row(
        "SELECT count(*), count(DISTINCT seq) FROM
           (SELECT change_seq AS seq FROM items UNION ALL SELECT change_seq FROM tombstones)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
      )
      .unwrap();
    assert_eq!(total, distinct, "{what}: duplicate change_seq");
    self.last = now;
  }
}

#[test]
fn change_seq_monotonic_across_sweep_merge_rekey_and_reopen() {
  let d = tempfile::tempdir().unwrap();
  let (k1, k2) = (DataKey::generate(), DataKey::generate());
  let hk1 = hash_key_of(&k1);
  let mut w = SeqWatch { last: 0 };
  let mut s = Store::open_encrypted(d.path(), &k1).unwrap();
  let mut ids = Vec::new();
  for i in 0..20 {
    ids.push(s.insert(text_item(&hk1, Selection::Clipboard, &format!("x{i}"), t(i))).unwrap().id());
    w.check(&s, true, "insert");
  }
  s.insert(image_item(&hk1, INLINE_MAX + 1, 3, t(20))).unwrap();
  w.check(&s, true, "insert blob");
  s.insert(image_item(&hk1, INLINE_MAX + 1, 3, t(21))).unwrap();
  w.check(&s, true, "bump");
  s.touch(ids[0], t(22)).unwrap();
  w.check(&s, true, "touch");
  s.delete(ids[1]).unwrap();
  w.check(&s, true, "delete");
  assert!(!s.delete(ids[1]).unwrap());
  w.check(&s, false, "delete missing");

  // Sweep: 21 items left (ids[0] touched to t(22)); keep the newest 5.
  let limits = RetentionLimits { max_age: Duration::from_secs(3650 * 86_400), max_items: 5 };
  let before = w.last;
  assert_eq!(s.retention_sweep(t(23), limits).unwrap(), 15);
  w.check(&s, true, "sweep");
  let tomb: Vec<(i64, i64)> = {
    let mut st = s
      .conn()
      .prepare("SELECT change_seq, item_id FROM tombstones WHERE change_seq > ?1 ORDER BY 1")
      .unwrap();
    st.query_map(params![before], |r| Ok((r.get(0)?, r.get(1)?)))
      .unwrap()
      .collect::<rusqlite::Result<_>>()
      .unwrap()
  };
  let seqs: Vec<i64> = tomb.iter().map(|x| x.0).collect();
  assert_eq!(seqs, (before + 1..=before + 15).collect::<Vec<_>>(), "consecutive, no gaps");
  // Oldest deleted first: x2, x3, ... (x0 was touched, x1 deleted).
  assert_eq!(tomb[0].1, ids[2].0);
  assert_eq!(s.retention_sweep(t(23), limits).unwrap(), 0);
  w.check(&s, false, "empty sweep");

  // Merge a session (one dupe of the newest clipboard item -> bump).
  let sk = DataKey::generate();
  let shk = hash_key_of(&sk);
  let mut sess = Store::open_session(&sk).unwrap();
  let newest = s.latest(Selection::Clipboard).unwrap().unwrap();
  let canon = newest.reps.iter().find(|r| !r.is_alias()).unwrap().clone();
  let mut dup = image_item(&shk, 1, 0, t(30));
  dup.hash = dedupe_hash(&shk, &canon);
  dup.reps = vec![canon];
  dup.source_app = newest.source_app.clone();
  sess.insert(dup).unwrap();
  sess.insert(text_item(&shk, Selection::Primary, "s1", t(31))).unwrap();
  sess.insert(text_item(&shk, Selection::Clipboard, "s2", t(32))).unwrap();
  let r = s.merge_from_session(&sess).unwrap();
  assert_eq!((r.inserted, r.bumped), (2, 1));
  w.check(&s, true, "merge");

  // Rekey re-hashes but allocates no seqs.
  s.rekey(&k2).unwrap();
  w.check(&s, false, "rekey");
  drop(s);

  let mut s = Store::open_encrypted(d.path(), &k2).unwrap();
  w.check(&s, false, "reopen");
  s.insert(text_item(&hash_key_of(&k2), Selection::Clipboard, "after", t(40))).unwrap();
  w.check(&s, true, "insert after reopen");
}

#[test]
fn reset_change_seq_counter_is_raised_at_open() {
  let d = tempfile::tempdir().unwrap();
  let k = DataKey::generate();
  let hk = hash_key_of(&k);
  let max = {
    let mut s = Store::open_encrypted(d.path(), &k).unwrap();
    let a = s.insert(text_item(&hk, Selection::Clipboard, "a", t(0))).unwrap().id();
    s.insert(text_item(&hk, Selection::Clipboard, "b", t(1))).unwrap();
    s.delete(a).unwrap(); // the max seq lives in tombstones
    let max = max_stored_change_seq(s.conn()).unwrap();
    s.conn().execute("DELETE FROM meta WHERE key = ?1", params![META_CHANGE_SEQ]).unwrap();
    max
  };
  let s = Store::open_encrypted(d.path(), &k).unwrap();
  assert_eq!(SeqWatch::counter(&s), max);
  s.conn().execute("UPDATE meta SET value = 1 WHERE key = ?1", params![META_CHANGE_SEQ]).unwrap();
  drop(s);
  let mut s = Store::open_encrypted(d.path(), &k).unwrap();
  let id = s.insert(text_item(&hk, Selection::Clipboard, "c", t(2))).unwrap().id();
  assert_eq!(s.get(id).unwrap().unwrap().change_seq, max + 1);
}

#[test]
fn migration_collapse_leaves_duplicate_blobs_to_gc() {
  let d = tempfile::tempdir().unwrap();
  let k = DataKey::generate();
  let hk = hash_key_of(&k);
  let (keep, dup) = {
    let mut s = Store::open_encrypted(d.path(), &k).unwrap();
    let img = image_item(&hk, INLINE_MAX + 1, 4, t(0));
    let dup = s.insert(img.clone()).unwrap().id();
    // A second copy with its own blob file (the old dedupe let it in once
    // another item had been copied in between).
    let mut again = img.clone();
    again.hash = dedupe_hash(&hk, &Representation::new("image/png", b"other".to_vec()));
    again.created_at = t(5);
    let keep = s.insert(again).unwrap().id();
    s.conn()
      .execute(
        "UPDATE items SET hash = (SELECT hash FROM items WHERE id = ?1) WHERE id = ?2",
        params![dup.0, keep.0],
      )
      .unwrap();
    s.conn().execute_batch(ROLLBACK_TO_V3).unwrap();
    assert_eq!(blob_names(d.path()).len(), 2);
    (keep, dup)
  };

  let mut s = Store::open_encrypted(d.path(), &k).unwrap();
  assert_eq!(s.get(dup).unwrap(), None);
  let it = s.get(keep).unwrap().unwrap();
  assert_eq!(it.created_at, t(0));
  assert_eq!(it.reps[0].data.len(), INLINE_MAX + 1);
  assert_eq!(blob_names(d.path()), referenced_blobs(&s));
  assert_eq!(blob_names(d.path()).len(), 1);
  assert_eq!(
    s.insert(image_item(&hk, INLINE_MAX + 1, 4, t(9))).unwrap(),
    InsertOutcome::Bumped(keep)
  );
}

#[test]
fn replace_content_swaps_blob_files() {
  let d = tempfile::tempdir().unwrap();
  let k = DataKey::generate();
  let hk = hash_key_of(&k);
  let mut s = Store::open_encrypted(d.path(), &k).unwrap();
  let id = s.insert_derived(image_item(&hk, INLINE_MAX + 1, 1, t(0)), None).unwrap().id();
  let first = blob_names(d.path());
  assert_eq!(first.len(), 1);

  let v2 = image_item(&hk, INLINE_MAX + 7, 2, t(1));
  assert_eq!(s.replace_content(id, v2.clone()).unwrap(), Some(ReplaceOutcome::Replaced(id)));
  let second = blob_names(d.path());
  assert_eq!(second.len(), 1, "old blob removed after the commit");
  assert_ne!(second, first);
  assert_eq!(referenced_blobs(&s), second);
  assert_eq!(s.get(id).unwrap().unwrap().reps, v2.reps);

  // Small again: inline, no blob files left.
  let v3 = image_item(&hk, 10, 3, t(2));
  s.replace_content(id, v3.clone()).unwrap();
  assert!(blob_names(d.path()).is_empty());
  drop(s);
  let s = Store::open_encrypted(d.path(), &k).unwrap();
  assert_eq!(s.get(id).unwrap().unwrap().reps, v3.reps);
}

#[test]
fn session_merge_remaps_derived_from() {
  let d = tempfile::tempdir().unwrap();
  let k = DataKey::generate();
  let hk = hash_key_of(&k);
  let mut p = Store::open_encrypted(d.path(), &k).unwrap();
  // Shift persistent ids away from the session's.
  for i in 0..3 {
    p.insert(text_item(&hk, Selection::Clipboard, &format!("p{i}"), t(i))).unwrap();
  }
  let sk = DataKey::generate();
  let shk = hash_key_of(&sk);
  let mut s = Store::open_session(&sk).unwrap();
  let orig = s.insert(text_item(&shk, Selection::Clipboard, "orig", t(10))).unwrap().id();
  let mut ed = text_item(&shk, Selection::Clipboard, "edited", t(11));
  ed.source_app = Some("spool.editor".into());
  let edit = s.insert_derived(ed, Some(orig)).unwrap().id();
  let mut lost = text_item(&shk, Selection::Clipboard, "lost origin", t(12));
  lost.source_app = Some("spool.editor".into());
  let orphan = s.insert_derived(lost, Some(ItemId(999))).unwrap().id();
  // The original is used after the edit, so it is merged after it.
  s.touch(orig, t(20)).unwrap();

  let report = p.merge_from_session(&s).unwrap();
  let map: std::collections::HashMap<ItemId, ItemId> =
    report.ids.iter().map(|(sid, out)| (*sid, out.id())).collect();
  assert_ne!(map[&orig], orig);
  assert_eq!(p.derived_from(map[&edit]).unwrap(), Some(map[&orig]));
  assert_eq!(p.derived_from(map[&orphan]).unwrap(), None, "origin not in the session");
  assert_eq!(p.derived_from(map[&orig]).unwrap(), None);
}
