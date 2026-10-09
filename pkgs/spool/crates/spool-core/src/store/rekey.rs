//! Changing an encrypted store's [`DataKey`] ([`Store::rekey`]) and
//! recovering an interrupted rekey at open.

use rusqlite::params;
use spool_crypto::{DataKey, Label, Zeroizing};

use super::{
  Encrypted, META_REKEY_NEW_KCV, META_REKEY_STATE, Mode, Store, blobs, checkpoint, get_meta,
  set_meta,
};
use crate::item::{ItemId, dedupe_hash};
use crate::{Error, Result};

/// New blob files are being written; the DB still references the old ones.
const STATE_STAGING: &str = "staging";
/// The DB references the new blob files (old ones still on disk); the
/// SQLCipher key may be old (rekey not committed) or new.
const STATE_SWAPPED: &str = "swapped";

/// Points at which tests inject a simulated crash.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RekeyStep {
  AfterMarker,
  AfterBlobsWritten,
  AfterSwap,
  AfterPragmaRekey,
  AfterOldBlobsDeleted,
}

impl Store {
  /// Re-encrypt the whole store under `new_key`: the database
  /// (`PRAGMA rekey` with the new `Label::Db` sub-key), every blob file (new
  /// `Label::Blob` sub-key, new file names) and every dedupe hash (new
  /// `Label::Hash` sub-key). Afterwards [`Store::hash_key`] returns the new
  /// hash key: the caller must recompile its `Policy`.
  ///
  /// Only for [`super::StoreKind::Encrypted`] stores. A no-op if `new_key`
  /// equals the current key.
  ///
  /// # Crash safety
  ///
  /// At every point **either the old or the new key opens the store**, and
  /// [`Store::open_encrypted`] finishes or undoes the rekey:
  ///
  /// 1. `meta.rekey_in_progress = "staging"` (+ the new key's check value)
  ///    is committed under the old key.
  /// 2. Every blob is decrypted and re-sealed under the new key into a new
  ///    file. Crash here: opening with the old key clears the marker and GC
  ///    removes the new files.
  /// 3. One transaction records `old -> new` names in `rekey_blobs`, points
  ///    the reps at the new files and sets the marker to `"swapped"`.
  ///    The old files stay on disk.
  /// 4. `history.db.kcv` lists both keys, then `PRAGMA rekey` (atomic: one
  ///    WAL transaction rewriting every page) and a checkpoint (durable).
  ///    Crash before its commit: the old key opens the DB, sees `"swapped"`
  ///    with a different new-key check value and rolls back (reps -> old
  ///    names, new files removed). After it: only the new key opens the DB
  ///    and rolls forward (step 5).
  /// 5. Under the new key: delete the old files, recompute every hash, drop
  ///    `rekey_blobs` and the marker in one transaction, then the kcv file
  ///    lists only the new key. Idempotent if interrupted.
  ///
  /// So the caller (spool-keys) must keep slots for **both** keys until
  /// this returns `Ok`, and on the next unlock try both if the first gives
  /// [`Error::WrongKey`]. A blob that fails to decrypt aborts the rekey
  /// (rolled back; delete the item and retry).
  pub fn rekey(&mut self, new_key: &DataKey) -> Result<()> {
    let Mode::Encrypted(old) = &self.mode else {
      return Err(Error::Unsupported("rekey needs a store opened with open_encrypted"));
    };
    let new = Encrypted::new(&old.dir, new_key);
    if new.kcv == old.kcv {
      return Ok(());
    }
    self.recover_rekey()?;
    match self.rekey_steps(new, new_key) {
      Ok(()) => {
        tracing::info!("store: rekey complete");
        Ok(())
      }
      Err(e) => {
        #[cfg(test)]
        if self.fail_at.get().is_some() {
          // Simulated crash: leave everything as it is on disk.
          return Err(e);
        }
        if let Err(r) = self.recover_rekey() {
          tracing::warn!(error = %r, "store: rekey failed and immediate recovery failed; will retry at open");
        }
        Err(e)
      }
    }
  }

  fn rekey_steps(&mut self, new: Encrypted, new_key: &DataKey) -> Result<()> {
    // Start from a clean WAL: every frame is old-key, the DB file is current.
    checkpoint(&self.conn, "TRUNCATE")?;

    // 1. Marker.
    {
      let tx = self.conn.transaction()?;
      tx.execute("DELETE FROM rekey_blobs", [])?;
      set_meta(&tx, META_REKEY_STATE, STATE_STAGING)?;
      set_meta(&tx, META_REKEY_NEW_KCV, &new.kcv[..])?;
      tx.commit()?;
    }
    self.crash_point(RekeyStep::AfterMarker)?;

    // 2. Re-seal blobs under the new key.
    let Mode::Encrypted(old) = &self.mode else { unreachable!("checked by rekey") };
    let files: Vec<(i64, String)> = {
      let mut stmt = self.conn.prepare(
        "SELECT item_id, blob_file FROM reps WHERE blob_file IS NOT NULL ORDER BY rowid",
      )?;
      stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
    };
    let mut pending = blobs::Pending::for_enc(&new);
    let mut map = Vec::with_capacity(files.len());
    for (item, name) in files {
      let data = blobs::read_with(old, item, &name)?;
      let new_name = pending.write(&new, &data)?;
      map.push((name, new_name));
    }
    self.crash_point(RekeyStep::AfterBlobsWritten)?;

    // 3. Swap names.
    {
      let tx = self.conn.transaction()?;
      for (o, n) in &map {
        tx.execute("INSERT INTO rekey_blobs(old, new) VALUES (?1, ?2)", params![o, n])?;
        tx.execute("UPDATE reps SET blob_file = ?2 WHERE blob_file = ?1", params![o, n])?;
      }
      set_meta(&tx, META_REKEY_STATE, STATE_SWAPPED)?;
      tx.commit()?;
    }
    pending.disarm();
    self.crash_point(RekeyStep::AfterSwap)?;

    // 4. Re-encrypt the database.
    blobs::write_kcv(&old.dir, &[old.kcv, new.kcv])?;
    super::apply_subkey(&self.conn, "rekey", &new_key.derive(Label::Db))?;
    // From here on only the new key decrypts the DB.
    self.mode = Mode::Encrypted(new);
    self.crash_point(RekeyStep::AfterPragmaRekey)?;

    // 5. Roll forward.
    self.finish_rekey()
  }

  /// Complete a rekey whose `PRAGMA rekey` committed (the store is open
  /// under the new key and the marker says `"swapped"`).
  fn finish_rekey(&mut self) -> Result<()> {
    // The rekey commit must be durable before the old files go: if it were
    // lost, the old key would need them.
    checkpoint(&self.conn, "TRUNCATE")?;
    let Mode::Encrypted(enc) = &self.mode else { unreachable!("encrypted only") };
    let olds: Vec<String> = {
      let mut stmt = self.conn.prepare("SELECT old FROM rekey_blobs")?;
      stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    let dir = enc.blob_dir();
    for o in &olds {
      if blobs::is_blob_name(o) {
        blobs::remove_if_exists(&dir.join(o))?;
      }
    }
    self.crash_point(RekeyStep::AfterOldBlobsDeleted)?;

    // Recompute dedupe hashes under the new hash key. An unreadable item
    // keeps its old hash (it only affects dedupe) so recovery cannot wedge.
    let hash_key = Zeroizing::new(*enc.hash.expose());
    let ids: Vec<i64> = {
      let mut stmt = self.conn.prepare("SELECT id FROM items")?;
      stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    let mut hashes = Vec::with_capacity(ids.len());
    for id in ids {
      match self.get(ItemId(id)) {
        Ok(Some(item)) => {
          if let Some(canon) = item.reps.iter().find(|r| !r.is_alias()) {
            hashes.push((id, dedupe_hash(&hash_key, canon)));
          }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(id, error = %e, "store: rekey: item unreadable; hash kept"),
      }
    }
    let new_kcv = enc.kcv;
    let kcv_dir = enc.dir.clone();
    {
      let tx = self.conn.transaction()?;
      for (id, h) in &hashes {
        tx.execute("UPDATE items SET hash = ?2 WHERE id = ?1", params![id, &h[..]])?;
      }
      tx.execute("DELETE FROM rekey_blobs", [])?;
      tx.execute(
        "DELETE FROM meta WHERE key IN (?1, ?2)",
        params![META_REKEY_STATE, META_REKEY_NEW_KCV],
      )?;
      tx.commit()?;
    }
    blobs::write_kcv(&kcv_dir, &[new_kcv])?;
    Ok(())
  }

  /// Finish or undo an interrupted rekey (see [`Store::rekey`]). No-op
  /// without a marker.
  pub(crate) fn recover_rekey(&mut self) -> Result<()> {
    let Some(state) = get_meta::<String>(&self.conn, META_REKEY_STATE)? else {
      return Ok(());
    };
    let Mode::Encrypted(enc) = &self.mode else {
      return Err(Error::Corrupt("rekey marker in a store without a data key".into()));
    };
    match state.as_str() {
      STATE_STAGING => {
        tracing::warn!("store: undoing an interrupted rekey (blobs were being re-sealed)");
        let (kcv, dir) = (enc.kcv, enc.dir.clone());
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM rekey_blobs", [])?;
        tx.execute(
          "DELETE FROM meta WHERE key IN (?1, ?2)",
          params![META_REKEY_STATE, META_REKEY_NEW_KCV],
        )?;
        tx.commit()?;
        blobs::write_kcv(&dir, &[kcv])?;
        // The half-written new files are orphans now; GC removes them.
        self.gc_orphan_blobs()?;
        Ok(())
      }
      STATE_SWAPPED => {
        let new_kcv: Option<Vec<u8>> = get_meta(&self.conn, META_REKEY_NEW_KCV)?;
        if new_kcv.as_deref() == Some(&enc.kcv[..]) {
          tracing::warn!("store: finishing an interrupted rekey");
          return self.finish_rekey();
        }
        tracing::warn!("store: undoing an interrupted rekey (database not yet re-encrypted)");
        let (kcv, dir, blob_dir) = (enc.kcv, enc.dir.clone(), enc.blob_dir());
        let news: Vec<String> = {
          let mut stmt = self.conn.prepare("SELECT new FROM rekey_blobs")?;
          stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        {
          let tx = self.conn.transaction()?;
          tx.execute(
            "UPDATE reps SET blob_file = (SELECT old FROM rekey_blobs WHERE new = reps.blob_file)
             WHERE blob_file IN (SELECT new FROM rekey_blobs)",
            [],
          )?;
          tx.execute("DELETE FROM rekey_blobs", [])?;
          tx.execute(
            "DELETE FROM meta WHERE key IN (?1, ?2)",
            params![META_REKEY_STATE, META_REKEY_NEW_KCV],
          )?;
          tx.commit()?;
        }
        // The rollback must be durable before the new files go.
        checkpoint(&self.conn, "TRUNCATE")?;
        for n in &news {
          if blobs::is_blob_name(n) {
            blobs::remove_if_exists(&blob_dir.join(n))?;
          }
        }
        blobs::write_kcv(&dir, &[kcv])?;
        Ok(())
      }
      other => Err(Error::Corrupt(format!("unknown rekey state {other:?}"))),
    }
  }

  #[cfg(test)]
  fn crash_point(&self, step: RekeyStep) -> Result<()> {
    if self.fail_at.get() == Some(step) {
      return Err(Error::Io(std::io::Error::other("injected crash")));
    }
    Ok(())
  }

  #[cfg(not(test))]
  #[inline(always)]
  fn crash_point(&self, _step: RekeyStep) -> Result<()> {
    Ok(())
  }
}
