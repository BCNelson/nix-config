#![allow(dead_code)]

use spool_search::{IndexDoc, IndexIdentity, OpenOutcome, Opened, RebuildReason};

pub const NOW: i64 = 1_800_000_000_000;
pub const HOUR: i64 = 3_600_000;
pub const DAY: i64 = 24 * HOUR;

pub fn doc(id: i64, text: &str) -> IndexDoc {
  IndexDoc {
    id,
    change_seq: id,
    text: text.to_string(),
    mime: vec!["text/plain".into(), "text/plain;charset=utf-8".into()],
    app: None,
    tags: vec![],
    created_at: NOW - HOUR,
    pinned: false,
  }
}

/// Temp dir on tmpfs when available: fsync on the dev machine's disk takes
/// 0.3-0.9 s, which makes commit-heavy tests crawl.
pub fn tempdir() -> tempfile::TempDir {
  tempfile::tempdir_in("/dev/shm").or_else(|_| tempfile::tempdir()).unwrap()
}

pub fn identity() -> IndexIdentity {
  IndexIdentity::new("db-uuid-1")
}

pub fn ready(o: OpenOutcome) -> Box<Opened> {
  match o {
    OpenOutcome::Ready(o) => o,
    OpenOutcome::NeedsRebuild(r) => panic!("expected Ready, got NeedsRebuild({r:?})"),
  }
}

pub fn needs(o: OpenOutcome) -> RebuildReason {
  match o {
    OpenOutcome::Ready(_) => panic!("expected NeedsRebuild, got Ready"),
    OpenOutcome::NeedsRebuild(r) => r,
  }
}

/// Small deterministic PRNG (xorshift64*), so tests need no extra deps.
pub struct Rng(pub u64);

impl Rng {
  pub fn next(&mut self) -> u64 {
    let mut x = self.0;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    self.0 = x;
    x.wrapping_mul(0x2545_f491_4f6c_dd1d)
  }

  pub fn below(&mut self, n: u64) -> u64 {
    self.next() % n
  }
}
