//! Blob files (`blobs/<32 hex>.bin`) and the key check value file.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use bytes::Bytes;
use spool_crypto::{DEFAULT_CHUNK_SIZE_LOG2, SubKey};

use super::{Encrypted, KCV_FILE_NAME, Mode};
use crate::{Error, Result};

/// Temp name used while replacing the key check value file.
pub(crate) const KCV_TMP_FILE_NAME: &str = "history.db.kcv.tmp";
const KCV_MAGIC: &[u8; 8] = b"SPLKCV01";
/// BLAKE3 `derive_key` context for key check values.
const KCV_CONTEXT: &str = "spool 2026-10 key check value v1";

/// Key check value of a store's `Label::Db` sub-key: a one-way KDF output
/// of a 256-bit random key, so it reveals nothing useful about the key.
pub(crate) fn kcv_of(db: &SubKey) -> [u8; 32] {
  blake3::derive_key(KCV_CONTEXT, db.expose())
}

/// `<32 lowercase hex>.bin`
pub(crate) fn is_blob_name(name: &str) -> bool {
  name.len() == 36
    && name.ends_with(".bin")
    && name[..32].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `spool_crypto::seal_file` temp files: `.<name>.tmp-<16 hex>`.
pub(crate) fn is_temp_name(name: &str) -> bool {
  name.starts_with('.') && name.contains(".bin.tmp-")
}

fn new_blob_name() -> String {
  let r: [u8; 16] = rand::random();
  let mut s = String::with_capacity(36);
  for b in r {
    s.push_str(&format!("{b:02x}"));
  }
  s.push_str(".bin");
  s
}

pub(crate) fn remove_if_exists(path: &Path) -> io::Result<()> {
  match fs::remove_file(path) {
    Ok(()) => Ok(()),
    Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
    Err(e) => Err(e),
  }
}

/// Create `blobs/` (0700) if missing; refuse a symlink or non-directory.
pub(crate) fn ensure_blob_dir(dir: &Path) -> Result<()> {
  match fs::DirBuilder::new().mode(0o700).create(dir) {
    Ok(()) => {}
    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
    Err(e) => return Err(e.into()),
  }
  let md = fs::symlink_metadata(dir)?;
  if !md.is_dir() {
    return Err(Error::Io(io::Error::other(format!("{} is not a directory", dir.display()))));
  }
  Ok(())
}

/// Read and authenticate the blob `file` of `item`.
pub(crate) fn read(mode: &Mode, item: i64, file: &str) -> Result<Bytes> {
  let Mode::Encrypted(enc) = mode else {
    return Err(Error::Corrupt(format!(
      "item {item}: references a blob file but this store has no blob key"
    )));
  };
  read_with(enc, item, file)
}

pub(crate) fn read_with(enc: &Encrypted, item: i64, file: &str) -> Result<Bytes> {
  if !is_blob_name(file) {
    return Err(Error::Corrupt(format!("item {item}: invalid blob file name")));
  }
  let pt = spool_crypto::open_file(&enc.blob_dir().join(file), &enc.blob, file.as_bytes())
    .map_err(|source| Error::Blob { item, file: file.to_string(), source })?;
  Ok(Bytes::copy_from_slice(&pt))
}

/// Blob files written during a not-yet-committed transaction. Dropping it
/// armed removes them (the transaction rolled back); call
/// [`Pending::disarm`] after the commit.
pub(crate) struct Pending {
  dir: Option<PathBuf>,
  files: Vec<String>,
}

impl Pending {
  pub(crate) fn new(mode: &Mode) -> Self {
    match mode {
      Mode::Encrypted(enc) => Self::for_enc(enc),
      _ => Self { dir: None, files: Vec::new() },
    }
  }

  pub(crate) fn for_enc(enc: &Encrypted) -> Self {
    Self { dir: Some(enc.blob_dir()), files: Vec::new() }
  }

  /// Seal `data` into a fresh blob file under `enc`'s blob key; returns its
  /// name. The file is fsynced and atomically renamed into place.
  pub(crate) fn write(&mut self, enc: &Encrypted, data: &[u8]) -> Result<String> {
    let name = new_blob_name();
    let path = enc.blob_dir().join(&name);
    spool_crypto::seal_file(&path, &enc.blob, name.as_bytes(), DEFAULT_CHUNK_SIZE_LOG2, data)
      .map_err(|e| Error::Io(e.into()))?;
    self.files.push(name.clone());
    Ok(name)
  }

  pub(crate) fn len(&self) -> usize {
    self.files.len()
  }

  pub(crate) fn disarm(mut self) {
    self.files.clear();
  }
}

impl Drop for Pending {
  fn drop(&mut self) {
    let Some(dir) = &self.dir else { return };
    for f in self.files.drain(..) {
      if let Err(e) = remove_if_exists(&dir.join(&f)) {
        tracing::warn!(file = %f, error = %e, "store: removing uncommitted blob failed; left for GC");
      }
    }
  }
}

/// Parse the key check value file: magic, count `u8`, `count * 32` bytes.
pub(crate) fn read_kcv(path: &Path) -> io::Result<Vec<[u8; 32]>> {
  let mut buf = Vec::new();
  File::open(path)?.take(8 + 1 + 4 * 32).read_to_end(&mut buf)?;
  let bad = || io::Error::new(io::ErrorKind::InvalidData, "bad key check value file");
  if buf.len() < 9 || &buf[..8] != KCV_MAGIC {
    return Err(bad());
  }
  let n = buf[8] as usize;
  if n == 0 || buf.len() != 9 + n * 32 {
    return Err(bad());
  }
  Ok(buf[9..].chunks_exact(32).map(|c| c.try_into().expect("32-byte chunk")).collect())
}

/// Atomically replace `dir/history.db.kcv` with `kcvs` (temp, fsync,
/// rename, dir fsync).
pub(crate) fn write_kcv(dir: &Path, kcvs: &[[u8; 32]]) -> Result<()> {
  let mut buf = Vec::with_capacity(9 + 32 * kcvs.len());
  buf.extend_from_slice(KCV_MAGIC);
  buf.push(u8::try_from(kcvs.len()).expect("at most two key check values"));
  for k in kcvs {
    buf.extend_from_slice(k);
  }
  let tmp = dir.join(KCV_TMP_FILE_NAME);
  remove_if_exists(&tmp)?;
  let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
  f.write_all(&buf)?;
  f.sync_all()?;
  drop(f);
  fs::rename(&tmp, dir.join(KCV_FILE_NAME))?;
  File::open(dir)?.sync_all()?;
  Ok(())
}

/// Classify a database that fails its first read with `SQLITE_NOTADB`.
///
/// SQLCipher cannot tell a wrong key from a damaged first page (both fail
/// the page HMAC), so the key check value file breaks the tie: if `kcv`
/// is the only value recorded, the key is right and the file is damaged.
/// While a rekey is in flight two values are recorded and the database may
/// be under either key, so a match there still reports `WrongKey` (try the
/// other key).
pub(crate) fn classify_unreadable(db: &Path, kcv_path: &Path, kcv: &[u8; 32]) -> Error {
  let mut head = [0u8; 16];
  let (len, n) = match File::open(db).and_then(|mut f| {
    let len = f.metadata()?.len();
    Ok((len, f.read(&mut head)?))
  }) {
    Ok(v) => v,
    Err(e) => return Error::Io(e),
  };
  if n == head.len() && &head == b"SQLite format 3\0" {
    return Error::NotEncrypted;
  }
  // SQLCipher's page size is 4096; a valid file is a whole number of pages.
  if len % 4096 != 0 {
    return Error::Corrupt(format!("database file length {len} is not a whole number of pages"));
  }
  match read_kcv(kcv_path) {
    Ok(list) if list.len() == 1 && list[0] == *kcv => Error::Corrupt(
      "database fails authentication although the key matches its key check value".into(),
    ),
    _ => Error::WrongKey,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn names() {
    let n = new_blob_name();
    assert!(is_blob_name(&n), "{n}");
    assert_ne!(n, new_blob_name());
    assert!(!is_blob_name("../../etc/passwd"));
    assert!(!is_blob_name(&n.to_uppercase()));
    assert!(!is_blob_name(&format!("{}.bim", &n[..32])));
    assert!(is_temp_name(&format!(".{n}.tmp-0011223344556677")));
    assert!(!is_temp_name(&n));
  }

  #[test]
  fn kcv_file_roundtrip() {
    let d = tempfile::tempdir().unwrap();
    write_kcv(d.path(), &[[1; 32], [2; 32]]).unwrap();
    assert_eq!(read_kcv(&d.path().join(KCV_FILE_NAME)).unwrap(), vec![[1; 32], [2; 32]]);
    write_kcv(d.path(), &[[3; 32]]).unwrap();
    assert_eq!(read_kcv(&d.path().join(KCV_FILE_NAME)).unwrap(), vec![[3; 32]]);
    std::fs::write(d.path().join(KCV_FILE_NAME), b"SPLKCV01\x01short").unwrap();
    assert!(read_kcv(&d.path().join(KCV_FILE_NAME)).is_err());
  }
}
