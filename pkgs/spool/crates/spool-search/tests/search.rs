//! Query behaviour, ranking and filters (in-memory index).

mod common;

use common::*;
use spool_search::{Error, Filters, IndexDoc, Indexer, RankParams, SearchIndex};

fn index(docs: &[IndexDoc]) -> (SearchIndex, Indexer) {
  let o = SearchIndex::in_memory().unwrap();
  let (search, mut indexer) = (o.search, o.indexer);
  for d in docs {
    indexer.upsert(d).unwrap();
  }
  indexer.commit().unwrap();
  (search, indexer)
}

fn ids_f(idx: &SearchIndex, q: &str, f: &Filters) -> Vec<i64> {
  idx.search_at(q, f, 50, 0, NOW).unwrap().into_iter().map(|h| h.id).collect()
}

fn ids(idx: &SearchIndex, q: &str) -> Vec<i64> {
  ids_f(idx, q, &Filters::default())
}

fn sorted(mut v: Vec<i64>) -> Vec<i64> {
  v.sort();
  v
}

#[test]
fn typing_prefixes() {
  let (s, _w) = index(&[
    doc(1, "kubectl get pods -n kube-system"),
    doc(2, "cargo build --release"),
    doc(3, "Kubernetes docs"),
  ]);
  assert_eq!(sorted(ids(&s, "k")), [1, 3]);
  assert_eq!(sorted(ids(&s, "kub")), [1, 3]);
  assert_eq!(sorted(ids(&s, "kube")), [1, 3]);
  // "kubec" is within distance 1 of "kuber..." too, but a true prefix
  // match ranks first.
  assert_eq!(ids(&s, "kubec"), [1, 3]);
  assert_eq!(ids(&s, "kubect"), [1]);
  assert_eq!(ids(&s, "kubectl"), [1]);
  // Completed word + word in progress: both must match.
  assert_eq!(ids(&s, "kubectl ge"), [1]);
  assert!(ids(&s, "kubectl rel").is_empty());
  assert_eq!(ids(&s, "CARGO bu"), [2]);
  // Accent folding.
  let (s, _w) = index(&[doc(1, "café crème")]);
  assert_eq!(ids(&s, "cafe"), [1]);
  assert_eq!(ids(&s, "CRÈME"), [1]);
}

#[test]
fn typos() {
  let (s, _w) = index(&[doc(1, "kubectl apply -f deploy.yaml"), doc(2, "systemctl restart nginx")]);
  // Missing letter, as the word being typed and as a completed word.
  assert_eq!(ids(&s, "kubctl"), [1]);
  assert_eq!(ids(&s, "kubctl "), [1]);
  // Transposition.
  assert_eq!(ids(&s, "systemclt restart"), [2]);
  assert_eq!(ids(&s, "ngnix "), [2]);
  // Short words are exact.
  let (s, _w) = index(&[doc(1, "cat file"), doc(2, "cut file")]);
  assert_eq!(ids(&s, "cat "), [1]);
  // Exact matches outrank fuzzy ones.
  let (s, _w) = index(&[doc(1, "nginx"), doc(2, "ngnix")]);
  assert_eq!(ids(&s, "nginx "), [1, 2]);
}

#[test]
fn recency_beats_slightly_better_old_match() {
  let mut old = doc(1, "docker compose up docker");
  old.created_at = NOW - 10 * DAY;
  let mut new = doc(2, "docker compose up");
  new.created_at = NOW - HOUR;
  let (s, _w) = index(&[old, new]);
  let hits = s.search_at("docker", &Filters::default(), 10, 0, NOW).unwrap();
  assert_eq!(hits.iter().map(|h| h.id).collect::<Vec<_>>(), [2, 1]);
  assert!(hits[0].score > hits[1].score);
  // Without the recency term the old item (higher tf) wins.
  s.set_rank_params(RankParams { recency_weight: 0.0, ..RankParams::default() });
  assert_eq!(ids(&s, "docker "), [1, 2]);
}

#[test]
fn pinned_boost() {
  let mut a = doc(1, "ssh root@server");
  let mut b = doc(2, "ssh root@server");
  a.created_at = NOW - 2 * HOUR;
  b.created_at = NOW - 2 * HOUR;
  b.pinned = true;
  let (s, _w) = index(&[a, b]);
  assert_eq!(ids(&s, "ssh"), [2, 1]);
  // Empty query: everything, pinned first among equals, then by recency.
  let mut c = doc(3, "newest");
  c.created_at = NOW;
  let (s, _w) = index(&[c, doc(4, "older")]);
  assert_eq!(ids(&s, ""), [3, 4]);
}

#[test]
fn filters() {
  let mut img = doc(1, "screenshot");
  img.mime = vec!["image/png".into()];
  img.app = Some("org.kde.spectacle".into());
  img.created_at = NOW - 3 * DAY;
  let mut txt = doc(2, "screenshot notes");
  txt.app = Some("org.kde.kate".into());
  txt.pinned = true;
  let (s, _w) = index(&[img, txt]);
  let f = |f: Filters| sorted(ids_f(&s, "screens", &f));
  assert_eq!(f(Filters::default()), [1, 2]);
  assert_eq!(f(Filters { mime_prefix: Some("image/".into()), ..Default::default() }), [1]);
  assert_eq!(f(Filters { mime_prefix: Some("text/plain".into()), ..Default::default() }), [2]);
  assert_eq!(
    f(Filters { mime_prefix: Some("video/".into()), ..Default::default() }),
    Vec::<i64>::new()
  );
  assert_eq!(f(Filters { app: Some("org.kde.kate".into()), ..Default::default() }), [2]);
  assert_eq!(f(Filters { pinned_only: true, ..Default::default() }), [2]);
  assert_eq!(f(Filters { created_from: Some(NOW - DAY), ..Default::default() }), [2]);
  assert_eq!(f(Filters { created_until: Some(NOW - DAY), ..Default::default() }), [1]);
  assert_eq!(
    f(Filters {
      created_from: Some(NOW - 4 * DAY),
      created_until: Some(NOW),
      ..Default::default()
    }),
    [1, 2]
  );
  // Filters do not change scores.
  let plain = s.search_at("notes", &Filters::default(), 5, 0, NOW).unwrap();
  let filtered =
    s.search_at("notes", &Filters { pinned_only: true, ..Default::default() }, 5, 0, NOW).unwrap();
  assert_eq!(plain, filtered);
  // Filters with an empty query.
  assert_eq!(
    ids_f(&s, "", &Filters { mime_prefix: Some("image/".into()), ..Default::default() }),
    [1]
  );
}

#[test]
fn explicit_syntax() {
  let mut a = doc(1, "git push origin main");
  a.app = Some("konsole".into());
  a.tags = vec!["work".into()];
  let mut b = doc(2, "main origin push secret");
  b.app = Some("firefox".into());
  b.mime = vec!["text/html".into()];
  let (s, _w) = index(&[a, b, doc(3, "unrelated")]);
  assert_eq!(ids(&s, "\"push origin\""), [1]);
  assert_eq!(sorted(ids(&s, "push origin")), [1, 2]);
  assert_eq!(ids(&s, "app:firefox push"), [2]);
  assert_eq!(ids(&s, "-secret push"), [1]);
  // A '-' after the first word is typing mode (think "rm -rf").
  assert_eq!(ids(&s, "push -secret"), [2]);
  assert_eq!(sorted(ids(&s, "-secret")), [1, 3]);
  assert_eq!(ids(&s, "tag:work"), [1]);
  assert_eq!(ids(&s, "tag:/work push"), [1]);
  assert_eq!(ids(&s, "mime:\"text/html\""), [2]);
  assert_eq!(ids(&s, "text:unrelated"), [3]);
  assert_eq!(sorted(ids(&s, "app:firefox OR app:konsole")), [1, 2]);
  assert_eq!(sorted(ids(&s, "-unrelated *")), [1, 2]);
  // Rejected.
  // `*\"` and `*["` panic inside tantivy-query-grammar 0.26; they must be
  // clean errors here.
  for q in [
    "text:[a TO z]",
    "app:{a TO b}",
    "\"x\" bogus:field",
    "text:IN [a b]",
    "*\\\"",
    "*[\"",
    "app:*",
  ] {
    let e = s.search_at(q, &Filters::default(), 5, 0, NOW).unwrap_err();
    assert!(matches!(e, Error::UnsupportedSyntax(_) | Error::InvalidQuery), "{q}: {e:?}");
  }
}

#[test]
fn rejects_long_or_complex_queries() {
  let (s, _w) = index(&[doc(1, "x")]);
  let long = "a".repeat(257);
  assert!(matches!(s.search(&long, &Filters::default(), 5, 0), Err(Error::QueryTooLong(257))));
  let many = (0..17).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ");
  assert!(matches!(s.search(&many, &Filters::default(), 5, 0), Err(Error::TooManyTerms(17))));
  // 17 tokens without whitespace (punctuation splits them).
  let packed = (0..17).map(|i| format!("w{i}")).collect::<Vec<_>>().join(".");
  assert!(matches!(s.search(&packed, &Filters::default(), 5, 0), Err(Error::TooManyTerms(17))));
  assert!(s.search(&"a".repeat(256), &Filters::default(), 5, 0).is_ok());
}

#[test]
fn upsert_delete_and_paging() {
  let (s, mut w) = index(&[doc(1, "alpha"), doc(2, "alpha beta")]);
  assert_eq!(sorted(ids(&s, "alpha")), [1, 2]);
  w.upsert(&doc(1, "gamma")).unwrap();
  w.delete(2, 10).unwrap();
  // Not visible before commit.
  assert_eq!(sorted(ids(&s, "alpha")), [1, 2]);
  w.commit().unwrap();
  assert!(ids(&s, "alpha").is_empty());
  assert_eq!(ids(&s, "gamma"), [1]);
  assert_eq!(s.num_docs(), 1);
  assert_eq!(w.last_change_seq(), 10);
  assert!(matches!(w.delete(-1, 11), Err(Error::NegativeId)));

  let docs: Vec<IndexDoc> = (1..=30)
    .map(|i| {
      let mut d = doc(i, "page");
      d.created_at = NOW - i * HOUR;
      d
    })
    .collect();
  let (s, _w) = index(&docs);
  let p1: Vec<i64> =
    s.search_at("page", &Filters::default(), 10, 0, NOW).unwrap().iter().map(|h| h.id).collect();
  let p2: Vec<i64> =
    s.search_at("page", &Filters::default(), 10, 10, NOW).unwrap().iter().map(|h| h.id).collect();
  assert_eq!(p1, (1..=10).collect::<Vec<_>>());
  assert_eq!(p2, (11..=20).collect::<Vec<_>>());
  assert!(s.search_at("page", &Filters::default(), 0, 0, NOW).unwrap().is_empty());
}

#[test]
fn ram_session_index() {
  let o = SearchIndex::in_memory().unwrap();
  assert_eq!(o.last_change_seq, 0);
  let (s, mut w) = (o.search, o.indexer);
  w.upsert(&doc(5, "pre-unlock clip")).unwrap();
  w.commit().unwrap();
  assert_eq!(ids(&s, "unlock"), [5]);
  // Searchable from another thread.
  let s2 = s.clone();
  let h = std::thread::spawn(move || s2.search("clip", &Filters::default(), 5, 0).unwrap().len());
  assert_eq!(h.join().unwrap(), 1);
}

#[test]
fn huge_text_is_truncated_not_rejected() {
  let mut big = "word ".repeat(300_000);
  big.push_str("needleatend");
  let (s, _w) = index(&[doc(1, &format!("needlestart {big}"))]);
  assert_eq!(ids(&s, "needlestart"), [1]);
  assert!(ids(&s, "needleatend ").is_empty());
}

#[test]
fn mid_word_substrings() {
  let (s, _w) = index(&[
    doc(1, "kubectl get pods"),
    doc(2, "see https://www.example.com/path?q=1"),
    doc(3, "Café Crème"),
    doc(4, "systemctl status"),
  ]);
  // As the word being typed and as a completed word.
  assert_eq!(sorted(ids(&s, "ctl")), [1, 4]);
  assert_eq!(sorted(ids(&s, "ctl ")), [1, 4]);
  assert_eq!(ids(&s, "ectl"), [1]);
  assert_eq!(ids(&s, "bect "), [1]);
  assert_eq!(ids(&s, "temct"), [4]);
  // Dotted / URL-ish fragments: each word matches mid-word.
  assert_eq!(ids(&s, "xample.co"), [2]);
  assert_eq!(ids(&s, "ample.com/pat"), [2]);
  assert_eq!(ids(&s, "ttps://www.examp"), [2]);
  // Combined with other words: all must match.
  assert_eq!(ids(&s, "ubect pods"), [1]);
  assert!(ids(&s, "ubect status").is_empty());
  // Unicode folding, both directions.
  assert_eq!(ids(&s, "afe"), [3]);
  assert_eq!(ids(&s, "afé"), [3]);
  assert_eq!(ids(&s, "RÈM"), [3]);
  assert_eq!(ids(&s, "rem "), [3]);
  // Not a substring anywhere.
  assert!(ids(&s, "ctx").is_empty());
  assert!(ids(&s, "kubectlzz ").is_empty());
}

#[test]
fn substrings_do_not_cross_word_boundaries() {
  let (s, _w) = index(&[doc(1, "abc def"), doc(2, "foo.bar"), doc(3, "xyz-qrs"), doc(4, "abcdef")]);
  // "cde" / "bcde" would only exist across the space in doc 1.
  assert_eq!(ids(&s, "cde"), [4]);
  assert_eq!(ids(&s, "bcde "), [4]);
  // Punctuation separates words too.
  assert!(ids(&s, "oob").is_empty());
  assert!(ids(&s, "oba").is_empty());
  assert!(ids(&s, "yzq").is_empty());
  // The last trigram of one word and the first of the next are not adjacent
  // positions: "…abc" + "bcd…" must not look like "abcd" to a phrase.
  let (s, _w) = index(&[doc(1, "xabc bcdx"), doc(2, "xabcdx")]);
  assert_eq!(ids(&s, "abcd "), [2]);
}

#[test]
fn ranking_exact_prefix_substring_fuzzy() {
  let (s, _w) = index(&[
    doc(4, "spoil"),
    doc(3, "myspoolx"),
    doc(2, "spoolerd"),
    doc(1, "spool"),
    doc(5, "unrelated"),
  ]);
  // Word being typed: exact > prefix > substring > fuzzy.
  assert_eq!(ids(&s, "spool"), [1, 2, 3, 4]);
  // Completed word: exact > substring (a prefix is a substring too) > fuzzy.
  let r = ids(&s, "spool ");
  assert_eq!((r[0], sorted(r[1..3].to_vec()), r[3]), (1, vec![2, 3], 4));
  // Adding the substring clause leaves the prefix ordering intact.
  let (s, _w) = index(&[
    doc(1, "kubectl get pods -n kube-system"),
    doc(2, "cargo build --release"),
    doc(3, "Kubernetes docs"),
  ]);
  assert_eq!(ids(&s, "kubec"), [1, 3]);
  assert_eq!(sorted(ids(&s, "kube")), [1, 3]);
}

#[test]
fn short_and_long_terms_stay_bounded() {
  let (s, _w) = index(&[doc(1, "kubectl ab"), doc(2, "xabx")]);
  // < 3 chars: no substring clause, only word / prefix.
  assert_eq!(ids(&s, "ab "), [1]);
  assert!(ids(&s, "ct").is_empty());
  assert!(ids(&s, "bx").is_empty());
  // 16 mid-word fragments, all needing the substring clause: within the
  // trigram budget (3 trigrams each).
  let words: Vec<String> =
    (0..16u8).map(|i| format!("zzk{c}mnopq{c}zz", c = (b'a' + i) as char)).collect();
  let (s, _w) = index(&[doc(1, &words.join(" ")), doc(2, "other")]);
  let frags: Vec<&str> = words.iter().map(|w| &w[2..10]).collect();
  assert_eq!(ids(&s, &frags.join(" ")), [1]);
  // Long words blow the trigram budget (4 x 21 > 64): later words fall back
  // to first + last trigram; still a clean, bounded query that matches.
  let words: Vec<String> = (0..4).map(|i| format!("{}{i:02}", "q".repeat(58))).collect();
  let (s, _w) = index(&[doc(1, &words.join(" ")), doc(2, "other")]);
  assert_eq!(ids(&s, &words.join(" ")), [1]);
  let frags: Vec<&str> = words.iter().map(|w| &w[3..]).collect();
  let t = std::time::Instant::now();
  assert_eq!(ids(&s, &frags.join(" ")), [1]);
  assert!(t.elapsed() < std::time::Duration::from_secs(1));
  // 16 words of 14 characters at the 256-char limit.
  let words: Vec<String> = (0..16).map(|i| format!("{}{i:02}", "w".repeat(13))).collect();
  let (s, _w) = index(&[doc(1, &words.join(" "))]);
  let frags: Vec<&str> = words.iter().map(|w| &w[1..]).collect();
  assert_eq!(ids(&s, &frags.join(" ")), [1]);
}
