//! Pipe I/O that runs off the Wayland thread: reading offers (with caps and a
//! deadline) and serving `send` requests (with a write deadline).

use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;

use crate::{FetchError, FetchResult};

const CHUNK: usize = 64 * 1024;

/// Poll `fds` until one is ready or `deadline` passes. Returns `false` on
/// deadline.
fn poll_until(fds: &mut [PollFd<'_>], deadline: Instant) -> Result<bool, Errno> {
  loop {
    let now = Instant::now();
    if now >= deadline {
      return Ok(false);
    }
    let left = deadline - now;
    // Round up so we never spin on a sub-ms remainder.
    let ts = Timespec::try_from(left.max(Duration::from_millis(1)))
      .unwrap_or(Timespec { tv_sec: i64::MAX, tv_nsec: 0 });
    match poll(fds, Some(&ts)) {
      Ok(0) => continue, // loop re-checks the deadline
      Ok(_) => return Ok(true),
      Err(Errno::INTR) => continue,
      Err(e) => return Err(e),
    }
  }
}

enum SlotState {
  Reading(OwnedFd),
  Done,
  /// Over `per_rep_cap` or read error: omitted from the result.
  Dropped,
}

struct Slot {
  mime: String,
  state: SlotState,
  buf: Vec<u8>,
}

/// Read every `(mime, read_end)` concurrently until EOF, enforcing the caps
/// and the deadline. Results keep the order of `reads`.
pub(crate) fn read_reps(
  reads: Vec<(String, OwnedFd)>,
  per_rep_cap: usize,
  total_cap: usize,
  deadline: Instant,
) -> FetchResult {
  let mut slots = Vec::with_capacity(reads.len());
  for (mime, fd) in reads {
    rustix::io::ioctl_fionbio(&fd, true).map_err(|e| FetchError::Io(e.to_string()))?;
    slots.push(Slot { mime, state: SlotState::Reading(fd), buf: Vec::new() });
  }
  let mut chunk = vec![0u8; CHUNK];
  let mut total = 0usize;

  loop {
    let active: Vec<usize> =
      (0..slots.len()).filter(|&i| matches!(slots[i].state, SlotState::Reading(_))).collect();
    if active.is_empty() {
      break;
    }
    let ready: Vec<usize> = {
      let mut pfds: Vec<PollFd<'_>> = active
        .iter()
        .map(|&i| match &slots[i].state {
          SlotState::Reading(fd) => PollFd::new(fd, PollFlags::IN),
          _ => unreachable!(),
        })
        .collect();
      match poll_until(&mut pfds, deadline) {
        Ok(true) => {}
        Ok(false) => return Err(FetchError::Timeout),
        Err(e) => return Err(FetchError::Io(e.to_string())),
      }
      active.iter().zip(&pfds).filter(|(_, p)| !p.revents().is_empty()).map(|(&i, _)| i).collect()
    };

    for i in ready {
      let slot = &mut slots[i];
      loop {
        if Instant::now() >= deadline {
          return Err(FetchError::Timeout);
        }
        let SlotState::Reading(fd) = &slot.state else { break };
        match rustix::io::read(fd, &mut chunk[..]) {
          Ok(0) => {
            slot.state = SlotState::Done;
            break;
          }
          Ok(n) => {
            if slot.buf.len() + n > per_rep_cap {
              tracing::debug!(mime = %slot.mime, cap = per_rep_cap, "representation over per-rep cap; omitted");
              total -= slot.buf.len();
              slot.buf = Vec::new();
              slot.state = SlotState::Dropped;
              break;
            }
            slot.buf.extend_from_slice(&chunk[..n]);
            total += n;
            if total > total_cap {
              tracing::debug!(total, cap = total_cap, "offer over total cap");
              return Err(FetchError::TooLarge);
            }
          }
          Err(Errno::INTR) => continue,
          Err(Errno::AGAIN) => break,
          Err(e) => {
            tracing::debug!(mime = %slot.mime, error = %e, "read error; representation omitted");
            total -= slot.buf.len();
            slot.buf = Vec::new();
            slot.state = SlotState::Dropped;
            break;
          }
        }
      }
    }
  }

  Ok(
    slots
      .into_iter()
      .filter(|s| matches!(s.state, SlotState::Done))
      .map(|s| (s.mime, s.buf))
      .collect(),
  )
}

/// Write `data` to `fd` (a paste client's pipe) and close it, giving up at
/// `deadline`. Errors are logged, never propagated.
pub(crate) fn write_all(fd: OwnedFd, data: &[u8], deadline: Instant) {
  if let Err(e) = rustix::io::ioctl_fionbio(&fd, true) {
    tracing::debug!(error = %e, "cannot make send fd non-blocking");
    return;
  }
  let mut off = 0;
  while off < data.len() {
    let end = (off + CHUNK).min(data.len());
    match rustix::io::write(&fd, &data[off..end]) {
      Ok(n) => off += n,
      Err(Errno::INTR) => {}
      Err(Errno::AGAIN) => {
        let mut pfd = [PollFd::new(&fd, PollFlags::OUT)];
        match poll_until(&mut pfd, deadline) {
          Ok(true) => {}
          Ok(false) => {
            tracing::debug!(written = off, len = data.len(), "send timed out; closing");
            return;
          }
          Err(e) => {
            tracing::debug!(error = %e, "poll on send fd failed");
            return;
          }
        }
      }
      Err(e) => {
        // EPIPE: the reader went away. Needs SIGPIPE ignored (Rust's default).
        tracing::debug!(error = %e, written = off, len = data.len(), "send write failed");
        return;
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use rustix::pipe::{PipeFlags, pipe_with};

  fn pipe() -> (OwnedFd, OwnedFd) {
    pipe_with(PipeFlags::CLOEXEC).unwrap()
  }

  fn soon(ms: u64) -> Instant {
    Instant::now() + Duration::from_millis(ms)
  }

  #[test]
  fn reads_until_eof_in_order() {
    let (r1, w1) = pipe();
    let (r2, w2) = pipe();
    rustix::io::write(&w2, b"two").unwrap();
    rustix::io::write(&w1, b"one").unwrap();
    drop((w1, w2));
    let res = read_reps(vec![("a".into(), r1), ("b".into(), r2)], 100, 100, soon(1000)).unwrap();
    assert_eq!(res, vec![("a".into(), b"one".to_vec()), ("b".into(), b"two".to_vec())]);
  }

  #[test]
  fn per_rep_cap_omits() {
    let (r1, w1) = pipe();
    let (r2, w2) = pipe();
    rustix::io::write(&w1, &[0u8; 20]).unwrap();
    rustix::io::write(&w2, &[0u8; 10]).unwrap();
    drop((w1, w2));
    let res = read_reps(vec![("big".into(), r1), ("ok".into(), r2)], 10, 100, soon(1000)).unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].0, "ok");
  }

  #[test]
  fn exact_cap_is_kept() {
    let (r, w) = pipe();
    rustix::io::write(&w, &[1u8; 10]).unwrap();
    drop(w);
    let res = read_reps(vec![("x".into(), r)], 10, 10, soon(1000)).unwrap();
    assert_eq!(res[0].1.len(), 10);
  }

  #[test]
  fn total_cap_fails() {
    let (r1, w1) = pipe();
    let (r2, w2) = pipe();
    rustix::io::write(&w1, &[0u8; 8]).unwrap();
    rustix::io::write(&w2, &[0u8; 8]).unwrap();
    drop((w1, w2));
    let res = read_reps(vec![("a".into(), r1), ("b".into(), r2)], 10, 12, soon(1000));
    assert_eq!(res, Err(FetchError::TooLarge));
  }

  #[test]
  fn never_closing_source_times_out() {
    let (r, w) = pipe();
    rustix::io::write(&w, b"partial").unwrap();
    let start = Instant::now();
    let res = read_reps(vec![("x".into(), r)], 100, 100, soon(150));
    assert_eq!(res, Err(FetchError::Timeout));
    assert!(start.elapsed() < Duration::from_secs(2));
    drop(w);
  }

  #[test]
  fn empty_request_is_ok() {
    assert_eq!(read_reps(vec![], 1, 1, soon(10)), Ok(vec![]));
  }

  #[test]
  fn write_times_out_on_full_pipe() {
    let (r, w) = pipe();
    let data = vec![7u8; 4 * 1024 * 1024]; // larger than any pipe buffer
    let start = Instant::now();
    write_all(w, &data, soon(150));
    assert!(start.elapsed() < Duration::from_secs(2));
    drop(r);
  }

  #[test]
  fn write_delivers_and_closes() {
    let (r, w) = pipe();
    let data: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
    let d2 = data.clone();
    let t = std::thread::spawn(move || write_all(w, &d2, soon(5000)));
    let got = read_reps(vec![("x".into(), r)], usize::MAX, usize::MAX, soon(5000)).unwrap();
    t.join().unwrap();
    assert_eq!(got[0].1, data);
  }

  #[test]
  fn write_to_closed_reader_returns() {
    let (r, w) = pipe();
    drop(r);
    write_all(w, b"hello", soon(1000));
  }
}
