//! Query latency at 50k documents. Ignored by default; run in release:
//! `cargo test -p spool-search --release --test latency -- --ignored --nocapture`

mod common;

use std::time::{Duration, Instant};

use common::*;
use spool_crypto::DataKey;
use spool_search::{Filters, IndexDoc, SearchIndex};

const DOCS: usize = 50_000;
const QUERIES: usize = 2_000;

fn word(rng: &mut Rng) -> String {
  const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyzeeaaiioonnrrsstt";
  let len = 3 + rng.below(8) as usize;
  (0..len).map(|_| LETTERS[rng.below(LETTERS.len() as u64) as usize] as char).collect()
}

fn pct(sorted: &[Duration], p: f64) -> Duration {
  sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
}

#[test]
#[ignore = "benchmark; run with --release --ignored --nocapture"]
fn latency_50k() {
  let mut rng = Rng(0x5eed_1234_abcd_0001);
  let vocab: Vec<String> = (0..20_000).map(|_| word(&mut rng)).collect();
  let common_words = ["kubectl", "git", "cargo", "https", "github", "com", "docker", "the", "and"];
  let docs: Vec<IndexDoc> = (0..DOCS)
    .map(|i| {
      let n = 3 + rng.below(40) as usize;
      let mut text = String::new();
      for _ in 0..n {
        if rng.below(5) == 0 {
          text.push_str(common_words[rng.below(common_words.len() as u64) as usize]);
        } else {
          // Zipf-ish: favour the start of the vocabulary.
          let r =
            rng.below(vocab.len() as u64) * rng.below(vocab.len() as u64) / vocab.len() as u64;
          text.push_str(&vocab[r as usize]);
        }
        text.push(' ');
      }
      let mut d = doc(i as i64 + 1, &text);
      d.created_at = NOW - rng.below(90) as i64 * DAY;
      d.pinned = rng.below(100) == 0;
      d
    })
    .collect();

  let t = tempdir();
  let dir = t.path().join("index");
  let key = DataKey::generate();
  let start = Instant::now();
  let o = SearchIndex::rebuild(&dir, &key, &identity(), DOCS as i64, docs).unwrap();
  let build = start.elapsed();
  let on_disk: u64 =
    std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().metadata().unwrap().len()).sum();
  drop(o);
  let start = Instant::now();
  let o = ready(SearchIndex::open(&dir, &key, &identity()).unwrap());
  let open = start.elapsed();
  assert_eq!(o.search.num_docs(), DOCS as u64);

  // Typing-style queries: prefixes of 1..=len chars, two-word queries,
  // completed words, and typos.
  let queries: Vec<String> = (0..QUERIES)
    .map(|_| {
      let w = &vocab[rng.below(2_000) as usize];
      match rng.below(4) {
        0 => w[..1 + rng.below(w.len() as u64) as usize].to_string(),
        1 => format!("{} {}", common_words[rng.below(9) as usize], &w[..2.min(w.len())]),
        2 => format!("{w} "),
        _ => {
          let mut c: Vec<char> = w.chars().collect();
          c.remove(rng.below(c.len() as u64) as usize);
          c.into_iter().collect()
        }
      }
    })
    .collect();
  // Warm up.
  for q in queries.iter().take(100) {
    o.search.search(q, &Filters::default(), 50, 0).unwrap();
  }
  let mut times = Vec::with_capacity(QUERIES);
  let mut hits = 0usize;
  for q in &queries {
    let s = Instant::now();
    hits += o.search.search(q, &Filters::default(), 50, 0).unwrap().len();
    times.push(s.elapsed());
  }
  times.sort();
  let mut ftimes = Vec::new();
  let f = Filters { mime_prefix: Some("text/".into()), ..Default::default() };
  for q in queries.iter().take(500) {
    let s = Instant::now();
    o.search.search(q, &f, 50, 0).unwrap();
    ftimes.push(s.elapsed());
  }
  ftimes.sort();
  // Mid-word fragments (substring matching): drop 1-2 leading characters
  // of a vocabulary word, sometimes the trailing one too.
  let mut stimes = Vec::new();
  let mut shits = 0usize;
  for _ in 0..500 {
    let w = &vocab[rng.below(2_000) as usize];
    let from = 1 + rng.below(2) as usize;
    let to = w.len() - rng.below(2) as usize;
    let q = if to > from { &w[from..to] } else { &w[..] };
    let s = Instant::now();
    shits += o.search.search(q, &Filters::default(), 50, 0).unwrap().len();
    stimes.push(s.elapsed());
  }
  stimes.sort();
  println!(
    "latency_50k: build {build:?}, open {open:?}, on-disk {:.1} MiB, avg hits {:.1}\n  \
     plain:    p50 {:?} p90 {:?} p99 {:?} max {:?}\n  \
     filtered: p50 {:?} p99 {:?}\n  \
     mid-word: p50 {:?} p99 {:?} max {:?}, avg hits {:.1}",
    on_disk as f64 / 1048576.0,
    hits as f64 / QUERIES as f64,
    pct(&times, 0.5),
    pct(&times, 0.9),
    pct(&times, 0.99),
    times.last().unwrap(),
    pct(&ftimes, 0.5),
    pct(&ftimes, 0.99),
    pct(&stimes, 0.5),
    pct(&stimes, 0.99),
    stimes.last().unwrap(),
    shits as f64 / 500.0,
  );
}
