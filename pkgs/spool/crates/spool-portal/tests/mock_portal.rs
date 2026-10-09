//! spool-portal against a **mock** `org.freedesktop.portal.Desktop` on a
//! **private** `dbus-daemon` (the user's session bus is never used). Gated
//! on `SPOOL_PORTAL_TESTS=1`; needs `dbus-daemon` on PATH (dev shell):
//!
//! ```sh
//! SPOOL_PORTAL_TESTS=1 cargo test -p spool-portal --test mock_portal
//! ```
//!
//! The mock implements just enough of GlobalShortcuts (v1) and
//! `org.freedesktop.host.portal.Registry` to check the request sequence,
//! the arguments spool sends, and the Activated -> Show mapping.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait_shim::Cursor;
use spool_compositor::{CompositorEvent, EventSink};
use spool_portal::{PortalConfig, PortalError, PortalShortcuts};
use zbus::message::Header;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, interface};

fn enabled() -> bool {
  if std::env::var("SPOOL_PORTAL_TESTS").as_deref() != Ok("1") {
    eprintln!("skipping: set SPOOL_PORTAL_TESTS=1 (dev shell) to run mock-portal tests");
    return false;
  }
  true
}

struct Bus {
  child: Child,
  addr: String,
  _dir: tempfile::TempDir,
}

impl Drop for Bus {
  fn drop(&mut self) {
    let _ = self.child.kill();
    let _ = self.child.wait();
  }
}

fn private_bus() -> Bus {
  let dir = tempfile::Builder::new().prefix("spp").tempdir_in("/tmp").unwrap();
  // Own config without <servicedir>/<standard_session_servicedirs>: with
  // `--session` the bus would D-Bus-activate the real xdg-desktop-portal
  // (and its backends) from the system's service directories.
  let conf = dir.path().join("bus.conf");
  std::fs::write(
    &conf,
    format!(
      r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}/bus</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
      dir.path().display()
    ),
  )
  .unwrap();
  let mut child = Command::new("dbus-daemon")
    .env_clear()
    .env("PATH", std::env::var_os("PATH").unwrap_or_default())
    .args(["--nofork", "--nopidfile", "--print-address=1"])
    .arg(format!("--config-file={}", conf.display()))
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .process_group(0)
    .spawn()
    .expect("dbus-daemon (dev shell)");
  let mut line = String::new();
  BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
  let got = line.trim().to_owned();
  assert!(got.starts_with("unix:path=/tmp/spp"), "unexpected bus {got}");
  if let Ok(inherited) = std::env::var("DBUS_SESSION_BUS_ADDRESS") {
    assert_ne!(inherited, got, "refusing to use the inherited session bus");
  }
  Bus { child, addr: got, _dir: dir }
}

async fn connect(addr: &str) -> Connection {
  zbus::connection::Builder::address(addr).unwrap().build().await.unwrap()
}

#[derive(Default, Debug)]
struct Log {
  calls: Vec<String>,
  app_id: Option<String>,
  session: Option<OwnedObjectPath>,
  client: Option<String>,
  shortcuts: Vec<(String, HashMap<String, OwnedValue>)>,
}

#[derive(Clone)]
struct Mock {
  log: Arc<Mutex<Log>>,
  /// Reply to BindShortcuts with "cancelled" (response 1).
  cancel_bind: bool,
}

fn token(opts: &HashMap<String, OwnedValue>, key: &str) -> String {
  String::try_from(opts.get(key).expect(key).clone()).unwrap()
}

fn sender_id(hdr: &Header<'_>) -> String {
  hdr.sender().unwrap().as_str().trim_start_matches(':').replace('.', "_")
}

/// Emits `Request.Response` on `path` a moment after the method returned
/// (the client subscribes before calling, as the real portal expects).
fn respond(
  conn: &Connection,
  dest: String,
  path: String,
  code: u32,
  results: HashMap<String, OwnedValue>,
) {
  let conn = conn.clone();
  tokio::spawn(async move {
    tokio::time::sleep(Duration::from_millis(20)).await;
    conn
      .emit_signal(
        Some(dest.as_str()),
        path.as_str(),
        "org.freedesktop.portal.Request",
        "Response",
        &(code, results),
      )
      .await
      .unwrap();
  });
}

#[interface(name = "org.freedesktop.host.portal.Registry")]
impl Mock {
  async fn register(&self, app_id: String, _options: HashMap<String, OwnedValue>) {
    let mut l = self.log.lock().unwrap();
    l.calls.push("Register".into());
    l.app_id = Some(app_id);
  }
}

struct Shortcuts(Mock);

#[interface(name = "org.freedesktop.portal.GlobalShortcuts")]
impl Shortcuts {
  #[zbus(property, name = "version")]
  fn version(&self) -> u32 {
    1
  }

  async fn create_session(
    &self,
    options: HashMap<String, OwnedValue>,
    #[zbus(header)] hdr: Header<'_>,
    #[zbus(connection)] conn: &Connection,
  ) -> OwnedObjectPath {
    let id = sender_id(&hdr);
    let req =
      format!("/org/freedesktop/portal/desktop/request/{id}/{}", token(&options, "handle_token"));
    let session = format!(
      "/org/freedesktop/portal/desktop/session/{id}/{}",
      token(&options, "session_handle_token")
    );
    {
      let mut l = self.0.log.lock().unwrap();
      l.calls.push("CreateSession".into());
      l.session = Some(ObjectPath::try_from(session.clone()).unwrap().into());
      l.client = Some(hdr.sender().unwrap().to_string());
    }
    // xdg-desktop-portal sends the handle as a string.
    let results = HashMap::from([(
      "session_handle".to_owned(),
      OwnedValue::try_from(Value::from(session)).unwrap(),
    )]);
    respond(conn, hdr.sender().unwrap().to_string(), req.clone(), 0, results);
    ObjectPath::try_from(req).unwrap().into()
  }

  async fn bind_shortcuts(
    &self,
    session: OwnedObjectPath,
    shortcuts: Vec<(String, HashMap<String, OwnedValue>)>,
    _parent_window: String,
    options: HashMap<String, OwnedValue>,
    #[zbus(header)] hdr: Header<'_>,
    #[zbus(connection)] conn: &Connection,
  ) -> OwnedObjectPath {
    let id = sender_id(&hdr);
    let req =
      format!("/org/freedesktop/portal/desktop/request/{id}/{}", token(&options, "handle_token"));
    {
      let mut l = self.0.log.lock().unwrap();
      l.calls.push("BindShortcuts".into());
      assert_eq!(l.session.as_ref(), Some(&session));
      l.shortcuts = shortcuts.clone();
    }
    let bound: Vec<(String, HashMap<String, Value>)> = shortcuts
      .iter()
      .map(|(id, _)| {
        (
          id.clone(),
          HashMap::from([
            ("description".to_owned(), Value::from("Spool: show clipboard history")),
            ("trigger_description".to_owned(), Value::from("Super+V")),
          ]),
        )
      })
      .collect();
    let results = if self.0.cancel_bind {
      HashMap::new()
    } else {
      HashMap::from([("shortcuts".to_owned(), OwnedValue::try_from(Value::from(bound)).unwrap())])
    };
    respond(
      conn,
      hdr.sender().unwrap().to_string(),
      req.clone(),
      u32::from(self.0.cancel_bind),
      results,
    );
    ObjectPath::try_from(req).unwrap().into()
  }
}

async fn start_mock(bus: &Bus, cancel_bind: bool) -> (Connection, Mock) {
  let conn = connect(&bus.addr).await;
  let mock = Mock { log: Arc::default(), cancel_bind };
  conn.object_server().at("/org/freedesktop/portal/desktop", mock.clone()).await.unwrap();
  conn
    .object_server()
    .at("/org/freedesktop/portal/desktop", Shortcuts(mock.clone()))
    .await
    .unwrap();
  conn.request_name("org.freedesktop.portal.Desktop").await.unwrap();
  (conn, mock)
}

async fn activate(portal: &Connection, client: &str, session: &str, id: &str) {
  portal
    .emit_signal(
      Some(client),
      "/org/freedesktop/portal/desktop",
      "org.freedesktop.portal.GlobalShortcuts",
      "Activated",
      &(ObjectPath::try_from(session).unwrap(), id, 1234u64, HashMap::<String, Value>::new()),
    )
    .await
    .unwrap();
}

mod async_trait_shim {
  /// Fixed cursor provider.
  pub struct Cursor(pub (i32, i32));
  #[async_trait::async_trait]
  impl spool_compositor::CursorProvider for Cursor {
    async fn cursor(&self) -> Option<(i32, i32)> {
      Some(self.0)
    }
  }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn binds_and_forwards_activations() {
  if !enabled() {
    return;
  }
  let bus = private_bus();
  let (portal, mock) = start_mock(&bus, false).await;
  let client = connect(&bus.addr).await;
  let (sink, mut rx) = EventSink::new();
  sink.active_window(Some("foot"), Some("0x5581f00ba2a0"));
  let _ = rx.recv().await;

  let ps = PortalShortcuts::start(
    &client,
    PortalConfig::default(),
    sink.clone(),
    Some(Arc::new(Cursor((300, 400)))),
  )
  .await
  .expect("bind");
  assert_eq!(ps.trigger_description(), Some("Super+V"));
  let (session, client_name) = {
    let l = mock.log.lock().unwrap();
    // Registry first: it must precede any other portal call on the connection.
    assert_eq!(l.calls, ["Register", "CreateSession", "BindShortcuts"]);
    assert_eq!(l.app_id.as_deref(), Some("dev.bcnelson.spool.daemon"));
    assert_eq!(l.shortcuts.len(), 1);
    let (id, opts) = &l.shortcuts[0];
    assert_eq!(id, "spool-show");
    assert_eq!(
      String::try_from(opts["description"].clone()).unwrap(),
      "Spool: show clipboard history"
    );
    assert_eq!(String::try_from(opts["preferred_trigger"].clone()).unwrap(), "LOGO+v");
    (l.session.clone().unwrap().to_string(), l.client.clone().unwrap())
  };

  // Foreign session and foreign shortcut ids are ignored.
  activate(&portal, &client_name, "/org/freedesktop/portal/desktop/session/x/other", "spool-show")
    .await;
  activate(&portal, &client_name, &session, "something-else").await;
  activate(&portal, &client_name, &session, "spool-show").await;
  let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
  assert_eq!(
    ev,
    CompositorEvent::Show {
      cursor: Some((300, 400)),
      app_id: Some("foot".into()),
      window_id: Some("0x5581f00ba2a0".into())
    }
  );
  assert!(rx.try_recv().is_err(), "filtered activations must not produce events");
  assert!(ps.is_running());
  ps.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_cursor_provider_means_centered() {
  if !enabled() {
    return;
  }
  let bus = private_bus();
  let (portal, mock) = start_mock(&bus, false).await;
  let client = connect(&bus.addr).await;
  let (sink, mut rx) = EventSink::new();
  let cfg = PortalConfig { app_id: None, ..Default::default() };
  let _ps = PortalShortcuts::start(&client, cfg, sink, None).await.expect("bind");
  let (session, client_name) = {
    let l = mock.log.lock().unwrap();
    assert_eq!(l.calls, ["CreateSession", "BindShortcuts"], "no Register without an app id");
    (l.session.clone().unwrap().to_string(), l.client.clone().unwrap())
  };
  activate(&portal, &client_name, &session, "spool-show").await;
  let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
  assert_eq!(ev, CompositorEvent::Show { cursor: None, app_id: None, window_id: None });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_binding() {
  if !enabled() {
    return;
  }
  let bus = private_bus();
  let (_portal, _mock) = start_mock(&bus, true).await;
  let client = connect(&bus.addr).await;
  let (sink, _rx) = EventSink::new();
  match PortalShortcuts::start(&client, PortalConfig::default(), sink, None).await {
    Err(PortalError::Cancelled) => {}
    Err(e) => panic!("expected Cancelled, got {e}"),
    Ok(_) => panic!("expected Cancelled"),
  }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_portal_is_unsupported() {
  if !enabled() {
    return;
  }
  let bus = private_bus();
  let client = connect(&bus.addr).await;
  let (sink, _rx) = EventSink::new();
  match PortalShortcuts::start(&client, PortalConfig::default(), sink, None).await {
    Err(PortalError::Unsupported(_)) => {}
    Err(e) => panic!("expected Unsupported, got {e}"),
    Ok(_) => panic!("expected Unsupported"),
  }
}
