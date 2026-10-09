//! Full-text search over Spool's clipboard history.
//!
//! A Tantivy index whose files are encrypted at rest ([`EncryptedDirectory`],
//! keyed by `DataKey::derive(Label::Index)`), plus a typing-oriented query
//! layer and recency/pin-aware ranking.
//!
//! - [`SearchIndex::open`] opens the on-disk index and reports
//!   [`OpenOutcome::Ready`] (with the `last_change_seq` stored in the commit
//!   payload, for catch-up from the store) or [`OpenOutcome::NeedsRebuild`].
//! - [`SearchIndex::rebuild`] builds a fresh index in a sibling directory and
//!   swaps it in atomically.
//! - [`SearchIndex::in_memory`] is the unencrypted RAM index for the
//!   pre-unlock session.
//! - [`Indexer`] is the single writer: [`Indexer::upsert`], [`Indexer::delete`],
//!   batched [`Indexer::commit_if_due`] (>= [`COMMIT_BATCH`]), [`Indexer::commit`].
//! - [`SearchIndex::search`] returns ranked [`Hit`]s.
//!
//! The crate never logs indexed text or query strings.
//!
//! Matching (typing mode) is per word: exact, prefix of the word being typed,
//! edit distance 1 for words of 4+ characters, and (M7) mid-word substrings
//! for words of 3+ characters via a per-word trigram field (`ctl` finds
//! `kubectl`). Explicit syntax does not use substring matching.

#![deny(unsafe_code)]

mod directory;
mod error;
mod index;
mod query;
mod schema;

pub use directory::EncryptedDirectory;
pub use error::{Error, Result};
pub use index::{
  COMMIT_BATCH, IndexDoc, IndexIdentity, Indexer, MAX_INDEXED_TEXT, OpenOutcome, Opened,
  RebuildReason, SearchIndex,
};
pub use query::{Filters, Hit, MAX_QUERY_CHARS, MAX_QUERY_TERMS, RankParams};
pub use schema::{SCHEMA_VERSION, TOKENIZER, TRIGRAM_TOKENIZER};
