//! Key flow: obtain the history data key from a key slot and open the
//! encrypted store, without ever blocking capture.
//!
//! The daemon starts on an in-memory session store and captures right away.
//! [`run`] (its own task) then:
//!
//! 1. `keyslots.json` missing (first run) -> [`KeySlots::create_new`] with
//!    the configured provider, then [`Store::open_encrypted`] (fresh DB).
//!    If a `history.db` already exists without key slots, that is fatal (the
//!    history cannot be decrypted; it is never deleted automatically).
//! 2. `keyslots.json` present -> try each slot of the provider's kind:
//!    unwrap the data key, then validate it by opening the store
//!    ([`spool_core::Error::WrongKey`] -> try the remaining slots).
//! 3. A retryable error (wallet locked / not running / prompt dismissed) ->
//!    report [`KeyEvent::Waiting`], log once, wait with
//!    [`KeySource::wait_for_unlock`] (never prompts) and try again. A short
//!    backoff guards against a provider that claims to be unlocked while
//!    unlocking still fails.
//! 4. Fatal errors (wrong key, corrupt slot file or DB, plaintext DB at the
//!    encrypted path, an interrupted `spool-keyctl rotate` that left
//!    `keyslots.json.next` / `.old` behind ([`interrupted_rotation`]), ...) -> [`KeyEvent::Failed`]; the daemon stays on the
//!    session store. Nothing is wiped.
//! 5. Success -> [`KeyEvent::Opened`] with the open store and its dedupe hash
//!    key; the orchestrator merges the session into it and swaps stores.
//!
//! The picker's unlock panel ([`crate::unlock`]) opens the store through the
//! same [`OpenGate`]: whoever opens it first wins, every later open (the
//! wallet unlocking after a passphrase did, say) stops without touching the
//! database (a second `open_encrypted` would run its orphan-blob GC under the
//! live store).
//!
//! Never logs key material or clipboard content.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use spool_core::store::{DB_FILE_NAME, Store};
use spool_crypto::{DataKey, Zeroizing};
use spool_keys::{KeyProvider, KeySlots, SecretServiceProvider};
use tokio::sync::mpsc;

/// Boxed `Send` future (the [`KeySource`] trait must be dyn-compatible).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Shortest pause between two attempts when the wait returned immediately.
const MIN_BACKOFF: Duration = Duration::from_secs(1);
/// Longest such pause.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// A key provider plus a way to wait until it may work.
pub trait KeySource: Send + Sync + 'static {
  /// The provider used to create / unlock slots (must not prompt).
  fn provider(&self) -> &dyn KeyProvider;
  /// Resolve once a retry may succeed (e.g. the wallet was unlocked).
  /// Must not prompt and must not return immediately while nothing changed
  /// (the caller backs off anyway, but should not have to).
  fn wait_for_unlock(&self) -> BoxFuture<'_, spool_keys::Result<()>>;
}

/// The production source: a non-prompting Secret Service provider.
#[derive(Debug, Clone)]
pub struct SecretServiceSource(SecretServiceProvider);

impl Default for SecretServiceSource {
  fn default() -> Self {
    Self(SecretServiceProvider::new(false))
  }
}

impl KeySource for SecretServiceSource {
  fn provider(&self) -> &dyn KeyProvider {
    &self.0
  }

  fn wait_for_unlock(&self) -> BoxFuture<'_, spool_keys::Result<()>> {
    Box::pin(self.0.wait_for_unlock(None))
  }
}

/// The persistent store, opened and ready to take over.
pub struct OpenedStore {
  pub store: Store,
  /// Its dedupe hash key (compiles the new `Policy`).
  pub hash_key: Zeroizing<[u8; 32]>,
  /// The history data key (keys the on-disk search index, `Label::Index`).
  pub data_key: DataKey,
  /// The state directory the store lives in (the search index goes to
  /// `<dir>/index`).
  pub dir: PathBuf,
}

impl std::fmt::Debug for OpenedStore {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("OpenedStore").field("store", &self.store).finish_non_exhaustive()
  }
}

/// Progress reports from [`run`] to the orchestrator.
#[derive(Debug)]
pub enum KeyEvent {
  /// The key store is locked or unavailable; waiting for it.
  Waiting { reason: String },
  /// A data key was obtained; opening the store.
  Unlocking,
  /// Done: merge the session into this store and switch to it.
  Opened(Box<OpenedStore>),
  /// Gave up (fatal); stay on the session store.
  Failed(String),
}

enum Outcome {
  Opened(Box<OpenedStore>),
  Retry(spool_keys::Error),
  Fatal(String),
  /// Someone else (the picker's unlock) opened the store first.
  Superseded,
}

/// Serializes opening the encrypted store and lets it happen once per
/// process run (see the module docs). Cheap to clone.
#[derive(Clone, Default)]
pub struct OpenGate(Arc<tokio::sync::Mutex<bool>>);

/// Why [`OpenGate::open`] did not open the store.
#[derive(Debug)]
pub enum GateError {
  /// Already opened through this gate.
  AlreadyOpen,
  Store(spool_core::Error),
}

impl OpenGate {
  /// Open the encrypted store in `dir` with `key` unless it was already
  /// opened through this gate. Only a successful open closes the gate.
  pub async fn open(&self, dir: &Path, key: spool_keys::DataKey) -> Result<OpenedStore, GateError> {
    let mut opened = self.0.lock().await;
    if *opened {
      return Err(GateError::AlreadyOpen);
    }
    let store = open(dir, key).await.map_err(GateError::Store)?;
    *opened = true;
    Ok(store)
  }
}

/// Run the key flow for the encrypted store in `dir` (see the module docs).
/// Returns after sending `Opened` or `Failed`, or when the receiver is gone.
/// Tests only; the daemon uses [`run_gated`] (shared with the picker).
#[cfg(test)]
pub async fn run(source: Arc<dyn KeySource>, dir: PathBuf, tx: mpsc::Sender<KeyEvent>) {
  run_gated(source, dir, tx, OpenGate::default()).await
}

/// [`run`], sharing `gate` with the picker's unlock path. Also returns
/// (quietly) once the store was opened through the gate by someone else.
pub async fn run_gated(
  source: Arc<dyn KeySource>,
  dir: PathBuf,
  tx: mpsc::Sender<KeyEvent>,
  gate: OpenGate,
) {
  let mut announced = false;
  let mut backoff = MIN_BACKOFF;
  loop {
    match attempt(source.as_ref(), &dir, &tx, &gate).await {
      Outcome::Superseded => {
        tracing::debug!("key flow: history already unlocked another way");
        return;
      }
      Outcome::Opened(opened) => {
        let _ = tx.send(KeyEvent::Opened(opened)).await;
        return;
      }
      Outcome::Fatal(msg) => {
        tracing::error!(
          "cannot unlock the encrypted history: {msg}. Clipboard history is kept in memory only \
           until this is fixed and spoold is restarted (nothing was deleted)"
        );
        let _ = tx.send(KeyEvent::Failed(msg)).await;
        return;
      }
      Outcome::Retry(e) => {
        if !announced {
          tracing::info!(
            "key store not available yet ({e}); capturing into memory and waiting for it to unlock"
          );
          announced = true;
        } else {
          tracing::debug!("key store still not available ({e}); waiting again");
        }
        if tx.send(KeyEvent::Waiting { reason: e.to_string() }).await.is_err() {
          return;
        }
        let started = tokio::time::Instant::now();
        match source.wait_for_unlock().await {
          Ok(()) => {}
          Err(e) if e.is_retryable() => {}
          Err(e) => {
            let msg = format!("waiting for the key store failed: {e}");
            tracing::error!("{msg}; clipboard history is kept in memory only");
            let _ = tx.send(KeyEvent::Failed(msg)).await;
            return;
          }
        }
        // The wait said "go" right away but the last attempt failed anyway
        // (e.g. the collection is open but the item is not): back off so
        // this never becomes a busy loop.
        if started.elapsed() < backoff {
          tokio::time::sleep(backoff).await;
          backoff = (backoff * 2).min(MAX_BACKOFF);
        } else {
          backoff = MIN_BACKOFF;
        }
      }
    }
  }
}

async fn attempt(
  source: &dyn KeySource,
  dir: &Path,
  tx: &mpsc::Sender<KeyEvent>,
  gate: &OpenGate,
) -> Outcome {
  if interrupted_rotation(dir) {
    return Outcome::Fatal(INTERRUPTED_ROTATION.into());
  }
  let path = dir.join(spool_keys::FILE_NAME);
  match KeySlots::load(&path) {
    Err(spool_keys::Error::NotFound(_)) => first_run(source, dir, &path, tx, gate).await,
    Err(e) => Outcome::Fatal(format!("cannot read {}: {e}", path.display())),
    Ok(slots) => unlock_existing(source, dir, &slots, tx, gate).await,
  }
}

async fn first_run(
  source: &dyn KeySource,
  dir: &Path,
  path: &Path,
  tx: &mpsc::Sender<KeyEvent>,
  gate: &OpenGate,
) -> Outcome {
  let db = dir.join(DB_FILE_NAME);
  if db.symlink_metadata().is_ok() {
    return Outcome::Fatal(format!(
      "{} exists but {} is missing, so the history cannot be decrypted; move the database away \
       to start a new history",
      db.display(),
      path.display()
    ));
  }
  match KeySlots::create_new(path, source.provider()).await {
    Ok((_slots, key)) => {
      tracing::info!(provider = %source.provider().kind(), "created a new history key");
      let _ = tx.send(KeyEvent::Unlocking).await;
      match gate.open(dir, key).await {
        Ok(opened) => Outcome::Opened(Box::new(opened)),
        Err(GateError::AlreadyOpen) => Outcome::Superseded,
        Err(GateError::Store(e)) => {
          Outcome::Fatal(format!("opening the new encrypted history: {e}"))
        }
      }
    }
    Err(e) if e.is_retryable() => Outcome::Retry(e),
    Err(e) => Outcome::Fatal(format!("creating the history key: {e}")),
  }
}

async fn unlock_existing(
  source: &dyn KeySource,
  dir: &Path,
  slots: &KeySlots,
  tx: &mpsc::Sender<KeyEvent>,
  gate: &OpenGate,
) -> Outcome {
  let provider = source.provider();
  let mut retry: Option<spool_keys::Error> = None;
  let mut failures: Vec<String> = Vec::new();
  for slot in slots.slots() {
    if slot.kind() != provider.kind() {
      failures.push(format!("slot {} ({}): provider not configured", slot.id(), slot.kind()));
      continue;
    }
    match slots.unlock_slot(&slot.id(), provider).await {
      Err(e) if e.is_retryable() => {
        tracing::debug!(slot = %slot.id(), "key slot not available: {e}");
        retry.get_or_insert(e);
      }
      Err(e) => {
        tracing::warn!(slot = %slot.id(), "key slot failed: {e}");
        failures.push(format!("slot {}: {e}", slot.id()));
      }
      Ok(key) => {
        let _ = tx.send(KeyEvent::Unlocking).await;
        match gate.open(dir, key).await {
          Ok(opened) => {
            tracing::info!(slot = %slot.id(), provider = %slot.kind(), "history unlocked");
            return Outcome::Opened(Box::new(opened));
          }
          Err(GateError::AlreadyOpen) => return Outcome::Superseded,
          // keyslots.json has no key check value: a slot can unwrap a key
          // that is not this history's. Try the others.
          Err(GateError::Store(spool_core::Error::WrongKey)) => {
            tracing::warn!(slot = %slot.id(), "key slot yields a key that does not open the history");
            failures.push(format!("slot {}: key does not open the history", slot.id()));
          }
          Err(GateError::Store(e)) => {
            return Outcome::Fatal(format!("opening the encrypted history: {e}"));
          }
        }
      }
    }
  }
  match retry {
    Some(e) => Outcome::Retry(e),
    None if failures.is_empty() => Outcome::Fatal("keyslots.json has no key slots".into()),
    None => Outcome::Fatal(format!("no key slot could be unlocked ({})", failures.join("; "))),
  }
}

/// Key state reason while `spool-keyctl rotate` is unfinished.
pub const INTERRUPTED_ROTATION: &str = "interrupted key rotation — run spool-keyctl recover";

/// True if `spool-keyctl rotate` was interrupted in `dir`: a staged
/// `keyslots.json.next` or a not yet cleaned-up `keyslots.json.old` exists.
/// The history may already be re-encrypted under the staged key, and
/// creating or unlocking slots now could make the rotation unrecoverable,
/// so spoold neither creates nor unlocks anything until `spool-keyctl
/// recover` ran (capture stays in memory).
pub fn interrupted_rotation(dir: &Path) -> bool {
  [".next", ".old"]
    .iter()
    .any(|suffix| dir.join(format!("{}{suffix}", spool_keys::FILE_NAME)).symlink_metadata().is_ok())
}

/// Turn off SQLCipher's own logging (process-global; any connection will
/// do). Otherwise every wrong-key open prints "hmac check failed" lines to
/// stderr; the condition is reported as `Error::WrongKey` anyway.
pub fn quiet_sqlcipher(store: &Store) {
  if let Err(e) = store.conn().execute_batch("PRAGMA cipher_log_level = NONE;") {
    tracing::debug!("could not lower the SQLCipher log level: {e}");
  }
}

/// Open the encrypted store with `key` on a blocking thread.
async fn open(dir: &Path, key: spool_keys::DataKey) -> spool_core::Result<OpenedStore> {
  let dir = dir.to_path_buf();
  let key = DataKey::from_bytes(key);
  tokio::task::spawn_blocking(move || {
    let mut store = Store::open_encrypted(&dir, &key)?;
    let hash_key = Zeroizing::new(store.hash_key()?);
    Ok(OpenedStore { store, hash_key, data_key: key, dir })
  })
  .await
  .unwrap_or_else(|e| Err(spool_core::Error::Io(std::io::Error::other(format!("open task: {e}")))))
}
