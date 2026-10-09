//! FemtoVG-over-EGL renderer (cargo feature `gpu`, runtime opt-in with
//! `SPOOL_PICKER_RENDERER=gpu`).
//!
//! * One EGL display + GLES context for the life of the process. The
//!   context is made current *surfaceless* at start (FemtoVG compiles its
//!   shaders then), and an `EGLSurface` over a `wl_egl_window` is attached
//!   to the layer surface's `wl_surface`. Both survive unmap/remap (Hide just
//!   attaches a null buffer; the next `eglSwapBuffers` maps again); they are
//!   only recreated when the layer surface is (output switch).
//! * `eglSwapBuffers` attaches, damages and commits; the app puts its
//!   viewport/frame-callback/presentation-feedback requests on the
//!   `wl_surface` first, so they land in the same commit. Swap interval 0:
//!   the app throttles on its own frame callbacks and the UI thread never
//!   blocks in EGL.
//! * No pre-render while hidden: drawing would need an offscreen FBO plus a
//!   blit on Show, and FemtoVG's Slint backend always targets the default
//!   framebuffer. The first frame after Show is rendered on the configure
//!   (a GPU frame of this UI costs ~1-2 ms on real hardware).
//! * Any failure during init (no `libEGL.so.1`, no Wayland EGL platform, no
//!   `EGL_KHR_surfaceless_context`, context creation, FemtoVG setup) or a
//!   software GL implementation (llvmpipe/softpipe/swrast: slower than
//!   Slint's software renderer *and* it JITs, which `MemoryDenyWriteExecute`
//!   forbids) returns `Err` and the caller falls back to software.
//!   `SPOOL_PICKER_GPU_ALLOW_SOFTWARE=1` accepts software GL (tests only).

#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_void};
use std::num::NonZeroU32;
use std::rc::{Rc, Weak};

use anyhow::{Context as _, anyhow, bail};
use khronos_egl as egl;
use slint::PhysicalSize;
use slint::platform::femtovg_renderer::{FemtoVGRenderer, OpenGLInterface};
use slint::platform::{Renderer, WindowAdapter};
use smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface;
use smithay_client_toolkit::reexports::client::{Connection, Proxy};
use wayland_egl::WlEglSurface;

type Egl = egl::DynamicInstance<egl::EGL1_5>;
type BoxErr = Box<dyn std::error::Error + Send + Sync>;

const PLATFORM_WAYLAND_KHR: egl::Enum = 0x31D8;
const GL_RENDERER: u32 = 0x1F01;
const GL_VERSION: u32 = 0x1F02;

struct Attached {
  /// Must be destroyed before `wl_egl` and before the `wl_surface`.
  egl_surface: egl::Surface,
  wl_egl: WlEglSurface,
  surface: WlSurface,
}

/// EGL state shared by the OpenGL interface (owned by FemtoVG) and the app.
pub struct EglState {
  egl: Egl,
  display: egl::Display,
  config: egl::Config,
  context: egl::Context,
  attached: RefCell<Option<Attached>>,
  size: Cell<(i32, i32)>,
  pub renderer_name: String,
}

impl EglState {
  fn new(conn: &Connection) -> anyhow::Result<Self> {
    // SAFETY: loading the system libEGL; the trait bound checks it exports
    // the EGL 1.5 entry points.
    let egl = unsafe { Egl::load_required() }.map_err(|e| anyhow!("load libEGL.so.1: {e}"))?;
    let display_ptr = conn.backend().display_ptr() as *mut c_void;
    if display_ptr.is_null() {
      bail!("no libwayland-client display pointer");
    }
    // SAFETY: a live wl_display* from the same libwayland-client this
    // process uses (wayland-backend `client_system`).
    let display =
      unsafe { egl.get_platform_display(PLATFORM_WAYLAND_KHR, display_ptr, &[egl::ATTRIB_NONE]) }
        .context("eglGetPlatformDisplay(wayland)")?;
    egl.initialize(display).context("eglInitialize")?;
    let exts = egl.query_string(Some(display), egl::EXTENSIONS).context("EGL extensions")?;
    if !exts.to_string_lossy().split(' ').any(|e| e == "EGL_KHR_surfaceless_context") {
      let _ = egl.terminate(display);
      bail!("EGL_KHR_surfaceless_context missing");
    }
    egl.bind_api(egl::OPENGL_ES_API).context("eglBindAPI(GLES)")?;
    let attrs = [
      egl::SURFACE_TYPE,
      egl::WINDOW_BIT,
      egl::RENDERABLE_TYPE,
      egl::OPENGL_ES2_BIT,
      egl::RED_SIZE,
      8,
      egl::GREEN_SIZE,
      8,
      egl::BLUE_SIZE,
      8,
      egl::ALPHA_SIZE,
      8,
      // FemtoVG fills paths through the stencil buffer.
      egl::STENCIL_SIZE,
      8,
      egl::NONE,
    ];
    let config = egl
      .choose_first_config(display, &attrs)
      .context("eglChooseConfig")?
      .ok_or_else(|| anyhow!("no RGBA8 + stencil GLES config"))?;
    let context = [3, 2]
      .into_iter()
      .find_map(|major| {
        egl
          .create_context(display, config, None, &[egl::CONTEXT_MAJOR_VERSION, major, egl::NONE])
          .ok()
      })
      .ok_or_else(|| anyhow!("eglCreateContext failed for GLES 3 and 2"))?;
    egl.make_current(display, None, None, Some(context)).context("eglMakeCurrent(surfaceless)")?;
    let mut state = EglState {
      egl,
      display,
      config,
      context,
      attached: RefCell::new(None),
      size: Cell::new((1, 1)),
      renderer_name: String::new(),
    };
    state.renderer_name = format!(
      "{} ({})",
      state.gl_string(GL_RENDERER).unwrap_or_default(),
      state.gl_string(GL_VERSION).unwrap_or_default()
    );
    Ok(state)
  }

  fn gl_string(&self, name: u32) -> Option<String> {
    let f = self.egl.get_proc_address("glGetString")?;
    // SAFETY: glGetString's C signature; the context is current.
    let get: extern "system" fn(u32) -> *const std::ffi::c_char = unsafe { std::mem::transmute(f) };
    let p = get(name);
    if p.is_null() {
      return None;
    }
    // SAFETY: GL returns a static NUL-terminated string.
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
  }

  fn is_software(&self) -> bool {
    let n = self.renderer_name.to_ascii_lowercase();
    ["llvmpipe", "softpipe", "swrast", "software"].iter().any(|s| n.contains(s))
  }

  /// Bind (or rebind) to `surface` with a `w`×`h` buffer.
  pub fn attach(&self, surface: &WlSurface) -> anyhow::Result<()> {
    if self.attached.borrow().as_ref().is_some_and(|a| &a.surface == surface) {
      return Ok(());
    }
    self.detach();
    let (w, h) = self.size.get();
    let wl_egl = WlEglSurface::new(surface.id(), w.max(1), h.max(1))
      .map_err(|e| anyhow!("wl_egl_window_create: {e}"))?;
    // SAFETY: `wl_egl` is a live wl_egl_window* of the display's platform.
    let egl_surface = unsafe {
      self.egl.create_window_surface(self.display, self.config, wl_egl.ptr() as *mut c_void, None)
    }
    .context("eglCreateWindowSurface")?;
    self
      .egl
      .make_current(self.display, Some(egl_surface), Some(egl_surface), Some(self.context))
      .context("eglMakeCurrent(window)")?;
    // The app throttles on frame callbacks; never block in eglSwapBuffers.
    let _ = self.egl.swap_interval(self.display, 0);
    *self.attached.borrow_mut() = Some(Attached { egl_surface, wl_egl, surface: surface.clone() });
    Ok(())
  }

  /// Drop the EGL surface + wl_egl_window (before the wl_surface goes).
  pub fn detach(&self) {
    if let Some(a) = self.attached.borrow_mut().take() {
      let _ = self.egl.make_current(self.display, None, None, Some(self.context));
      let _ = self.egl.destroy_surface(self.display, a.egl_surface);
      drop(a.wl_egl);
    }
  }

  pub fn is_attached(&self) -> bool {
    self.attached.borrow().is_some()
  }

  fn current(&self) -> Result<(), BoxErr> {
    let a = self.attached.borrow();
    let s = a.as_ref().map(|a| a.egl_surface);
    self.egl.make_current(self.display, s, s, Some(self.context)).map_err(|e| e.into())
  }
}

impl Drop for EglState {
  fn drop(&mut self) {
    self.detach();
    let _ = self.egl.make_current(self.display, None, None, None);
    let _ = self.egl.destroy_context(self.display, self.context);
    let _ = self.egl.terminate(self.display);
  }
}

/// What FemtoVG holds.
struct GlIface(Rc<EglState>);

// SAFETY: get_proc_address forwards to eglGetProcAddress of the loaded
// libEGL, which stays loaded as long as `EglState` (kept alive by this Rc).
unsafe impl OpenGLInterface for GlIface {
  fn ensure_current(&self) -> Result<(), BoxErr> {
    self.0.current()
  }

  fn swap_buffers(&self) -> Result<(), BoxErr> {
    let a = self.0.attached.borrow();
    let a = a.as_ref().ok_or("no EGL surface attached")?;
    self.0.egl.swap_buffers(self.0.display, a.egl_surface).map_err(|e| e.into())
  }

  fn resize(&self, width: NonZeroU32, height: NonZeroU32) -> Result<(), BoxErr> {
    let (w, h) = (width.get() as i32, height.get() as i32);
    self.0.size.set((w, h));
    if let Some(a) = self.0.attached.borrow().as_ref() {
      a.wl_egl.resize(w, h, 0, 0);
    }
    Ok(())
  }

  fn get_proc_address(&self, name: &CStr) -> *const c_void {
    name
      .to_str()
      .ok()
      .and_then(|n| self.0.egl.get_proc_address(n))
      .map_or(std::ptr::null(), |f| f as *const c_void)
  }
}

/// Slint window adapter backed by FemtoVG.
pub struct GpuWindow {
  window: slint::Window,
  renderer: FemtoVGRenderer,
  size: Cell<PhysicalSize>,
  needs_redraw: Cell<bool>,
  pub egl: Rc<EglState>,
}

impl GpuWindow {
  /// Set up EGL + FemtoVG, or explain why not (the caller falls back).
  pub fn new(conn: &Connection) -> anyhow::Result<Rc<Self>> {
    let egl = Rc::new(EglState::new(conn)?);
    let allow_sw = matches!(
      std::env::var("SPOOL_PICKER_GPU_ALLOW_SOFTWARE").as_deref(),
      Ok("1" | "true" | "yes")
    );
    if egl.is_software() && !allow_sw {
      bail!(
        "GL implementation is software ({}); Slint's software renderer is faster",
        egl.renderer_name
      );
    }
    let renderer =
      FemtoVGRenderer::new(GlIface(egl.clone())).map_err(|e| anyhow!("FemtoVG init: {e}"))?;
    Ok(Rc::new_cyclic(|w: &Weak<GpuWindow>| GpuWindow {
      window: slint::Window::new(w.clone() as Weak<dyn WindowAdapter>),
      renderer,
      size: Cell::new(PhysicalSize::new(0, 0)),
      needs_redraw: Cell::new(true),
      egl,
    }))
  }

  pub fn needs_redraw(&self) -> bool {
    self.needs_redraw.get()
  }

  /// Render + swap (one commit) if Slint asked for a frame. Needs an
  /// attached surface.
  pub fn draw_if_needed(&self) -> Result<bool, slint::PlatformError> {
    if !self.needs_redraw.get() || !self.egl.is_attached() {
      return Ok(false);
    }
    self.needs_redraw.set(false);
    self.renderer.render()?;
    Ok(true)
  }
}

impl WindowAdapter for GpuWindow {
  fn window(&self) -> &slint::Window {
    &self.window
  }

  fn renderer(&self) -> &dyn Renderer {
    &self.renderer
  }

  fn size(&self) -> PhysicalSize {
    self.size.get()
  }

  fn set_size(&self, size: slint::WindowSize) {
    let sf = self.window.scale_factor();
    self.size.set(size.to_physical(sf));
    self.window.dispatch_event(slint::platform::WindowEvent::Resized { size: size.to_logical(sf) });
  }

  fn request_redraw(&self) {
    self.needs_redraw.set(true);
  }
}
