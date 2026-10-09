//! Store benchmarks: insert (new / dedupe bump), `recent(50)`, `latest` and
//! `retention_sweep`, on a temp-file database (WAL, real fsyncs) and on an
//! in-memory one for comparison.
//!
//! Run with `cargo bench -p spool-core --bench store`.
//!
//! Fixtures: items are generated through the public API
//! (`Policy::manual_item` + `Store::insert`), one transaction per item since
//! the API has no batch insert. Each size is built once into a template
//! database cached next to the bench binary
//! (`<target>/release/spool-bench-fixtures/`) and reused by later runs
//! (building 50k used to take ~7 min while `insert` was O(n); it is now a
//! few seconds). Benches copy the template file (file backend) or
//! load its rows with `ATTACH` + `INSERT ... SELECT` (memory backend, via the
//! doc-hidden `Store::conn`), so the measured operations always start from
//! the same state. Bump `FIXTURE_VERSION` when the generator changes.

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use criterion::{BatchSize, BenchmarkId, Criterion, SamplingMode, criterion_group, criterion_main};
use spool_core::config::{Config, RetentionLimits};
use spool_core::item::{NewItem, Selection};
use spool_core::policy::Policy;
use spool_core::store::{InsertOutcome, Store};
use tempfile::TempDir;

const SIZES: &[usize] = &[5_000, 50_000];
/// Part of the cached template file name; bump when `text`/`new_item` or
/// the schema change so stale fixtures are rebuilt (v2: migration 3).
const FIXTURE_VERSION: u32 = 2;
/// Timestamp of item 0; item `i` is created `i` seconds later.
const BASE_MS: u64 = 1_760_000_000_000;

fn at(i: u64) -> SystemTime {
  SystemTime::UNIX_EPOCH + Duration::from_millis(BASE_MS + i * 1000)
}

const WORDS: &[&str] = &[
  "copy",
  "the",
  "build",
  "log",
  "from",
  "line",
  "42",
  "and",
  "paste",
  "it",
  "into",
  "chat",
  "url",
  "https://example.org/a/b",
  "meeting",
  "notes",
  "fn",
  "main()",
  "TODO",
  "fix",
];

/// Small (~60-200 byte) unique text, like a typical clipboard copy.
fn text(i: u64) -> Bytes {
  let mut s = format!("item {i}:");
  let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
  for _ in 0..8 + (i % 24) {
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    s.push(' ');
    s.push_str(WORDS[(x % WORDS.len() as u64) as usize]);
  }
  Bytes::from(s)
}

fn new_item(policy: &Policy, i: u64) -> NewItem {
  // Every 10th item on the primary selection, like a desktop that records it.
  let sel = if i % 10 == 9 { Selection::Primary } else { Selection::Clipboard };
  policy.manual_item(sel, "text/plain;charset=utf-8", text(i), at(i)).unwrap()
}

fn policy_for(store: &mut Store) -> Policy {
  let key = store.hash_key().unwrap();
  Policy::new(Config { primary_selection: true, ..Config::default() }, key).unwrap()
}

/// Insert items `0..n` through the public API.
///
/// `synchronous=OFF` is set on this connection only (not persisted; benches
/// reopen with the store's own defaults). With the default, every insert
/// fsyncs the WAL: 5k items took ~400 s under the agent `lowprio.slice`
/// (IOWeight=10).
fn populate(store: &mut Store, n: usize) {
  store.conn().pragma_update(None, "synchronous", "OFF").unwrap();
  let policy = policy_for(store);
  for i in 0..n as u64 {
    let out = store.insert(new_item(&policy, i)).unwrap();
    assert!(matches!(out, InsertOutcome::Inserted(_)));
  }
}

fn fixture_dir() -> PathBuf {
  // <target>/release/deps/store-<hash> -> <target>/release/spool-bench-fixtures
  let exe = std::env::current_exe().unwrap();
  let release = exe.parent().and_then(Path::parent).unwrap();
  release.join("spool-bench-fixtures")
}

/// A populated file database (cached across runs) that benches copy.
struct Template {
  path: PathBuf,
  n: usize,
  /// Scratch space for copies; removed on drop.
  scratch: TempDir,
}

impl Template {
  fn get(n: usize) -> Self {
    let dir = fixture_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("template-v{FIXTURE_VERSION}-{n}.db"));
    let cached =
      path.exists() && Store::open(&path, None).and_then(|s| s.count()).ok() == Some(n as u64);
    if cached {
      eprintln!("[store bench] using cached {}", path.display());
    } else {
      let tmp = dir.join(format!("template-v{FIXTURE_VERSION}-{n}.db.building"));
      for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", tmp.display()));
      }
      let t0 = Instant::now();
      let mut store = Store::open(&tmp, None).unwrap();
      populate(&mut store, n);
      drop(store); // last connection closed: WAL checkpointed and removed
      std::fs::rename(&tmp, &path).unwrap();
      eprintln!("[store bench] built {} ({n} items) in {:.1?}", path.display(), t0.elapsed());
    }
    Self { path, n, scratch: tempfile::tempdir().unwrap() }
  }

  /// Fresh file copy in its own temp dir (deleted when the `TempDir` drops).
  fn open_copy(&self) -> (Store, TempDir) {
    let dir = tempfile::tempdir_in(self.scratch.path()).unwrap();
    let path = dir.path().join("history.db");
    std::fs::copy(&self.path, &path).unwrap();
    (Store::open(&path, None).unwrap(), dir)
  }

  /// In-memory store with the template's rows (same ids, hashes, key).
  fn open_memory(&self) -> Store {
    let store = Store::open_in_memory().unwrap();
    let c = store.conn();
    c.execute("ATTACH DATABASE ?1 AS t", [self.path.to_str().unwrap()]).unwrap();
    c.execute_batch(
      "INSERT OR REPLACE INTO main.meta SELECT * FROM t.meta;
       INSERT INTO main.items SELECT * FROM t.items ORDER BY id;
       INSERT INTO main.reps SELECT * FROM t.reps ORDER BY rowid;
       INSERT INTO main.tombstones SELECT * FROM t.tombstones;
       DETACH DATABASE t;",
    )
    .unwrap();
    assert_eq!(store.count().unwrap(), self.n as u64);
    store
  }
}

/// One populated store per (backend, size), shared by the read and insert
/// benches. Inserts grow it by a few hundred rows during measurement (< 10%
/// even at 5k), which is acceptable drift.
struct Fixture {
  backend: &'static str,
  n: usize,
  store: Store,
  policy: Policy,
  next: u64,
  _dir: Option<TempDir>,
}

fn fixtures(templates: &[Template]) -> Vec<Fixture> {
  let mut out = Vec::new();
  for t in templates {
    let (mut store, dir) = t.open_copy();
    let policy = policy_for(&mut store);
    out.push(Fixture { backend: "file", n: t.n, store, policy, next: t.n as u64, _dir: Some(dir) });
  }
  for t in templates {
    let mut store = t.open_memory();
    let policy = policy_for(&mut store);
    out.push(Fixture { backend: "memory", n: t.n, store, policy, next: t.n as u64, _dir: None });
  }
  out
}

fn id(f: &Fixture) -> BenchmarkId {
  BenchmarkId::new(f.backend, f.n)
}

fn bench_store(c: &mut Criterion) {
  let templates: Vec<Template> = SIZES.iter().map(|&n| Template::get(n)).collect();
  let mut fx = fixtures(&templates);

  // ---- reads (before the inserts mutate the fixtures) ----
  let mut g = c.benchmark_group("store_recent_50");
  g.sample_size(20).measurement_time(Duration::from_secs(3));
  for f in &fx {
    assert_eq!(f.store.recent(50).unwrap().len(), 50);
    g.bench_function(id(f), |b| b.iter(|| f.store.recent(black_box(50)).unwrap()));
  }
  g.finish();

  let mut g = c.benchmark_group("store_latest");
  g.sample_size(20).measurement_time(Duration::from_secs(3));
  for f in &fx {
    assert!(f.store.latest(Selection::Clipboard).unwrap().is_some());
    g.bench_function(id(f), |b| {
      b.iter(|| f.store.latest(black_box(Selection::Clipboard)).unwrap())
    });
  }
  g.finish();

  // ---- inserts (file backend: one WAL fsync per insert) ----
  let mut g = c.benchmark_group("store_insert_new");
  g.sample_size(10).measurement_time(Duration::from_secs(5)).sampling_mode(SamplingMode::Flat);
  for f in &mut fx {
    let bid = id(f);
    g.bench_function(bid, |b| {
      b.iter_batched(
        || {
          f.next += 1;
          new_item(&f.policy, f.next)
        },
        |item| {
          let out = f.store.insert(item).unwrap();
          debug_assert!(matches!(out, InsertOutcome::Inserted(_)));
          out
        },
        BatchSize::SmallInput,
      )
    });
  }
  g.finish();

  let mut g = c.benchmark_group("store_insert_duplicate");
  g.sample_size(10).measurement_time(Duration::from_secs(5)).sampling_mode(SamplingMode::Flat);
  for f in &mut fx {
    // A new clipboard copy, inserted once; re-inserting it is the
    // dedupe-bump path (same hash as the newest clipboard item).
    f.next += 1;
    let dup = f.policy.manual_item(Selection::Clipboard, "text/plain", text(u64::MAX), at(f.next));
    let dup = dup.unwrap();
    f.store.insert(dup.clone()).unwrap();
    assert!(matches!(f.store.insert(dup.clone()).unwrap(), InsertOutcome::Bumped(_)));
    let bid = id(f);
    g.bench_function(bid, |b| {
      b.iter_batched(|| dup.clone(), |item| f.store.insert(item).unwrap(), BatchSize::SmallInput)
    });
  }
  g.finish();
  drop(fx);

  // ---- retention sweep: delete the oldest 10% (fresh copy per iteration) ----
  let mut g = c.benchmark_group("store_retention_sweep_10pct");
  g.sample_size(10).measurement_time(Duration::from_secs(10)).sampling_mode(SamplingMode::Flat);
  for t in &templates {
    let limits = RetentionLimits {
      max_age: Duration::from_secs(3650 * 86_400),
      max_items: (t.n - t.n / 10) as u32,
    };
    let now = at(t.n as u64);
    g.bench_function(BenchmarkId::new("file", t.n), |b| {
      b.iter_batched(
        || t.open_copy(),
        |(mut store, dir)| {
          assert_eq!(store.retention_sweep(now, limits).unwrap(), t.n / 10);
          (store, dir) // dropped (and deleted) outside the measurement
        },
        BatchSize::PerIteration,
      )
    });
  }
  // In-memory comparison (no fsync / checkpoint cost).
  for t in &templates {
    let limits = RetentionLimits {
      max_age: Duration::from_secs(3650 * 86_400),
      max_items: (t.n - t.n / 10) as u32,
    };
    g.bench_function(BenchmarkId::new("memory", t.n), |b| {
      b.iter_batched(
        || t.open_memory(),
        |mut store| {
          assert_eq!(store.retention_sweep(at(t.n as u64), limits).unwrap(), t.n / 10);
          store
        },
        BatchSize::PerIteration,
      )
    });
  }
  g.finish();
}

criterion_group!(benches, bench_store);
criterion_main!(benches);
