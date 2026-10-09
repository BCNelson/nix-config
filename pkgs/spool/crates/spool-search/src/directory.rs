//! [`EncryptedDirectory`]: a [`tantivy::Directory`] whose files are
//! encrypted at rest with `spool-crypto`.
//!
//! | Tantivy call | On disk |
//! | --- | --- |
//! | `open_write` | streaming [`ChunkedWriter`] into a 0600 temp file; on `terminate` the final chunk is sealed, the file fsynced and renamed into place with `RENAME_NOREPLACE` (files are write-once) |
//! | `get_file_handle` | [`ChunkedReader`]; v1 decrypts the whole file on first open into a zeroize-on-drop buffer shared through [`OwnedBytes`], cached per path until `delete` |
//! | `atomic_write` / `atomic_read` | [`spool_crypto::seal`] / [`spool_crypto::open`] single blob, AAD = path; temp + fsync + rename + dir fsync |
//! | `watch` | callbacks fired by our own `atomic_write("meta.json")` (single writer, no inotify) |
//! | `acquire_lock` | `flock(2)` on a plaintext, empty lock file |
//!
//! Every file is bound to its Tantivy-relative path (chunked AAD / sealed
//! AAD), so renaming or swapping files fails authentication.
//!
//! The cache is the seam for a future lazy reader: [`DecryptedFile`] is the
//! only `FileHandle` type, and replacing its whole-file buffer with a per-chunk
//! LRU over a [`ChunkedReader`] needs no change elsewhere (the chunked format
//! is already random-access).

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::ops::{Deref, Range};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rustix::fs::{FlockOperation, RenameFlags};
use spool_crypto::{ChunkedReader, ChunkedWriter, DEFAULT_CHUNK_SIZE_LOG2, SubKey};
use tantivy::directory::error::{DeleteError, LockError, OpenReadError, OpenWriteError};
use tantivy::directory::{
  AntiCallToken, DirectoryLock, FileHandle, Lock, OwnedBytes, TerminatingWrite, WatchCallback,
  WatchCallbackList, WatchHandle, WritePtr,
};
use tantivy::{Directory, HasLen};
use zeroize::Zeroize;

/// Tantivy's metadata file; writing it fires the watch callbacks.
const META_FILE: &str = "meta.json";
/// Marker in temp-file names (`.<name>.spooltmp-<rand>`).
const TMP_MARKER: &str = ".spooltmp-";

/// Encrypted on-disk Tantivy directory. Cheap to clone (shared state).
#[derive(Clone)]
pub struct EncryptedDirectory {
  inner: Arc<Inner>,
}

struct Inner {
  root: PathBuf,
  key: SubKey,
  cache: Mutex<HashMap<PathBuf, OwnedBytes>>,
  watchers: WatchCallbackList,
  /// Number of AEAD authentication failures seen (wrong key or tampering).
  auth_failures: AtomicU64,
}

impl fmt::Debug for EncryptedDirectory {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("EncryptedDirectory").field("root", &self.inner.root).finish_non_exhaustive()
  }
}

impl EncryptedDirectory {
  /// Open (creating it 0700 if missing) the directory at `root`. `key` should
  /// be `DataKey::derive(Label::Index)`. Stale temp files from a crashed
  /// writer are removed, so only call this while holding the index's
  /// instance lock ([`SearchIndex`](crate::SearchIndex) does).
  pub fn open(root: &Path, key: SubKey) -> io::Result<Self> {
    create_private_dir(root)?;
    for entry in fs::read_dir(root)? {
      let entry = entry?;
      if entry.file_name().to_string_lossy().contains(TMP_MARKER) {
        let _ = fs::remove_file(entry.path());
      }
    }
    Ok(EncryptedDirectory {
      inner: Arc::new(Inner {
        root: root.to_path_buf(),
        key,
        cache: Mutex::new(HashMap::new()),
        watchers: WatchCallbackList::default(),
        auth_failures: AtomicU64::new(0),
      }),
    })
  }

  /// The directory on disk.
  pub fn root(&self) -> &Path {
    &self.inner.root
  }

  /// How many decryptions failed authentication since this directory was
  /// opened (used to classify open failures as "decrypt failure").
  pub fn auth_failures(&self) -> u64 {
    self.inner.auth_failures.load(Ordering::Relaxed)
  }

  /// Total plaintext bytes currently held in the decrypted-file cache.
  pub fn cached_bytes(&self) -> usize {
    self.inner.cache.lock().unwrap_or_else(|e| e.into_inner()).values().map(|b| b.len()).sum()
  }

  fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
    // Tantivy only uses flat relative names; refuse anything that could
    // escape the root.
    let mut comps = path.components();
    match (comps.next(), comps.next()) {
      (Some(Component::Normal(_)), None) => Ok(self.inner.root.join(path)),
      _ => Err(io::Error::new(io::ErrorKind::InvalidInput, "unexpected index file path")),
    }
  }

  fn note_crypto_error(&self, e: spool_crypto::Error) -> io::Error {
    if matches!(e, spool_crypto::Error::Auth) {
      self.inner.auth_failures.fetch_add(1, Ordering::Relaxed);
    }
    io::Error::from(e)
  }

  fn decrypt_file(&self, path: &Path, full: &Path) -> Result<OwnedBytes, OpenReadError> {
    let io_err = |e: io::Error| {
      if e.kind() == io::ErrorKind::NotFound {
        OpenReadError::FileDoesNotExist(path.to_path_buf())
      } else {
        OpenReadError::wrap_io_error(e, path.to_path_buf())
      }
    };
    let file = File::open(full).map_err(io_err)?;
    let logical = path.as_os_str().as_encoded_bytes();
    let mut reader = ChunkedReader::open(&self.inner.key, logical, io::BufReader::new(file))
      .map_err(|e| io_err(self.note_crypto_error(e)))?;
    let mut plain = reader.read_all().map_err(|e| io_err(self.note_crypto_error(e)))?;
    Ok(OwnedBytes::new(ZeroBuf(std::mem::take(&mut *plain))))
  }
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
  use std::os::unix::fs::DirBuilderExt;
  match fs::DirBuilder::new().recursive(true).mode(0o700).create(dir) {
    Ok(()) => Ok(()),
    Err(e) if e.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => Ok(()),
    Err(e) => Err(e),
  }
}

fn tmp_path(final_path: &Path) -> PathBuf {
  // Only needs to be unique, not secret.
  let name = final_path.file_name().unwrap_or_default();
  let mut tmp = OsString::from(".");
  tmp.push(name);
  tmp.push(format!("{TMP_MARKER}{:016x}", rand::random::<u64>()));
  final_path.with_file_name(tmp)
}

fn sync_dir(dir: &Path) -> io::Result<()> {
  File::open(dir)?.sync_all()
}

/// Decrypted plaintext buffer, zeroized when the last `OwnedBytes` drops.
struct ZeroBuf(Vec<u8>);

impl Deref for ZeroBuf {
  type Target = [u8];
  fn deref(&self) -> &[u8] {
    &self.0
  }
}

impl Drop for ZeroBuf {
  fn drop(&mut self) {
    self.0.zeroize();
  }
}

// SAFETY: `ZeroBuf` derefs into its `Vec`'s heap allocation, which does not
// move when the `ZeroBuf` itself moves, and the vector is never mutated
// (only zeroized in `Drop`, after the last borrow is gone).
#[allow(unsafe_code)]
unsafe impl stable_deref_trait::StableDeref for ZeroBuf {}

/// A decrypted index file (v1: the whole plaintext in memory).
#[derive(Clone)]
struct DecryptedFile {
  bytes: OwnedBytes,
}

impl fmt::Debug for DecryptedFile {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "DecryptedFile({} bytes)", self.bytes.len())
  }
}

impl HasLen for DecryptedFile {
  fn len(&self) -> usize {
    self.bytes.len()
  }
}

impl FileHandle for DecryptedFile {
  fn read_bytes(&self, range: Range<usize>) -> io::Result<OwnedBytes> {
    if range.start > range.end || range.end > self.bytes.len() {
      return Err(io::Error::new(io::ErrorKind::InvalidInput, "read past end of index file"));
    }
    Ok(self.bytes.slice(range))
  }
}

/// Streaming encrypted writer behind a [`WritePtr`].
struct EncryptedWriter {
  inner: Option<ChunkedWriter<BufWriter<File>>>,
  tmp: PathBuf,
  final_path: PathBuf,
}

impl Write for EncryptedWriter {
  fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
    self.inner.as_mut().ok_or_else(terminated)?.write(buf)
  }

  fn flush(&mut self) -> io::Result<()> {
    // Nothing becomes visible before `terminate`; flushing the partial chunk
    // is impossible by design, so only push completed chunks to the kernel.
    self.inner.as_mut().ok_or_else(terminated)?.flush()
  }
}

fn terminated() -> io::Error {
  io::Error::other("index file writer already terminated")
}

impl TerminatingWrite for EncryptedWriter {
  fn terminate_ref(&mut self, _: AntiCallToken) -> io::Result<()> {
    let w = self.inner.take().ok_or_else(terminated)?;
    let file = w.finish()?.into_inner().map_err(|e| e.into_error())?;
    file.sync_all()?;
    drop(file);
    // Write-once: never replace an existing file. Tantivy calls
    // `sync_directory` before publishing meta.json, so no dir fsync here.
    rustix::fs::renameat_with(
      rustix::fs::CWD,
      &self.tmp,
      rustix::fs::CWD,
      &self.final_path,
      RenameFlags::NOREPLACE,
    )
    .map_err(io::Error::from)?;
    Ok(())
  }
}

impl Drop for EncryptedWriter {
  fn drop(&mut self) {
    if self.inner.take().is_some() {
      let _ = fs::remove_file(&self.tmp);
    }
  }
}

impl Directory for EncryptedDirectory {
  fn get_file_handle(&self, path: &Path) -> Result<Arc<dyn FileHandle>, OpenReadError> {
    let full = self.resolve(path).map_err(|e| OpenReadError::wrap_io_error(e, path.into()))?;
    if let Some(b) = self.inner.cache.lock().unwrap_or_else(|e| e.into_inner()).get(path) {
      return Ok(Arc::new(DecryptedFile { bytes: b.clone() }));
    }
    // Decrypt outside the lock; a racing open of the same file just wins or
    // loses the insert.
    let bytes = self.decrypt_file(path, &full)?;
    let bytes = self
      .inner
      .cache
      .lock()
      .unwrap_or_else(|e| e.into_inner())
      .entry(path.to_path_buf())
      .or_insert(bytes)
      .clone();
    Ok(Arc::new(DecryptedFile { bytes }))
  }

  fn delete(&self, path: &Path) -> Result<(), DeleteError> {
    let full = self
      .resolve(path)
      .map_err(|e| DeleteError::IoError { io_error: Arc::new(e), filepath: path.into() })?;
    self.inner.cache.lock().unwrap_or_else(|e| e.into_inner()).remove(path);
    match fs::remove_file(&full) {
      Ok(()) => Ok(()),
      Err(e) if e.kind() == io::ErrorKind::NotFound => {
        Err(DeleteError::FileDoesNotExist(path.to_path_buf()))
      }
      Err(e) => Err(DeleteError::IoError { io_error: Arc::new(e), filepath: path.into() }),
    }
  }

  fn exists(&self, path: &Path) -> Result<bool, OpenReadError> {
    let full = self.resolve(path).map_err(|e| OpenReadError::wrap_io_error(e, path.into()))?;
    full.try_exists().map_err(|e| OpenReadError::wrap_io_error(e, path.into()))
  }

  fn open_write(&self, path: &Path) -> Result<WritePtr, OpenWriteError> {
    let wrap = |e: io::Error| OpenWriteError::wrap_io_error(e, path.to_path_buf());
    let full = self.resolve(path).map_err(wrap)?;
    if full.try_exists().map_err(wrap)? {
      return Err(OpenWriteError::FileAlreadyExists(path.to_path_buf()));
    }
    let tmp = tmp_path(&full);
    let file =
      OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(wrap)?;
    let logical = path.as_os_str().as_encoded_bytes();
    let chunked = match ChunkedWriter::new(
      &self.inner.key,
      logical,
      DEFAULT_CHUNK_SIZE_LOG2,
      BufWriter::new(file),
    ) {
      Ok(w) => w,
      Err(e) => {
        let _ = fs::remove_file(&tmp);
        return Err(wrap(e));
      }
    };
    let w = EncryptedWriter { inner: Some(chunked), tmp, final_path: full };
    Ok(BufWriter::new(Box::new(w)))
  }

  fn atomic_read(&self, path: &Path) -> Result<Vec<u8>, OpenReadError> {
    let full = self.resolve(path).map_err(|e| OpenReadError::wrap_io_error(e, path.into()))?;
    let sealed = match fs::read(&full) {
      Ok(b) => b,
      Err(e) if e.kind() == io::ErrorKind::NotFound => {
        return Err(OpenReadError::FileDoesNotExist(path.to_path_buf()));
      }
      Err(e) => return Err(OpenReadError::wrap_io_error(e, path.into())),
    };
    let aad = path.as_os_str().as_encoded_bytes();
    let mut plain = spool_crypto::open(self.inner.key.expose(), aad, &sealed)
      .map_err(|e| OpenReadError::wrap_io_error(self.note_crypto_error(e), path.into()))?;
    // Tantivy wants a plain Vec; the metadata it holds (segment ids, the
    // commit payload, schema) contains no clipboard text.
    Ok(std::mem::take(&mut *plain))
  }

  fn atomic_write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
    let full = self.resolve(path)?;
    let sealed =
      spool_crypto::seal(self.inner.key.expose(), path.as_os_str().as_encoded_bytes(), data);
    let tmp = tmp_path(&full);
    let write = || -> io::Result<()> {
      let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
      f.write_all(&sealed)?;
      f.sync_all()?;
      drop(f);
      fs::rename(&tmp, &full)
    };
    if let Err(e) = write() {
      let _ = fs::remove_file(&tmp);
      return Err(e);
    }
    sync_dir(&self.inner.root)?;
    if path == Path::new(META_FILE) {
      // The FutureResult only reports callback panics; nobody waits on it.
      drop(self.inner.watchers.broadcast());
    }
    Ok(())
  }

  fn sync_directory(&self) -> io::Result<()> {
    sync_dir(&self.inner.root)
  }

  fn acquire_lock(&self, lock: &Lock) -> Result<DirectoryLock, LockError> {
    let io = |e: io::Error| LockError::IoError(Arc::new(e));
    let full = self.resolve(&lock.filepath).map_err(io)?;
    // The lock file is plaintext but always empty.
    let file = OpenOptions::new()
      .read(true)
      .write(true)
      .create(true)
      .truncate(false)
      .mode(0o600)
      .open(&full)
      .map_err(io)?;
    let op = if lock.is_blocking {
      FlockOperation::LockExclusive
    } else {
      FlockOperation::NonBlockingLockExclusive
    };
    match rustix::fs::flock(&file, op) {
      Ok(()) => Ok(DirectoryLock::from(Box::new(file))),
      Err(rustix::io::Errno::WOULDBLOCK) => Err(LockError::LockBusy),
      Err(e) => Err(io(e.into())),
    }
  }

  fn watch(&self, watch_callback: WatchCallback) -> tantivy::Result<WatchHandle> {
    Ok(self.inner.watchers.subscribe(watch_callback))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use spool_crypto::{DataKey, Label};

  fn dir() -> (tempfile::TempDir, EncryptedDirectory) {
    let t = tempfile::tempdir().unwrap();
    let d = EncryptedDirectory::open(t.path(), DataKey::generate().derive(Label::Index)).unwrap();
    (t, d)
  }

  #[test]
  fn atomic_round_trip_and_ciphertext() {
    let (t, d) = dir();
    d.atomic_write(Path::new("meta.json"), b"{\"marker\":\"PLAINMARK\"}").unwrap();
    assert_eq!(d.atomic_read(Path::new("meta.json")).unwrap(), b"{\"marker\":\"PLAINMARK\"}");
    let raw = fs::read(t.path().join("meta.json")).unwrap();
    assert!(!raw.windows(9).any(|w| w == b"PLAINMARK"));
    // Overwrite works (atomic_write replaces).
    d.atomic_write(Path::new("meta.json"), b"two").unwrap();
    assert_eq!(d.atomic_read(Path::new("meta.json")).unwrap(), b"two");
    assert!(matches!(
      d.atomic_read(Path::new("nope.json")),
      Err(OpenReadError::FileDoesNotExist(_))
    ));
    // Bound to the path: a copy under another name does not open.
    fs::copy(t.path().join("meta.json"), t.path().join("other.json")).unwrap();
    assert!(d.atomic_read(Path::new("other.json")).is_err());
    assert_eq!(d.auth_failures(), 1);
    // No temp files left behind.
    let names: Vec<_> = fs::read_dir(t.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert!(names.iter().all(|n| !n.to_string_lossy().contains(TMP_MARKER)), "{names:?}");
  }

  #[test]
  fn write_once_files() {
    let (t, d) = dir();
    let p = Path::new("seg.idx");
    let mut w = d.open_write(p).unwrap();
    w.write_all(b"hello index").unwrap();
    // Invisible until terminated.
    assert!(!d.exists(p).unwrap());
    w.terminate().unwrap();
    assert!(d.exists(p).unwrap());
    let h = d.get_file_handle(p).unwrap();
    assert_eq!(h.len(), 11);
    assert_eq!(h.read_bytes(6..11).unwrap().as_slice(), b"index");
    assert!(h.read_bytes(6..12).is_err());
    assert!(matches!(d.open_write(p), Err(OpenWriteError::FileAlreadyExists(_))));
    // Dropped without terminate: nothing appears, temp removed.
    let mut w = d.open_write(Path::new("gone.idx")).unwrap();
    w.write_all(b"x").unwrap();
    drop(w);
    assert!(!d.exists(Path::new("gone.idx")).unwrap());
    assert_eq!(fs::read_dir(t.path()).unwrap().count(), 1);
    d.delete(p).unwrap();
    assert_eq!(d.cached_bytes(), 0);
    assert!(matches!(d.delete(p), Err(DeleteError::FileDoesNotExist(_))));
    assert!(matches!(d.get_file_handle(p), Err(OpenReadError::FileDoesNotExist(_))));
  }

  #[test]
  fn rejects_escaping_paths() {
    let (_t, d) = dir();
    assert!(d.open_write(Path::new("../x")).is_err());
    assert!(d.open_write(Path::new("/tmp/x")).is_err());
    assert!(d.atomic_write(Path::new("a/b"), b"").is_err());
  }

  #[test]
  fn lock_is_exclusive() {
    let (_t, d) = dir();
    let lock = Lock { filepath: PathBuf::from(".tantivy-writer.lock"), is_blocking: false };
    let held = d.acquire_lock(&lock).unwrap();
    // A second open file description (as another process would have) fails.
    assert!(matches!(d.acquire_lock(&lock), Err(LockError::LockBusy)));
    let other = d.clone();
    assert!(matches!(other.acquire_lock(&lock), Err(LockError::LockBusy)));
    drop(held);
    let _again = d.acquire_lock(&lock).unwrap();
  }

  #[test]
  fn watch_fires_on_meta_write() {
    let (_t, d) = dir();
    let hits = Arc::new(AtomicU64::new(0));
    let h2 = hits.clone();
    let handle = d.watch(WatchCallback::new(move || {
      h2.fetch_add(1, Ordering::SeqCst);
    }));
    d.atomic_write(Path::new(".managed.json"), b"[]").unwrap();
    d.atomic_write(Path::new("meta.json"), b"{}").unwrap();
    // Callbacks run on a spawned thread.
    for _ in 0..200 {
      if hits.load(Ordering::SeqCst) > 0 {
        break;
      }
      std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    drop(handle);
  }
}
