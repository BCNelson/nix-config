//! Blocking paster on its own Wayland connection: KWin's
//! `org_kde_kwin_fake_input` or, elsewhere, `zwp_virtual_keyboard_v1`.

use std::fs::File;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_keyboard, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;
use wayland_protocols_plasma::fake_input::client::org_kde_kwin_fake_input::OrgKdeKwinFakeInput;
use xkbcommon::xkb;

use crate::PasteError;
use crate::chord::PasteChord;
use crate::keys::{KeyCodes, default_keymap, keymap_from_string};

/// Interface name of KWin's fake input global.
pub const FAKE_INPUT_GLOBAL: &str = "org_kde_kwin_fake_input";
/// `keyboard_key` needs v4.
const FAKE_INPUT_MIN: u32 = 4;
const FAKE_INPUT_MAX: u32 = 5;
/// Upper bound for a keymap we are willing to read.
const MAX_KEYMAP: u32 = 4 << 20;
/// Interface name of the wlroots/Hyprland virtual keyboard manager.
pub const VIRTUAL_KEYBOARD_GLOBAL: &str = "zwp_virtual_keyboard_manager_v1";

/// Which protocol a [`Paster`] uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteBackend {
  /// KWin's `org_kde_kwin_fake_input` (preferred when advertised, v4+).
  FakeInput,
  /// `zwp_virtual_keyboard_v1` (wlroots compositors, Hyprland).
  VirtualKeyboard,
}

/// Paster options.
#[derive(Debug, Clone)]
pub struct PasteConfig {
  /// Wayland display name or absolute socket path (`None`: `$WAYLAND_DISPLAY`).
  pub display: Option<String>,
  /// Delay between key events, so the target sees distinct, ordered events.
  pub spacing: Duration,
}

impl Default for PasteConfig {
  fn default() -> Self {
    Self { display: None, spacing: Duration::from_millis(8) }
  }
}

#[derive(Default)]
struct State {
  keymap: Option<xkb::Keymap>,
  keymap_serial: u64,
  seat_has_keyboard: bool,
  keyboard: Option<wl_keyboard::WlKeyboard>,
}

/// The virtual keyboard and the keymap it currently has.
struct Vk {
  kb: ZwpVirtualKeyboardV1,
  seat: wl_seat::WlSeat,
  /// Keymap uploaded to `kb` (what the target will interpret our keycodes
  /// with) and its canonical text, for change detection.
  keymap: xkb::Keymap,
  uploaded: String,
  epoch: Instant,
}

enum Backend {
  Fake(OrgKdeKwinFakeInput),
  Vk(Vk),
}

/// Sends paste chords through `org_kde_kwin_fake_input` (KWin) or
/// `zwp_virtual_keyboard_v1` (wlroots, Hyprland). Not `Send` (the xkb keymap
/// is not); use [`PasteHandle`](crate::PasteHandle) from async code.
pub struct Paster {
  conn: Connection,
  queue: EventQueue<State>,
  state: State,
  backend: Backend,
  spacing: Duration,
}

/// Connects to `display` (name relative to `$XDG_RUNTIME_DIR`, absolute
/// socket path, or `None` for `$WAYLAND_DISPLAY`).
pub(crate) fn connect_display(display: Option<&str>) -> Result<Connection, PasteError> {
  connect(display)
}

fn connect(display: Option<&str>) -> Result<Connection, PasteError> {
  let Some(name) = display else {
    return Connection::connect_to_env().map_err(|e| PasteError::Connect(e.to_string()));
  };
  let path = if name.starts_with('/') {
    PathBuf::from(name)
  } else {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
      .ok_or_else(|| PasteError::Connect("XDG_RUNTIME_DIR unset".into()))?;
    PathBuf::from(dir).join(name)
  };
  let stream = UnixStream::connect(&path).map_err(|e| PasteError::Connect(e.to_string()))?;
  Connection::from_socket(stream).map_err(|e| PasteError::Connect(e.to_string()))
}

fn werr(e: impl std::fmt::Display) -> PasteError {
  PasteError::Wayland(e.to_string())
}

fn advertised(globals: &wayland_client::globals::GlobalList, iface: &str) -> Option<u32> {
  globals.contents().with_list(|l| l.iter().find(|g| g.interface == iface).map(|g| g.version))
}

/// Writes `text` (NUL-terminated, as compositors expect) to a sealed-size
/// memfd for `zwp_virtual_keyboard_v1.keymap`.
fn keymap_fd(text: &str) -> Result<(File, u32), PasteError> {
  use rustix::fs::{MemfdFlags, memfd_create};
  let fd = memfd_create("spool-keymap", MemfdFlags::CLOEXEC)
    .map_err(|e| PasteError::Wayland(format!("memfd: {e}")))?;
  let mut f = File::from(fd);
  f.write_all(text.as_bytes())
    .and_then(|()| f.write_all(&[0]))
    .map_err(|e| PasteError::Wayland(format!("write keymap: {e}")))?;
  let size =
    u32::try_from(text.len() + 1).map_err(|_| PasteError::Wayland("keymap too big".into()))?;
  Ok((f, size))
}

impl Paster {
  /// Connects and picks a backend: `org_kde_kwin_fake_input` v4+ when
  /// advertised (KWin; authenticates), else `zwp_virtual_keyboard_manager_v1`
  /// (wlroots/Hyprland), else [`PasteError::Unsupported`]. Fetches the
  /// keymap through `wl_seat.get_keyboard`.
  pub fn connect(cfg: &PasteConfig) -> Result<Self, PasteError> {
    let conn = connect(cfg.display.as_deref())?;
    let (globals, queue) = registry_queue_init::<State>(&conn).map_err(werr)?;

    let fake_version = advertised(&globals, FAKE_INPUT_GLOBAL);
    if let Some(v) = fake_version
      && v >= FAKE_INPUT_MIN
    {
      return Self::connect_fake(conn, globals, queue, cfg);
    }
    if advertised(&globals, VIRTUAL_KEYBOARD_GLOBAL).is_some() {
      return Self::connect_vk(conn, globals, queue, cfg);
    }
    Err(PasteError::Unsupported(match fake_version {
      None => format!("neither {FAKE_INPUT_GLOBAL} nor {VIRTUAL_KEYBOARD_GLOBAL} advertised"),
      Some(v) => format!("{FAKE_INPUT_GLOBAL} v{v} < v{FAKE_INPUT_MIN}"),
    }))
  }

  /// The M5 KWin path (unchanged behaviour).
  fn connect_fake(
    conn: Connection,
    globals: wayland_client::globals::GlobalList,
    mut queue: EventQueue<State>,
    cfg: &PasteConfig,
  ) -> Result<Self, PasteError> {
    let qh = queue.handle();
    let fake: OrgKdeKwinFakeInput =
      globals.bind(&qh, FAKE_INPUT_MIN..=FAKE_INPUT_MAX, ()).map_err(werr)?;
    fake.authenticate("spool".into(), "paste clipboard history item".into());

    let mut state = State::default();
    match globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=7, ()) {
      Ok(seat) => {
        queue.roundtrip(&mut state).map_err(werr)?; // capabilities
        if state.seat_has_keyboard {
          state.keyboard = Some(seat.get_keyboard(&qh, ()));
          queue.roundtrip(&mut state).map_err(werr)?; // keymap
        }
      }
      Err(e) => tracing::debug!(error = %e, "no wl_seat; using fallback keycodes"),
    }
    if state.keymap.is_none() {
      tracing::info!("no keymap from the compositor; paste uses US evdev keycodes");
    }
    Ok(Self { conn, queue, state, backend: Backend::Fake(fake), spacing: cfg.spacing })
  }

  /// wlroots/Hyprland: a virtual keyboard carrying the seat's current keymap
  /// (or the libxkbcommon default, honouring `XKB_DEFAULT_*`, if the seat
  /// has no keyboard yet).
  fn connect_vk(
    conn: Connection,
    globals: wayland_client::globals::GlobalList,
    mut queue: EventQueue<State>,
    cfg: &PasteConfig,
  ) -> Result<Self, PasteError> {
    let qh = queue.handle();
    let seat = globals
      .bind::<wl_seat::WlSeat, _, _>(&qh, 1..=7, ())
      .map_err(|e| PasteError::Unsupported(format!("no wl_seat for the virtual keyboard: {e}")))?;
    let mgr: ZwpVirtualKeyboardManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(werr)?;
    let mut state = State::default();
    queue.roundtrip(&mut state).map_err(werr)?; // capabilities
    fetch_keymap(&seat, &qh, &mut queue, &mut state)?;
    let keymap = match &state.keymap {
      Some(k) => k.clone(),
      None => {
        tracing::info!("no keymap from the compositor; virtual keyboard uses the xkb default");
        default_keymap()
          .ok_or_else(|| PasteError::Unsupported("cannot compile a default keymap".into()))?
      }
    };
    let text = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let kb = mgr.create_virtual_keyboard(&seat, &qh, ());
    let (f, size) = keymap_fd(&text)?;
    kb.keymap(wl_keyboard::KeymapFormat::XkbV1.into(), f.as_fd(), size);
    // A compositor that refuses (`unauthorized`) kills the connection here.
    queue
      .roundtrip(&mut state)
      .map_err(|e| PasteError::Unsupported(format!("virtual keyboard refused: {e}")))?;
    let vk = Vk { kb, seat, keymap, uploaded: text, epoch: Instant::now() };
    Ok(Self { conn, queue, state, backend: Backend::Vk(vk), spacing: cfg.spacing })
  }

  /// The protocol this paster uses.
  pub fn backend(&self) -> PasteBackend {
    match self.backend {
      Backend::Fake(_) => PasteBackend::FakeInput,
      Backend::Vk(_) => PasteBackend::VirtualKeyboard,
    }
  }

  /// Keycodes the next paste would use.
  pub fn keycodes(&self, layout: Option<u32>) -> KeyCodes {
    let keymap = match &self.backend {
      Backend::Fake(_) => self.state.keymap.as_ref(),
      Backend::Vk(vk) => Some(&vk.keymap),
    };
    keymap.map(|m| KeyCodes::resolve(m, layout)).unwrap_or(KeyCodes::FALLBACK)
  }

  /// How many keymaps have been received (tests).
  pub fn keymap_serial(&self) -> u64 {
    self.state.keymap_serial
  }

  /// Processes pending events (keymap changes) with one roundtrip; also a
  /// liveness check of the connection.
  pub fn refresh(&mut self) -> Result<(), PasteError> {
    self.queue.roundtrip(&mut self.state).map(|_| ()).map_err(werr)
  }

  /// Presses `chord` in whatever window has keyboard focus. `layout` is the
  /// active layout index if known. Blocks for ~`spacing * events`.
  ///
  /// The caller is responsible for making sure focus is on the intended
  /// window first (see `spool_kwin::ActiveWindowTracker::wait_for`).
  pub fn paste(&mut self, chord: PasteChord, layout: Option<u32>) -> Result<(), PasteError> {
    self.refresh()?;
    if matches!(self.backend, Backend::Vk(_)) {
      return self.paste_vk(chord, layout);
    }
    let Backend::Fake(fake) = &self.backend else { unreachable!() };
    let keys = self.keycodes(layout);
    let seq = chord.sequence(&keys);
    tracing::debug!(?chord, "sending paste chord");
    for (i, e) in seq.iter().enumerate() {
      if i > 0 {
        std::thread::sleep(self.spacing);
      }
      // wl_keyboard::KeyState: 0 released, 1 pressed.
      fake.keyboard_key(e.code, u32::from(e.pressed));
      self.conn.flush().map_err(werr)?;
    }
    // Make sure the compositor processed everything before returning.
    self.refresh()
  }

  /// Virtual-keyboard paste: re-reads the seat keymap (the user may have
  /// changed layouts), re-uploads it if it changed, then sends key events
  /// plus explicit modifier state computed with xkb, in group `layout`.
  fn paste_vk(&mut self, chord: PasteChord, layout: Option<u32>) -> Result<(), PasteError> {
    let qh = self.queue.handle();
    let Backend::Vk(vk) = &mut self.backend else { unreachable!() };
    fetch_keymap(&vk.seat, &qh, &mut self.queue, &mut self.state)?;
    // Compare xkbcommon's canonical serialisations: the compositor may echo
    // our own upload back reformatted.
    if let Some(k) = &self.state.keymap
      && k.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1) != vk.uploaded
    {
      let text = k.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
      let (f, size) = keymap_fd(&text)?;
      vk.kb.keymap(wl_keyboard::KeymapFormat::XkbV1.into(), f.as_fd(), size);
      vk.keymap = k.clone();
      vk.uploaded = text;
      tracing::debug!("virtual keyboard keymap updated");
    }
    let group = layout.filter(|&l| l < vk.keymap.num_layouts()).unwrap_or(0);
    let keys = KeyCodes::resolve(&vk.keymap, Some(group));
    let seq = chord.sequence(&keys);
    let mut xs = xkb::State::new(&vk.keymap);
    let send_mods = |xs: &xkb::State, kb: &ZwpVirtualKeyboardV1| {
      kb.modifiers(
        xs.serialize_mods(xkb::STATE_MODS_DEPRESSED),
        xs.serialize_mods(xkb::STATE_MODS_LATCHED),
        xs.serialize_mods(xkb::STATE_MODS_LOCKED),
        group,
      );
    };
    tracing::debug!(?chord, "sending paste chord (virtual keyboard)");
    send_mods(&xs, &vk.kb);
    for (i, e) in seq.iter().enumerate() {
      if i > 0 {
        std::thread::sleep(self.spacing);
      }
      let time = u32::try_from(vk.epoch.elapsed().as_millis() & 0xffff_ffff).unwrap_or(0);
      vk.kb.key(time, e.code, u32::from(e.pressed));
      let dir = if e.pressed { xkb::KeyDirection::Down } else { xkb::KeyDirection::Up };
      xs.update_key(xkb::Keycode::new(e.code + 8), dir);
      send_mods(&xs, &vk.kb);
      self.conn.flush().map_err(werr)?;
    }
    self.refresh()
  }
}

/// `wl_seat.get_keyboard` + roundtrip: the compositor sends the current
/// keymap to every new `wl_keyboard`. The keyboard is released right away
/// (virtual-keyboard path only; the fake-input path keeps its keyboard).
fn fetch_keymap(
  seat: &wl_seat::WlSeat,
  qh: &QueueHandle<State>,
  queue: &mut EventQueue<State>,
  state: &mut State,
) -> Result<(), PasteError> {
  if !state.seat_has_keyboard {
    return Ok(());
  }
  let kb = seat.get_keyboard(qh, ());
  let res = queue.roundtrip(state).map(|_| ()).map_err(werr);
  if kb.version() >= 3 {
    kb.release();
  }
  res
}

impl Drop for Paster {
  fn drop(&mut self) {
    if let Some(kb) = self.state.keyboard.take()
      && kb.version() >= 3
    {
      kb.release();
    }
    match &self.backend {
      Backend::Fake(fake) => {
        if fake.version() >= 5 {
          fake.destroy();
        }
      }
      Backend::Vk(vk) => {
        vk.kb.destroy();
        if vk.seat.version() >= 5 {
          vk.seat.release();
        }
      }
    }
    let _ = self.conn.flush();
  }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
  fn event(
    _: &mut Self,
    _: &wl_registry::WlRegistry,
    _: wl_registry::Event,
    _: &GlobalListContents,
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
  }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
  fn event(
    state: &mut Self,
    _: &wl_seat::WlSeat,
    event: wl_seat::Event,
    _: &(),
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
    if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event {
      state.seat_has_keyboard = caps.contains(wl_seat::Capability::Keyboard);
    }
  }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
  fn event(
    state: &mut Self,
    _: &wl_keyboard::WlKeyboard,
    event: wl_keyboard::Event,
    _: &(),
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
    let wl_keyboard::Event::Keymap { format, fd, size } = event else {
      return;
    };
    if format != WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) || size == 0 || size > MAX_KEYMAP {
      tracing::warn!(size, "ignoring unusable keymap");
      return;
    }
    // pread from offset 0: the fd may be shared/sealed (v7+ must not be
    // mapped MAP_SHARED), and reading does not need a mapping at all.
    let file = File::from(fd);
    let mut buf = vec![0u8; size as usize];
    let mut off = 0usize;
    while off < buf.len() {
      match file.read_at(&mut buf[off..], off as u64) {
        Ok(0) => break,
        Ok(n) => off += n,
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
        Err(e) => {
          tracing::warn!(error = %e, "reading keymap failed");
          return;
        }
      }
    }
    buf.truncate(off);
    let Ok(text) = String::from_utf8(buf) else {
      tracing::warn!("keymap is not UTF-8");
      return;
    };
    match keymap_from_string(text) {
      Some(k) => {
        state.keymap = Some(k);
        state.keymap_serial += 1;
        tracing::debug!(serial = state.keymap_serial, "keymap updated");
      }
      None => tracing::warn!("keymap failed to compile"),
    }
  }
}

impl Dispatch<OrgKdeKwinFakeInput, ()> for State {
  fn event(
    _: &mut Self,
    _: &OrgKdeKwinFakeInput,
    _: <OrgKdeKwinFakeInput as Proxy>::Event,
    _: &(),
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
  }
}

impl Dispatch<ZwpVirtualKeyboardManagerV1, ()> for State {
  fn event(
    _: &mut Self,
    _: &ZwpVirtualKeyboardManagerV1,
    _: <ZwpVirtualKeyboardManagerV1 as Proxy>::Event,
    _: &(),
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
  }
}

impl Dispatch<ZwpVirtualKeyboardV1, ()> for State {
  fn event(
    _: &mut Self,
    _: &ZwpVirtualKeyboardV1,
    _: <ZwpVirtualKeyboardV1 as Proxy>::Event,
    _: &(),
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
  }
}
