//! Durable, private file writes and bounded reads.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use rand::RngCore;

use crate::error::{Error, Result};

fn parent_dir(path: &Path) -> PathBuf {
  match path.parent() {
    Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
    _ => PathBuf::from("."),
  }
}

/// Write `bytes` to `path` atomically and durably:
/// create the parent dir (0700) if missing, write a mode-0600 temp file in the
/// same directory (`O_EXCL`), fsync it, rename over `path`, fsync the dir.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
  let dir = parent_dir(path);
  if !dir.exists() {
    DirBuilder::new()
      .recursive(true)
      .mode(0o700)
      .create(&dir)
      .map_err(|e| Error::io(format!("creating {}", dir.display()), e))?;
  }
  let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
  let mut suffix = [0u8; 8];
  rand::rng().fill_bytes(&mut suffix);
  let suffix: String = suffix.iter().map(|b| format!("{b:02x}")).collect();
  let tmp = dir.join(format!(".{name}.tmp-{suffix}"));

  let result = (|| {
    let mut f = OpenOptions::new()
      .write(true)
      .create_new(true)
      .mode(0o600)
      .open(&tmp)
      .map_err(|e| Error::io(format!("creating {}", tmp.display()), e))?;
    // Belt and braces against an unusual umask.
    f.set_permissions(fs::Permissions::from_mode(0o600))
      .map_err(|e| Error::io(format!("chmod {}", tmp.display()), e))?;
    f.write_all(bytes).map_err(|e| Error::io(format!("writing {}", tmp.display()), e))?;
    f.sync_all().map_err(|e| Error::io(format!("fsync {}", tmp.display()), e))?;
    drop(f);
    fs::rename(&tmp, path)
      .map_err(|e| Error::io(format!("renaming {} -> {}", tmp.display(), path.display()), e))
  })();
  if result.is_err() {
    let _ = fs::remove_file(&tmp);
    return result;
  }
  sync_dir(&dir)
}

pub(crate) fn sync_dir(dir: &Path) -> Result<()> {
  File::open(dir)
    .and_then(|d| d.sync_all())
    .map_err(|e| Error::io(format!("fsync dir {}", dir.display()), e))
}

/// Remove `path` and fsync its directory. Missing file is not an error.
pub(crate) fn remove_durable(path: &Path) -> Result<()> {
  match fs::remove_file(path) {
    Ok(()) => sync_dir(&parent_dir(path)),
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
    Err(e) => Err(Error::io(format!("removing {}", path.display()), e)),
  }
}

/// Read a regular file of at most `limit` bytes.
pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
  let f = File::open(path).map_err(|e| {
    if e.kind() == std::io::ErrorKind::NotFound {
      Error::NotFound(path.to_path_buf())
    } else {
      Error::io(format!("opening {}", path.display()), e)
    }
  })?;
  let meta = f.metadata().map_err(|e| Error::io(format!("stat {}", path.display()), e))?;
  if !meta.is_file() {
    return Err(Error::corrupt(path, "not a regular file"));
  }
  if meta.len() > limit {
    return Err(Error::TooLarge { path: path.to_path_buf(), size: meta.len() });
  }
  if meta.permissions().mode() & 0o077 != 0 {
    tracing::warn!(path = %path.display(), "keyslots file is accessible by group/other; expected mode 0600");
  }
  let mut buf = Vec::new();
  f.take(limit + 1)
    .read_to_end(&mut buf)
    .map_err(|e| Error::io(format!("reading {}", path.display()), e))?;
  if buf.len() as u64 > limit {
    return Err(Error::TooLarge { path: path.to_path_buf(), size: buf.len() as u64 });
  }
  Ok(buf)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn atomic_write_is_private_and_leaves_no_temp() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("sub/keyslots.json");
    write_atomic(&p, b"one").unwrap();
    write_atomic(&p, b"two").unwrap();
    assert_eq!(fs::read(&p).unwrap(), b"two");
    assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(p.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    let names: Vec<_> =
      fs::read_dir(p.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names.len(), 1);
  }

  #[test]
  fn bounded_read_limits() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("f");
    fs::write(&p, vec![b'x'; 11]).unwrap();
    assert_eq!(read_bounded(&p, 11).unwrap().len(), 11);
    assert!(matches!(read_bounded(&p, 10), Err(Error::TooLarge { size: 11, .. })));
    assert!(matches!(read_bounded(&d.path().join("nope"), 10), Err(Error::NotFound(_))));
    assert!(matches!(read_bounded(d.path(), 10), Err(Error::Corrupt { .. })));
  }
}
