//! Process hardening and peer checks.

use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Set to `1` to start despite an unsafe PATH (development only; logged at
/// error level on every start).
pub const INSECURE_PATH_ENV: &str = "SPOOL_INSECURE_PATH_OK";

/// Store prefix whose contents are immutable and therefore always trusted.
const NIX_STORE: &str = "/nix/store";

/// `umask(0o077)`: files/dirs we create are private by default.
pub fn set_private_umask() {
  // SAFETY: umask has no memory-safety preconditions.
  unsafe {
    libc::umask(0o077);
  }
}

/// `prctl(PR_SET_DUMPABLE, 0)`: no core dumps, no same-uid ptrace.
pub fn disable_dumpable() -> io::Result<()> {
  // SAFETY: PR_SET_DUMPABLE takes plain integer arguments.
  let r = unsafe {
    libc::prctl(
      libc::PR_SET_DUMPABLE,
      0 as libc::c_ulong,
      0 as libc::c_ulong,
      0 as libc::c_ulong,
      0 as libc::c_ulong,
    )
  };
  if r == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Our effective uid.
pub fn our_uid() -> u32 {
  rustix::process::geteuid().as_raw()
}

/// Why a PATH entry is unsafe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathProblemKind {
  /// Relative or empty entry (resolves against the cwd).
  NotAbsolute,
  /// Neither owned by root nor inside `/nix/store`.
  NotRootOwned { uid: u32 },
  /// Group- or world-writable (mode shown).
  Writable { mode: u32 },
  /// stat failed (missing dirs are *not* a problem and are skipped).
  Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathProblem {
  pub dir: PathBuf,
  pub kind: PathProblemKind,
}

impl std::fmt::Display for PathProblem {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let d = self.dir.display();
    match &self.kind {
      PathProblemKind::NotAbsolute => write!(f, "PATH entry {d:?} is not an absolute path"),
      PathProblemKind::NotRootOwned { uid } => {
        write!(f, "PATH directory {d} is owned by uid {uid}, not root")
      }
      PathProblemKind::Writable { mode } => {
        write!(f, "PATH directory {d} is group/world-writable (mode {mode:04o})")
      }
      PathProblemKind::Unreadable(e) => write!(f, "PATH directory {d} cannot be checked: {e}"),
    }
  }
}

/// What the PATH audit needs to know about one entry, after following
/// symlinks. Separate from `std::fs::Metadata` so the ownership/mode logic
/// is testable without root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirInfo {
  /// Fully resolved path (symlinks followed).
  pub resolved: PathBuf,
  pub uid: u32,
  /// `st_mode` (permission bits are what matter).
  pub mode: u32,
}

/// Real filesystem lookup for [`check_path_dirs_with`].
fn stat_dir(dir: &Path) -> io::Result<DirInfo> {
  let resolved = std::fs::canonicalize(dir)?;
  let md = std::fs::metadata(&resolved)?;
  Ok(DirInfo { resolved, uid: md.uid(), mode: md.mode() })
}

/// Audit a PATH value: every existing directory must be root-owned or under
/// `/nix/store`, and not group/world-writable (symlinks are resolved; the
/// check applies to the target). Nonexistent entries are skipped. Pure
/// apart from `stat` calls, so it is unit-testable with tempdirs.
pub fn check_path_dirs(path_var: &OsStr) -> Vec<PathProblem> {
  check_path_dirs_with(path_var, stat_dir)
}

/// [`check_path_dirs`] with an injectable lookup (`NotFound` = skip).
pub fn check_path_dirs_with(
  path_var: &OsStr,
  lookup: impl Fn(&Path) -> io::Result<DirInfo>,
) -> Vec<PathProblem> {
  let mut problems = Vec::new();
  for raw in path_var.as_bytes().split(|b| *b == b':') {
    let dir = PathBuf::from(OsStr::from_bytes(raw));
    if raw.is_empty() || !dir.is_absolute() {
      problems.push(PathProblem { dir, kind: PathProblemKind::NotAbsolute });
      continue;
    }
    let info = match lookup(&dir) {
      Ok(i) => i,
      Err(e) if e.kind() == io::ErrorKind::NotFound => {
        tracing::debug!(dir = %dir.display(), "PATH entry does not exist; ignored");
        continue;
      }
      Err(e) => {
        problems.push(PathProblem { dir, kind: PathProblemKind::Unreadable(e.to_string()) });
        continue;
      }
    };
    if let Some(kind) = assess_dir(&info) {
      // Name the resolved directory when a symlink led somewhere else.
      let dir = if info.resolved != dir { info.resolved } else { dir };
      problems.push(PathProblem { dir, kind });
    }
  }
  problems
}

/// Ownership/mode verdict for one resolved PATH directory.
fn assess_dir(info: &DirInfo) -> Option<PathProblemKind> {
  // Store paths are immutable (read-only bind mount, root-owned, 0555);
  // trust them regardless of what the stat says.
  if info.resolved.starts_with(NIX_STORE) {
    return None;
  }
  if info.uid != 0 {
    return Some(PathProblemKind::NotRootOwned { uid: info.uid });
  }
  if info.mode & 0o022 != 0 {
    return Some(PathProblemKind::Writable { mode: info.mode & 0o7777 });
  }
  None
}

/// Run [`check_path_dirs`] on `$PATH`; on problems, log each one and return
/// an error unless `SPOOL_INSECURE_PATH_OK=1`, in which case log loudly at
/// error level and continue.
pub fn enforce_path_policy() -> anyhow::Result<()> {
  let path = std::env::var_os("PATH").unwrap_or_default();
  let problems = check_path_dirs(&path);
  let insecure_ok = std::env::var_os(INSECURE_PATH_ENV).is_some_and(|v| v == "1");
  evaluate_path_problems(&problems, insecure_ok)
}

fn evaluate_path_problems(problems: &[PathProblem], insecure_ok: bool) -> anyhow::Result<()> {
  if problems.is_empty() {
    return Ok(());
  }
  for p in problems {
    tracing::error!("unsafe PATH: {p}");
  }
  if insecure_ok {
    tracing::error!(
      "!!! {INSECURE_PATH_ENV}=1: starting despite {} unsafe PATH entr{} — a writable PATH \
       directory lets other code hijack helpers spoold runs. Development use only. !!!",
      problems.len(),
      if problems.len() == 1 { "y" } else { "ies" }
    );
    return Ok(());
  }
  anyhow::bail!(
    "refusing to start: {} (set {INSECURE_PATH_ENV}=1 to override for development)",
    problems.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")
  )
}

/// Find executable `bin` on `$PATH`, using only directories that pass the
/// PATH audit ([`check_path_dirs`]), and only a file that is itself
/// trustworthy (under `/nix/store`, or root-owned and not group/world
/// writable, symlinks resolved). Never the current directory, never a path
/// from a config file. With `SPOOL_INSECURE_PATH_OK=1` (development) the
/// ownership checks are skipped like the startup audit. Returns the
/// canonical path.
pub fn find_on_audited_path(bin: &str) -> Option<PathBuf> {
  let path = std::env::var_os("PATH").unwrap_or_default();
  let insecure_ok = std::env::var_os(INSECURE_PATH_ENV).is_some_and(|v| v == "1");
  find_on_path_with(&path, bin, insecure_ok, stat_dir)
}

fn find_on_path_with(
  path_var: &OsStr,
  bin: &str,
  insecure_ok: bool,
  lookup: impl Fn(&Path) -> io::Result<DirInfo>,
) -> Option<PathBuf> {
  if bin.is_empty() || bin.contains('/') {
    return None;
  }
  for raw in path_var.as_bytes().split(|b| *b == b':') {
    let dir = PathBuf::from(OsStr::from_bytes(raw));
    if raw.is_empty() || !dir.is_absolute() {
      continue;
    }
    if !insecure_ok && !check_path_dirs_with(dir.as_os_str(), &lookup).is_empty() {
      continue;
    }
    let candidate = dir.join(bin);
    let Ok(info) = lookup(&candidate) else { continue };
    let is_exec_file = info.mode & 0o170000 == 0o100000 && info.mode & 0o111 != 0;
    if !is_exec_file {
      continue;
    }
    if !insecure_ok && assess_dir(&info).is_some() {
      tracing::warn!(file = %info.resolved.display(), "{bin} on PATH is not trustworthy; ignored");
      continue;
    }
    return Some(info.resolved);
  }
  None
}

/// Arrange for `fd` to become fd 3 of the child `cmd` spawns, with
/// `FD_CLOEXEC` cleared (every other fd spoold owns is close-on-exec).
pub fn pass_fd_as_3(cmd: &mut tokio::process::Command, fd: std::os::fd::RawFd) {
  // SAFETY: the closure runs in the forked child before exec and only calls
  // the async-signal-safe `dup2` / `fcntl` on plain integers; it allocates
  // nothing and touches no locks. `fd` stays open in the parent until the
  // spawn returns (the caller drops it afterwards).
  unsafe {
    cmd.pre_exec(move || {
      if fd == 3 {
        // dup2(3, 3) would leave CLOEXEC set.
        if libc::fcntl(3, libc::F_SETFD, 0) < 0 {
          return Err(io::Error::last_os_error());
        }
      } else if libc::dup2(fd, 3) < 0 {
        return Err(io::Error::last_os_error());
      }
      Ok(())
    });
  }
}

/// Credentials of a socket peer (`SO_PEERCRED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCred {
  pub pid: Option<i32>,
  pub uid: u32,
  pub gid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerRejection {
  #[error("peer uid {0} differs from ours")]
  WrongUid(u32),
  #[error("peer pid unknown")]
  NoPid,
  #[error("peer pid {0} is a Flatpak sandbox")]
  Flatpak(i32),
  /// `/proc/<pid>/root` could not be inspected, so we cannot prove the peer
  /// shares our root (fail closed; see [`probe_peer_root`]).
  #[error("cannot inspect root of peer pid {0}: {1}")]
  RootUnknown(i32, String),
}

/// Read `SO_PEERCRED` from a connected tokio unix stream.
pub fn peer_cred(stream: &tokio::net::UnixStream) -> io::Result<PeerCred> {
  let c = stream.peer_cred()?;
  Ok(PeerCred { pid: c.pid(), uid: c.uid(), gid: c.gid() })
}

/// Result of inspecting a peer's root directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootProbe {
  /// `/proc/<pid>/root` is the same directory (dev+inode) as our `/`.
  SameRoot,
  /// Different root (container/sandbox); `flatpak` = `/.flatpak-info`
  /// exists inside it.
  OtherRoot { flatpak: bool },
  /// `/proc/<pid>/root` (or `.flatpak-info` in it) could not be inspected.
  Unreadable(String),
}

/// Inspect `/proc/<pid>/root`.
///
/// Design choice: an unreadable `/proc/<pid>/root` (EACCES because the peer
/// is non-dumpable or in a user namespace we cannot see into, or ENOENT
/// because it already exited) is reported as [`RootProbe::Unreadable`] and
/// the peer is **rejected**. "Unreadable means not Flatpak" would only be
/// safe if we knew the peer's root is our own, but proving that requires
/// exactly the access that just failed (`/proc/<pid>/root`, `ns/mnt` are
/// all behind the same ptrace-read check), so we fail closed. Ordinary
/// same-uid clients (spoolctl, scripts) are dumpable and readable; a
/// non-dumpable client (e.g. a hardened compositor process) would need a
/// different channel.
pub fn probe_peer_root(pid: i32) -> RootProbe {
  probe_root_at(Path::new("/"), &PathBuf::from(format!("/proc/{pid}/root")))
}

fn probe_root_at(our_root: &Path, peer_root: &Path) -> RootProbe {
  let ours = match std::fs::metadata(our_root) {
    Ok(m) => m,
    Err(e) => return RootProbe::Unreadable(format!("stat {}: {e}", our_root.display())),
  };
  let theirs = match std::fs::metadata(peer_root) {
    Ok(m) => m,
    Err(e) => return RootProbe::Unreadable(e.to_string()),
  };
  if ours.dev() == theirs.dev() && ours.ino() == theirs.ino() {
    return RootProbe::SameRoot;
  }
  match std::fs::symlink_metadata(peer_root.join(".flatpak-info")) {
    Ok(_) => RootProbe::OtherRoot { flatpak: true },
    Err(e) if e.kind() == io::ErrorKind::NotFound => RootProbe::OtherRoot { flatpak: false },
    Err(e) => RootProbe::Unreadable(e.to_string()),
  }
}

/// Accept only peers with our uid whose `/proc/<pid>/root/.flatpak-info`
/// does not exist (and whose root we can inspect; see [`probe_peer_root`]).
pub fn check_peer(cred: &PeerCred) -> Result<(), PeerRejection> {
  check_peer_with(cred, our_uid(), probe_peer_root)
}

/// [`check_peer`] with injected uid and root probe (unit tests).
pub fn check_peer_with(
  cred: &PeerCred,
  our_uid: u32,
  probe: impl Fn(i32) -> RootProbe,
) -> Result<(), PeerRejection> {
  if cred.uid != our_uid {
    return Err(PeerRejection::WrongUid(cred.uid));
  }
  let pid = match cred.pid {
    Some(p) if p > 0 => p,
    _ => return Err(PeerRejection::NoPid),
  };
  match probe(pid) {
    RootProbe::SameRoot | RootProbe::OtherRoot { flatpak: false } => Ok(()),
    RootProbe::OtherRoot { flatpak: true } => Err(PeerRejection::Flatpak(pid)),
    RootProbe::Unreadable(e) => Err(PeerRejection::RootUnknown(pid, e)),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::HashMap;

  fn info(resolved: &str, uid: u32, mode: u32) -> io::Result<DirInfo> {
    Ok(DirInfo { resolved: resolved.into(), uid, mode: 0o040000 | mode })
  }

  fn fake(
    entries: Vec<(&'static str, io::Result<DirInfo>)>,
  ) -> impl Fn(&Path) -> io::Result<DirInfo> {
    let map: HashMap<PathBuf, Result<DirInfo, io::ErrorKind>> =
      entries.into_iter().map(|(k, v)| (PathBuf::from(k), v.map_err(|e| e.kind()))).collect();
    move |p: &Path| match map.get(p) {
      Some(Ok(i)) => Ok(i.clone()),
      Some(Err(k)) => Err(io::Error::from(*k)),
      None => Err(io::Error::from(io::ErrorKind::NotFound)),
    }
  }

  #[test]
  fn path_all_good() {
    let lookup = fake(vec![
      ("/run/current-system/sw/bin", info("/nix/store/abc-system-path/bin", 0, 0o555)),
      ("/usr/bin", info("/usr/bin", 0, 0o755)),
      ("/nix/store/xyz/bin", info("/nix/store/xyz/bin", 0, 0o555)),
    ]);
    let p = check_path_dirs_with(
      OsStr::new("/run/current-system/sw/bin:/usr/bin:/nix/store/xyz/bin:/does/not/exist"),
      lookup,
    );
    assert!(p.is_empty(), "{p:?}");
  }

  #[test]
  fn path_problems() {
    let lookup = fake(vec![
      ("/home/u/bin", info("/home/u/bin", 1000, 0o755)),
      ("/opt/bin", info("/opt/bin", 0, 0o775)),
      ("/tmp/link", info("/tmp/real", 0, 0o1777)),
      ("/secret", Err(io::Error::from(io::ErrorKind::PermissionDenied))),
    ]);
    let p =
      check_path_dirs_with(OsStr::new("/home/u/bin:/opt/bin::rel/bin:/tmp/link:/secret"), lookup);
    assert_eq!(p.len(), 6, "{p:?}");
    assert_eq!(p[0].kind, PathProblemKind::NotRootOwned { uid: 1000 });
    assert_eq!(p[1].kind, PathProblemKind::Writable { mode: 0o775 });
    assert_eq!(p[2].kind, PathProblemKind::NotAbsolute);
    assert_eq!(p[3], PathProblem { dir: "rel/bin".into(), kind: PathProblemKind::NotAbsolute });
    // Symlink: the resolved target is named.
    assert_eq!(
      p[4],
      PathProblem { dir: "/tmp/real".into(), kind: PathProblemKind::Writable { mode: 0o1777 } }
    );
    assert!(matches!(p[5].kind, PathProblemKind::Unreadable(_)));
    assert!(p[0].to_string().contains("/home/u/bin"));
  }

  #[test]
  fn path_store_symlink_trusted_even_if_odd_owner() {
    // A user-owned symlink (e.g. ~/.nix-profile/bin) into the store is fine:
    // only the resolved target matters.
    let lookup =
      fake(vec![("/home/u/.nix-profile/bin", info("/nix/store/p-env/bin", 1000, 0o555))]);
    assert!(check_path_dirs_with(OsStr::new("/home/u/.nix-profile/bin"), lookup).is_empty());
  }

  #[test]
  fn path_real_fs() {
    // Real stat calls: a tempdir is owned by us (not root) -> problem unless
    // we are root; a missing dir is skipped.
    let d = tempfile::tempdir().unwrap();
    let missing = d.path().join("missing");
    let var = format!("{}:{}", d.path().display(), missing.display());
    let p = check_path_dirs(OsStr::new(&var));
    if our_uid() == 0 {
      assert!(p.is_empty());
    } else {
      assert_eq!(p.len(), 1);
      assert_eq!(p[0].kind, PathProblemKind::NotRootOwned { uid: our_uid() });
    }
    // Symlink into the dir resolves to it.
    let link = d.path().join("link");
    std::os::unix::fs::symlink(d.path(), &link).unwrap();
    let p = check_path_dirs(link.as_os_str());
    if our_uid() != 0 {
      assert_eq!(p[0].dir, std::fs::canonicalize(d.path()).unwrap());
    }
  }

  #[test]
  fn audited_lookup() {
    let file = |resolved: &str, uid: u32, mode: u32| -> io::Result<DirInfo> {
      Ok(DirInfo { resolved: resolved.into(), uid, mode: 0o100000 | mode })
    };
    let lookup = |p: &Path| -> io::Result<DirInfo> {
      match p.to_str().unwrap() {
        "/home/u/bin" => info("/home/u/bin", 1000, 0o755),
        "/home/u/bin/spool-picker" => file("/home/u/bin/spool-picker", 1000, 0o755),
        "/run/sw/bin" => info("/nix/store/sys/bin", 0, 0o555),
        "/run/sw/bin/spool-picker" => file("/nix/store/p/bin/spool-picker", 0, 0o555),
        "/opt/bin" => info("/opt/bin", 0, 0o755),
        "/opt/bin/noexec" => file("/opt/bin/noexec", 0, 0o644),
        "/opt/bin/evil" => file("/home/u/evil", 1000, 0o755),
        "/opt/bin/dir" => info("/opt/bin/dir", 0, 0o755),
        _ => Err(io::Error::from(io::ErrorKind::NotFound)),
      }
    };
    let path = OsStr::new("rel:/home/u/bin:/opt/bin:/run/sw/bin");
    // The user-owned dir is skipped; the store one wins (canonical path).
    assert_eq!(
      find_on_path_with(path, "spool-picker", false, lookup),
      Some(PathBuf::from("/nix/store/p/bin/spool-picker"))
    );
    // Development override: first hit.
    assert_eq!(
      find_on_path_with(path, "spool-picker", true, lookup),
      Some(PathBuf::from("/home/u/bin/spool-picker"))
    );
    // Not executable, a symlink to a user-owned file, a directory: no.
    for bin in ["noexec", "evil", "dir", "missing", "", "../x", "a/b"] {
      assert_eq!(find_on_path_with(path, bin, false, lookup), None, "{bin}");
    }
    // Real filesystem: something from the system PATH resolves (or the
    // sandbox has none); a relative-only PATH never does.
    assert_eq!(find_on_audited_path("definitely-not-a-binary-xyz"), None);
    assert_eq!(find_on_path_with(OsStr::new("bin:."), "sh", true, stat_dir), None);
  }

  #[test]
  fn path_override() {
    let probs = vec![PathProblem { dir: "/x".into(), kind: PathProblemKind::NotAbsolute }];
    assert!(evaluate_path_problems(&probs, false).is_err());
    assert!(evaluate_path_problems(&probs, true).is_ok());
    assert!(evaluate_path_problems(&[], false).is_ok());
    let msg = evaluate_path_problems(&probs, false).unwrap_err().to_string();
    assert!(msg.contains("/x") && msg.contains(INSECURE_PATH_ENV), "{msg}");
  }

  #[test]
  fn peer_rules() {
    let c = PeerCred { pid: Some(42), uid: 1000, gid: 100 };
    assert_eq!(check_peer_with(&c, 1000, |_| RootProbe::SameRoot), Ok(()));
    assert_eq!(check_peer_with(&c, 1000, |_| RootProbe::OtherRoot { flatpak: false }), Ok(()));
    assert_eq!(
      check_peer_with(&c, 1000, |_| RootProbe::OtherRoot { flatpak: true }),
      Err(PeerRejection::Flatpak(42))
    );
    assert!(matches!(
      check_peer_with(&c, 1000, |_| RootProbe::Unreadable("EACCES".into())),
      Err(PeerRejection::RootUnknown(42, _))
    ));
    assert_eq!(check_peer_with(&c, 0, |_| RootProbe::SameRoot), Err(PeerRejection::WrongUid(1000)));
    let nopid = PeerCred { pid: None, ..c };
    assert_eq!(check_peer_with(&nopid, 1000, |_| RootProbe::SameRoot), Err(PeerRejection::NoPid));
    let zero = PeerCred { pid: Some(0), ..c };
    assert_eq!(check_peer_with(&zero, 1000, |_| RootProbe::SameRoot), Err(PeerRejection::NoPid));
  }

  #[test]
  fn root_probe_fs() {
    // Ourselves: same root.
    assert_eq!(probe_peer_root(std::process::id() as i32), RootProbe::SameRoot);
    assert!(
      check_peer(&PeerCred { pid: Some(std::process::id() as i32), uid: our_uid(), gid: 0 })
        .is_ok()
    );

    // A fake "other root" with and without .flatpak-info.
    let d = tempfile::tempdir().unwrap();
    assert_eq!(probe_root_at(Path::new("/"), d.path()), RootProbe::OtherRoot { flatpak: false });
    std::fs::write(d.path().join(".flatpak-info"), "[Application]\n").unwrap();
    assert_eq!(probe_root_at(Path::new("/"), d.path()), RootProbe::OtherRoot { flatpak: true });
    assert!(matches!(
      probe_root_at(Path::new("/"), &d.path().join("gone")),
      RootProbe::Unreadable(_)
    ));
  }
}
