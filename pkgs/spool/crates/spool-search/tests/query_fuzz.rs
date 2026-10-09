//! Property test over arbitrary query strings: never panics, bounded time.

mod common;

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use common::*;
use proptest::prelude::*;
use spool_search::{Filters, Indexer, SearchIndex};

fn index() -> &'static (SearchIndex, std::sync::Mutex<Indexer>) {
  static IDX: OnceLock<(SearchIndex, std::sync::Mutex<Indexer>)> = OnceLock::new();
  IDX.get_or_init(|| {
    let o = SearchIndex::in_memory().unwrap();
    let (s, mut w) = (o.search, o.indexer);
    let texts = [
      "kubectl get pods",
      "https://example.com/a?b=c",
      "café crème brûlée",
      "SELECT * FROM t WHERE x = 'y';",
      "fn main() { println!(\"hi\"); }",
      "日本語のテキスト",
    ];
    for (i, t) in texts.iter().enumerate() {
      let mut d = doc(i as i64 + 1, t);
      d.tags = vec!["work".into()];
      d.app = Some("app".into());
      w.upsert(&d).unwrap();
    }
    w.commit().unwrap();
    (s, std::sync::Mutex::new(w))
  })
}

/// Characters that exercise the query grammar.
fn grammar_string() -> impl Strategy<Value = String> {
  let atoms = prop_oneof![
    Just("\"".to_string()),
    Just(":".to_string()),
    Just("-".to_string()),
    Just("+".to_string()),
    Just("(".to_string()),
    Just(")".to_string()),
    Just("[".to_string()),
    Just("]".to_string()),
    Just("{".to_string()),
    Just("}".to_string()),
    Just("*".to_string()),
    Just("^".to_string()),
    Just("~".to_string()),
    Just("/".to_string()),
    Just("\\".to_string()),
    Just(" ".to_string()),
    Just(" TO ".to_string()),
    Just(" AND ".to_string()),
    Just(" OR ".to_string()),
    Just(" IN ".to_string()),
    Just("app:".to_string()),
    Just("mime:".to_string()),
    Just("tag:".to_string()),
    Just("text:".to_string()),
    Just("pinned:".to_string()),
    Just("id:".to_string()),
    Just("kube".to_string()),
    Just("é".to_string()),
    Just("2".to_string()),
    "[a-z]{1,6}",
  ];
  prop::collection::vec(atoms, 0..40).prop_map(|v| v.concat())
}

fn check(q: &str) {
  let (s, _) = index();
  let t = Instant::now();
  let r = s.search_at(q, &Filters::default(), 20, 0, NOW);
  let el = t.elapsed();
  // Generous bound (debug build, busy machine); real queries take ~ms.
  assert!(el < Duration::from_secs(2), "query took {el:?}");
  if let Ok(hits) = r {
    assert!(hits.len() <= 20);
    assert!(hits.iter().all(|h| h.score.is_finite()));
  }
}

proptest! {
  // PROPTEST_CASES overrides the default of 2000.
  #![proptest_config(ProptestConfig {
    cases: std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(2000),
    ..ProptestConfig::default()
  })]

  #[test]
  fn arbitrary_unicode_never_panics(q in "\\PC{0,300}") {
    check(&q);
  }

  #[test]
  fn grammar_soup_never_panics(q in grammar_string()) {
    check(&q);
  }
}
