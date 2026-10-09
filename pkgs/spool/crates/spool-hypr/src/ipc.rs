//! Hyprland IPC sockets: requests and the focus event stream.

use std::path::{Path, PathBuf};
use std::time::Duration;

use spool_compositor::{CursorProvider, EventSink};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::task::JoinHandle;

use crate::HyprError;
use crate::parse::{self, HyprEvent, HyprWindow};

/// Request socket file name.
pub const REQUEST_SOCKET: &str = ".socket.sock";
/// Event socket file name.
pub const EVENT_SOCKET: &str = ".socket2.sock";
/// Largest reply we read from the request socket.
const MAX_REPLY: u64 = 1 << 20;
/// Longest event line we keep; longer ones (huge titles) are skipped.
const MAX_LINE: usize = 64 << 10;

/// Handle on one Hyprland instance's IPC directory. Cheap to clone.
#[derive(Debug, Clone)]
pub struct HyprIpc {
  dir: PathBuf,
  timeout: Duration,
}

fn valid_signature(sig: &str) -> bool {
  !sig.is_empty() && sig.len() <= 256 && !sig.contains('/') && sig != "." && sig != ".."
}

impl HyprIpc {
  /// The instance named by `HYPRLAND_INSTANCE_SIGNATURE`, under
  /// `$XDG_RUNTIME_DIR/hypr/` (or the pre-0.40 `/tmp/hypr/`).
  pub fn from_env() -> Result<Self, HyprError> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
      .map_err(|_| HyprError::Unsupported("HYPRLAND_INSTANCE_SIGNATURE unset".into()))?;
    if !valid_signature(&sig) {
      return Err(HyprError::Unsupported("malformed HYPRLAND_INSTANCE_SIGNATURE".into()));
    }
    let mut candidates = Vec::new();
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR") {
      candidates.push(PathBuf::from(rt).join("hypr").join(&sig));
    }
    candidates.push(PathBuf::from("/tmp/hypr").join(&sig));
    candidates
      .into_iter()
      .find(|d| d.join(REQUEST_SOCKET).exists())
      .map(Self::from_dir)
      .ok_or_else(|| HyprError::Unsupported("no Hyprland socket directory".into()))
  }

  /// An explicit socket directory (tests, nested sessions).
  pub fn from_dir(dir: impl Into<PathBuf>) -> Self {
    Self { dir: dir.into(), timeout: Duration::from_secs(1) }
  }

  pub fn dir(&self) -> &Path {
    &self.dir
  }

  /// Sends one request (`j/cursorpos`, ...) and returns the whole reply.
  pub async fn request(&self, cmd: &str) -> Result<String, HyprError> {
    let fut = async {
      let mut s = UnixStream::connect(self.dir.join(REQUEST_SOCKET)).await?;
      s.write_all(cmd.as_bytes()).await?;
      let mut buf = Vec::new();
      (&mut s).take(MAX_REPLY).read_to_end(&mut buf).await?;
      String::from_utf8(buf).map_err(|_| HyprError::Protocol("reply is not UTF-8".into()))
    };
    tokio::time::timeout(self.timeout, fut).await.map_err(|_| HyprError::Timeout)?
  }

  /// Cursor position in global layout coordinates.
  pub async fn cursor_pos(&self) -> Result<(i32, i32), HyprError> {
    parse::parse_cursorpos(&self.request("j/cursorpos").await?)
  }

  /// The focused client, `None` if nothing is focused.
  pub async fn active_window(&self) -> Result<Option<HyprWindow>, HyprError> {
    parse::parse_active_window(&self.request("j/activewindow").await?)
  }
}

#[async_trait::async_trait]
impl CursorProvider for HyprIpc {
  async fn cursor(&self) -> Option<(i32, i32)> {
    match self.cursor_pos().await {
      Ok(c) => Some(c),
      Err(e) => {
        tracing::debug!(error = %e, "hyprland cursorpos failed");
        None
      }
    }
  }
}

/// Reads one `\n`-terminated line into `buf` (without the newline).
/// `Ok(None)` on EOF, `Ok(Some(false))` if the line exceeded [`MAX_LINE`]
/// (it was consumed and dropped).
async fn read_line_capped(
  r: &mut BufReader<UnixStream>,
  buf: &mut Vec<u8>,
) -> std::io::Result<Option<bool>> {
  buf.clear();
  let mut overlong = false;
  loop {
    let chunk = r.fill_buf().await?;
    if chunk.is_empty() {
      return Ok(None);
    }
    let (take, done) = match chunk.iter().position(|&b| b == b'\n') {
      Some(i) => (i, true),
      None => (chunk.len(), false),
    };
    if !overlong {
      if buf.len() + take > MAX_LINE {
        overlong = true;
        buf.clear();
      } else {
        buf.extend_from_slice(&chunk[..take]);
      }
    }
    r.consume(if done { take + 1 } else { take });
    if done {
      return Ok(Some(!overlong));
    }
  }
}

/// Follows focus changes on the event socket and feeds an [`EventSink`].
/// The task stops when Hyprland closes the socket or the handle is dropped.
pub struct HyprFocus {
  task: JoinHandle<()>,
}

impl HyprFocus {
  /// Connects to the event socket, reports the currently focused window,
  /// then follows `activewindow`/`activewindowv2`.
  pub async fn spawn(ipc: HyprIpc, sink: EventSink) -> Result<Self, HyprError> {
    let stream = UnixStream::connect(ipc.dir.join(EVENT_SOCKET)).await?;
    // Seed after subscribing so a change in between is not lost.
    let mut last: Option<(Option<String>, Option<String>)> = None;
    match ipc.active_window().await {
      Ok(w) => {
        emit(&sink, &mut last, w.as_ref().and_then(|w| w.class.clone()), w.map(|w| w.address))
      }
      Err(e) => tracing::debug!(error = %e, "initial hyprland activewindow failed"),
    }
    let task = tokio::spawn(follow(ipc, sink, BufReader::new(stream), last));
    Ok(Self { task })
  }

  /// Whether the event task is still running.
  pub fn is_running(&self) -> bool {
    !self.task.is_finished()
  }
}

impl Drop for HyprFocus {
  fn drop(&mut self) {
    self.task.abort();
  }
}

fn emit(
  sink: &EventSink,
  last: &mut Option<(Option<String>, Option<String>)>,
  class: Option<String>,
  address: Option<String>,
) {
  let cur = (class, address);
  if last.as_ref() == Some(&cur) {
    return;
  }
  sink.active_window(cur.0.as_deref(), cur.1.as_deref());
  *last = Some(cur);
}

async fn follow(
  ipc: HyprIpc,
  sink: EventSink,
  mut r: BufReader<UnixStream>,
  mut last: Option<(Option<String>, Option<String>)>,
) {
  let mut buf = Vec::with_capacity(256);
  // `activewindow` (class) immediately precedes `activewindowv2` (address).
  let mut pending_class: Option<Option<String>> = None;
  loop {
    if sink.is_closed() {
      return;
    }
    match read_line_capped(&mut r, &mut buf).await {
      Ok(None) => {
        tracing::warn!("hyprland event socket closed; active-window tracking stopped");
        return;
      }
      Err(e) => {
        tracing::warn!(error = %e, "hyprland event socket failed; active-window tracking stopped");
        return;
      }
      Ok(Some(false)) => {
        pending_class = None;
        continue;
      }
      Ok(Some(true)) => {}
    }
    let Ok(line) = std::str::from_utf8(&buf) else { continue };
    match parse::parse_event(line) {
      Some(HyprEvent::ActiveWindow { class }) => pending_class = Some(class),
      Some(HyprEvent::ActiveWindowV2 { address: None }) => {
        pending_class = None;
        emit(&sink, &mut last, None, None);
      }
      Some(HyprEvent::ActiveWindowV2 { address: Some(addr) }) => {
        let class = match pending_class.take() {
          Some(c) => c,
          // No preceding `activewindow` (overlong title line, odd
          // ordering): ask, and only trust the answer for this address.
          None => match ipc.active_window().await {
            Ok(Some(w)) if w.address == addr => w.class,
            _ => None,
          },
        };
        emit(&sink, &mut last, class, Some(addr));
      }
      Some(HyprEvent::CloseWindow { .. }) | None => {}
    }
  }
}
