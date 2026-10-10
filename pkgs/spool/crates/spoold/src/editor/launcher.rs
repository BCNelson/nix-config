//! Starting the external editor and telling the user about problems.
//!
//! spoold never runs the editor as its own child: a child would inherit
//! spoold's seccomp sandbox (`MemoryDenyWriteExecute` breaks JIT-using
//! editors, `RestrictAddressFamilies=AF_UNIX` breaks anything that talks to
//! the network, `NoNewPrivileges`, the cleared environment). Instead
//! [`SystemdLauncher`] asks the systemd **user** manager (on the session
//! bus) for a transient service, `spool-edit-<random>.service`
//! (`Type=exec`, `CollectMode=inactive-or-failed`), running
//! `/bin/sh -c 'exec "$@"' spool-edit <argv...>` so the command is found on
//! the user manager's `PATH` and runs with the user's normal environment.
//! The editor's exit is followed through the unit's `ActiveState`
//! (`PropertiesChanged`), the start job's `JobRemoved`, and a slow poll as a
//! fallback.
//!
//! Both are traits so the session manager can be tested with fakes.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use zbus::message::Type as MsgType;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{MatchRule, MessageStream};

/// How the editor ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorExit {
  /// Exited normally (status 0, or a clean signal such as SIGTERM).
  Success,
  /// Failed / crashed (`why` is for the log; never content).
  Failed(String),
}

/// Resolves when the editor exits.
pub type ExitFuture = Pin<Box<dyn Future<Output = EditorExit> + Send>>;

/// Starts an editor command outside spoold.
#[async_trait::async_trait]
pub trait EditorLauncher: Send + Sync + 'static {
  /// Start `argv` (already expanded) as unit `unit`. `Ok` once it runs;
  /// the future resolves when it exits.
  async fn launch(&self, unit: &str, argv: &[String]) -> Result<ExitFuture, String>;
}

/// Desktop notifications (content-free: only reasons).
pub trait Notifier: Send + Sync + 'static {
  fn notify(&self, summary: &str, body: &str);
}

/// A lazily connected, shared session bus connection.
#[derive(Clone, Default)]
pub struct SessionBus {
  conn: Arc<tokio::sync::Mutex<Option<zbus::Connection>>>,
}

const BUS_TIMEOUT: Duration = Duration::from_secs(5);

impl SessionBus {
  pub async fn get(&self) -> Result<zbus::Connection, String> {
    let mut slot = self.conn.lock().await;
    if let Some(c) = slot.as_ref() {
      return Ok(c.clone());
    }
    let c = tokio::time::timeout(BUS_TIMEOUT, zbus::Connection::session())
      .await
      .map_err(|_| "connecting to the session bus timed out".to_string())?
      .map_err(|e| format!("no session bus: {e}"))?;
    *slot = Some(c.clone());
    Ok(c)
  }
}

const SYSTEMD: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const UNIT_IFACE: &str = "org.freedesktop.systemd1.Unit";
const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";
/// How often the unit's state is polled in case a signal was missed.
const POLL: Duration = Duration::from_secs(3);

/// `/org/freedesktop/systemd1/unit/<escaped name>` (systemd's bus path
/// escaping: every byte outside `[A-Za-z0-9]` -> `_xx`).
pub fn unit_object_path(unit: &str) -> String {
  let mut p = String::from("/org/freedesktop/systemd1/unit/");
  for (i, b) in unit.bytes().enumerate() {
    if b.is_ascii_alphabetic() || (b.is_ascii_digit() && i > 0) {
      p.push(b as char);
    } else {
      p.push_str(&format!("_{b:02x}"));
    }
  }
  p
}

/// The `ExecStart` argv: `sh -c 'exec "$@"'` resolves the command on the
/// user manager's `PATH` (transient units need an absolute path) without
/// ever interpreting the arguments.
pub fn exec_argv(argv: &[String]) -> Vec<String> {
  let mut v = vec!["/bin/sh".to_string(), "-c".into(), "exec \"$@\"".into(), "spool-edit".into()];
  v.extend(argv.iter().cloned());
  v
}

/// [`EditorLauncher`] over the systemd user manager.
pub struct SystemdLauncher {
  bus: SessionBus,
  subscribed: tokio::sync::OnceCell<()>,
}

impl SystemdLauncher {
  pub fn new(bus: SessionBus) -> Self {
    Self { bus, subscribed: tokio::sync::OnceCell::new() }
  }
}

/// Lifecycle as reported by `ActiveState`.
fn state_exit(state: &str, seen_running: &mut bool) -> Option<EditorExit> {
  match state {
    "activating" | "active" | "reloading" | "deactivating" => {
      *seen_running = true;
      None
    }
    "failed" => Some(EditorExit::Failed("the editor unit failed".into())),
    "inactive" if *seen_running => Some(EditorExit::Success),
    _ => None,
  }
}

#[async_trait::async_trait]
impl EditorLauncher for SystemdLauncher {
  async fn launch(&self, unit: &str, argv: &[String]) -> Result<ExitFuture, String> {
    let conn = self.bus.get().await?;
    // systemd only emits unit signals to subscribed clients.
    self
      .subscribed
      .get_or_try_init(|| async {
        conn
          .call_method(Some(SYSTEMD), SYSTEMD_PATH, Some(MANAGER), "Subscribe", &())
          .await
          .map(|_| ())
          .map_err(|e| format!("systemd user manager not reachable: {e}"))
      })
      .await?;
    let path = unit_object_path(unit);
    // Listen before starting, so a quick exit cannot be missed.
    let rule = MatchRule::builder()
      .msg_type(MsgType::Signal)
      .sender(SYSTEMD)
      .and_then(|b| b.interface(PROPS_IFACE))
      .and_then(|b| b.member("PropertiesChanged"))
      .and_then(|b| b.path(path.clone()))
      .map_err(|e| e.to_string())?
      .build();
    let props =
      MessageStream::for_match_rule(rule, &conn, Some(64)).await.map_err(|e| e.to_string())?;
    let rule = MatchRule::builder()
      .msg_type(MsgType::Signal)
      .sender(SYSTEMD)
      .and_then(|b| b.interface(MANAGER))
      .and_then(|b| b.member("JobRemoved"))
      .and_then(|b| b.path(SYSTEMD_PATH))
      .map_err(|e| e.to_string())?
      .build();
    let jobs =
      MessageStream::for_match_rule(rule, &conn, Some(64)).await.map_err(|e| e.to_string())?;

    let exec = exec_argv(argv);
    let mut env: Vec<String> = Vec::new();
    for k in ["WAYLAND_DISPLAY", "DISPLAY"] {
      if let Some(v) = std::env::var_os(k).and_then(|v| v.into_string().ok()) {
        env.push(format!("{k}={v}"));
      }
    }
    let props_v: Vec<(&str, Value<'_>)> = vec![
      ("Description", Value::from("Spool: edit a clipboard item")),
      ("Type", Value::from("exec")),
      ("CollectMode", Value::from("inactive-or-failed")),
      ("WorkingDirectory", Value::from("~")),
      ("Environment", Value::from(env)),
      ("ExecStart", Value::from(vec![(exec[0].clone(), exec.clone(), false)])),
    ];
    let aux: Vec<(&str, Vec<(&str, Value<'_>)>)> = Vec::new();
    let reply = conn
      .call_method(
        Some(SYSTEMD),
        SYSTEMD_PATH,
        Some(MANAGER),
        "StartTransientUnit",
        &(unit, "fail", props_v, aux),
      )
      .await
      .map_err(|e| format!("starting the editor unit failed: {e}"))?;
    let job: OwnedObjectPath = reply.body().deserialize().map_err(|e| e.to_string())?;
    let unit = unit.to_owned();
    Ok(Box::pin(watch_unit(conn, unit, path, job, props, jobs)))
  }
}

async fn watch_unit(
  conn: zbus::Connection,
  unit: String,
  path: String,
  job: OwnedObjectPath,
  mut props: MessageStream,
  mut jobs: MessageStream,
) -> EditorExit {
  let mut seen_running = false;
  let mut poll = tokio::time::interval(POLL);
  poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
  poll.tick().await;
  loop {
    tokio::select! {
      Some(Ok(msg)) = props.next() => {
        type Changed = (String, HashMap<String, OwnedValue>, Vec<String>);
        if let Ok((iface, changed, _)) = msg.body().deserialize::<Changed>()
          && iface == UNIT_IFACE
          && let Some(state) = changed.get("ActiveState").and_then(|v| v.downcast_ref::<String>().ok())
          && let Some(exit) = state_exit(&state, &mut seen_running)
        {
          return exit;
        }
      }
      Some(Ok(msg)) = jobs.next() => {
        type Removed = (u32, OwnedObjectPath, String, String);
        if let Ok((_, j, _, result)) = msg.body().deserialize::<Removed>()
          && j == job
        {
          if result != "done" {
            return EditorExit::Failed(format!("start job {result}"));
          }
          seen_running = true;
        }
      }
      _ = poll.tick() => {
        let got = conn
          .call_method(
            Some(SYSTEMD),
            path.as_str(),
            Some(PROPS_IFACE),
            "Get",
            &(UNIT_IFACE, "ActiveState"),
          )
          .await;
        match got {
          Ok(reply) => {
            let state = reply
              .body()
              .deserialize::<OwnedValue>()
              .ok()
              .and_then(|v| v.downcast_ref::<String>().ok());
            if let Some(exit) = state.and_then(|s| state_exit(&s, &mut seen_running)) {
              return exit;
            }
          }
          // Collected (CollectMode) after it stopped and we missed the signal.
          Err(e) if seen_running => {
            tracing::debug!(unit = %unit, "editor unit gone ({e}); assuming it exited");
            return EditorExit::Success;
          }
          Err(e) => tracing::debug!(unit = %unit, "polling the editor unit: {e}"),
        }
      }
    }
  }
}

/// [`Notifier`] over `org.freedesktop.Notifications` (fire and forget).
pub struct DbusNotifier {
  bus: SessionBus,
}

impl DbusNotifier {
  pub fn new(bus: SessionBus) -> Self {
    Self { bus }
  }
}

impl Notifier for DbusNotifier {
  fn notify(&self, summary: &str, body: &str) {
    let bus = self.bus.clone();
    let (summary, body) = (summary.to_owned(), body.to_owned());
    tokio::spawn(async move {
      let conn = match bus.get().await {
        Ok(c) => c,
        Err(e) => return tracing::debug!("notification not sent: {e}"),
      };
      let hints: HashMap<&str, Value<'_>> = HashMap::new();
      let r = conn
        .call_method(
          Some("org.freedesktop.Notifications"),
          "/org/freedesktop/Notifications",
          Some("org.freedesktop.Notifications"),
          "Notify",
          &(
            "Spool",
            0u32,
            "edit-paste",
            summary.as_str(),
            body.as_str(),
            Vec::<&str>::new(),
            hints,
            -1i32,
          ),
        )
        .await;
      if let Err(e) = r {
        tracing::debug!("notification not sent: {e}");
      }
    });
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn unit_paths_are_escaped_like_systemd() {
    assert_eq!(
      unit_object_path("spool-edit-0a1b.service"),
      "/org/freedesktop/systemd1/unit/spool_2dedit_2d0a1b_2eservice"
    );
    assert_eq!(unit_object_path("1x.service"), "/org/freedesktop/systemd1/unit/_31x_2eservice");
  }

  #[test]
  fn argv_is_passed_through_sh_without_interpretation() {
    let argv = vec!["kate".to_string(), "--block".into(), "/run/x/it em;$(rm).txt".into()];
    let v = exec_argv(&argv);
    assert_eq!(&v[..4], ["/bin/sh", "-c", "exec \"$@\"", "spool-edit"]);
    assert_eq!(&v[4..], &argv[..]);
    // And sh really runs it verbatim.
    let out = std::process::Command::new(&v[0])
      .args(&v[1..4])
      .args(["printf", "%s|", "a b", "$(echo no)", "*"])
      .output()
      .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "a b|$(echo no)|*|");
  }

  #[test]
  fn active_state_transitions() {
    let mut seen = false;
    assert_eq!(state_exit("inactive", &mut seen), None, "not started yet");
    assert_eq!(state_exit("activating", &mut seen), None);
    assert!(seen);
    assert_eq!(state_exit("active", &mut seen), None);
    assert_eq!(state_exit("inactive", &mut seen), Some(EditorExit::Success));
    let mut seen = false;
    assert!(matches!(state_exit("failed", &mut seen), Some(EditorExit::Failed(_))));
  }
}
