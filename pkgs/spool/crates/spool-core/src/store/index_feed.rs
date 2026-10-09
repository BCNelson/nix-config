//! Change feed for the daemon's full-text index (spool-search), plus the
//! list queries the picker needs (summaries in a given order, with tags).
//!
//! The index follows the store by `change_seq`: every insert, bump, touch,
//! delete and retention sweep allocates one, so "everything after seq `n`"
//! ([`Store::items_for_index`] + [`Store::tombstones_after`]) is exactly what
//! an index that covers `n` is missing. [`Store::max_change_seq`] is the
//! counter itself, so "nothing changed" is one keyed `meta` read.
//!
//! The text fed for an item is its canonical text representation (first of
//! [`TEXT_MIMES`] it offers, aliases resolved), cut to the caller's byte cap;
//! items without text feed their preview (e.g. `[image/png 1234 bytes]`).
//! Large text reps that live in blob files are decrypted only as far as the
//! cap. Nothing here logs content.

use std::fs::File;

use rusqlite::{OptionalExtension, params};

use super::{META_CHANGE_SEQ, Mode, Store, blobs, from_millis, get_meta, set_meta};
use crate::item::{ItemFlags, ItemId, ItemSummary, Selection, TEXT_MIMES};
use crate::{Error, Result};

/// `meta` key holding the database's random identity (32 lowercase hex
/// chars). An on-disk search index records it, so an index built from
/// another database (restored, replaced) is detected and rebuilt.
pub const META_DB_UUID: &str = "db_uuid";

/// One item as the search index sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexItem {
  pub id: ItemId,
  pub change_seq: i64,
  /// Canonical text (cut to the requested cap), or the preview for items
  /// without a text representation.
  pub text: String,
  /// Offered mimes, stored order.
  pub mimes: Vec<String>,
  pub source_app: Option<String>,
  pub tags: Vec<String>,
  /// ms since the Unix epoch.
  pub created_at: i64,
  /// ms since the Unix epoch (`created_at` if never re-used).
  pub last_used_at: i64,
  pub pinned: bool,
}

/// A deleted item (`tombstones` row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexTombstone {
  pub id: ItemId,
  pub change_seq: i64,
}

/// A list row plus the item's tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedSummary {
  pub summary: ItemSummary,
  pub tags: Vec<String>,
}

/// Columns read for summaries (`ItemSummary` without mimes).
const SUMMARY_COLS: &str =
  "id, created_at, last_used_at, selection, source_app, flags, preview, total_size";

impl Store {
  /// This database's identity (see [`META_DB_UUID`]); generated and
  /// persisted on first call.
  pub fn db_uuid(&mut self) -> Result<String> {
    let tx = self.conn.transaction()?;
    let existing: Option<String> = get_meta(&tx, META_DB_UUID)?;
    let id = match existing {
      Some(s) if s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()) => s,
      _ => {
        let r: [u8; 16] = rand::random();
        let s: String = r.iter().map(|b| format!("{b:02x}")).collect();
        set_meta(&tx, META_DB_UUID, &s)?;
        s
      }
    };
    tx.commit()?;
    Ok(id)
  }

  /// The highest `change_seq` allocated so far (0 for a fresh store).
  pub fn max_change_seq(&self) -> Result<i64> {
    if let Some(n) = get_meta::<i64>(&self.conn, META_CHANGE_SEQ)? {
      return Ok(n);
    }
    Ok(self.conn.query_row(
      "SELECT max(coalesce((SELECT max(change_seq) FROM items), 0),
                  coalesce((SELECT max(change_seq) FROM tombstones), 0))",
      [],
      |r| r.get(0),
    )?)
  }

  /// Live items with `after < change_seq <= upto`, ascending by
  /// `change_seq`, at most `limit`. `text` is cut to `max_text` bytes (on a
  /// char boundary). A blob that cannot be read is logged and the item is
  /// fed with its preview instead (it is still listed and deletable).
  pub fn items_for_index(
    &self,
    after: i64,
    upto: i64,
    limit: usize,
    max_text: usize,
  ) -> Result<Vec<IndexItem>> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    type Row = (i64, i64, Option<i64>, Option<i64>, Option<String>, Option<i64>, Option<String>);
    let rows: Vec<Row> = {
      let mut stmt = self.conn.prepare_cached(
        "SELECT id, change_seq, created_at, last_used_at, source_app, flags, preview
         FROM items WHERE change_seq > ?1 AND change_seq <= ?2
         ORDER BY change_seq ASC LIMIT ?3",
      )?;
      stmt
        .query_map(params![after, upto, limit], |r| {
          Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
        })?
        .collect::<Result<_, _>>()?
    };
    rows
      .into_iter()
      .map(|(id, change_seq, created, used, source_app, flags, preview)| {
        let reps = self.rep_rows(id)?;
        let mimes = reps.iter().map(|r| r.mime.clone()).collect();
        let text = match self.text_of(id, &reps, max_text) {
          Ok(Some(t)) => t,
          Ok(None) => cut(preview.unwrap_or_default(), max_text),
          Err(e) => {
            tracing::warn!(id, error = %e, "index feed: text unreadable; indexing the preview");
            cut(preview.unwrap_or_default(), max_text)
          }
        };
        let created_at = created.unwrap_or(0);
        Ok(IndexItem {
          id: ItemId(id),
          change_seq,
          text,
          mimes,
          source_app,
          tags: self.tags_of(id)?,
          created_at,
          last_used_at: used.unwrap_or(created_at),
          pinned: ItemFlags::from_bits_retain(flags.unwrap_or(0) as u32)
            .contains(ItemFlags::PINNED),
        })
      })
      .collect()
  }

  /// Tombstones with `after < change_seq <= upto`, ascending.
  pub fn tombstones_after(&self, after: i64, upto: i64) -> Result<Vec<IndexTombstone>> {
    let mut stmt = self.conn.prepare_cached(
      "SELECT change_seq, item_id FROM tombstones
       WHERE change_seq > ?1 AND change_seq <= ?2 ORDER BY change_seq ASC",
    )?;
    let rows = stmt.query_map(params![after, upto], |r| {
      Ok(IndexTombstone { change_seq: r.get(0)?, id: ItemId(r.get(1)?) })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
  }

  /// Summaries (with tags) of `ids`, in the given order; ids that no
  /// longer exist are skipped. Never reads blob files.
  pub fn summaries(&self, ids: &[ItemId]) -> Result<Vec<TaggedSummary>> {
    let mut stmt =
      self.conn.prepare_cached(&format!("SELECT {SUMMARY_COLS} FROM items WHERE id = ?1"))?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
      let Some(raw) = stmt.query_row(params![id.0], RawSummary::from_row).optional()? else {
        continue;
      };
      out.push(self.tagged(raw)?);
    }
    Ok(out)
  }

  /// [`Store::recent`] order (pinned first, then newest `last_used_at`),
  /// skipping `offset`, at most `limit`, with tags.
  pub fn recent_page(&self, offset: usize, limit: usize) -> Result<Vec<TaggedSummary>> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let offset = i64::try_from(offset).unwrap_or(i64::MAX);
    let raws: Vec<RawSummary> = {
      let mut stmt = self.conn.prepare_cached(&format!(
        "SELECT {SUMMARY_COLS} FROM items
         ORDER BY (flags & 1) DESC, last_used_at DESC, id DESC LIMIT ?1 OFFSET ?2"
      ))?;
      stmt.query_map(params![limit, offset], RawSummary::from_row)?.collect::<Result<_, _>>()?
    };
    raws.into_iter().map(|r| self.tagged(r)).collect()
  }

  fn tagged(&self, raw: RawSummary) -> Result<TaggedSummary> {
    let mimes = self.rep_rows(raw.id)?.into_iter().map(|r| r.mime).collect();
    let tags = self.tags_of(raw.id)?;
    let selection = raw
      .selection
      .as_deref()
      .and_then(Selection::parse)
      .ok_or_else(|| Error::Corrupt(format!("item {}: bad selection", raw.id)))?;
    let created_at = from_millis(raw.created_at.unwrap_or(0));
    Ok(TaggedSummary {
      summary: ItemSummary {
        id: ItemId(raw.id),
        created_at,
        last_used_at: raw.last_used_at.map(from_millis).unwrap_or(created_at),
        selection,
        source_app: raw.source_app,
        flags: ItemFlags::from_bits_retain(raw.flags.unwrap_or(0) as u32),
        preview: raw.preview,
        total_size: raw.total_size.unwrap_or(0).max(0) as u64,
        mimes,
      },
      tags,
    })
  }

  fn tags_of(&self, id: i64) -> Result<Vec<String>> {
    let mut stmt = self.conn.prepare_cached(
      "SELECT tag FROM tags WHERE item_id = ?1 AND tag IS NOT NULL ORDER BY rowid",
    )?;
    let rows = stmt.query_map(params![id], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
  }

  fn rep_rows(&self, id: i64) -> Result<Vec<RepRow>> {
    let mut stmt = self.conn.prepare_cached(
      "SELECT mime, alias_of, inline, blob_file FROM reps WHERE item_id = ?1 ORDER BY rowid",
    )?;
    let rows = stmt.query_map(params![id], |r| {
      Ok(RepRow { mime: r.get(0)?, alias_of: r.get(1)?, inline: r.get(2)?, blob_file: r.get(3)? })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
  }

  /// The canonical text of an item, cut to `max` bytes; `None` without a
  /// text representation.
  fn text_of(&self, id: i64, reps: &[RepRow], max: usize) -> Result<Option<String>> {
    let Some(rep) = TEXT_MIMES.iter().find_map(|m| reps.iter().find(|r| r.mime == *m)) else {
      return Ok(None);
    };
    let rep = match &rep.alias_of {
      None => rep,
      Some(canon) => match reps.iter().find(|r| &r.mime == canon && r.alias_of.is_none()) {
        Some(r) => r,
        None => return Ok(None),
      },
    };
    let bytes: Vec<u8> = match (&rep.inline, &rep.blob_file) {
      (_, Some(file)) => self.blob_prefix(id, file, max)?,
      (Some(b), None) => b[..b.len().min(max)].to_vec(),
      (None, None) => Vec::new(),
    };
    Ok(Some(utf8_prefix(&bytes)))
  }

  /// The first `max` plaintext bytes of blob `file` (only the chunks that
  /// cover them are decrypted).
  fn blob_prefix(&self, id: i64, file: &str, max: usize) -> Result<Vec<u8>> {
    let Mode::Encrypted(enc) = &self.mode else {
      return Err(Error::Corrupt(format!(
        "item {id}: references a blob file but this store has no blob key"
      )));
    };
    if !blobs::is_blob_name(file) {
      return Err(Error::Corrupt(format!("item {id}: invalid blob file name")));
    }
    let blob_err = |source| Error::Blob { item: id, file: file.to_string(), source };
    let f =
      File::open(enc.blob_dir().join(file)).map_err(|e| blob_err(spool_crypto::Error::Io(e)))?;
    let mut r =
      spool_crypto::ChunkedReader::open(&enc.blob, file.as_bytes(), f).map_err(blob_err)?;
    let end = r.len().min(max as u64);
    Ok(r.read_range(0..end).map_err(blob_err)?.to_vec())
  }
}

struct RepRow {
  mime: String,
  alias_of: Option<String>,
  inline: Option<Vec<u8>>,
  blob_file: Option<String>,
}

struct RawSummary {
  id: i64,
  created_at: Option<i64>,
  last_used_at: Option<i64>,
  selection: Option<String>,
  source_app: Option<String>,
  flags: Option<i64>,
  preview: Option<String>,
  total_size: Option<i64>,
}

impl RawSummary {
  fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
    Ok(Self {
      id: r.get(0)?,
      created_at: r.get(1)?,
      last_used_at: r.get(2)?,
      selection: r.get(3)?,
      source_app: r.get(4)?,
      flags: r.get(5)?,
      preview: r.get(6)?,
      total_size: r.get(7)?,
    })
  }
}

/// UTF-8 text of `b`; an incomplete sequence at the very end (from the
/// byte cap) is dropped, other invalid bytes become U+FFFD.
fn utf8_prefix(b: &[u8]) -> String {
  match std::str::from_utf8(b) {
    Ok(s) => s.to_owned(),
    Err(e) if e.error_len().is_none() => {
      String::from_utf8_lossy(&b[..e.valid_up_to()]).into_owned()
    }
    Err(_) => String::from_utf8_lossy(b).into_owned(),
  }
}

fn cut(mut s: String, max: usize) -> String {
  if s.len() > max {
    let mut end = max;
    while !s.is_char_boundary(end) {
      end -= 1;
    }
    s.truncate(end);
  }
  s
}

#[cfg(test)]
mod tests {
  use std::time::{Duration, SystemTime};

  use bytes::Bytes;
  use spool_crypto::DataKey;

  use super::*;
  use crate::config::RetentionLimits;
  use crate::item::{NewItem, Representation};

  const UTF8: &str = "text/plain;charset=utf-8";

  fn item(at: u64, reps: Vec<Representation>, preview: &str) -> NewItem {
    NewItem {
      selection: Selection::Clipboard,
      source_app: Some("org.example.App".into()),
      created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(at),
      flags: ItemFlags::empty(),
      hash: *blake3::hash(format!("{at}{preview}").as_bytes()).as_bytes(),
      preview: Some(preview.into()),
      reps,
    }
  }

  fn text(at: u64, s: &str) -> NewItem {
    item(
      at,
      vec![
        Representation::new(UTF8, Bytes::from(s.to_owned())),
        Representation::alias("TEXT", UTF8),
      ],
      s,
    )
  }

  #[test]
  fn feed_follows_change_seq() {
    let mut s = Store::open_in_memory().unwrap();
    assert_eq!(s.max_change_seq().unwrap(), 0);
    let a = s.insert(text(1, "alpha one")).unwrap().id();
    let b = s.insert(text(2, "beta two")).unwrap().id();
    let png = s
      .insert(item(
        3,
        vec![Representation::new("image/png", &b"\x89PNG"[..])],
        "[image/png 4 bytes]",
      ))
      .unwrap()
      .id();
    let max = s.max_change_seq().unwrap();
    assert_eq!(max, 3);

    let all = s.items_for_index(0, max, 100, 1 << 20).unwrap();
    assert_eq!(all.iter().map(|i| i.id).collect::<Vec<_>>(), [a, b, png]);
    assert_eq!(all[0].text, "alpha one");
    assert_eq!(all[0].mimes, [UTF8, "TEXT"]);
    assert_eq!(all[0].source_app.as_deref(), Some("org.example.App"));
    assert_eq!(all[2].text, "[image/png 4 bytes]");
    assert_eq!(all[2].mimes, ["image/png"]);
    // Paging and the cap.
    assert_eq!(s.items_for_index(0, max, 2, 1 << 20).unwrap().len(), 2);
    assert_eq!(s.items_for_index(1, max, 100, 3).unwrap()[0].text, "bet");

    // Touch moves `a` to the end; delete leaves a tombstone.
    s.touch(a, SystemTime::UNIX_EPOCH + Duration::from_secs(9)).unwrap();
    assert!(s.delete(b).unwrap());
    let max2 = s.max_change_seq().unwrap();
    let changed = s.items_for_index(max, max2, 100, 1 << 20).unwrap();
    assert_eq!(changed.iter().map(|i| i.id).collect::<Vec<_>>(), [a]);
    assert_eq!(changed[0].last_used_at, 9000);
    assert_eq!(changed[0].created_at, 1000);
    let tombs = s.tombstones_after(max, max2).unwrap();
    assert_eq!(tombs, [IndexTombstone { id: b, change_seq: max2 }]);
    assert!(s.tombstones_after(max2, max2).unwrap().is_empty());

    // Retention tombstones too.
    let limits = RetentionLimits { max_age: Duration::from_secs(1), max_items: 0 };
    assert_eq!(s.retention_sweep(SystemTime::now(), limits).unwrap(), 2);
    let max3 = s.max_change_seq().unwrap();
    let ids: Vec<ItemId> = s.tombstones_after(max2, max3).unwrap().iter().map(|t| t.id).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&a) && ids.contains(&png));
  }

  #[test]
  fn utf8_cut_and_db_uuid() {
    assert_eq!(utf8_prefix("aé".as_bytes()), "aé");
    assert_eq!(utf8_prefix(&"aé".as_bytes()[..2]), "a");
    assert_eq!(cut("aéb".into(), 2), "a");
    let mut s = Store::open_in_memory().unwrap();
    let u = s.db_uuid().unwrap();
    assert_eq!(u.len(), 32);
    assert_eq!(s.db_uuid().unwrap(), u);
    assert_ne!(Store::open_in_memory().unwrap().db_uuid().unwrap(), u);
  }

  #[test]
  fn summaries_and_recent_page() {
    let mut s = Store::open_in_memory().unwrap();
    let a = s.insert(text(1, "a")).unwrap().id();
    let b = s.insert(text(2, "b")).unwrap().id();
    s.conn().execute("INSERT INTO tags(item_id, tag) VALUES (?1, 'work')", params![a.0]).unwrap();
    let got = s.summaries(&[b, ItemId(999), a]).unwrap();
    assert_eq!(got.iter().map(|t| t.summary.id).collect::<Vec<_>>(), [b, a]);
    assert_eq!(got[1].tags, ["work"]);
    assert_eq!(got[1].summary.mimes, [UTF8, "TEXT"]);
    let page: Vec<ItemId> = s.recent_page(0, 10).unwrap().iter().map(|t| t.summary.id).collect();
    assert_eq!(page, [b, a]);
    let page: Vec<ItemId> = s.recent_page(1, 10).unwrap().iter().map(|t| t.summary.id).collect();
    assert_eq!(page, [a]);
  }

  #[test]
  fn large_text_in_a_blob_is_fed_up_to_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let key = DataKey::generate();
    let mut s = Store::open_encrypted(dir.path(), &key).unwrap();
    let mut big = String::from("blobmarker ");
    while big.len() <= super::super::INLINE_MAX + 4096 {
      big.push_str("filler words é ");
    }
    let id = s.insert(text(1, &big)).unwrap().id();
    let blob_files: i64 = s
      .conn()
      .query_row("SELECT count(*) FROM reps WHERE blob_file IS NOT NULL", [], |r| r.get(0))
      .unwrap();
    assert_eq!(blob_files, 1);
    let max = s.max_change_seq().unwrap();
    let got = s.items_for_index(0, max, 10, 1000).unwrap();
    assert_eq!(got[0].id, id);
    assert!(got[0].text.starts_with("blobmarker filler"));
    assert!(got[0].text.len() <= 1000 && got[0].text.len() >= 997);
    let full = s.items_for_index(0, max, 10, usize::MAX).unwrap();
    assert_eq!(full[0].text, big);

    // A missing blob file: the item is still fed, with its preview.
    for e in std::fs::read_dir(dir.path().join(super::super::BLOB_DIR_NAME)).unwrap() {
      std::fs::remove_file(e.unwrap().path()).unwrap();
    }
    let got = s.items_for_index(0, max, 10, 1 << 20).unwrap();
    assert_eq!(got[0].text, big[..got[0].text.len()]);
  }
}
