//! Script host link: `scripts.sock` and the `spool-scripts` connection.
//!
//! The host is a separate process (its own user unit; it needs the network
//! and to start programs, which spoold's sandbox forbids) that connects to
//! `scripts.sock` next to the public socket. Per connection: `SO_PEERCRED`
//! -> `security::check_peer` -> `/proc/<pid>/exe` must be the
//! `spool-scripts` found on the audited PATH ([`ExeCheck`]) -> at most one
//! host at a time -> `Hello::script` exchange -> `ScriptReq::Register`.
//! Then a writer task forwards [`ScriptEvt`]s from the orchestrator and the
//! reader turns `FilterDone`s into [`ScriptEvent`]s for it.
//!
//! The orchestrator side ([`Scripts`]) never waits on the host: it parks an
//! item, sends it, and finishes it on `FilterDone`, on its deadline or when
//! the host goes away ([`ScriptEvent::HostGone`]); see
//! `Orchestrator::filter_or_store`.
//!
//! Never log what the host sends: verdicts carry no text, and items are
//! logged only by hash prefix on the orchestrator side.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use spool_proto::{
  FrameError, Hello, ScriptEvt, ScriptItem, ScriptReq, Verdict, read_frame_async, write_frame_async,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use crate::security::{self, PeerCred};

/// The host binary spoold accepts (looked up on the audited PATH).
pub const HOST_BIN: &str = "spool-scripts";

/// Longest wait for the host's `Hello` and `Register`.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// What the host connection reports to the orchestrator.
#[derive(Debug)]
pub enum ScriptEvent {
  FilterDone {
    seq: u64,
    verdict: Verdict,
  },
  /// The host disconnected: everything waiting on it is finished
  /// unfiltered.
  HostGone,
}

/// A connected host.
#[derive(Debug, Clone)]
struct HostConn {
  id: u64,
  tx: mpsc::UnboundedSender<ScriptEvt>,
  filters: bool,
}

#[derive(Debug, Default)]
struct Shared {
  host: Option<HostConn>,
  next_id: u64,
}

/// Orchestrator side of the link.
pub struct Scripts {
  shared: Option<Arc<Mutex<Shared>>>,
  events: Option<mpsc::UnboundedReceiver<ScriptEvent>>,
  budget: Duration,
}

impl Scripts {
  /// No script support (`[scripts] enabled = false`, or no host binary).
  pub fn disabled() -> Self {
    Self { shared: None, events: None, budget: Duration::ZERO }
  }

  /// A link plus the server half that [`serve`] runs.
  pub fn new(budget: Duration, check: ExeCheck) -> (Self, ScriptServer) {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    (
      Self { shared: Some(shared.clone()), events: Some(events_rx), budget },
      ScriptServer { shared, events_tx, check },
    )
  }

  /// The event stream (once; `None` when disabled).
  pub fn take_events(&mut self) -> Option<mpsc::UnboundedReceiver<ScriptEvent>> {
    self.events.take()
  }

  pub fn budget(&self) -> Duration {
    self.budget
  }

  /// Send `item` to a connected host that has filters. `false` = nobody to
  /// ask (store it now).
  pub fn send_filter(&self, seq: u64, item: ScriptItem) -> bool {
    let Some(shared) = &self.shared else { return false };
    let host = shared.lock().expect("scripts lock").host.clone();
    let Some(host) = host.filter(|h| h.filters) else { return false };
    let budget_ms = u32::try_from(self.budget.as_millis()).unwrap_or(u32::MAX);
    host.tx.send(ScriptEvt::Filter { seq, budget_ms, item }).is_ok()
  }
}

/// How the peer's executable is verified.
#[derive(Debug, Clone)]
pub enum ExeCheck {
  /// `/proc/<pid>/exe` (canonical) must equal this path.
  Exact(PathBuf),
}

impl ExeCheck {
  fn verify(&self, pid: i32) -> Result<(), String> {
    match self {
      ExeCheck::Exact(want) => {
        let got = std::fs::read_link(format!("/proc/{pid}/exe"))
          .map_err(|e| format!("cannot read the executable of pid {pid}: {e}"))?;
        let got = std::fs::canonicalize(&got).unwrap_or(got);
        if &got == want {
          Ok(())
        } else {
          Err(format!("pid {pid} runs {}, not {}", got.display(), want.display()))
        }
      }
    }
  }
}

/// Accept side, run by [`serve`].
pub struct ScriptServer {
  shared: Arc<Mutex<Shared>>,
  events_tx: mpsc::UnboundedSender<ScriptEvent>,
  check: ExeCheck,
}

/// Everything [`serve`] and the orchestrator need, for `[scripts] enabled`.
/// `Ok(None)` (logged) when `spool-scripts` is not on the audited PATH.
pub fn setup(
  public_socket: &Path,
  budget: Duration,
) -> anyhow::Result<Option<(Scripts, UnixListener, ScriptServer)>> {
  let Some(exe) = security::find_on_audited_path(HOST_BIN) else {
    tracing::warn!(
      "[scripts] enabled, but {HOST_BIN} is not on the audited PATH; scripts disabled"
    );
    return Ok(None);
  };
  let listener = bind(&spool_proto::paths::script_socket_for(public_socket))?;
  let (link, server) = Scripts::new(budget, ExeCheck::Exact(exe));
  Ok(Some((link, listener, server)))
}

/// Bind `path` (0600). The directory is the public socket's, already
/// checked 0700 by `ipc::bind`, and the public socket's lock guards against
/// a second daemon, so a leftover file is ours and is replaced.
pub fn bind(path: &Path) -> anyhow::Result<UnixListener> {
  use std::os::unix::fs::{FileTypeExt, PermissionsExt};
  match std::fs::symlink_metadata(path) {
    Ok(md) if md.file_type().is_socket() => {
      std::fs::remove_file(path).with_context(|| format!("removing stale {}", path.display()))?;
    }
    Ok(_) => anyhow::bail!("{} exists and is not a socket; refusing to remove it", path.display()),
    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
    Err(e) => return Err(e).with_context(|| format!("stat {}", path.display())),
  }
  let listener = UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
  std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    .with_context(|| format!("chmod 0600 {}", path.display()))?;
  tracing::info!(path = %path.display(), "listening for the script host");
  Ok(listener)
}

/// Accept loop. Returns only on a fatal listener error.
pub async fn serve(listener: UnixListener, server: ScriptServer) -> anyhow::Result<()> {
  let server = Arc::new(server);
  loop {
    let (stream, _) = match listener.accept().await {
      Ok(s) => s,
      Err(e) if crate::ipc::is_transient_accept_error(&e) => {
        tracing::warn!("script socket accept: {e}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        continue;
      }
      Err(e) => return Err(e).context("accept on the script socket"),
    };
    let server = server.clone();
    tokio::spawn(async move { server.handle(stream).await });
  }
}

#[derive(Debug, thiserror::Error)]
enum ConnError {
  #[error("frame: {0}")]
  Frame(#[from] FrameError),
  #[error("protocol version mismatch (host v{0})")]
  Version(u16),
  #[error("handshake timed out")]
  Timeout,
  #[error("expected Register first")]
  NoRegister,
}

impl ScriptServer {
  /// Whether a host is registered (tests wait on it after a handshake).
  #[cfg(test)]
  pub fn connected_probe(&self) -> impl Fn() -> bool + Send + 'static {
    let shared = self.shared.clone();
    move || shared.lock().expect("scripts lock").host.is_some()
  }

  async fn handle(&self, mut stream: UnixStream) {
    let peer = match security::peer_cred(&stream) {
      Ok(p) => p,
      Err(e) => return tracing::warn!("script socket: SO_PEERCRED failed: {e}"),
    };
    if let Err(why) = self.admit(&peer) {
      return tracing::warn!(pid = ?peer.pid, "rejected script host: {why}");
    }
    let filters = match handshake(&mut stream).await {
      Ok(f) => f,
      Err(e) => return tracing::warn!(pid = ?peer.pid, "script host handshake: {e}"),
    };
    let (tx, mut rx) = mpsc::unbounded_channel::<ScriptEvt>();
    let id = {
      let mut s = self.shared.lock().expect("scripts lock");
      if s.host.is_some() {
        drop(s);
        return tracing::warn!(pid = ?peer.pid, "rejected script host: one is already connected");
      }
      s.next_id += 1;
      let id = s.next_id;
      s.host = Some(HostConn { id, tx, filters });
      id
    };
    tracing::info!(pid = ?peer.pid, filters, "script host connected");

    let (mut rd, mut wr) = stream.into_split();
    let writer = tokio::spawn(async move {
      while let Some(evt) = rx.recv().await {
        if let Err(e) = write_frame_async(&mut wr, &evt).await {
          tracing::debug!("script host write: {e}");
          break;
        }
      }
    });
    let why = loop {
      match read_frame_async::<_, ScriptReq>(&mut rd).await {
        Ok(ScriptReq::FilterDone { seq, verdict }) => {
          let _ = self.events_tx.send(ScriptEvent::FilterDone { seq, verdict });
        }
        Ok(ScriptReq::Register { .. }) => break "registered twice".to_string(),
        Err(FrameError::Eof) => break "disconnected".to_string(),
        Err(e) => break e.to_string(),
      }
    };
    writer.abort();
    {
      let mut s = self.shared.lock().expect("scripts lock");
      if s.host.as_ref().is_some_and(|h| h.id == id) {
        s.host = None;
      }
    }
    let _ = self.events_tx.send(ScriptEvent::HostGone);
    tracing::info!(pid = ?peer.pid, "script host gone: {why}");
  }

  fn admit(&self, peer: &PeerCred) -> Result<(), String> {
    security::check_peer(peer).map_err(|e| e.to_string())?;
    let pid = peer.pid.ok_or("no pid")?;
    self.check.verify(pid)
  }
}

/// `Hello` both ways, then `Register`. Returns whether the host has filters.
async fn handshake(stream: &mut UnixStream) -> Result<bool, ConnError> {
  tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
    let hello: Hello = read_frame_async(stream).await?;
    write_frame_async(stream, &Hello::script()).await?;
    if !hello.is_script_compatible() {
      return Err(ConnError::Version(hello.proto));
    }
    match read_frame_async(stream).await? {
      ScriptReq::Register { filters } => Ok(filters),
      ScriptReq::FilterDone { .. } => Err(ConnError::NoRegister),
    }
  })
  .await
  .map_err(|_| ConnError::Timeout)?
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn exe_check_accepts_only_the_expected_binary() {
    let me = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
    let pid = std::process::id() as i32;
    assert!(ExeCheck::Exact(me).verify(pid).is_ok());
    let err = ExeCheck::Exact(PathBuf::from("/nix/store/x-spool/bin/spool-scripts"))
      .verify(pid)
      .unwrap_err();
    assert!(err.contains("not /nix/store/x-spool/bin/spool-scripts"), "{err}");
    assert!(ExeCheck::Exact(PathBuf::from("/x")).verify(i32::MAX).is_err());
  }

  #[test]
  fn disabled_never_sends() {
    let s = Scripts::disabled();
    assert!(!s.send_filter(1, item()));
  }

  pub(crate) fn item() -> ScriptItem {
    ScriptItem {
      id: None,
      selection: spool_proto::WireSelection::Clipboard,
      source_app: None,
      created_at_ms: 0,
      tags: vec![],
      reps: vec![],
      aliases: vec![],
    }
  }
}
