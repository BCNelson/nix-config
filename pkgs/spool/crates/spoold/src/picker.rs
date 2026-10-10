//! The resident picker: spoold's side of picker protocol v3
//! (INTERFACES.md "Picker channel").
//!
//! # Launch
//!
//! [`locate`] finds `spool-picker` **only** on spoold's audited PATH
//! ([`security::find_on_audited_path`]; never next to spoold's own binary,
//! never from a config file). Not found -> the orchestrator keeps
//! [`crate::autopaste::UnwiredPicker`] (headless / CLI-only installs).
//!
//! [`ResidentPicker::start`] spawns a host task that creates a
//! `socketpair(AF_UNIX, SOCK_STREAM)` and execs the picker once (it stays
//! resident) with:
//! - the child end on fd 3 ([`security::pass_fd_as_3`]),
//! - an environment cleared down to `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`,
//!   `XDG_CONFIG_HOME`, `HOME` (kdeglobals), `LANG`, `LC_*` and
//!   `SPOOL_PICKER_LOG`, plus `SPOOL_PICKER_FD=3`,
//!   `SPOOL_PICKER_RENDERER` (`[picker] renderer`) and
//!   `SPOOL_PICKER_PRERENDER` (`[picker] prerender`),
//! - stdin/stdout `/dev/null`, stderr inherited (the journal).
//!
//! The picker sends `Hello{proto: 3}` first; the host answers
//! `Hello::picker()` and closes after it on a mismatch (and stops
//! restarting: a version mismatch does not heal).
//!
//! # Restart
//!
//! EOF / a broken frame = the picker is gone: it is killed (if still
//! running) and reaped, a visible picker is reported as
//! `Hidden{Closed}`, and it is restarted after a backoff (250 ms doubling to
//! 30 s, reset after a run of 60 s), at most [`MAX_RESTARTS`] times per
//! [`RESTART_WINDOW`]. A `gpu` picker that dies before `Ready` is relaunched
//! with `SPOOL_PICKER_RENDERER=software`. While it is down a `Show` is kept
//! and delivered after the next handshake if it is still fresh.
//!
//! # Messages
//!
//! - `Query` -> `Request::Search` with `limit + 1` (-> `more`), answered
//!   with `Page` or `Error{seq}` (`BadQuery` / `Internal`); `Thumb` ->
//!   `Request::Thumb` (image representations only, <= [`MAX_THUMB_BYTES`]);
//!   every `seq` gets exactly one answer.
//! - `Select` -> `Request::Select` (the picker already hid itself);
//!   `Pin`/`Delete`/`Tag` -> `Request::Edit` (the store's change feed
//!   updates the index); `Hidden` -> visibility + `Request::PickerHidden`
//!   (cancels a `spoolctl pick`); `Edit{id, mime}` -> `Request::EditItem`
//!   (external editor, [`crate::editor`]; a failure comes back as
//!   `Error{seq: None}`). All are forwarded in arrival order.
//! - Pushed: `Show`/`Hide` (orchestrator), `NewItem` (stored clipboard
//!   items), `IndexProgress` (index rebuild), `Locked{prompts}` /
//!   `Unlocked` (key state; prompts from `keyslots.json`, see
//!   [`unlock::prompts`]).
//! - `Unlock` -> [`unlock::attempt`] on its own task (one per provider at a
//!   time; a FIDO2 touch request while one is waiting is ignored, a PIN
//!   arriving meanwhile is queued); success goes to the orchestrator as
//!   `KeyEvent::Opened` (the key flow's merge + switch), and the picker gets
//!   `Unlocked` once the orchestrator reports the history readable.
//!
//! Frames from the picker are read into zeroize-on-drop buffers (they may
//! carry a passphrase / PIN) and capped at [`MAX_REQ_FRAME`]. Never logs
//! queries, item content or secrets.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use spool_core::config::{PickerRenderer, PickerSettings};
use spool_core::item::ItemId;
use spool_crypto::Zeroizing;
use spool_proto::Hello;
use spool_proto::{
  CursorPos, FrameError, HideReason, ItemPreview, PickerErrorCode, PickerEvt, PickerReq,
  UnlockFailReason, UnlockPrompt, UnlockProvider, UnlockSecret, decode_payload, read_frame_async,
  write_frame_async,
};
use tokio::io::AsyncReadExt;
use tokio::net::unix::OwnedReadHalf;
use tokio::sync::{mpsc, oneshot, watch};

use crate::autopaste::{LockView, PickerError, PickerLauncher, SelectError, ShowContext};
use crate::editor::EditFailKind;
use crate::index::{self, IndexStatus, SearchError};
use crate::keyflow::{KeyEvent, OpenGate};
use crate::orchestrator::{EditOp, Request};
use crate::security;
use crate::unlock::{self, Attempt, UnlockProviders};

/// Picker executable name (looked up on the audited PATH only).
pub const PICKER_BIN: &str = "spool-picker";
/// Largest thumbnail source sent to the picker.
pub const MAX_THUMB_BYTES: usize = 8 * 1024 * 1024;
/// Largest frame accepted from the picker (requests are small).
pub const MAX_REQ_FRAME: usize = 64 * 1024;
/// Restarts allowed per [`RESTART_WINDOW`] before pausing.
pub const MAX_RESTARTS: usize = 5;
pub const RESTART_WINDOW: Duration = Duration::from_secs(60);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const MIN_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// A run this long counts as healthy (backoff resets).
const STABLE_RUN: Duration = Duration::from_secs(60);
/// A `Show` that arrived while the picker was down is delivered after the
/// next handshake only if it is younger than this.
const SHOW_FRESH: Duration = Duration::from_secs(5);
/// Longest wait for a killed picker to be reaped.
const REAP_WAIT: Duration = Duration::from_secs(2);

/// `spool-picker` on spoold's audited PATH, if any (logged once).
pub fn locate() -> Option<PathBuf> {
  let found = security::find_on_audited_path(PICKER_BIN);
  match &found {
    Some(p) => tracing::info!(exe = %p.display(), "picker found"),
    None => tracing::info!(
      "{PICKER_BIN} is not on spoold's PATH: no picker (Show/Pick unavailable; the CLI works)"
    ),
  }
  found
}

/// A launched picker connection end (+ the process, if any).
pub struct Spawned {
  pub sock: StdUnixStream,
  pub child: Option<tokio::process::Child>,
}

/// Starts a picker process (tests use an in-process fake).
pub trait Spawner: Send + Sync + 'static {
  fn spawn(&mut self, renderer: PickerRenderer) -> std::io::Result<Spawned>;
}

/// The real spawner: `exe` with fd 3 and a scrubbed environment.
pub struct ProcessSpawner {
  exe: PathBuf,
  prerender: bool,
}

impl ProcessSpawner {
  pub fn new(exe: PathBuf, settings: &PickerSettings) -> Self {
    Self { exe, prerender: settings.prerender }
  }
}

/// Environment variables passed through to the picker (plus `LC_*`).
const ENV_ALLOW: &[&str] =
  &["WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "HOME", "LANG", "SPOOL_PICKER_LOG"];

/// The picker's complete environment, from spoold's `vars`.
pub fn picker_env(
  vars: impl Iterator<Item = (OsString, OsString)>,
  renderer: PickerRenderer,
  prerender: bool,
) -> Vec<(OsString, OsString)> {
  let mut out: Vec<(OsString, OsString)> = vars
    .filter(|(k, _)| {
      let k = k.to_string_lossy();
      ENV_ALLOW.contains(&k.as_ref()) || k.starts_with("LC_")
    })
    .collect();
  out.push(("SPOOL_PICKER_FD".into(), "3".into()));
  out.push(("SPOOL_PICKER_RENDERER".into(), renderer.as_str().into()));
  out.push(("SPOOL_PICKER_PRERENDER".into(), if prerender { "1" } else { "0" }.into()));
  out
}

impl Spawner for ProcessSpawner {
  fn spawn(&mut self, renderer: PickerRenderer) -> std::io::Result<Spawned> {
    let (ours, theirs) = StdUnixStream::pair()?;
    let theirs: OwnedFd = theirs.into();
    let mut cmd = tokio::process::Command::new(&self.exe);
    cmd
      .env_clear()
      .envs(picker_env(std::env::vars_os(), renderer, self.prerender))
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::inherit())
      .kill_on_drop(true);
    security::pass_fd_as_3(&mut cmd, theirs.as_raw_fd());
    let child = cmd.spawn()?;
    // Our copy of the child's end must go, or EOF is never seen.
    drop(theirs);
    tracing::info!(pid = child.id(), renderer = renderer.as_str(), "picker started");
    Ok(Spawned { sock: ours, child: Some(child) })
  }
}

/// What the picker host needs for unlocking (absent without a key flow).
#[derive(Clone)]
pub struct UnlockCtx {
  /// State directory (`keyslots.json`, the store).
  pub dir: PathBuf,
  /// The orchestrator's key-event channel (shared with the key flow).
  pub key_tx: mpsc::Sender<KeyEvent>,
  pub gate: OpenGate,
  pub providers: Arc<dyn UnlockProviders>,
}

/// The host's links to the rest of the daemon.
pub struct HostDeps {
  pub requests: mpsc::Sender<Request>,
  pub index_status: watch::Receiver<IndexStatus>,
  pub unlock: Option<UnlockCtx>,
}

enum HostCmd {
  Show(PickerEvt),
  Hide,
  NewItem(ItemPreview),
  Lock(LockView),
}

#[derive(Default)]
struct Shared {
  /// Shown (a `Show` was sent, no `Hidden` since).
  visible: AtomicBool,
  /// Gave up (protocol mismatch): `show` fails.
  dead: AtomicBool,
}

/// [`PickerLauncher`] backed by the resident `spool-picker` process.
pub struct ResidentPicker {
  cmds: mpsc::UnboundedSender<HostCmd>,
  shared: Arc<Shared>,
  task: tokio::task::JoinHandle<()>,
}

impl ResidentPicker {
  /// Start the host task (launches the picker right away). Call inside the
  /// runtime, after Wayland is up.
  pub fn start(spawner: Box<dyn Spawner>, deps: HostDeps, settings: &PickerSettings) -> Self {
    let (cmds, rx) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared::default());
    let host = Host::new(spawner, deps, settings.renderer, shared.clone());
    let task = tokio::spawn(host.run(rx));
    Self { cmds, shared, task }
  }
}

impl Drop for ResidentPicker {
  fn drop(&mut self) {
    // Drops the connection and the child (kill_on_drop).
    self.task.abort();
  }
}

impl PickerLauncher for ResidentPicker {
  fn show(&mut self, ctx: &ShowContext) -> Result<(), PickerError> {
    if self.shared.dead.load(Ordering::SeqCst) {
      return Err(PickerError::Failed("the picker could not be started".into()));
    }
    let evt = PickerEvt::Show {
      cursor: ctx.cursor.map(|(x, y)| CursorPos { x, y }),
      output: None,
      target_window: ctx.target.as_ref().map(|t| t.window_id.clone()),
      scale_hint: None,
    };
    self.shared.visible.store(true, Ordering::SeqCst);
    self.cmds.send(HostCmd::Show(evt)).map_err(|_| PickerError::Failed("picker host gone".into()))
  }

  fn hide(&mut self) {
    if self.shared.visible.load(Ordering::SeqCst) {
      let _ = self.cmds.send(HostCmd::Hide);
    }
  }

  fn wants_items(&self) -> bool {
    !self.cmds.is_closed()
  }

  fn item_stored(&mut self, preview: ItemPreview) {
    let _ = self.cmds.send(HostCmd::NewItem(preview));
  }

  fn lock_changed(&mut self, lock: LockView) {
    let _ = self.cmds.send(HostCmd::Lock(lock));
  }
}

/// How one picker connection ended.
enum End {
  /// spoold is shutting down.
  Shutdown,
  /// Incompatible protocol: do not restart.
  Incompatible,
  /// The picker went away (crash, exit, broken frame).
  Lost { ready: bool },
}

type UnlockResult = (UnlockProvider, UnlockRes);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnlockRes {
  /// Handed to the orchestrator (or opened already): wait for `Unlocked`.
  Opened,
  NeedsPin,
  Failed(UnlockFailReason),
}

#[derive(Default)]
struct UnlockState {
  pass_busy: bool,
  fido_busy: bool,
  kwallet_busy: bool,
  /// A PIN that arrived while a FIDO2 attempt was running.
  fido_queued: Option<Zeroizing<String>>,
  /// A key asked for its PIN: prompt `Fido2Pin` from now on.
  need_pin: bool,
  /// The attempt whose store went to the orchestrator.
  awaiting: Option<UnlockProvider>,
}

struct Host {
  spawner: Box<dyn Spawner>,
  requests: mpsc::Sender<Request>,
  index_status: Option<watch::Receiver<IndexStatus>>,
  unlock_ctx: Option<UnlockCtx>,
  shared: Arc<Shared>,
  renderer: PickerRenderer,
  lock: LockView,
  pending_show: Option<(PickerEvt, Instant)>,
  unlock: UnlockState,
  unlock_tx: mpsc::UnboundedSender<UnlockResult>,
  unlock_rx: Option<mpsc::UnboundedReceiver<UnlockResult>>,
  restarts: VecDeque<Instant>,
  backoff: Duration,
  /// Per connection: the prompt set last sent (`None` = not sent yet).
  sent_prompts: Option<Vec<UnlockPrompt>>,
  /// Per connection: index rebuild progress was reported (for the final
  /// `done == total`).
  progress_total: Option<u64>,
  ready: bool,
}

/// Sends to the connected picker (dropped silently when none).
type Out = mpsc::UnboundedSender<PickerEvt>;

impl Host {
  fn new(
    spawner: Box<dyn Spawner>,
    deps: HostDeps,
    renderer: PickerRenderer,
    shared: Arc<Shared>,
  ) -> Self {
    let (unlock_tx, unlock_rx) = mpsc::unbounded_channel();
    Self {
      spawner,
      requests: deps.requests,
      index_status: Some(deps.index_status),
      unlock_ctx: deps.unlock,
      shared,
      renderer,
      lock: LockView::Unlocked,
      pending_show: None,
      unlock: UnlockState::default(),
      unlock_tx,
      unlock_rx: Some(unlock_rx),
      restarts: VecDeque::new(),
      backoff: MIN_BACKOFF,
      sent_prompts: None,
      progress_total: None,
      ready: false,
    }
  }

  async fn run(mut self, mut cmds: mpsc::UnboundedReceiver<HostCmd>) {
    let mut unlock_rx = self.unlock_rx.take().expect("run once");
    let mut index_status = self.index_status.take().expect("run once");
    let mut paused_logged = false;
    loop {
      // Restart budget.
      let now = Instant::now();
      while self.restarts.front().is_some_and(|t| now.duration_since(*t) > RESTART_WINDOW) {
        self.restarts.pop_front();
      }
      if self.restarts.len() >= MAX_RESTARTS {
        let wait = RESTART_WINDOW.saturating_sub(now.duration_since(self.restarts[0]));
        if !paused_logged {
          tracing::error!(
            restarts = MAX_RESTARTS,
            window_s = RESTART_WINDOW.as_secs(),
            "the picker keeps dying; pausing restarts"
          );
          paused_logged = true;
        }
        if !self.idle(wait, &mut cmds, &mut unlock_rx).await {
          return;
        }
        continue;
      }
      self.restarts.push_back(now);
      let spawned = match self.spawner.spawn(self.renderer) {
        Ok(s) => s,
        Err(e) => {
          tracing::warn!("starting {PICKER_BIN}: {e}");
          let d = self.next_backoff();
          if !self.idle(d, &mut cmds, &mut unlock_rx).await {
            return;
          }
          continue;
        }
      };
      let started = Instant::now();
      let end = self.serve(spawned, &mut cmds, &mut unlock_rx, &mut index_status).await;
      if self.shared.visible.swap(false, Ordering::SeqCst) {
        let _ = self.requests.send(Request::PickerHidden { reason: HideReason::Closed }).await;
      }
      match end {
        End::Shutdown => return,
        End::Incompatible => {
          self.shared.dead.store(true, Ordering::SeqCst);
          // Keep draining commands so the orchestrator never blocks.
          while self.idle(Duration::from_secs(3600), &mut cmds, &mut unlock_rx).await {}
          return;
        }
        End::Lost { ready } => {
          if !ready && self.renderer == PickerRenderer::Gpu {
            tracing::warn!("the gpu picker died before it was ready; relaunching it with software");
            self.renderer = PickerRenderer::Software;
          }
          if started.elapsed() >= STABLE_RUN {
            self.backoff = MIN_BACKOFF;
            paused_logged = false;
          }
          let d = self.next_backoff();
          tracing::info!(retry_ms = d.as_millis() as u64, "restarting the picker");
          if !self.idle(d, &mut cmds, &mut unlock_rx).await {
            return;
          }
        }
      }
    }
  }

  fn next_backoff(&mut self) -> Duration {
    let d = self.backoff;
    self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
    d
  }

  /// Wait `d` without a picker, still taking commands. `false` = shutdown.
  async fn idle(
    &mut self,
    d: Duration,
    cmds: &mut mpsc::UnboundedReceiver<HostCmd>,
    unlock_rx: &mut mpsc::UnboundedReceiver<UnlockResult>,
  ) -> bool {
    let until = tokio::time::Instant::now() + d;
    loop {
      tokio::select! {
        () = tokio::time::sleep_until(until) => return true,
        cmd = cmds.recv() => match cmd {
          None => return false,
          Some(HostCmd::Show(evt)) => self.pending_show = Some((evt, Instant::now())),
          Some(HostCmd::Hide) => {
            self.pending_show = None;
            self.shared.visible.store(false, Ordering::SeqCst);
          }
          Some(HostCmd::NewItem(_)) => {}
          Some(HostCmd::Lock(l)) => self.set_lock(l, None),
        },
        Some(res) = unlock_rx.recv() => self.on_unlock_result(res, None),
      }
    }
  }

  /// One picker connection, from handshake to EOF.
  async fn serve(
    &mut self,
    spawned: Spawned,
    cmds: &mut mpsc::UnboundedReceiver<HostCmd>,
    unlock_rx: &mut mpsc::UnboundedReceiver<UnlockResult>,
    index_status: &mut watch::Receiver<IndexStatus>,
  ) -> End {
    let Spawned { sock, mut child } = spawned;
    let end = self.serve_conn(sock, cmds, unlock_rx, index_status).await;
    if let Some(c) = child.as_mut() {
      reap(c).await;
    }
    end
  }

  async fn serve_conn(
    &mut self,
    sock: StdUnixStream,
    cmds: &mut mpsc::UnboundedReceiver<HostCmd>,
    unlock_rx: &mut mpsc::UnboundedReceiver<UnlockResult>,
    index_status: &mut watch::Receiver<IndexStatus>,
  ) -> End {
    let stream =
      match sock.set_nonblocking(true).and_then(|()| tokio::net::UnixStream::from_std(sock)) {
        Ok(s) => s,
        Err(e) => {
          tracing::warn!("picker socket: {e}");
          return End::Lost { ready: false };
        }
      };
    let (mut rd, mut wr) = stream.into_split();
    // Handshake: the picker speaks first.
    let hello: Hello =
      match tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame_async(&mut rd)).await {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
          tracing::warn!("picker handshake: {e}");
          return End::Lost { ready: false };
        }
        Err(_) => {
          tracing::warn!("picker did not say hello within {} s", HANDSHAKE_TIMEOUT.as_secs());
          return End::Lost { ready: false };
        }
      };
    if write_frame_async(&mut wr, &Hello::picker()).await.is_err() {
      return End::Lost { ready: false };
    }
    if !hello.is_picker_compatible() {
      tracing::error!(
        theirs = hello.proto,
        ours = Hello::picker().proto,
        "{PICKER_BIN} speaks another picker protocol version; not restarting it (install \
         matching spool and spool-picker packages)"
      );
      return End::Incompatible;
    }
    tracing::debug!("picker connected");

    let (req_tx, mut req_rx) = mpsc::unbounded_channel::<PickerReq>();
    let reader = tokio::spawn(async move {
      loop {
        match read_req(&mut rd).await {
          Ok(r) => {
            if req_tx.send(r).is_err() {
              return;
            }
          }
          Err(FrameError::Eof) => return,
          Err(e) => {
            tracing::warn!("picker channel: {e}");
            return;
          }
        }
      }
    });
    let (out, mut out_rx) = mpsc::unbounded_channel::<PickerEvt>();
    let writer = tokio::spawn(async move {
      while let Some(evt) = out_rx.recv().await {
        if write_frame_async(&mut wr, &evt).await.is_err() {
          return;
        }
      }
    });

    self.ready = false;
    self.sent_prompts = None;
    self.progress_total = None;
    if self.lock != LockView::Unlocked {
      self.send_prompts(&out, true);
    }
    let st = *index_status.borrow_and_update();
    self.on_index_status(st, &out);
    if let Some((evt, at)) = self.pending_show.take()
      && at.elapsed() < SHOW_FRESH
    {
      self.shared.visible.store(true, Ordering::SeqCst);
      let _ = out.send(evt);
    }

    let end = loop {
      tokio::select! {
        req = req_rx.recv() => match req {
          Some(r) => self.on_req(r, &out).await,
          None => break End::Lost { ready: self.ready },
        },
        cmd = cmds.recv() => match cmd {
          None => break End::Shutdown,
          Some(HostCmd::Show(evt)) => { let _ = out.send(evt); }
          Some(HostCmd::Hide) => { let _ = out.send(PickerEvt::Hide); }
          Some(HostCmd::NewItem(p)) => { let _ = out.send(PickerEvt::NewItem { preview: p }); }
          Some(HostCmd::Lock(l)) => self.set_lock(l, Some(&out)),
        },
        Ok(()) = index_status.changed() => {
          let st = *index_status.borrow_and_update();
          self.on_index_status(st, &out);
        }
        Some(res) = unlock_rx.recv() => self.on_unlock_result(res, Some(&out)),
      }
    };
    // Let queued events go out before closing on shutdown (EOF tells the
    // picker to exit).
    drop(out);
    if matches!(end, End::Shutdown) {
      let _ = tokio::time::timeout(Duration::from_millis(500), writer).await;
    } else {
      writer.abort();
    }
    reader.abort();
    end
  }

  fn on_index_status(&mut self, st: IndexStatus, out: &Out) {
    match st {
      IndexStatus::Rebuilding(p) => {
        self.progress_total = Some(p.total);
        let _ = out.send(PickerEvt::IndexProgress { done: p.done, total: p.total });
      }
      _ => {
        if let Some(total) = self.progress_total.take() {
          let _ = out.send(PickerEvt::IndexProgress { done: total, total });
        }
      }
    }
  }

  /// Send `Locked{prompts}` if the prompt set changed (or `force`).
  fn send_prompts(&mut self, out: &Out, force: bool) {
    let prompts = match &self.unlock_ctx {
      Some(ctx) => unlock::prompts(&ctx.dir, self.unlock.need_pin),
      None => Vec::new(),
    };
    if force || self.sent_prompts.as_ref() != Some(&prompts) {
      tracing::debug!(?prompts, "picker: locked");
      self.sent_prompts = Some(prompts.clone());
      let _ = out.send(PickerEvt::Locked { providers: prompts });
    }
  }

  fn set_lock(&mut self, lock: LockView, out: Option<&Out>) {
    let prev = std::mem::replace(&mut self.lock, lock);
    match lock {
      LockView::Unlocked => {
        self.unlock.awaiting = None;
        self.unlock.need_pin = false;
        self.unlock.fido_queued = None;
        if prev != LockView::Unlocked
          && let Some(out) = out
        {
          tracing::debug!("picker: unlocked");
          self.sent_prompts = None;
          let _ = out.send(PickerEvt::Unlocked);
        }
      }
      LockView::Locked { failed } => {
        if failed && let Some(p) = self.unlock.awaiting.take() {
          // The store opened but the switch failed (merge error).
          if let Some(out) = out {
            let _ =
              out.send(PickerEvt::UnlockFailed { provider: p, reason: UnlockFailReason::Other });
          }
        }
        if let Some(out) = out {
          self.send_prompts(out, prev == LockView::Unlocked);
        }
      }
    }
  }

  async fn forward(&self, req: Request) -> bool {
    self.requests.send(req).await.is_ok()
  }

  async fn on_req(&mut self, req: PickerReq, out: &Out) {
    match req {
      PickerReq::Ready => {
        tracing::debug!("picker ready");
        self.ready = true;
      }
      PickerReq::Query { seq, q, filters, offset, limit } => {
        let lim = limit.min(index::MAX_LIMIT - 1);
        let (reply, rx) = oneshot::channel();
        let req = Request::Search { q, filters, offset, limit: lim + 1, reply };
        if !self.forward(req).await {
          let _ = out.send(err(Some(seq), PickerErrorCode::Unavailable, "spoold is shutting down"));
          return;
        }
        let out = out.clone();
        tokio::spawn(async move {
          let evt = match rx.await {
            Ok(Ok(mut items)) => {
              let more = items.len() > lim as usize;
              items.truncate(lim as usize);
              PickerEvt::Page { seq, offset, items, more }
            }
            // spool-search's messages never contain the query.
            Ok(Err(SearchError::BadQuery(m))) => err(Some(seq), PickerErrorCode::BadQuery, &m),
            Ok(Err(SearchError::Internal(m))) => {
              tracing::warn!("picker search failed: {m}");
              err(Some(seq), PickerErrorCode::Internal, "search failed")
            }
            Err(_) => err(Some(seq), PickerErrorCode::Unavailable, "search was dropped"),
          };
          let _ = out.send(evt);
        });
      }
      PickerReq::Thumb { seq, id, mime } => {
        let (reply, rx) = oneshot::channel();
        if !self.forward(Request::Thumb { id: ItemId(id), mime, reply }).await {
          let _ = out.send(err(Some(seq), PickerErrorCode::Unavailable, "spoold is shutting down"));
          return;
        }
        let out = out.clone();
        tokio::spawn(async move {
          let evt = match rx.await {
            Ok(Ok(bytes)) => PickerEvt::Thumb { seq, id, bytes },
            Ok(Err((code, m))) => err(Some(seq), code, m),
            Err(_) => err(Some(seq), PickerErrorCode::Unavailable, "thumbnail was dropped"),
          };
          let _ = out.send(evt);
        });
      }
      PickerReq::Select { id, mode } => {
        let (reply, rx) = oneshot::channel();
        if !self.forward(Request::Select { id: ItemId(id), mode, reply }).await {
          return;
        }
        let out = out.clone();
        tokio::spawn(async move {
          match rx.await {
            Ok(Ok(outcome)) => tracing::debug!(?outcome, "picker select done"),
            Ok(Err(e)) => {
              tracing::info!("picker select failed: {e}");
              let (code, m) = match e {
                SelectError::NotFound => (PickerErrorCode::NotFound, "That item no longer exists"),
                SelectError::NoData => (PickerErrorCode::NotFound, "Nothing to paste in this mode"),
                SelectError::Unavailable(_) => {
                  (PickerErrorCode::Unavailable, "Could not set the clipboard")
                }
                SelectError::Internal(_) => (PickerErrorCode::Internal, "Selecting failed"),
              };
              let _ = out.send(err(None, code, m));
            }
            Err(_) => {}
          }
        });
      }
      PickerReq::Pin { id, on } => self.edit(id, EditOp::Pin(on), out).await,
      PickerReq::Delete { id } => self.edit(id, EditOp::Delete, out).await,
      PickerReq::Tag { id, tag, on } => self.edit(id, EditOp::Tag { tag, on }, out).await,
      PickerReq::Unlock { provider, secret } => self.on_unlock(provider, secret, out),
      PickerReq::Hidden { reason } => {
        tracing::debug!(?reason, "picker hidden");
        self.shared.visible.store(false, Ordering::SeqCst);
        let _ = self.forward(Request::PickerHidden { reason }).await;
      }
      PickerReq::Edit { id, mime } => {
        let (reply, rx) = oneshot::channel();
        if !self.forward(Request::EditItem { id: ItemId(id), mime, reply }).await {
          return;
        }
        let out = out.clone();
        tokio::spawn(async move {
          if let Ok(Err(f)) = rx.await {
            let code = match f.kind {
              EditFailKind::NotFound => PickerErrorCode::NotFound,
              EditFailKind::Internal => PickerErrorCode::Internal,
              _ => PickerErrorCode::Unavailable,
            };
            // Short and content-free (mime, program name at most).
            let _ = out.send(err(None, code, &f.message));
          }
        });
      }
    }
  }

  async fn edit(&self, id: i64, op: EditOp, out: &Out) {
    let (reply, rx) = oneshot::channel();
    if !self.forward(Request::Edit { id: ItemId(id), op, reply }).await {
      return;
    }
    let out = out.clone();
    tokio::spawn(async move {
      if let Ok(Err((code, m))) = rx.await {
        let _ = out.send(err(None, code, m));
      }
    });
  }

  fn on_unlock(&mut self, provider: UnlockProvider, secret: Option<UnlockSecret>, out: &Out) {
    if self.lock == LockView::Unlocked {
      let _ = out.send(PickerEvt::Unlocked);
      return;
    }
    let Some(ctx) = self.unlock_ctx.clone() else {
      let _ = out.send(PickerEvt::UnlockFailed { provider, reason: UnlockFailReason::Unavailable });
      return;
    };
    let fail = |reason| PickerEvt::UnlockFailed { provider, reason };
    let key_provider = match (provider, secret) {
      (UnlockProvider::Passphrase, Some(s)) => {
        if self.unlock.pass_busy {
          tracing::debug!("passphrase unlock already running; ignored");
          return;
        }
        let s = s.into_inner();
        if s.is_empty() {
          let _ = out.send(fail(UnlockFailReason::WrongSecret));
          return;
        }
        self.unlock.pass_busy = true;
        ctx.providers.passphrase(s)
      }
      (UnlockProvider::Passphrase, None) => {
        let _ = out.send(fail(UnlockFailReason::WrongSecret));
        return;
      }
      (UnlockProvider::Fido2, pin) => {
        if self.unlock.fido_busy {
          match pin {
            Some(p) => self.unlock.fido_queued = Some(p.into_inner()),
            None => tracing::debug!("security key request already waiting; ignored"),
          }
          return;
        }
        self.unlock.fido_busy = true;
        ctx.providers.fido2(pin.map(UnlockSecret::into_inner))
      }
      (UnlockProvider::KWallet, _) => {
        if self.unlock.kwallet_busy {
          return;
        }
        self.unlock.kwallet_busy = true;
        ctx.providers.secret_service()
      }
    };
    tracing::info!(?provider, "unlock requested from the picker");
    let tx = self.unlock_tx.clone();
    tokio::spawn(async move {
      let res = match unlock::attempt(&ctx.dir, key_provider.as_ref(), &ctx.gate).await {
        Attempt::Opened(store) => {
          if ctx.key_tx.send(KeyEvent::Opened(store)).await.is_ok() {
            UnlockRes::Opened
          } else {
            UnlockRes::Failed(UnlockFailReason::Other)
          }
        }
        Attempt::AlreadyOpen => UnlockRes::Opened,
        Attempt::NeedsPin => UnlockRes::NeedsPin,
        Attempt::Failed(r) => UnlockRes::Failed(r),
      };
      // The provider (and the secret inside it) is zeroized here.
      drop(key_provider);
      let _ = tx.send((provider, res));
    });
  }

  fn on_unlock_result(&mut self, (provider, res): UnlockResult, out: Option<&Out>) {
    match provider {
      UnlockProvider::Passphrase => self.unlock.pass_busy = false,
      UnlockProvider::Fido2 => self.unlock.fido_busy = false,
      UnlockProvider::KWallet => self.unlock.kwallet_busy = false,
    }
    tracing::debug!(?provider, ?res, "unlock attempt finished");
    match res {
      UnlockRes::Opened => {
        if self.lock == LockView::Unlocked {
          if let Some(out) = out {
            let _ = out.send(PickerEvt::Unlocked);
          }
        } else {
          self.unlock.awaiting = Some(provider);
        }
      }
      UnlockRes::NeedsPin => {
        self.unlock.need_pin = true;
        if let Some(out) = out {
          // Always: the picker leaves its "waiting" state on Locked.
          self.send_prompts(out, true);
        }
      }
      UnlockRes::Failed(reason) => {
        if let Some(out) = out {
          let _ = out.send(PickerEvt::UnlockFailed { provider, reason });
        }
      }
    }
    if provider == UnlockProvider::Fido2
      && self.lock != LockView::Unlocked
      && let Some(pin) = self.unlock.fido_queued.take()
      && let Some(out) = out
    {
      let pin = UnlockSecret::new(pin);
      self.on_unlock(UnlockProvider::Fido2, Some(pin), out);
    }
  }
}

fn err(seq: Option<u32>, code: PickerErrorCode, message: &str) -> PickerEvt {
  PickerEvt::Error { seq, code, message: message.to_owned() }
}

/// Read one `PickerReq` into a zeroize-on-drop buffer (it may hold a
/// passphrase), at most [`MAX_REQ_FRAME`] bytes.
async fn read_req(rd: &mut OwnedReadHalf) -> Result<PickerReq, FrameError> {
  let mut prefix = [0u8; 4];
  let mut got = 0;
  while got < 4 {
    match rd.read(&mut prefix[got..]).await? {
      0 if got == 0 => return Err(FrameError::Eof),
      0 => return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into()),
      n => got += n,
    }
  }
  let len = u32::from_be_bytes(prefix) as usize;
  if len > MAX_REQ_FRAME {
    return Err(FrameError::TooLarge(len));
  }
  let mut payload = Zeroizing::new(vec![0u8; len]);
  rd.read_exact(&mut payload).await?;
  decode_payload(&payload)
}

/// Kill (if needed) and reap the picker, logging how it ended.
async fn reap(child: &mut tokio::process::Child) {
  let status = match child.try_wait() {
    Ok(Some(st)) => Some(st),
    _ => match tokio::time::timeout(Duration::from_millis(200), child.wait()).await {
      Ok(Ok(st)) => Some(st),
      _ => {
        let _ = child.start_kill();
        tokio::time::timeout(REAP_WAIT, child.wait()).await.ok().and_then(Result::ok)
      }
    },
  };
  match status {
    Some(st) if st.success() => tracing::info!("picker exited"),
    Some(st) => tracing::warn!("picker ended: {st}"),
    None => tracing::warn!("picker could not be reaped"),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn env_is_allowlisted() {
    let vars = [
      ("WAYLAND_DISPLAY", "wayland-1"),
      ("XDG_RUNTIME_DIR", "/run/user/1"),
      ("XDG_CONFIG_HOME", "/h/.config"),
      ("HOME", "/h"),
      ("LANG", "de_DE.UTF-8"),
      ("LC_TIME", "C"),
      ("PATH", "/bin"),
      ("LD_PRELOAD", "/evil.so"),
      ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/x"),
      ("SPOOL_STATE_DIR", "/s"),
      ("SPOOL_PICKER_RENDERER", "gpu"),
      ("SPOOL_PICKER_FD", "7"),
    ]
    .map(|(k, v)| (OsString::from(k), OsString::from(v)));
    let env = picker_env(vars.into_iter(), PickerRenderer::Software, false);
    let get = |k: &str| {
      let v: Vec<_> = env.iter().filter(|(n, _)| n == k).map(|(_, v)| v.clone()).collect();
      v
    };
    for k in ["WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "HOME", "LANG", "LC_TIME"] {
      assert_eq!(get(k).len(), 1, "{k}");
    }
    for k in ["PATH", "LD_PRELOAD", "DBUS_SESSION_BUS_ADDRESS", "SPOOL_STATE_DIR"] {
      assert!(get(k).is_empty(), "{k} leaked");
    }
    // Ours, never the inherited ones.
    assert_eq!(get("SPOOL_PICKER_FD"), ["3"]);
    assert_eq!(get("SPOOL_PICKER_RENDERER"), ["software"]);
    assert_eq!(get("SPOOL_PICKER_PRERENDER"), ["0"]);
    let env = picker_env(std::iter::empty(), PickerRenderer::Gpu, true);
    assert!(env.contains(&("SPOOL_PICKER_RENDERER".into(), "gpu".into())));
    assert!(env.contains(&("SPOOL_PICKER_PRERENDER".into(), "1".into())));
  }
}
