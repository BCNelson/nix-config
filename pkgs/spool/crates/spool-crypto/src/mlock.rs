//! Best-effort `mlock` of small secret allocations, and the [`Secret32`] box
//! every key type in this crate is built on.
//!
//! This is the only module allowed to use `unsafe` (libc `sysconf`, `mlock`,
//! `munlock`). None of these calls dereference memory: `mlock`/`munlock` only
//! change page attributes of an address range and fail with `ENOMEM` (no UB)
//! if the range is not mapped.
//!
//! `mlock` does not nest: one `munlock` unlocks a page no matter how many
//! secrets share it. Secrets are 32-byte heap allocations, so several share a
//! page; a process-wide refcount per page makes sure a page is only unlocked
//! when the last secret on it is dropped.

#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use zeroize::Zeroize;

/// page address -> number of live secrets on it (only pages we locked).
static LOCKED_PAGES: Mutex<BTreeMap<usize, usize>> = Mutex::new(BTreeMap::new());

fn page_size() -> usize {
  static PAGE: OnceLock<usize> = OnceLock::new();
  *PAGE.get_or_init(|| {
    // SAFETY: sysconf has no memory-safety preconditions and is thread-safe.
    let r = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if r > 0 && (r as usize).is_power_of_two() { r as usize } else { 4096 }
  })
}

fn pages_of(addr: usize, len: usize) -> impl Iterator<Item = usize> {
  let ps = page_size();
  let first = addr & !(ps - 1);
  let last = (addr + len.max(1) - 1) & !(ps - 1);
  (first..=last).step_by(ps)
}

/// Pages this guard holds a reference on; unlocking happens on drop.
struct LockGuard {
  pages: Vec<usize>,
}

fn lock(addr: usize, len: usize) -> LockGuard {
  let ps = page_size();
  let mut held = Vec::with_capacity(2);
  let mut map = LOCKED_PAGES.lock().unwrap_or_else(|e| e.into_inner());
  for page in pages_of(addr, len) {
    if let Some(n) = map.get_mut(&page) {
      *n += 1;
      held.push(page);
      continue;
    }
    // SAFETY: mlock only changes the residency attribute of the page range;
    // it does not read or write memory. The page contains a live allocation
    // owned by the caller, and an invalid range yields ENOMEM, not UB.
    let rc = unsafe { libc::mlock(page as *const libc::c_void, ps) };
    if rc == 0 {
      map.insert(page, 1);
      held.push(page);
    } else {
      tracing::debug!(
        error = %std::io::Error::last_os_error(),
        "mlock of key memory failed; continuing without it"
      );
    }
  }
  LockGuard { pages: held }
}

impl Drop for LockGuard {
  fn drop(&mut self) {
    let ps = page_size();
    let mut map = LOCKED_PAGES.lock().unwrap_or_else(|e| e.into_inner());
    for page in self.pages.drain(..) {
      let Some(n) = map.get_mut(&page) else { continue };
      *n -= 1;
      if *n == 0 {
        map.remove(&page);
        // SAFETY: as for mlock above; munlock does not touch memory contents.
        let rc = unsafe { libc::munlock(page as *const libc::c_void, ps) };
        if rc != 0 {
          tracing::debug!(error = %std::io::Error::last_os_error(), "munlock failed");
        }
      }
    }
  }
}

/// 32 secret bytes in a stable heap allocation: best-effort mlocked, zeroized
/// on drop (before the page is unlocked). No `Clone`, no `Debug`.
pub(crate) struct Secret32 {
  bytes: Box<[u8; 32]>,
  // Declared after `bytes`; `Drop::drop` below zeroizes before fields drop.
  _lock: LockGuard,
}

impl Secret32 {
  /// A zeroed, locked secret; fill it through [`Secret32::bytes_mut`].
  pub(crate) fn zeroed() -> Self {
    let bytes = Box::new([0u8; 32]);
    let lock = lock(bytes.as_ptr() as usize, bytes.len());
    Secret32 { bytes, _lock: lock }
  }

  pub(crate) fn bytes(&self) -> &[u8; 32] {
    &self.bytes
  }

  pub(crate) fn bytes_mut(&mut self) -> &mut [u8; 32] {
    &mut self.bytes
  }
}

impl Zeroize for Secret32 {
  fn zeroize(&mut self) {
    self.bytes.zeroize();
  }
}

impl Drop for Secret32 {
  fn drop(&mut self) {
    self.bytes.zeroize();
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn page_refcount_balances() {
    let a = Secret32::zeroed();
    let b = Secret32::zeroed();
    let pages: Vec<usize> = pages_of(a.bytes.as_ptr() as usize, 32).collect();
    drop(b);
    drop(a);
    let map = LOCKED_PAGES.lock().unwrap();
    // Other tests may hold secrets on the same pages concurrently, so only
    // check that nothing went negative / leaked a zero entry.
    for p in pages {
      if let Some(n) = map.get(&p) {
        assert!(*n > 0);
      }
    }
  }

  #[test]
  fn region_spanning_two_pages() {
    let ps = page_size();
    let pages: Vec<usize> = pages_of(ps * 10 - 4, 32).collect();
    assert_eq!(pages, vec![ps * 9, ps * 10]);
    let pages: Vec<usize> = pages_of(ps * 10, 32).collect();
    assert_eq!(pages, vec![ps * 10]);
  }
}
