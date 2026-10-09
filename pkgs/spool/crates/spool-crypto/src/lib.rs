//! Spool on-disk symmetric cryptography.
//!
//! Spool encrypts its history with one random 256-bit [`DataKey`]. Every
//! consumer gets its own [`SubKey`] via HKDF-SHA256 with a fixed [`Label`]:
//!
//! | Label | Use |
//! | --- | --- |
//! | `spool/db/v1` | SQLCipher raw key ([`SubKey::sqlcipher_pragma_hex`]) |
//! | `spool/index/v1` | Tantivy encrypted directory (M3) |
//! | `spool/blob/v1` | large blob files ([`chunked`]) |
//! | `spool/hash/v1` | keyed BLAKE3 dedupe key |
//!
//! On top of that this crate provides:
//!
//! - [`chunked`]: a chunked ChaCha20-Poly1305 format for write-once files with
//!   random-access reads ([`ChunkedWriter`], [`ChunkedReader`], [`seal_file`],
//!   [`open_file`]).
//! - [`sealed`]: XChaCha20-Poly1305 for small blobs such as key slots
//!   ([`seal`], [`open`]).
//!
//! # Key hygiene
//!
//! Key material lives in a heap allocation that is zeroized on drop and
//! best-effort `mlock`ed (a failed `mlock` is logged at debug and otherwise
//! ignored). Key types have no `Clone` and a redacted `Debug`.
//! [`DataKey::expose`] / [`SubKey::expose`] are the **only** ways to get at the
//! raw bytes; callers must not copy them anywhere that outlives the key.
//!
//! `unsafe` is denied crate-wide; the single exception is the private `mlock`
//! module (two libc calls plus `sysconf`).

#![deny(unsafe_code)]

pub mod chunked;
mod error;
mod key;
mod mlock;
pub mod sealed;

pub use chunked::{
  ChunkedReader, ChunkedWriter, DEFAULT_CHUNK_SIZE_LOG2, HEADER_LEN, MAX_CHUNK_SIZE_LOG2,
  MIN_CHUNK_SIZE_LOG2, open_file, plaintext_len, seal_file,
};
pub use error::{Error, Result};
pub use key::{DataKey, KEY_LEN, Label, SubKey};
pub use sealed::{open, seal};
pub use zeroize::Zeroizing;
