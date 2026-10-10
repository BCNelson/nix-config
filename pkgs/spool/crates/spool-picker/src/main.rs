//! `spool-picker`: the Spool history picker.
//!
//! # Contract with spoold
//!
//! * spoold creates a `socketpair(AF_UNIX, SOCK_STREAM)`, puts one end on
//!   fd 3 of the child (CLOEXEC cleared), sets `SPOOL_PICKER_FD=3` and
//!   `WAYLAND_DISPLAY`, and execs `spool-picker` once (resident process).
//! * The picker sends `Hello` first; spoold answers with its `Hello`.
//!   Then frames are `PickerEvt` (daemon -> picker) and `PickerReq`
//!   (picker -> daemon), postcard + u32 BE length (`spool_proto` framing).
//! * Picker protocol v3 (`spool_proto::picker`): the picker sends
//!   `Hello::picker()` first. It queries the first page (`Query{seq, q: "",
//!   offset: 0}`) at startup, pre-renders it and then sends `Ready`; `Show`
//!   maps the surface, `Hide` (or Esc, focus loss, a selection) unmaps it,
//!   and every unmap is reported as `Hidden{reason}`. `Page`/`Thumb`/`Error`
//!   answers carry the request's `seq` and may come in any order. While the
//!   daemon reports `Locked{providers}` an unlock panel sits above the list
//!   (`Unlock` -> `Unlocked` / `UnlockFailed`).
//! * Renderer: `SPOOL_PICKER_RENDERER=software` (default) or `gpu`
//!   (FemtoVG over EGL; only in builds with the `gpu` feature). The GPU path
//!   falls back to software if EGL cannot be set up or is a software GL
//!   (llvmpipe), and does not pre-render while hidden (see `gpu.rs`).
//! * EOF on the socket = exit 0. Logs go to stderr (never item content);
//!   `SPOOL_PICKER_LOG` sets the filter.
//! * `SPOOL_PICKER_PRERENDER=0` frees the frame buffers while hidden instead
//!   of keeping the next frame pre-rendered (default: pre-render).
//! * Theme: `$XDG_CONFIG_HOME/kdeglobals` (or `~/.config`), watched with
//!   inotify.

#![deny(unsafe_code)]

mod app;
mod edit;
#[cfg(feature = "gpu")]
mod gpu;
mod ipc;
mod model;
mod render;
mod sanitize;
mod theme;
mod thumb;

pub mod ui {
  slint::include_modules!();
}

use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use smithay_client_toolkit::reexports::calloop::channel::Event as ChanEvent;
use smithay_client_toolkit::reexports::calloop::generic::Generic;
use smithay_client_toolkit::reexports::calloop::{EventLoop, Interest, Mode, PostAction};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::Connection;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;

use crate::app::{App, ThumbDone, ThumbJob};
use crate::ipc::{Channel, Inbound};
use crate::render::{Backend, SoftwareFrameRenderer};

struct PickerPlatform {
  window: Rc<dyn WindowAdapter>,
  start: Instant,
}

impl Platform for PickerPlatform {
  fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
    Ok(self.window.clone())
  }

  fn duration_since_start(&self) -> Duration {
    self.start.elapsed()
  }
}

/// Take ownership of the inherited socketpair end named by `SPOOL_PICKER_FD`.
fn take_picker_fd() -> anyhow::Result<OwnedFd> {
  let raw = std::env::var("SPOOL_PICKER_FD")
    .context("SPOOL_PICKER_FD not set (spool-picker is started by spoold)")?;
  let n: i32 = raw.trim().parse().context("SPOOL_PICKER_FD is not a number")?;
  if n < 3 {
    bail!("SPOOL_PICKER_FD must be >= 3");
  }
  // Refuse anything that is not an open socket before taking ownership.
  let borrowed =
    rustix::io::fcntl_getfd(unsafe_borrow(n)).map(|_| ()).context("SPOOL_PICKER_FD is not open");
  borrowed?;
  // SAFETY: the fd was inherited from spoold for our exclusive use, is open
  // (checked above) and nothing else in this process owns it.
  #[allow(unsafe_code)]
  let fd = unsafe { OwnedFd::from_raw_fd(n) };
  let st = rustix::fs::fstat(&fd).context("fstat picker fd")?;
  if rustix::fs::FileType::from_raw_mode(st.st_mode) != rustix::fs::FileType::Socket {
    bail!("SPOOL_PICKER_FD is not a socket");
  }
  rustix::io::fcntl_setfd(&fd, rustix::io::FdFlags::CLOEXEC)?;
  Ok(fd)
}

#[allow(unsafe_code)]
fn unsafe_borrow(n: i32) -> std::os::fd::BorrowedFd<'static> {
  // SAFETY: only used for an F_GETFD probe on an fd number we were handed;
  // an invalid number yields EBADF, not UB, for fcntl.
  unsafe { std::os::fd::BorrowedFd::borrow_raw(n) }
}

fn main() {
  // Before anything else: no core dumps / ptrace by same-uid processes of a
  // process that holds clipboard history.
  if let Err(e) =
    rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
  {
    eprintln!("spool-picker: PR_SET_DUMPABLE failed: {e}");
    std::process::exit(1);
  }
  tracing_subscriber::fmt()
    .with_writer(std::io::stderr)
    .with_env_filter(
      tracing_subscriber::EnvFilter::try_from_env("SPOOL_PICKER_LOG")
        .unwrap_or_else(|_| "info".into()),
    )
    .init();
  match run() {
    Ok(()) => {}
    Err(e) => {
      tracing::error!("{e:#}");
      std::process::exit(1);
    }
  }
}

fn run() -> anyhow::Result<()> {
  let t0 = Instant::now();
  let stage =
    |name: &str| tracing::debug!(ms = t0.elapsed().as_secs_f64() * 1e3, "startup: {name}");
  let fd = take_picker_fd()?;
  let (chan, inbound) = Channel::connect(fd)?;
  stage("handshake");

  let conn = Connection::connect_to_env().context("connect to the Wayland compositor")?;
  stage("wayland connect");
  let renderer = choose_renderer(&conn);
  stage("renderer");
  slint::platform::set_platform(Box::new(PickerPlatform {
    window: renderer.adapter(),
    start: Instant::now(),
  }))
  .map_err(|e| anyhow::anyhow!("set Slint platform: {e}"))?;
  stage("platform");
  let ui = ui::PickerWindow::new()?;
  stage("slint ui");

  let (globals, queue) = registry_queue_init::<App>(&conn).context("Wayland registry")?;
  let qh = queue.handle();
  let mut event_loop: EventLoop<'static, App> = EventLoop::try_new()?;
  let handle = event_loop.handle();

  // Thumbnail decoding off the UI thread.
  let (job_tx, job_rx) = std::sync::mpsc::channel::<ThumbJob>();
  let (done_tx, done_rx) =
    smithay_client_toolkit::reexports::calloop::channel::channel::<ThumbDone>();
  std::thread::Builder::new().name("picker-thumbs".into()).spawn(move || {
    while let Ok(job) = job_rx.recv() {
      let result = thumb::decode_thumbnail(&job.bytes, job.box_w, job.box_h);
      if done_tx.send(ThumbDone { id: job.id, result }).is_err() {
        return;
      }
    }
  })?;

  let mut app = App::new(conn.clone(), &globals, qh, handle.clone(), ui, renderer, chan, job_tx)?;
  stage("wayland + app");

  // Learn outputs (wl_output, then xdg-output names/geometry) and seat
  // capabilities before creating the surface.
  let mut queue = queue;
  queue.roundtrip(&mut app).context("Wayland roundtrip")?;
  queue.roundtrip(&mut app).context("Wayland roundtrip")?;
  stage("roundtrips");
  WaylandSource::new(conn.clone(), queue)
    .insert(handle.clone())
    .map_err(|e| anyhow::anyhow!("{e}"))?;
  handle
    .insert_source(done_rx, |e, _, app: &mut App| {
      if let ChanEvent::Msg(done) = e {
        app.on_thumb_done(done);
      }
    })
    .map_err(|e| anyhow::anyhow!("{e}"))?;
  if let Some(src) = theme_watch() {
    handle
      .insert_source(src, |_, fd, app: &mut App| {
        let mut buf = [std::mem::MaybeUninit::<u8>::uninit(); 4096];
        let mut hit = false;
        let mut reader = rustix::fs::inotify::Reader::new(fd.as_fd(), &mut buf);
        while let Ok(ev) = reader.next() {
          if ev.file_name().is_some_and(|n| n.to_bytes() == b"kdeglobals") {
            hit = true;
          }
        }
        if hit {
          app.reload_theme();
        }
        Ok(PostAction::Continue)
      })
      .map_err(|e| anyhow::anyhow!("{e}"))?;
  }

  app.init_surface();
  stage("surface");
  // Only now listen to the daemon: a Show must find the surface in place.
  handle
    .insert_source(inbound, |e, _, app: &mut App| match e {
      ChanEvent::Msg(Inbound::Evt(evt)) => app.on_daemon_event(evt),
      ChanEvent::Msg(Inbound::Closed(why)) => app.exit = Some(why),
      ChanEvent::Closed => app.exit = Some("channel closed".into()),
    })
    .map_err(|e| anyhow::anyhow!("{e}"))?;

  loop {
    if let Some(why) = app.exit.take() {
      tracing::info!("exiting: {why}");
      return Ok(());
    }
    slint::platform::update_timers_and_animations();
    app.drain_ui_actions();
    app.process_deadlines();
    app.maybe_render();
    app.flush();
    let mut timeout = slint::platform::duration_until_next_timer_update();
    if let Some(d) = app.next_deadline() {
      timeout = Some(timeout.map_or(d, |t| t.min(d)));
    }
    event_loop.dispatch(timeout, &mut app)?;
  }
}

/// `SPOOL_PICKER_RENDERER`: `software` (default) or `gpu`, with fallback.
fn choose_renderer(conn: &Connection) -> Backend {
  let want = std::env::var("SPOOL_PICKER_RENDERER").unwrap_or_default();
  match want.trim() {
    "" | "software" => {}
    #[cfg(feature = "gpu")]
    "gpu" => match gpu::GpuWindow::new(conn) {
      Ok(g) => {
        tracing::info!(gl = %g.egl.renderer_name, "renderer: gpu (FemtoVG/EGL)");
        return Backend::Gpu(g);
      }
      Err(e) => tracing::warn!("gpu renderer unavailable, using software: {e:#}"),
    },
    #[cfg(not(feature = "gpu"))]
    "gpu" => tracing::warn!("built without the `gpu` feature; using the software renderer"),
    other => tracing::warn!(renderer = other, "unknown SPOOL_PICKER_RENDERER; using software"),
  }
  let _ = conn;
  tracing::info!("renderer: software");
  Backend::Software(SoftwareFrameRenderer::new())
}

/// inotify on the config dir (kdeglobals is replaced by rename).
fn theme_watch() -> Option<Generic<OwnedFd>> {
  use rustix::fs::inotify;
  let dir = theme::config_dir()?;
  let fd = inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK).ok()?;
  inotify::add_watch(
    &fd,
    &dir,
    inotify::WatchFlags::CLOSE_WRITE | inotify::WatchFlags::MOVED_TO | inotify::WatchFlags::CREATE,
  )
  .ok()?;
  Some(Generic::new(fd, Interest::READ, Mode::Level))
}
