//! IPC integration tests: the real socket server (`ipc::bind` + `ipc::serve`)
//! on a temp socket with a fake orchestrator backend, driven by spoolctl's
//! real blocking `Client` (compiled in via `#[path]`) and by raw frames.

#[path = "../../spoolctl/src/client.rs"]
#[allow(dead_code)]
mod client;

use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use spool_proto::{
  ErrorCode, FrameError, Hello, MAX_FRAME, PauseState, PublicReq, PublicResp, StatusInfo,
  read_frame, write_frame,
};
use tokio::sync::mpsc;

use crate::ipc;
use crate::orchestrator::Request;

/// Fake orchestrator: answers Status from local state, toggles pause.
async fn fake_backend(mut rx: mpsc::Receiver<Request>) {
  let mut paused = PauseState::Recording;
  while let Some(Request::Public { req, reply, .. }) = rx.recv().await {
    let resp = match req {
      PublicReq::Status => PublicResp::Status(StatusInfo {
        version: "test".into(),
        paused,
        item_count: 42,
        compositor: Some("fake".into()),
        primary_enabled: false,
        encrypted: false,
        unlocked: true,
        key_state: "session-only".into(),
        capabilities: None,
      }),
      PublicReq::Pause { secs: None } => {
        paused = PauseState::PausedIndefinitely;
        PublicResp::Ok
      }
      PublicReq::Pause { secs: Some(s) } => {
        paused = PauseState::PausedUntil { unix_ms: u64::from(s) * 1000 };
        PublicResp::Ok
      }
      PublicReq::Resume => {
        paused = PauseState::Recording;
        PublicResp::Ok
      }
      PublicReq::Show | PublicReq::Pick | PublicReq::Edit | PublicReq::New { .. } => {
        PublicResp::NotYetImplemented
      }
      PublicReq::Current => PublicResp::Empty,
      PublicReq::Copy { .. } => PublicResp::Ok,
    };
    let _ = reply.send(resp);
  }
}

struct Server {
  sock: PathBuf,
  // Field order: guard (removes the socket) drops before the tempdir.
  _guard: ipc::InstanceGuard,
  _dir: tempfile::TempDir,
}

/// Bind on `<tmp>/spool/sock` and serve with the fake backend.
async fn start() -> Server {
  let dir = tempfile::tempdir().unwrap();
  let sock = dir.path().join("spool").join("sock");
  let ipc::BoundSocket { listener, guard } = ipc::bind(&sock).unwrap();
  let (tx, rx) = mpsc::channel(8);
  tokio::spawn(fake_backend(rx));
  tokio::spawn(ipc::serve(listener, tx, ipc::show_limiter()));
  Server { sock, _guard: guard, _dir: dir }
}

fn blocking<T: Send + 'static>(
  f: impl FnOnce() -> T + Send + 'static,
) -> tokio::task::JoinHandle<T> {
  tokio::task::spawn_blocking(f)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_status_pause_resume() {
  let srv = start().await;
  let sock = srv.sock.clone();
  blocking(move || {
    let mut c = client::Client::connect_to(&sock).unwrap();
    let PublicResp::Status(s) = c.request(&PublicReq::Status).unwrap() else { panic!() };
    assert_eq!((s.item_count, s.paused), (42, PauseState::Recording));
    assert_eq!(c.request(&PublicReq::Pause { secs: Some(5) }).unwrap(), PublicResp::Ok);
    let PublicResp::Status(s) = c.request(&PublicReq::Status).unwrap() else { panic!() };
    assert_eq!(s.paused, PauseState::PausedUntil { unix_ms: 5000 });
    assert_eq!(c.request(&PublicReq::Pause { secs: None }).unwrap(), PublicResp::Ok);
    assert_eq!(c.request(&PublicReq::Resume).unwrap(), PublicResp::Ok);
    // A second connection sees the shared backend state.
    let mut c2 = client::Client::connect_to(&sock).unwrap();
    let PublicResp::Status(s) = c2.request(&PublicReq::Status).unwrap() else { panic!() };
    assert_eq!(s.paused, PauseState::Recording);
    assert_eq!(c2.request(&PublicReq::Current).unwrap(), PublicResp::Empty);
  })
  .await
  .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_mismatch_closes() {
  let srv = start().await;
  let sock = srv.sock.clone();
  blocking(move || {
    let mut s = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    write_frame(&mut s, &Hello { proto: 999 }).unwrap();
    let h: Hello = read_frame(&mut s).unwrap();
    assert_eq!(h, Hello::current());
    // Server closes after its Hello; a request gets no answer.
    let _ = write_frame(&mut s, &PublicReq::Status);
    assert!(matches!(
      read_frame::<_, PublicResp>(&mut s),
      Err(FrameError::Eof | FrameError::Io(_))
    ));
  })
  .await
  .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_and_malformed_frames() {
  let srv = start().await;
  let sock = srv.sock.clone();
  blocking(move || {
    let mut s = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    write_frame(&mut s, &Hello::current()).unwrap();
    let _: Hello = read_frame(&mut s).unwrap();
    // Only a length prefix announcing MAX_FRAME + 1: rejected before the
    // payload is read.
    s.write_all(&((MAX_FRAME as u32) + 1).to_be_bytes()).unwrap();
    match read_frame::<_, PublicResp>(&mut s).unwrap() {
      PublicResp::Error { code: ErrorCode::TooLarge, .. } => {}
      other => panic!("unexpected {other:?}"),
    }
    let mut rest = Vec::new();
    s.read_to_end(&mut rest).unwrap();
    assert!(rest.is_empty(), "connection should be closed");

    // Garbage payload -> BadRequest, then close.
    let mut s = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    write_frame(&mut s, &Hello::current()).unwrap();
    let _: Hello = read_frame(&mut s).unwrap();
    s.write_all(&[0, 0, 0, 2, 0xff, 0xff]).unwrap();
    match read_frame::<_, PublicResp>(&mut s).unwrap() {
      PublicResp::Error { code: ErrorCode::BadRequest, .. } => {}
      other => panic!("unexpected {other:?}"),
    }
    assert!(matches!(read_frame::<_, PublicResp>(&mut s), Err(FrameError::Eof)));

    // The client refuses to send an oversized frame itself.
    let mut c = client::Client::connect_to(&sock).unwrap();
    let big = PublicReq::Copy { mime: "x/y".into(), data: vec![0; MAX_FRAME] };
    assert!(c.request(&big).unwrap_err().to_string().contains("exceeds"));
  })
  .await
  .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn show_pick_rate_limited() {
  let srv = start().await;
  let sock = srv.sock.clone();
  blocking(move || {
    let mut c = client::Client::connect_to(&sock).unwrap();
    for _ in 0..ipc::RATE_BURST {
      assert_eq!(c.request(&PublicReq::Show).unwrap(), PublicResp::NotYetImplemented);
    }
    // Limit is shared across connections and covers Pick too.
    let mut c2 = client::Client::connect_to(&sock).unwrap();
    match c2.request(&PublicReq::Pick).unwrap() {
      PublicResp::Error { code: ErrorCode::RateLimited, .. } => {}
      other => panic!("unexpected {other:?}"),
    }
    // Other requests are not limited.
    assert!(matches!(c.request(&PublicReq::Status).unwrap(), PublicResp::Status(_)));
  })
  .await
  .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bind_single_instance_and_stale_socket() {
  let srv = start().await;
  // Same path again: lock held -> refused.
  let err = ipc::bind(&srv.sock).unwrap_err().to_string();
  assert!(err.contains("already running"), "{err}");
  let meta = std::fs::metadata(&srv.sock).unwrap();
  assert_eq!(meta.mode() & 0o777, 0o600);
  assert_eq!(std::fs::metadata(srv.sock.parent().unwrap()).unwrap().mode() & 0o777, 0o700);

  // Stale socket (file exists, nobody listening, no lock) is replaced.
  let dir = tempfile::tempdir().unwrap();
  let sock = dir.path().join("spool").join("sock");
  std::fs::create_dir(sock.parent().unwrap()).unwrap();
  std::fs::set_permissions(sock.parent().unwrap(), std::fs::Permissions::from_mode(0o700)).unwrap();
  drop(std::os::unix::net::UnixListener::bind(&sock).unwrap());
  assert!(sock.exists());
  let b = ipc::bind(&sock).unwrap();
  drop(b);
  // The guard removes our socket on drop.
  assert!(!sock.exists());

  // A live socket without the lock (e.g. a foreign listener): refused.
  let dir2 = tempfile::tempdir().unwrap();
  std::fs::set_permissions(dir2.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
  let sock2 = dir2.path().join("sock");
  let _live = std::os::unix::net::UnixListener::bind(&sock2).unwrap();
  let err = ipc::bind(&sock2).unwrap_err().to_string();
  assert!(err.contains("already answering"), "{err}");

  // A regular file at the socket path is never removed.
  let dir3 = tempfile::tempdir().unwrap();
  std::fs::set_permissions(dir3.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
  let f = dir3.path().join("sock");
  std::fs::write(&f, b"keep").unwrap();
  assert!(ipc::bind(&f).unwrap_err().to_string().contains("not a socket"));
  assert_eq!(std::fs::read(&f).unwrap(), b"keep");
}

#[tokio::test]
async fn bind_refuses_loose_dir() {
  let dir = tempfile::tempdir().unwrap();
  let parent = dir.path().join("spool");
  std::fs::create_dir(&parent).unwrap();
  std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
  let err = ipc::bind(&parent.join("sock")).unwrap_err().to_string();
  assert!(err.contains("0755"), "{err}");
}

#[test]
fn client_reports_missing_daemon() {
  let dir = tempfile::tempdir().unwrap();
  let err = client::Client::connect_to(&dir.path().join("sock")).err().unwrap().to_string();
  assert!(err.contains("not running"), "{err}");
}

#[test]
fn client_reports_version_mismatch() {
  let dir = tempfile::tempdir().unwrap();
  let p: &Path = &dir.path().join("sock");
  let l = std::os::unix::net::UnixListener::bind(p).unwrap();
  let t = std::thread::spawn(move || {
    let (mut s, _) = l.accept().unwrap();
    let _: Hello = read_frame(&mut s).unwrap();
    write_frame(&mut s, &Hello { proto: 999 }).unwrap();
  });
  let err = client::Client::connect_to(p).err().unwrap().to_string();
  assert!(err.contains("mismatch") && err.contains("v999"), "{err}");
  t.join().unwrap();
}
