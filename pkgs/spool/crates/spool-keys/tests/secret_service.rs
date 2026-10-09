//! Secret Service integration tests. They ONLY run through
//! `tests/secret-service-harness.sh` (private D-Bus + throwaway
//! gnome-keyring). With `SPOOL_SECRET_SERVICE_TESTS` unset they are no-ops;
//! with it set but without the harness markers they panic before touching
//! D-Bus.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use oo7::dbus::{Collection, Service};
use oo7::zbus;
use spool_keys::{
  APPLICATION, Error, ITEM_LABEL, KeyProvider, KeySlots, SecretServiceProvider, SessionProvider,
  SlotId, slot_attributes,
};

/// Paranoid check that we are on the harness's private bus. Returns false
/// (skip) if the tests are not enabled; panics if enabled but unsafe.
fn guard() -> bool {
  if std::env::var("SPOOL_SECRET_SERVICE_TESTS").as_deref() != Ok("1") {
    eprintln!("skipping Secret Service tests (set SPOOL_SECRET_SERVICE_TESTS=1 via the harness)");
    return false;
  }
  let env = |k: &str| {
    std::env::var(k).unwrap_or_else(|_| {
      panic!("{k} not set: run these tests via crates/spool-keys/tests/secret-service-harness.sh")
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
  for k in ["HOME", "XDG_DATA_HOME", "XDG_RUNTIME_DIR"] {
    assert!(Path::new(&env(k)).starts_with(&sandbox), "{k} is not inside the sandbox");
  }
  true
}

async fn default_collection() -> (Service, Collection) {
  let s = Service::new().await.expect("connect to test Secret Service");
  let c = s.with_alias("default").await.unwrap().expect("default collection");
  (s, c)
}

async fn spool_items_for(slot: &SlotId) -> usize {
  let (_s, c) = default_collection().await;
  c.search_items(&slot_attributes(slot)).await.unwrap().len()
}

async fn all_spool_items() -> usize {
  let (_s, c) = default_collection().await;
  c.search_items(&[("application", APPLICATION)]).await.unwrap().len()
}

/// Unlock the test keyring without a prompt via gnome-keyring's private
/// `UnlockWithMasterPassword` (plain transfer session).
async fn unlock_test_keyring(collection: &Collection) {
  let password = std::env::var("SPOOL_TEST_KEYRING_PASSWORD").unwrap();
  let conn = zbus::Connection::session().await.unwrap();
  let reply = conn
    .call_method(
      Some("org.freedesktop.secrets"),
      "/org/freedesktop/secrets",
      Some("org.freedesktop.Secret.Service"),
      "OpenSession",
      &("plain", zbus::zvariant::Value::from("")),
    )
    .await
    .unwrap();
  let (_out, session): (zbus::zvariant::OwnedValue, zbus::zvariant::OwnedObjectPath) =
    reply.body().deserialize().unwrap();
  let secret = (session.clone(), Vec::<u8>::new(), password.into_bytes(), "text/plain");
  conn
    .call_method(
      Some("org.freedesktop.secrets"),
      "/org/freedesktop/secrets",
      Some("org.gnome.keyring.InternalUnsupportedGuiltRiddenInterface"),
      "UnlockWithMasterPassword",
      &(collection.path(), secret),
    )
    .await
    .unwrap();
}

fn tmp_path() -> (tempfile::TempDir, PathBuf) {
  let d = tempfile::tempdir().unwrap();
  let p = d.path().join("spool/keyslots.json");
  (d, p)
}

fn assert_fatal(e: &Error) {
  assert!(!e.is_retryable(), "expected fatal, got retryable: {e}");
}

#[tokio::test]
async fn secret_service_end_to_end() {
  if !guard() {
    return;
  }
  let start = Instant::now();
  let step = |name: &str| eprintln!("[{:>8.3?}] {name}", start.elapsed());
  let ss = SecretServiceProvider::new(false);
  assert!(ss.is_unlocked().await.unwrap(), "test keyring should start unlocked");
  let (_d, path) = tmp_path();

  // ---- create_new -> reload -> unlock_any ---------------------------------------
  step("create_new -> reload -> unlock_any");
  let (mut ks, dk) = KeySlots::create_new(&path, &ss).await.unwrap();
  let s1 = ks.slots()[0].id();
  assert_eq!(spool_items_for(&s1).await, 1);
  {
    let (_s, c) = default_collection().await;
    let item = c.search_items(&slot_attributes(&s1)).await.unwrap().remove(0);
    assert_eq!(item.label().await.unwrap(), ITEM_LABEL);
    let attrs = item.attributes().await.unwrap();
    assert_eq!(attrs.get("application").map(String::as_str), Some("spool"));
    assert_eq!(attrs.get("slot"), Some(&s1.to_string()));
  }
  let reloaded = KeySlots::load(&path).unwrap();
  let (k, sid) = reloaded.unlock_any(&[&ss]).await.unwrap();
  assert_eq!((*k, sid), (*dk, s1));

  // ---- second slot add, then remove ---------------------------------------------
  step("second slot add, then remove");
  let s2 = ks.add_slot(&ss, &dk).await.unwrap();
  assert_eq!(spool_items_for(&s2).await, 1);
  assert_eq!(*KeySlots::load(&path).unwrap().unlock_slot(&s2, &ss).await.unwrap(), *dk);
  let session = SessionProvider::new();
  let s3 = ks.add_slot(&session, &dk).await.unwrap();
  assert_eq!(KeySlots::load(&path).unwrap().slots().len(), 3);
  ks.remove_slot(&s2, &[&ss, &session], false).await.unwrap();
  assert_eq!(spool_items_for(&s2).await, 0);
  ks.remove_slot(&s3, &[&ss, &session], false).await.unwrap();
  assert_eq!(KeySlots::load(&path).unwrap().slots().len(), 1);

  // ---- wrong KEK in the keyring -> fatal ----------------------------------------
  step("wrong KEK in the keyring -> fatal");
  {
    let (_s, c) = default_collection().await;
    let item = c.search_items(&slot_attributes(&s1)).await.unwrap().remove(0);
    let original = item.secret().await.unwrap();
    item.set_secret(oo7::Secret::text(B64.encode([0x5Au8; 32]))).await.unwrap();
    let e = ks.unlock_any(&[&ss]).await.unwrap_err();
    assert_fatal(&e);
    let Error::NoSlotUnlocked { attempts } = &e else { panic!("{e}") };
    assert!(matches!(attempts[0].error, Error::WrongKey), "{e}");
    // Garbage (not a 32-byte key) is also fatal.
    item.set_secret(oo7::Secret::text("garbage")).await.unwrap();
    assert_fatal(&ks.unlock_any(&[&ss]).await.unwrap_err());
    item.set_secret(original).await.unwrap();
    assert_eq!(*ks.unlock_any(&[&ss]).await.unwrap().0, *dk);
  }

  // ---- tampered keyslots.json -> error ------------------------------------------
  step("tampered keyslots.json -> error");
  let s2 = ks.add_slot(&ss, &dk).await.unwrap();
  let orig = std::fs::read(&path).unwrap();
  let json = |b: &[u8]| serde_json::from_slice::<serde_json::Value>(b).unwrap();
  let check_tampered = |v: serde_json::Value| {
    let p = path.clone();
    let ss = ss.clone();
    async move {
      std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
      let e = KeySlots::load(&p).unwrap().unlock_any(&[&ss]).await.unwrap_err();
      assert_fatal(&e);
    }
  };
  let mut v = json(&orig);
  for i in 0..2 {
    let mut w = B64.decode(v["slots"][i]["wrapped"].as_str().unwrap()).unwrap();
    w[40] ^= 0x80;
    v["slots"][i]["wrapped"] = B64.encode(&w).into();
  }
  check_tampered(v).await;
  let mut v = json(&orig);
  v["data_key_id"] = uuid_str().into();
  check_tampered(v).await;
  let mut v = json(&orig);
  let w0 = v["slots"][0]["wrapped"].clone();
  v["slots"][0]["wrapped"] = v["slots"][1]["wrapped"].clone();
  v["slots"][1]["wrapped"] = w0;
  check_tampered(v).await;
  std::fs::write(&path, &orig).unwrap();
  assert_eq!(*KeySlots::load(&path).unwrap().unlock_any(&[&ss]).await.unwrap().0, *dk);

  // ---- rotate -------------------------------------------------------------------
  step("rotate");
  let new_dk = zeroize::Zeroizing::new([0x42u8; 32]);
  let old = ks.clone();
  ks.rotate(&new_dk, &[&ss]).await.unwrap();
  assert_eq!(spool_items_for(&s1).await, 0, "old KEK must be destroyed");
  assert_eq!(spool_items_for(&s2).await, 0, "old KEK must be destroyed");
  for s in ks.slots() {
    assert_eq!(spool_items_for(&s.id()).await, 1);
  }
  let (k, _) = KeySlots::load(&path).unwrap().unlock_any(&[&ss]).await.unwrap();
  assert_eq!(*k, *new_dk);
  let e = old.unlock_any(&[&ss]).await.unwrap_err();
  assert_fatal(&e); // stale copy (e.g. a snapshot) is useless now

  // ---- wipe ---------------------------------------------------------------------
  step("wipe");
  let ids: Vec<_> = ks.slots().iter().map(|s| s.id()).collect();
  ks.wipe(&[&ss]).await.unwrap();
  assert!(!path.exists());
  for id in ids {
    assert_eq!(spool_items_for(&id).await, 0);
  }

  // ---- destroy_all cleans orphans -----------------------------------------------
  step("destroy_all cleans orphans");
  let orphan = SlotId::new();
  ss.enroll(&orphan).await.unwrap();
  assert!(all_spool_items().await >= 1);
  assert!(ss.destroy_all().await.unwrap() >= 1);
  assert_eq!(all_spool_items().await, 0);

  // ---- locked collection, allow_prompt = false -> retryable Locked --------------
  step("locked collection, allow_prompt = false -> retryable Locked");
  let (ks, dk) = KeySlots::create_new(&path, &ss).await.unwrap();
  let (_s, c) = default_collection().await;
  c.lock(None).await.unwrap();
  assert!(c.is_locked().await.unwrap());
  assert!(!ss.is_unlocked().await.unwrap());
  let e = ks.unlock_any(&[&ss]).await.unwrap_err();
  assert!(e.is_retryable() && e.is_locked(), "{e}");
  let (_d2, path2) = tmp_path();
  let e = KeySlots::create_new(&path2, &ss).await.unwrap_err();
  assert!(matches!(e, Error::Locked(_)), "{e}");
  assert!(!path2.exists());
  let t = Instant::now();
  let e = ss.wait_for_unlock(Some(Duration::from_millis(1500))).await.unwrap_err();
  assert!(matches!(e, Error::Locked(_)), "{e}");
  assert!(t.elapsed() < Duration::from_secs(5));

  // ---- wait_for_unlock wakes when the keyring is unlocked -----------------------
  step("wait_for_unlock wakes when the keyring is unlocked");
  let waiter = {
    let ss = ss.clone();
    tokio::spawn(async move { ss.wait_for_unlock(Some(Duration::from_secs(20))).await })
  };
  tokio::time::sleep(Duration::from_millis(300)).await;
  let t = Instant::now();
  unlock_test_keyring(&c).await;
  waiter.await.unwrap().unwrap();
  eprintln!("wait_for_unlock returned {:?} after unlock", t.elapsed());
  let (k, _) = ks.unlock_any(&[&ss]).await.unwrap();
  assert_eq!(*k, *dk);
  ks.wipe(&[&ss]).await.unwrap();
  assert_eq!(all_spool_items().await, 0);
  step("done");
}

fn uuid_str() -> String {
  // A fresh random UUID string without depending on `uuid` directly.
  SlotId::new().to_string()
}

#[test]
fn guard_skips_without_env() {
  // In plain `cargo test` (and `nix build`) the gate is unset: no D-Bus.
  if std::env::var_os("SPOOL_SECRET_SERVICE_TESTS").is_none() {
    assert!(!guard());
  }
}
