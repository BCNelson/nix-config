//! Well-known runtime paths shared by `spoold` and its clients.

use std::env;
use std::path::PathBuf;

/// Environment override for the public socket path. Intended for tests and
/// for running a development daemon next to a real one; both `spoold` and
/// `spoolctl` honour it.
pub const SOCKET_ENV: &str = "SPOOL_SOCKET";

/// Directory under `$XDG_RUNTIME_DIR` holding the socket (mode 0700).
pub const RUNTIME_SUBDIR: &str = "spool";

/// Socket file name inside [`RUNTIME_SUBDIR`].
pub const SOCKET_NAME: &str = "sock";

#[derive(Debug, thiserror::Error)]
pub enum PathError {
  #[error("XDG_RUNTIME_DIR is not set (and {SOCKET_ENV} is not set)")]
  NoRuntimeDir,
  #[error("{0} must be an absolute path")]
  NotAbsolute(&'static str),
}

/// Public socket path: `$SPOOL_SOCKET` if set (must be absolute), else
/// `$XDG_RUNTIME_DIR/spool/sock`.
pub fn public_socket_path() -> Result<PathBuf, PathError> {
  socket_path_from(env::var_os(SOCKET_ENV), env::var_os("XDG_RUNTIME_DIR"))
}

fn socket_path_from(
  override_: Option<std::ffi::OsString>,
  runtime: Option<std::ffi::OsString>,
) -> Result<PathBuf, PathError> {
  if let Some(p) = override_.filter(|p| !p.is_empty()) {
    let p = PathBuf::from(p);
    return if p.is_absolute() { Ok(p) } else { Err(PathError::NotAbsolute(SOCKET_ENV)) };
  }
  let rt = runtime.filter(|p| !p.is_empty()).ok_or(PathError::NoRuntimeDir)?;
  let rt = PathBuf::from(rt);
  if !rt.is_absolute() {
    return Err(PathError::NotAbsolute("XDG_RUNTIME_DIR"));
  }
  Ok(rt.join(RUNTIME_SUBDIR).join(SOCKET_NAME))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn default_path() {
    let p = socket_path_from(None, Some("/run/user/1000".into())).unwrap();
    assert_eq!(p, PathBuf::from("/run/user/1000/spool/sock"));
  }

  #[test]
  fn override_wins() {
    let p = socket_path_from(Some("/tmp/x.sock".into()), Some("/run/user/1".into())).unwrap();
    assert_eq!(p, PathBuf::from("/tmp/x.sock"));
  }

  #[test]
  fn errors() {
    assert!(matches!(socket_path_from(None, None), Err(PathError::NoRuntimeDir)));
    assert!(matches!(socket_path_from(Some("rel".into()), None), Err(PathError::NotAbsolute(_))));
    assert!(matches!(socket_path_from(None, Some("rel".into())), Err(PathError::NotAbsolute(_))));
  }
}
