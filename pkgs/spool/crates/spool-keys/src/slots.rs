//! `keyslots.json`: the LUKS-style slot file and the [`KeySlots`] API.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::error::{Error, Result, SlotAttempt};
use crate::fido2::Fido2Params;
use crate::fsutil;
use crate::passphrase::PassphraseParams;
use crate::provider::{KeyProvider, ProviderKind, SlotParams};
use crate::secret_service::slot_attributes;
use crate::wrap::{self, WRAPPED_LEN};

/// The history data key (256 bits). Zeroized on drop.
pub type DataKey = Zeroizing<[u8; 32]>;

/// Current `keyslots.json` format version.
pub const FORMAT_VERSION: u64 = 1;
/// Maximum accepted size of `keyslots.json`.
pub const MAX_FILE_SIZE: u64 = 64 * 1024;
/// Maximum number of slots.
pub const MAX_SLOTS: usize = 32;
/// File name inside the Spool state directory.
pub const FILE_NAME: &str = "keyslots.json";

/// Identifies one slot (random UUIDv4). Serialized as a hyphenated string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SlotId(pub Uuid);

impl SlotId {
  /// A fresh random id.
  pub fn new() -> Self {
    SlotId(Uuid::new_v4())
  }
}

impl Default for SlotId {
  fn default() -> Self {
    Self::new()
  }
}

impl fmt::Display for SlotId {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Display::fmt(&self.0.hyphenated(), f)
  }
}

impl FromStr for SlotId {
  type Err = uuid::Error;
  fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
    Uuid::parse_str(s).map(SlotId)
  }
}

/// Identifies one data key generation. Changes on [`KeySlots::rotate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DataKeyId(pub Uuid);

impl DataKeyId {
  /// A fresh random id.
  pub fn new() -> Self {
    DataKeyId(Uuid::new_v4())
  }
}

impl Default for DataKeyId {
  fn default() -> Self {
    Self::new()
  }
}

impl fmt::Display for DataKeyId {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Display::fmt(&self.0.hyphenated(), f)
  }
}

/// One unlock method's slot.
#[derive(Clone, PartialEq, Eq)]
pub struct Slot {
  id: SlotId,
  kind: ProviderKind,
  params: SlotParams,
  /// `nonce || ciphertext || tag` ([`WRAPPED_LEN`] bytes).
  wrapped: Vec<u8>,
}

impl Slot {
  /// Slot id.
  pub fn id(&self) -> SlotId {
    self.id
  }
  /// Provider kind.
  pub fn kind(&self) -> ProviderKind {
    self.kind
  }
  /// Provider params.
  pub fn params(&self) -> &SlotParams {
    &self.params
  }
}

impl fmt::Debug for Slot {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Slot")
      .field("id", &self.id)
      .field("kind", &self.kind)
      .field("params", &self.params)
      .finish_non_exhaustive()
  }
}

// ---- on-disk representation ---------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRepr {
  version: u64,
  data_key_id: Uuid,
  slots: Vec<SlotRepr>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SlotRepr {
  id: Uuid,
  provider: ProviderKind,
  params: serde_json::Value,
  wrapped: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretServiceParamsRepr {
  attributes: BTreeMap<String, String>,
}

/// Validate params for a slot (both on load and after `enroll`).
fn check_params(
  kind: ProviderKind,
  id: &SlotId,
  params: &SlotParams,
) -> std::result::Result<(), String> {
  match (kind, params) {
    (ProviderKind::SecretService, SlotParams::SecretService { attributes }) => {
      if *attributes != slot_attributes(id) {
        return Err(format!(
          "slot {id}: secret-service attributes must be exactly {{application: \"spool\", slot: \"{id}\"}}"
        ));
      }
      Ok(())
    }
    (ProviderKind::Session, SlotParams::Session) => Ok(()),
    (ProviderKind::Passphrase, SlotParams::Passphrase(p)) => {
      p.validate().map_err(|e| format!("slot {id}: passphrase params: {e}"))
    }
    (ProviderKind::Fido2Hmac, SlotParams::Fido2Hmac(p)) => {
      p.validate().map_err(|e| format!("slot {id}: fido2-hmac params: {e}"))
    }
    (k, SlotParams::Opaque(v)) if !k.is_implemented() && v.is_object() => Ok(()),
    (k, _) => Err(format!("slot {id}: params do not match provider {k}")),
  }
}

fn parse_params(
  kind: ProviderKind,
  id: &SlotId,
  v: serde_json::Value,
) -> std::result::Result<SlotParams, String> {
  let params = match kind {
    ProviderKind::SecretService => {
      let p: SecretServiceParamsRepr = serde_json::from_value(v)
        .map_err(|e| format!("slot {id}: bad secret-service params: {e}"))?;
      SlotParams::SecretService { attributes: p.attributes }
    }
    ProviderKind::Session => match v {
      serde_json::Value::Object(m) if m.is_empty() => SlotParams::Session,
      _ => return Err(format!("slot {id}: session params must be {{}}")),
    },
    ProviderKind::Passphrase => SlotParams::Passphrase(
      PassphraseParams::from_json(v)
        .map_err(|e| format!("slot {id}: bad passphrase params: {e}"))?,
    ),
    ProviderKind::Fido2Hmac => SlotParams::Fido2Hmac(
      Fido2Params::from_json(v).map_err(|e| format!("slot {id}: bad fido2-hmac params: {e}"))?,
    ),
    _ => SlotParams::Opaque(v),
  };
  check_params(kind, id, &params)?;
  Ok(params)
}

fn parse(path: &Path, bytes: &[u8]) -> Result<(DataKeyId, Vec<Slot>)> {
  // Version first, so a future format gets a clear error instead of
  // "unknown field".
  let v: serde_json::Value = serde_json::from_slice(bytes)
    .map_err(|e| Error::corrupt(path, format!("invalid JSON: {e}")))?;
  let version = v
    .get("version")
    .and_then(serde_json::Value::as_u64)
    .ok_or_else(|| Error::corrupt(path, "missing or invalid \"version\""))?;
  if version != FORMAT_VERSION {
    return Err(Error::UnsupportedVersion { path: path.to_path_buf(), version });
  }
  // Parse the bytes again (not the Value) so duplicate keys are rejected.
  let repr: FileRepr =
    serde_json::from_slice(bytes).map_err(|e| Error::corrupt(path, e.to_string()))?;
  if repr.slots.is_empty() {
    return Err(Error::corrupt(path, "no slots"));
  }
  if repr.slots.len() > MAX_SLOTS {
    return Err(Error::corrupt(path, format!("more than {MAX_SLOTS} slots")));
  }
  let mut seen = HashSet::new();
  let mut slots = Vec::with_capacity(repr.slots.len());
  for s in repr.slots {
    let id = SlotId(s.id);
    if !seen.insert(id) {
      return Err(Error::corrupt(path, format!("duplicate slot id {id}")));
    }
    let wrapped = B64
      .decode(s.wrapped.as_bytes())
      .map_err(|e| Error::corrupt(path, format!("slot {id}: bad base64 in \"wrapped\": {e}")))?;
    if wrapped.len() != WRAPPED_LEN {
      return Err(Error::corrupt(
        path,
        format!("slot {id}: \"wrapped\" must be {WRAPPED_LEN} bytes, got {}", wrapped.len()),
      ));
    }
    let params = parse_params(s.provider, &id, s.params).map_err(|e| Error::corrupt(path, e))?;
    slots.push(Slot { id, kind: s.provider, params, wrapped });
  }
  Ok((DataKeyId(repr.data_key_id), slots))
}

fn serialize(data_key_id: &DataKeyId, slots: &[Slot]) -> Vec<u8> {
  let repr = FileRepr {
    version: FORMAT_VERSION,
    data_key_id: data_key_id.0,
    slots: slots
      .iter()
      .map(|s| SlotRepr {
        id: s.id.0,
        provider: s.kind,
        params: s.params.to_json(),
        wrapped: B64.encode(&s.wrapped),
      })
      .collect(),
  };
  let mut out = serde_json::to_vec_pretty(&repr).expect("keyslots serialize");
  out.push(b'\n');
  out
}

fn find_provider<'a>(
  providers: &[&'a dyn KeyProvider],
  kind: ProviderKind,
) -> Option<&'a dyn KeyProvider> {
  providers.iter().copied().find(|p| p.kind() == kind)
}

fn missing_provider(kind: ProviderKind) -> Error {
  if kind.is_implemented() { Error::NoProvider(kind) } else { Error::NotImplemented(kind) }
}

/// The parsed `keyslots.json` plus its path.
///
/// Every mutating method persists the file before returning (temp file +
/// fsync + rename + fsync dir, mode 0600). There is no file locking: exactly
/// one process (`spoold`) is expected to own the file.
#[derive(Debug, Clone)]
pub struct KeySlots {
  path: PathBuf,
  data_key_id: DataKeyId,
  slots: Vec<Slot>,
}

impl KeySlots {
  /// `$XDG_STATE_HOME/spool/keyslots.json`, falling back to
  /// `$HOME/.local/state/spool/keyslots.json`.
  pub fn default_path() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
      .filter(|v| Path::new(v).is_absolute())
      .map(PathBuf::from)
      .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(state.join("spool").join(FILE_NAME))
  }

  /// Load and strictly validate an existing file (size limit, version,
  /// unknown fields, slot ids, wrapped lengths, params).
  /// Missing file -> [`Error::NotFound`].
  pub fn load(path: impl AsRef<Path>) -> Result<Self> {
    let path = path.as_ref();
    let bytes = fsutil::read_bounded(path, MAX_FILE_SIZE)?;
    let (data_key_id, slots) = parse(path, &bytes)?;
    Ok(KeySlots { path: path.to_path_buf(), data_key_id, slots })
  }

  /// Create a brand-new file: generates a random data key and data key id,
  /// enrolls `provider` as the first slot and writes the file. Refuses to
  /// overwrite an existing file. If writing fails, the freshly enrolled
  /// provider secret is destroyed again (best effort).
  pub async fn create_new(
    path: impl AsRef<Path>,
    provider: &dyn KeyProvider,
  ) -> Result<(Self, DataKey)> {
    let path = path.as_ref();
    if path.symlink_metadata().is_ok() {
      return Err(Error::AlreadyExists(path.to_path_buf()));
    }
    let data_key = wrap::random_key();
    let mut ks =
      KeySlots { path: path.to_path_buf(), data_key_id: DataKeyId::new(), slots: Vec::new() };
    let slot = ks.enroll_slot(provider, &data_key).await?;
    ks.slots.push(slot);
    if let Err(e) = ks.save() {
      let s = ks.slots.pop().expect("just pushed");
      if let Err(de) = provider.destroy(&s.id, &s.params).await {
        tracing::warn!(slot = %s.id, error = %de, "could not destroy provider secret after failed create");
      }
      return Err(e);
    }
    tracing::info!(slot = %ks.slots[0].id, provider = %provider.kind(), "created keyslots file");
    Ok((ks, data_key))
  }

  /// Path of the backing file.
  pub fn path(&self) -> &Path {
    &self.path
  }

  /// Current data key id.
  pub fn data_key_id(&self) -> DataKeyId {
    self.data_key_id
  }

  /// Slots in file order.
  pub fn slots(&self) -> &[Slot] {
    &self.slots
  }

  /// Look up a slot.
  pub fn slot(&self, id: &SlotId) -> Option<&Slot> {
    self.slots.iter().find(|s| s.id == *id)
  }

  fn save(&self) -> Result<()> {
    fsutil::write_atomic(&self.path, &serialize(&self.data_key_id, &self.slots))
  }

  async fn enroll_slot(&self, provider: &dyn KeyProvider, data_key: &[u8; 32]) -> Result<Slot> {
    let id = SlotId::new();
    let kind = provider.kind();
    let (params, kek) = provider.enroll(&id).await?;
    if let Err(reason) = check_params(kind, &id, &params) {
      let _ = provider.destroy(&id, &params).await;
      return Err(Error::Provider { kind, reason });
    }
    let wrapped = wrap::seal(&kek, data_key, &wrap::aad(&id, &self.data_key_id));
    Ok(Slot { id, kind, params, wrapped })
  }

  /// Add a slot for `provider`, wrapping `data_key`.
  ///
  /// `data_key` must be the key obtained from [`KeySlots::create_new`] /
  /// [`KeySlots::unlock_any`] for *this* file: the format has no key check
  /// value, so a wrong key would be wrapped faithfully and later "unlock"
  /// to garbage.
  pub async fn add_slot(
    &mut self,
    provider: &dyn KeyProvider,
    data_key: &[u8; 32],
  ) -> Result<SlotId> {
    if self.slots.len() >= MAX_SLOTS {
      return Err(Error::TooManySlots(MAX_SLOTS));
    }
    let slot = self.enroll_slot(provider, data_key).await?;
    let id = slot.id;
    self.slots.push(slot);
    if let Err(e) = self.save() {
      let s = self.slots.pop().expect("just pushed");
      let _ = provider.destroy(&s.id, &s.params).await;
      return Err(e);
    }
    tracing::info!(slot = %id, provider = %provider.kind(), "added key slot");
    Ok(id)
  }

  /// Remove slot `id` and destroy its provider secret.
  ///
  /// Refuses to remove the last slot unless `force` (which deletes the file:
  /// history becomes undecryptable). The file is updated first; if the
  /// provider secret then cannot be destroyed (no provider passed, wallet
  /// locked, ...) the slot is still gone from the file and
  /// [`Error::DestroyIncomplete`] is returned.
  pub async fn remove_slot(
    &mut self,
    id: &SlotId,
    providers: &[&dyn KeyProvider],
    force: bool,
  ) -> Result<()> {
    let idx = self.slots.iter().position(|s| s.id == *id).ok_or(Error::NoSuchSlot(*id))?;
    if self.slots.len() == 1 && !force {
      return Err(Error::LastSlot);
    }
    let slot = self.slots.remove(idx);
    let persisted =
      if self.slots.is_empty() { fsutil::remove_durable(&self.path) } else { self.save() };
    if let Err(e) = persisted {
      self.slots.insert(idx, slot);
      return Err(e);
    }
    tracing::info!(slot = %slot.id, provider = %slot.kind, "removed key slot");
    let res = match find_provider(providers, slot.kind) {
      None => Err(missing_provider(slot.kind)),
      Some(p) => p.destroy(&slot.id, &slot.params).await,
    };
    res.map_err(|error| Error::DestroyIncomplete {
      failures: vec![SlotAttempt { slot: slot.id, kind: slot.kind, error }],
    })
  }

  /// Unwrap the data key through one specific slot.
  pub async fn unlock_slot(&self, id: &SlotId, provider: &dyn KeyProvider) -> Result<DataKey> {
    let slot = self.slot(id).ok_or(Error::NoSuchSlot(*id))?;
    if provider.kind() != slot.kind {
      return Err(missing_provider(slot.kind));
    }
    let kek = provider.unlock(&slot.id, &slot.params).await?;
    wrap::open(&kek, &slot.wrapped, &wrap::aad(&slot.id, &self.data_key_id))
  }

  /// Try slots until one yields the data key.
  ///
  /// Order: slots whose provider is non-interactive first, then interactive
  /// ones, each group in file order; slots with no matching provider in
  /// `providers` are recorded as failed attempts. Pass only the providers you
  /// are willing to use right now (e.g. the daemon at startup passes a
  /// non-prompting Secret Service provider).
  ///
  /// On failure returns [`Error::NoSlotUnlocked`] listing every attempt; its
  /// [`Error::is_retryable`] is true iff some slot failed for a transient
  /// reason (locked / unavailable / dismissed). Wrong key or tampering
  /// ([`Error::WrongKey`]) is fatal for that slot.
  pub async fn unlock_any(&self, providers: &[&dyn KeyProvider]) -> Result<(DataKey, SlotId)> {
    let mut order: Vec<(u8, &Slot, Option<&dyn KeyProvider>)> = self
      .slots
      .iter()
      .map(|s| {
        let p = find_provider(providers, s.kind);
        let rank = match p {
          Some(p) if !p.interactive() => 0,
          Some(_) => 1,
          None => 2,
        };
        (rank, s, p)
      })
      .collect();
    order.sort_by_key(|(rank, _, _)| *rank); // stable: keeps file order within a rank

    let mut attempts = Vec::new();
    for (_, slot, provider) in order {
      let res = match provider {
        None => Err(missing_provider(slot.kind)),
        Some(p) => match p.unlock(&slot.id, &slot.params).await {
          Ok(kek) => wrap::open(&kek, &slot.wrapped, &wrap::aad(&slot.id, &self.data_key_id)),
          Err(e) => Err(e),
        },
      };
      match res {
        Ok(dk) => {
          tracing::info!(slot = %slot.id, provider = %slot.kind, "unlocked data key");
          return Ok((dk, slot.id));
        }
        Err(error) => {
          tracing::debug!(slot = %slot.id, provider = %slot.kind, %error, "key slot did not unlock");
          attempts.push(SlotAttempt { slot: slot.id, kind: slot.kind, error });
        }
      }
    }
    Err(Error::NoSlotUnlocked { attempts })
  }

  /// Re-key: wrap `new_data_key` in every slot under **fresh** KEKs, with a
  /// new data key id, then destroy the old provider secrets.
  ///
  /// Every slot gets a new slot id and a newly enrolled provider secret
  /// (e.g. a new keyring item); the old secrets are destroyed after the new
  /// file is durably written. Re-wrapping under the *old* KEKs would not be
  /// enough: on copy-on-write filesystems (btrfs, ZFS), snapshots or backups
  /// an old `keyslots.json` can survive, and old wrapped value + old KEK
  /// would still yield the old data key. Destroying the old KEKs makes those
  /// stale copies useless (they are also bound to the old data key id).
  ///
  /// Needs a provider for every slot's kind (checked before anything
  /// changes). If enrolling any slot fails, the new secrets enrolled so far
  /// are destroyed and the file is untouched. If destroying an old secret
  /// fails, the file is already rotated and [`Error::DestroyIncomplete`] is
  /// returned.
  ///
  /// Crash safety for the caller: the data key in the history DB must change
  /// in step with this file. Recommended order: (1) `rotate`, (2) re-key the
  /// DB with the new key (keep the old key in memory until it commits).
  /// A crash between (1) and (2) leaves a DB that only the old (now
  /// destroyed) key opens, so the daemon should do both without yielding.
  pub async fn rotate(
    &mut self,
    new_data_key: &[u8; 32],
    providers: &[&dyn KeyProvider],
  ) -> Result<()> {
    let mut plan = Vec::with_capacity(self.slots.len());
    for s in &self.slots {
      plan.push(find_provider(providers, s.kind).ok_or_else(|| missing_provider(s.kind))?);
    }
    let new_id = DataKeyId::new();
    let mut staged = KeySlots { path: self.path.clone(), data_key_id: new_id, slots: Vec::new() };
    let mut new_slots: Vec<(Slot, &dyn KeyProvider)> = Vec::new();
    for p in &plan {
      match staged.enroll_slot(*p, new_data_key).await {
        Ok(slot) => new_slots.push((slot, *p)),
        Err(e) => {
          for (s, p) in &new_slots {
            let _ = p.destroy(&s.id, &s.params).await;
          }
          return Err(e);
        }
      }
    }
    staged.slots = new_slots.iter().map(|(s, _)| s.clone()).collect();
    if let Err(e) = staged.save() {
      for (s, p) in &new_slots {
        let _ = p.destroy(&s.id, &s.params).await;
      }
      return Err(e);
    }
    let old = std::mem::replace(self, staged);
    tracing::info!(data_key_id = %self.data_key_id, "rotated data key");

    let mut failures = Vec::new();
    for (s, p) in old.slots.iter().zip(plan) {
      if let Err(error) = p.destroy(&s.id, &s.params).await {
        failures.push(SlotAttempt { slot: s.id, kind: s.kind, error });
      }
    }
    if failures.is_empty() { Ok(()) } else { Err(Error::DestroyIncomplete { failures }) }
  }

  /// Destroy every slot's provider secret, then delete `keyslots.json`.
  ///
  /// The file is deleted even if some secrets could not be destroyed (no
  /// provider, wallet locked, ...); those are reported in
  /// [`Error::DestroyIncomplete`]. Orphaned Secret Service items can be
  /// cleaned up later with
  /// [`crate::SecretServiceProvider::destroy_all`].
  pub async fn wipe(self, providers: &[&dyn KeyProvider]) -> Result<()> {
    let mut failures = Vec::new();
    for s in &self.slots {
      let res = match find_provider(providers, s.kind) {
        None => Err(missing_provider(s.kind)),
        Some(p) => p.destroy(&s.id, &s.params).await,
      };
      if let Err(error) = res {
        failures.push(SlotAttempt { slot: s.id, kind: s.kind, error });
      }
    }
    fsutil::remove_durable(&self.path)?;
    tracing::info!(path = %self.path.display(), "wiped keyslots");
    if failures.is_empty() { Ok(()) } else { Err(Error::DestroyIncomplete { failures }) }
  }
}

#[cfg(test)]
mod tests;
