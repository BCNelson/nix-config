//! Query construction (typing mode and explicit syntax), filters and ranking.

use std::ops::Bound;

use tantivy::query::{
  AllQuery, BooleanQuery, BoostQuery, ConstScoreQuery, FuzzyTermQuery, Occur, PhraseQuery, Query,
  QueryParser, RangeQuery, TermQuery,
};
use tantivy::query_grammar::{self, UserInputAst, UserInputLeaf};
use tantivy::schema::IndexRecordOption;
use tantivy::tokenizer::TokenStream;
use tantivy::{DateTime, Index, Term};

use crate::error::{Error, Result};
use crate::schema::{Fields, GRAM, analyzer, trigrams};

/// Longest accepted query, in characters.
pub const MAX_QUERY_CHARS: usize = 256;
/// Most terms accepted in one query.
pub const MAX_QUERY_TERMS: usize = 16;
/// Terms at least this long (in characters) get edit distance 1.
const FUZZY_MIN_CHARS: usize = 4;
/// Weight of a fuzzy (constant-score) match relative to an exact BM25 match.
const FUZZY_BOOST: f32 = 0.5;
/// Constant score of a mid-word substring match: below a true prefix match
/// (1.0) and any exact BM25 match, above a fuzzy match ([`FUZZY_BOOST`]).
const SUBSTRING_SCORE: f32 = 0.75;
/// Most trigram terms generated for one query (across all its words). A
/// word whose full cover does not fit uses its first and last trigram; one
/// for which even that does not fit gets no substring clause.
const MAX_TRIGRAM_TERMS: usize = 64;
/// Field prefixes that switch to explicit syntax.
const EXPLICIT_FIELDS: [&str; 5] = ["text:", "app:", "mime:", "tag:", "pinned:"];

/// One search result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
  /// The store's item id.
  pub id: i64,
  /// Final ranking score (BM25 tweaked by recency and pin).
  pub score: f32,
}

/// Restrictions applied on top of the query. They never change scores.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filters {
  /// Keep items with at least one MIME type starting with this (e.g.
  /// `image/` or `text/plain`).
  pub mime_prefix: Option<String>,
  /// Keep items whose source app is exactly this.
  pub app: Option<String>,
  /// Keep pinned items only.
  pub pinned_only: bool,
  /// Keep items created at or after this (ms since the Unix epoch).
  pub created_from: Option<i64>,
  /// Keep items created strictly before this (ms since the Unix epoch).
  pub created_until: Option<i64>,
}

/// Ranking parameters:
/// `score = bm25 * (1 + recency_weight * 0.5^(age / half_life)) + pin_weight * pinned`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RankParams {
  /// Recency half-life in seconds (default one day).
  pub half_life_secs: f64,
  /// Recency weight `w_r` (default 2.0): a brand-new item scores up to 3x.
  pub recency_weight: f64,
  /// Additive pin bonus `w_p` (default 1.0).
  pub pin_weight: f64,
}

impl Default for RankParams {
  fn default() -> Self {
    RankParams { half_life_secs: 86_400.0, recency_weight: 2.0, pin_weight: 1.0 }
  }
}

impl RankParams {
  /// Final score from the BM25 score, item age and pin flag.
  pub fn score(&self, bm25: f32, age_ms: i64, pinned: bool) -> f32 {
    let age_secs = (age_ms.max(0) as f64) / 1000.0;
    let decay =
      if self.half_life_secs > 0.0 { (-age_secs / self.half_life_secs).exp2() } else { 0.0 };
    let pin = if pinned { self.pin_weight } else { 0.0 };
    (f64::from(bm25) * (1.0 + self.recency_weight * decay) + pin) as f32
  }
}

/// Reject over-long queries before doing any work.
pub(crate) fn check_limits(q: &str) -> Result<()> {
  let chars = q.chars().count();
  if chars > MAX_QUERY_CHARS {
    return Err(Error::QueryTooLong(chars));
  }
  let words = q.split_whitespace().count();
  if words > MAX_QUERY_TERMS {
    return Err(Error::TooManyTerms(words));
  }
  Ok(())
}

/// Explicit syntax: a quote, a known `field:` prefix, or a leading `-`.
pub(crate) fn is_explicit(q: &str) -> bool {
  let t = q.trim_start();
  t.starts_with('-')
    || q.contains('"')
    || q.split_whitespace().any(|w| {
      let w = w.trim_start_matches(['-', '+', '(']);
      EXPLICIT_FIELDS.iter().any(|f| w.starts_with(f))
    })
}

/// Build the scoring query for `q` (filters are added separately).
pub(crate) fn build(index: &Index, f: &Fields, q: &str) -> Result<Box<dyn Query>> {
  check_limits(q)?;
  if is_explicit(q) { explicit(index, f, q) } else { typing(f, q) }
}

/// Mid-word substring clause for one (folded) word of 3+ characters: a
/// phrase over `text_tri` with trigrams that cover the word (every third
/// window plus the last), pinned to their offsets, so it matches exactly the
/// words containing `word`. The trigram analyzer leaves a position gap
/// between words, so a match never spans two words. `None` for shorter words
/// or when `budget` (trigram terms left) is exhausted.
fn substring(f: &Fields, word: &str, budget: &mut usize) -> Option<Box<dyn Query>> {
  let grams = trigrams(word);
  let last = grams.len().checked_sub(1)?;
  let mut picks: Vec<usize> = (0..=last).step_by(GRAM).collect();
  if picks.last() != Some(&last) {
    picks.push(last);
  }
  if picks.len() > *budget {
    // Over budget: first and last trigram only (still exact for words of up
    // to 6 characters; longer ones may match a word that differs only in
    // the middle).
    picks = if last == 0 { vec![0] } else { vec![0, last] };
    if picks.len() > *budget {
      return None;
    }
  }
  *budget -= picks.len();
  let term = |i: usize| Term::from_field_text(f.text_tri, grams[i]);
  let q: Box<dyn Query> = if picks.len() == 1 {
    Box::new(TermQuery::new(term(picks[0]), IndexRecordOption::Basic))
  } else {
    Box::new(PhraseQuery::new_with_offset(picks.into_iter().map(|i| (i, term(i))).collect()))
  };
  Some(Box::new(ConstScoreQuery::new(q, SUBSTRING_SCORE)))
}

/// Typing mode: every completed word must match (exactly, or within edit
/// distance 1 if it has 4+ characters); the word being typed matches as a
/// prefix (with distance 1 if it has 4+ characters). Any word of 3+
/// characters may also match mid-word (substring, via trigrams).
fn typing(f: &Fields, q: &str) -> Result<Box<dyn Query>> {
  let mut a = analyzer();
  let mut ts = a.token_stream(q);
  let mut tokens: Vec<(String, usize)> = vec![];
  while ts.advance() {
    let t = ts.token();
    tokens.push((t.text.clone(), t.offset_to));
  }
  if tokens.is_empty() {
    return Ok(Box::new(AllQuery));
  }
  if tokens.len() > MAX_QUERY_TERMS {
    return Err(Error::TooManyTerms(tokens.len()));
  }
  // The last token is still being typed iff nothing follows it.
  let in_progress = tokens.last().is_some_and(|(_, end)| *end == q.len());
  let n = tokens.len();
  let mut budget = MAX_TRIGRAM_TERMS;
  let clauses: Vec<(Occur, Box<dyn Query>)> = tokens
    .into_iter()
    .enumerate()
    .map(|(i, (tok, _))| {
      let term = Term::from_field_text(f.text, &tok);
      let long = tok.chars().count() >= FUZZY_MIN_CHARS;
      let exact: Box<dyn Query> =
        Box::new(TermQuery::new(term.clone(), IndexRecordOption::WithFreqs));
      let fuzzy: Option<FuzzyTermQuery> = if i + 1 == n && in_progress {
        Some(FuzzyTermQuery::new_prefix(term, u8::from(long), true))
      } else if long {
        Some(FuzzyTermQuery::new(term, 1, true))
      } else {
        None
      };
      let mut alts: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Should, exact)];
      if let Some(fz) = fuzzy {
        alts.push((Occur::Should, Box::new(BoostQuery::new(Box::new(fz), FUZZY_BOOST))));
      }
      if i + 1 == n && in_progress && long {
        // A true prefix outranks a prefix within edit distance 1.
        let prefix = FuzzyTermQuery::new_prefix(Term::from_field_text(f.text, &tok), 0, false);
        alts.push((Occur::Should, Box::new(prefix)));
      }
      if let Some(sub) = substring(f, &tok, &mut budget) {
        alts.push((Occur::Should, sub));
      }
      let q: Box<dyn Query> = if alts.len() == 1 {
        alts.pop().expect("one").1
      } else {
        Box::new(BooleanQuery::new(alts))
      };
      (Occur::Must, q)
    })
    .collect();
  Ok(Box::new(BooleanQuery::new(clauses)))
}

/// Explicit syntax through Tantivy's grammar, restricted: terms, phrases,
/// `+`/`-`, `AND`/`OR`, parentheses, `field:` for text/app/mime/tag/pinned.
/// No ranges, sets, regexes or `field:*`.
fn explicit(index: &Index, f: &Fields, q: &str) -> Result<Box<dyn Query>> {
  // `field:*` is an exists query (rejected below); a `*` glued to anything
  // else trips a panic in tantivy-query-grammar 0.26 (`*\"`, `*["`).
  if q.split_whitespace().any(|w| w != "*" && w.contains('*')) {
    return Err(Error::UnsupportedSyntax("wildcard"));
  }
  // Belt and braces: the grammar must never take the daemon down.
  let mut ast = no_panic(|| query_grammar::parse_query(q))?.map_err(|_| Error::InvalidQuery)?;
  let mut words = 0usize;
  sanitize(&mut ast, &mut words)?;
  if words > MAX_QUERY_TERMS {
    return Err(Error::TooManyTerms(words));
  }
  // A purely negative query ("-foo") means "everything except".
  if let UserInputAst::Clause(cs) = &mut ast {
    if !cs.is_empty() && cs.iter().all(|(o, _)| *o == Some(query_grammar::Occur::MustNot)) {
      cs.push((Some(query_grammar::Occur::Must), UserInputAst::Leaf(Box::new(UserInputLeaf::All))));
    }
  }
  let mut parser = QueryParser::for_index(index, vec![f.text]);
  parser.set_conjunction_by_default();
  no_panic(|| parser.build_query_from_user_input_ast(ast))?.map_err(|_| Error::InvalidQuery)
}

/// Run third-party query parsing, turning a panic into [`Error::InvalidQuery`].
/// The panic message comes from Tantivy and does not include the query.
fn no_panic<T>(f: impl FnOnce() -> T) -> Result<T> {
  std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|_| {
    tracing::debug!("query parser panicked; rejecting query");
    Error::InvalidQuery
  })
}

fn sanitize(ast: &mut UserInputAst, words: &mut usize) -> Result<()> {
  match ast {
    UserInputAst::Clause(cs) => cs.iter_mut().try_for_each(|(_, c)| sanitize(c, words)),
    UserInputAst::Boost(inner, _) => sanitize(inner, words),
    UserInputAst::Leaf(leaf) => match leaf.as_mut() {
      UserInputLeaf::Literal(lit) => {
        *words += lit.phrase.split_whitespace().count().max(1);
        match lit.field_name.as_deref() {
          None | Some("text" | "app" | "mime" | "pinned") => {}
          Some("tag") => {
            // Tags are facets; accept `tag:work` as well as `tag:/work`.
            if !lit.phrase.starts_with('/') {
              lit.phrase.insert(0, '/');
            }
          }
          Some(_) => return Err(Error::UnsupportedSyntax("unknown field")),
        }
        Ok(())
      }
      UserInputLeaf::All => Ok(()),
      UserInputLeaf::Range { .. } => Err(Error::UnsupportedSyntax("range")),
      UserInputLeaf::Set { .. } => Err(Error::UnsupportedSyntax("set")),
      UserInputLeaf::Exists { .. } => Err(Error::UnsupportedSyntax("exists")),
      UserInputLeaf::Regex { .. } => Err(Error::UnsupportedSyntax("regex")),
    },
  }
}

/// Wrap `scoring` with filter clauses that contribute no score.
pub(crate) fn with_filters(f: &Fields, scoring: Box<dyn Query>, flt: &Filters) -> Box<dyn Query> {
  let mut filters: Vec<Box<dyn Query>> = vec![];
  if let Some(p) = &flt.mime_prefix {
    let term = Term::from_field_text(f.mime, p);
    filters.push(Box::new(FuzzyTermQuery::new_prefix(term, 0, false)));
  }
  if let Some(app) = &flt.app {
    filters
      .push(Box::new(TermQuery::new(Term::from_field_text(f.app, app), IndexRecordOption::Basic)));
  }
  if flt.pinned_only {
    filters.push(Box::new(TermQuery::new(
      Term::from_field_bool(f.pinned, true),
      IndexRecordOption::Basic,
    )));
  }
  if flt.created_from.is_some() || flt.created_until.is_some() {
    let lo = flt.created_from.map_or(Bound::Unbounded, |ms| {
      Bound::Included(Term::from_field_date(f.created, DateTime::from_timestamp_millis(ms)))
    });
    let hi = flt.created_until.map_or(Bound::Unbounded, |ms| {
      Bound::Excluded(Term::from_field_date(f.created, DateTime::from_timestamp_millis(ms)))
    });
    filters.push(Box::new(RangeQuery::new(lo, hi)));
  }
  if filters.is_empty() {
    return scoring;
  }
  let mut clauses = vec![(Occur::Must, scoring)];
  clauses.extend(
    filters
      .into_iter()
      .map(|q| (Occur::Must, Box::new(ConstScoreQuery::new(q, 0.0)) as Box<dyn Query>)),
  );
  Box::new(BooleanQuery::new(clauses))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn explicit_detection() {
    for q in
      ["\"exact phrase\"", "app:firefox", "-secret", "  -x y", "foo mime:image/png", "+tag:work"]
    {
      assert!(is_explicit(q), "{q}");
    }
    for q in ["kubectl get", "https://example.com/a:b", "rm -rf /", "a-b", "key: value", ""] {
      assert!(!is_explicit(q), "{q}");
    }
  }

  #[test]
  fn substring_cover_and_budget() {
    let (_, f) = crate::schema::build();
    let mut budget = MAX_TRIGRAM_TERMS;
    assert!(substring(&f, "ab", &mut budget).is_none());
    assert_eq!(budget, 64);
    substring(&f, "ctl", &mut budget).unwrap();
    assert_eq!(budget, 63);
    // 7 chars -> windows 0..=4 -> picks 0, 3, 4.
    substring(&f, "kubectl", &mut budget).unwrap();
    assert_eq!(budget, 60);
    // 64 chars -> 62 windows -> 22 picks (0, 3, .., 60, 61).
    let mut budget = MAX_TRIGRAM_TERMS;
    let long = "a".repeat(64);
    for _ in 0..2 {
      substring(&f, &long, &mut budget).unwrap();
    }
    assert_eq!(budget, 20);
    // Full cover does not fit: first + last trigram.
    substring(&f, &long, &mut budget).unwrap();
    assert_eq!(budget, 18);
    let mut budget = 1;
    assert!(substring(&f, &long, &mut budget).is_none());
    assert_eq!(budget, 1);
    substring(&f, "abc", &mut budget).unwrap();
    assert_eq!(budget, 0);
    assert!(substring(&f, "abc", &mut budget).is_none());
  }

  #[test]
  fn limits() {
    assert!(check_limits(&"a".repeat(256)).is_ok());
    assert!(matches!(check_limits(&"a".repeat(257)), Err(Error::QueryTooLong(257))));
    assert!(matches!(check_limits(&"a ".repeat(17)), Err(Error::TooManyTerms(17))));
  }

  #[test]
  fn parser_panics_become_errors() {
    let r: Result<()> = no_panic(|| panic!("boom"));
    assert!(matches!(r, Err(Error::InvalidQuery)));
  }

  #[test]
  fn rank_formula() {
    let p = RankParams::default();
    assert!((p.score(1.0, 0, false) - 3.0).abs() < 1e-6);
    assert!((p.score(1.0, 86_400_000, false) - 2.0).abs() < 1e-6);
    assert!((p.score(1.0, 86_400_000, true) - 3.0).abs() < 1e-6);
    // Future timestamps (clock skew) are treated as age 0.
    assert!((p.score(1.0, -5000, false) - 3.0).abs() < 1e-6);
  }
}
