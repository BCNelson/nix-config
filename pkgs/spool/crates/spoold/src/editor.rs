//! "Edit an item in an external editor": the editing sessions.
//!
//! A session (started from the picker's Ctrl+E / Ctrl+Shift+E, `spoolctl
//! edit` or `spoolctl new`) is:
//!
//! 1. The chosen representation (aliases resolved, text as UTF-8) written to
//!    `$XDG_RUNTIME_DIR/spool/edit/<random>/item.<ext>` (dir 0700, file
//!    0600; tmpfs). Stale `edit/*` dirs are removed at startup
//!    ([`cleanup_stale`]).
//! 2. The configured editor command (`[editor]`, see
//!    `spool_core::config::EditorSettings`) started as a transient systemd
//!    user unit ([`launcher::SystemdLauncher`]), never as spoold's child.
//! 3. The directory watched with inotify ([`watch::DirWatch`]); after
//!    [`DEBOUNCE`] of quiet the file is read (capped) and, if it changed,
//!    handed to the orchestrator ([`EditorEvent::Saved`]), which runs the
//!    manual-copy ingest policy and stores it: the first accepted save as a
//!    new item (`source_app = spool.editor`, the original's tags,
//!    `derived_from`), later ones replace that item's content.
//! 4. Editor exited ([`EditorEvent::Exited`]): the file is read once more,
//!    then on success the latest accepted version is published to the
//!    clipboard; the directory is deleted either way.
//!
//! The task here only moves bytes; every decision (policy, store, publish,
//! notifications) is the orchestrator's (`crate::edit_flow`). Never logs
//! content or the temp file's path.

pub mod content;
pub mod launcher;
pub mod watch;

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use spool_core::item::ItemId;
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, oneshot};

use self::launcher::{EditorExit, EditorLauncher, Notifier};
use self::watch::DirWatch;

/// Concurrent editing sessions; more are refused.
pub const MAX_SESSIONS: usize = 4;
/// Quiet time after the last change before the file is read.
pub const DEBOUNCE: Duration = Duration::from_millis(300);
/// An editor that exits this soon without a save probably did not block
/// (forked into an existing instance): the user is told.
pub const QUICK_EXIT: Duration = Duration::from_secs(2);
/// Directory under `$XDG_RUNTIME_DIR/spool`.
pub const EDIT_SUBDIR: &str = "edit";

/// What sessions need from the outside world (fakes in tests).
pub struct EditorDeps {
  pub launcher: Arc<dyn EditorLauncher>,
  pub notifier: Arc<dyn Notifier>,
  /// `$XDG_RUNTIME_DIR/spool/edit`.
  pub root: PathBuf,
  pub debounce: Duration,
}

/// `edit/` next to the public socket: `$XDG_RUNTIME_DIR/spool/edit` by
/// default (spoold's unit has `RuntimeDirectory=spool`, a 0700 tmpfs dir);
/// a daemon started with `SPOOL_SOCKET` elsewhere keeps its sessions there.
pub fn edit_root(socket_path: &Path) -> Option<PathBuf> {
  socket_path.parent().filter(|p| p.is_absolute()).map(|p| p.join(EDIT_SUBDIR))
}

/// The production [`EditorDeps`]: systemd user units and
/// `org.freedesktop.Notifications` on the session bus (connected lazily, at
/// the first edit). Clears stale sessions first; `None` (editing
/// unavailable, logged) if the directory cannot be prepared.
pub fn system_deps(socket_path: &Path) -> Option<EditorDeps> {
  let Some(root) = edit_root(socket_path) else {
    tracing::warn!("no directory for edit sessions; external editing unavailable");
    return None;
  };
  match cleanup_stale(&root) {
    Ok(0) => {}
    Ok(n) => tracing::info!(removed = n, "removed stale edit sessions"),
    Err(e) => {
      tracing::warn!("preparing the edit directory failed ({e}); external editing unavailable");
      return None;
    }
  }
  let bus = launcher::SessionBus::default();
  Some(EditorDeps {
    launcher: Arc::new(launcher::SystemdLauncher::new(bus.clone())),
    notifier: Arc::new(launcher::DbusNotifier::new(bus)),
    root,
    debounce: DEBOUNCE,
  })
}

/// Make sure `root` exists (0700) and is empty: sessions of a previous run
/// (crash, `kill -9`) left their plaintext temp files here. Returns how
/// many stale entries were removed.
pub fn cleanup_stale(root: &Path) -> io::Result<usize> {
  match fs::symlink_metadata(root) {
    Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
      return Err(io::Error::other("edit directory is not a directory"));
    }
    Ok(_) => {}
    Err(e) if e.kind() == io::ErrorKind::NotFound => {
      fs::DirBuilder::new().recursive(true).mode(0o700).create(root)?;
    }
    Err(e) => return Err(e),
  }
  fs::set_permissions(root, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
  let mut n = 0;
  for entry in fs::read_dir(root)? {
    let path = entry?.path();
    let r = match fs::symlink_metadata(&path) {
      Ok(m) if m.is_dir() => fs::remove_dir_all(&path),
      Ok(_) => fs::remove_file(&path),
      Err(e) => Err(e),
    };
    match r {
      Ok(()) => n += 1,
      Err(e) => tracing::warn!("removing a stale edit directory failed: {e}"),
    }
  }
  Ok(n)
}

/// Why an editing session could not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditFailKind {
  /// The item or representation is gone.
  NotFound,
  /// Editing is not set up here (no runtime dir / session bus / systemd).
  Unavailable,
  /// [`MAX_SESSIONS`] sessions are open.
  Busy,
  /// No `[editor.mime]` entry for the type.
  NoEditor,
  /// Invalid mime from `spoolctl new`.
  BadRequest,
  Internal,
}

/// A failed start: kind + a short message for humans (never content).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditFailure {
  pub kind: EditFailKind,
  pub message: String,
}

impl EditFailure {
  pub fn new(kind: EditFailKind, message: impl Into<String>) -> Self {
    Self { kind, message: message.into() }
  }
}

impl std::fmt::Display for EditFailure {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(&self.message)
  }
}

/// Answer to whoever asked for the session: `Ok` once the editor runs.
pub type EditReply = oneshot::Sender<Result<(), EditFailure>>;

/// Who asked (picker failures also become notifications: it is hidden).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOrigin {
  Picker,
  Socket,
}

/// Why a saved file could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadIssue {
  /// Larger than the cap (`max_rep_bytes`).
  TooLarge,
  /// Missing or unreadable (e.g. between an editor's delete and rename).
  Unreadable,
}

/// Session task -> orchestrator.
#[derive(Debug)]
pub enum EditorEvent {
  /// The file changed (debounced; only when its bytes differ from the last
  /// read).
  Saved { sid: u64, data: Result<Vec<u8>, ReadIssue> },
  /// The editor exited (after a final read).
  Exited { sid: u64, exit: EditorExit },
  /// The editor could not be started (the requester was answered).
  LaunchFailed { sid: u64, why: String },
}

/// One open editing session (orchestrator-owned bookkeeping).
pub struct Session {
  pub dir: PathBuf,
  /// What is in the file (`text/plain;charset=utf-8`, `text/html`, ...).
  pub edit_mime: String,
  /// The item being edited (`None`: `spoolctl new`).
  pub original: Option<ItemId>,
  /// The item holding the accepted saves, and whether this session created
  /// it (`false`: a save deduped onto an existing item, which must not be
  /// rewritten by later saves).
  pub item: Option<(ItemId, bool)>,
  pub saves: u32,
  pub started: Instant,
  pub origin: EditOrigin,
  pub task: tokio::task::JoinHandle<()>,
}

/// All sessions plus the event channel their tasks report on.
pub struct Editors {
  pub deps: Option<EditorDeps>,
  pub sessions: HashMap<u64, Session>,
  next_sid: u64,
  pub tx: mpsc::UnboundedSender<EditorEvent>,
  rx: Option<mpsc::UnboundedReceiver<EditorEvent>>,
}

impl Editors {
  pub fn new(deps: Option<EditorDeps>) -> Self {
    let (tx, rx) = mpsc::unbounded_channel();
    Self { deps, sessions: HashMap::new(), next_sid: 0, tx, rx: Some(rx) }
  }

  /// The event receiver (once, for the run loop).
  pub fn take_events(&mut self) -> Option<mpsc::UnboundedReceiver<EditorEvent>> {
    self.rx.take()
  }

  pub fn next_sid(&mut self) -> u64 {
    self.next_sid += 1;
    self.next_sid
  }

  /// After the session store was merged into the encrypted one: follow the
  /// ids sessions hold.
  pub fn remap_ids(&mut self, map: impl Fn(ItemId) -> Option<spool_core::store::InsertOutcome>) {
    use spool_core::store::InsertOutcome;
    for s in self.sessions.values_mut() {
      s.original = s.original.and_then(|id| map(id).map(|o| o.id()));
      s.item = s.item.and_then(|(id, owned)| {
        map(id).map(|o| match o {
          InsertOutcome::Inserted(n) => (n, owned),
          InsertOutcome::Bumped(n) => (n, false),
        })
      });
    }
  }

  /// Shutdown: stop watching, delete every temp dir. The editors keep
  /// running (their units are not stopped); their saves go nowhere.
  pub fn shutdown(&mut self) {
    for (_, s) in self.sessions.drain() {
      s.task.abort();
      remove_dir(&s.dir);
    }
  }
}

/// Create `<root>/<random>/item.<ext>` holding `data` (dir 0700, file
/// 0600, created exclusively). Returns (dir, file, random name).
pub fn create_session_files(
  root: &Path,
  ext: &str,
  data: &[u8],
) -> io::Result<(PathBuf, PathBuf, String)> {
  let name = format!("{:016x}", rand::random::<u64>());
  let dir = root.join(&name);
  fs::DirBuilder::new().mode(0o700).create(&dir)?;
  let file = dir.join(format!("item.{ext}"));
  let r = fs::OpenOptions::new()
    .write(true)
    .create_new(true)
    .mode(0o600)
    .open(&file)
    .and_then(|mut f| f.write_all(data).and_then(|()| f.flush()));
  if let Err(e) = r {
    remove_dir(&dir);
    return Err(e);
  }
  Ok((dir, file, name))
}

/// Best-effort removal of a session directory (logged without its path).
pub fn remove_dir(dir: &Path) {
  if let Err(e) = fs::remove_dir_all(dir)
    && e.kind() != io::ErrorKind::NotFound
  {
    tracing::warn!("removing an edit directory failed: {e}");
  }
}

fn digest(data: &[u8]) -> [u8; 32] {
  *blake3::hash(data).as_bytes()
}

/// Read `path`, at most `cap` bytes (more = [`ReadIssue::TooLarge`]).
async fn read_capped(path: &Path, cap: usize) -> Result<Vec<u8>, ReadIssue> {
  let f = tokio::fs::File::open(path).await.map_err(|_| ReadIssue::Unreadable)?;
  let mut data = Vec::new();
  f.take(cap as u64 + 1).read_to_end(&mut data).await.map_err(|_| ReadIssue::Unreadable)?;
  if data.len() > cap {
    return Err(ReadIssue::TooLarge);
  }
  Ok(data)
}

/// What a session task needs.
pub struct SessionSpec {
  pub sid: u64,
  pub dir: PathBuf,
  pub file: PathBuf,
  pub unit: String,
  pub argv: Vec<String>,
  /// Read cap (`max_rep_bytes`).
  pub max_bytes: usize,
  /// The bytes initially written (unchanged saves are ignored).
  pub initial: Vec<u8>,
  pub debounce: Duration,
}

/// The session task: watch, launch, debounce, read, report.
pub async fn run_session(
  spec: SessionSpec,
  launcher: Arc<dyn EditorLauncher>,
  tx: mpsc::UnboundedSender<EditorEvent>,
  reply: EditReply,
) {
  let SessionSpec { sid, dir, file, unit, argv, max_bytes, initial, debounce } = spec;
  let name = file.file_name().map(|n| n.to_owned()).unwrap_or_default();
  let mut watch = match DirWatch::new(&dir, &name) {
    Ok(w) => Some(w),
    Err(e) => {
      let why = format!("watching the edit directory failed: {e}");
      let _ = reply.send(Err(EditFailure::new(EditFailKind::Internal, "could not watch the file")));
      let _ = tx.send(EditorEvent::LaunchFailed { sid, why });
      return;
    }
  };
  let program = argv.first().cloned().unwrap_or_default();
  let mut exit = match launcher.launch(&unit, &argv).await {
    Ok(f) => f,
    Err(why) => {
      let _ = reply.send(Err(EditFailure::new(
        EditFailKind::Unavailable,
        format!("could not start the editor ({program})"),
      )));
      let _ = tx.send(EditorEvent::LaunchFailed { sid, why });
      return;
    }
  };
  tracing::info!(sid, unit = %unit, program = %program, "editor started");
  let _ = reply.send(Ok(()));
  let mut last = digest(&initial);
  drop(initial);
  let mut due: Option<tokio::time::Instant> = None;

  async fn report(
    sid: u64,
    file: &Path,
    cap: usize,
    last: &mut [u8; 32],
    tx: &mpsc::UnboundedSender<EditorEvent>,
  ) {
    let data = read_capped(file, cap).await;
    if let Ok(d) = &data {
      let h = digest(d);
      if h == *last {
        return;
      }
      *last = h;
    }
    if data == Err(ReadIssue::Unreadable) {
      // Mid-save (deleted before the rename): the next event re-reads.
      return;
    }
    let _ = tx.send(EditorEvent::Saved { sid, data });
  }

  loop {
    tokio::select! {
      r = async { watch.as_mut().expect("guarded").changed().await }, if watch.is_some() => {
        match r {
          Ok(()) => due = Some(tokio::time::Instant::now() + debounce),
          Err(e) => {
            tracing::warn!(sid, "edit file watch failed ({e}); saves are read when the editor exits");
            watch = None;
          }
        }
      }
      () = async { tokio::time::sleep_until(due.expect("guarded")).await }, if due.is_some() => {
        due = None;
        report(sid, &file, max_bytes, &mut last, &tx).await;
      }
      status = &mut exit => {
        report(sid, &file, max_bytes, &mut last, &tx).await;
        let _ = tx.send(EditorEvent::Exited { sid, exit: status });
        return;
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::os::unix::fs::PermissionsExt;

  #[test]
  fn session_files_are_private() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("edit");
    assert_eq!(cleanup_stale(&root).unwrap(), 0);
    let (dir, file, name) = create_session_files(&root, "txt", b"hello").unwrap();
    assert_eq!(dir, root.join(&name));
    assert_eq!(name.len(), 16);
    assert_eq!(file, dir.join("item.txt"));
    assert_eq!(fs::read(&file).unwrap(), b"hello");
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&file), 0o600);
    let (dir2, ..) = create_session_files(&root, "png", b"").unwrap();
    assert_ne!(dir, dir2);
    remove_dir(&dir2);
    assert!(!dir2.exists());
    remove_dir(&dir2); // already gone: fine
  }

  #[test]
  fn stale_dirs_are_removed_at_startup() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("edit");
    fs::create_dir_all(&root).unwrap();
    create_session_files(&root, "txt", b"left over").unwrap();
    create_session_files(&root, "html", b"<b>x</b>").unwrap();
    fs::write(root.join("stray"), b"x").unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(cleanup_stale(&root).unwrap(), 3);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    assert_eq!(fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o700);
    // A symlink in place of the directory is refused.
    let link = d.path().join("link");
    std::os::unix::fs::symlink(d.path(), &link).unwrap();
    assert!(cleanup_stale(&link).is_err());
  }

  #[tokio::test]
  async fn ids_follow_the_session_merge() {
    use spool_core::store::InsertOutcome;
    let mut e = Editors::new(None);
    let session = |original, item| Session {
      dir: PathBuf::from("/nonexistent/spool-test"),
      edit_mime: "text/plain".into(),
      original,
      item,
      saves: 1,
      started: Instant::now(),
      origin: EditOrigin::Picker,
      task: tokio::spawn(async {}),
    };
    e.sessions.insert(1, session(Some(ItemId(1)), Some((ItemId(2), true))));
    e.sessions.insert(2, session(Some(ItemId(3)), Some((ItemId(4), true))));
    e.sessions.insert(3, session(None, Some((ItemId(9), true))));
    e.remap_ids(|id| match id.0 {
      1 => Some(InsertOutcome::Inserted(ItemId(101))),
      2 => Some(InsertOutcome::Inserted(ItemId(102))),
      3 => Some(InsertOutcome::Bumped(ItemId(103))),
      // Deduped onto an existing persistent item: no longer ours to rewrite.
      4 => Some(InsertOutcome::Bumped(ItemId(104))),
      _ => None,
    });
    let s = |n| (e.sessions[&n].original, e.sessions[&n].item);
    assert_eq!(s(1), (Some(ItemId(101)), Some((ItemId(102), true))));
    assert_eq!(s(2), (Some(ItemId(103)), Some((ItemId(104), false))));
    assert_eq!(s(3), (None, None));
    e.shutdown();
    assert!(e.sessions.is_empty());
  }

  #[tokio::test]
  async fn capped_reads() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("f");
    fs::write(&p, b"12345").unwrap();
    assert_eq!(read_capped(&p, 5).await, Ok(b"12345".to_vec()));
    assert_eq!(read_capped(&p, 4).await, Err(ReadIssue::TooLarge));
    assert_eq!(read_capped(&d.path().join("missing"), 4).await, Err(ReadIssue::Unreadable));
  }
}
