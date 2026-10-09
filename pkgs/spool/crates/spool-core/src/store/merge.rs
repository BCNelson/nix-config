//! Merging the pre-unlock session store into the persistent store.
//!
//! An in-memory database cannot be `ATTACH`ed from another connection, so
//! the merge copies rows through [`Store::get`] (which also resolves any
//! blob files of the source) and the same insert path as
//! [`Store::insert`].

use rusqlite::params;
use spool_crypto::Zeroizing;

use super::{InsertOutcome, RowData, Store, blobs, insert_row, to_millis};
use crate::item::{ItemId, dedupe_hash};
use crate::{Error, Result};

/// What [`Store::merge_from_session`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeReport {
  /// Session items that became new rows.
  pub inserted: usize,
  /// Session items that bumped the newest persistent item instead.
  pub bumped: usize,
  /// Blob files written for large reps.
  pub blob_files: usize,
  /// `(session id, outcome in this store)`, oldest first. The daemon uses
  /// it to remap ids it handed out while locked (e.g. `PolicyState`).
  pub ids: Vec<(ItemId, InsertOutcome)>,
}

impl Store {
  /// Copy every item of `session` into this store, in one transaction:
  ///
  /// - oldest first (by `last_used_at`, then id), preserving `created_at`,
  ///   `last_used_at`, selection, source app, flags, preview and tags;
  /// - re-hashed with **this** store's hash key (the session's hashes were
  ///   keyed with the session key);
  /// - deduped like [`Store::insert`]: equal to the newest item of the same
  ///   selection bumps it (`last_used_at = max(old, session)`; flags are
  ///   not merged);
  /// - reps larger than [`super::INLINE_MAX`] become blob files (encrypted
  ///   stores);
  /// - fresh `change_seq`s. Session tombstones are ignored.
  ///
  /// On error nothing is committed and blob files written so far are
  /// removed. `session` is not modified; drop it only after `Ok`.
  pub fn merge_from_session(&mut self, session: &Store) -> Result<MergeReport> {
    let hash_key = Zeroizing::new(self.hash_key()?);
    let ids: Vec<i64> = {
      let mut stmt =
        session.conn.prepare("SELECT id FROM items ORDER BY last_used_at ASC, id ASC")?;
      stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    let mut report = MergeReport::default();
    let Store { conn, mode, .. } = self;
    let mut pending = blobs::Pending::new(mode);
    let tx = conn.transaction()?;
    let mut tags_stmt = session.conn.prepare("SELECT tag FROM tags WHERE item_id = ?1")?;
    for sid in ids {
      let Some(item) = session.get(ItemId(sid))? else { continue };
      let canonical = item.reps.iter().find(|r| !r.is_alias()).ok_or_else(|| {
        Error::Corrupt(format!("session item {sid}: no canonical representation"))
      })?;
      let hash = dedupe_hash(&hash_key, canonical);
      let row = RowData {
        selection: item.selection,
        source_app: item.source_app.as_deref(),
        created_at: to_millis(item.created_at),
        last_used_at: to_millis(item.last_used_at),
        flags: item.flags,
        hash: &hash,
        preview: item.preview.as_deref(),
        total_size: item.total_size as i64,
      };
      let before = pending.len();
      let out = insert_row(&tx, mode, &row, &item.reps, &mut pending)?;
      report.blob_files += pending.len() - before;
      match out {
        InsertOutcome::Inserted(id) => {
          report.inserted += 1;
          let tags: Vec<Option<String>> =
            tags_stmt.query_map(params![sid], |r| r.get(0))?.collect::<Result<_, _>>()?;
          for tag in tags {
            tx.execute("INSERT INTO tags(item_id, tag) VALUES (?1, ?2)", params![id.0, tag])?;
          }
        }
        InsertOutcome::Bumped(_) => report.bumped += 1,
      }
      report.ids.push((item.id, out));
    }
    tx.commit()?;
    pending.disarm();
    tracing::info!(
      inserted = report.inserted,
      bumped = report.bumped,
      blob_files = report.blob_files,
      "store: merged session"
    );
    Ok(report)
  }
}
