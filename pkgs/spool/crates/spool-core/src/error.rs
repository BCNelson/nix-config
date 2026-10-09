//! Crate error type.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
  /// SQLite error. `SQLITE_CORRUPT` / `SQLITE_NOTADB` after a successful
  /// open are mapped to [`Error::Corrupt`] instead (see `From`).
  #[error("sqlite: {0}")]
  Sqlite(#[source] rusqlite::Error),
  #[error("schema migration: {0}")]
  Migration(#[from] rusqlite_migration::Error),
  #[error("i/o: {0}")]
  Io(#[from] std::io::Error),
  #[error("config {path}: {message}")]
  Config { path: PathBuf, message: String },
  /// Invalid policy configuration (bad regex, bad allowlist pattern, ...).
  #[error("policy: {0}")]
  Policy(String),
  /// Database exists but cannot be read with the given key (wrong key, or
  /// encrypted DB opened without a key).
  #[error("database is locked or the key is wrong")]
  WrongKey,
  /// Stored data violates an invariant (unknown selection string, bad hash
  /// length, ...).
  #[error("corrupt store: {0}")]
  Corrupt(String),
  /// An encrypted store was requested but the file is a plaintext SQLite
  /// database (e.g. a development database at the production path).
  #[error("database is not encrypted")]
  NotEncrypted,
  /// A blob file of `item` is missing, unreadable, or fails authentication
  /// (tampered, truncated, swapped). The item row itself is intact and can
  /// be listed and deleted.
  #[error("item {item}: blob file {file}: {source}")]
  Blob {
    item: i64,
    file: String,
    #[source]
    source: spool_crypto::Error,
  },
  /// The operation does not apply to this kind of store.
  #[error("unsupported: {0}")]
  Unsupported(&'static str),
}

impl From<rusqlite::Error> for Error {
  fn from(e: rusqlite::Error) -> Self {
    use rusqlite::ErrorCode::{DatabaseCorrupt, NotADatabase};
    match e.sqlite_error_code() {
      Some(code @ (DatabaseCorrupt | NotADatabase)) => {
        Error::Corrupt(format!("sqlite reports {code:?}: {e}"))
      }
      _ => Error::Sqlite(e),
    }
  }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
