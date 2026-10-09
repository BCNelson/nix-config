//! Chunked ChaCha20-Poly1305 format for write-once files with random access.
//!
//! ```text
//! header (44 bytes):
//!   magic "SPLCRY01" (8) | version u8 = 1 | chunk_size_log2 u8 | reserved u16 = 0 | salt [u8; 32]
//! chunks:
//!   ciphertext || 16-byte Poly1305 tag
//!   every chunk except the last holds exactly chunk_size plaintext bytes;
//!   the last holds 1..=chunk_size, or 0 only when the whole plaintext is
//!   empty (an empty file is exactly one empty final chunk).
//! ```
//!
//! - File key = HKDF-SHA256(ikm = sub-key, salt = header salt, info =
//!   `spool/file/v1`). A fresh random salt per file makes counter nonces safe.
//! - Nonce (12 bytes) = chunk index as u64 big-endian || last flag (1 for the
//!   final chunk, else 0) || 3 zero bytes.
//! - AAD = the 44 header bytes || caller-supplied logical path.
//!
//! So swapping files between paths, editing the header, reordering, dropping,
//! duplicating or appending chunks, and truncation (the final flag goes
//! missing) all fail authentication. The plaintext length follows from the
//! file length ([`plaintext_len`]); [`ChunkedReader::open`] authenticates the
//! final chunk up front, so `len()` is trustworthy before any data is read.
//! Reading any other chunk authenticates that chunk only.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use chacha20poly1305::aead::{AeadInOut, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, Tag};
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::key::{SubKey, hkdf32, os_random};
use crate::mlock::Secret32;

/// File magic.
pub const MAGIC: [u8; 8] = *b"SPLCRY01";
/// Format version written to and accepted from the header.
pub const VERSION: u8 = 1;
/// Header length in bytes.
pub const HEADER_LEN: usize = 8 + 1 + 1 + 2 + SALT_LEN;
/// Poly1305 tag length.
pub const TAG_LEN: usize = 16;
const SALT_LEN: usize = 32;
/// HKDF info for per-file keys.
pub const FILE_KEY_INFO: &[u8] = b"spool/file/v1";
/// Default chunk size: 64 KiB.
pub const DEFAULT_CHUNK_SIZE_LOG2: u8 = 16;
/// Smallest allowed chunk size: 4 KiB.
pub const MIN_CHUNK_SIZE_LOG2: u8 = 12;
/// Largest allowed chunk size: 1 MiB.
pub const MAX_CHUNK_SIZE_LOG2: u8 = 20;

fn check_log2(log2: u8) -> Result<usize> {
  if (MIN_CHUNK_SIZE_LOG2..=MAX_CHUNK_SIZE_LOG2).contains(&log2) {
    Ok(1usize << log2)
  } else {
    Err(Error::ChunkSize(log2))
  }
}

fn nonce(index: u64, last: bool) -> Nonce {
  let mut n = [0u8; 12];
  n[..8].copy_from_slice(&index.to_be_bytes());
  n[8] = u8::from(last);
  Nonce::from(n)
}

/// Plaintext length of a chunked file of `file_len` bytes (header included)
/// with `chunk_size` plaintext bytes per chunk, or [`Error::InvalidLength`]
/// if no plaintext encrypts to exactly that length.
pub fn plaintext_len(file_len: u64, chunk_size: u64) -> Result<u64> {
  let bad = Error::InvalidLength(file_len);
  if chunk_size == 0 {
    return Err(bad);
  }
  let body = file_len.checked_sub(HEADER_LEN as u64).ok_or(Error::InvalidLength(file_len))?;
  let full = chunk_size.checked_add(TAG_LEN as u64).ok_or(Error::InvalidLength(file_len))?;
  let (q, r) = (body / full, body % full);
  match r {
    // Exactly q full chunks; the last one is full. q == 0 means no chunks at
    // all, which is impossible (an empty file still has its final chunk).
    0 if q > 0 => Ok(q * chunk_size),
    0 => Err(bad),
    // An empty final chunk after full chunks is never written (a plaintext
    // of k * chunk_size ends in a full final chunk), so it is rejected: every
    // plaintext length has exactly one valid file length and vice versa.
    r if r == TAG_LEN as u64 && q > 0 => Err(bad),
    // A short final chunk needs at least its tag.
    r if r > TAG_LEN as u64 || q == 0 && r == TAG_LEN as u64 => {
      Ok(q * chunk_size + (r - TAG_LEN as u64))
    }
    _ => Err(bad),
  }
}

/// Build the AAD: header || logical path.
fn aad(header: &[u8; HEADER_LEN], logical_path: &[u8]) -> Vec<u8> {
  let mut a = Vec::with_capacity(HEADER_LEN + logical_path.len());
  a.extend_from_slice(header);
  a.extend_from_slice(logical_path);
  a
}

fn cipher(key: &Secret32) -> ChaCha20Poly1305 {
  // Built per chunk so the expanded key never outlives the call; the cipher
  // zeroizes its key copy on drop (`zeroize` feature).
  ChaCha20Poly1305::new(&Key::from(*key.bytes()))
}

/// Streaming writer for the chunked format.
///
/// Writes the header immediately; buffers up to one chunk of plaintext
/// (zeroized), and seals a chunk only once it knows more data follows. The
/// final chunk is sealed by [`ChunkedWriter::finish`]. Dropping the writer
/// without `finish` leaves a file with no final chunk, which never
/// authenticates.
///
/// The writer is not atomic: write to a temporary file and rename (as
/// [`seal_file`] does) if readers may see the path.
pub struct ChunkedWriter<W: Write> {
  out: W,
  key: Secret32,
  aad: Vec<u8>,
  chunk_size: usize,
  buf: Zeroizing<Vec<u8>>,
  index: u64,
  /// Set after an I/O error; the stream position is unknown, so refuse more.
  poisoned: bool,
}

impl<W: Write> ChunkedWriter<W> {
  /// Start a file under `sub` (normally the [`Label::Blob`](crate::Label) or
  /// `Index` sub-key) for `logical_path`, which readers must present
  /// identically. Writes the header to `out`.
  ///
  /// Errors: `InvalidInput` for a chunk size outside 12..=20, or the I/O
  /// error from writing the header.
  pub fn new(sub: &SubKey, logical_path: &[u8], chunk_size_log2: u8, out: W) -> io::Result<Self> {
    let mut salt = [0u8; SALT_LEN];
    os_random(&mut salt);
    Self::with_salt(sub, logical_path, chunk_size_log2, salt, out)
  }

  /// Deterministic constructor for test vectors only.
  pub(crate) fn with_salt(
    sub: &SubKey,
    logical_path: &[u8],
    chunk_size_log2: u8,
    salt: [u8; SALT_LEN],
    mut out: W,
  ) -> io::Result<Self> {
    let chunk_size =
      check_log2(chunk_size_log2).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let mut header = [0u8; HEADER_LEN];
    header[..8].copy_from_slice(&MAGIC);
    header[8] = VERSION;
    header[9] = chunk_size_log2;
    // header[10..12]: reserved, zero.
    header[12..].copy_from_slice(&salt);
    out.write_all(&header)?;
    Ok(ChunkedWriter {
      out,
      key: hkdf32(sub.expose(), Some(&salt), FILE_KEY_INFO),
      aad: aad(&header, logical_path),
      chunk_size,
      buf: Zeroizing::new(Vec::with_capacity(chunk_size + TAG_LEN)),
      index: 0,
      poisoned: false,
    })
  }

  fn check(&self) -> io::Result<()> {
    if self.poisoned {
      Err(io::Error::other("ChunkedWriter failed earlier; output is incomplete"))
    } else {
      Ok(())
    }
  }

  /// Encrypt `buf` in place as chunk `self.index` and write it out.
  fn seal_chunk(&mut self, last: bool) -> io::Result<()> {
    let n = nonce(self.index, last);
    let tag = cipher(&self.key)
      .encrypt_inout_detached(&n, &self.aad, self.buf.as_mut_slice().into())
      .map_err(|_| io::Error::other("chunk encryption failed"))?;
    self.buf.extend_from_slice(&tag);
    let res = self.out.write_all(&self.buf);
    self.buf.clear();
    if res.is_err() {
      self.poisoned = true;
    }
    res?;
    self.index =
      self.index.checked_add(1).ok_or_else(|| io::Error::other("chunk index overflow"))?;
    Ok(())
  }

  /// Seal the final chunk (possibly empty), flush, and return the inner
  /// writer. The caller is responsible for `fsync` if needed.
  pub fn finish(mut self) -> io::Result<W> {
    self.check()?;
    self.seal_chunk(true)?;
    self.out.flush()?;
    let ChunkedWriter { out, .. } = self;
    Ok(out)
  }
}

impl<W: Write> Write for ChunkedWriter<W> {
  fn write(&mut self, data: &[u8]) -> io::Result<usize> {
    self.check()?;
    let mut rest = data;
    while !rest.is_empty() {
      // Only seal a full buffer once more data is known to follow, so a
      // plaintext of exactly k * chunk_size ends in a full *final* chunk.
      if self.buf.len() == self.chunk_size {
        self.seal_chunk(false)?;
      }
      let take = (self.chunk_size - self.buf.len()).min(rest.len());
      self.buf.extend_from_slice(&rest[..take]);
      rest = &rest[take..];
    }
    Ok(data.len())
  }

  /// Flushes the inner writer. Does not seal the buffered partial chunk.
  fn flush(&mut self) -> io::Result<()> {
    self.check()?;
    self.out.flush()
  }
}

/// Random-access reader for the chunked format.
pub struct ChunkedReader<R: Read + Seek> {
  inner: R,
  key: Secret32,
  aad: Vec<u8>,
  chunk_size: usize,
  plaintext_len: u64,
  chunk_count: u64,
}

impl<R: Read + Seek> ChunkedReader<R> {
  /// Parse and check the header, derive the file key, compute the
  /// plaintext length from the file length, and authenticate the final
  /// chunk (so truncation / extension is detected here).
  pub fn open(sub: &SubKey, logical_path: &[u8], mut inner: R) -> Result<Self> {
    let file_len = inner.seek(SeekFrom::End(0))?;
    inner.seek(SeekFrom::Start(0))?;
    let mut header = [0u8; HEADER_LEN];
    if file_len < HEADER_LEN as u64 {
      return Err(Error::InvalidLength(file_len));
    }
    inner.read_exact(&mut header)?;
    if header[..8] != MAGIC {
      return Err(Error::BadHeader("bad magic"));
    }
    if header[8] != VERSION {
      return Err(Error::BadHeader("unsupported version"));
    }
    if header[10..12] != [0, 0] {
      return Err(Error::BadHeader("reserved bits set"));
    }
    let chunk_size = check_log2(header[9]).map_err(|_| Error::BadHeader("bad chunk size"))?;
    let plaintext_len = plaintext_len(file_len, chunk_size as u64)?;
    // At least one chunk; the last may be empty or full.
    let chunk_count = plaintext_len.div_ceil(chunk_size as u64).max(1);
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&header[12..]);
    let mut r = ChunkedReader {
      inner,
      key: hkdf32(sub.expose(), Some(&salt), FILE_KEY_INFO),
      aad: aad(&header, logical_path),
      chunk_size,
      plaintext_len,
      chunk_count,
    };
    r.read_chunk(chunk_count - 1)?;
    Ok(r)
  }

  /// Plaintext length in bytes (authenticated by `open`).
  pub fn len(&self) -> u64 {
    self.plaintext_len
  }

  pub fn is_empty(&self) -> bool {
    self.plaintext_len == 0
  }

  /// Number of chunks (at least 1).
  pub fn chunk_count(&self) -> u64 {
    self.chunk_count
  }

  /// Plaintext bytes per chunk.
  pub fn chunk_size(&self) -> usize {
    self.chunk_size
  }

  /// Decrypt and authenticate chunk `index`.
  pub fn read_chunk(&mut self, index: u64) -> Result<Zeroizing<Vec<u8>>> {
    if index >= self.chunk_count {
      return Err(Error::OutOfRange);
    }
    let last = index == self.chunk_count - 1;
    let cs = self.chunk_size as u64;
    let pt_len = if last { self.plaintext_len - index * cs } else { cs } as usize;
    let off = HEADER_LEN as u64 + index * (cs + TAG_LEN as u64);
    let mut buf = Zeroizing::new(vec![0u8; pt_len + TAG_LEN]);
    self.inner.seek(SeekFrom::Start(off))?;
    self.inner.read_exact(&mut buf)?;
    let tag = Tag::try_from(&buf[pt_len..]).expect("tag slice is 16 bytes");
    cipher(&self.key)
      .decrypt_inout_detached(&nonce(index, last), &self.aad, buf[..pt_len].as_mut().into(), &tag)
      .map_err(|_| Error::Auth)?;
    buf.truncate(pt_len);
    Ok(buf)
  }

  /// Decrypt the plaintext bytes in `range`, touching only the chunks it
  /// covers. Errors with [`Error::OutOfRange`] if the range is inverted or
  /// extends past [`len`](Self::len).
  pub fn read_range(&mut self, range: Range<u64>) -> Result<Zeroizing<Vec<u8>>> {
    if range.start > range.end || range.end > self.plaintext_len {
      return Err(Error::OutOfRange);
    }
    let total = usize::try_from(range.end - range.start).map_err(|_| Error::OutOfRange)?;
    let mut out = Zeroizing::new(Vec::with_capacity(total));
    if total == 0 {
      return Ok(out);
    }
    let cs = self.chunk_size as u64;
    let (first, last) = (range.start / cs, (range.end - 1) / cs);
    for i in first..=last {
      let chunk = self.read_chunk(i)?;
      let base = i * cs;
      let lo = (range.start.max(base) - base) as usize;
      let hi = (range.end.min(base + chunk.len() as u64) - base) as usize;
      out.extend_from_slice(&chunk[lo..hi]);
    }
    Ok(out)
  }

  /// Decrypt the whole file.
  pub fn read_all(&mut self) -> Result<Zeroizing<Vec<u8>>> {
    self.read_range(0..self.plaintext_len)
  }

  /// Give back the inner reader.
  pub fn into_inner(self) -> R {
    self.inner
  }
}

/// Atomically write `plaintext` to `path` in the chunked format: write to a
/// mode-0600 temp file in the same directory, fsync it, rename over `path`,
/// then fsync the directory.
pub fn seal_file(
  path: &Path,
  sub: &SubKey,
  logical_path: &[u8],
  chunk_size_log2: u8,
  plaintext: &[u8],
) -> Result<()> {
  check_log2(chunk_size_log2)?;
  let dir = match path.parent() {
    Some(d) if !d.as_os_str().is_empty() => d,
    _ => Path::new("."),
  };
  let name = path
    .file_name()
    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
  let mut rnd = [0u8; 8];
  os_random(&mut rnd);
  let mut tmp_name = std::ffi::OsString::from(".");
  tmp_name.push(name);
  tmp_name.push(format!(".tmp-{:016x}", u64::from_ne_bytes(rnd)));
  let tmp = dir.join(tmp_name);

  let write = || -> Result<()> {
    let f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    let mut w = ChunkedWriter::new(sub, logical_path, chunk_size_log2, io::BufWriter::new(f))?;
    w.write_all(plaintext)?;
    let f = w.finish()?.into_inner().map_err(|e| e.into_error())?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)?;
    Ok(())
  };
  if let Err(e) = write() {
    let _ = fs::remove_file(&tmp);
    return Err(e);
  }
  File::open(dir)?.sync_all()?;
  Ok(())
}

/// Read and authenticate a whole chunked file written for `logical_path`.
pub fn open_file(path: &Path, sub: &SubKey, logical_path: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
  let f = File::open(path)?;
  ChunkedReader::open(sub, logical_path, io::BufReader::new(f))?.read_all()
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{DataKey, Label};

  #[test]
  fn plaintext_len_table() {
    let h = HEADER_LEN as u64;
    let cs = 4096u64;
    let full = cs + 16;
    // Impossible lengths.
    for bad in [0, h - 1, h, h + 1, h + 15, h + full + 1, h + full + 15, h + 3 * full + 15] {
      assert!(plaintext_len(bad, cs).is_err(), "{bad} should be rejected");
    }
    assert!(plaintext_len(h + 16, 0).is_err());
    // Possible lengths.
    assert_eq!(plaintext_len(h + 16, cs).unwrap(), 0);
    assert_eq!(plaintext_len(h + 17, cs).unwrap(), 1);
    assert_eq!(plaintext_len(h + full, cs).unwrap(), cs);
    // Full chunk followed by an empty final chunk: non-canonical, rejected.
    assert!(plaintext_len(h + full + 16, cs).is_err());
    assert!(plaintext_len(h + 3 * full + 16, cs).is_err());
    assert_eq!(plaintext_len(h + full + 17, cs).unwrap(), cs + 1);
    assert_eq!(plaintext_len(h + 5 * full + 23, cs).unwrap(), 5 * cs + 7);
    // No overflow panics at the extremes.
    let _ = plaintext_len(u64::MAX, cs);
    let _ = plaintext_len(u64::MAX, u64::MAX);
  }

  #[test]
  fn nonce_layout() {
    let n = nonce(0x0102_0304_0506_0708, true);
    assert_eq!(n.as_slice(), &[1, 2, 3, 4, 5, 6, 7, 8, 1, 0, 0, 0]);
    let n = nonce(3, false);
    assert_eq!(n.as_slice(), &[0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0]);
  }

  /// Pinned format vector: all-zero DataKey, blob sub-key, salt = 0x11 * 32,
  /// logical path "vec", chunk 4 KiB, plaintext = 5000 bytes of i % 251.
  /// Cross-checked against Python `cryptography` (ChaCha20Poly1305 + HKDF).
  /// A failure means the on-disk format changed.
  #[test]
  fn format_vector_is_stable() {
    let sub = DataKey::from_bytes(Zeroizing::new([0u8; 32])).derive(Label::Blob);
    let pt: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    let mut w = ChunkedWriter::with_salt(&sub, b"vec", 12, [0x11; 32], Vec::new()).unwrap();
    w.write_all(&pt).unwrap();
    let file = w.finish().unwrap();
    assert_eq!(file.len(), HEADER_LEN + 4096 + 16 + 904 + 16);
    let hex: String = file.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
      &hex[..HEADER_LEN * 2],
      &format!("53504c435259303101{:02x}0000{}", 12, "11".repeat(32))
    );
    // tag of chunk 0 and tag of the final chunk
    let t0 = &hex[(HEADER_LEN + 4096) * 2..(HEADER_LEN + 4112) * 2];
    let t1 = &hex[hex.len() - 32..];
    assert_eq!(t0, "4c1536d795a4ad89514e2c90726d7c37");
    assert_eq!(t1, "f9661ef7b84cee23a72beab233b1d5d3");
    use sha2::Digest;
    let digest: String = sha2::Sha256::digest(&file).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(digest, "fbfd709463e9abdb82d0ed32ebceb944bada78d3adf6dc8d6a034edf2c98ad9e");
    let mut r = ChunkedReader::open(&sub, b"vec", io::Cursor::new(file)).unwrap();
    assert_eq!(&r.read_all().unwrap()[..], &pt[..]);
  }
}
