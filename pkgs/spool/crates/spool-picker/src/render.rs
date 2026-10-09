//! Rendering Slint into `wl_shm` canvases.
//!
//! `FrameRenderer` is the seam for the software path: the backend hands it a
//! writable ARGB8888 canvas and gets back the damaged rectangles (Slint's
//! software renderer). `Backend` picks between that and, with the `gpu`
//! feature, FemtoVG over EGL (`crate::gpu`), which owns its buffers and
//! presents through `eglSwapBuffers` instead.

use bytemuck::{Pod, Zeroable};
use slint::platform::software_renderer::{
  MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, TargetPixel,
};

/// One `wl_shm::Format::Argb8888` pixel (little-endian memory order B, G, R,
/// A), premultiplied alpha as Wayland requires.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct Bgra8 {
  pub b: u8,
  pub g: u8,
  pub r: u8,
  pub a: u8,
}

impl TargetPixel for Bgra8 {
  fn blend(&mut self, c: PremultipliedRgbaColor) {
    let inv = (u8::MAX - c.alpha) as u16;
    self.r = (self.r as u16 * inv / 255) as u8 + c.red;
    self.g = (self.g as u16 * inv / 255) as u8 + c.green;
    self.b = (self.b as u16 * inv / 255) as u8 + c.blue;
    self.a = (self.a as u16 + c.alpha as u16 - (self.a as u16 * c.alpha as u16) / 255) as u8;
  }

  fn from_rgb(r: u8, g: u8, b: u8) -> Self {
    Self { r, g, b, a: 255 }
  }

  fn background() -> Self {
    Self { r: 0, g: 0, b: 0, a: 0 }
  }
}

/// A damaged rectangle in buffer pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
  pub x: i32,
  pub y: i32,
  pub w: i32,
  pub h: i32,
}

pub trait FrameRenderer {
  /// Render into `canvas` (`width` pixels per row, ARGB8888). Returns the
  /// damage, or `None` if nothing needed drawing.
  fn render(&mut self, canvas: &mut [u8], width: usize) -> Option<Vec<Rect>>;
  /// The canvases' previous contents are gone (resize/scale change):
  /// the next frames must be full redraws.
  fn invalidate(&mut self);
  fn request_redraw(&self);
}

/// Slint software renderer with two alternating buffers (partial redraw:
/// each frame repaints what changed since that buffer was last drawn).
pub struct SoftwareFrameRenderer {
  pub window: std::rc::Rc<MinimalSoftwareWindow>,
}

impl SoftwareFrameRenderer {
  pub fn new() -> Self {
    Self { window: MinimalSoftwareWindow::new(RepaintBufferType::SwappedBuffers) }
  }
}

impl FrameRenderer for SoftwareFrameRenderer {
  fn render(&mut self, canvas: &mut [u8], width: usize) -> Option<Vec<Rect>> {
    let pixels: &mut [Bgra8] = bytemuck::cast_slice_mut(canvas);
    let mut damage = None;
    self.window.draw_if_needed(|r| {
      let region = r.render(pixels, width);
      damage = Some(
        region
          .iter()
          .map(|(p, s)| Rect { x: p.x, y: p.y, w: s.width as i32, h: s.height as i32 })
          .collect(),
      );
    });
    damage
  }

  fn invalidate(&mut self) {
    // Switching the buffer type drops the renderer's damage history; the
    // callback only runs when a redraw is pending, so request one first.
    self.window.request_redraw();
    self.window.draw_if_needed(|r| {
      r.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
      r.set_repaint_buffer_type(RepaintBufferType::SwappedBuffers);
    });
    self.window.request_redraw();
  }

  fn request_redraw(&self) {
    self.window.request_redraw();
  }
}

/// The renderer chosen at startup.
pub enum Backend {
  Software(SoftwareFrameRenderer),
  #[cfg(feature = "gpu")]
  Gpu(std::rc::Rc<crate::gpu::GpuWindow>),
}

impl Backend {
  pub fn adapter(&self) -> std::rc::Rc<dyn slint::platform::WindowAdapter> {
    match self {
      Backend::Software(s) => s.window.clone(),
      #[cfg(feature = "gpu")]
      Backend::Gpu(g) => g.clone(),
    }
  }

  pub fn is_gpu(&self) -> bool {
    !matches!(self, Backend::Software(_))
  }

  pub fn invalidate(&mut self) {
    match self {
      Backend::Software(s) => s.invalidate(),
      #[cfg(feature = "gpu")]
      Backend::Gpu(g) => slint::platform::WindowAdapter::request_redraw(&**g),
    }
  }

  pub fn request_redraw(&self) {
    match self {
      Backend::Software(s) => s.request_redraw(),
      #[cfg(feature = "gpu")]
      Backend::Gpu(g) => slint::platform::WindowAdapter::request_redraw(&**g),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn bgra_layout_matches_argb8888_le() {
    let p = Bgra8::from_rgb(1, 2, 3);
    let bytes: &[u8] = bytemuck::bytes_of(&p);
    assert_eq!(bytes, &[3, 2, 1, 255]);
    assert_eq!(u32::from_le_bytes(bytes.try_into().unwrap()), 0xFF01_0203);
  }

  #[test]
  fn blend_is_premultiplied_over() {
    let mut p = Bgra8::background();
    p.blend(PremultipliedRgbaColor { red: 50, green: 0, blue: 0, alpha: 128 });
    assert_eq!((p.r, p.a), (50, 128));
    let mut q = Bgra8::from_rgb(200, 200, 200);
    q.blend(PremultipliedRgbaColor { red: 0, green: 0, blue: 0, alpha: 255 });
    assert_eq!((q.r, q.g, q.b, q.a), (0, 0, 0, 255));
  }
}
