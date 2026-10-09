//! SQLite (SQLCipher) history store.
//!
//! The store is synchronous and not `Sync`; the daemon keeps it on one
//! thread/task (e.g. behind `spawn_blocking` or a dedicated thread).
//!
//! # Kinds of store
//!
//! | Constructor | Kind | Hash key | Large reps (> [`INLINE_MAX`]) |
//! | --- | --- | --- | --- |
//! | [`Store::open_encrypted`] | [`StoreKind::Encrypted`] | `Label::Hash` sub-key | chunk-encrypted files in `blobs/` |
//! | [`Store::open_session`] | [`StoreKind::Session`] | session `Label::Hash` sub-key | inline (in memory) |
//! | [`Store::open`], [`Store::open_in_memory`] | [`StoreKind::Plain`] | random `meta.hash_key` | inline |
//!
//! `Store::open(path, None)` is the plaintext development/M1 path (the
//! daemon uses it until key slots are wired). `Store::open(path, Some(raw))`
//! keeps the M1 raw-key behaviour (SQLCipher keyed with `raw` directly) but
//! is otherwise a plain store: it has no blob key, so large reps stay
//! inline, and the hash key is the random `meta` key. Production uses
//! [`Store::open_encrypted`].
//!
//! # Encrypted layout (`dir`, mode 0700, owned by the caller)
//!
//! - `history.db` (+ `-wal`, `-shm`): SQLCipher, raw key = `Label::Db`
//!   sub-key (`PRAGMA key = "x'<hex>'"` is the first statement). WAL pages
//!   are encrypted like the main file; the `-shm` wal-index holds no content.
//! - `blobs/<32 hex>.bin`: representations larger than [`INLINE_MAX`], sealed
//!   with [`spool_crypto::seal_file`] under the `Label::Blob` sub-key with
//!   the file name as the logical path (so files cannot be swapped).
//! - `history.db.kcv`: key check value(s), `BLAKE3-derive_key` of the Db
//!   sub-key. Only consulted when the database will not decrypt, to tell
//!   [`Error::WrongKey`] from a damaged database ([`Error::Corrupt`]).
//!
//! # Pragmas
//!
//! - `journal_mode=WAL`, `synchronous=NORMAL`: a per-copy fsync made copy
//!   latency spike to hundreds of ms on a busy NVMe. In WAL mode NORMAL can
//!   lose the last few commits on power loss but never corrupts. Operations
//!   that delete blob files first force a checkpoint so the commit that
//!   dropped the reference is durable before the file goes.
//! - `temp_store=MEMORY`: temp b-trees (sorts, temp tables) never hit disk.
//!   SQLCipher does not encrypt temp files, so this is required.
//! - `secure_delete` is **not** set: with SQLCipher, freed pages hold only
//!   ciphertext, so zeroing them buys nothing. (In plaintext dev mode freed
//!   pages may retain old content; plaintext mode is not for real use.)
//! - `cipher_memory_security` per [`StoreOptions`] (default on). SQLCipher
//!   treats it as **process-global** and sticky: once any connection turns it
//!   on, `OFF` is ignored for the rest of the process.
//!
//! # Session store
//!
//! [`Store::open_session`] is an in-memory database used before the user
//! unlocks. SQLCipher does not run in-memory pages through its codec, so the
//! session key's real job is to key the dedupe hash; the content lives in the
//! daemon heap like any clipboard data (and `cipher_memory_security` wipes
//! SQLite's freed memory). [`Store::merge_from_session`] copies it into the
//! persistent store after unlock.
//!
//! # Crash safety
//!
//! - Inserts write blob files (fsynced, atomic rename) before the DB commit;
//!   a crash in between leaves an orphan file that [`Store::gc_orphan_blobs`]
//!   (run at open) removes.
//! - `delete` / `retention_sweep` remove blob files only after the DB commit
//!   and a WAL checkpoint. A crash in between leaves orphans (GC'd).
//! - Merge runs in one transaction; a failure leaves the persistent store
//!   untouched (written blob files are removed, or GC'd after a crash). The
//!   session is not modified: the caller drops it only after `Ok`.
//! - Rekey: see [`Store::rekey`].
//!
//! # Indexes and `change_seq`
//!
//! Every hot operation is logarithmic in the item count:
//!
//! - `items_sel_recent (selection, last_used_at DESC, id DESC)`: the insert
//!   dedupe lookup and [`Store::latest`].
//! - `items_pin_recent ((flags & 1) DESC, last_used_at DESC, id DESC)`:
//!   [`Store::recent`] (pinned first) and the retention sweep's
//!   oldest-unpinned scan.
//! - `items_change_seq`, `tombstones` PK: the `max(change_seq)` floor.
//! - `tags_item`: the `ON DELETE CASCADE` lookup into `tags`.
//!
//! `change_seq`s are allocated from the `meta.change_seq` counter (one
//! keyed read + write). The invariant "counter >= every `change_seq` in
//! `items`/`tombstones`" is restored once at open (a lost or reset counter
//! is raised to that floor), not on every allocation. Bulk operations
//! (the retention sweep) reserve a contiguous range in one update.

mod blobs;
pub mod index_feed;
mod merge;
mod rekey;

#[cfg(test)]
mod m2_tests;

#[cfg(test)]
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use bytes::Bytes;
use rusqlite::{Connection, OptionalExtension, params};
use rusqlite_migration::{M, Migrations};
use spool_crypto::{DataKey, Label, SubKey, Zeroizing};

pub use merge::MergeReport;

use crate::config::RetentionLimits;
use crate::item::{
  Item, ItemFlags, ItemHash, ItemId, ItemSummary, NewItem, Representation, Selection, hash_prefix,
};
use crate::{Error, Result};

/// Schema migrations. Append only; never edit a released entry.
pub static MIGRATIONS: LazyLock<Migrations<'static>> = LazyLock::new(|| {
  Migrations::new(vec![
    M::up(
      r#"
CREATE TABLE meta(
  key   TEXT PRIMARY KEY,
  value BLOB
);
CREATE TABLE items(
  id           INTEGER PRIMARY KEY,
  change_seq   INTEGER NOT NULL,
  created_at   INTEGER,
  last_used_at INTEGER,
  selection    TEXT CHECK(selection IN ('clipboard','primary')),
  source_app   TEXT,
  flags        INTEGER,
  hash         BLOB NOT NULL,
  preview      TEXT,
  total_size   INTEGER
);
CREATE TABLE reps(
  item_id   INTEGER REFERENCES items ON DELETE CASCADE,
  mime      TEXT,
  alias_of  TEXT,
  inline    BLOB,
  blob_file TEXT,
  size      INTEGER,
  PRIMARY KEY(item_id, mime)
);
CREATE TABLE tags(
  item_id INTEGER REFERENCES items ON DELETE CASCADE,
  tag     TEXT
);
CREATE TABLE tombstones(
  change_seq INTEGER PRIMARY KEY,
  item_id    INTEGER
);
CREATE INDEX items_recent ON items(flags, last_used_at DESC);
"#,
    ),
    // M2: blob files and rekey bookkeeping.
    M::up(
      r#"
CREATE INDEX reps_blob_file ON reps(blob_file) WHERE blob_file IS NOT NULL;
CREATE TABLE rekey_blobs(
  old TEXT PRIMARY KEY,
  new TEXT NOT NULL UNIQUE
);
"#,
    ),
    // Indexes for the hot paths (see the "Indexes" module docs). The
    // expression `(flags & 1)` must match `PINNED_EXPR` textually.
    M::up(
      r#"
CREATE INDEX items_sel_recent ON items(selection, last_used_at DESC, id DESC);
CREATE INDEX items_change_seq ON items(change_seq);
CREATE INDEX items_pin_recent ON items((flags & 1) DESC, last_used_at DESC, id DESC);
CREATE INDEX tags_item ON tags(item_id);
DROP INDEX items_recent;
"#,
    ),
  ])
});

/// `meta` key holding the 32-byte dedupe hash key (plain stores only).
pub const META_HASH_KEY: &str = "hash_key";
/// `meta` key holding the last allocated `change_seq` (i64).
pub const META_CHANGE_SEQ: &str = "change_seq";
/// `meta` key present while a [`Store::rekey`] is in flight
/// (`"staging"` or `"swapped"`).
pub const META_REKEY_STATE: &str = "rekey_in_progress";
/// `meta` key holding the new key's check value during a rekey.
pub const META_REKEY_NEW_KCV: &str = "rekey_new_kcv";

/// Database file name inside the state directory.
pub const DB_FILE_NAME: &str = "history.db";
/// Blob directory name inside the state directory.
pub const BLOB_DIR_NAME: &str = "blobs";
/// Lock file inside the state directory. `spoold` holds a non-blocking
/// exclusive `flock` on it for its whole lifetime and `spool-keyctl` for
/// every mutating command, so the two never use one state directory at
/// the same time (whatever socket path each of them was started with).
pub const STATE_LOCK_FILE_NAME: &str = "state.lock";
/// Key check value file name inside the state directory.
pub const KCV_FILE_NAME: &str = "history.db.kcv";
/// Representations larger than this many bytes are stored as encrypted
/// blob files (encrypted stores only).
pub const INLINE_MAX: usize = 1024 * 1024;

/// Timestamps are stored as integer **milliseconds** since the Unix epoch.
pub fn to_millis(t: SystemTime) -> i64 {
  t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub fn from_millis(ms: i64) -> SystemTime {
  SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms.max(0) as u64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
  /// A new row was created.
  Inserted(ItemId),
  /// The hash equals the newest item of the same selection; its
  /// `last_used_at` (and `change_seq`) were bumped instead.
  Bumped(ItemId),
}

impl InsertOutcome {
  pub fn id(self) -> ItemId {
    match self {
      InsertOutcome::Inserted(id) | InsertOutcome::Bumped(id) => id,
    }
  }
}

/// Which constructor a [`Store`] came from (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
  Plain,
  Encrypted,
  Session,
}

/// Options for the keyed constructors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreOptions {
  /// `PRAGMA cipher_memory_security`: SQLCipher wipes memory it frees.
  /// Default `true`. Process-global and sticky once enabled.
  pub cipher_memory_security: bool,
}

impl Default for StoreOptions {
  fn default() -> Self {
    Self { cipher_memory_security: true }
  }
}

/// Keys of an encrypted store, all derived from its [`DataKey`].
pub(crate) struct Encrypted {
  dir: PathBuf,
  blob: SubKey,
  hash: SubKey,
  kcv: [u8; 32],
}

impl Encrypted {
  fn new(dir: &Path, key: &DataKey) -> Self {
    Self {
      dir: dir.to_path_buf(),
      blob: key.derive(Label::Blob),
      hash: key.derive(Label::Hash),
      kcv: blobs::kcv_of(&key.derive(Label::Db)),
    }
  }

  fn blob_dir(&self) -> PathBuf {
    self.dir.join(BLOB_DIR_NAME)
  }
}

pub(crate) enum Mode {
  Plain,
  Encrypted(Encrypted),
  Session { hash: SubKey },
}

pub struct Store {
  conn: Connection,
  mode: Mode,
  #[cfg(test)]
  fail_at: Cell<Option<rekey::RekeyStep>>,
}

impl std::fmt::Debug for Store {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Store").field("kind", &self.kind()).finish_non_exhaustive()
  }
}

/// `PRAGMA key = "x'<hex>'"` from a hex string (kept in zeroizing buffers;
/// SQLite's own copy of the statement text is outside our control).
fn apply_key_hex(conn: &Connection, pragma: &str, hex: &str) -> Result<()> {
  let sql = Zeroizing::new(format!("PRAGMA {pragma} = \"x'{hex}'\";"));
  conn.execute_batch(&sql)?;
  Ok(())
}

fn apply_subkey(conn: &Connection, pragma: &str, sub: &SubKey) -> Result<()> {
  apply_key_hex(conn, pragma, &sub.sqlcipher_pragma_hex())
}

fn set_memory_security(conn: &Connection, on: bool) -> Result<()> {
  conn.execute_batch(if on {
    "PRAGMA cipher_memory_security = ON;"
  } else {
    "PRAGMA cipher_memory_security = OFF;"
  })?;
  Ok(())
}

/// The first real read: fails with `SQLITE_NOTADB` on a wrong/missing key.
fn verify_readable(conn: &Connection) -> rusqlite::Result<()> {
  conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)).map(|_| ())
}

fn is_notadb(e: &rusqlite::Error) -> bool {
  e.sqlite_error_code() == Some(rusqlite::ErrorCode::NotADatabase)
}

/// Pragmas for file-backed databases (see the module docs).
fn file_pragmas(conn: &Connection) -> Result<()> {
  conn.pragma_update(None, "journal_mode", "WAL")?;
  conn.pragma_update(None, "synchronous", "NORMAL")?;
  conn.pragma_update(None, "temp_store", "MEMORY")?;
  conn.pragma_update(None, "foreign_keys", "ON")?;
  Ok(())
}

/// Run a WAL checkpoint (`PASSIVE`/`TRUNCATE`). It syncs the WAL, so every
/// commit so far is durable afterwards. No-op for non-WAL databases.
fn checkpoint(conn: &Connection, kind: &str) -> Result<()> {
  let busy: i64 = conn.query_row(&format!("PRAGMA wal_checkpoint({kind})"), [], |r| r.get(0))?;
  if busy != 0 {
    return Err(Error::Io(std::io::Error::other("WAL checkpoint could not complete")));
  }
  Ok(())
}

fn get_meta<T: rusqlite::types::FromSql>(conn: &Connection, key: &str) -> Result<Option<T>> {
  Ok(
    conn
      .prepare_cached("SELECT value FROM meta WHERE key = ?1")?
      .query_row(params![key], |r| r.get(0))
      .optional()?,
  )
}

fn set_meta(conn: &Connection, key: &str, value: impl rusqlite::ToSql) -> Result<()> {
  conn
    .prepare_cached(
      "INSERT INTO meta(key, value) VALUES (?1, ?2)
       ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )?
    .execute(params![key, value])?;
  Ok(())
}

/// Column values for a new `items` row.
pub(crate) struct RowData<'a> {
  selection: Selection,
  source_app: Option<&'a str>,
  created_at: i64,
  last_used_at: i64,
  flags: ItemFlags,
  hash: &'a ItemHash,
  preview: Option<&'a str>,
  total_size: i64,
}

impl Store {
  fn from_parts(conn: Connection, mode: Mode) -> Self {
    Self {
      conn,
      mode,
      #[cfg(test)]
      fail_at: Cell::new(None),
    }
  }

  /// Open (creating if needed) the database at `path` as a **plain** store
  /// (development / M1; see the module docs). When `key` is `Some`,
  /// `PRAGMA key = "x'<hex>'"` is issued before anything else. Then sets
  /// `journal_mode=WAL`, `synchronous=NORMAL`, `temp_store=MEMORY`,
  /// `foreign_keys=ON` and runs migrations. The caller is responsible for
  /// the parent directory's permissions (0700); the daemon runs with umask
  /// 077.
  pub fn open(path: &Path, key: Option<&[u8; 32]>) -> Result<Self> {
    let conn = Connection::open(path)?;
    if let Some(k) = key {
      let mut hex = Zeroizing::new(String::with_capacity(64));
      for b in k {
        hex.push_str(&format!("{b:02x}"));
      }
      apply_key_hex(&conn, "key", &hex)?;
    }
    match verify_readable(&conn) {
      Ok(()) => {}
      Err(e) if is_notadb(&e) => return Err(Error::WrongKey),
      Err(e) => return Err(e.into()),
    }
    file_pragmas(&conn)?;
    Self::finish_init(conn, Mode::Plain)
  }

  /// In-memory plain database (tests). WAL is not applicable and is skipped.
  pub fn open_in_memory() -> Result<Self> {
    let conn = Connection::open_in_memory()?;
    Self::finish_init(conn, Mode::Plain)
  }

  /// Open (creating if needed) the encrypted store in `dir` with default
  /// [`StoreOptions`]. See [`Store::open_encrypted_with`].
  pub fn open_encrypted(dir: &Path, key: &DataKey) -> Result<Self> {
    Self::open_encrypted_with(dir, key, &StoreOptions::default())
  }

  /// Open (creating if needed) the encrypted store in `dir`
  /// (`history.db`, `blobs/`, `history.db.kcv`). `dir` must exist; the
  /// caller owns its permissions (0700).
  ///
  /// `PRAGMA key` (the `Label::Db` sub-key) is the first statement, then a
  /// read of `sqlite_master` verifies it:
  /// - [`Error::WrongKey`]: the database does not decrypt with this key.
  /// - [`Error::NotEncrypted`]: the file is a plaintext SQLite database.
  /// - [`Error::Corrupt`]: the file length is impossible, or the key matches
  ///   the stored key check value but the database still fails to decrypt.
  ///
  /// Then: pragmas, migrations, recovery of an interrupted [`Store::rekey`],
  /// key check value refresh, and [`Store::gc_orphan_blobs`].
  pub fn open_encrypted_with(dir: &Path, key: &DataKey, opts: &StoreOptions) -> Result<Self> {
    let db_path = dir.join(DB_FILE_NAME);
    let enc = Encrypted::new(dir, key);
    let conn = Connection::open(&db_path)?;
    apply_subkey(&conn, "key", &key.derive(Label::Db))?;
    set_memory_security(&conn, opts.cipher_memory_security)?;
    if let Err(e) = verify_readable(&conn) {
      drop(conn);
      return Err(if is_notadb(&e) {
        blobs::classify_unreadable(&db_path, &dir.join(KCV_FILE_NAME), &enc.kcv)
      } else {
        e.into()
      });
    }
    blobs::ensure_blob_dir(&enc.blob_dir())?;
    file_pragmas(&conn)?;
    let mut store = Self::finish_init(conn, Mode::Encrypted(enc))?;
    store.recover_rekey()?;
    store.ensure_kcv()?;
    let removed = store.gc_orphan_blobs()?;
    if removed > 0 {
      tracing::info!(removed, "store: removed orphan blob files");
    }
    Ok(store)
  }

  /// In-memory session store keyed with a session [`DataKey`] (used before
  /// unlock). Same schema; large reps stay inline; the dedupe hash key is
  /// the session key's `Label::Hash` sub-key. See the module docs for what
  /// the key does and does not protect.
  pub fn open_session(key: &DataKey) -> Result<Self> {
    Self::open_session_with(key, &StoreOptions::default())
  }

  /// [`Store::open_session`] with explicit options.
  pub fn open_session_with(key: &DataKey, opts: &StoreOptions) -> Result<Self> {
    let conn = Connection::open_in_memory()?;
    apply_subkey(&conn, "key", &key.derive(Label::Db))?;
    set_memory_security(&conn, opts.cipher_memory_security)?;
    verify_readable(&conn)?;
    Self::finish_init(conn, Mode::Session { hash: key.derive(Label::Hash) })
  }

  fn finish_init(mut conn: Connection, mode: Mode) -> Result<Self> {
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    MIGRATIONS.to_latest(&mut conn)?;
    reconcile_change_seq(&conn)?;
    Ok(Self::from_parts(conn, mode))
  }

  /// Delete an encrypted store's files in `dir`: the database, its WAL /
  /// SHM / rollback journal, the key check value and the `blobs/`
  /// directory. Missing files are fine. Destroying the key (slots) is
  /// spool-keys' job; without it the files are unreadable anyway.
  pub fn wipe(dir: &Path) -> Result<()> {
    for name in [
      DB_FILE_NAME.to_string(),
      format!("{DB_FILE_NAME}-wal"),
      format!("{DB_FILE_NAME}-shm"),
      format!("{DB_FILE_NAME}-journal"),
      KCV_FILE_NAME.to_string(),
      blobs::KCV_TMP_FILE_NAME.to_string(),
    ] {
      blobs::remove_if_exists(&dir.join(name))?;
    }
    match std::fs::remove_dir_all(dir.join(BLOB_DIR_NAME)) {
      Ok(()) => {}
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
      Err(e) => return Err(e.into()),
    }
    Ok(())
  }

  /// Which kind of store this is.
  pub fn kind(&self) -> StoreKind {
    match self.mode {
      Mode::Plain => StoreKind::Plain,
      Mode::Encrypted(_) => StoreKind::Encrypted,
      Mode::Session { .. } => StoreKind::Session,
    }
  }

  /// Raw connection, for crate-internal helpers and tests only.
  #[doc(hidden)]
  pub fn conn(&self) -> &Connection {
    &self.conn
  }

  /// Insert `item`, or bump the newest item of the same selection if its
  /// hash equals `item.hash`. Allocates a new `change_seq` either way.
  ///
  /// A bump sets `last_used_at = item.created_at` (never moving it
  /// backwards). Duplicate mimes in `item.reps` keep the first. In an
  /// encrypted store, reps larger than [`INLINE_MAX`] go to blob files.
  pub fn insert(&mut self, item: NewItem) -> Result<InsertOutcome> {
    if item.reps.is_empty() {
      return Err(Error::Corrupt("insert: item has no representations".into()));
    }
    let now_ms = to_millis(item.created_at);
    let row = RowData {
      selection: item.selection,
      source_app: item.source_app.as_deref(),
      created_at: now_ms,
      last_used_at: now_ms,
      flags: item.flags,
      hash: &item.hash,
      preview: item.preview.as_deref(),
      total_size: item.total_size() as i64,
    };
    let Store { conn, mode, .. } = self;
    let mut pending = blobs::Pending::new(mode);
    let tx = conn.transaction()?;
    let out = insert_row(&tx, mode, &row, &item.reps, &mut pending)?;
    tx.commit()?;
    pending.disarm();
    match out {
      InsertOutcome::Bumped(id) => {
        tracing::debug!(id = id.0, hash = %hash_prefix(&item.hash), "store: dedupe bump");
      }
      InsertOutcome::Inserted(id) => tracing::debug!(
        id = id.0,
        hash = %hash_prefix(&item.hash),
        len = item.total_size(),
        selection = item.selection.as_str(),
        "store: inserted"
      ),
    }
    Ok(out)
  }

  /// The item with all reps. Blob files are read and authenticated; a
  /// missing or tampered file yields [`Error::Blob`] naming the item.
  pub fn get(&self, id: ItemId) -> Result<Option<Item>> {
    let row = self
      .conn
      .query_row(
        &format!("SELECT {ITEM_COLS} FROM items WHERE id = ?1"),
        params![id.0],
        RawItem::from_row,
      )
      .optional()?;
    row.map(|raw| self.load_item(raw)).transpose()
  }

  /// Newest item (by `last_used_at`) of `selection`.
  pub fn latest(&self, selection: Selection) -> Result<Option<Item>> {
    let row = self
      .conn
      .query_row(
        &format!(
          "SELECT {ITEM_COLS} FROM items WHERE selection = ?1
           ORDER BY last_used_at DESC, id DESC LIMIT 1"
        ),
        params![selection.as_str()],
        RawItem::from_row,
      )
      .optional()?;
    row.map(|raw| self.load_item(raw)).transpose()
  }

  /// Newest-first summaries across both selections, pinned first. Never
  /// reads blob files.
  pub fn recent(&self, limit: usize) -> Result<Vec<ItemSummary>> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    // Walks `items_pin_recent` in order; stops after `limit` rows.
    let mut stmt = self.conn.prepare_cached(&format!(
      "SELECT {ITEM_COLS} FROM items
       ORDER BY {PINNED_EXPR} DESC, last_used_at DESC, id DESC LIMIT ?1"
    ))?;
    let raws = stmt.query_map(params![limit], RawItem::from_row)?.collect::<Result<Vec<_>, _>>()?;
    let ids: Vec<i64> = raws.iter().map(|r| r.id).collect();
    let mut mimes = mimes_of(&self.conn, &ids)?;
    raws
      .into_iter()
      .map(|raw| {
        let mimes = mimes.remove(&raw.id).unwrap_or_default();
        let item = raw.into_item(Vec::new())?;
        Ok(ItemSummary {
          id: item.id,
          created_at: item.created_at,
          last_used_at: item.last_used_at,
          selection: item.selection,
          source_app: item.source_app,
          flags: item.flags,
          preview: item.preview,
          total_size: item.total_size,
          mimes,
        })
      })
      .collect()
  }

  /// Set `last_used_at = at` (user re-selected the item). Returns `false` if
  /// the item does not exist. Also allocates a new `change_seq`.
  pub fn touch(&mut self, id: ItemId, at: SystemTime) -> Result<bool> {
    let tx = self.conn.transaction()?;
    let exists: bool =
      tx.query_row("SELECT EXISTS(SELECT 1 FROM items WHERE id = ?1)", params![id.0], |r| {
        r.get(0)
      })?;
    if !exists {
      return Ok(false);
    }
    let seq = next_change_seq(&tx)?;
    tx.execute(
      "UPDATE items SET last_used_at = ?1, change_seq = ?2 WHERE id = ?3",
      params![to_millis(at), seq, id.0],
    )?;
    tx.commit()?;
    Ok(true)
  }

  /// Pin (`on`) or unpin the item (the `PINNED` flag; pinned items are
  /// listed first and exempt from retention). Allocates a new `change_seq`
  /// (the search index re-reads the item). Returns `false` if the item does
  /// not exist.
  pub fn set_pinned(&mut self, id: ItemId, on: bool) -> Result<bool> {
    let tx = self.conn.transaction()?;
    let flags: Option<i64> = tx
      .query_row("SELECT flags FROM items WHERE id = ?1", params![id.0], |r| r.get(0))
      .optional()?;
    let Some(flags) = flags else { return Ok(false) };
    let bit = i64::from(ItemFlags::PINNED.bits());
    let flags = if on { flags | bit } else { flags & !bit };
    let seq = next_change_seq(&tx)?;
    tx.execute(
      "UPDATE items SET flags = ?1, change_seq = ?2 WHERE id = ?3",
      params![flags, seq, id.0],
    )?;
    tx.commit()?;
    Ok(true)
  }

  /// Add (`on`) or remove a tag on the item. Adding an existing tag or
  /// removing a missing one changes nothing but still succeeds. Allocates a
  /// new `change_seq` when something changed. Returns `false` if the item
  /// does not exist. The caller validates the tag text.
  pub fn set_tag(&mut self, id: ItemId, tag: &str, on: bool) -> Result<bool> {
    let tx = self.conn.transaction()?;
    let exists: bool =
      tx.query_row("SELECT EXISTS(SELECT 1 FROM items WHERE id = ?1)", params![id.0], |r| {
        r.get(0)
      })?;
    if !exists {
      return Ok(false);
    }
    let has: bool = tx.query_row(
      "SELECT EXISTS(SELECT 1 FROM tags WHERE item_id = ?1 AND tag = ?2)",
      params![id.0, tag],
      |r| r.get(0),
    )?;
    if has == on {
      return Ok(true);
    }
    if on {
      tx.execute("INSERT INTO tags(item_id, tag) VALUES (?1, ?2)", params![id.0, tag])?;
    } else {
      tx.execute("DELETE FROM tags WHERE item_id = ?1 AND tag = ?2", params![id.0, tag])?;
    }
    let seq = next_change_seq(&tx)?;
    tx.execute("UPDATE items SET change_seq = ?1 WHERE id = ?2", params![seq, id.0])?;
    tx.commit()?;
    Ok(true)
  }

  /// Delete the item (cascades to reps/tags) and write a tombstone with a
  /// new `change_seq`. Returns `false` if it did not exist. Its blob files
  /// are removed after the commit.
  pub fn delete(&mut self, id: ItemId) -> Result<bool> {
    let tx = self.conn.transaction()?;
    let files = blob_files_of(&tx, id.0)?;
    let deleted = delete_with_tombstone(&tx, id.0)?;
    tx.commit()?;
    if deleted {
      tracing::debug!(id = id.0, "store: deleted");
    }
    self.remove_blob_files(&files);
    Ok(deleted)
  }

  /// Delete unpinned items older than `limits.max_age` (by `last_used_at`)
  /// and, beyond that, the oldest unpinned items above `limits.max_items`.
  /// Writes tombstones; removes blob files after the commit. Returns the
  /// number deleted.
  pub fn retention_sweep(&mut self, now: SystemTime, limits: RetentionLimits) -> Result<usize> {
    let cutoff = now.checked_sub(limits.max_age).map(to_millis).unwrap_or(0);
    let tx = self.conn.transaction()?;
    // Both selects and the count walk the unpinned half of
    // `items_pin_recent` (covering; no table lookups).
    let mut doomed: Vec<i64> = {
      let mut stmt = tx.prepare_cached(&format!(
        "SELECT id FROM items WHERE {PINNED_EXPR} = 0 AND coalesce(last_used_at, 0) < ?1"
      ))?;
      stmt.query_map(params![cutoff], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    let remaining: i64 =
      tx.query_row(&format!("SELECT count(*) FROM items WHERE {PINNED_EXPR} = 0"), [], |r| {
        r.get(0)
      })?;
    let excess = remaining - doomed.len() as i64 - i64::from(limits.max_items);
    if excess > 0 {
      let mut stmt = tx.prepare_cached(&format!(
        "SELECT id FROM items WHERE {PINNED_EXPR} = 0 AND coalesce(last_used_at, 0) >= ?1
         ORDER BY last_used_at ASC, id ASC LIMIT ?2"
      ))?;
      let more: Vec<i64> =
        stmt.query_map(params![cutoff, excess], |r| r.get(0))?.collect::<Result<_, _>>()?;
      doomed.extend(more);
    }
    let (n, files) = delete_many_with_tombstones(&tx, &doomed)?;
    tx.commit()?;
    if n > 0 {
      tracing::debug!(deleted = n, "store: retention sweep");
    }
    self.remove_blob_files(&files);
    Ok(n)
  }

  /// Number of stored items.
  pub fn count(&self) -> Result<u64> {
    let n: i64 = self.conn.query_row("SELECT count(*) FROM items", [], |r| r.get(0))?;
    Ok(n.max(0) as u64)
  }

  /// The dedupe hash key. Encrypted and session stores: the `Label::Hash`
  /// sub-key of their [`DataKey`]. Plain stores: the per-install random key
  /// in `meta` (`META_HASH_KEY`), generated and persisted on first call.
  ///
  /// The returned copy is the caller's to keep secret (it compiles the
  /// `Policy`); prefer wrapping it in `Zeroizing`.
  pub fn hash_key(&mut self) -> Result<[u8; 32]> {
    match &self.mode {
      Mode::Encrypted(e) => return Ok(*e.hash.expose()),
      Mode::Session { hash } => return Ok(*hash.expose()),
      Mode::Plain => {}
    }
    let tx = self.conn.transaction()?;
    let existing: Option<Vec<u8>> = get_meta(&tx, META_HASH_KEY)?;
    let key = match existing {
      Some(v) => <[u8; 32]>::try_from(v.as_slice())
        .map_err(|_| Error::Corrupt(format!("meta.{META_HASH_KEY} has length {}", v.len())))?,
      None => {
        let k: [u8; 32] = rand::random();
        tx.execute("INSERT INTO meta(key, value) VALUES (?1, ?2)", params![META_HASH_KEY, &k[..]])?;
        k
      }
    };
    tx.commit()?;
    Ok(key)
  }

  /// Remove files in `blobs/` that no rep references (left by a crash
  /// between writing a blob and committing, or after a failed delete), plus
  /// stale `seal_file` temp files. Unknown file names are left alone (and
  /// logged). Returns the number removed. No-op unless encrypted. Run at
  /// open; safe to call any time.
  pub fn gc_orphan_blobs(&mut self) -> Result<usize> {
    let Mode::Encrypted(enc) = &self.mode else {
      return Ok(0);
    };
    let dir = enc.blob_dir();
    let entries = match std::fs::read_dir(&dir) {
      Ok(e) => e,
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
      Err(e) => return Err(e.into()),
    };
    // Every reference we compare against must be durable before a file
    // goes, or a power loss could resurrect a reference to it.
    checkpoint(&self.conn, "PASSIVE")?;
    let referenced: HashSet<String> = {
      let mut stmt = self.conn.prepare(
        "SELECT blob_file FROM reps WHERE blob_file IS NOT NULL
         UNION SELECT old FROM rekey_blobs UNION SELECT new FROM rekey_blobs",
      )?;
      stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    let mut removed = 0;
    for entry in entries {
      let entry = entry?;
      let name = entry.file_name();
      let Some(name) = name.to_str() else {
        tracing::warn!("store: unexpected non-UTF-8 file in the blob directory");
        continue;
      };
      if blobs::is_blob_name(name) {
        if referenced.contains(name) {
          continue;
        }
      } else if !blobs::is_temp_name(name) {
        tracing::warn!(file = name, "store: unexpected file in the blob directory");
        continue;
      }
      blobs::remove_if_exists(&entry.path())?;
      tracing::debug!(file = name, "store: removed orphan blob file");
      removed += 1;
    }
    Ok(removed)
  }

  /// Best-effort removal of blob files whose references were just
  /// committed away. Failures are logged; GC retries at next open.
  fn remove_blob_files(&self, files: &[String]) {
    let Mode::Encrypted(enc) = &self.mode else {
      return;
    };
    if files.is_empty() {
      return;
    }
    if let Err(e) = checkpoint(&self.conn, "PASSIVE") {
      tracing::warn!(error = %e, "store: checkpoint failed; blob files left for GC");
      return;
    }
    let dir = enc.blob_dir();
    for f in files {
      if let Err(e) = blobs::remove_if_exists(&dir.join(f)) {
        tracing::warn!(file = %f, error = %e, "store: removing blob file failed; left for GC");
      }
    }
  }

  /// Rewrite the key check value file if it does not hold exactly the
  /// current key's value.
  fn ensure_kcv(&self) -> Result<()> {
    let Mode::Encrypted(enc) = &self.mode else {
      return Ok(());
    };
    let current = blobs::read_kcv(&enc.dir.join(KCV_FILE_NAME)).ok();
    if current.as_deref() != Some(&[enc.kcv][..]) {
      blobs::write_kcv(&enc.dir, &[enc.kcv])?;
    }
    Ok(())
  }

  fn load_item(&self, raw: RawItem) -> Result<Item> {
    type RepRow = (String, Option<String>, Option<Vec<u8>>, Option<String>);
    let rows: Vec<RepRow> = {
      let mut stmt = self.conn.prepare_cached(
        "SELECT mime, alias_of, inline, blob_file FROM reps WHERE item_id = ?1 ORDER BY rowid",
      )?;
      stmt
        .query_map(params![raw.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?
    };
    let reps = rows
      .into_iter()
      .map(|(mime, alias_of, inline, blob_file)| {
        Ok(match (alias_of, blob_file) {
          (Some(canon), _) => Representation::alias(mime, canon),
          (None, Some(file)) => Representation::new(mime, blobs::read(&self.mode, raw.id, &file)?),
          (None, None) => Representation::new(mime, Bytes::from(inline.unwrap_or_default())),
        })
      })
      .collect::<Result<Vec<_>>>()?;
    raw.into_item(reps)
  }
}

/// Insert a row (or bump the newest same-selection item with an equal
/// hash) inside the caller's transaction. Large reps of encrypted stores
/// are written to blob files recorded in `pending`.
pub(crate) fn insert_row(
  tx: &Connection,
  mode: &Mode,
  row: &RowData<'_>,
  reps: &[Representation],
  pending: &mut blobs::Pending,
) -> Result<InsertOutcome> {
  // One `items_sel_recent` seek.
  let newest: Option<(i64, Vec<u8>)> = tx
    .prepare_cached(
      "SELECT id, hash FROM items WHERE selection = ?1
       ORDER BY last_used_at DESC, id DESC LIMIT 1",
    )?
    .query_row(params![row.selection.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))
    .optional()?;
  if let Some((id, hash)) = newest
    && hash.as_slice() == row.hash.as_slice()
  {
    let seq = next_change_seq(tx)?;
    tx.execute(
      "UPDATE items SET last_used_at = max(coalesce(last_used_at, 0), ?1), change_seq = ?2
       WHERE id = ?3",
      params![row.last_used_at, seq, id],
    )?;
    return Ok(InsertOutcome::Bumped(ItemId(id)));
  }

  let seq = next_change_seq(tx)?;
  tx.execute(
    "INSERT INTO items(change_seq, created_at, last_used_at, selection, source_app, flags,
                       hash, preview, total_size)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    params![
      seq,
      row.created_at,
      row.last_used_at,
      row.selection.as_str(),
      row.source_app,
      row.flags.bits(),
      &row.hash[..],
      row.preview,
      row.total_size,
    ],
  )?;
  let id = tx.last_insert_rowid();
  let mut stmt = tx.prepare_cached(
    "INSERT INTO reps(item_id, mime, alias_of, inline, blob_file, size)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
  )?;
  let mut seen = HashSet::new();
  for rep in reps {
    // Duplicate mimes keep the first (checked before any blob is written).
    if !seen.insert(rep.mime.as_str()) {
      continue;
    }
    let (inline, blob, size): (Option<&[u8]>, Option<String>, i64) = match (&rep.alias_of, mode) {
      (Some(_), _) => (None, None, 0),
      (None, Mode::Encrypted(enc)) if rep.data.len() > INLINE_MAX => {
        (None, Some(pending.write(enc, &rep.data)?), rep.data.len() as i64)
      }
      (None, _) => (Some(&rep.data[..]), None, rep.data.len() as i64),
    };
    stmt.execute(params![id, rep.mime, rep.alias_of, inline, blob, size])?;
  }
  Ok(InsertOutcome::Inserted(ItemId(id)))
}

/// Blob file names of one item's reps.
fn blob_files_of(conn: &Connection, id: i64) -> Result<Vec<String>> {
  let mut stmt = conn
    .prepare_cached("SELECT blob_file FROM reps WHERE item_id = ?1 AND blob_file IS NOT NULL")?;
  Ok(stmt.query_map(params![id], |r| r.get(0))?.collect::<Result<_, _>>()?)
}

const ITEM_COLS: &str = "id, change_seq, created_at, last_used_at, selection, source_app, flags, \
                         hash, preview, total_size";

/// An `items` row before validation.
struct RawItem {
  id: i64,
  change_seq: i64,
  created_at: Option<i64>,
  last_used_at: Option<i64>,
  selection: Option<String>,
  source_app: Option<String>,
  flags: Option<i64>,
  hash: Vec<u8>,
  preview: Option<String>,
  total_size: Option<i64>,
}

impl RawItem {
  fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
    Ok(Self {
      id: r.get(0)?,
      change_seq: r.get(1)?,
      created_at: r.get(2)?,
      last_used_at: r.get(3)?,
      selection: r.get(4)?,
      source_app: r.get(5)?,
      flags: r.get(6)?,
      hash: r.get(7)?,
      preview: r.get(8)?,
      total_size: r.get(9)?,
    })
  }

  fn into_item(self, reps: Vec<Representation>) -> Result<Item> {
    let selection = self
      .selection
      .as_deref()
      .and_then(Selection::parse)
      .ok_or_else(|| Error::Corrupt(format!("item {}: bad selection", self.id)))?;
    let hash = <[u8; 32]>::try_from(self.hash.as_slice())
      .map_err(|_| Error::Corrupt(format!("item {}: bad hash length", self.id)))?;
    let created_at = from_millis(self.created_at.unwrap_or(0));
    Ok(Item {
      id: ItemId(self.id),
      change_seq: self.change_seq,
      created_at,
      last_used_at: self.last_used_at.map(from_millis).unwrap_or(created_at),
      selection,
      source_app: self.source_app,
      flags: ItemFlags::from_bits_retain(self.flags.unwrap_or(0) as u32),
      hash,
      preview: self.preview,
      total_size: self.total_size.unwrap_or(0).max(0) as u64,
      reps,
    })
  }
}

/// `items` column expression for the pinned bit. Must match the
/// `items_pin_recent` index expression textually, or SQLite cannot use it.
const PINNED_EXPR: &str = "(flags & 1)";
const _: () = assert!(ItemFlags::PINNED.bits() == 1, "PINNED_EXPR and items_pin_recent use bit 0");

/// Ids per `IN (...)` list in bulk statements (well under SQLite's
/// variable limit).
const IN_CHUNK: usize = 500;

/// `?,?,...,?` with `n` placeholders.
fn placeholders(n: usize) -> String {
  let mut s = "?,".repeat(n);
  s.pop();
  s
}

/// Highest `change_seq` in `items`/`tombstones` (two index lookups).
fn max_stored_change_seq(conn: &Connection) -> Result<i64> {
  Ok(conn.query_row(
    "SELECT max(coalesce((SELECT max(change_seq) FROM items), 0),
                coalesce((SELECT max(change_seq) FROM tombstones), 0))",
    [],
    |r| r.get(0),
  )?)
}

/// Restore the invariant `meta.change_seq >= max(change_seq)` over
/// `items`/`tombstones` (guards against a lost/reset counter). Run once at
/// open; every allocation afterwards only touches the counter.
fn reconcile_change_seq(conn: &Connection) -> Result<()> {
  let stored: Option<i64> = get_meta(conn, META_CHANGE_SEQ)?;
  let floor = max_stored_change_seq(conn)?;
  if stored.unwrap_or(0) < floor {
    if stored.is_some() {
      tracing::warn!(stored, floor, "store: change_seq counter behind the data; raised");
    }
    set_meta(conn, META_CHANGE_SEQ, floor)?;
  }
  Ok(())
}

/// Reserve `n >= 1` consecutive `change_seq`s and return the first. Must
/// be called inside the caller's transaction. O(1): reads and bumps the
/// `meta.change_seq` counter (see [`reconcile_change_seq`]).
fn alloc_change_seqs(conn: &Connection, n: usize) -> Result<i64> {
  debug_assert!(n >= 1);
  let stored: i64 = get_meta(conn, META_CHANGE_SEQ)?.unwrap_or(0);
  let first = stored + 1;
  set_meta(conn, META_CHANGE_SEQ, stored + n as i64)?;
  Ok(first)
}

/// Allocate the next `change_seq` (`meta.change_seq + 1`, persisted).
fn next_change_seq(conn: &Connection) -> Result<i64> {
  alloc_change_seqs(conn, 1)
}

/// Delete one item and write its tombstone (inside the caller's transaction).
fn delete_with_tombstone(conn: &Connection, id: i64) -> Result<bool> {
  let n = conn.execute("DELETE FROM items WHERE id = ?1", params![id])?;
  if n == 0 {
    return Ok(false);
  }
  let seq = next_change_seq(conn)?;
  conn.execute("INSERT INTO tombstones(change_seq, item_id) VALUES (?1, ?2)", params![seq, id])?;
  Ok(true)
}

/// Delete `ids` (in chunks) and write their tombstones with consecutive
/// `change_seq`s in `ids` order, inside the caller's transaction. Returns
/// the number deleted and the blob files they referenced.
fn delete_many_with_tombstones(conn: &Connection, ids: &[i64]) -> Result<(usize, Vec<String>)> {
  let mut files = Vec::new();
  let mut gone = Vec::with_capacity(ids.len());
  for chunk in ids.chunks(IN_CHUNK) {
    let list = placeholders(chunk.len());
    let params = rusqlite::params_from_iter(chunk);
    {
      let mut stmt = conn.prepare_cached(&format!(
        "SELECT blob_file FROM reps WHERE item_id IN ({list}) AND blob_file IS NOT NULL"
      ))?;
      for f in stmt.query_map(params.clone(), |r| r.get::<_, String>(0))? {
        files.push(f?);
      }
    }
    let deleted: HashSet<i64> = {
      let mut stmt =
        conn.prepare_cached(&format!("DELETE FROM items WHERE id IN ({list}) RETURNING id"))?;
      stmt.query_map(params, |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    gone.extend(chunk.iter().copied().filter(|id| deleted.contains(id)));
  }
  if !gone.is_empty() {
    let first = alloc_change_seqs(conn, gone.len())?;
    let mut stmt =
      conn.prepare_cached("INSERT INTO tombstones(change_seq, item_id) VALUES (?1, ?2)")?;
    for (seq, id) in (first..).zip(&gone) {
      stmt.execute(params![seq, id])?;
    }
  }
  Ok((gone.len(), files))
}

/// Mime lists (rep order) of `ids`, in one query per [`IN_CHUNK`] ids.
fn mimes_of(conn: &Connection, ids: &[i64]) -> Result<HashMap<i64, Vec<String>>> {
  let mut out: HashMap<i64, Vec<String>> = HashMap::with_capacity(ids.len());
  for chunk in ids.chunks(IN_CHUNK) {
    let mut stmt = conn.prepare_cached(&format!(
      "SELECT item_id, mime FROM reps WHERE item_id IN ({}) ORDER BY rowid",
      placeholders(chunk.len())
    ))?;
    let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |r| {
      Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    for row in rows {
      let (id, mime) = row?;
      out.entry(id).or_default().push(mime);
    }
  }
  Ok(out)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn migrations_valid() {
    MIGRATIONS.validate().unwrap();
  }

  #[test]
  fn links_sqlcipher() {
    let s = Store::open_in_memory().unwrap();
    let v: String = s.conn().query_row("PRAGMA cipher_version", [], |r| r.get(0)).unwrap();
    assert!(!v.is_empty(), "not linked against SQLCipher");
  }

  #[test]
  fn open_file_plain_and_keyed() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("plain.db");
    drop(Store::open(&p, None).unwrap());
    let s = Store::open(&p, None).unwrap();
    let mode: String = s.conn().query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    assert_eq!(mode, "wal");

    let k = d.path().join("keyed.db");
    drop(Store::open(&k, Some(&[7; 32])).unwrap());
    assert!(Store::open(&k, Some(&[7; 32])).is_ok());
    assert!(matches!(Store::open(&k, Some(&[8; 32])), Err(crate::Error::WrongKey)));
    assert!(matches!(Store::open(&k, None), Err(crate::Error::WrongKey)));
  }

  #[test]
  fn migration_creates_hot_path_indexes() {
    let s = Store::open_in_memory().unwrap();
    let mut st = s.conn().prepare("SELECT name FROM sqlite_master WHERE type = 'index'").unwrap();
    let names: HashSet<String> =
      st.query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    for want in ["items_sel_recent", "items_change_seq", "items_pin_recent", "tags_item"] {
      assert!(names.contains(want), "missing index {want}: {names:?}");
    }
    assert!(!names.contains("items_recent"), "superseded index still present");
  }

  /// `EXPLAIN QUERY PLAN` details of `sql`, joined with " | ".
  fn plan(s: &Store, sql: &str) -> String {
    let mut st = s.conn().prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    let rows: Vec<String> =
      st.query_map([], |r| r.get(3)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    rows.join(" | ")
  }

  /// The hot queries are index seeks/ordered walks: no full table scan
  /// and no sort. Run with `--nocapture` to see the plans.
  #[test]
  fn hot_queries_use_indexes() {
    let s = Store::open_in_memory().unwrap();
    let ids = "1, 2, 3";
    let cases = [
      (
        "dedupe/latest",
        format!(
          "SELECT {ITEM_COLS} FROM items WHERE selection = 'clipboard'
           ORDER BY last_used_at DESC, id DESC LIMIT 1"
        ),
        "items_sel_recent",
      ),
      (
        "recent",
        format!(
          "SELECT {ITEM_COLS} FROM items
           ORDER BY {PINNED_EXPR} DESC, last_used_at DESC, id DESC LIMIT 50"
        ),
        "items_pin_recent",
      ),
      (
        "recent mimes",
        format!("SELECT item_id, mime FROM reps WHERE item_id IN ({ids}) ORDER BY rowid"),
        "sqlite_autoindex_reps_1",
      ),
      ("max change_seq", "SELECT max(change_seq) FROM items".into(), "items_change_seq"),
      (
        "sweep excess",
        format!(
          "SELECT id FROM items WHERE {PINNED_EXPR} = 0 AND coalesce(last_used_at, 0) >= 5
           ORDER BY last_used_at ASC, id ASC LIMIT 10"
        ),
        "items_pin_recent",
      ),
      ("sweep delete", format!("DELETE FROM items WHERE id IN ({ids})"), "tags_item"),
    ];
    for (name, sql, index) in cases {
      let p = plan(&s, &sql);
      println!("{name}: {p}");
      assert!(p.contains(index), "{name}: expected {index}: {p}");
      // `SCAN items USING INDEX` is an ordered index walk cut off by LIMIT.
      let full_scan = p.split(" | ").any(|step| {
        (step.starts_with("SCAN items") || step.starts_with("SCAN tags")) && !step.contains("INDEX")
      });
      assert!(!full_scan, "{name}: full table scan: {p}");
      // The mime query sorts only the (<= limit x mimes) matched rows.
      if name != "recent mimes" {
        assert!(!p.contains("TEMP B-TREE"), "{name}: sorts: {p}");
      }
    }
  }

  #[test]
  fn millis_roundtrip() {
    let t = from_millis(1_700_000_000_123);
    assert_eq!(to_millis(t), 1_700_000_000_123);
  }
}

#[cfg(test)]
mod store_tests {
  use std::time::Duration;

  use super::*;
  use crate::item::{ItemFlags, Representation};

  fn t(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + secs)
  }

  fn text_item(sel: Selection, text: &str, at: SystemTime) -> NewItem {
    let canon = Representation::new("text/plain;charset=utf-8", text.as_bytes().to_vec());
    NewItem {
      selection: sel,
      source_app: Some("org.kde.kate".into()),
      created_at: at,
      flags: ItemFlags::empty(),
      hash: crate::item::dedupe_hash(&[3; 32], &canon),
      preview: Some(text.into()),
      reps: vec![canon, Representation::alias("UTF8_STRING", "text/plain;charset=utf-8")],
    }
  }

  fn seqs(s: &Store) -> (i64, i64) {
    let items: i64 = s
      .conn()
      .query_row("SELECT coalesce(max(change_seq),0) FROM items", [], |r| r.get(0))
      .unwrap();
    let tombs: i64 = s
      .conn()
      .query_row("SELECT coalesce(max(change_seq),0) FROM tombstones", [], |r| r.get(0))
      .unwrap();
    (items, tombs)
  }

  fn max_seq(s: &Store) -> i64 {
    let (a, b) = seqs(s);
    a.max(b)
  }

  #[test]
  fn insert_get_roundtrip() {
    let mut s = Store::open_in_memory().unwrap();
    let ni = text_item(Selection::Clipboard, "hello", t(0));
    let id = s.insert(ni.clone()).unwrap();
    assert!(matches!(id, InsertOutcome::Inserted(_)));
    let it = s.get(id.id()).unwrap().unwrap();
    assert_eq!(it.id, id.id());
    assert_eq!(it.selection, Selection::Clipboard);
    assert_eq!(it.source_app.as_deref(), Some("org.kde.kate"));
    assert_eq!(it.created_at, t(0));
    assert_eq!(it.last_used_at, t(0));
    assert_eq!(it.hash, ni.hash);
    assert_eq!(it.preview.as_deref(), Some("hello"));
    assert_eq!(it.total_size, 5);
    assert_eq!(it.reps, ni.reps);
    assert_eq!(it.resolve("UTF8_STRING").unwrap().as_ref(), b"hello");
    let (blob, inline_alias): (Option<String>, Option<Vec<u8>>) = s
      .conn()
      .query_row(
        "SELECT (SELECT blob_file FROM reps WHERE mime='text/plain;charset=utf-8'),
                (SELECT inline FROM reps WHERE mime='UTF8_STRING')",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
      )
      .unwrap();
    assert_eq!((blob, inline_alias), (None, None));
    assert_eq!(s.get(ItemId(999)).unwrap(), None);
    assert_eq!(s.count().unwrap(), 1);
  }

  #[test]
  fn dedupe_bumps_newest_same_selection_only() {
    let mut s = Store::open_in_memory().unwrap();
    let a = s.insert(text_item(Selection::Clipboard, "a", t(0))).unwrap().id();
    let seq_a = s.get(a).unwrap().unwrap().change_seq;
    let out = s.insert(text_item(Selection::Clipboard, "a", t(10))).unwrap();
    assert_eq!(out, InsertOutcome::Bumped(a));
    let it = s.get(a).unwrap().unwrap();
    assert_eq!(it.last_used_at, t(10));
    assert_eq!(it.created_at, t(0));
    assert!(it.change_seq > seq_a);
    assert_eq!(s.count().unwrap(), 1);

    // Same content on the other selection is a new row.
    assert!(matches!(
      s.insert(text_item(Selection::Primary, "a", t(11))).unwrap(),
      InsertOutcome::Inserted(_)
    ));
    // Not the newest any more -> new row.
    s.insert(text_item(Selection::Clipboard, "b", t(12))).unwrap();
    assert!(matches!(
      s.insert(text_item(Selection::Clipboard, "a", t(13))).unwrap(),
      InsertOutcome::Inserted(_)
    ));
    assert_eq!(s.count().unwrap(), 4);
  }

  #[test]
  fn latest_and_recent_ordering() {
    let mut s = Store::open_in_memory().unwrap();
    assert_eq!(s.latest(Selection::Clipboard).unwrap(), None);
    let a = s.insert(text_item(Selection::Clipboard, "a", t(0))).unwrap().id();
    let b = s.insert(text_item(Selection::Clipboard, "b", t(1))).unwrap().id();
    let p = s.insert(text_item(Selection::Primary, "p", t(2))).unwrap().id();
    assert_eq!(s.latest(Selection::Clipboard).unwrap().unwrap().id, b);
    assert_eq!(s.latest(Selection::Primary).unwrap().unwrap().id, p);
    assert!(s.touch(a, t(5)).unwrap());
    assert!(!s.touch(ItemId(999), t(5)).unwrap());
    assert_eq!(s.latest(Selection::Clipboard).unwrap().unwrap().id, a);

    // Pin b: it goes first in recent().
    s.conn().execute("UPDATE items SET flags = 1 WHERE id = ?1", params![b.0]).unwrap();
    let r = s.recent(10).unwrap();
    assert_eq!(r.iter().map(|x| x.id).collect::<Vec<_>>(), vec![b, a, p]);
    assert_eq!(r[0].flags, ItemFlags::PINNED);
    assert_eq!(r[1].mimes, vec!["text/plain;charset=utf-8".to_string(), "UTF8_STRING".into()]);
    assert_eq!(r[1].total_size, 1);
    assert_eq!(s.recent(1).unwrap().len(), 1);
    assert_eq!(s.recent(0).unwrap().len(), 0);
  }

  #[test]
  fn delete_writes_tombstone_and_cascades() {
    let mut s = Store::open_in_memory().unwrap();
    let a = s.insert(text_item(Selection::Clipboard, "a", t(0))).unwrap().id();
    let before = max_seq(&s);
    assert!(s.delete(a).unwrap());
    assert!(!s.delete(a).unwrap());
    assert_eq!(s.get(a).unwrap(), None);
    let reps: i64 = s.conn().query_row("SELECT count(*) FROM reps", [], |r| r.get(0)).unwrap();
    assert_eq!(reps, 0);
    let (seq, item): (i64, i64) = s
      .conn()
      .query_row("SELECT change_seq, item_id FROM tombstones", [], |r| Ok((r.get(0)?, r.get(1)?)))
      .unwrap();
    assert_eq!(item, a.0);
    assert!(seq > before);
    assert_eq!(s.count().unwrap(), 0);
  }

  #[test]
  fn change_seq_monotonic() {
    let mut s = Store::open_in_memory().unwrap();
    let mut last = 0;
    let mut check = |s: &Store| {
      let now: i64 = s
        .conn()
        .query_row(
          "SELECT CAST(value AS INTEGER) FROM meta WHERE key = ?1",
          params![META_CHANGE_SEQ],
          |r| r.get(0),
        )
        .unwrap();
      assert!(now > last, "{now} <= {last}");
      assert_eq!(now, max_seq(s));
      last = now;
    };
    let a = s.insert(text_item(Selection::Clipboard, "a", t(0))).unwrap().id();
    check(&s);
    s.insert(text_item(Selection::Clipboard, "a", t(1))).unwrap();
    check(&s);
    let b = s.insert(text_item(Selection::Clipboard, "b", t(2))).unwrap().id();
    check(&s);
    s.touch(a, t(3)).unwrap();
    check(&s);
    s.delete(b).unwrap();
    check(&s);
    // Deleting the newest item must not let its seq be reused.
    s.delete(a).unwrap();
    check(&s);
    s.insert(text_item(Selection::Clipboard, "c", t(4))).unwrap();
    check(&s);
  }

  #[test]
  fn pin_and_tag_bump_change_seq() {
    let mut s = Store::open_in_memory().unwrap();
    let a = s.insert(text_item(Selection::Clipboard, "a", t(1))).unwrap().id();
    let seq0 = max_seq(&s);
    assert!(s.set_pinned(a, true).unwrap());
    assert!(s.get(a).unwrap().unwrap().flags.contains(ItemFlags::PINNED));
    let seq1 = max_seq(&s);
    assert!(seq1 > seq0);
    assert!(s.set_pinned(a, false).unwrap());
    assert!(!s.get(a).unwrap().unwrap().flags.contains(ItemFlags::PINNED));
    assert!(!s.set_pinned(ItemId(999), true).unwrap());

    assert!(s.set_tag(a, "work", true).unwrap());
    assert!(s.set_tag(a, "work", true).unwrap(), "adding twice is fine");
    assert!(s.set_tag(a, "x", true).unwrap());
    let tags = |s: &Store| s.summaries(&[a]).unwrap()[0].tags.clone();
    let mut got = tags(&s);
    got.sort();
    assert_eq!(got, ["work", "x"]);
    let seq2 = max_seq(&s);
    assert!(s.set_tag(a, "work", false).unwrap());
    assert_eq!(tags(&s), ["x"]);
    assert!(max_seq(&s) > seq2);
    let seq3 = max_seq(&s);
    assert!(s.set_tag(a, "missing", false).unwrap());
    assert_eq!(max_seq(&s), seq3, "no-op leaves change_seq alone");
    assert!(!s.set_tag(ItemId(999), "x", true).unwrap());
  }

  #[test]
  fn retention_by_age_and_count_with_pinned_exempt() {
    let mut s = Store::open_in_memory().unwrap();
    let day = 86_400;
    let old_pinned = s.insert(text_item(Selection::Clipboard, "old pinned", t(0))).unwrap().id();
    s.conn().execute("UPDATE items SET flags = 1 WHERE id = ?1", params![old_pinned.0]).unwrap();
    let old = s.insert(text_item(Selection::Clipboard, "old", t(1))).unwrap().id();
    let mut fresh = Vec::new();
    for i in 0..5 {
      fresh.push(
        s.insert(text_item(Selection::Primary, &format!("f{i}"), t(20 * day + i))).unwrap().id(),
      );
    }
    let limits = RetentionLimits { max_age: Duration::from_secs(14 * day), max_items: 3 };
    let seq_before = max_seq(&s);
    let n = s.retention_sweep(t(21 * day), limits).unwrap();
    assert_eq!(n, 3, "1 by age + 2 oldest over the count cap");
    assert_eq!(s.get(old).unwrap(), None);
    assert!(s.get(old_pinned).unwrap().is_some());
    assert_eq!(s.get(fresh[0]).unwrap(), None);
    assert_eq!(s.get(fresh[1]).unwrap(), None);
    for id in &fresh[2..] {
      assert!(s.get(*id).unwrap().is_some());
    }
    assert_eq!(s.count().unwrap(), 4);
    let tombs: Vec<(i64, i64)> = {
      let mut st =
        s.conn().prepare("SELECT change_seq, item_id FROM tombstones ORDER BY change_seq").unwrap();
      st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
    };
    assert_eq!(tombs.len(), 3);
    assert!(tombs.iter().all(|(seq, _)| *seq > seq_before));
    // Idempotent.
    assert_eq!(s.retention_sweep(t(21 * day), limits).unwrap(), 0);
    // max_items = 0 removes every unpinned item, never the pinned one.
    let zero = RetentionLimits { max_age: Duration::from_secs(14 * day), max_items: 0 };
    assert_eq!(s.retention_sweep(t(21 * day), zero).unwrap(), 3);
    assert_eq!(s.count().unwrap(), 1);
    assert!(s.get(old_pinned).unwrap().is_some());
  }

  #[test]
  fn hash_key_generated_once() {
    let mut s = Store::open_in_memory().unwrap();
    let k1 = s.hash_key().unwrap();
    let k2 = s.hash_key().unwrap();
    assert_eq!(k1, k2);
    assert_ne!(k1, [0; 32]);
    let mut other = Store::open_in_memory().unwrap();
    assert_ne!(other.hash_key().unwrap(), k1);
  }

  #[test]
  fn reopen_persists_items_key_and_seq() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("h.db");
    let key = [5u8; 32];
    let (id, hk, seq) = {
      let mut s = Store::open(&p, Some(&key)).unwrap();
      let id = s.insert(text_item(Selection::Clipboard, "persist me", t(0))).unwrap().id();
      (id, s.hash_key().unwrap(), max_seq(&s))
    };
    let mut s = Store::open(&p, Some(&key)).unwrap();
    assert_eq!(s.hash_key().unwrap(), hk);
    let it = s.get(id).unwrap().unwrap();
    assert_eq!(it.resolve("UTF8_STRING").unwrap().as_ref(), b"persist me");
    let id2 = s.insert(text_item(Selection::Clipboard, "next", t(1))).unwrap().id();
    assert!(s.get(id2).unwrap().unwrap().change_seq > seq);
    drop(s);

    assert!(matches!(Store::open(&p, Some(&[6; 32])), Err(crate::Error::WrongKey)));
    assert!(matches!(Store::open(&p, None), Err(crate::Error::WrongKey)));
    // Content is not readable in plaintext on disk.
    let raw = std::fs::read(&p).unwrap();
    assert!(!raw.windows(10).any(|w| w == b"persist me"));
  }

  #[test]
  fn plain_reopen_persists() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("plain.db");
    let id =
      Store::open(&p, None).unwrap().insert(text_item(Selection::Primary, "x", t(0))).unwrap().id();
    let s = Store::open(&p, None).unwrap();
    assert_eq!(s.latest(Selection::Primary).unwrap().unwrap().id, id);
  }

  #[test]
  fn insert_rejects_empty_reps_and_keeps_first_duplicate_mime() {
    let mut s = Store::open_in_memory().unwrap();
    let mut ni = text_item(Selection::Clipboard, "a", t(0));
    ni.reps.clear();
    assert!(s.insert(ni).is_err());
    let mut ni = text_item(Selection::Clipboard, "a", t(0));
    ni.reps.push(Representation::new("text/plain;charset=utf-8", &b"zzz"[..]));
    let id = s.insert(ni).unwrap().id();
    assert_eq!(
      s.get(id).unwrap().unwrap().resolve("text/plain;charset=utf-8").unwrap().as_ref(),
      b"a"
    );
  }
}
