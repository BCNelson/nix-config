//! Tests: temp state dirs, a fake Secret Service (in-memory, shared across
//! simulated crashes like a real keyring), the real passphrase provider.
//! Nothing here touches the user's keyring, security keys or state dir.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use spool_core::item::{ItemFlags, NewItem, Representation, Selection, dedupe_hash};
use spool_core::store::Store;
use spool_crypto::DataKey;
use spool_keys::{
  Error as KeysError, Fido2Device, Fido2Provider, Kek, KeyProvider, KeySlots, PassphraseProvider,
  ProviderKind, SlotId, SlotParams, slot_attributes,
};
use zeroize::Zeroizing;

use crate::ops::{self, AddKind, Pending};
use crate::providers::{Fido2Opts, Providers};
use crate::ui::{PromptError, Ui};
use crate::{CrashPoint, Ctx, DaemonLock, KeyctlError, LockError, exit};

// ---- fakes ----------------------------------------------------------------------

#[derive(Default)]
struct ScriptUi {
  answers: Mutex<VecDeque<String>>,
  out: Mutex<Vec<String>>,
  notes: Mutex<Vec<String>>,
}

impl ScriptUi {
  fn new(answers: &[&str]) -> Self {
    let ui = ScriptUi::default();
    ui.push(answers);
    ui
  }
  fn push(&self, answers: &[&str]) {
    self.answers.lock().unwrap().extend(answers.iter().map(|s| s.to_string()));
  }
  fn next(&self, prompt: &str) -> Result<String, PromptError> {
    self.notes.lock().unwrap().push(format!("PROMPT {prompt}"));
    self.answers.lock().unwrap().pop_front().ok_or(PromptError::Aborted)
  }
  fn out_text(&self) -> String {
    self.out.lock().unwrap().join("\n")
  }
  fn all_text(&self) -> String {
    format!("{}\n{}", self.out_text(), self.notes.lock().unwrap().join("\n"))
  }
  fn remaining(&self) -> usize {
    self.answers.lock().unwrap().len()
  }
}

impl Ui for ScriptUi {
  fn out(&self, line: &str) {
    self.out.lock().unwrap().push(line.to_string());
  }
  fn note(&self, line: &str) {
    self.notes.lock().unwrap().push(line.to_string());
  }
  fn secret(&self, prompt: &str) -> Result<Zeroizing<String>, PromptError> {
    self.next(prompt).map(Zeroizing::new)
  }
  fn line(&self, prompt: &str) -> Result<String, PromptError> {
    self.next(prompt)
  }
}

#[derive(Default)]
struct SsState {
  items: HashMap<SlotId, [u8; 32]>,
  locked: bool,
}

/// In-memory stand-in for the Secret Service. Survives "crashes" (the state
/// is shared), like a real keyring would.
#[derive(Clone, Default)]
struct FakeSs(Arc<Mutex<SsState>>);

impl FakeSs {
  fn ids(&self) -> Vec<SlotId> {
    let mut v: Vec<SlotId> = self.0.lock().unwrap().items.keys().copied().collect();
    v.sort();
    v
  }
  fn set_locked(&self, l: bool) {
    self.0.lock().unwrap().locked = l;
  }
}

#[async_trait]
impl KeyProvider for FakeSs {
  fn kind(&self) -> ProviderKind {
    ProviderKind::SecretService
  }
  fn interactive(&self) -> bool {
    false
  }
  async fn enroll(&self, id: &SlotId) -> spool_keys::Result<(SlotParams, Kek)> {
    let mut st = self.0.lock().unwrap();
    if st.locked {
      return Err(KeysError::Locked(ProviderKind::SecretService));
    }
    let kek = *DataKey::generate().expose();
    st.items.insert(*id, kek);
    Ok((SlotParams::SecretService { attributes: slot_attributes(id) }, Zeroizing::new(kek)))
  }
  async fn unlock(&self, id: &SlotId, _p: &SlotParams) -> spool_keys::Result<Kek> {
    let st = self.0.lock().unwrap();
    if st.locked {
      return Err(KeysError::Locked(ProviderKind::SecretService));
    }
    st.items
      .get(id)
      .map(|k| Zeroizing::new(*k))
      .ok_or(KeysError::SecretMissing(ProviderKind::SecretService))
  }
  async fn destroy(&self, id: &SlotId, _p: &SlotParams) -> spool_keys::Result<()> {
    let mut st = self.0.lock().unwrap();
    if st.locked {
      return Err(KeysError::Locked(ProviderKind::SecretService));
    }
    st.items.remove(id);
    Ok(())
  }
}

struct FakeProviders {
  ss: FakeSs,
}

#[async_trait]
impl Providers for FakeProviders {
  fn secret_service(&self) -> Box<dyn KeyProvider> {
    Box::new(self.ss.clone())
  }
  fn passphrase(&self, p: Option<Zeroizing<String>>) -> Box<dyn KeyProvider> {
    Box::new(match p {
      Some(p) => PassphraseProvider::new(p),
      None => PassphraseProvider::without_passphrase(),
    })
  }
  fn fido2(&self, _opts: Fido2Opts) -> Box<dyn KeyProvider> {
    // Never used with a device here: no FIDO2 slots in these tests, and
    // `fido2_devices` reports none.
    Box::new(Fido2Provider::new(None))
  }
  async fn fido2_devices(&self) -> spool_keys::Result<Vec<Fido2Device>> {
    Ok(Vec::new())
  }
  async fn secret_service_destroy_all(&self) -> spool_keys::Result<usize> {
    let mut st = self.ss.0.lock().unwrap();
    let n = st.items.len();
    st.items.clear();
    Ok(n)
  }
}

// ---- fixture ----------------------------------------------------------------------

struct Fx {
  _tmp: tempfile::TempDir,
  state: PathBuf,
  socket: PathBuf,
  providers: FakeProviders,
}

const BIG: usize = 1024 * 1024 + 4096; // > INLINE_MAX: stored as a blob file

fn t(secs: u64) -> SystemTime {
  SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + secs)
}

fn add_items(store: &mut Store) {
  let hk = store.hash_key().unwrap();
  for (i, text) in ["alpha", "bravo", "charlie"].iter().enumerate() {
    let canon = Representation::new("text/plain;charset=utf-8", text.as_bytes().to_vec());
    store
      .insert(NewItem {
        selection: Selection::Clipboard,
        source_app: None,
        created_at: t(i as u64),
        flags: ItemFlags::empty(),
        hash: dedupe_hash(&hk, &canon),
        preview: Some(text.to_string()),
        reps: vec![canon],
      })
      .unwrap();
  }
  let canon = Representation::new("image/png", vec![7u8; BIG]);
  store
    .insert(NewItem {
      selection: Selection::Clipboard,
      source_app: None,
      created_at: t(10),
      flags: ItemFlags::empty(),
      hash: dedupe_hash(&hk, &canon),
      preview: Some("[image]".into()),
      reps: vec![canon],
    })
    .unwrap();
}

/// Check the store holds exactly the fixture items (blob included).
fn assert_items(store: &Store) {
  let recent = store.recent(10).unwrap();
  assert_eq!(recent.len(), 4, "items lost");
  let img = store.latest(Selection::Clipboard).unwrap().unwrap();
  assert_eq!(img.reps[0].data.len(), BIG);
}

impl Fx {
  /// A state dir with `n_ss` Secret Service slots, a history with items
  /// (one blob) and a fake index directory.
  async fn new(n_ss: usize) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let socket = tmp.path().join("run/spool/sock");
    let providers = FakeProviders { ss: FakeSs::default() };
    let (mut ks, key) =
      KeySlots::create_new(state.join(spool_keys::FILE_NAME), &providers.ss).await.unwrap();
    for _ in 1..n_ss {
      ks.add_slot(&providers.ss, &key).await.unwrap();
    }
    let mut store = Store::open_encrypted(&state, &DataKey::from_bytes(key)).unwrap();
    add_items(&mut store);
    drop(store);
    std::fs::create_dir(state.join("index")).unwrap();
    std::fs::write(state.join("index/meta.json"), b"x").unwrap();
    Fx { _tmp: tmp, state, socket, providers }
  }

  fn ctx<'a>(&'a self, ui: &'a ScriptUi) -> Ctx<'a> {
    Ctx::new(self.state.clone(), self.socket.clone(), ui, &self.providers)
  }

  fn slots(&self) -> KeySlots {
    KeySlots::load(self.state.join(spool_keys::FILE_NAME)).unwrap()
  }

  /// Unlock keyslots.json non-interactively through the fake wallet and
  /// open the store with it.
  async fn open_with_main(&self) -> Store {
    let ks = self.slots();
    let (k, _) = ks.unlock_any(&[&self.providers.ss]).await.unwrap();
    Store::open_encrypted(&self.state, &DataKey::from_bytes(k)).unwrap()
  }

  fn exists(&self, name: &str) -> bool {
    self.state.join(name).symlink_metadata().is_ok()
  }
}

fn path_exists(p: &Path) -> bool {
  p.symlink_metadata().is_ok()
}

// ---- status -----------------------------------------------------------------------

#[tokio::test]
async fn status_lists_slots_without_secrets() {
  let fx = Fx::new(2).await;
  let ui = ScriptUi::new(&[]);
  ops::status(&fx.ctx(&ui)).await.unwrap();
  let out = ui.out_text();
  let ks = fx.slots();
  assert!(out.contains(&ks.data_key_id().to_string()));
  for s in ks.slots() {
    assert!(out.contains(&s.id().to_string()));
  }
  assert!(out.contains("secret-service"));
  assert!(out.contains("history.db:   present"));
  assert!(out.contains("search index: present"));
  assert!(out.contains("spoold:       not running"));
  // The wrapped keys never appear.
  let raw: serde_json_free::Raw = serde_json_free::wrapped_values(&fx.state);
  for w in raw.0 {
    assert!(!out.contains(&w));
  }
  // Status must not create anything (no lock files, no socket dir).
  assert!(!path_exists(fx.socket.parent().unwrap()));
  assert!(!fx.exists("index.lock"));
}

/// Tiny extraction of the `wrapped` strings from keyslots.json without a
/// JSON dependency.
mod serde_json_free {
  use std::path::Path;
  pub struct Raw(pub Vec<String>);
  pub fn wrapped_values(state: &Path) -> Raw {
    let s = std::fs::read_to_string(state.join("keyslots.json")).unwrap();
    Raw(
      s.lines()
        .filter_map(|l| l.trim().strip_prefix("\"wrapped\": \""))
        .map(|v| v.trim_end_matches(['"', ',']).to_string())
        .collect(),
    )
  }
}

#[tokio::test]
async fn status_on_missing_state_dir() {
  let tmp = tempfile::tempdir().unwrap();
  let providers = FakeProviders { ss: FakeSs::default() };
  let ui = ScriptUi::new(&[]);
  let ctx = Ctx::new(tmp.path().join("nope"), tmp.path().join("sock"), &ui, &providers);
  ops::status(&ctx).await.unwrap();
  assert!(ui.out_text().contains("does not exist"));
  assert!(!path_exists(&tmp.path().join("nope")));
}

// ---- lock -------------------------------------------------------------------------

#[tokio::test]
async fn refuses_while_spoold_holds_the_socket_lock() {
  let fx = Fx::new(1).await;
  // A fake "spoold": the same flock spoold's ipc::bind takes.
  let daemon = DaemonLock::acquire(&fx.socket, None).unwrap();
  let ui = ScriptUi::new(&[]);
  let ctx = fx.ctx(&ui);
  for res in [
    ops::add(&ctx, AddKind::SecretService).await,
    ops::remove(&ctx, fx.slots().slots()[0].id(), true).await,
    ops::rotate(&ctx, false).await,
    ops::recover(&ctx).await,
    ops::wipe(&ctx).await,
  ] {
    let e = res.unwrap_err();
    assert!(matches!(e, KeyctlError::Lock(LockError::Held(_))), "{e:?}");
    assert_eq!(e.exit_code(), exit::DAEMON_RUNNING);
    assert!(e.to_string().contains("systemctl --user stop spool"));
  }
  assert_eq!(fx.providers.ss.ids().len(), 1, "nothing changed");
  assert!(fx.exists("history.db"));
  // Status still works and reports the daemon.
  ops::status(&ctx).await.unwrap();
  assert!(ui.out_text().contains("spoold:       running"));
  drop(daemon);
  // Released: works again.
  ops::add(&ctx, AddKind::SecretService).await.unwrap();
  assert_eq!(fx.slots().slots().len(), 2);
}

#[tokio::test]
async fn refuses_while_the_index_lock_is_held() {
  let fx = Fx::new(1).await;
  // spool-search's writer lock beside the index dir (spoold with the
  // on-disk index open, e.g. on another socket path).
  let f = std::fs::OpenOptions::new()
    .create(true)
    .truncate(false)
    .write(true)
    .open(fx.state.join("index.lock"))
    .unwrap();
  rustix::fs::flock(&f, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
  let ui = ScriptUi::new(&[]);
  let e = ops::add(&fx.ctx(&ui), AddKind::SecretService).await.unwrap_err();
  assert_eq!(e.exit_code(), exit::DAEMON_RUNNING, "{e}");
}

#[tokio::test]
async fn refuses_while_the_state_lock_is_held() {
  let fx = Fx::new(1).await;
  // spoold's state-directory lock (a daemon started with another socket
  // path still holds it).
  let f = std::fs::OpenOptions::new()
    .create(true)
    .truncate(false)
    .write(true)
    .open(fx.state.join(spool_core::store::STATE_LOCK_FILE_NAME))
    .unwrap();
  rustix::fs::flock(&f, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
  let ui = ScriptUi::new(&[]);
  let ctx = fx.ctx(&ui);
  let e = ops::add(&ctx, AddKind::SecretService).await.unwrap_err();
  assert_eq!(e.exit_code(), exit::DAEMON_RUNNING, "{e}");
  assert!(e.to_string().contains("state.lock"), "{e}");
  ops::status(&ctx).await.unwrap();
  assert!(ui.out_text().contains("spoold:       running"));
}

#[tokio::test]
async fn keyctl_holds_the_lock_while_running() {
  let fx = Fx::new(1).await;
  let ui = ScriptUi::new(&[]);
  let ctx = fx.ctx(&ui);
  let held = ctx.lock().unwrap();
  // A daemon starting now would fail its single-instance check ...
  assert!(matches!(DaemonLock::acquire(&fx.socket, None), Err(LockError::Held(_))));
  // ... and, on any other socket path, its state-directory lock.
  let other = fx.state.join("other.sock");
  assert!(
    matches!(DaemonLock::acquire(&other, Some(&fx.state)), Err(LockError::Held(p)) if p.ends_with("state.lock"))
  );
  drop(held);
  DaemonLock::acquire(&fx.socket, None).unwrap();
  DaemonLock::acquire(&other, Some(&fx.state)).unwrap();
}

// ---- add / remove -----------------------------------------------------------------

#[tokio::test]
async fn add_secret_service_slot() {
  let fx = Fx::new(1).await;
  let ui = ScriptUi::new(&[]);
  ops::add(&fx.ctx(&ui), AddKind::SecretService).await.unwrap();
  let ks = fx.slots();
  assert_eq!(ks.slots().len(), 2);
  assert_eq!(fx.providers.ss.ids().len(), 2);
  // Both slots unwrap the same (DB-opening) key.
  for s in ks.slots() {
    let k = ks.unlock_slot(&s.id(), &fx.providers.ss).await.unwrap();
    assert_items(&Store::open_encrypted(&fx.state, &DataKey::from_bytes(k)).unwrap());
  }
  assert!(ui.out_text().contains("added slot"));
}

#[tokio::test]
async fn unlock_fails_when_no_slot_opens() {
  let fx = Fx::new(1).await;
  fx.providers.ss.set_locked(true);
  let ui = ScriptUi::new(&[]);
  let e = ops::add(&fx.ctx(&ui), AddKind::SecretService).await.unwrap_err();
  assert_eq!(e.exit_code(), exit::UNLOCK_FAILED, "{e}");
  assert_eq!(fx.slots().slots().len(), 1);
}

#[tokio::test]
async fn stale_slot_is_skipped_by_db_validation() {
  // keyslots.json whose only slot unwraps a key that does not open the DB.
  let fx = Fx::new(1).await;
  let ks_path = fx.state.join(spool_keys::FILE_NAME);
  std::fs::remove_file(&ks_path).unwrap();
  KeySlots::create_new(&ks_path, &fx.providers.ss).await.unwrap();
  let ui = ScriptUi::new(&[]);
  let e = ops::add(&fx.ctx(&ui), AddKind::SecretService).await.unwrap_err();
  assert_eq!(e.exit_code(), exit::UNLOCK_FAILED);
  assert!(ui.all_text().contains("does not open"));
}

#[tokio::test]
async fn remove_slot_destroys_secret_and_guards_last_slot() {
  let fx = Fx::new(2).await;
  let ks = fx.slots();
  let (a, b) = (ks.slots()[0].id(), ks.slots()[1].id());
  let ui = ScriptUi::new(&[]);
  let ctx = fx.ctx(&ui);
  ops::remove(&ctx, a, false).await.unwrap();
  assert_eq!(fx.providers.ss.ids(), vec![b]);
  assert_eq!(fx.slots().slots().len(), 1);

  // Last slot: refused without --force, before any unlock.
  let e = ops::remove(&ctx, b, false).await.unwrap_err();
  assert!(e.to_string().contains("only key slot"), "{e}");
  // With --force: needs the typed confirmation.
  ui.push(&["nah"]);
  let e = ops::remove(&ctx, b, true).await.unwrap_err();
  assert_eq!(e.exit_code(), exit::ABORTED);
  assert_eq!(fx.slots().slots().len(), 1);
  ui.push(&["remove"]);
  ops::remove(&ctx, b, true).await.unwrap();
  assert!(!fx.exists("keyslots.json"));
  assert!(fx.providers.ss.ids().is_empty());
  assert!(ui.out_text().contains("spool-keyctl wipe"));

  // Unknown slot.
  let e = ops::remove(&ctx, SlotId::new(), false).await.unwrap_err();
  assert_eq!(e.exit_code(), exit::ERROR);
}

#[tokio::test]
async fn unlock_slot_option_restricts_unlock() {
  let fx = Fx::new(2).await;
  let id = fx.slots().slots()[1].id();
  let ui = ScriptUi::new(&[]);
  let mut ctx = fx.ctx(&ui);
  ctx.unlock_slot = Some(SlotId::new());
  let e = ops::add(&ctx, AddKind::SecretService).await.unwrap_err();
  assert!(e.to_string().contains("no key slot"), "{e}");
  ctx.unlock_slot = Some(id);
  ops::remove(&ctx, id, false).await.unwrap();
  assert_eq!(fx.slots().slots().len(), 1);
  assert_eq!(
    crate::keys_err(KeysError::DestroyIncomplete { failures: vec![] }).exit_code(),
    exit::INCOMPLETE
  );
}

/// Real Argon2id at full cost (256 MiB): slow in debug builds, so one test
/// covers add + unlock + remove for passphrases.
#[tokio::test(flavor = "multi_thread")]
async fn passphrase_slot_add_unlock_remove() {
  let fx = Fx::new(1).await;
  let ui = ScriptUi::new(&[
    "short",                  // rejected: < 12 chars
    "correct horse battery",  // ok
    "correct horse batteryX", // mismatch
    "correct horse battery",  // again
    "correct horse battery",  // repeat
  ]);
  let ctx = fx.ctx(&ui);
  ops::add(&ctx, AddKind::Passphrase { allow_short: false }).await.unwrap();
  assert_eq!(ui.remaining(), 0);
  let text = ui.all_text();
  assert!(text.contains("Too short"));
  assert!(text.contains("do not match"));
  assert!(!text.contains("correct horse"), "secret echoed in output");
  let ks = fx.slots();
  let pass = ks.slots().iter().find(|s| s.kind() == ProviderKind::Passphrase).unwrap().id();
  let ss = ks.slots().iter().find(|s| s.kind() == ProviderKind::SecretService).unwrap().id();

  // Wallet locked: only the passphrase can authorize removing the SS slot.
  fx.providers.ss.set_locked(true);
  ui.push(&["wrong passphrase!", "correct horse battery"]);
  let r = ops::remove(&ctx, ss, false).await;
  // The slot is gone from the file; its wallet item could not be destroyed
  // (wallet locked) -> exit 5.
  let e = r.unwrap_err();
  assert_eq!(e.exit_code(), exit::INCOMPLETE, "{e}");
  assert!(ui.all_text().contains("Wrong passphrase."));
  let ks = fx.slots();
  assert_eq!(ks.slots().len(), 1);
  assert_eq!(ks.slots()[0].id(), pass);
  let k = ks
    .unlock_slot(&pass, &PassphraseProvider::new(Zeroizing::new("correct horse battery".into())))
    .await
    .unwrap();
  assert_items(&Store::open_encrypted(&fx.state, &DataKey::from_bytes(k)).unwrap());
}

#[tokio::test]
async fn short_passphrase_needs_override() {
  let fx = Fx::new(1).await;
  let ui = ScriptUi::new(&["short", "short", "short"]);
  let e = ops::add(&fx.ctx(&ui), AddKind::Passphrase { allow_short: false }).await.unwrap_err();
  assert!(e.to_string().contains("no passphrase set"), "{e}");
  assert_eq!(fx.slots().slots().len(), 1);
}

// ---- rotate -----------------------------------------------------------------------

fn assert_no_rotation_leftovers(fx: &Fx) {
  assert!(!fx.exists("keyslots.json.next"));
  assert!(!fx.exists("keyslots.json.old"));
}

/// Wallet items == slots in keyslots.json (no orphans, nothing missing).
fn assert_wallet_matches(fx: &Fx) {
  let mut ids: Vec<SlotId> = fx.slots().slots().iter().map(|s| s.id()).collect();
  ids.sort();
  assert_eq!(fx.providers.ss.ids(), ids);
}

#[tokio::test]
async fn rotate_reenrolls_rekeys_and_destroys_old() {
  let fx = Fx::new(2).await;
  let before = fx.slots();
  let old_ids: Vec<SlotId> = before.slots().iter().map(|s| s.id()).collect();
  let old_key = before.unlock_any(&[&fx.providers.ss]).await.unwrap().0;
  let ui = ScriptUi::new(&["y"]);
  ops::rotate(&fx.ctx(&ui), false).await.unwrap();
  let after = fx.slots();
  assert_ne!(after.data_key_id(), before.data_key_id());
  assert_eq!(after.slots().len(), 2);
  for s in after.slots() {
    assert!(!old_ids.contains(&s.id()));
  }
  assert_wallet_matches(&fx);
  assert_no_rotation_leftovers(&fx);
  assert!(!fx.exists("index"), "index must be deleted after rekey");
  assert_items(&fx.open_with_main().await);
  // The old key no longer opens the history.
  assert!(matches!(
    Store::open_encrypted(&fx.state, &DataKey::from_bytes(old_key)),
    Err(spool_core::Error::WrongKey)
  ));
  assert!(ui.out_text().contains("rotated"));
}

#[tokio::test]
async fn rotate_declined_changes_nothing() {
  let fx = Fx::new(1).await;
  let before = fx.slots();
  let ui = ScriptUi::new(&["n"]);
  let e = ops::rotate(&fx.ctx(&ui), false).await.unwrap_err();
  assert_eq!(e.exit_code(), exit::ABORTED);
  assert_eq!(fx.slots().data_key_id(), before.data_key_id());
  assert_no_rotation_leftovers(&fx);
}

/// Wraps a [`ScriptUi`] and locks the fake wallet right before the first
/// new Secret Service slot is enrolled (after the unlock succeeded).
struct LockBeforeEnroll<'a>(&'a ScriptUi, FakeSs);

impl Ui for LockBeforeEnroll<'_> {
  fn out(&self, l: &str) {
    self.0.out(l)
  }
  fn note(&self, l: &str) {
    if l.starts_with("Storing a new key") {
      self.1.set_locked(true);
    }
    self.0.note(l)
  }
  fn secret(&self, p: &str) -> Result<Zeroizing<String>, PromptError> {
    self.0.secret(p)
  }
  fn line(&self, p: &str) -> Result<String, PromptError> {
    self.0.line(p)
  }
}

/// Enrollment fails (wallet locked) -> user answers `answer` -> nothing
/// changes and no staged file or new wallet item is left.
async fn rotate_enroll_failure(answer: &str, expect: &str) {
  let fx = Fx::new(2).await;
  let before = fx.slots();
  let wallet = fx.providers.ss.ids();
  let script = ScriptUi::new(&["y", answer, answer]);
  let ui = LockBeforeEnroll(&script, fx.providers.ss.clone());
  let ctx = Ctx::new(fx.state.clone(), fx.socket.clone(), &ui, &fx.providers);
  let e = ops::rotate(&ctx, false).await.unwrap_err();
  assert!(e.to_string().contains(expect), "{e}");
  fx.providers.ss.set_locked(false);
  assert_eq!(fx.slots().data_key_id(), before.data_key_id());
  assert_eq!(fx.providers.ss.ids(), wallet);
  assert_no_rotation_leftovers(&fx);
  assert_items(&fx.open_with_main().await);
}

#[tokio::test]
async fn rotate_abort_during_enroll_rolls_back() {
  rotate_enroll_failure("a", "aborted").await;
}

#[tokio::test]
async fn rotate_skip_every_slot_changes_nothing() {
  rotate_enroll_failure("s", "nothing changed").await;
}

/// Simulate a crash at `point`, check that some slot (in keyslots.json or
/// the staged file) still opens the history, then `recover` and check the
/// final state is consistent.
async fn crash_and_recover(point: CrashPoint) {
  let fx = Fx::new(2).await;
  let before = fx.slots();
  let ui = ScriptUi::new(&["y"]);
  let e = ops::rotate(&fx.ctx(&ui).with_crash_at(point), false).await.unwrap_err();
  assert!(e.to_string().contains("simulated crash"), "{point:?}: {e}");

  // Invariant: a slot for the key that opens the DB still exists.
  let mut opened = false;
  for f in ["keyslots.json", "keyslots.json.next"] {
    let Ok(ks) = KeySlots::load(fx.state.join(f)) else { continue };
    for s in ks.slots() {
      let Ok(k) = ks.unlock_slot(&s.id(), &fx.providers.ss).await else { continue };
      if let Ok(st) = Store::open_encrypted(&fx.state, &DataKey::from_bytes(k)) {
        assert_items(&st);
        opened = true;
      }
    }
  }
  assert!(opened, "{point:?}: no remaining slot opens the history");

  // Other commands refuse (or finish the cleanup) until recovered.
  let p = ops::pending(&fx.ctx(&ui));
  match point {
    CrashPoint::AfterSwitch => assert_eq!(p, Pending::Cleanup),
    _ => {
      assert_eq!(p, Pending::Rotation);
      let e = ops::add(&fx.ctx(&ui), AddKind::SecretService).await.unwrap_err();
      assert_eq!(e.exit_code(), exit::RECOVERY_NEEDED, "{point:?}");
    }
  }

  let ui = ScriptUi::new(&[]);
  ops::recover(&fx.ctx(&ui)).await.unwrap();
  assert_no_rotation_leftovers(&fx);
  assert_wallet_matches(&fx);
  assert_items(&fx.open_with_main().await);
  let rotated = fx.slots().data_key_id() != before.data_key_id();
  let expect_rotated =
    matches!(point, CrashPoint::AfterRekey | CrashPoint::AfterOldCopied | CrashPoint::AfterSwitch);
  assert_eq!(rotated, expect_rotated, "{point:?}: {}", ui.all_text());
  if rotated {
    assert!(!fx.exists("index"), "{point:?}: stale index kept");
  }
  // Recovering again is a no-op.
  let ui = ScriptUi::new(&[]);
  ops::recover(&fx.ctx(&ui)).await.unwrap();
  assert!(ui.out_text().contains("nothing to recover"));
  // And a fresh rotation works afterwards.
  let ui = ScriptUi::new(&["y"]);
  ops::rotate(&fx.ctx(&ui), false).await.unwrap();
  assert_wallet_matches(&fx);
  assert_items(&fx.open_with_main().await);
}

#[tokio::test]
async fn crash_after_first_enroll() {
  crash_and_recover(CrashPoint::AfterFirstEnroll).await;
}

#[tokio::test]
async fn crash_after_next_written() {
  crash_and_recover(CrashPoint::AfterNextWritten).await;
}

#[tokio::test]
async fn crash_after_rekey() {
  crash_and_recover(CrashPoint::AfterRekey).await;
}

#[tokio::test]
async fn crash_after_old_copied() {
  crash_and_recover(CrashPoint::AfterOldCopied).await;
}

#[tokio::test]
async fn crash_after_switch() {
  crash_and_recover(CrashPoint::AfterSwitch).await;
}

#[tokio::test]
async fn cleanup_pending_is_finished_by_next_command() {
  let fx = Fx::new(1).await;
  let ui = ScriptUi::new(&["y"]);
  let _ = ops::rotate(&fx.ctx(&ui).with_crash_at(CrashPoint::AfterSwitch), false).await;
  assert_eq!(fx.providers.ss.ids().len(), 2, "old + new wallet item");
  let ui = ScriptUi::new(&[]);
  ops::add(&fx.ctx(&ui), AddKind::SecretService).await.unwrap();
  assert_no_rotation_leftovers(&fx);
  assert_wallet_matches(&fx);
}

#[tokio::test]
async fn rotate_without_history_db() {
  let fx = Fx::new(1).await;
  spool_core::store::Store::wipe(&fx.state).unwrap();
  let before = fx.slots();
  let ui = ScriptUi::new(&["y"]);
  ops::rotate(&fx.ctx(&ui), false).await.unwrap();
  assert_ne!(fx.slots().data_key_id(), before.data_key_id());
  assert_wallet_matches(&fx);
  assert!(!fx.exists("history.db"), "rotate must not create a database");
}

// ---- wipe -------------------------------------------------------------------------

#[tokio::test]
async fn wipe_needs_confirmation() {
  let fx = Fx::new(1).await;
  let ui = ScriptUi::new(&["WIPE"]);
  let e = ops::wipe(&fx.ctx(&ui)).await.unwrap_err();
  assert_eq!(e.exit_code(), exit::ABORTED);
  assert!(fx.exists("history.db") && fx.exists("keyslots.json"));
  assert_eq!(fx.providers.ss.ids().len(), 1);
}

#[tokio::test]
async fn wipe_destroys_everything() {
  let fx = Fx::new(2).await;
  // An orphan wallet item (e.g. from an older partial wipe) and a pending
  // rotation file, plus an M1 plaintext backup.
  fx.providers.ss.enroll(&SlotId::new()).await.unwrap();
  let ui = ScriptUi::new(&["y"]);
  let _ = ops::rotate(&fx.ctx(&ui).with_crash_at(CrashPoint::AfterNextWritten), false).await;
  std::fs::write(fx.state.join("history.db.m1-plaintext.bak"), b"SQLite format 3\0").unwrap();
  let ui = ScriptUi::new(&["wipe"]);
  ops::wipe(&fx.ctx(&ui)).await.unwrap();
  for f in [
    "keyslots.json",
    "keyslots.json.next",
    "keyslots.json.old",
    "history.db",
    "history.db-wal",
    "history.db.kcv",
    "blobs",
    "index",
    "history.db.m1-plaintext.bak",
  ] {
    assert!(!fx.exists(f), "{f} survived the wipe");
  }
  assert!(fx.providers.ss.ids().is_empty(), "wallet items survived");
  assert!(ui.out_text().contains("copy-on-write"));
}

#[tokio::test]
async fn wipe_with_corrupt_keyslots() {
  let fx = Fx::new(1).await;
  std::fs::write(fx.state.join("keyslots.json"), b"{not json").unwrap();
  let ui = ScriptUi::new(&["wipe"]);
  ops::wipe(&fx.ctx(&ui)).await.unwrap();
  assert!(!fx.exists("keyslots.json") && !fx.exists("history.db"));
  // destroy_all still removed the orphaned item.
  assert!(fx.providers.ss.ids().is_empty());
}

#[tokio::test]
async fn wipe_reports_incomplete_when_wallet_locked() {
  let fx = Fx::new(1).await;
  fx.providers.ss.set_locked(true);
  let ui = ScriptUi::new(&["wipe"]);
  let e = ops::wipe(&fx.ctx(&ui)).await.unwrap_err();
  assert_eq!(e.exit_code(), exit::INCOMPLETE, "{e}");
  // Files are gone regardless.
  assert!(!fx.exists("keyslots.json") && !fx.exists("history.db"));
}
