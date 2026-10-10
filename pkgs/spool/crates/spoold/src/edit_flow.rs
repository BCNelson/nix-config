//! The orchestrator's side of editing sessions ([`crate::editor`]): starting
//! them (picker `Edit`, `spoolctl edit` / `new`), turning saves into items
//! through the manual-copy ingest policy, publishing on exit.
//!
//! Rules (INTERFACES.md "Editing"):
//! - The original item is never modified. The first accepted save is
//!   stored with [`Store::insert_derived`] (`source_app = spool.editor`,
//!   the original's tags, `derived_from`); later saves of the same session
//!   go through [`Store::replace_content`] on that item, unless that save
//!   deduped onto an item the session did not create (then a new insert).
//! - A save the policy refuses (secret pattern, size, empty, not UTF-8 /
//!   not an image) keeps the previous version and raises a content-free
//!   desktop notification.
//! - Editor exited normally after an accepted save: the latest version is
//!   published like a picker Copy. Failed / crashed: kept, not published.
//! - Works on the session store while history is locked; ids are remapped
//!   when it is merged ([`crate::editor::Editors::remap_ids`]).

use std::path::Path;

use spool_core::item::Item;
use spool_core::policy::SecretKind;
use spool_core::store::ReplaceOutcome;

use super::*;
use crate::editor::content::{
  EDITOR_SOURCE_APP, EditKind, UTF8_TEXT, edit_mime, file_ext, html_to_text, kind_of,
  saved_image_mime,
};
use crate::editor::launcher::EditorExit;
use crate::editor::{
  EditFailKind, EditFailure, EditOrigin, EditReply, EditorEvent, MAX_SESSIONS, QUICK_EXIT,
  ReadIssue, Session, SessionSpec, create_session_files, remove_dir, run_session,
};

/// What to edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditSource {
  /// Representation `mime` of item `id` (`None`: its preferred one, text
  /// first).
  Item { id: ItemId, mime: Option<String> },
  /// An empty file of type `mime` (`spoolctl new`).
  New { mime: String },
}

/// A session ready to launch.
struct Prepared {
  original: Option<ItemId>,
  edit_mime: String,
  data: Vec<u8>,
}

/// Human wording of a policy refusal (never content).
pub fn refusal_reason(reason: &DropReason) -> String {
  match reason {
    DropReason::Secret(kind) => format!(
      "looks like {}",
      match kind {
        SecretKind::PemPrivateKey => "a private key",
        SecretKind::GithubToken => "a GitHub token",
        SecretKind::AwsAccessKey => "an AWS access key",
        SecretKind::OpenAiStyleKey => "an API key",
        SecretKind::Jwt => "a JSON web token",
        SecretKind::SlackToken => "a Slack token",
        SecretKind::AgeSecretKey => "an age secret key",
        SecretKind::OtpCode => "a one-time code",
      }
    ),
    DropReason::TooLarge => "it is larger than the configured size limit".into(),
    DropReason::Empty => "the file is empty".into(),
    _ => "nothing usable in it".into(),
  }
}

/// Notification body for a refused save: `"Looks like a GitHub token. The
/// previous version is kept."`
pub fn refusal_body(reason: &str) -> String {
  let mut c = reason.chars();
  let first: String = c.next().map(|f| f.to_uppercase().collect()).unwrap_or_default();
  format!("{first}{}. The previous version is kept.", c.as_str())
}

/// [`EditFailure`] as a public-socket answer.
pub fn failure_resp(f: EditFailure) -> PublicResp {
  let code = match f.kind {
    EditFailKind::Unavailable | EditFailKind::Busy => ErrorCode::Unavailable,
    EditFailKind::NotFound | EditFailKind::NoEditor | EditFailKind::BadRequest => {
      ErrorCode::BadRequest
    }
    EditFailKind::Internal => ErrorCode::Internal,
  };
  PublicResp::Error { code, message: f.message }
}

/// Forward a session start's answer to a public client.
fn reply_to_public(
  rx: oneshot::Receiver<Result<(), EditFailure>>,
  to: oneshot::Sender<PublicResp>,
) {
  tokio::spawn(async move {
    let resp = match rx.await {
      Ok(Ok(())) => PublicResp::Ok,
      Ok(Err(f)) => failure_resp(f),
      Err(_) => internal("the edit session was dropped".into()),
    };
    let _ = to.send(resp);
  });
}

impl<W: WaylandSide> Orchestrator<W> {
  /// Content-free desktop notification (no-op without editing support).
  fn edit_notify(&self, summary: &str, body: &str) {
    if let Some(d) = &self.editors.deps {
      d.notifier.notify(summary, body);
    }
  }

  /// Start a session; `reply` gets `Ok` once the editor runs.
  pub(super) async fn start_edit(
    &mut self,
    origin: EditOrigin,
    source: EditSource,
    reply: EditReply,
  ) {
    let prepared = match self.prepare_edit(source).await {
      Ok(p) => p,
      Err(f) => {
        tracing::info!(kind = ?f.kind, "edit not started: {}", f.message);
        if origin == EditOrigin::Picker {
          self.edit_notify("Spool: cannot edit", &f.message);
        }
        let _ = reply.send(Err(f));
        return;
      }
    };
    let Prepared { original, edit_mime, data } = prepared;
    let deps = self.editors.deps.as_ref().expect("checked by prepare_edit");
    let (dir, file, name) = match create_session_files(&deps.root, file_ext(&edit_mime), &data) {
      Ok(v) => v,
      Err(e) => {
        tracing::error!("creating the edit file failed: {e}");
        let f = EditFailure::new(EditFailKind::Internal, "could not create the edit file");
        if origin == EditOrigin::Picker {
          self.edit_notify("Spool: cannot edit", &f.message);
        }
        let _ = reply.send(Err(f));
        return;
      }
    };
    let argv = match self.config.editor.command_for(&edit_mime, &file) {
      Ok(a) => a,
      Err(e) => {
        remove_dir(&dir);
        let _ = reply.send(Err(EditFailure::new(EditFailKind::NoEditor, e.to_string())));
        return;
      }
    };
    let launcher = deps.launcher.clone();
    let debounce = deps.debounce;
    let sid = self.editors.next_sid();
    tracing::info!(
      sid,
      mime = %edit_mime,
      bytes = data.len(),
      ?origin,
      from_item = original.map(|i| i.0),
      "edit session starting"
    );
    let spec = SessionSpec {
      sid,
      dir: dir.clone(),
      file,
      unit: format!("spool-edit-{name}.service"),
      argv,
      max_bytes: self.config.max_rep_bytes,
      initial: data,
      debounce,
    };
    let task = tokio::spawn(run_session(spec, launcher, self.editors.tx.clone(), reply));
    self.editors.sessions.insert(
      sid,
      Session {
        dir,
        edit_mime,
        original,
        item: None,
        saves: 0,
        started: std::time::Instant::now(),
        origin,
        task,
      },
    );
  }

  async fn prepare_edit(&mut self, source: EditSource) -> Result<Prepared, EditFailure> {
    if self.editors.deps.is_none() {
      return Err(EditFailure::new(
        EditFailKind::Unavailable,
        "editing is not available (no runtime directory)",
      ));
    }
    if self.editors.sessions.len() >= MAX_SESSIONS {
      return Err(EditFailure::new(
        EditFailKind::Busy,
        format!("too many open edit sessions (at most {MAX_SESSIONS})"),
      ));
    }
    let (original, mime, data) = match source {
      EditSource::Item { id, mime } => {
        let item: Item = match self.store.call(move |s| s.get(id)).await {
          Ok(Ok(Some(item))) => item,
          Ok(Ok(None)) => {
            return Err(EditFailure::new(EditFailKind::NotFound, "that item no longer exists"));
          }
          Ok(Err(e)) => {
            tracing::error!(%id, "edit: reading the item failed: {e}");
            return Err(EditFailure::new(EditFailKind::Internal, "could not read the item"));
          }
          Err(e) => {
            tracing::error!(%id, "edit: {e}");
            return Err(EditFailure::new(EditFailKind::Unavailable, "store unavailable"));
          }
        };
        let rep = match &mime {
          Some(m) => item.resolve(m).map(|d| (m.clone(), d.to_vec())),
          None => item.preferred().map(|(m, d)| (m.to_owned(), d.to_vec())),
        };
        let Some((m, d)) = rep else {
          return Err(EditFailure::new(EditFailKind::NotFound, "that format is not available"));
        };
        (Some(id), m, d)
      }
      EditSource::New { mime } => {
        if let Err(why) = validate_mime(&mime) {
          return Err(EditFailure::new(EditFailKind::BadRequest, why));
        }
        (None, mime, Vec::new())
      }
    };
    let edit_mime = edit_mime(&mime);
    // Text is edited as UTF-8.
    let data = match kind_of(&edit_mime) {
      EditKind::Text | EditKind::Html => match String::from_utf8(data) {
        Ok(s) => s.into_bytes(),
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned().into_bytes(),
      },
      EditKind::Image | EditKind::Other => data,
    };
    // Checked before anything touches the disk.
    if let Err(e) = self.config.editor.command_for(&edit_mime, Path::new("/")) {
      return Err(EditFailure::new(EditFailKind::NoEditor, e.to_string()));
    }
    Ok(Prepared { original, edit_mime, data })
  }

  /// `PickerReq::Edit`. A waiting `spoolctl edit` gets the outcome too; a
  /// waiting `spoolctl pick` is cancelled (the user chose to edit).
  pub(super) async fn picker_edit(&mut self, id: ItemId, mime: String, reply: EditReply) {
    let source = EditSource::Item { id, mime: Some(mime) };
    match self.pick.take() {
      Some(pick) if self.pick_edit && !pick.is_closed() => {
        let (tx, rx) = oneshot::channel();
        self.start_edit(EditOrigin::Socket, source, tx).await;
        tokio::spawn(async move {
          let r = rx.await.unwrap_or_else(|_| {
            Err(EditFailure::new(EditFailKind::Internal, "the edit session was dropped"))
          });
          let _ = pick.send(match r.clone() {
            Ok(()) => PublicResp::Ok,
            Err(f) => failure_resp(f),
          });
          let _ = reply.send(r);
        });
      }
      other => {
        if let Some(pick) = other {
          let _ = pick.send(PublicResp::Cancelled);
        }
        self.start_edit(EditOrigin::Picker, source, reply).await;
      }
    }
  }

  /// `Select` while a `spoolctl edit` waits: edit the preferred
  /// representation.
  pub(super) async fn select_edit(
    &mut self,
    id: ItemId,
    pick: oneshot::Sender<PublicResp>,
    reply: SelectReply,
  ) {
    let (tx, rx) = oneshot::channel();
    self.start_edit(EditOrigin::Socket, EditSource::Item { id, mime: None }, tx).await;
    reply_to_public(rx, pick);
    let _ = reply.send(Ok(SelectOutcome::Returned));
  }

  /// `PublicReq::New`.
  pub(super) async fn start_new(&mut self, mime: String, reply: oneshot::Sender<PublicResp>) {
    let (tx, rx) = oneshot::channel();
    self.start_edit(EditOrigin::Socket, EditSource::New { mime }, tx).await;
    reply_to_public(rx, reply);
  }

  pub(super) async fn on_editor_event(&mut self, ev: EditorEvent) {
    match ev {
      EditorEvent::Saved { sid, data } => self.editor_saved(sid, data).await,
      EditorEvent::Exited { sid, exit } => self.editor_exited(sid, exit).await,
      EditorEvent::LaunchFailed { sid, why } => {
        tracing::warn!(sid, "editor not started: {why}");
        if let Some(s) = self.editors.sessions.remove(&sid) {
          remove_dir(&s.dir);
          if s.origin == EditOrigin::Picker {
            self.edit_notify(
              "Spool: cannot edit",
              "The editor could not be started; see `journalctl --user -u spool`.",
            );
          }
        }
      }
    }
  }

  fn edit_refused(&self, sid: u64, reason: &str) {
    tracing::info!(sid, reason, "edit not saved");
    self.edit_notify("Spool: edit not saved", &refusal_body(reason));
  }

  async fn editor_saved(&mut self, sid: u64, data: Result<Vec<u8>, ReadIssue>) {
    let Some(s) = self.editors.sessions.get_mut(&sid) else { return };
    s.saves += 1;
    let (edit_mime, original, current) = (s.edit_mime.clone(), s.original, s.item);
    let data = match data {
      Ok(d) => Bytes::from(d),
      Err(ReadIssue::TooLarge) => {
        return self.edit_refused(sid, &refusal_reason(&DropReason::TooLarge));
      }
      Err(ReadIssue::Unreadable) => return,
    };
    let reps: Vec<(String, Bytes)> = match kind_of(&edit_mime) {
      EditKind::Text => {
        if std::str::from_utf8(&data).is_err() {
          return self.edit_refused(sid, "the file is not valid UTF-8 text");
        }
        vec![(edit_mime.clone(), data)]
      }
      EditKind::Html => {
        let Ok(html) = std::str::from_utf8(&data) else {
          return self.edit_refused(sid, "the file is not valid UTF-8 text");
        };
        let text = html_to_text(html);
        let mut v = vec![(edit_mime.clone(), data.clone())];
        if !text.trim().is_empty() {
          v.push((UTF8_TEXT.to_owned(), Bytes::from(text.into_bytes())));
        }
        v
      }
      EditKind::Image => match saved_image_mime(&edit_mime, &data) {
        Some(m) => vec![(m.to_owned(), data)],
        None => return self.edit_refused(sid, "the file is not a PNG, JPEG or WebP image"),
      },
      EditKind::Other => vec![(edit_mime.clone(), data)],
    };
    let now = SystemTime::now();
    let mut item = match self.policy.manual_reps(Selection::Clipboard, reps, now) {
      Ok(item) => item,
      Err(reason) => return self.edit_refused(sid, &refusal_reason(&reason)),
    };
    item.source_app = Some(EDITOR_SOURCE_APP.to_owned());
    let hp = hash_prefix(&item.hash);
    let size = item.total_size();
    let want_preview = self.picker.wants_items();
    let res = self
      .store
      .call(move |s| -> spool_core::Result<_> {
        let (id, owned, what) = match current {
          Some((id, true)) => match s.replace_content(id, item.clone())? {
            Some(ReplaceOutcome::Replaced(id)) => (id, true, "replaced"),
            Some(ReplaceOutcome::Merged(other)) => (other, false, "merged"),
            // Deleted meanwhile (picker, retention): start over.
            None => match s.insert_derived(item, original)? {
              InsertOutcome::Inserted(id) => (id, true, "inserted"),
              InsertOutcome::Bumped(id) => (id, false, "bumped"),
            },
          },
          _ => match s.insert_derived(item, original)? {
            InsertOutcome::Inserted(id) => (id, true, "inserted"),
            InsertOutcome::Bumped(id) => (id, false, "bumped"),
          },
        };
        let preview =
          if want_preview { s.summaries(&[id]).ok().and_then(|mut v| v.pop()) } else { None };
        Ok((id, owned, what, preview))
      })
      .await;
    match res {
      Ok(Ok((id, owned, what, preview))) => {
        tracing::info!(sid, %id, hash = %hp, bytes = size, what, "edit saved");
        if let Some(s) = self.editors.sessions.get_mut(&sid) {
          s.item = Some((id, owned));
        }
        if let Some(p) = preview {
          self.picker.item_stored(index::to_preview(p));
        }
      }
      Ok(Err(e)) => {
        tracing::error!(sid, hash = %hp, "storing an edit failed: {e}");
        self.edit_notify("Spool: edit not saved", "Storing it failed; see the spool log.");
      }
      Err(e) => tracing::error!(sid, "storing an edit: {e}"),
    }
  }

  async fn editor_exited(&mut self, sid: u64, exit: EditorExit) {
    let Some(s) = self.editors.sessions.remove(&sid) else { return };
    remove_dir(&s.dir);
    match (exit, s.item) {
      (EditorExit::Success, Some((id, _))) => match self.publish_item(id, SelectMode::Copy).await {
        Ok(()) => tracing::info!(sid, %id, "editor closed; edited item is on the clipboard"),
        Err(e) => tracing::warn!(sid, %id, "editor closed; publishing the edit failed: {e}"),
      },
      (EditorExit::Success, None) if s.saves == 0 && s.started.elapsed() < QUICK_EXIT => {
        tracing::info!(sid, "editor exited immediately without saving");
        self.edit_notify(
          "Spool: editor closed immediately",
          "Nothing was saved. The editor command must wait until you are done \
           (e.g. kate --block, code --wait, a terminal that does not reuse a running instance).",
        );
      }
      (EditorExit::Success, None) => tracing::info!(sid, "editor closed; nothing new to store"),
      (EditorExit::Failed(why), item) => {
        tracing::warn!(sid, "editor failed: {why}");
        if item.is_some() {
          self.edit_notify(
            "Spool: editor failed",
            "The last saved version is kept in history but was not put on the clipboard.",
          );
        }
      }
    }
  }
}
