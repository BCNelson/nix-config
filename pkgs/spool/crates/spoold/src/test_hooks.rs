//! Test-only debug socket (feature `test-hooks`; never in the package
//! build). When `SPOOL_TEST_HOOK_SOCKET` is set, spoold listens there and
//! answers search queries through the same in-daemon API the picker uses,
//! so the E2E suite can check search across restarts.
//!
//! Wire: one framed request `(query: String, limit: u32)` per connection,
//! answered with `Result<Vec<ItemPreview>, String>`.
//!
//! `SPOOL_TEST_HOOK_CTL_SOCKET` (M5): one framed `mode: u8` per connection
//! (0 = Copy, 1 = Paste, 2 = PastePlain) sends `Request::Select` for the most
//! recent item, exactly as the picker would, and answers with
//! `Result<String, String>` (the `Debug` form of the outcome).

use std::path::PathBuf;

use spool_core::item::ItemId;
use spool_proto::{ItemPreview, QueryFilters, SelectMode, read_frame_async, write_frame_async};
use tokio::net::UnixListener;
use tokio::sync::{mpsc, oneshot};

use crate::orchestrator::Request;

/// Removes the sockets on drop.
pub struct Hook(Option<PathBuf>, Option<PathBuf>);

impl Drop for Hook {
  fn drop(&mut self) {
    for p in [&self.0, &self.1].into_iter().flatten() {
      let _ = std::fs::remove_file(p);
    }
  }
}

pub fn start(requests: mpsc::Sender<Request>) -> anyhow::Result<Hook> {
  let ctl = start_ctl(requests.clone())?;
  let Some(path) = std::env::var_os("SPOOL_TEST_HOOK_SOCKET").map(PathBuf::from) else {
    return Ok(Hook(None, ctl));
  };
  let _ = std::fs::remove_file(&path);
  let listener = UnixListener::bind(&path)?;
  tracing::warn!(socket = %path.display(), "TEST HOOKS ENABLED: debug search socket (tests only)");
  tokio::spawn(async move {
    while let Ok((mut stream, _)) = listener.accept().await {
      let requests = requests.clone();
      tokio::spawn(async move {
        let Ok((q, limit)) = read_frame_async::<_, (String, u32)>(&mut stream).await else {
          return;
        };
        let (reply, rx) = oneshot::channel();
        let req = Request::Search { q, filters: QueryFilters::default(), offset: 0, limit, reply };
        if requests.send(req).await.is_err() {
          return;
        }
        let resp: Result<Vec<ItemPreview>, String> = match rx.await {
          Ok(r) => r.map_err(|e| e.to_string()),
          Err(_) => Err("orchestrator gone".into()),
        };
        let _ = write_frame_async(&mut stream, &resp).await;
      });
    }
  });
  Ok(Hook(Some(path), ctl))
}

fn start_ctl(requests: mpsc::Sender<Request>) -> anyhow::Result<Option<PathBuf>> {
  let Some(path) = std::env::var_os("SPOOL_TEST_HOOK_CTL_SOCKET").map(PathBuf::from) else {
    return Ok(None);
  };
  let _ = std::fs::remove_file(&path);
  let listener = UnixListener::bind(&path)?;
  tracing::warn!(socket = %path.display(), "TEST HOOKS ENABLED: debug select socket (tests only)");
  tokio::spawn(async move {
    while let Ok((mut stream, _)) = listener.accept().await {
      let requests = requests.clone();
      tokio::spawn(async move {
        let Ok(mode) = read_frame_async::<_, u8>(&mut stream).await else { return };
        let resp = select_latest(&requests, mode).await;
        let _ = write_frame_async(&mut stream, &resp).await;
      });
    }
  });
  Ok(Some(path))
}

async fn select_latest(requests: &mpsc::Sender<Request>, mode: u8) -> Result<String, String> {
  let mode = match mode {
    0 => SelectMode::Copy,
    1 => SelectMode::Paste,
    2 => SelectMode::PastePlain,
    m => return Err(format!("bad mode {m}")),
  };
  let (reply, rx) = oneshot::channel();
  let req = Request::Search {
    q: String::new(),
    filters: QueryFilters::default(),
    offset: 0,
    limit: 1,
    reply,
  };
  requests.send(req).await.map_err(|_| "orchestrator gone")?;
  let items = rx.await.map_err(|_| "orchestrator gone")?.map_err(|e| e.to_string())?;
  let id = items.first().ok_or("history is empty")?.id;
  let (reply, rx) = oneshot::channel();
  requests
    .send(Request::Select { id: ItemId(id), mode, reply })
    .await
    .map_err(|_| "orchestrator gone")?;
  Ok(format!("{:?}", rx.await.map_err(|_| "orchestrator gone")?))
}

/// `spoold --probe-fake-input` (test-hooks builds only): as non-dumpable as
/// the real daemon, try to get KWin's fake input in-process. Exit status 0
/// = granted. Shows why auto-paste runs in the `spool-paster` helper.
pub fn probe_fake_input() -> anyhow::Result<()> {
  let cfg = spool_paste::PasteConfig::default();
  match spool_paste::Paster::connect(&cfg) {
    Ok(p) if p.backend() == spool_paste::PasteBackend::FakeInput => {
      eprintln!("probe: fake input granted");
      Ok(())
    }
    Ok(p) => anyhow::bail!("probe: unsupported (got {:?} instead)", p.backend()),
    Err(e) => anyhow::bail!("probe: {e}"),
  }
}
