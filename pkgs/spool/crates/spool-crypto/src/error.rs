use std::io;

/// Errors from spool-crypto. Messages never contain key or plaintext bytes.
#[derive(Debug, thiserror::Error)]
pub enum Error {
  /// AEAD authentication failed: wrong key, wrong logical path / AAD, or the
  /// data was modified, truncated, extended or reordered.
  #[error("authentication failed (wrong key or tampered data)")]
  Auth,
  /// The file header is malformed (bad magic, unknown version, non-zero
  /// reserved bits, unsupported chunk size).
  #[error("bad header: {0}")]
  BadHeader(&'static str),
  /// The file length cannot be produced by the chunked format.
  #[error("impossible encrypted file length {0}")]
  InvalidLength(u64),
  /// A chunk size outside `MIN_CHUNK_SIZE_LOG2..=MAX_CHUNK_SIZE_LOG2`.
  #[error("unsupported chunk size log2 {0}")]
  ChunkSize(u8),
  /// A chunk index or byte range outside the plaintext.
  #[error("out of range")]
  OutOfRange,
  /// A sealed blob is too short to contain a nonce and tag.
  #[error("sealed blob too short")]
  Truncated,
  #[error(transparent)]
  Io(#[from] io::Error),
}

impl From<Error> for io::Error {
  fn from(e: Error) -> io::Error {
    match e {
      Error::Io(e) => e,
      other => io::Error::new(io::ErrorKind::InvalidData, other),
    }
  }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
