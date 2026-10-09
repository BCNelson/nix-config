//! The `secret-service` provider (KWallet 6, gnome-keyring, oo7-daemon) via
//! the [`oo7`] crate.
//!
//! # Backend selection
//!
//! `oo7::Keyring::new()` silently picks its *file* backend (via the
//! `org.freedesktop.portal.Secret` portal) when it detects a Flatpak/Snap
//! sandbox. Spool runs host-side and must talk to the real Secret Service, so
//! this module never uses `oo7::Keyring`: it uses [`oo7::dbus::Service`]
//! directly, which always connects to `org.freedesktop.secrets` on the session
//! bus (`$DBUS_SESSION_BUS_ADDRESS`), trying an encrypted (DH + AES) transfer
//! session first and falling back to plain.
//!
//! # Storage
//!
//! Each slot's KEK is one item in the collection behind the `default` alias
//! (KWallet: the default wallet, normally `kdewallet`; gnome-keyring: usually
//! `login`). Label [`ITEM_LABEL`], attributes
//! `{application: "spool", slot: "<slot uuid>"}`, secret = the 32-byte KEK as
//! standard base64 text (text survives every implementation's content-type
//! handling and shows up sanely in wallet managers).
//!
//! # Locked wallets
//!
//! With `allow_prompt = false` (what the daemon uses at startup) a locked
//! collection yields [`Error::Locked`] (retryable) and no unlock prompt is
//! ever requested. Use [`SecretServiceProvider::wait_for_unlock`] to wait
//! until the user opens the wallet, then retry
//! [`crate::KeySlots::unlock_any`].
//!
//! A new `zbus` connection + Secret Service session is opened per operation
//! and dropped before the operation returns (oo7 closes the session from a
//! tokio task on drop, so a cached `Service` outliving the runtime would
//! panic). All methods must run inside a tokio runtime.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use futures_util::StreamExt;
use oo7::dbus::{Collection, Service, ServiceError};
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::provider::{Kek, KeyProvider, ProviderKind, SlotParams};
use crate::slots::SlotId;
use crate::wrap;

/// Value of the `application` attribute on every Spool item.
pub const APPLICATION: &str = "spool";
/// User-visible item label.
pub const ITEM_LABEL: &str = "Spool clipboard history key";
/// The Secret Service alias Spool stores its items under.
pub const COLLECTION_ALIAS: &str = "default";

const KIND: ProviderKind = ProviderKind::SecretService;

/// The lookup attributes for a slot's item.
pub fn slot_attributes(slot: &SlotId) -> BTreeMap<String, String> {
  BTreeMap::from([
    ("application".to_owned(), APPLICATION.to_owned()),
    ("slot".to_owned(), slot.to_string()),
  ])
}

fn map_err(e: oo7::dbus::Error) -> Error {
  use oo7::dbus::Error as E;
  match e {
    E::Service(ServiceError::IsLocked(_)) => Error::Locked(KIND),
    E::Service(ServiceError::NoSuchObject(_)) | E::Deleted => Error::SecretMissing(KIND),
    E::Dismissed => Error::Dismissed(KIND),
    E::Crypto(e) => Error::Provider { kind: KIND, reason: format!("transfer encryption: {e}") },
    E::ZBus(oo7::zbus::Error::MethodError(name, _, _)) if name.as_str().ends_with(".IsLocked") => {
      Error::Locked(KIND)
    }
    // Bus/transport problems, service not running yet, session gone, ...
    other => Error::Unavailable { kind: KIND, reason: other.to_string() },
  }
}

/// Secret Service-backed [`KeyProvider`].
#[derive(Debug, Clone)]
pub struct SecretServiceProvider {
  allow_prompt: bool,
}

impl SecretServiceProvider {
  /// `allow_prompt = false`: a locked wallet yields [`Error::Locked`] and no
  /// prompt is shown (daemon startup). `true`: ask the Secret Service to
  /// unlock (shows the wallet's password dialog; oo7 waits up to 30 s per
  /// D-Bus call), and allow creating the default collection on enroll.
  pub fn new(allow_prompt: bool) -> Self {
    Self { allow_prompt }
  }

  /// Whether this instance may trigger unlock prompts.
  pub fn allow_prompt(&self) -> bool {
    self.allow_prompt
  }

  async fn connect(&self) -> Result<Service> {
    Service::new().await.map_err(map_err)
  }

  /// The `default` collection. Missing -> create it only when prompting is
  /// allowed and `create` (creating one shows a password dialog), otherwise
  /// [`Error::Unavailable`].
  async fn collection(&self, service: &Service, create: bool) -> Result<Option<Collection>> {
    if let Some(c) = service.with_alias(COLLECTION_ALIAS).await.map_err(map_err)? {
      return Ok(Some(c));
    }
    if create && self.allow_prompt {
      return service.default_collection().await.map(Some).map_err(map_err);
    }
    Ok(None)
  }

  async fn ensure_unlocked(&self, c: &Collection) -> Result<()> {
    if c.is_locked().await.map_err(map_err)? {
      if !self.allow_prompt {
        return Err(Error::Locked(KIND));
      }
      c.unlock(None).await.map_err(map_err)?;
      if c.is_locked().await.map_err(map_err)? {
        return Err(Error::Locked(KIND));
      }
    }
    Ok(())
  }

  fn check_params<'a>(
    slot: &SlotId,
    params: &'a SlotParams,
  ) -> Result<&'a BTreeMap<String, String>> {
    match params {
      SlotParams::SecretService { attributes } if *attributes == slot_attributes(slot) => {
        Ok(attributes)
      }
      _ => Err(Error::ProviderCorrupt {
        kind: KIND,
        reason: "slot params do not match this slot".into(),
      }),
    }
  }

  async fn read_kek(&self, c: &Collection, attrs: &BTreeMap<String, String>) -> Result<Kek> {
    let items = c.search_items(attrs).await.map_err(map_err)?;
    let item = match items.as_slice() {
      [] => return Err(Error::SecretMissing(KIND)),
      [item] => item,
      _ => {
        return Err(Error::ProviderCorrupt {
          kind: KIND,
          reason: format!("{} items match one slot", items.len()),
        });
      }
    };
    if item.is_locked().await.map_err(map_err)? {
      if !self.allow_prompt {
        return Err(Error::Locked(KIND));
      }
      item.unlock(None).await.map_err(map_err)?;
    }
    let secret = item.secret().await.map_err(map_err)?;
    decode_kek(&secret)
  }

  /// Probe without prompting: `Ok(true)` if the default collection exists and
  /// is unlocked, `Ok(false)` if it is locked or does not exist yet, `Err`
  /// (retryable [`Error::Unavailable`]) if no Secret Service is reachable.
  pub async fn is_unlocked(&self) -> Result<bool> {
    let service = self.connect().await?;
    match service.with_alias(COLLECTION_ALIAS).await.map_err(map_err)? {
      None => Ok(false),
      Some(c) => Ok(!c.is_locked().await.map_err(map_err)?),
    }
  }

  /// Wait (never prompting) until the default collection is unlocked, e.g.
  /// because the user opened KWallet or the PAM module unlocked the keyring
  /// at login. Returns `Ok(())` once [`Self::is_unlocked`] is true; then the
  /// caller retries [`crate::KeySlots::unlock_any`].
  ///
  /// Wakes on the Secret Service `CollectionCreated` / `CollectionChanged`
  /// signals, and also polls with exponential backoff (1 s doubling to 30 s)
  /// because not every implementation signals lock-state changes (and the
  /// service may not be on the bus yet). With `max_wait = Some(d)` gives up
  /// after `d` with the last retryable error ([`Error::Locked`] or
  /// [`Error::Unavailable`]). Cancel-safe: drop the future to stop waiting.
  pub async fn wait_for_unlock(&self, max_wait: Option<Duration>) -> Result<()> {
    const MAX_DELAY: Duration = Duration::from_secs(30);
    let deadline = max_wait.map(|d| tokio::time::Instant::now() + d);
    let mut delay = Duration::from_secs(1);
    loop {
      let last = match self.is_unlocked().await {
        Ok(true) => return Ok(()),
        Ok(false) => Error::Locked(KIND),
        Err(e) if e.is_retryable() => e,
        Err(e) => return Err(e),
      };
      let mut sleep_for = delay;
      if let Some(deadline) = deadline {
        let now = tokio::time::Instant::now();
        if now >= deadline {
          return Err(last);
        }
        sleep_for = sleep_for.min(deadline - now);
      }
      match self.connect().await {
        Ok(service) => {
          let changed = service.receive_collection_changed().await;
          let created = service.receive_collection_created().await;
          match (changed, created) {
            (Ok(changed), Ok(created)) => {
              let mut changed = std::pin::pin!(changed);
              let mut created = std::pin::pin!(created);
              // Re-check once after subscribing to close the race with an
              // unlock that happened in between.
              if let Ok(true) = self.is_unlocked().await {
                return Ok(());
              }
              tokio::select! {
                _ = tokio::time::sleep(sleep_for) => {}
                _ = changed.next() => {}
                _ = created.next() => {}
              }
            }
            _ => tokio::time::sleep(sleep_for).await,
          }
        }
        Err(_) => tokio::time::sleep(sleep_for).await,
      }
      delay = (delay * 2).min(MAX_DELAY);
    }
  }

  /// Delete every item with `application = "spool"` in the default
  /// collection (orphan cleanup after a partial wipe, or when
  /// `keyslots.json` is unreadable). Returns how many were deleted.
  pub async fn destroy_all(&self) -> Result<usize> {
    let service = self.connect().await?;
    let Some(c) = self.collection(&service, false).await? else {
      return Ok(0);
    };
    self.ensure_unlocked(&c).await?;
    let items = c.search_items(&[("application", APPLICATION)]).await.map_err(map_err)?;
    let n = items.len();
    for item in items {
      item.delete(None).await.map_err(map_err)?;
    }
    if n > 0 {
      tracing::info!(count = n, "deleted Spool items from the Secret Service");
    }
    Ok(n)
  }
}

fn decode_kek(secret: &[u8]) -> Result<Kek> {
  let bad =
    || Error::ProviderCorrupt { kind: KIND, reason: "stored key is not 32 bytes of base64".into() };
  let decoded = Zeroizing::new(B64.decode(secret.trim_ascii()).map_err(|_| bad())?);
  if decoded.len() != 32 {
    return Err(bad());
  }
  let mut kek = Zeroizing::new([0u8; 32]);
  kek.copy_from_slice(&decoded);
  Ok(kek)
}

#[async_trait]
impl KeyProvider for SecretServiceProvider {
  fn kind(&self) -> ProviderKind {
    KIND
  }

  fn interactive(&self) -> bool {
    self.allow_prompt
  }

  async fn enroll(&self, slot_id: &SlotId) -> Result<(SlotParams, Kek)> {
    let service = self.connect().await?;
    let c = self.collection(&service, true).await?.ok_or_else(|| Error::Unavailable {
      kind: KIND,
      reason: "no default collection (wallet not set up)".into(),
    })?;
    self.ensure_unlocked(&c).await?;
    let kek = wrap::random_key();
    let attrs = slot_attributes(slot_id);
    let encoded = Zeroizing::new(B64.encode(&kek[..]));
    c.create_item(ITEM_LABEL, &attrs, oo7::Secret::text(encoded.as_str()), true, None)
      .await
      .map_err(map_err)?;
    // Read back: catches implementations that mangle secrets or attributes.
    let back = self.read_kek(&c, &attrs).await?;
    if *back != *kek {
      return Err(Error::ProviderCorrupt {
        kind: KIND,
        reason: "stored key did not read back".into(),
      });
    }
    tracing::debug!(slot = %slot_id, "stored slot key in the Secret Service");
    Ok((SlotParams::SecretService { attributes: attrs }, kek))
  }

  async fn unlock(&self, slot_id: &SlotId, params: &SlotParams) -> Result<Kek> {
    let attrs = Self::check_params(slot_id, params)?;
    let service = self.connect().await?;
    let c = self
      .collection(&service, false)
      .await?
      .ok_or_else(|| Error::Unavailable { kind: KIND, reason: "no default collection".into() })?;
    self.ensure_unlocked(&c).await?;
    self.read_kek(&c, attrs).await
  }

  async fn destroy(&self, slot_id: &SlotId, params: &SlotParams) -> Result<()> {
    let attrs = Self::check_params(slot_id, params)?;
    let service = self.connect().await?;
    let Some(c) = self.collection(&service, false).await? else {
      return Ok(());
    };
    self.ensure_unlocked(&c).await?;
    for item in c.search_items(attrs).await.map_err(map_err)? {
      item.delete(None).await.map_err(map_err)?;
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn attributes_shape() {
    let id = SlotId::new();
    let a = slot_attributes(&id);
    assert_eq!(a.len(), 2);
    assert_eq!(a["application"], "spool");
    assert_eq!(a["slot"], id.to_string());
  }

  #[test]
  fn kek_decoding() {
    let k = [7u8; 32];
    assert_eq!(*decode_kek(B64.encode(k).as_bytes()).unwrap(), k);
    assert_eq!(*decode_kek(format!("{}\n", B64.encode(k)).as_bytes()).unwrap(), k);
    assert!(decode_kek(B64.encode([1u8; 31]).as_bytes()).is_err());
    assert!(decode_kek(b"not base64!").is_err());
    let e = decode_kek(b"").unwrap_err();
    assert!(!e.is_retryable());
  }

  #[test]
  fn interactive_follows_allow_prompt() {
    assert!(!SecretServiceProvider::new(false).interactive());
    assert!(SecretServiceProvider::new(true).interactive());
  }
}
