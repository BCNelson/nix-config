//! Exclusion against a running `spoold`.
//!
//! `spoold` takes non-blocking exclusive `flock`s on `<socket>.lock`
//! (`$XDG_RUNTIME_DIR/spool/sock.lock`, or `$SPOOL_SOCKET.lock`) and on
//! `<state>/state.lock` for its whole lifetime, and spool-search holds
//! `<state>/index.lock` while the on-disk index is open. `spool-keyctl` takes
//! **all three** (non-blocking) and keeps them until it exits, so a running
//! daemon makes it refuse, and a daemon started meanwhile fails its
//! single-instance or state-lock check instead of opening the store
//! mid-operation. The state lock makes this independent of the socket path
//! (`SPOOL_SOCKET`) either side was started with.

use std::fs::File;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use rustix::fs::{FlockOperation, OFlags, flock};

/// Why the lock could not be taken.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
  /// Somebody (spoold) holds it.
  #[error("spoold appears to be running ({} is locked); stop it first: systemctl --user stop spool", .0.display())]
  Held(PathBuf),
  /// Filesystem problem.
  #[error("{context}: {source}")]
  Io {
    /// What was being done.
    context: String,
    /// Cause.
    #[source]
    source: std::io::Error,
  },
}

fn io(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> LockError {
  let context = context.into();
  move |source| LockError::Io { context, source }
}

/// The held locks. Released on drop.
#[derive(Debug)]
pub struct DaemonLock {
  _files: Vec<File>,
}

/// `<socket>.lock`, as spoold computes it.
pub fn socket_lock_path(socket: &Path) -> PathBuf {
  let mut s = socket.as_os_str().to_owned();
  s.push(".lock");
  PathBuf::from(s)
}

fn open_lock_file(path: &Path) -> Result<File, LockError> {
  std::fs::OpenOptions::new()
    .read(true)
    .write(true)
    .create(true)
    .truncate(false)
    .mode(0o600)
    .custom_flags((OFlags::NOFOLLOW | OFlags::CLOEXEC).bits() as i32)
    .open(path)
    .map_err(io(format!("opening lock file {}", path.display())))
}

fn try_lock(path: &Path) -> Result<File, LockError> {
  let f = open_lock_file(path)?;
  match flock(&f, FlockOperation::NonBlockingLockExclusive) {
    Ok(()) => Ok(f),
    Err(rustix::io::Errno::WOULDBLOCK) => Err(LockError::Held(path.to_path_buf())),
    Err(e) => Err(io(format!("flock {}", path.display()))(e.into())),
  }
}

/// Create `dir` with mode 0700 if missing (like spoold's socket directory);
/// an existing directory must be ours and not a symlink.
fn ensure_private_dir(dir: &Path) -> Result<(), LockError> {
  match dir.symlink_metadata() {
    Ok(md) => {
      if md.file_type().is_symlink() || !md.is_dir() {
        return Err(LockError::Io {
          context: format!("{} is not a directory", dir.display()),
          source: std::io::Error::other("refusing to use it"),
        });
      }
      if md.uid() != rustix::process::getuid().as_raw() {
        return Err(LockError::Io {
          context: format!("{} is owned by another user", dir.display()),
          source: std::io::Error::other("refusing to use it"),
        });
      }
      Ok(())
    }
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::DirBuilder::new()
      .recursive(true)
      .mode(0o700)
      .create(dir)
      .map_err(io(format!("creating {}", dir.display()))),
    Err(e) => Err(io(format!("stat {}", dir.display()))(e)),
  }
}

impl DaemonLock {
  /// Take spoold's single-instance lock for `socket` and the state and
  /// index locks in `state_dir`. Creates the socket's directory (0700) if needed; the state
  /// directory must exist already if `state_dir_lock` is set.
  pub fn acquire(socket: &Path, state_dir: Option<&Path>) -> Result<DaemonLock, LockError> {
    let mut files = Vec::new();
    if let Some(parent) = socket.parent().filter(|p| !p.as_os_str().is_empty()) {
      ensure_private_dir(parent)?;
    }
    files.push(try_lock(&socket_lock_path(socket))?);
    if let Some(dir) = state_dir {
      let paths = crate::paths::StatePaths::new(dir);
      files.push(try_lock(&paths.state_lock())?);
      files.push(try_lock(&paths.index_lock())?);
    }
    Ok(DaemonLock { _files: files })
  }

  /// True if the locks are currently held by someone else (probe: take and
  /// release immediately). For `status`, which does not need exclusion.
  pub fn probe(socket: &Path, state_dir: &Path) -> Option<PathBuf> {
    let paths = crate::paths::StatePaths::new(state_dir);
    let candidates = [socket_lock_path(socket), paths.state_lock(), paths.index_lock()];
    for p in candidates {
      // Only probe existing files: never create anything for status.
      if p.symlink_metadata().is_err() {
        continue;
      }
      if let Err(LockError::Held(p)) = try_lock(&p) {
        return Some(p);
      }
    }
    None
  }
}
