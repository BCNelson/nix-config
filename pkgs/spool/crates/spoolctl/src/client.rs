//! Blocking client for the public socket.
//!
//! (Also compiled into `spoold`'s IPC integration tests via `#[path]`, so it
//! must only depend on `std`, `anyhow` and `spool_proto`.)

use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use spool_proto::{FrameError, Hello, PublicReq, PublicResp, read_frame, write_frame};

/// Read/write timeout on the socket.
pub const IO_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Client {
  stream: UnixStream,
}

impl Client {
  /// Connect to `spool_proto::paths::public_socket_path()` and perform the
  /// `Hello` exchange (error with both versions on mismatch).
  pub fn connect() -> anyhow::Result<Self> {
    let path = spool_proto::paths::public_socket_path()?;
    Self::connect_to(&path)
  }

  /// [`Client::connect`] to an explicit socket path.
  pub fn connect_to(path: &Path) -> anyhow::Result<Self> {
    let stream = match UnixStream::connect(path) {
      Ok(s) => s,
      Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
        anyhow::bail!("spoold is not running (nothing listening on {})", path.display())
      }
      Err(e) => return Err(e).with_context(|| format!("connecting to {}", path.display())),
    };
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut client = Self { stream };
    client.handshake()?;
    Ok(client)
  }

  fn handshake(&mut self) -> anyhow::Result<()> {
    let ours = Hello::current();
    write_frame(&mut self.stream, &ours).map_err(|e| frame_err(e, "sending hello"))?;
    let theirs: Hello = read_frame(&mut self.stream).map_err(|e| match e {
      FrameError::Eof => {
        anyhow::anyhow!("spoold closed the connection during the handshake (rejected this client?)")
      }
      e => frame_err(e, "reading hello"),
    })?;
    if !theirs.is_compatible() {
      anyhow::bail!(
        "protocol version mismatch: spoolctl speaks v{}, spoold speaks v{}; restart spoold or \
         use a matching spoolctl",
        ours.proto,
        theirs.proto
      );
    }
    Ok(())
  }

  /// [`Client::request`] without a read timeout (`Pick`: the daemon
  /// answers when the user has chosen).
  pub fn request_untimed(&mut self, req: &PublicReq) -> anyhow::Result<PublicResp> {
    self.stream.set_read_timeout(None)?;
    let r = self.request(req);
    self.stream.set_read_timeout(Some(IO_TIMEOUT))?;
    r
  }

  /// Send one request and wait for its response.
  pub fn request(&mut self, req: &PublicReq) -> anyhow::Result<PublicResp> {
    write_frame(&mut self.stream, req).map_err(|e| frame_err(e, "sending request"))?;
    read_frame(&mut self.stream).map_err(|e| match e {
      FrameError::Eof => anyhow::anyhow!("spoold closed the connection without replying"),
      e => frame_err(e, "reading response"),
    })
  }
}

fn frame_err(e: FrameError, what: &str) -> anyhow::Error {
  match e {
    FrameError::TooLarge(n) => {
      anyhow::anyhow!(
        "{what}: message of {n} bytes exceeds the {} byte limit",
        spool_proto::MAX_FRAME
      )
    }
    FrameError::Io(io)
      if io.kind() == io::ErrorKind::WouldBlock || io.kind() == io::ErrorKind::TimedOut =>
    {
      anyhow::anyhow!("{what}: spoold did not respond within {} s", IO_TIMEOUT.as_secs())
    }
    e => anyhow::anyhow!("{what}: {e}"),
  }
}
