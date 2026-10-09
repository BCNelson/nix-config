//! Daemon-side filesystem locations.

use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::Context;

/// State directory: `override_` (from `--state-dir` / `SPOOL_STATE_DIR`) or
/// `$XDG_STATE_HOME/spool` (falls back to `~/.local/state/spool`). Holds
/// `history.db` (+ WAL), `blobs/`, `history.db.kcv` and `keyslots.json`.
pub fn state_dir(override_: Option<PathBuf>) -> anyhow::Result<PathBuf> {
  if let Some(p) = override_ {
    return Ok(p);
  }
  let base = std::env::var_os("XDG_STATE_HOME")
    .filter(|v| !v.is_empty())
    .map(PathBuf::from)
    .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
    .ok_or_else(|| anyhow::anyhow!("neither XDG_STATE_HOME nor HOME is set"))?;
  Ok(base.join("spool"))
}

/// Suffix for a quarantined M1 plaintext database.
pub const PLAINTEXT_BACKUP_SUFFIX: &str = ".m1-plaintext.bak";

/// True if `path` starts with the plaintext SQLite header.
pub fn is_plaintext_sqlite(path: &Path) -> std::io::Result<bool> {
  use std::io::Read;
  let mut head = [0u8; 16];
  let mut f = match std::fs::File::open(path) {
    Ok(f) => f,
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
    Err(e) => return Err(e),
  };
  let mut n = 0;
  while n < head.len() {
    match f.read(&mut head[n..])? {
      0 => break,
      k => n += k,
    }
  }
  Ok(n == head.len() && &head == b"SQLite format 3\0")
}

/// If `dir/<db_name>` is a plaintext SQLite database (M1 development data
/// at the production path), rename it and its `-wal` / `-shm` /
/// `-journal` files to `<name>.m1-plaintext.bak[-wal|-shm|-journal]` so a
/// fresh encrypted database can be created. Never deletes anything; refuses
/// to overwrite an existing backup. Returns the backup path if it moved one.
pub fn quarantine_plaintext_db(dir: &Path, db_name: &str) -> anyhow::Result<Option<PathBuf>> {
  let db = dir.join(db_name);
  if !is_plaintext_sqlite(&db).with_context(|| format!("reading {}", db.display()))? {
    return Ok(None);
  }
  let backup = dir.join(format!("{db_name}{PLAINTEXT_BACKUP_SUFFIX}"));
  if backup.symlink_metadata().is_ok() {
    anyhow::bail!(
      "{} is an unencrypted database and {} already exists; delete one of them",
      db.display(),
      backup.display()
    );
  }
  // Side files first, the main file last: a crash in between leaves a
  // plaintext main DB that the next start quarantines again.
  for suffix in ["-wal", "-shm", "-journal"] {
    let from = dir.join(format!("{db_name}{suffix}"));
    let to = dir.join(format!("{db_name}{PLAINTEXT_BACKUP_SUFFIX}{suffix}"));
    match std::fs::rename(&from, &to) {
      Ok(()) => {}
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
      Err(e) => return Err(e).with_context(|| format!("renaming {}", from.display())),
    }
  }
  std::fs::rename(&db, &backup).with_context(|| format!("renaming {}", db.display()))?;
  Ok(Some(backup))
}

/// The state directory's exclusive lock ([`lock_state_dir`]); released on
/// drop. The lock file itself stays.
#[derive(Debug)]
pub struct StateLock {
  _file: std::fs::File,
}

/// Take the non-blocking exclusive `flock` on `<dir>/state.lock`
/// ([`spool_core::store::STATE_LOCK_FILE_NAME`]), which `spool-keyctl`
/// also takes: spoold refuses to start while key slots are being changed,
/// and spool-keyctl refuses while spoold runs, even when the two use
/// different socket paths. `dir` must exist.
pub fn lock_state_dir(dir: &Path) -> anyhow::Result<StateLock> {
  use std::os::unix::fs::OpenOptionsExt;
  let path = dir.join(spool_core::store::STATE_LOCK_FILE_NAME);
  let file = std::fs::OpenOptions::new()
    .read(true)
    .write(true)
    .create(true)
    .truncate(false)
    .mode(0o600)
    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
    .open(&path)
    .with_context(|| format!("opening lock file {}", path.display()))?;
  match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
    Ok(()) => Ok(StateLock { _file: file }),
    Err(rustix::io::Errno::WOULDBLOCK) => anyhow::bail!(
      "the state directory {} is in use (lock {} is held by spool-keyctl or another spoold)",
      dir.display(),
      path.display()
    ),
    Err(e) => Err(std::io::Error::from(e)).with_context(|| format!("flock {}", path.display())),
  }
}

/// Create `dir` (and parents) if missing and ensure it is owned by us with
/// mode 0700 (chmod if needed; error if owned by another uid or a symlink).
pub fn ensure_private_dir(dir: &Path) -> anyhow::Result<()> {
  ensure_dir(dir, ModePolicy::Fix)
}

/// Like [`ensure_private_dir`] but refuses (instead of fixing) an existing
/// directory whose mode is not exactly 0700. Used for the socket directory:
/// a loosened mode there means something else touched it.
pub fn ensure_strict_private_dir(dir: &Path) -> anyhow::Result<()> {
  ensure_dir(dir, ModePolicy::Refuse)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModePolicy {
  Fix,
  Refuse,
}

/// The parts of `lstat` the private-dir check looks at (injectable for tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirMeta {
  pub is_symlink: bool,
  pub is_dir: bool,
  pub uid: u32,
  pub mode: u32,
}

impl DirMeta {
  fn from_lstat(md: &std::fs::Metadata) -> Self {
    Self {
      is_symlink: md.file_type().is_symlink(),
      is_dir: md.is_dir(),
      uid: md.uid(),
      mode: md.mode() & 0o7777,
    }
  }
}

/// Verdict on an existing directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirVerdict {
  Ok,
  /// Ours, but mode is not 0700 (shown).
  WrongMode(u32),
  Bad(String),
}

pub fn assess_private_dir(meta: &DirMeta, our_uid: u32) -> DirVerdict {
  if meta.is_symlink {
    return DirVerdict::Bad("is a symlink".into());
  }
  if !meta.is_dir {
    return DirVerdict::Bad("is not a directory".into());
  }
  if meta.uid != our_uid {
    return DirVerdict::Bad(format!("is owned by uid {}, not {our_uid}", meta.uid));
  }
  if meta.mode != 0o700 {
    return DirVerdict::WrongMode(meta.mode);
  }
  DirVerdict::Ok
}

fn ensure_dir(dir: &Path, policy: ModePolicy) -> anyhow::Result<()> {
  match std::fs::symlink_metadata(dir) {
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
      std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("creating {}", dir.display()))?;
      // recursive create tolerates races; re-check what is there now and
      // fix the mode of the leaf we just made regardless of umask.
      check_existing(dir, ModePolicy::Fix)
    }
    Err(e) => Err(e).with_context(|| format!("stat {}", dir.display())),
    Ok(_) => check_existing(dir, policy),
  }
}

fn check_existing(dir: &Path, policy: ModePolicy) -> anyhow::Result<()> {
  let md = std::fs::symlink_metadata(dir).with_context(|| format!("stat {}", dir.display()))?;
  match assess_private_dir(&DirMeta::from_lstat(&md), crate::security::our_uid()) {
    DirVerdict::Ok => Ok(()),
    DirVerdict::WrongMode(mode) if policy == ModePolicy::Fix => {
      tracing::warn!(dir = %dir.display(), "fixing directory mode {mode:04o} -> 0700");
      std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("chmod 0700 {}", dir.display()))
    }
    DirVerdict::WrongMode(mode) => {
      anyhow::bail!("refusing to use {}: mode is {mode:04o}, expected 0700", dir.display())
    }
    DirVerdict::Bad(why) => anyhow::bail!("refusing to use {}: it {why}", dir.display()),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn meta(uid: u32, mode: u32) -> DirMeta {
    DirMeta { is_symlink: false, is_dir: true, uid, mode }
  }

  #[test]
  fn assess_rules() {
    assert_eq!(assess_private_dir(&meta(1000, 0o700), 1000), DirVerdict::Ok);
    assert_eq!(assess_private_dir(&meta(1000, 0o755), 1000), DirVerdict::WrongMode(0o755));
    assert!(matches!(assess_private_dir(&meta(0, 0o700), 1000), DirVerdict::Bad(_)));
    let link = DirMeta { is_symlink: true, ..meta(1000, 0o700) };
    assert!(matches!(assess_private_dir(&link, 1000), DirVerdict::Bad(_)));
    let file = DirMeta { is_dir: false, ..meta(1000, 0o700) };
    assert!(matches!(assess_private_dir(&file, 1000), DirVerdict::Bad(_)));
  }

  #[test]
  fn creates_and_fixes() {
    let d = tempfile::tempdir().unwrap();
    let nested = d.path().join("a/b/spool");
    ensure_private_dir(&nested).unwrap();
    let mode = std::fs::metadata(&nested).unwrap().mode() & 0o7777;
    assert_eq!(mode, 0o700);

    std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Strict refuses, lenient fixes.
    let err = ensure_strict_private_dir(&nested).unwrap_err().to_string();
    assert!(err.contains("0755"), "{err}");
    ensure_private_dir(&nested).unwrap();
    assert_eq!(std::fs::metadata(&nested).unwrap().mode() & 0o7777, 0o700);
    ensure_strict_private_dir(&nested).unwrap();
  }

  #[test]
  fn plaintext_db_is_quarantined() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    assert_eq!(quarantine_plaintext_db(dir, "history.db").unwrap(), None);
    // Not SQLite plaintext (e.g. SQLCipher: random first page): untouched.
    std::fs::write(dir.join("history.db"), [0x5a; 4096]).unwrap();
    assert_eq!(quarantine_plaintext_db(dir, "history.db").unwrap(), None);
    assert!(dir.join("history.db").exists());

    let mut plain = b"SQLite format 3\0".to_vec();
    plain.resize(4096, 0);
    std::fs::write(dir.join("history.db"), &plain).unwrap();
    std::fs::write(dir.join("history.db-wal"), b"wal").unwrap();
    let backup = quarantine_plaintext_db(dir, "history.db").unwrap().unwrap();
    assert_eq!(backup, dir.join("history.db.m1-plaintext.bak"));
    assert_eq!(std::fs::read(&backup).unwrap(), plain);
    assert_eq!(std::fs::read(dir.join("history.db.m1-plaintext.bak-wal")).unwrap(), b"wal");
    assert!(!dir.join("history.db").exists() && !dir.join("history.db-wal").exists());

    // A second plaintext DB never overwrites the backup.
    std::fs::write(dir.join("history.db"), &plain).unwrap();
    assert!(quarantine_plaintext_db(dir, "history.db").is_err());
    assert!(dir.join("history.db").exists());
  }

  #[test]
  fn state_lock_is_exclusive() {
    let d = tempfile::tempdir().unwrap();
    let held = lock_state_dir(d.path()).unwrap();
    let err = lock_state_dir(d.path()).unwrap_err().to_string();
    assert!(err.contains("is in use"), "{err}");
    drop(held);
    let _again = lock_state_dir(d.path()).unwrap();
    assert!(d.path().join(spool_core::store::STATE_LOCK_FILE_NAME).is_file());
  }

  #[test]
  fn refuses_symlink_and_file() {
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real");
    ensure_private_dir(&real).unwrap();
    let link = d.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(ensure_private_dir(&link).unwrap_err().to_string().contains("symlink"));
    let file = d.path().join("file");
    std::fs::write(&file, b"").unwrap();
    assert!(ensure_strict_private_dir(&file).is_err());
  }
}
