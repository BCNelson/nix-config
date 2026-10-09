//! The `session` provider: KEKs live only in this process's memory.
//!
//! Used for the pre-unlock session database (history kept only until the
//! real key is available) and for users who choose no persistence. A slot
//! enrolled here can never be unlocked by another process or after a restart:
//! `unlock` then fails with [`Error::Unavailable`].

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::error::{Error, Result};
use crate::provider::{Kek, KeyProvider, ProviderKind, SlotParams};
use crate::slots::SlotId;
use crate::wrap;

/// In-memory KEK store. Dropping it forgets (and zeroizes) every KEK.
#[derive(Default)]
pub struct SessionProvider {
  keks: Mutex<HashMap<SlotId, Kek>>,
}

impl SessionProvider {
  /// An empty provider.
  pub fn new() -> Self {
    Self::default()
  }

  /// Number of KEKs currently held.
  pub fn len(&self) -> usize {
    self.keks.lock().expect("session provider mutex").len()
  }

  /// True if no KEKs are held.
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }
}

impl std::fmt::Debug for SessionProvider {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("SessionProvider").field("slots", &self.len()).finish()
  }
}

#[async_trait]
impl KeyProvider for SessionProvider {
  fn kind(&self) -> ProviderKind {
    ProviderKind::Session
  }

  fn interactive(&self) -> bool {
    false
  }

  async fn enroll(&self, slot_id: &SlotId) -> Result<(SlotParams, Kek)> {
    let kek = wrap::random_key();
    self.keks.lock().expect("session provider mutex").insert(*slot_id, kek.clone());
    Ok((SlotParams::Session, kek))
  }

  async fn unlock(&self, slot_id: &SlotId, params: &SlotParams) -> Result<Kek> {
    if *params != SlotParams::Session {
      return Err(Error::ProviderCorrupt {
        kind: ProviderKind::Session,
        reason: "slot params are not session params".into(),
      });
    }
    self.keks.lock().expect("session provider mutex").get(slot_id).cloned().ok_or_else(|| {
      Error::Unavailable {
        kind: ProviderKind::Session,
        reason: "session key is gone (process restarted)".into(),
      }
    })
  }

  async fn destroy(&self, slot_id: &SlotId, _params: &SlotParams) -> Result<()> {
    self.keks.lock().expect("session provider mutex").remove(slot_id);
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn enroll_unlock_destroy() {
    let p = SessionProvider::new();
    let id = SlotId::new();
    let (params, kek) = p.enroll(&id).await.unwrap();
    assert_eq!(params, SlotParams::Session);
    assert_eq!(*p.unlock(&id, &params).await.unwrap(), *kek);
    assert!(!p.interactive());
    p.destroy(&id, &params).await.unwrap();
    assert!(p.is_empty());
    let e = p.unlock(&id, &params).await.unwrap_err();
    assert!(matches!(e, Error::Unavailable { .. }) && e.is_retryable());
    // Idempotent.
    p.destroy(&id, &params).await.unwrap();
  }

  #[tokio::test]
  async fn new_instance_cannot_unlock() {
    let id = SlotId::new();
    let (params, _) = SessionProvider::new().enroll(&id).await.unwrap();
    let e = SessionProvider::new().unlock(&id, &params).await.unwrap_err();
    assert!(matches!(e, Error::Unavailable { kind: ProviderKind::Session, .. }));
  }

  #[tokio::test]
  async fn distinct_keks_per_slot() {
    let p = SessionProvider::new();
    let (_, a) = p.enroll(&SlotId::new()).await.unwrap();
    let (_, b) = p.enroll(&SlotId::new()).await.unwrap();
    assert_ne!(*a, *b);
    assert_eq!(p.len(), 2);
  }
}
