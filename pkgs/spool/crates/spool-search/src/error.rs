use std::io;

/// Errors from spool-search. Messages never contain indexed text or the
/// query string.
#[derive(Debug, thiserror::Error)]
pub enum Error {
  /// The query is longer than [`MAX_QUERY_CHARS`](crate::MAX_QUERY_CHARS).
  #[error("query too long ({0} characters)")]
  QueryTooLong(usize),
  /// The query has more than [`MAX_QUERY_TERMS`](crate::MAX_QUERY_TERMS) terms.
  #[error("query has too many terms ({0})")]
  TooManyTerms(usize),
  /// Range, set, regex or exists syntax, or an unknown field.
  #[error("unsupported query syntax: {0}")]
  UnsupportedSyntax(&'static str),
  /// The explicit query did not parse.
  #[error("query syntax error")]
  InvalidQuery,
  /// Item ids must be non-negative.
  #[error("negative item id")]
  NegativeId,
  /// Another process (or another `Indexer` in this one) holds the index lock.
  #[error("search index is locked by another writer")]
  Locked,
  #[error(transparent)]
  Tantivy(#[from] tantivy::TantivyError),
  #[error(transparent)]
  Io(#[from] io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
