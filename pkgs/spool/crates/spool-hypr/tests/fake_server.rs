//! spool-hypr against a fake Hyprland: two Unix sockets in a temp dir that
//! answer with recorded-format fixtures. No real Hyprland involved.

use std::path::Path;
use std::time::Duration;

use spool_compositor::{CompositorEvent, CursorProvider, EventSink};
use spool_hypr::{HyprFocus, HyprIpc, HyprWindow};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

const ACTIVE_FOOT: &str = include_str!("fixtures/activewindow.json");
const CURSOR: &str = include_str!("fixtures/cursorpos.json");

/// Request socket: answers `j/cursorpos` / `j/activewindow` from `active`.
fn serve_requests(dir: &Path, active: std::sync::Arc<std::sync::Mutex<String>>) {
  let l = UnixListener::bind(dir.join(".socket.sock")).unwrap();
  tokio::spawn(async move {
    loop {
      let (mut s, _) = l.accept().await.unwrap();
      let active = active.clone();
      tokio::spawn(async move {
        let mut buf = [0u8; 256];
        let n = s.read(&mut buf).await.unwrap();
        let reply = match &buf[..n] {
          b"j/cursorpos" => CURSOR.to_owned(),
          b"j/activewindow" => active.lock().unwrap().clone(),
          _ => "unknown request".to_owned(),
        };
        s.write_all(reply.as_bytes()).await.unwrap();
        // Hyprland closes after one reply.
      });
    }
  });
}

/// Event socket: accepts one subscriber and returns a sender for raw bytes.
fn serve_events(dir: &Path) -> mpsc::UnboundedSender<Vec<u8>> {
  let l = UnixListener::bind(dir.join(".socket2.sock")).unwrap();
  let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
  tokio::spawn(async move {
    let (mut s, _) = l.accept().await.unwrap();
    while let Some(b) = rx.recv().await {
      if b.is_empty() {
        return; // close
      }
      s.write_all(&b).await.unwrap();
    }
  });
  tx
}

async fn next(rx: &mut mpsc::Receiver<CompositorEvent>) -> (Option<String>, Option<String>) {
  match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("timeout").unwrap() {
    CompositorEvent::ActiveWindow { app_id, window_id, .. } => (app_id, window_id),
    other => panic!("unexpected {other:?}"),
  }
}

fn s(v: &str) -> Option<String> {
  Some(v.to_owned())
}

#[tokio::test]
async fn requests_and_cursor_provider() {
  let dir = tempfile::tempdir().unwrap();
  let active = std::sync::Arc::new(std::sync::Mutex::new(ACTIVE_FOOT.to_owned()));
  serve_requests(dir.path(), active.clone());
  let ipc = HyprIpc::from_dir(dir.path());
  assert_eq!(ipc.cursor_pos().await.unwrap(), (1287, 642));
  assert_eq!(ipc.cursor().await, Some((1287, 642)));
  assert_eq!(
    ipc.active_window().await.unwrap(),
    Some(HyprWindow { address: "0x5581f00ba2a0".into(), class: s("foot") })
  );
  *active.lock().unwrap() = "{}".into();
  assert_eq!(ipc.active_window().await.unwrap(), None);
  // Missing socket -> error, and the provider degrades to None.
  let gone = HyprIpc::from_dir(dir.path().join("nope"));
  assert!(gone.cursor_pos().await.is_err());
  assert_eq!(gone.cursor().await, None);
}

#[tokio::test]
async fn focus_stream_feeds_sink() {
  let dir = tempfile::tempdir().unwrap();
  let active = std::sync::Arc::new(std::sync::Mutex::new(ACTIVE_FOOT.to_owned()));
  serve_requests(dir.path(), active.clone());
  let events = serve_events(dir.path());
  let (sink, mut rx) = EventSink::new();
  let tracker = sink.tracker();
  let focus = HyprFocus::spawn(HyprIpc::from_dir(dir.path()), sink.clone()).await.unwrap();

  // Seeded from j/activewindow.
  assert_eq!(next(&mut rx).await, (s("foot"), s("0x5581f00ba2a0")));
  assert!(tracker.is_active("0x5581f00ba2a0"));

  // Recorded stream: duplicate focus is not re-reported, titles with commas
  // are fine, unrelated events are ignored.
  events.send(include_bytes!("fixtures/socket2.txt").to_vec()).unwrap();
  assert_eq!(next(&mut rx).await, (s("org.kde.kate"), s("0x5581f00c1e40")));
  assert_eq!(next(&mut rx).await, (None, None));
  assert_eq!(tracker.current_app_id(), None);

  // Split writes and an overlong title line; v2 without a preceding
  // activewindow falls back to j/activewindow (address must match).
  events.send(b"activewindow>>kitty,".to_vec()).unwrap();
  events.send(format!("{}\nactivewindowv2>>55", "t".repeat(70 << 10)).into_bytes()).unwrap();
  events.send(b"81f00ba2a0\n".to_vec()).unwrap();
  *active.lock().unwrap() = ACTIVE_FOOT.to_owned();
  assert_eq!(next(&mut rx).await, (s("foot"), s("0x5581f00ba2a0")));

  // Fallback query disagrees on the address -> class unknown.
  events.send(b"activewindowv2>>abc\n".to_vec()).unwrap();
  assert_eq!(next(&mut rx).await, (None, s("0xabc")));

  // Show through the same sink picks up the tracked target.
  sink.show(HyprIpc::from_dir(dir.path()).cursor().await);
  assert_eq!(
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap(),
    CompositorEvent::Show { cursor: Some((1287, 642)), app_id: None, window_id: s("0xabc") }
  );

  // Hyprland goes away -> the task ends.
  events.send(Vec::new()).unwrap();
  let start = std::time::Instant::now();
  while focus.is_running() {
    assert!(start.elapsed() < Duration::from_secs(5), "focus task did not stop");
    tokio::time::sleep(Duration::from_millis(20)).await;
  }
}

#[tokio::test]
async fn no_event_socket_is_an_error() {
  let dir = tempfile::tempdir().unwrap();
  let (sink, _rx) = EventSink::new();
  assert!(HyprFocus::spawn(HyprIpc::from_dir(dir.path()), sink).await.is_err());
}
