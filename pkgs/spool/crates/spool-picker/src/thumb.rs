//! Thumbnail decoding inside the picker, with hard limits. Only PNG, JPEG and
//! WebP decoders are compiled in (see Cargo.toml `image` features).

use std::io::Cursor;

use image::{ImageFormat, ImageReader, Limits, RgbaImage};

/// Largest encoded thumbnail we accept from the daemon.
pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
/// Largest decoded width/height.
pub const MAX_DIM: u32 = 4096;
/// Largest single allocation the decoder may make (4096² RGBA = 64 MiB).
pub const MAX_ALLOC: u64 = 64 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum ThumbError {
  Empty,
  TooLarge(usize),
  Format,
  Limits,
  Decode,
}

impl std::fmt::Display for ThumbError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      ThumbError::Empty => f.write_str("empty"),
      ThumbError::TooLarge(n) => write!(f, "input too large ({n} bytes)"),
      ThumbError::Format => f.write_str("unsupported format"),
      ThumbError::Limits => f.write_str("image exceeds decode limits"),
      ThumbError::Decode => f.write_str("decode error"),
    }
  }
}

fn limits() -> Limits {
  let mut l = Limits::default();
  l.max_image_width = Some(MAX_DIM);
  l.max_image_height = Some(MAX_DIM);
  l.max_alloc = Some(MAX_ALLOC);
  l
}

/// Decode `bytes` and scale it to fit in `box_w`×`box_h` physical pixels
/// (aspect preserved, never upscaled).
pub fn decode_thumbnail(bytes: &[u8], box_w: u32, box_h: u32) -> Result<RgbaImage, ThumbError> {
  if bytes.is_empty() {
    return Err(ThumbError::Empty);
  }
  if bytes.len() > MAX_INPUT_BYTES {
    return Err(ThumbError::TooLarge(bytes.len()));
  }
  let mut reader =
    ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|_| ThumbError::Format)?;
  match reader.format() {
    Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP) => {}
    _ => return Err(ThumbError::Format),
  }
  reader.limits(limits());
  let img = reader.decode().map_err(|e| match e {
    image::ImageError::Limits(_) => ThumbError::Limits,
    image::ImageError::Unsupported(_) => ThumbError::Format,
    _ => ThumbError::Decode,
  })?;
  let (w, h) = (img.width(), img.height());
  if w == 0 || h == 0 {
    return Err(ThumbError::Decode);
  }
  let img = if w > box_w || h > box_h { img.thumbnail(box_w.max(1), box_h.max(1)) } else { img };
  Ok(img.into_rgba8())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
      crc ^= b as u32;
      for _ in 0..8 {
        crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
      }
    }
    !crc
  }

  fn chunk(out: &mut Vec<u8>, ty: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut c = ty.to_vec();
    c.extend_from_slice(data);
    out.extend_from_slice(&c);
    out.extend_from_slice(&crc32(&c).to_be_bytes());
  }

  /// A PNG with an arbitrary IHDR size; the IDAT is a valid zlib stream of
  /// `rows` zero rows (stored blocks), so the file stays tiny.
  fn png(width: u32, height: u32) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    chunk(&mut out, b"IHDR", &ihdr);
    // Raw scanlines for small images; for huge ones a short stream (the
    // decoder must refuse on the header alone).
    let row = 1 + width as usize * 3;
    let raw = if (width as u64 * height as u64) <= 64 * 64 {
      vec![0u8; row * height as usize]
    } else {
      vec![0u8; 1024]
    };
    let mut z = vec![0x78, 0x01];
    for (i, block) in raw.chunks(65535).enumerate() {
      let last = (i + 1) * 65535 >= raw.len();
      z.push(last as u8);
      z.extend_from_slice(&(block.len() as u16).to_le_bytes());
      z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
      z.extend_from_slice(block);
    }
    let adler = {
      let (mut a, mut b) = (1u32, 0u32);
      for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
      }
      (b << 16) | a
    };
    z.extend_from_slice(&adler.to_be_bytes());
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
  }

  #[test]
  fn decodes_and_scales_small_png() {
    let img = decode_thumbnail(&png(64, 32), 16, 16).expect("decode");
    assert_eq!((img.width(), img.height()), (16, 8));
    // Never upscales.
    let img = decode_thumbnail(&png(4, 4), 100, 100).expect("decode");
    assert_eq!((img.width(), img.height()), (4, 4));
  }

  #[test]
  fn rejects_huge_dimensions_without_allocating() {
    // "Zip bomb": tiny file, header claims 60000×60000 (10 GiB of RGB).
    let bomb = png(60_000, 60_000);
    assert!(bomb.len() < 4096);
    assert_eq!(decode_thumbnail(&bomb, 64, 64), Err(ThumbError::Limits));
    // Just over the per-axis limit.
    assert_eq!(decode_thumbnail(&png(MAX_DIM + 1, 1), 64, 64), Err(ThumbError::Limits));
    assert_eq!(decode_thumbnail(&png(1, MAX_DIM + 1), 64, 64), Err(ThumbError::Limits));
  }

  #[test]
  fn rejects_oversized_input_and_other_formats() {
    let big = vec![0u8; MAX_INPUT_BYTES + 1];
    assert_eq!(decode_thumbnail(&big, 64, 64), Err(ThumbError::TooLarge(MAX_INPUT_BYTES + 1)));
    assert_eq!(decode_thumbnail(&[], 64, 64), Err(ThumbError::Empty));
    assert_eq!(decode_thumbnail(b"GIF89a\x01\x00\x01\x00", 64, 64), Err(ThumbError::Format));
    assert_eq!(decode_thumbnail(b"BM\x00\x00\x00\x00", 64, 64), Err(ThumbError::Format));
    assert_eq!(decode_thumbnail(b"<svg xmlns='x'/>", 64, 64), Err(ThumbError::Format));
  }

  #[test]
  fn truncated_png_is_a_decode_error() {
    let mut p = png(32, 32);
    p.truncate(p.len() - 30);
    assert_eq!(decode_thumbnail(&p, 64, 64), Err(ThumbError::Decode));
  }
}
