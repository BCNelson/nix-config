use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;

use async_trait::async_trait;

use super::*;
use crate::provider::Kek;
use crate::session::SessionProvider;

/// Test provider with a fixed kind and scripted behaviour.
struct Fake {
  kind: ProviderKind,
  interactive: bool,
  mode: Mutex<Mode>,
  inner: SessionProvider,
  calls: Mutex<Vec<&'static str>>,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
  Ok,
  Locked,
  WrongKek,
  FailDestroy,
}

impl Fake {
  fn new(kind: ProviderKind, interactive: bool) -> Self {
    Fake {
      kind,
      interactive,
      mode: Mutex::new(Mode::Ok),
      inner: SessionProvider::new(),
      calls: Mutex::new(Vec::new()),
    }
  }
  fn set(&self, m: Mode) {
    *self.mode.lock().unwrap() = m;
  }
  fn calls(&self) -> Vec<&'static str> {
    self.calls.lock().unwrap().clone()
  }
}

#[async_trait]
impl KeyProvider for Fake {
  fn kind(&self) -> ProviderKind {
    self.kind
  }
  fn interactive(&self) -> bool {
    self.interactive
  }
  async fn enroll(&self, slot_id: &SlotId) -> Result<(SlotParams, Kek)> {
    self.calls.lock().unwrap().push("enroll");
    let (_, kek) = self.inner.enroll(slot_id).await?;
    let params = match self.kind {
      ProviderKind::SecretService => {
        SlotParams::SecretService { attributes: slot_attributes(slot_id) }
      }
      ProviderKind::Session => SlotParams::Session,
      _ => SlotParams::Opaque(serde_json::json!({})),
    };
    Ok((params, kek))
  }
  async fn unlock(&self, slot_id: &SlotId, _params: &SlotParams) -> Result<Kek> {
    self.calls.lock().unwrap().push("unlock");
    match *self.mode.lock().unwrap() {
      Mode::Locked => return Err(Error::Locked(self.kind)),
      Mode::WrongKek => return Ok(crate::wrap::random_key()),
      _ => {}
    }
    self.inner.unlock(slot_id, &SlotParams::Session).await
  }
  async fn destroy(&self, slot_id: &SlotId, _params: &SlotParams) -> Result<()> {
    self.calls.lock().unwrap().push("destroy");
    if *self.mode.lock().unwrap() == Mode::FailDestroy {
      return Err(Error::Locked(self.kind));
    }
    self.inner.destroy(slot_id, &SlotParams::Session).await
  }
}

fn tmp() -> (tempfile::TempDir, PathBuf) {
  let d = tempfile::tempdir().unwrap();
  let p = d.path().join("spool").join(FILE_NAME);
  (d, p)
}

fn read_json(p: &Path) -> serde_json::Value {
  serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
}

fn write_json(p: &Path, v: &serde_json::Value) {
  std::fs::write(p, serde_json::to_vec(v).unwrap()).unwrap();
}

#[tokio::test]
async fn create_reload_unlock_session() {
  let (_d, path) = tmp();
  let sess = SessionProvider::new();
  let (ks, dk) = KeySlots::create_new(&path, &sess).await.unwrap();
  assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
  let re = KeySlots::load(&path).unwrap();
  assert_eq!(re.data_key_id(), ks.data_key_id());
  assert_eq!(re.slots(), ks.slots());
  let (dk2, sid) = re.unlock_any(&[&sess]).await.unwrap();
  assert_eq!(*dk2, *dk);
  assert_eq!(sid, ks.slots()[0].id());
  // A new process (fresh session provider) cannot unlock: retryable Unavailable.
  let e = re.unlock_any(&[&SessionProvider::new()]).await.unwrap_err();
  assert!(e.is_retryable() && !e.is_locked(), "{e}");
}

#[tokio::test]
async fn create_refuses_existing_file() {
  let (_d, path) = tmp();
  let sess = SessionProvider::new();
  KeySlots::create_new(&path, &sess).await.unwrap();
  assert!(matches!(KeySlots::create_new(&path, &sess).await, Err(Error::AlreadyExists(_))));
}

#[tokio::test]
async fn file_format_shape() {
  let (_d, path) = tmp();
  let ss = Fake::new(ProviderKind::SecretService, false);
  let (ks, _) = KeySlots::create_new(&path, &ss).await.unwrap();
  let v = read_json(&path);
  assert_eq!(v["version"], 1);
  assert_eq!(v["data_key_id"], ks.data_key_id().to_string());
  let s = &v["slots"][0];
  let id = ks.slots()[0].id().to_string();
  assert_eq!(s["id"], id);
  assert_eq!(s["provider"], "secret-service");
  assert_eq!(s["params"], serde_json::json!({"attributes": {"application": "spool", "slot": id}}));
  assert_eq!(B64.decode(s["wrapped"].as_str().unwrap()).unwrap().len(), WRAPPED_LEN);
  assert_eq!(s.as_object().unwrap().len(), 4);
}

#[tokio::test]
async fn add_remove_slots() {
  let (_d, path) = tmp();
  let a = Fake::new(ProviderKind::SecretService, false);
  let b = SessionProvider::new();
  let (mut ks, dk) = KeySlots::create_new(&path, &a).await.unwrap();
  let first = ks.slots()[0].id();
  let second = ks.add_slot(&b, &dk).await.unwrap();
  assert_eq!(KeySlots::load(&path).unwrap().slots().len(), 2);
  // Only the session provider: unlocks via the second slot.
  let (k, sid) = KeySlots::load(&path).unwrap().unlock_any(&[&b]).await.unwrap();
  assert_eq!((*k, sid), (*dk, second));

  ks.remove_slot(&first, &[&a, &b], false).await.unwrap();
  assert_eq!(a.calls().last(), Some(&"destroy"));
  let re = KeySlots::load(&path).unwrap();
  assert_eq!(re.slots().len(), 1);
  assert_eq!(re.slots()[0].id(), second);

  assert!(matches!(ks.remove_slot(&first, &[&b], false).await, Err(Error::NoSuchSlot(_))));
  assert!(matches!(ks.remove_slot(&second, &[&b], false).await, Err(Error::LastSlot)));
  assert!(path.exists());
  ks.remove_slot(&second, &[&b], true).await.unwrap();
  assert!(!path.exists());
  assert!(b.is_empty());
}

#[tokio::test]
async fn remove_without_provider_reports_destroy_failure() {
  let (_d, path) = tmp();
  let a = Fake::new(ProviderKind::SecretService, false);
  let b = SessionProvider::new();
  let (mut ks, dk) = KeySlots::create_new(&path, &a).await.unwrap();
  let first = ks.slots()[0].id();
  ks.add_slot(&b, &dk).await.unwrap();
  let e = ks.remove_slot(&first, &[&b], false).await.unwrap_err();
  assert!(matches!(e, Error::DestroyIncomplete { .. }));
  // Still removed from the file.
  assert_eq!(KeySlots::load(&path).unwrap().slots().len(), 1);
}

#[tokio::test]
async fn wrong_kek_is_fatal() {
  let (_d, path) = tmp();
  let a = Fake::new(ProviderKind::SecretService, false);
  let (ks, _) = KeySlots::create_new(&path, &a).await.unwrap();
  a.set(Mode::WrongKek);
  let e = ks.unlock_any(&[&a]).await.unwrap_err();
  assert!(!e.is_retryable(), "{e}");
  match e {
    Error::NoSlotUnlocked { attempts } => {
      assert_eq!(attempts.len(), 1);
      assert!(matches!(attempts[0].error, Error::WrongKey));
    }
    other => panic!("unexpected {other}"),
  }
  let e = ks.unlock_slot(&ks.slots()[0].id(), &a).await.unwrap_err();
  assert!(matches!(e, Error::WrongKey));
}

#[tokio::test]
async fn locked_is_retryable() {
  let (_d, path) = tmp();
  let a = Fake::new(ProviderKind::SecretService, false);
  let (ks, _) = KeySlots::create_new(&path, &a).await.unwrap();
  a.set(Mode::Locked);
  let e = ks.unlock_any(&[&a]).await.unwrap_err();
  assert!(e.is_retryable() && e.is_locked());
}

#[tokio::test]
async fn mixed_failures_retryable_if_any_retryable() {
  let (_d, path) = tmp();
  let a = Fake::new(ProviderKind::SecretService, false);
  let b = Fake::new(ProviderKind::Session, false);
  let (mut ks, dk) = KeySlots::create_new(&path, &a).await.unwrap();
  ks.add_slot(&b, &dk).await.unwrap();
  a.set(Mode::WrongKek);
  b.set(Mode::Locked);
  let e = ks.unlock_any(&[&a, &b]).await.unwrap_err();
  assert!(e.is_retryable());
  b.set(Mode::Ok);
  let (k, sid) = ks.unlock_any(&[&a, &b]).await.unwrap();
  assert_eq!((*k, sid), (*dk, ks.slots()[1].id()));
}

#[tokio::test]
async fn non_interactive_first() {
  let (_d, path) = tmp();
  let inter = Fake::new(ProviderKind::SecretService, true);
  let quiet = Fake::new(ProviderKind::Session, false);
  let (mut ks, dk) = KeySlots::create_new(&path, &inter).await.unwrap();
  let quiet_id = ks.add_slot(&quiet, &dk).await.unwrap();
  let (_, sid) = ks.unlock_any(&[&inter, &quiet]).await.unwrap();
  assert_eq!(sid, quiet_id);
  assert!(!inter.calls().contains(&"unlock"));
  // Falls through to the interactive one when the quiet one fails.
  quiet.set(Mode::Locked);
  let (_, sid) = ks.unlock_any(&[&inter, &quiet]).await.unwrap();
  assert_eq!(sid, ks.slots()[0].id());
}

#[tokio::test]
async fn missing_and_unimplemented_providers() {
  let (_d, path) = tmp();
  let a = Fake::new(ProviderKind::SecretService, false);
  let (ks, _) = KeySlots::create_new(&path, &a).await.unwrap();
  let e = ks.unlock_any(&[]).await.unwrap_err();
  assert!(!e.is_retryable());
  let Error::NoSlotUnlocked { attempts } = e else { panic!() };
  assert!(matches!(attempts[0].error, Error::NoProvider(ProviderKind::SecretService)));

  // An unimplemented kind loads (params kept opaque) and reports NotImplemented.
  let mut v = read_json(&path);
  v["slots"][0]["provider"] = "tpm2".into();
  v["slots"][0]["params"] = serde_json::json!({"pcrs": [7]});
  write_json(&path, &v);
  let ks = KeySlots::load(&path).unwrap();
  assert_eq!(ks.slots()[0].params(), &SlotParams::Opaque(serde_json::json!({"pcrs": [7]})));
  let Err(Error::NoSlotUnlocked { attempts }) = ks.unlock_any(&[&a]).await else { panic!() };
  assert!(matches!(attempts[0].error, Error::NotImplemented(ProviderKind::Tpm2)));
  // Round trip keeps the opaque params.
  let mut ks = ks;
  let b = SessionProvider::new();
  // Can't add without the data key in real use; here any key is fine.
  ks.add_slot(&b, &[0u8; 32]).await.unwrap();
  assert_eq!(read_json(&path)["slots"][0]["params"], serde_json::json!({"pcrs": [7]}));
}

#[tokio::test]
async fn tampering_is_detected() {
  let (_d, path) = tmp();
  let a = Fake::new(ProviderKind::SecretService, false);
  let b = Fake::new(ProviderKind::Session, false);
  let (mut ks, dk) = KeySlots::create_new(&path, &a).await.unwrap();
  ks.add_slot(&b, &dk).await.unwrap();
  let orig = read_json(&path);
  let fatal = |e: Error| {
    assert!(!e.is_retryable(), "{e}");
    let Error::NoSlotUnlocked { attempts } = e else { panic!("{e}") };
    assert!(attempts.iter().all(|a| matches!(a.error, Error::WrongKey)));
  };

  // Flip one byte of each wrapped value.
  let mut v = orig.clone();
  for i in 0..2 {
    let mut w = B64.decode(v["slots"][i]["wrapped"].as_str().unwrap()).unwrap();
    w[30] ^= 0x01;
    v["slots"][i]["wrapped"] = B64.encode(&w).into();
  }
  write_json(&path, &v);
  fatal(KeySlots::load(&path).unwrap().unlock_any(&[&a, &b]).await.unwrap_err());

  // Change data_key_id.
  let mut v = orig.clone();
  v["data_key_id"] = DataKeyId::new().to_string().into();
  write_json(&path, &v);
  fatal(KeySlots::load(&path).unwrap().unlock_any(&[&a, &b]).await.unwrap_err());

  // Swap the two slots' wrapped values.
  let mut v = orig.clone();
  let w0 = v["slots"][0]["wrapped"].clone();
  v["slots"][0]["wrapped"] = v["slots"][1]["wrapped"].clone();
  v["slots"][1]["wrapped"] = w0;
  write_json(&path, &v);
  fatal(KeySlots::load(&path).unwrap().unlock_any(&[&a, &b]).await.unwrap_err());

  // Change a slot id (and its attributes, so params validation passes): the
  // AAD no longer matches even with the right KEK.
  let mut v = orig.clone();
  let orig_id = ks.slots()[0].id();
  let nid = SlotId::new().to_string();
  v["slots"][0]["id"] = nid.clone().into();
  v["slots"][0]["params"]["attributes"]["slot"] = nid.into();
  write_json(&path, &v);
  let ks2 = KeySlots::load(&path).unwrap();
  let kek = a.inner.unlock(&orig_id, &SlotParams::Session).await.unwrap();
  let s2 = &ks2.slots()[0];
  assert!(matches!(
    wrap::open(&kek, &s2.wrapped, &wrap::aad(&s2.id(), &ks2.data_key_id())),
    Err(Error::WrongKey)
  ));
  assert!(wrap::open(&kek, &s2.wrapped, &wrap::aad(&orig_id, &ks2.data_key_id())).is_ok());

  // Untouched file still works.
  write_json(&path, &orig);
  let (k, _) = KeySlots::load(&path).unwrap().unlock_any(&[&a, &b]).await.unwrap();
  assert_eq!(*k, *dk);
}

fn load_str(s: &str) -> Result<KeySlots> {
  let d = tempfile::tempdir().unwrap();
  let p = d.path().join(FILE_NAME);
  std::fs::write(&p, s).unwrap();
  KeySlots::load(&p)
}

fn valid_doc() -> serde_json::Value {
  let id = SlotId::new().to_string();
  serde_json::json!({
    "version": 1,
    "data_key_id": DataKeyId::new().to_string(),
    "slots": [{
      "id": id,
      "provider": "secret-service",
      "params": {"attributes": {"application": "spool", "slot": id}},
      "wrapped": B64.encode([0u8; WRAPPED_LEN]),
    }]
  })
}

#[test]
fn strict_parsing() {
  let ok = valid_doc();
  load_str(&ok.to_string()).unwrap();

  let corrupt = |v: serde_json::Value, what: &str| match load_str(&v.to_string()) {
    Err(Error::Corrupt { .. }) => {}
    other => panic!("{what}: expected Corrupt, got {other:?}"),
  };

  let mut v = ok.clone();
  v["extra"] = 1.into();
  corrupt(v, "unknown top-level field");
  let mut v = ok.clone();
  v["slots"][0]["extra"] = 1.into();
  corrupt(v, "unknown slot field");
  let mut v = ok.clone();
  v["slots"][0]["params"]["extra"] = 1.into();
  corrupt(v, "unknown params field");
  let mut v = ok.clone();
  v["slots"][0]["params"]["attributes"]["slot"] = SlotId::new().to_string().into();
  corrupt(v, "attributes slot mismatch");
  let mut v = ok.clone();
  v["slots"][0]["params"]["attributes"]["application"] = "other".into();
  corrupt(v, "attributes application mismatch");
  let mut v = ok.clone();
  v["slots"][0]["params"]["attributes"]["x"] = "y".into();
  corrupt(v, "extra attribute");
  let mut v = ok.clone();
  v["slots"][0]["provider"] = "nope".into();
  corrupt(v, "unknown provider");
  let mut v = ok.clone();
  v["slots"][0]["wrapped"] = B64.encode([0u8; WRAPPED_LEN - 1]).into();
  corrupt(v, "short wrapped");
  let mut v = ok.clone();
  v["slots"][0]["wrapped"] = "!!!".into();
  corrupt(v, "bad base64");
  let mut v = ok.clone();
  v["slots"] = serde_json::json!([]);
  corrupt(v, "no slots");
  let mut v = ok.clone();
  let s = v["slots"][0].clone();
  v["slots"] = serde_json::json!([s.clone(), s]);
  corrupt(v, "duplicate slot ids");
  let mut v = ok.clone();
  v["data_key_id"] = "not-a-uuid".into();
  corrupt(v, "bad uuid");
  let mut v = ok.clone();
  v.as_object_mut().unwrap().remove("version");
  corrupt(v, "missing version");
  let mut v = ok.clone();
  v["slots"][0]["provider"] = "session".into();
  corrupt(v, "session with non-empty params");
  let mut v = ok.clone();
  v["slots"][0]["provider"] = "session".into();
  v["slots"][0]["params"] = serde_json::json!({});
  load_str(&v.to_string()).unwrap();

  // Duplicate keys.
  let s = ok.to_string().replacen("\"version\":1", "\"version\":1,\"version\":1", 1);
  assert!(matches!(load_str(&s), Err(Error::Corrupt { .. })));

  let mut v = ok.clone();
  v["version"] = 2.into();
  assert!(matches!(load_str(&v.to_string()), Err(Error::UnsupportedVersion { version: 2, .. })));

  assert!(matches!(load_str("not json"), Err(Error::Corrupt { .. })));

  // Too many slots.
  let mut v = ok.clone();
  let slots: Vec<_> = (0..=MAX_SLOTS)
    .map(|_| {
      let id = SlotId::new().to_string();
      serde_json::json!({"id": id, "provider": "session", "params": {}, "wrapped": B64.encode([0u8; WRAPPED_LEN])})
    })
    .collect();
  v["slots"] = slots.into();
  corrupt(v, "too many slots");
}

#[test]
fn size_limit() {
  let v = valid_doc();
  // Pad with whitespace to just over the limit; still otherwise valid JSON.
  let s = v.to_string();
  let pad = " ".repeat(MAX_FILE_SIZE as usize + 1 - s.len());
  assert!(matches!(load_str(&format!("{s}{pad}")), Err(Error::TooLarge { .. })));
  let pad = " ".repeat(MAX_FILE_SIZE as usize - s.len());
  assert!(load_str(&format!("{s}{pad}")).is_ok());
}

#[tokio::test]
async fn rotate_rewraps_with_fresh_keks() {
  let (_d, path) = tmp();
  let a = Fake::new(ProviderKind::SecretService, false);
  let b = SessionProvider::new();
  let (mut ks, dk) = KeySlots::create_new(&path, &a).await.unwrap();
  ks.add_slot(&b, &dk).await.unwrap();
  let before = ks.clone();
  let new_dk = crate::wrap::random_key();
  ks.rotate(&new_dk, &[&a, &b]).await.unwrap();
  assert_ne!(ks.data_key_id(), before.data_key_id());
  assert_eq!(ks.slots().len(), 2);
  for (n, o) in ks.slots().iter().zip(before.slots()) {
    assert_ne!(n.id(), o.id());
    assert_eq!(n.kind(), o.kind());
  }
  // Old secrets destroyed: old file copy (e.g. a btrfs snapshot) no longer unlocks.
  let e = before.unlock_any(&[&a, &b]).await.unwrap_err();
  assert!(matches!(e, Error::NoSlotUnlocked { .. }));
  let (k, _) = KeySlots::load(&path).unwrap().unlock_any(&[&a, &b]).await.unwrap();
  assert_eq!(*k, *new_dk);
  assert_eq!(b.len(), 1);

  // Missing provider: refused before any change.
  let snapshot = std::fs::read(&path).unwrap();
  assert!(matches!(ks.rotate(&dk, &[&b]).await, Err(Error::NoProvider(_))));
  assert_eq!(std::fs::read(&path).unwrap(), snapshot);
}

#[tokio::test]
async fn wipe_destroys_and_deletes() {
  let (_d, path) = tmp();
  let a = Fake::new(ProviderKind::SecretService, false);
  let b = SessionProvider::new();
  let (mut ks, dk) = KeySlots::create_new(&path, &a).await.unwrap();
  ks.add_slot(&b, &dk).await.unwrap();
  ks.wipe(&[&a, &b]).await.unwrap();
  assert!(!path.exists());
  assert!(b.is_empty());
  assert!(a.inner.is_empty());

  // Destroy failure: file still deleted, failure reported.
  let (ks, _) = KeySlots::create_new(&path, &a).await.unwrap();
  a.set(Mode::FailDestroy);
  let e = ks.wipe(&[&a]).await.unwrap_err();
  assert!(matches!(e, Error::DestroyIncomplete { ref failures } if failures.len() == 1));
  assert!(!path.exists());
}

#[test]
fn default_path_uses_xdg_state_home() {
  // Only checks shape; does not mutate the environment.
  if let Some(p) = KeySlots::default_path() {
    assert!(p.ends_with("spool/keyslots.json"));
  }
}

#[test]
fn errors_do_not_leak_debug_key_material() {
  let s = format!(
    "{:?}",
    Slot {
      id: SlotId::new(),
      kind: ProviderKind::Session,
      params: SlotParams::Session,
      wrapped: vec![0xAB; WRAPPED_LEN]
    }
  );
  assert!(!s.contains("171"), "{s}");
}
