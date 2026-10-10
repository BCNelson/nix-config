//! inotify on an editing session's directory.
//!
//! The directory is watched rather than the file: editors often save by
//! writing a temp file and renaming it over the original (vim's
//! `backupcopy=no`, Kate, Krita), which replaces the inode a file watch
//! would follow. Only events naming the edited file (or a queue overflow)
//! count; swap / backup / probe files next to it are ignored.

use std::ffi::OsString;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};
use rustix::io::Errno;
use tokio::io::unix::AsyncFd;

pub struct DirWatch {
  fd: AsyncFd<OwnedFd>,
  name: OsString,
}

impl DirWatch {
  /// Watch `dir` for changes to its entry `name`. Must be called inside a
  /// tokio runtime.
  pub fn new(dir: &Path, name: &std::ffi::OsStr) -> io::Result<Self> {
    let fd = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?;
    inotify::add_watch(
      &fd,
      dir,
      WatchFlags::CLOSE_WRITE
        | WatchFlags::MODIFY
        | WatchFlags::MOVED_TO
        | WatchFlags::CREATE
        | WatchFlags::DELETE,
    )?;
    Ok(Self { fd: AsyncFd::new(fd)?, name: name.to_owned() })
  }

  /// Wait until the watched file was (possibly) changed.
  pub async fn changed(&mut self) -> io::Result<()> {
    loop {
      let mut guard = self.fd.readable().await?;
      let mut relevant = false;
      let mut buf = [MaybeUninit::<u8>::uninit(); 4096];
      {
        let mut reader = inotify::Reader::new(guard.get_inner(), &mut buf);
        loop {
          match reader.next() {
            Ok(ev) => {
              let ours = ev.file_name().is_some_and(|n| n.to_bytes() == self.name.as_bytes());
              if ours || ev.events().contains(ReadFlags::QUEUE_OVERFLOW) {
                relevant = true;
              }
            }
            Err(Errno::AGAIN) => break,
            Err(Errno::INTR) => continue,
            Err(e) => return Err(e.into()),
          }
        }
      }
      guard.clear_ready();
      if relevant {
        return Ok(());
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::time::Duration;

  #[tokio::test]
  async fn sees_writes_and_renames_of_the_file_only() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("item.txt");
    std::fs::write(&file, b"v0").unwrap();
    let mut w = DirWatch::new(d.path(), std::ffi::OsStr::new("item.txt")).unwrap();
    let wait = Duration::from_secs(5);

    // Other files in the directory (swap, backup, probe files) don't count.
    std::fs::write(d.path().join(".item.txt.swp"), b"x").unwrap();
    std::fs::write(d.path().join("4913"), b"x").unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(150), w.changed()).await.is_err());

    std::fs::write(&file, b"v1").unwrap();
    tokio::time::timeout(wait, w.changed()).await.expect("write seen").unwrap();

    // Save by rename (new inode).
    let tmp = d.path().join("item.txt.new");
    std::fs::write(&tmp, b"v2").unwrap();
    // Drain the events of writing the temp file first.
    let _ = tokio::time::timeout(Duration::from_millis(100), w.changed()).await;
    std::fs::rename(&tmp, &file).unwrap();
    tokio::time::timeout(wait, w.changed()).await.expect("rename seen").unwrap();
    // Still watching after the inode changed.
    std::fs::write(&file, b"v3").unwrap();
    tokio::time::timeout(wait, w.changed()).await.expect("write after rename seen").unwrap();
  }
}
