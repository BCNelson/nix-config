//! Locations inside the Spool state directory, resolved exactly like
//! `spoold` does (`--state-dir` / `SPOOL_STATE_DIR`, else
//! `$XDG_STATE_HOME/spool`, else `~/.local/state/spool`).

use std::path::{Path, PathBuf};

use anyhow::Context as _;

/// `spoold`'s on-disk search index directory (`spoold::index::INDEX_DIR_NAME`).
pub const INDEX_DIR_NAME: &str = "index";
/// Suffix of a rotation's staged slot file (`keyslots.json.next`).
pub const NEXT_SUFFIX: &str = ".next";
/// Suffix of the copy of the pre-rotation slot file kept until its provider
/// secrets are destroyed (`keyslots.json.old`).
pub const OLD_SUFFIX: &str = ".old";
/// spoold's quarantined M1 plaintext database (`paths::PLAINTEXT_BACKUP_SUFFIX`).
pub const PLAINTEXT_BACKUP_SUFFIX: &str = ".m1-plaintext.bak";

/// State directory: `override_` or `$XDG_STATE_HOME/spool` (falls back to
/// `~/.local/state/spool`). Same rules as `spoold::paths::state_dir`.
pub fn state_dir(override_: Option<PathBuf>) -> anyhow::Result<PathBuf> {
  if let Some(p) = override_.filter(|p| !p.as_os_str().is_empty()) {
    return Ok(p);
  }
  let base = std::env::var_os("XDG_STATE_HOME")
    .filter(|v| !v.is_empty())
    .map(PathBuf::from)
    .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
    .ok_or_else(|| anyhow::anyhow!("neither XDG_STATE_HOME nor HOME is set"))?;
  Ok(base.join("spool"))
}

/// Every path this tool reads or writes, derived from the state directory.
#[derive(Debug, Clone)]
pub struct StatePaths {
  /// The state directory itself.
  pub dir: PathBuf,
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
  let mut s = p.as_os_str().to_owned();
  s.push(suffix);
  PathBuf::from(s)
}

impl StatePaths {
  /// Paths under `dir`.
  pub fn new(dir: impl Into<PathBuf>) -> Self {
    StatePaths { dir: dir.into() }
  }
  /// `keyslots.json`.
  pub fn keyslots(&self) -> PathBuf {
    self.dir.join(spool_keys::FILE_NAME)
  }
  /// `keyslots.json.next`: staged slots of an unfinished rotation.
  pub fn keyslots_next(&self) -> PathBuf {
    with_suffix(&self.keyslots(), NEXT_SUFFIX)
  }
  /// `keyslots.json.old`: pre-rotation slots whose secrets are still to be
  /// destroyed.
  pub fn keyslots_old(&self) -> PathBuf {
    with_suffix(&self.keyslots(), OLD_SUFFIX)
  }
  /// `state.lock`: spoold holds it while running; spool-keyctl takes it
  /// for mutating commands.
  pub fn state_lock(&self) -> PathBuf {
    self.dir.join(spool_core::store::STATE_LOCK_FILE_NAME)
  }
  /// `history.db`.
  pub fn db(&self) -> PathBuf {
    self.dir.join(spool_core::store::DB_FILE_NAME)
  }
  /// `blobs/`.
  pub fn blobs(&self) -> PathBuf {
    self.dir.join(spool_core::store::BLOB_DIR_NAME)
  }
  /// `index/` (spool-search).
  pub fn index(&self) -> PathBuf {
    self.dir.join(INDEX_DIR_NAME)
  }
  /// `index.rebuild/` (spool-search's staging directory).
  pub fn index_rebuild(&self) -> PathBuf {
    with_suffix(&self.index(), ".rebuild")
  }
  /// `index.lock` (spool-search's writer lock, held by spoold while its
  /// on-disk index is open).
  pub fn index_lock(&self) -> PathBuf {
    with_suffix(&self.index(), ".lock")
  }
  /// spoold's quarantined plaintext M1 database and its side files.
  pub fn plaintext_backups(&self) -> Vec<PathBuf> {
    let base =
      self.dir.join(format!("{}{PLAINTEXT_BACKUP_SUFFIX}", spool_core::store::DB_FILE_NAME));
    let mut v = vec![base.clone()];
    for s in ["-wal", "-shm", "-journal"] {
      v.push(with_suffix(&base, s));
    }
    v
  }
  /// True if `history.db` exists.
  pub fn has_db(&self) -> bool {
    self.db().symlink_metadata().is_ok()
  }
  /// True if the index directory exists.
  pub fn has_index(&self) -> bool {
    self.index().symlink_metadata().is_ok() || self.index_rebuild().symlink_metadata().is_ok()
  }
}

/// fsync a directory (makes renames / unlinks in it durable).
pub fn fsync_dir(dir: &Path) -> anyhow::Result<()> {
  std::fs::File::open(dir)
    .and_then(|f| f.sync_all())
    .with_context(|| format!("fsync {}", dir.display()))
}

fn parent(p: &Path) -> &Path {
  p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

/// Durably copy `from` to `to` (0600 temp file, fsync, rename, fsync dir).
pub fn copy_durable(from: &Path, to: &Path) -> anyhow::Result<()> {
  use std::io::Write as _;
  use std::os::unix::fs::OpenOptionsExt as _;
  let bytes = std::fs::read(from).with_context(|| format!("reading {}", from.display()))?;
  let tmp = with_suffix(to, ".tmp");
  let _ = std::fs::remove_file(&tmp);
  let mut f = std::fs::OpenOptions::new()
    .write(true)
    .create_new(true)
    .mode(0o600)
    .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
    .open(&tmp)
    .with_context(|| format!("creating {}", tmp.display()))?;
  f.write_all(&bytes)
    .and_then(|()| f.sync_all())
    .with_context(|| format!("writing {}", tmp.display()))?;
  drop(f);
  std::fs::rename(&tmp, to)
    .with_context(|| format!("renaming {} to {}", tmp.display(), to.display()))?;
  fsync_dir(parent(to))
}

/// Durably rename `from` over `to` (atomic replace, then fsync the dir).
pub fn rename_durable(from: &Path, to: &Path) -> anyhow::Result<()> {
  std::fs::rename(from, to)
    .with_context(|| format!("renaming {} to {}", from.display(), to.display()))?;
  fsync_dir(parent(to))
}

/// Remove a file if it exists, then fsync its directory.
pub fn remove_file_durable(p: &Path) -> anyhow::Result<bool> {
  match std::fs::remove_file(p) {
    Ok(()) => {
      fsync_dir(parent(p))?;
      Ok(true)
    }
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
    Err(e) => Err(e).with_context(|| format!("removing {}", p.display())),
  }
}

/// Remove a directory tree if it exists (refuses to follow a symlink: a
/// symlink is removed itself).
pub fn remove_tree(p: &Path) -> anyhow::Result<bool> {
  match p.symlink_metadata() {
    Ok(md) if md.is_dir() => {
      std::fs::remove_dir_all(p).with_context(|| format!("removing {}", p.display()))?;
      fsync_dir(parent(p))?;
      Ok(true)
    }
    Ok(_) => remove_file_durable(p),
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
    Err(e) => Err(e).with_context(|| format!("stat {}", p.display())),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn override_wins_and_names() {
    let d = state_dir(Some("/x/y".into())).unwrap();
    assert_eq!(d, PathBuf::from("/x/y"));
    let p = StatePaths::new(&d);
    assert_eq!(p.keyslots(), Path::new("/x/y/keyslots.json"));
    assert_eq!(p.keyslots_next(), Path::new("/x/y/keyslots.json.next"));
    assert_eq!(p.keyslots_old(), Path::new("/x/y/keyslots.json.old"));
    assert_eq!(p.index(), Path::new("/x/y/index"));
    assert_eq!(p.index_rebuild(), Path::new("/x/y/index.rebuild"));
    assert_eq!(p.index_lock(), Path::new("/x/y/index.lock"));
    assert_eq!(p.plaintext_backups()[0], Path::new("/x/y/history.db.m1-plaintext.bak"));
  }
}
