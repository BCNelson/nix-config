//! Public socket server.
//!
//! Per connection: `SO_PEERCRED` -> `security::check_peer` (reject other
//! uids and Flatpak peers by closing without a reply) -> `Hello` exchange
//! (`spool_proto::Hello`; close after replying on version mismatch) -> loop
//! { read `PublicReq`, rate-limit `Show`/`Pick` (reply
//! `Error{RateLimited}`), forward as `orchestrator::Request::Public`, write
//! the `PublicResp` }. Idle connections are closed after
//! [`IDLE_TIMEOUT`]. Malformed frames close the connection (an oversized
//! frame gets a best-effort `Error{TooLarge}` first).

use std::fs::File;
use std::io;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use spool_proto::{
  ErrorCode, FrameError, Hello, PublicReq, PublicResp, read_frame_async, write_frame_async,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};

use crate::orchestrator::Request;
use crate::security::{self, PeerCred};

/// Close a connection after this long without a complete request.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Show/Pick: at most this many per [`RATE_WINDOW`] (across connections).
pub const RATE_BURST: u32 = 5;
pub const RATE_WINDOW: Duration = Duration::from_secs(2);

/// A bound public socket plus the single-instance guard.
#[derive(Debug)]
pub struct BoundSocket {
  pub listener: UnixListener,
  pub guard: InstanceGuard,
}

/// Holds the `flock` on `<socket>.lock` for the daemon's lifetime. On drop
/// it removes the socket file (safe: we hold the lock, so it is ours) and
/// then releases the lock.
#[derive(Debug)]
pub struct InstanceGuard {
  pub path: PathBuf,
  _lock: File,
}

impl Drop for InstanceGuard {
  fn drop(&mut self) {
    if let Err(e) = std::fs::remove_file(&self.path)
      && e.kind() != io::ErrorKind::NotFound
    {
      tracing::warn!(path = %self.path.display(), "removing socket: {e}");
    }
  }
}

/// Lock file next to the socket: `<socket>.lock`.
fn lock_path(path: &Path) -> PathBuf {
  let mut s = path.as_os_str().to_owned();
  s.push(".lock");
  PathBuf::from(s)
}

/// Create the socket's parent directory with mode 0700 (refusing an existing
/// one with the wrong owner/mode), take the single-instance lock, remove a
/// stale socket file only if nothing is listening on it (refuse to start if
/// another daemon is), and bind. Must be called inside a tokio runtime.
pub fn bind(path: &Path) -> anyhow::Result<BoundSocket> {
  let parent = path
    .parent()
    .filter(|p| !p.as_os_str().is_empty())
    .ok_or_else(|| anyhow::anyhow!("socket path {} has no parent directory", path.display()))?;
  crate::paths::ensure_strict_private_dir(parent)?;

  let lock_file = lock_path(path);
  let lock = std::fs::OpenOptions::new()
    .read(true)
    .write(true)
    .create(true)
    .truncate(false)
    .mode(0o600)
    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
    .open(&lock_file)
    .with_context(|| format!("opening lock file {}", lock_file.display()))?;
  match rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
    Ok(()) => {}
    Err(rustix::io::Errno::WOULDBLOCK) => {
      anyhow::bail!("another spoold is already running (lock {} is held)", lock_file.display())
    }
    Err(e) => return Err(io::Error::from(e)).context("flock on the lock file"),
  }

  match std::fs::symlink_metadata(path) {
    Ok(md) => {
      if !md.file_type().is_socket() {
        anyhow::bail!("{} exists and is not a socket; refusing to remove it", path.display());
      }
      match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => anyhow::bail!("another spoold is already answering on {}", path.display()),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
          tracing::info!(path = %path.display(), "removing stale socket");
          std::fs::remove_file(path)
            .with_context(|| format!("removing stale socket {}", path.display()))?;
        }
        Err(e) => {
          return Err(e).with_context(|| format!("probing existing socket {}", path.display()));
        }
      }
    }
    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
    Err(e) => return Err(e).with_context(|| format!("stat {}", path.display())),
  }

  let listener = UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
  std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    .with_context(|| format!("chmod 0600 {}", path.display()))?;
  tracing::info!(path = %path.display(), "listening");
  Ok(BoundSocket { listener, guard: InstanceGuard { path: path.to_owned(), _lock: lock } })
}

/// The `Show`/`Pick` limiter shared by the socket and the hotkey path.
pub fn show_limiter() -> Arc<Mutex<RateLimiter>> {
  Arc::new(Mutex::new(RateLimiter::new(RATE_BURST, RATE_WINDOW)))
}

/// Accept loop; spawns one task per connection. Returns only on a fatal
/// listener error. `limiter` ([`show_limiter`]) is shared with the
/// orchestrator's hotkey `Show`.
pub async fn serve(
  listener: UnixListener,
  requests: mpsc::Sender<Request>,
  limiter: Arc<Mutex<RateLimiter>>,
) -> anyhow::Result<()> {
  loop {
    let (stream, _) = match listener.accept().await {
      Ok(s) => s,
      Err(e) if is_transient_accept_error(&e) => {
        tracing::warn!("accept: {e}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        continue;
      }
      Err(e) => return Err(e).context("accept on public socket"),
    };
    let requests = requests.clone();
    let limiter = limiter.clone();
    tokio::spawn(async move {
      handle_connection(stream, requests, limiter).await;
    });
  }
}

fn is_transient_accept_error(e: &io::Error) -> bool {
  matches!(
    e.raw_os_error(),
    Some(
      libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM | libc::ECONNABORTED | libc::EINTR
    )
  )
}

async fn handle_connection(
  mut stream: UnixStream,
  requests: mpsc::Sender<Request>,
  limiter: Arc<Mutex<RateLimiter>>,
) {
  let peer = match security::peer_cred(&stream) {
    Ok(p) => p,
    Err(e) => {
      tracing::warn!("SO_PEERCRED failed: {e}; closing");
      return;
    }
  };
  if let Err(why) = security::check_peer(&peer) {
    tracing::warn!(pid = ?peer.pid, uid = peer.uid, "rejected public client: {why}");
    return;
  }
  match connection_loop(&mut stream, peer, &requests, &limiter).await {
    Ok(()) => tracing::debug!(pid = ?peer.pid, "client disconnected"),
    Err(e) => tracing::debug!(pid = ?peer.pid, "client connection closed: {e}"),
  }
}

#[derive(Debug, thiserror::Error)]
enum ConnError {
  #[error("idle timeout")]
  Idle,
  #[error("frame: {0}")]
  Frame(#[from] FrameError),
  #[error("protocol version mismatch (client v{0})")]
  Version(u16),
  #[error("daemon is shutting down")]
  Shutdown,
  #[error("client went away while picking")]
  Gone,
}

async fn read_with_timeout<T: serde::de::DeserializeOwned>(
  stream: &mut UnixStream,
) -> Result<T, ConnError> {
  // read_frame_async is not cancel-safe, but a timeout closes the
  // connection, so a half-read frame never matters.
  match tokio::time::timeout(IDLE_TIMEOUT, read_frame_async(stream)).await {
    Err(_) => Err(ConnError::Idle),
    Ok(r) => Ok(r?),
  }
}

async fn connection_loop(
  stream: &mut UnixStream,
  peer: PeerCred,
  requests: &mpsc::Sender<Request>,
  limiter: &Mutex<RateLimiter>,
) -> Result<(), ConnError> {
  let hello: Hello = read_with_timeout(stream).await?;
  write_frame_async(stream, &Hello::current()).await?;
  if !hello.is_compatible() {
    return Err(ConnError::Version(hello.proto));
  }

  loop {
    let req: PublicReq = match read_with_timeout(stream).await {
      Ok(r) => r,
      Err(ConnError::Frame(FrameError::Eof)) => return Ok(()),
      Err(ConnError::Frame(FrameError::TooLarge(n))) => {
        let _ = write_frame_async(
          stream,
          &error(ErrorCode::TooLarge, format!("frame of {n} bytes exceeds the limit")),
        )
        .await;
        return Err(FrameError::TooLarge(n).into());
      }
      Err(ConnError::Frame(FrameError::Codec(e))) => {
        let _ = write_frame_async(stream, &error(ErrorCode::BadRequest, "malformed request")).await;
        return Err(FrameError::Codec(e).into());
      }
      Err(e) => return Err(e),
    };
    tracing::debug!(pid = ?peer.pid, req = req_name(&req), "request");

    // `Edit` opens the picker and `New` an editor: same budget as Show/Pick.
    let resp =
      if matches!(req, PublicReq::Show | PublicReq::Pick | PublicReq::Edit | PublicReq::New { .. })
        && !limiter.lock().unwrap_or_else(|p| p.into_inner()).allow(Instant::now())
      {
        error(ErrorCode::RateLimited, "too many Show/Pick/Edit requests; slow down")
      } else {
        // `Edit` waits for the user's choice like `Pick`.
        let is_pick = matches!(req, PublicReq::Pick | PublicReq::Edit);
        let (reply, mut rx) = oneshot::channel();
        if requests.send(Request::Public { req, peer, reply }).await.is_err() {
          let _ =
            write_frame_async(stream, &error(ErrorCode::Unavailable, "daemon is shutting down"))
              .await;
          return Err(ConnError::Shutdown);
        }
        if is_pick {
          // The user may take a while; no idle timeout. A client that goes
          // away (or talks out of turn) drops `rx`, so the orchestrator falls
          // back to a normal selection.
          tokio::select! {
            r = &mut rx => r.unwrap_or_else(|_| error(ErrorCode::Internal, "request was dropped")),
            () = client_gone(stream) => return Err(ConnError::Gone),
          }
        } else {
          rx.await.unwrap_or_else(|_| error(ErrorCode::Internal, "request was dropped"))
        }
      };
    write_frame_async(stream, &resp).await?;
  }
}

/// Resolves once the client closed its end (or sent bytes out of turn while
/// waiting for a `Pick`). Readiness alone is not enough: tokio keeps it set
/// until a read would block.
async fn client_gone(stream: &UnixStream) {
  let mut b = [0u8; 1];
  loop {
    if stream.readable().await.is_err() {
      return;
    }
    match stream.try_read(&mut b) {
      Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
      Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
      _ => return,
    }
  }
}

fn error(code: ErrorCode, message: impl Into<String>) -> PublicResp {
  PublicResp::Error { code, message: message.into() }
}

/// Request kind for logs (never the payload).
pub fn req_name(req: &PublicReq) -> &'static str {
  match req {
    PublicReq::Show => "Show",
    PublicReq::Pick => "Pick",
    PublicReq::Copy { .. } => "Copy",
    PublicReq::Current => "Current",
    PublicReq::Pause { .. } => "Pause",
    PublicReq::Resume => "Resume",
    PublicReq::Status => "Status",
    PublicReq::Edit => "Edit",
    PublicReq::New { .. } => "New",
  }
}

/// Token bucket for `Show`/`Pick`: holds up to `burst` tokens, refilled
/// continuously at `burst` per `window`.
#[derive(Debug)]
pub struct RateLimiter {
  burst: u32,
  window: Duration,
  tokens: f64,
  last: Option<Instant>,
}

impl RateLimiter {
  pub fn new(burst: u32, window: Duration) -> Self {
    Self { burst, window, tokens: f64::from(burst), last: None }
  }

  /// `true` if the call at `now` is allowed (and records it).
  pub fn allow(&mut self, now: Instant) -> bool {
    if let Some(last) = self.last {
      let elapsed = now.saturating_duration_since(last).as_secs_f64();
      let rate = f64::from(self.burst) / self.window.as_secs_f64().max(f64::EPSILON);
      self.tokens = (self.tokens + elapsed * rate).min(f64::from(self.burst));
    }
    // Never move `last` backwards (Instant can be passed out of order).
    if self.last.is_none_or(|l| now > l) {
      self.last = Some(now);
    }
    if self.tokens >= 1.0 {
      self.tokens -= 1.0;
      true
    } else {
      false
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn rate_limiter_burst_and_refill() {
    let t0 = Instant::now();
    let mut rl = RateLimiter::new(5, Duration::from_secs(2));
    for _ in 0..5 {
      assert!(rl.allow(t0));
    }
    assert!(!rl.allow(t0));
    // 5 per 2 s = one token per 400 ms.
    assert!(!rl.allow(t0 + Duration::from_millis(300)));
    assert!(rl.allow(t0 + Duration::from_millis(450)));
    assert!(!rl.allow(t0 + Duration::from_millis(500)));
    // After a long pause the bucket is full again but capped at the burst.
    let later = t0 + Duration::from_secs(60);
    for _ in 0..5 {
      assert!(rl.allow(later));
    }
    assert!(!rl.allow(later));
  }

  #[test]
  fn rate_limiter_out_of_order_instants() {
    let t0 = Instant::now() + Duration::from_secs(10);
    let mut rl = RateLimiter::new(1, Duration::from_secs(1));
    assert!(rl.allow(t0));
    assert!(!rl.allow(t0 - Duration::from_secs(5)));
    assert!(rl.allow(t0 + Duration::from_secs(1)));
  }

  #[test]
  fn lock_path_is_sibling() {
    assert_eq!(
      lock_path(Path::new("/run/user/1/spool/sock")),
      Path::new("/run/user/1/spool/sock.lock")
    );
  }
}
