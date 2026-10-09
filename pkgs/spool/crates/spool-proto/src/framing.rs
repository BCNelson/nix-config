//! Length-prefixed postcard framing: `u32` big-endian length + payload.

use std::io::{self, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Maximum payload length of one frame (16 MiB). Larger frames are rejected
/// by both writers and readers; readers reject based on the length prefix
/// alone, before allocating.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
  /// The peer closed the stream cleanly on a frame boundary.
  #[error("connection closed")]
  Eof,
  #[error("frame of {0} bytes exceeds the {MAX_FRAME}-byte limit")]
  TooLarge(usize),
  #[error("i/o error: {0}")]
  Io(#[from] io::Error),
  #[error("codec error: {0}")]
  Codec(#[from] postcard::Error),
}

/// Encode `msg` as a complete frame (length prefix + payload).
pub fn encode_frame<T: Serialize + ?Sized>(msg: &T) -> Result<Vec<u8>, FrameError> {
  // Reserve the prefix, serialize after it, then patch the length in.
  let mut buf = postcard::to_extend(msg, vec![0u8; 4])?;
  let len = buf.len() - 4;
  if len > MAX_FRAME {
    return Err(FrameError::TooLarge(len));
  }
  buf[..4].copy_from_slice(&(len as u32).to_be_bytes());
  Ok(buf)
}

/// Decode a payload (without length prefix). Trailing bytes are an error.
pub fn decode_payload<T: DeserializeOwned>(payload: &[u8]) -> Result<T, FrameError> {
  let (msg, rest) = postcard::take_from_bytes(payload)?;
  if !rest.is_empty() {
    return Err(FrameError::Codec(postcard::Error::DeserializeBadEncoding));
  }
  Ok(msg)
}

fn check_len(prefix: [u8; 4]) -> Result<usize, FrameError> {
  let len = u32::from_be_bytes(prefix) as usize;
  if len > MAX_FRAME {
    return Err(FrameError::TooLarge(len));
  }
  Ok(len)
}

/// Write one frame and flush.
pub fn write_frame<W: Write + ?Sized, T: Serialize + ?Sized>(
  w: &mut W,
  msg: &T,
) -> Result<(), FrameError> {
  let buf = encode_frame(msg)?;
  w.write_all(&buf)?;
  w.flush()?;
  Ok(())
}

/// Read exactly one frame. Returns [`FrameError::Eof`] if the stream ends
/// before the first byte of the length prefix; an EOF anywhere later is an
/// `Io(UnexpectedEof)`.
pub fn read_frame<R: Read + ?Sized, T: DeserializeOwned>(r: &mut R) -> Result<T, FrameError> {
  let mut prefix = [0u8; 4];
  let mut got = 0;
  while got < 4 {
    match r.read(&mut prefix[got..]) {
      Ok(0) if got == 0 => return Err(FrameError::Eof),
      Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into()),
      Ok(n) => got += n,
      Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
      Err(e) => return Err(e.into()),
    }
  }
  let len = check_len(prefix)?;
  let mut payload = vec![0u8; len];
  r.read_exact(&mut payload)?;
  decode_payload(&payload)
}

/// Async [`write_frame`].
#[cfg(feature = "tokio")]
pub async fn write_frame_async<W, T>(w: &mut W, msg: &T) -> Result<(), FrameError>
where
  W: tokio::io::AsyncWrite + Unpin + ?Sized,
  T: Serialize + ?Sized,
{
  use tokio::io::AsyncWriteExt;
  let buf = encode_frame(msg)?;
  w.write_all(&buf).await?;
  w.flush().await?;
  Ok(())
}

/// Async [`read_frame`], same EOF semantics. Cancel-safety: not cancel-safe
/// (a cancelled read may leave a partial frame consumed); wrap the whole
/// connection in a timeout instead of individual reads.
#[cfg(feature = "tokio")]
pub async fn read_frame_async<R, T>(r: &mut R) -> Result<T, FrameError>
where
  R: tokio::io::AsyncRead + Unpin + ?Sized,
  T: DeserializeOwned,
{
  use tokio::io::AsyncReadExt;
  let mut prefix = [0u8; 4];
  let mut got = 0;
  while got < 4 {
    match r.read(&mut prefix[got..]).await? {
      0 if got == 0 => return Err(FrameError::Eof),
      0 => return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into()),
      n => got += n,
    }
  }
  let len = check_len(prefix)?;
  let mut payload = vec![0u8; len];
  r.read_exact(&mut payload).await?;
  decode_payload(&payload)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::*;
  use std::io::Cursor;

  fn roundtrip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(msg: T) {
    let mut buf = Vec::new();
    write_frame(&mut buf, &msg).unwrap();
    assert_eq!(u32::from_be_bytes(buf[..4].try_into().unwrap()) as usize, buf.len() - 4);
    let back: T = read_frame(&mut Cursor::new(buf)).unwrap();
    assert_eq!(back, msg);
  }

  #[test]
  fn roundtrip_public() {
    roundtrip(Hello::current());
    for req in [
      PublicReq::Show,
      PublicReq::Pick,
      PublicReq::Copy { mime: "text/plain;charset=utf-8".into(), data: b"hi\0there".to_vec() },
      PublicReq::Current,
      PublicReq::Pause { secs: Some(30) },
      PublicReq::Pause { secs: None },
      PublicReq::Resume,
      PublicReq::Status,
    ] {
      roundtrip(req);
    }
    for resp in [
      PublicResp::Ok,
      PublicResp::Current { mime: "image/png".into(), data: vec![0x89, b'P', b'N', b'G'] },
      PublicResp::Empty,
      PublicResp::Status(StatusInfo {
        version: "0.1.0".into(),
        paused: PauseState::PausedUntil { unix_ms: 1_700_000_000_000 },
        item_count: 42,
        compositor: Some("wayland-1".into()),
        primary_enabled: false,
        encrypted: true,
        unlocked: true,
        key_state: "ready".into(),
        capabilities: Some(crate::Capabilities {
          compositor: "kwin".into(),
          hotkey: "kwin-script".into(),
          focus: "kwin-script".into(),
          cursor: true,
          auto_paste: true,
          paste_backend: "fake-input".into(),
        }),
      }),
      PublicResp::Error { code: ErrorCode::RateLimited, message: "slow down".into() },
      PublicResp::NotYetImplemented,
      PublicResp::Picked { mime: "text/plain".into(), data: b"x".to_vec() },
      PublicResp::Cancelled,
    ] {
      roundtrip(resp);
    }
  }

  #[test]
  fn roundtrip_picker() {
    roundtrip(PickerReq::Query {
      seq: 1,
      q: "foo".into(),
      filters: QueryFilters { pinned_only: true, ..Default::default() },
      offset: 0,
      limit: 50,
    });
    roundtrip(PickerReq::Unlock { provider: UnlockProvider::KWallet, secret: None });
    roundtrip(PickerEvt::Show {
      cursor: Some(CursorPos { x: -5, y: 10 }),
      output: Some("DP-1".into()),
      target_window: None,
      scale_hint: None,
    });
    roundtrip(PickerEvt::Page {
      seq: 1,
      offset: 0,
      more: false,
      items: vec![ItemPreview {
        id: 7,
        kind: PreviewKind::Text,
        preview: "hello".into(),
        mimes: vec!["text/plain".into()],
        source_app: None,
        created_unix_ms: 1,
        last_used_unix_ms: 2,
        pinned: false,
        tags: vec![],
        total_size: 5,
      }],
    });
  }

  #[test]
  fn multiple_frames_then_eof() {
    let mut buf = Vec::new();
    write_frame(&mut buf, &Hello::current()).unwrap();
    write_frame(&mut buf, &PublicReq::Status).unwrap();
    let mut c = Cursor::new(buf);
    assert_eq!(read_frame::<_, Hello>(&mut c).unwrap(), Hello::current());
    assert_eq!(read_frame::<_, PublicReq>(&mut c).unwrap(), PublicReq::Status);
    assert!(matches!(read_frame::<_, PublicReq>(&mut c), Err(FrameError::Eof)));
  }

  #[test]
  fn truncated_is_unexpected_eof() {
    let mut buf = Vec::new();
    write_frame(&mut buf, &PublicReq::Copy { mime: "a".into(), data: vec![1; 100] }).unwrap();
    for cut in [2, 10] {
      let r = read_frame::<_, PublicReq>(&mut Cursor::new(&buf[..cut]));
      match r {
        Err(FrameError::Io(e)) => assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof),
        other => panic!("unexpected {other:?}"),
      }
    }
  }

  #[test]
  fn oversized_prefix_rejected_without_alloc() {
    let mut buf = ((MAX_FRAME + 1) as u32).to_be_bytes().to_vec();
    buf.extend_from_slice(&[0; 8]);
    assert!(matches!(
      read_frame::<_, PublicReq>(&mut Cursor::new(buf)),
      Err(FrameError::TooLarge(n)) if n == MAX_FRAME + 1
    ));
  }

  #[test]
  fn oversized_write_rejected() {
    let big = PublicReq::Copy { mime: "x".into(), data: vec![0; MAX_FRAME] };
    assert!(matches!(encode_frame(&big), Err(FrameError::TooLarge(_))));
  }

  #[test]
  fn max_size_payload_ok() {
    // Payload exactly at the limit is accepted.
    let data = vec![7u8; MAX_FRAME - 16];
    let msg = PublicReq::Copy { mime: "x".into(), data };
    let frame = encode_frame(&msg).unwrap();
    assert!(frame.len() - 4 <= MAX_FRAME);
    let back: PublicReq = read_frame(&mut Cursor::new(frame)).unwrap();
    assert_eq!(back, msg);
  }

  #[test]
  fn trailing_garbage_rejected() {
    let mut payload = postcard::to_stdvec(&PublicReq::Status).unwrap();
    payload.push(0xff);
    assert!(decode_payload::<PublicReq>(&payload).is_err());
  }

  #[test]
  fn garbage_payload_rejected() {
    let mut buf = 3u32.to_be_bytes().to_vec();
    buf.extend_from_slice(&[0xff, 0xff, 0xff]);
    assert!(matches!(read_frame::<_, PublicReq>(&mut Cursor::new(buf)), Err(FrameError::Codec(_))));
  }

  #[test]
  fn hello_compat() {
    assert!(Hello::current().is_compatible());
    assert!(!Hello { proto: PROTO_VERSION + 1 }.is_compatible());
  }

  #[cfg(feature = "tokio")]
  #[tokio::test]
  async fn async_roundtrip() {
    let (mut a, mut b) = tokio::io::duplex(64);
    let msg = PublicReq::Copy { mime: "text/plain".into(), data: vec![9; 1000] };
    let m2 = msg.clone();
    let w = tokio::spawn(async move {
      write_frame_async(&mut a, &Hello::current()).await.unwrap();
      write_frame_async(&mut a, &m2).await.unwrap();
    });
    assert_eq!(read_frame_async::<_, Hello>(&mut b).await.unwrap(), Hello::current());
    assert_eq!(read_frame_async::<_, PublicReq>(&mut b).await.unwrap(), msg);
    w.await.unwrap();
    assert!(matches!(read_frame_async::<_, PublicReq>(&mut b).await, Err(FrameError::Eof)));
  }
}
