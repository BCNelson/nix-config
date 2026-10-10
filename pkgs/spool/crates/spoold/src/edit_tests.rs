//! Editing sessions through the orchestrator with a fake editor launcher
//! (the test plays the editor: it writes the temp file and decides when the
//! "editor" exits), a fake notifier, a fake Wayland side and a plain store
//! file the test also reads.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;

use spool_core::config::EditorSettings;
use spool_core::item::Item;

use super::*;
use crate::editor::launcher::{EditorExit, EditorLauncher, ExitFuture, Notifier};
use crate::editor::{EditFailKind, EditFailure, EditorDeps, MAX_SESSIONS};

const UTF8: &str = "text/plain;charset=utf-8";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDRfake";
const JPEG: &[u8] = &[0xff, 0xd8, 0xff, 0xe0, 0, 0x10, b'J', b'F', b'I', b'F'];
const GITHUB_TOKEN: &str = "ghp_0123456789abcdefghijklmnopqrstuvwxyz";

/// One started "editor": its argv and the switch that ends it.
struct Launch {
  unit: String,
  argv: Vec<String>,
  exit: oneshot::Sender<EditorExit>,
}

impl Launch {
  fn file(&self) -> PathBuf {
    PathBuf::from(self.argv.last().unwrap())
  }
}

struct FakeLauncher {
  launches: mpsc::UnboundedSender<Launch>,
  fail: bool,
}

#[async_trait::async_trait]
impl EditorLauncher for FakeLauncher {
  async fn launch(&self, unit: &str, argv: &[String]) -> Result<ExitFuture, String> {
    if self.fail {
      return Err("no systemd here".into());
    }
    let (tx, rx) = oneshot::channel();
    let _ = self.launches.send(Launch { unit: unit.into(), argv: argv.to_vec(), exit: tx });
    Ok(Box::pin(async move { rx.await.unwrap_or(EditorExit::Failed("dropped".into())) }))
  }
}

#[derive(Default)]
struct FakeNotifier(StdMutex<Vec<(String, String)>>);

impl Notifier for FakeNotifier {
  fn notify(&self, summary: &str, body: &str) {
    self.0.lock().unwrap().push((summary.into(), body.into()));
  }
}

struct FakeWayland(mpsc::UnboundedSender<Vec<(String, Vec<u8>)>>);

impl WaylandSide for FakeWayland {
  fn fetch(
    &self,
    _: OfferToken,
    _: Vec<String>,
    _: usize,
    _: usize,
    _: Duration,
  ) -> Result<(), WaylandError> {
    Ok(())
  }

  fn set_selection(
    &self,
    _: Selection,
    reps: Vec<(String, Arc<[u8]>)>,
  ) -> Result<(), WaylandError> {
    let _ = self.0.send(reps.into_iter().map(|(m, d)| (m, d.to_vec())).collect());
    Ok(())
  }
}

#[derive(Default)]
struct Shows(StdMutex<usize>);

struct FakePicker(Arc<Shows>);

impl PickerLauncher for FakePicker {
  fn show(&mut self, _: &ShowContext) -> Result<(), PickerError> {
    *self.0.0.lock().unwrap() += 1;
    Ok(())
  }
  fn hide(&mut self) {}
}

fn editor_config() -> Config {
  Config {
    editor: EditorSettings {
      terminal: vec!["term".into(), "-e".into()],
      mime: [
        ("text/*".to_string(), vec!["{terminal}".into(), "vi".into(), "{file}".into()]),
        ("text/html".to_string(), vec!["htmledit".into(), "--wait".into(), "{file}".into()]),
        ("image/*".to_string(), vec!["paint".into(), "{file}".into()]),
      ]
      .into(),
    },
    ..Config::default()
  }
}

struct H {
  _dir: tempfile::TempDir,
  root: PathBuf,
  db: Store,
  req: mpsc::Sender<Request>,
  sets: mpsc::UnboundedReceiver<Vec<(String, Vec<u8>)>>,
  launches: mpsc::UnboundedReceiver<Launch>,
  notes: Arc<FakeNotifier>,
  shows: Arc<Shows>,
  stop: Option<oneshot::Sender<()>>,
  task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl H {
  fn start(config: Config) -> Self {
    Self::start_with(config, false)
  }

  fn start_with(config: Config, fail_launch: bool) -> Self {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("history.db");
    let store = Store::open(&db_path, None).unwrap();
    let root = dir.path().join("rt/spool/edit");
    crate::editor::cleanup_stale(&root).unwrap();
    let (ltx, launches) = mpsc::unbounded_channel();
    let notes = Arc::new(FakeNotifier::default());
    let shows = Arc::new(Shows::default());
    let deps = EditorDeps {
      launcher: Arc::new(FakeLauncher { launches: ltx, fail: fail_launch }),
      notifier: notes.clone(),
      root: root.clone(),
      debounce: Duration::from_millis(30),
    };
    let (stx, sets) = mpsc::unbounded_channel();
    let orch = Orchestrator::new(config, store, FakeWayland(stx))
      .unwrap()
      .with_picker(Box::new(FakePicker(shows.clone())))
      .with_editor(deps);
    let (_wl_tx, wl_rx) = mpsc::unbounded_channel();
    let (req, req_rx) = mpsc::channel(REQUEST_QUEUE);
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let task = tokio::spawn(orch.run(wl_rx, req_rx, async move {
      let _ = stop_rx.await;
      drop(_wl_tx);
    }));
    let db = Store::open(&db_path, None).unwrap();
    Self { _dir: dir, root, db, req, sets, launches, notes, shows, stop: Some(stop_tx), task }
  }

  async fn ask(&self, req: PublicReq) -> PublicResp {
    let (reply, rx) = oneshot::channel();
    let peer = PeerCred { pid: Some(1), uid: 0, gid: 0 };
    self.req.send(Request::Public { req, peer, reply }).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), rx).await.expect("reply").unwrap()
  }

  /// Copy `data` as `mime` (stored and published); returns the new item.
  async fn copy(&mut self, mime: &str, data: &[u8]) -> Item {
    let r = self.ask(PublicReq::Copy { mime: mime.into(), data: data.to_vec() }).await;
    assert_eq!(r, PublicResp::Ok);
    self.next_set().await;
    self.db.latest(Selection::Clipboard).unwrap().unwrap()
  }

  async fn edit(&self, id: ItemId, mime: &str) -> Result<(), EditFailure> {
    let (reply, rx) = oneshot::channel();
    self.req.send(Request::EditItem { id, mime: mime.into(), reply }).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), rx).await.expect("edit reply").unwrap()
  }

  async fn launch(&mut self) -> Launch {
    tokio::time::timeout(Duration::from_secs(5), self.launches.recv())
      .await
      .expect("editor launched")
      .unwrap()
  }

  async fn next_set(&mut self) -> Vec<(String, Vec<u8>)> {
    tokio::time::timeout(Duration::from_secs(5), self.sets.recv())
      .await
      .expect("clipboard publish")
      .unwrap()
  }

  fn no_set(&mut self) {
    assert!(self.sets.try_recv().is_err(), "unexpected clipboard publish");
  }

  fn items_from_editor(&self) -> Vec<Item> {
    let mut v: Vec<Item> = self
      .db
      .recent(100)
      .unwrap()
      .into_iter()
      .filter(|s| s.source_app.as_deref() == Some("spool.editor"))
      .map(|s| self.db.get(s.id).unwrap().unwrap())
      .collect();
    v.sort_by_key(|i| i.id);
    v
  }

  /// Wait until the editor items satisfy `pred`.
  async fn wait_items(&self, what: &str, pred: impl Fn(&[Item]) -> bool) -> Vec<Item> {
    for _ in 0..250 {
      let items = self.items_from_editor();
      if pred(&items) {
        return items;
      }
      tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for: {what}; have {:?}", self.items_from_editor());
  }

  async fn wait_notes(&self, n: usize) -> Vec<(String, String)> {
    for _ in 0..250 {
      let v = self.notes.0.lock().unwrap().clone();
      if v.len() >= n {
        return v;
      }
      tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {n} notifications: {:?}", self.notes.0.lock().unwrap());
  }

  fn sessions_on_disk(&self) -> usize {
    std::fs::read_dir(&self.root).unwrap().count()
  }

  async fn wait_no_sessions_on_disk(&self) {
    for _ in 0..250 {
      if self.sessions_on_disk() == 0 {
        return;
      }
      tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("edit dirs left behind");
  }

  /// Stop the orchestrator; returns how many edit dirs are left.
  async fn stop(mut self) -> usize {
    let _ = self.stop.take().unwrap().send(());
    tokio::time::timeout(Duration::from_secs(10), &mut self.task).await.unwrap().unwrap().unwrap();
    self.sessions_on_disk()
  }
}

fn text_of(item: &Item, mime: &str) -> String {
  String::from_utf8(item.resolve(mime).unwrap().to_vec()).unwrap()
}

fn mode(p: &Path) -> u32 {
  std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[tokio::test]
async fn text_edit_creates_a_new_item_updates_it_and_publishes_on_exit() {
  let mut h = H::start(editor_config());
  let orig = h.copy(UTF8, b"original text").await;
  h.db.set_tag(orig.id, "work", true).unwrap();
  h.db.set_pinned(orig.id, true).unwrap();
  let orig = h.db.get(orig.id).unwrap().unwrap();

  // Aliases resolve: the canonical text is edited.
  h.edit(orig.id, "UTF8_STRING").await.unwrap();
  let l = h.launch().await;
  let file = l.file();
  assert_eq!(l.argv, ["term", "-e", "vi", file.to_str().unwrap()]);
  assert!(l.unit.starts_with("spool-edit-") && l.unit.ends_with(".service"), "{}", l.unit);
  assert_eq!(file.file_name().unwrap(), "item.txt");
  assert!(file.starts_with(&h.root));
  assert_eq!(std::fs::read(&file).unwrap(), b"original text");
  assert_eq!(mode(&file), 0o600);
  assert_eq!(mode(file.parent().unwrap()), 0o700);

  std::fs::write(&file, b"edit one").unwrap();
  let items = h.wait_items("first save", |v| v.len() == 1).await;
  let new = &items[0];
  assert_ne!(new.id, orig.id);
  assert_eq!(text_of(new, UTF8), "edit one");
  assert_eq!(text_of(new, "UTF8_STRING"), "edit one", "text aliases like a normal copy");
  assert_eq!(h.db.derived_from(new.id).unwrap(), Some(orig.id));
  assert!(!new.flags.contains(ItemFlags::PINNED), "the pin is not copied");
  let tags = h.db.summaries(&[new.id]).unwrap().pop().unwrap().tags;
  assert_eq!(tags, ["work"]);
  assert_eq!(h.db.get(orig.id).unwrap().unwrap(), orig, "the original is never modified");

  // A later save rewrites that item in place.
  std::fs::write(&file, b"edit two").unwrap();
  let items =
    h.wait_items("second save", |v| v.len() == 1 && text_of(&v[0], UTF8) == "edit two").await;
  assert_eq!(items[0].id, new.id);
  // Saving unchanged content is a no-op.
  std::fs::write(&file, b"edit two").unwrap();
  tokio::time::sleep(Duration::from_millis(150)).await;
  assert_eq!(h.items_from_editor().len(), 1);
  h.no_set();

  // Editor closes: the last version goes on the clipboard, the dir goes.
  l.exit.send(EditorExit::Success).unwrap();
  let set = h.next_set().await;
  assert!(set.iter().any(|(m, d)| m == UTF8 && d == b"edit two"), "{set:?}");
  assert!(set.iter().any(|(m, d)| m == "UTF8_STRING" && d == b"edit two"));
  h.wait_no_sessions_on_disk().await;
  assert!(h.notes.0.lock().unwrap().is_empty());
  assert_eq!(h.db.count().unwrap(), 2);
  h.stop().await;
}

#[tokio::test]
async fn refused_save_keeps_the_previous_version_and_notifies_without_content() {
  let mut h = H::start(editor_config());
  assert_eq!(h.ask(PublicReq::New { mime: "text/plain".into() }).await, PublicResp::Ok);
  let l = h.launch().await;
  let file = l.file();
  assert_eq!(std::fs::read(&file).unwrap(), b"", "new: empty file");

  std::fs::write(&file, b"draft").unwrap();
  let items = h.wait_items("draft", |v| v.len() == 1).await;
  let id = items[0].id;
  assert_eq!(h.db.derived_from(id).unwrap(), None, "spoolctl new has no origin");

  std::fs::write(&file, format!("token {GITHUB_TOKEN}")).unwrap();
  let notes = h.wait_notes(1).await;
  assert_eq!(notes[0].0, "Spool: edit not saved");
  assert!(notes[0].1.contains("GitHub token"), "{notes:?}");
  assert!(!notes[0].1.contains("ghp_"), "never content in a notification");
  let items = h.items_from_editor();
  assert_eq!((items.len(), text_of(&items[0], UTF8).as_str()), (1, "draft"));

  // Too large (max_rep_bytes) is refused the same way.
  std::fs::write(&file, vec![b'x'; Config::default().max_rep_bytes + 1]).unwrap();
  let notes = h.wait_notes(2).await;
  assert!(notes[1].1.contains("size limit"), "{notes:?}");

  // Fixed again, then closed: the accepted version is published.
  std::fs::write(&file, b"draft, final").unwrap();
  h.wait_items("final", |v| v.len() == 1 && text_of(&v[0], UTF8) == "draft, final").await;
  l.exit.send(EditorExit::Success).unwrap();
  let set = h.next_set().await;
  assert!(set.iter().any(|(m, d)| m == UTF8 && d == b"draft, final"));
  h.stop().await;
}

#[tokio::test]
async fn html_edit_stores_html_plus_derived_text() {
  let mut h = H::start(editor_config());
  let orig = h.copy("text/html", b"<p>old</p>").await;
  h.edit(orig.id, "text/html").await.unwrap();
  let l = h.launch().await;
  assert_eq!(l.argv[..2], ["htmledit", "--wait"]);
  assert_eq!(l.file().file_name().unwrap(), "item.html");
  assert_eq!(std::fs::read(l.file()).unwrap(), b"<p>old</p>");
  std::fs::write(l.file(), b"<p>Hello <b>world</b> &amp; you</p>").unwrap();
  let items = h.wait_items("html save", |v| v.len() == 1).await;
  let it = &items[0];
  assert_eq!(it.reps[0].mime, "text/html", "HTML stays canonical");
  assert_eq!(text_of(it, "text/html"), "<p>Hello <b>world</b> &amp; you</p>");
  assert_eq!(text_of(it, UTF8), "Hello world & you");
  assert_eq!(text_of(it, "UTF8_STRING"), "Hello world & you");
  assert_eq!(it.preview.as_deref(), Some("Hello world & you"));
  l.exit.send(EditorExit::Success).unwrap();
  let set = h.next_set().await;
  assert!(set.iter().any(|(m, _)| m == "text/html"));
  assert!(set.iter().any(|(m, d)| m == UTF8 && d == b"Hello world & you"));
  h.stop().await;
}

#[tokio::test]
async fn image_edit_sniffs_the_saved_bytes() {
  let mut h = H::start(editor_config());
  let orig = h.copy("image/png", PNG).await;
  h.edit(orig.id, "image/png").await.unwrap();
  let l = h.launch().await;
  assert_eq!(l.argv[0], "paint");
  assert_eq!(l.file().file_name().unwrap(), "item.png");
  assert_eq!(std::fs::read(l.file()).unwrap(), PNG);

  // Not an image: refused.
  std::fs::write(l.file(), b"definitely not a png").unwrap();
  let notes = h.wait_notes(1).await;
  assert!(notes[0].1.contains("not a PNG, JPEG or WebP image"), "{notes:?}");
  assert!(h.items_from_editor().is_empty());
  // A JPEG saved over item.png is stored as what it is.
  std::fs::write(l.file(), JPEG).unwrap();
  let items = h.wait_items("jpeg save", |v| v.len() == 1).await;
  assert_eq!(items[0].reps.len(), 1, "image edits store that image only");
  assert_eq!(items[0].reps[0].mime, "image/jpeg");
  assert_eq!(items[0].reps[0].data.as_ref(), JPEG);
  l.exit.send(EditorExit::Success).unwrap();
  let set = h.next_set().await;
  assert_eq!(set, vec![("image/jpeg".to_string(), JPEG.to_vec())]);
  h.stop().await;
}

#[tokio::test]
async fn failed_editor_keeps_the_last_version_but_does_not_publish() {
  let mut h = H::start(editor_config());
  assert_eq!(h.ask(PublicReq::New { mime: UTF8.into() }).await, PublicResp::Ok);
  let l = h.launch().await;
  std::fs::write(l.file(), b"kept").unwrap();
  h.wait_items("save", |v| v.len() == 1).await;
  l.exit.send(EditorExit::Failed("core-dump".into())).unwrap();
  let notes = h.wait_notes(1).await;
  assert_eq!(notes[0].0, "Spool: editor failed");
  h.wait_no_sessions_on_disk().await;
  h.no_set();
  assert_eq!(text_of(&h.items_from_editor()[0], UTF8), "kept");
  h.stop().await;
}

#[tokio::test]
async fn save_just_before_exit_is_not_lost() {
  let mut h = H::start(editor_config());
  assert_eq!(h.ask(PublicReq::New { mime: UTF8.into() }).await, PublicResp::Ok);
  let l = h.launch().await;
  // Written and exited within the debounce window.
  std::fs::write(l.file(), b"last second").unwrap();
  l.exit.send(EditorExit::Success).unwrap();
  let set = h.next_set().await;
  assert!(set.iter().any(|(m, d)| m == UTF8 && d == b"last second"));
  h.stop().await;
}

#[tokio::test]
async fn editor_exiting_at_once_without_a_save_is_reported() {
  let mut h = H::start(editor_config());
  assert_eq!(h.ask(PublicReq::New { mime: UTF8.into() }).await, PublicResp::Ok);
  let l = h.launch().await;
  l.exit.send(EditorExit::Success).unwrap();
  let notes = h.wait_notes(1).await;
  assert_eq!(notes[0].0, "Spool: editor closed immediately");
  assert!(notes[0].1.contains("--block"), "{notes:?}");
  h.wait_no_sessions_on_disk().await;
  h.no_set();
  assert!(h.items_from_editor().is_empty());
  h.stop().await;
}

#[tokio::test]
async fn refusals_before_launch() {
  let mut c = editor_config();
  c.editor.mime.retain(|k, _| k == "text/*");
  let mut h = H::start(c);
  let img = h.copy("image/png", PNG).await;
  let e = h.edit(img.id, "image/png").await.unwrap_err();
  assert_eq!(e.kind, EditFailKind::NoEditor);
  assert_eq!(e.message, "no editor configured for image/png");
  // The picker is hidden, so its failures are also notifications.
  let notes = h.wait_notes(1).await;
  assert_eq!(notes[0], ("Spool: cannot edit".into(), "no editor configured for image/png".into()));
  assert_eq!(h.edit(ItemId(999), UTF8).await.unwrap_err().kind, EditFailKind::NotFound);
  assert_eq!(h.edit(img.id, "text/html").await.unwrap_err().kind, EditFailKind::NotFound);
  let r = h.ask(PublicReq::New { mime: "bad mime".into() }).await;
  assert!(matches!(r, PublicResp::Error { code: ErrorCode::BadRequest, .. }), "{r:?}");
  assert_eq!(h.sessions_on_disk(), 0);

  // At most MAX_SESSIONS at once.
  let mut open = Vec::new();
  for _ in 0..MAX_SESSIONS {
    assert_eq!(h.ask(PublicReq::New { mime: UTF8.into() }).await, PublicResp::Ok);
    open.push(h.launch().await);
  }
  let r = h.ask(PublicReq::New { mime: UTF8.into() }).await;
  assert!(
    matches!(&r, PublicResp::Error { code: ErrorCode::Unavailable, message } if message.contains("too many")),
    "{r:?}"
  );
  assert_eq!(h.sessions_on_disk(), MAX_SESSIONS);
  // Shutdown deletes the temp dirs (the editors are left running).
  assert_eq!(h.stop().await, 0);
  drop(open);
}

#[tokio::test]
async fn launch_failure_is_reported_and_cleaned_up() {
  let h = H::start_with(editor_config(), true);
  let r = h.ask(PublicReq::New { mime: UTF8.into() }).await;
  assert!(matches!(r, PublicResp::Error { code: ErrorCode::Unavailable, .. }), "{r:?}");
  h.wait_no_sessions_on_disk().await;
  h.stop().await;
}

#[tokio::test]
async fn spoolctl_edit_opens_the_picker_and_edits_the_choice() {
  let mut h = H::start(editor_config());
  let orig = h.copy(UTF8, b"pick me").await;
  // `spoolctl edit` waits for the user's choice.
  let (reply, rx) = oneshot::channel();
  let peer = PeerCred { pid: Some(1), uid: 0, gid: 0 };
  h.req.send(Request::Public { req: PublicReq::Edit, peer, reply }).await.unwrap();
  // Enter in the picker: the preferred representation is edited.
  let (sel_reply, sel_rx) = oneshot::channel();
  h.req.send(Request::PickerHidden { reason: HideReason::Selected }).await.unwrap();
  h.req
    .send(Request::Select { id: orig.id, mode: SelectMode::Paste, reply: sel_reply })
    .await
    .unwrap();
  assert_eq!(sel_rx.await.unwrap(), Ok(SelectOutcome::Returned));
  let l = h.launch().await;
  assert_eq!(std::fs::read(l.file()).unwrap(), b"pick me");
  assert_eq!(
    tokio::time::timeout(Duration::from_secs(5), rx).await.unwrap().unwrap(),
    PublicResp::Ok
  );
  assert_eq!(*h.shows.0.lock().unwrap(), 1);
  h.no_set(); // nothing pasted / published by the choice itself

  // Esc instead: cancelled. Ctrl+Shift+E (picker Edit) answers it too.
  let (reply, rx) = oneshot::channel();
  h.req.send(Request::Public { req: PublicReq::Edit, peer, reply }).await.unwrap();
  h.req.send(Request::PickerHidden { reason: HideReason::Esc }).await.unwrap();
  assert_eq!(rx.await.unwrap(), PublicResp::Cancelled);
  let (reply, rx) = oneshot::channel();
  h.req.send(Request::Public { req: PublicReq::Edit, peer, reply }).await.unwrap();
  h.req.send(Request::PickerHidden { reason: HideReason::Selected }).await.unwrap();
  h.edit(orig.id, UTF8).await.unwrap();
  assert_eq!(rx.await.unwrap(), PublicResp::Ok);
  h.launch().await;

  // A plain `spoolctl pick` is cancelled by Ctrl+E (the user chose to edit).
  let (reply, rx) = oneshot::channel();
  h.req.send(Request::Public { req: PublicReq::Pick, peer, reply }).await.unwrap();
  h.edit(orig.id, UTF8).await.unwrap();
  assert_eq!(rx.await.unwrap(), PublicResp::Cancelled);
  drop(l);
  h.stop().await;
}

#[test]
fn refusal_wording() {
  use crate::orchestrator::edit_flow::{refusal_body, refusal_reason};
  use spool_core::policy::SecretKind;
  assert_eq!(
    refusal_body(&refusal_reason(&DropReason::Secret(SecretKind::GithubToken))),
    "Looks like a GitHub token. The previous version is kept."
  );
  assert_eq!(refusal_reason(&DropReason::Empty), "the file is empty");
}
