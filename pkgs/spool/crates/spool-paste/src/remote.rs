//! Auto-paste through the `spool-paster` helper process.
//!
//! KWin grants `org_kde_kwin_fake_input` by reading the client's
//! `/proc/<pid>/exe` and matching it against desktop files. spoold is
//! non-dumpable (`PR_SET_DUMPABLE=0`: it holds the history data key), and
//! an unprivileged KWin cannot read a non-dumpable process's
//! `/proc/<pid>/exe`, so spoold itself is never granted fake input. The
//! Wayland paste connection therefore lives in `spool-paster`, a tiny,
//! dumpable helper that holds no secrets: it only receives
//! `{chord, layout}` requests from its parent over a socketpair (its stdin)
//! and presses keys. The desktop file `dev.bcnelson.spool.paster.desktop`
//! (`X-KDE-Wayland-Interfaces=org_kde_kwin_fake_input`) names the helper.
//!
//! Note: like any desktop-file grant this is per executable, so any process
//! of the user can run `spool-paster` and inject the paste chords; it cannot
//! read anything through it.
//!
//! # Wire (stream socket, big endian)
//!
//! - helper -> parent, once: `b"SPL1"`, then `0, backend` (0 = fake input,
//!   1 = virtual keyboard) or an error (`kind, u16 len, message`).
//! - parent -> helper: `chord (0 = Ctrl+V, 1 = Ctrl+Shift+V, 2 =
//!   Shift+Insert), has_layout (0/1), layout u32`.
//! - helper -> parent: `0` (ok) or an error (`kind, u16 len, message`).
//!
//! Error kinds: 1 Unsupported, 2 Connect, 3 Wayland, 4 ThreadGone. EOF on
//! the socket ends the helper.

use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

use crate::PasteError;
use crate::chord::PasteChord;
use crate::paster::{PasteBackend, PasteConfig, Paster};

/// Helper binary name.
pub const PASTER_BIN: &str = "spool-paster";
const MAGIC: &[u8; 4] = b"SPL1";
/// Longest wait for the helper's hello (it connects to Wayland first).
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest wait for one paste (a chord is a handful of 8 ms key events).
const PASTE_TIMEOUT: Duration = Duration::from_secs(3);
/// Longest error message accepted from the helper.
const MAX_MSG: usize = 1024;

fn chord_byte(c: PasteChord) -> u8 {
  match c {
    PasteChord::CtrlV => 0,
    PasteChord::CtrlShiftV => 1,
    PasteChord::ShiftInsert => 2,
  }
}

fn chord_from(b: u8) -> Option<PasteChord> {
  match b {
    0 => Some(PasteChord::CtrlV),
    1 => Some(PasteChord::CtrlShiftV),
    2 => Some(PasteChord::ShiftInsert),
    _ => None,
  }
}

fn encode_err(e: &PasteError) -> Vec<u8> {
  let (kind, msg) = match e {
    PasteError::Unsupported(m) => (1u8, m.as_str()),
    PasteError::Connect(m) => (2, m.as_str()),
    PasteError::Wayland(m) => (3, m.as_str()),
    PasteError::ThreadGone => (4, ""),
  };
  let msg = &msg.as_bytes()[..msg.len().min(MAX_MSG)];
  let mut v = vec![kind];
  v.extend_from_slice(&(msg.len() as u16).to_be_bytes());
  v.extend_from_slice(msg);
  v
}

fn decode_err(kind: u8, msg: Vec<u8>) -> PasteError {
  let msg = String::from_utf8_lossy(&msg).into_owned();
  match kind {
    1 => PasteError::Unsupported(msg),
    2 => PasteError::Connect(msg),
    3 => PasteError::Wayland(msg),
    _ => PasteError::ThreadGone,
  }
}

/// The helper's main loop (`spool-paster`): connects, says hello, serves
/// requests on `sock` until EOF. Returns the process exit code.
pub fn serve_helper(mut sock: std::os::unix::net::UnixStream, cfg: &PasteConfig) -> i32 {
  let mut paster = match Paster::connect(cfg) {
    Ok(p) => {
      let backend = match p.backend() {
        PasteBackend::FakeInput => 0,
        PasteBackend::VirtualKeyboard => 1,
      };
      if sock.write_all(&[MAGIC[0], MAGIC[1], MAGIC[2], MAGIC[3], 0, backend]).is_err() {
        return 1;
      }
      Some(p)
    }
    Err(e) => {
      let mut v = MAGIC.to_vec();
      v.extend(encode_err(&e));
      let _ = sock.write_all(&v);
      return 2;
    }
  };
  let mut req = [0u8; 6];
  loop {
    if sock.read_exact(&mut req).is_err() {
      return 0; // parent gone
    }
    let Some(chord) = chord_from(req[0]) else { return 1 };
    let layout = (req[1] == 1).then(|| u32::from_be_bytes([req[2], req[3], req[4], req[5]]));
    let mut res = match paster.as_mut() {
      Some(p) => p.paste(chord, layout),
      None => Err(PasteError::Wayland("not connected".into())),
    };
    if matches!(res, Err(PasteError::Wayland(_))) {
      // Compositor restart: reconnect once.
      drop(paster.take());
      paster = Paster::connect(cfg).ok();
      if let Some(p) = paster.as_mut() {
        res = p.paste(chord, layout);
      }
    }
    let out = match res {
      Ok(()) => vec![0],
      Err(e) => encode_err(&e),
    };
    if sock.write_all(&out).is_err() {
      return 0;
    }
  }
}

struct Conn {
  _child: tokio::process::Child,
  sock: UnixStream,
}

/// Parent side: owns the `spool-paster` child (restarted once on failure).
pub struct RemotePaster {
  exe: PathBuf,
  cfg: PasteConfig,
  backend: PasteBackend,
  conn: Mutex<Option<Conn>>,
}

impl std::fmt::Debug for RemotePaster {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("RemotePaster").field("exe", &self.exe).field("backend", &self.backend).finish()
  }
}

async fn read_err(sock: &mut UnixStream, kind: u8) -> PasteError {
  let mut len = [0u8; 2];
  if sock.read_exact(&mut len).await.is_err() {
    return decode_err(kind, Vec::new());
  }
  let n = usize::from(u16::from_be_bytes(len)).min(MAX_MSG);
  let mut msg = vec![0u8; n];
  let _ = sock.read_exact(&mut msg).await;
  decode_err(kind, msg)
}

/// Starts the helper with a scrubbed environment (only the Wayland
/// variables) and its stdin as our socket; waits for its hello.
async fn launch(exe: &Path, cfg: &PasteConfig) -> Result<(Conn, PasteBackend), PasteError> {
  let (ours, theirs) =
    std::os::unix::net::UnixStream::pair().map_err(|e| PasteError::Connect(e.to_string()))?;
  ours.set_nonblocking(true).map_err(|e| PasteError::Connect(e.to_string()))?;
  let mut cmd = tokio::process::Command::new(exe);
  cmd
    .env_clear()
    .stdin(Stdio::from(OwnedFd::from(theirs)))
    .stdout(Stdio::null())
    .stderr(Stdio::inherit())
    .kill_on_drop(true)
    .arg("--spacing-ms")
    .arg(cfg.spacing.as_millis().to_string());
  for var in ["XDG_RUNTIME_DIR", "WAYLAND_DISPLAY"] {
    if let Some(v) = std::env::var_os(var) {
      cmd.env(var, v);
    }
  }
  if let Some(d) = &cfg.display {
    cmd.env("WAYLAND_DISPLAY", d);
  }
  let child =
    cmd.spawn().map_err(|e| PasteError::Connect(format!("starting {}: {e}", exe.display())))?;
  drop(cmd); // closes our copy of the child's end
  let mut sock = UnixStream::from_std(ours).map_err(|e| PasteError::Connect(e.to_string()))?;
  let hello = async {
    let mut h = [0u8; 5];
    sock
      .read_exact(&mut h)
      .await
      .map_err(|e| PasteError::Connect(format!("{PASTER_BIN} exited before saying hello: {e}")))?;
    if &h[..4] != MAGIC {
      return Err(PasteError::Connect(format!("{PASTER_BIN}: bad hello")));
    }
    if h[4] != 0 {
      return Err(read_err(&mut sock, h[4]).await);
    }
    let mut b = [0u8; 1];
    sock.read_exact(&mut b).await.map_err(|e| PasteError::Connect(e.to_string()))?;
    Ok(if b[0] == 1 { PasteBackend::VirtualKeyboard } else { PasteBackend::FakeInput })
  };
  let backend = tokio::time::timeout(HELLO_TIMEOUT, hello)
    .await
    .map_err(|_| PasteError::Connect(format!("{PASTER_BIN} did not answer")))??;
  Ok((Conn { _child: child, sock }, backend))
}

impl RemotePaster {
  /// Starts `exe` (the `spool-paster` binary). Errors (including
  /// [`PasteError::Unsupported`] from the helper) are returned as the
  /// in-process [`crate::PasteHandle::spawn`] would.
  pub async fn spawn(exe: &Path, cfg: PasteConfig) -> Result<Self, PasteError> {
    let (conn, backend) = launch(exe, &cfg).await?;
    Ok(Self { exe: exe.to_owned(), cfg, backend, conn: Mutex::new(Some(conn)) })
  }

  /// Backend the helper chose at start.
  pub fn backend(&self) -> PasteBackend {
    self.backend
  }

  /// `spool-paster` next to `exe_dir`'s executable, else on `PATH`.
  pub fn locate(own_exe: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = own_exe.and_then(Path::parent) {
      let p = dir.join(PASTER_BIN);
      if p.is_file() {
        return Some(p);
      }
    }
    std::env::var_os("PATH").and_then(|p| {
      std::env::split_paths(&p).map(|d| d.join(PASTER_BIN)).find(|p| p.is_absolute() && p.is_file())
    })
  }

  async fn request(conn: &mut Conn, req: &[u8; 6]) -> Result<Result<(), PasteError>, ()> {
    let io = async {
      conn.sock.write_all(req).await.map_err(|_| ())?;
      let mut st = [0u8; 1];
      conn.sock.read_exact(&mut st).await.map_err(|_| ())?;
      Ok(if st[0] == 0 { Ok(()) } else { Err(read_err(&mut conn.sock, st[0]).await) })
    };
    tokio::time::timeout(PASTE_TIMEOUT, io).await.map_err(|_| ())?
  }

  /// Presses `chord` in the focused window. A dead helper is restarted once.
  pub async fn paste(&self, chord: PasteChord, layout: Option<u32>) -> Result<(), PasteError> {
    let l = layout.unwrap_or(0).to_be_bytes();
    let req = [chord_byte(chord), u8::from(layout.is_some()), l[0], l[1], l[2], l[3]];
    let mut guard = self.conn.lock().await;
    if let Some(c) = guard.as_mut()
      && let Ok(r) = Self::request(c, &req).await
    {
      return r;
    }
    // Helper died or hung: restart it once.
    *guard = None;
    let (mut c, _) = launch(&self.exe, &self.cfg).await?;
    let r = Self::request(&mut c, &req).await.map_err(|()| PasteError::ThreadGone)?;
    *guard = Some(c);
    r
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn wire_roundtrips() {
    for c in [PasteChord::CtrlV, PasteChord::CtrlShiftV, PasteChord::ShiftInsert] {
      assert_eq!(chord_from(chord_byte(c)), Some(c));
    }
    assert_eq!(chord_from(9), None);
    for e in [
      PasteError::Unsupported("u".into()),
      PasteError::Connect("c".into()),
      PasteError::Wayland("w".into()),
      PasteError::ThreadGone,
    ] {
      let v = encode_err(&e);
      let n = usize::from(u16::from_be_bytes([v[1], v[2]]));
      assert_eq!(decode_err(v[0], v[3..3 + n].to_vec()), e);
    }
  }

  #[tokio::test]
  async fn missing_helper_is_a_connect_error() {
    let r = RemotePaster::spawn(Path::new("/nonexistent/spool-paster"), PasteConfig::default());
    assert!(matches!(r.await, Err(PasteError::Connect(_))));
  }
}
