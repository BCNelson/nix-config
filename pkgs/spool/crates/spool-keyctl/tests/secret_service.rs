//! spool-keyctl against a real Secret Service. ONLY runs through
//! `tests/secret-service-harness.sh` (private D-Bus + throwaway
//! gnome-keyring). With `SPOOL_SECRET_SERVICE_TESTS` unset it is a no-op;
//! with it set but without the harness markers it panics before touching
//! D-Bus.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use spool_core::item::{ItemFlags, NewItem, Representation, Selection, dedupe_hash};
use spool_core::store::Store;
use spool_crypto::DataKey;
use spool_keyctl::ops::{self, AddKind};
use spool_keyctl::{Ctx, PromptError, Providers, SystemProviders, Ui};
use spool_keys::{Error, KeySlots, SecretServiceProvider};
use zeroize::Zeroizing;

/// Same paranoia as spool-keys' Secret Service tests.
fn guard() -> bool {
  if std::env::var("SPOOL_SECRET_SERVICE_TESTS").as_deref() != Ok("1") {
    eprintln!("skipping Secret Service tests (set SPOOL_SECRET_SERVICE_TESTS=1 via the harness)");
    return false;
  }
  let env = |k: &str| {
    std::env::var(k).unwrap_or_else(|_| {
      panic!("{k} not set: run these tests via crates/spool-keyctl/tests/secret-service-harness.sh")
    })
  };
  let parent = env("SPOOL_TEST_PARENT_DBUS");
  let addr = env("DBUS_SESSION_BUS_ADDRESS");
  assert!(!addr.is_empty(), "empty bus address");
  assert_ne!(addr, parent, "refusing to run on the inherited (real) session bus");
  let sandbox = PathBuf::from(env("SPOOL_TEST_SANDBOX"));
  assert!(sandbox.is_absolute(), "sandbox must be absolute");
  let name = sandbox.file_name().and_then(|n| n.to_str()).unwrap_or_default();
  assert!(name.starts_with("spool-ss-test."), "unexpected sandbox dir {name}");
  let bus_dir = format!("{}/bus/", sandbox.display());
  assert!(addr.contains(&bus_dir), "bus socket is not inside the sandbox");
  let marker =
    std::fs::read_to_string(sandbox.join(".spool-test-sandbox")).expect("sandbox marker");
  assert_eq!(marker, addr, "sandbox marker does not match the bus address");
  for k in ["HOME", "XDG_DATA_HOME", "XDG_RUNTIME_DIR", "XDG_STATE_HOME"] {
    assert!(Path::new(&env(k)).starts_with(&sandbox), "{k} is not inside the sandbox");
  }
  true
}

struct ScriptUi(Mutex<VecDeque<&'static str>>);

impl Ui for ScriptUi {
  fn out(&self, l: &str) {
    eprintln!("out: {l}");
  }
  fn note(&self, l: &str) {
    eprintln!("note: {l}");
  }
  fn secret(&self, _p: &str) -> Result<Zeroizing<String>, PromptError> {
    panic!("no secret prompts expected with Secret Service slots only");
  }
  fn line(&self, p: &str) -> Result<String, PromptError> {
    let a = self.0.lock().unwrap().pop_front().expect("unexpected prompt");
    eprintln!("prompt: {p}{a}");
    Ok(a.into())
  }
}

fn ui(answers: &[&'static str]) -> ScriptUi {
  ScriptUi(Mutex::new(answers.iter().copied().collect()))
}

async fn secret_present(ks: &KeySlots, ss: &SecretServiceProvider) -> usize {
  let mut n = 0;
  for s in ks.slots() {
    match ks.unlock_slot(&s.id(), ss).await {
      Ok(_) => n += 1,
      Err(Error::SecretMissing(_)) => {}
      Err(e) => panic!("unexpected {e}"),
    }
  }
  n
}

#[tokio::test]
async fn keyctl_with_secret_service() {
  if !guard() {
    return;
  }
  let tmp = tempfile::tempdir().unwrap();
  let state = tmp.path().join("state");
  std::fs::create_dir(&state).unwrap();
  let socket = tmp.path().join("run/spool/sock");
  let ss = SecretServiceProvider::new(false);
  let ks_path = state.join(spool_keys::FILE_NAME);

  // A history like spoold would leave it.
  let (ks, key) = KeySlots::create_new(&ks_path, &ss).await.unwrap();
  let first = ks.slots()[0].id();
  {
    let mut st = Store::open_encrypted(&state, &DataKey::from_bytes(key)).unwrap();
    let hk = st.hash_key().unwrap();
    let canon = Representation::new("text/plain;charset=utf-8", b"hello".to_vec());
    st.insert(NewItem {
      selection: Selection::Clipboard,
      source_app: None,
      created_at: std::time::SystemTime::now(),
      flags: ItemFlags::empty(),
      hash: dedupe_hash(&hk, &canon),
      preview: Some("hello".into()),
      reps: vec![canon],
    })
    .unwrap();
  }
  let providers = SystemProviders;

  // add secret-service
  let u = ui(&[]);
  ops::add(&Ctx::new(state.clone(), socket.clone(), &u, &providers), AddKind::SecretService)
    .await
    .unwrap();
  let ks = KeySlots::load(&ks_path).unwrap();
  assert_eq!(ks.slots().len(), 2);
  assert_eq!(secret_present(&ks, &ss).await, 2);
  let old = ks.clone();

  // rotate: new items, old ones destroyed, history readable with the new key
  let u = ui(&["y"]);
  ops::rotate(&Ctx::new(state.clone(), socket.clone(), &u, &providers), false).await.unwrap();
  let ks = KeySlots::load(&ks_path).unwrap();
  assert_ne!(ks.data_key_id(), old.data_key_id());
  assert_eq!(secret_present(&ks, &ss).await, 2);
  for s in old.slots() {
    assert!(matches!(old.unlock_slot(&s.id(), &ss).await, Err(Error::SecretMissing(_))));
  }
  let (k, _) = ks.unlock_any(&[&ss]).await.unwrap();
  assert_eq!(Store::open_encrypted(&state, &DataKey::from_bytes(k)).unwrap().count().unwrap(), 1);

  // remove one
  let gone = ks.slots()[0].id();
  let u = ui(&[]);
  ops::remove(&Ctx::new(state.clone(), socket.clone(), &u, &providers), gone, false).await.unwrap();
  assert!(matches!(ks.unlock_slot(&gone, &ss).await, Err(Error::SecretMissing(_))));
  assert_ne!(gone, first);

  // wipe: no Spool items left in the keyring, no files
  let u = ui(&["wipe"]);
  ops::wipe(&Ctx::new(state.clone(), socket.clone(), &u, &providers)).await.unwrap();
  assert_eq!(providers.secret_service_destroy_all().await.unwrap(), 0);
  assert!(!ks_path.exists() && !state.join("history.db").exists());
}
