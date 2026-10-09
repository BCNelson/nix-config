//! FIDO2 provider logic against a fake authenticator. Nothing here touches
//! hidraw: every provider is built with `with_backend(fake)`.

use std::collections::HashMap;
use std::sync::Mutex;

use hmac::{KeyInit, Mac};

use super::*;
use crate::slots::KeySlots;

type HmacSha256 = hmac::Hmac<sha2::Sha256>;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Behaviour {
  Ok,
  Timeout,
  Denied,
  NoHmacSecret,
  Unplugged,
}

struct Cred {
  rp_id: String,
  secret: [u8; 32],
  secret_uv: [u8; 32],
}

struct FakeKey {
  path: String,
  pin: Option<String>,
  /// CTAP 2.0 key with a PIN set: makeCredential needs the PIN.
  pin_for_make: bool,
  behaviour: Behaviour,
  creds: HashMap<Vec<u8>, Cred>,
}

#[derive(Default)]
struct Fake {
  keys: Mutex<Vec<FakeKey>>,
  /// (device, operation, pin passed?)
  log: Mutex<Vec<(String, &'static str, bool)>>,
}

impl Fake {
  fn new() -> Arc<Self> {
    Arc::new(Fake::default())
  }
  fn plug(&self, path: &str) {
    self.plug_with(path, None, false);
  }
  fn plug_with(&self, path: &str, pin: Option<&str>, pin_for_make: bool) {
    self.keys.lock().unwrap().push(FakeKey {
      path: path.into(),
      pin: pin.map(Into::into),
      pin_for_make,
      behaviour: Behaviour::Ok,
      creds: HashMap::new(),
    });
  }
  fn unplug(&self, path: &str) -> FakeKey {
    let mut keys = self.keys.lock().unwrap();
    let i = keys.iter().position(|k| k.path == path).unwrap();
    keys.remove(i)
  }
  fn replug(&self, key: FakeKey) {
    self.keys.lock().unwrap().push(key);
  }
  fn set(&self, path: &str, b: Behaviour) {
    self.keys.lock().unwrap().iter_mut().find(|k| k.path == path).unwrap().behaviour = b;
  }
  fn log(&self) -> Vec<(String, &'static str, bool)> {
    self.log.lock().unwrap().clone()
  }
  fn clear_log(&self) {
    self.log.lock().unwrap().clear();
  }
  fn record(&self, dev: &str, op: &'static str, pin: bool) {
    self.log.lock().unwrap().push((dev.into(), op, pin));
  }
}

fn random32() -> [u8; 32] {
  let mut b = [0u8; 32];
  rand::rng().fill_bytes(&mut b);
  b
}

fn check_pin(key: &FakeKey, pin: Option<&str>) -> std::result::Result<(), DevError> {
  match (&key.pin, pin) {
    (_, None) => Err(DevError::PinRequired),
    (None, Some(_)) => Err(DevError::Unsupported("no PIN set".into())),
    (Some(want), Some(got)) if want == got => Ok(()),
    (Some(_), Some(_)) => Err(DevError::PinInvalid),
  }
}

impl Authenticator for Fake {
  fn devices(&self) -> std::result::Result<Vec<Fido2Device>, DevError> {
    Ok(
      self
        .keys
        .lock()
        .unwrap()
        .iter()
        .map(|k| Fido2Device {
          path: k.path.clone(),
          manufacturer: "Fake".into(),
          product: "Key".into(),
        })
        .collect(),
    )
  }

  fn make_credential(
    &self,
    dev: &str,
    rp_id: &str,
    user_id: &[u8],
    uv: bool,
    pin: Option<&str>,
  ) -> std::result::Result<Vec<u8>, DevError> {
    self.record(dev, "make", pin.is_some());
    assert_eq!(user_id.len(), 16);
    let mut keys = self.keys.lock().unwrap();
    let key = keys.iter_mut().find(|k| k.path == dev).ok_or(DevError::Gone("gone".into()))?;
    match key.behaviour {
      Behaviour::Timeout => return Err(DevError::Timeout),
      Behaviour::Denied => return Err(DevError::Denied),
      Behaviour::Unplugged => return Err(DevError::Gone("unplugged".into())),
      _ => {}
    }
    if uv || (key.pin_for_make && key.pin.is_some()) || pin.is_some() {
      check_pin(key, pin)?;
    }
    let id = random32().to_vec();
    key
      .creds
      .insert(id.clone(), Cred { rp_id: rp_id.into(), secret: random32(), secret_uv: random32() });
    Ok(id)
  }

  fn has_credential(
    &self,
    dev: &str,
    rp_id: &str,
    credential_id: &[u8],
  ) -> std::result::Result<bool, DevError> {
    self.record(dev, "probe", false);
    let keys = self.keys.lock().unwrap();
    let key = keys.iter().find(|k| k.path == dev).ok_or(DevError::Gone("gone".into()))?;
    if key.behaviour == Behaviour::Unplugged {
      return Err(DevError::Gone("unplugged".into()));
    }
    Ok(key.creds.get(credential_id).is_some_and(|c| c.rp_id == rp_id))
  }

  fn hmac_secret(
    &self,
    dev: &str,
    rp_id: &str,
    credential_id: &[u8],
    salt: &[u8; SALT_LEN],
    uv: bool,
    pin: Option<&str>,
  ) -> std::result::Result<Kek, DevError> {
    self.record(dev, "hmac", pin.is_some());
    let keys = self.keys.lock().unwrap();
    let key = keys.iter().find(|k| k.path == dev).ok_or(DevError::Gone("gone".into()))?;
    match key.behaviour {
      Behaviour::Timeout => return Err(DevError::Timeout),
      Behaviour::Denied => return Err(DevError::Denied),
      Behaviour::Unplugged => return Err(DevError::Gone("unplugged".into())),
      Behaviour::NoHmacSecret => {
        return Err(DevError::Unsupported("no hmac-secret".into()));
      }
      Behaviour::Ok => {}
    }
    let cred = match key.creds.get(credential_id) {
      Some(c) if c.rp_id == rp_id => c,
      _ => return Err(DevError::NoCredentials),
    };
    // Like libfido2: a PIN means UV was performed, which selects the UV secret.
    let did_uv = uv || pin.is_some();
    if did_uv {
      check_pin(key, pin)?;
    }
    let secret = if did_uv { &cred.secret_uv } else { &cred.secret };
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(secret).unwrap();
    mac.update(salt);
    let mut kek = Zeroizing::new([0u8; 32]);
    kek.copy_from_slice(&mac.finalize().into_bytes());
    Ok(kek)
  }
}

fn provider(fake: &Arc<Fake>, pin: Option<&str>) -> Fido2Provider {
  Fido2Provider::with_backend(fake.clone(), pin.map(|p| Zeroizing::new(p.to_owned())))
}

fn file(dir: &tempfile::TempDir) -> std::path::PathBuf {
  dir.path().join("keyslots.json")
}

fn fido_params(ks: &KeySlots, i: usize) -> Fido2Params {
  match ks.slots()[i].params() {
    SlotParams::Fido2Hmac(p) => p.clone(),
    other => panic!("{other:?}"),
  }
}

fn single_error(e: Error) -> Error {
  match e {
    Error::NoSlotUnlocked { mut attempts } if attempts.len() == 1 => attempts.remove(0).error,
    other => panic!("{other:?}"),
  }
}

// ---- happy paths -------------------------------------------------------------

#[tokio::test]
async fn enroll_unlock_reload() {
  let fake = Fake::new();
  fake.plug("/dev/hidraw0");
  let dir = tempfile::tempdir().unwrap();
  let p = provider(&fake, None);
  assert!(p.interactive());
  assert_eq!(p.kind(), ProviderKind::Fido2Hmac);
  let (ks, dk) = KeySlots::create_new(file(&dir), &p).await.unwrap();
  let params = fido_params(&ks, 0);
  assert_eq!(params.rp_id, DEFAULT_RP_ID);
  assert!(!params.uv);
  // Enroll = make credential + one hmac (two touches), no PIN sent.
  assert_eq!(
    fake.log().iter().map(|(_, op, pin)| (*op, *pin)).collect::<Vec<_>>(),
    [("make", false), ("hmac", false)]
  );

  // On-disk format.
  let json: serde_json::Value =
    serde_json::from_slice(&std::fs::read(file(&dir)).unwrap()).unwrap();
  let on_disk = &json["slots"][0]["params"];
  let keys: Vec<_> = on_disk.as_object().unwrap().keys().cloned().collect();
  assert_eq!(keys, ["credential_id", "rp_id", "salt", "uv"]);
  assert_eq!(on_disk["rp_id"], "spool:clipboard");
  assert_eq!(on_disk["uv"], false);
  assert_eq!(B64.decode(on_disk["salt"].as_str().unwrap()).unwrap().len(), 32);
  assert_eq!(B64.decode(on_disk["credential_id"].as_str().unwrap()).unwrap(), params.credential_id);

  let ks2 = KeySlots::load(file(&dir)).unwrap();
  assert_eq!(fido_params(&ks2, 0), params);
  let (got, id) = ks2.unlock_any(&[&provider(&fake, None)]).await.unwrap();
  assert_eq!(*got, *dk);
  assert_eq!(id, ks.slots()[0].id());

  // Destroy is a no-op and idempotent.
  let p = provider(&fake, None);
  p.destroy(&id, ks.slots()[0].params()).await.unwrap();
  p.destroy(&id, ks.slots()[0].params()).await.unwrap();
}

#[tokio::test]
async fn kek_is_hmac_of_salt() {
  let fake = Fake::new();
  fake.plug("/dev/hidraw0");
  let p = provider(&fake, None);
  let (params, kek) = p.enroll(&SlotId::new()).await.unwrap();
  let SlotParams::Fido2Hmac(fp) = &params else { panic!() };
  let keys = fake.keys.lock().unwrap();
  let cred = &keys[0].creds[&fp.credential_id];
  let mut mac = <HmacSha256 as KeyInit>::new_from_slice(&cred.secret).unwrap();
  mac.update(&fp.salt);
  assert_eq!(kek.as_slice(), mac.finalize().into_bytes().as_slice());
}

#[tokio::test]
async fn salts_and_credentials_are_per_slot() {
  let fake = Fake::new();
  fake.plug("/dev/hidraw0");
  let p = provider(&fake, None);
  let (a, ka) = p.enroll(&SlotId::new()).await.unwrap();
  let (b, kb) = p.enroll(&SlotId::new()).await.unwrap();
  let (SlotParams::Fido2Hmac(a), SlotParams::Fido2Hmac(b)) = (a, b) else { panic!() };
  assert_ne!(a.salt, b.salt);
  assert_ne!(a.credential_id, b.credential_id);
  assert_ne!(*ka, *kb);
}

// ---- multiple keys -----------------------------------------------------------

#[tokio::test]
async fn one_slot_per_key_and_wrong_key_is_skipped() {
  let fake = Fake::new();
  let dir = tempfile::tempdir().unwrap();
  fake.plug("/dev/hidraw0");
  let (mut ks, dk) = KeySlots::create_new(file(&dir), &provider(&fake, None)).await.unwrap();
  let key_a = fake.unplug("/dev/hidraw0");
  fake.plug("/dev/hidraw1");
  ks.add_slot(&provider(&fake, None), &dk).await.unwrap();
  let slot_b = ks.slots()[1].id();

  // Only key B connected: slot A is unavailable, slot B unlocks.
  fake.clear_log();
  let (got, id) = ks.unlock_any(&[&provider(&fake, None)]).await.unwrap();
  assert_eq!((*got, id), (*dk, slot_b));
  // Slot A was only probed on B (no touch requested for it).
  let log = fake.log();
  assert_eq!(log[0], ("/dev/hidraw1".into(), "probe", false));
  assert_eq!(log[1], ("/dev/hidraw1".into(), "probe", false));
  assert_eq!(log[2], ("/dev/hidraw1".into(), "hmac", false));
  assert_eq!(log.len(), 3);

  // Both connected, B first in enumeration order: slot A's unlock skips B.
  fake.replug(key_a);
  fake.clear_log();
  let a_params = ks.slots()[0].params().clone();
  let kek = provider(&fake, None).unlock(&ks.slots()[0].id(), &a_params).await.unwrap();
  assert_eq!(kek.len(), 32);
  let ops: Vec<_> = fake.log().into_iter().map(|(d, op, _)| (d, op)).collect();
  assert_eq!(
    ops,
    [
      ("/dev/hidraw1".to_owned(), "probe"),
      ("/dev/hidraw0".to_owned(), "probe"),
      ("/dev/hidraw0".to_owned(), "hmac")
    ]
  );
}

#[tokio::test]
async fn credential_on_no_connected_key_is_unavailable() {
  let fake = Fake::new();
  fake.plug("/dev/hidraw0");
  let (params, _) = provider(&fake, None).enroll(&SlotId::new()).await.unwrap();
  fake.unplug("/dev/hidraw0");
  fake.plug("/dev/hidraw1");
  fake.plug("/dev/hidraw2");
  fake.clear_log();
  let e = provider(&fake, None).unlock(&SlotId::new(), &params).await.unwrap_err();
  assert!(matches!(e, Error::Unavailable { kind: ProviderKind::Fido2Hmac, .. }), "{e:?}");
  assert!(e.is_retryable());
  // Both wrong keys were only probed: no touch was requested.
  let ops: Vec<_> = fake.log().into_iter().map(|(_, op, _)| op).collect();
  assert_eq!(ops, ["probe", "probe"]);
}

#[tokio::test]
async fn no_device_is_unavailable() {
  let fake = Fake::new();
  let e = provider(&fake, None).enroll(&SlotId::new()).await.unwrap_err();
  assert!(matches!(e, Error::Unavailable { .. }) && e.is_retryable());
  fake.plug("/dev/hidraw0");
  let (params, _) = provider(&fake, None).enroll(&SlotId::new()).await.unwrap();
  fake.unplug("/dev/hidraw0");
  let e = provider(&fake, None).unlock(&SlotId::new(), &params).await.unwrap_err();
  assert!(matches!(e, Error::Unavailable { .. }) && e.is_retryable());
}

#[tokio::test]
async fn enroll_with_several_keys_needs_a_path() {
  let fake = Fake::new();
  fake.plug("/dev/hidraw0");
  fake.plug("/dev/hidraw1");
  let e = provider(&fake, None).enroll(&SlotId::new()).await.unwrap_err();
  assert!(matches!(e, Error::Unavailable { .. }), "{e:?}");
  let devs = provider(&fake, None).devices().await.unwrap();
  assert_eq!(devs.len(), 2);
  let p = provider(&fake, None).with_device_path(devs[1].path.clone());
  p.enroll(&SlotId::new()).await.unwrap();
  assert!(fake.keys.lock().unwrap()[0].creds.is_empty());
  assert_eq!(fake.keys.lock().unwrap()[1].creds.len(), 1);
  let e = provider(&fake, None).with_device_path("/dev/nope").enroll(&SlotId::new()).await;
  assert!(matches!(e, Err(Error::Unavailable { .. })));
}

#[tokio::test]
async fn unplugged_key_is_skipped() {
  let fake = Fake::new();
  fake.plug("/dev/hidraw0");
  let (params, kek) = provider(&fake, None).enroll(&SlotId::new()).await.unwrap();
  fake.plug("/dev/hidraw9");
  fake.set("/dev/hidraw9", Behaviour::Unplugged);
  // hidraw9 enumerates first after a replug of hidraw0.
  let k0 = fake.unplug("/dev/hidraw0");
  fake.replug(k0);
  assert_eq!(*provider(&fake, None).unlock(&SlotId::new(), &params).await.unwrap(), *kek);
  fake.unplug("/dev/hidraw0");
  let e = provider(&fake, None).unlock(&SlotId::new(), &params).await.unwrap_err();
  assert!(matches!(e, Error::Unavailable { .. }));
}

// ---- touch / errors --------------------------------------------------------------

#[tokio::test]
async fn timeout_and_cancel_are_dismissed() {
  let fake = Fake::new();
  fake.plug("/dev/hidraw0");
  let dir = tempfile::tempdir().unwrap();
  let (ks, _) = KeySlots::create_new(file(&dir), &provider(&fake, None)).await.unwrap();
  for b in [Behaviour::Timeout, Behaviour::Denied] {
    fake.set("/dev/hidraw0", b);
    let e = ks.unlock_any(&[&provider(&fake, None)]).await.unwrap_err();
    assert!(e.is_retryable());
    assert!(matches!(single_error(e), Error::Dismissed(ProviderKind::Fido2Hmac)));
    let e = provider(&fake, None).enroll(&SlotId::new()).await.unwrap_err();
    assert!(matches!(e, Error::Dismissed(_)));
  }
}

#[tokio::test]
async fn key_without_hmac_secret_is_fatal() {
  let fake = Fake::new();
  fake.plug("/dev/hidraw0");
  fake.set("/dev/hidraw0", Behaviour::NoHmacSecret);
  let e = provider(&fake, None).enroll(&SlotId::new()).await.unwrap_err();
  assert!(matches!(e, Error::Provider { kind: ProviderKind::Fido2Hmac, .. }));
  assert!(!e.is_retryable());
}

// ---- PIN / UV ----------------------------------------------------------------------

#[tokio::test]
async fn pin_needed_for_make_credential() {
  let fake = Fake::new();
  fake.plug_with("/dev/hidraw0", Some("1234"), true);
  let e = provider(&fake, None).enroll(&SlotId::new()).await.unwrap_err();
  assert!(matches!(e, Error::NeedsSecret(ProviderKind::Fido2Hmac)) && e.is_retryable());
  assert!(e.needs_secret());
  let e = provider(&fake, Some("0000")).enroll(&SlotId::new()).await.unwrap_err();
  assert!(matches!(e, Error::PinInvalid(_)) && !e.is_retryable() && e.needs_secret());

  fake.clear_log();
  let dir = tempfile::tempdir().unwrap();
  let (ks, dk) = KeySlots::create_new(file(&dir), &provider(&fake, Some("1234"))).await.unwrap();
  assert!(!fido_params(&ks, 0).uv);
  // PIN used for make, but NOT for the (non-UV) hmac.
  let ops: Vec<_> = fake.log().into_iter().map(|(_, op, pin)| (op, pin)).collect();
  assert_eq!(ops, [("make", true), ("hmac", false)]);
  // Unlock needs no PIN; a PIN handed in anyway is not sent (it would
  // select the UV secret and give the wrong KEK).
  assert_eq!(*ks.unlock_any(&[&provider(&fake, None)]).await.unwrap().0, *dk);
  fake.clear_log();
  assert_eq!(*ks.unlock_any(&[&provider(&fake, Some("1234"))]).await.unwrap().0, *dk);
  assert!(fake.log().iter().all(|(_, _, pin)| !pin));
}

#[tokio::test]
async fn require_uv_slots() {
  let fake = Fake::new();
  fake.plug_with("/dev/hidraw0", Some("1234"), false);
  let dir = tempfile::tempdir().unwrap();
  let p = provider(&fake, Some("1234")).require_uv(true);
  let (ks, dk) = KeySlots::create_new(file(&dir), &p).await.unwrap();
  assert!(fido_params(&ks, 0).uv);
  let json = std::fs::read_to_string(file(&dir)).unwrap();
  assert!(json.contains("\"uv\": true"));

  // No PIN -> NeedsSecret (prompt), wrong PIN -> PinInvalid, right PIN -> ok.
  let e = ks.unlock_any(&[&provider(&fake, None)]).await.unwrap_err();
  assert!(e.needs_secret() && e.is_retryable());
  assert_eq!(e.secret_kinds(), [ProviderKind::Fido2Hmac]);
  let e = ks.unlock_any(&[&provider(&fake, Some("9999"))]).await.unwrap_err();
  assert!(e.needs_secret() && !e.is_retryable());
  assert!(matches!(single_error(e), Error::PinInvalid(ProviderKind::Fido2Hmac)));
  // The provider's own require_uv flag does not matter for unlock.
  let (got, _) = ks.unlock_any(&[&provider(&fake, Some("1234"))]).await.unwrap();
  assert_eq!(*got, *dk);
}

#[tokio::test]
async fn uv_and_non_uv_keks_differ() {
  let fake = Fake::new();
  fake.plug_with("/dev/hidraw0", Some("1234"), false);
  let (params, kek) = provider(&fake, None).enroll(&SlotId::new()).await.unwrap();
  let SlotParams::Fido2Hmac(mut fp) = params else { panic!() };
  // Flip uv in the (tampered) params: the key now answers with its UV
  // secret, so the KEK differs and unwrapping would fail with WrongKey.
  fp.uv = true;
  let other =
    provider(&fake, Some("1234")).unlock(&SlotId::new(), &SlotParams::Fido2Hmac(fp)).await.unwrap();
  assert_ne!(*other, *kek);
}

#[test]
fn error_mapping() {
  let cases = [
    (DevError::NoCredentials, true),
    (DevError::PinRequired, true),
    (DevError::PinInvalid, false),
    (DevError::PinBlocked, false),
    (DevError::Timeout, true),
    (DevError::Denied, true),
    (DevError::Unsupported("x".into()), false),
    (DevError::Gone("x".into()), true),
    (DevError::Other("x".into()), false),
  ];
  for (d, retryable) in cases {
    let e = map_err(d.clone());
    assert_eq!(e.is_retryable(), retryable, "{d:?} -> {e:?}");
  }
  assert!(matches!(map_err(DevError::PinBlocked), Error::PinBlocked(ProviderKind::Fido2Hmac)));
  assert!(matches!(map_err(DevError::Timeout), Error::Dismissed(ProviderKind::Fido2Hmac)));
}

// ---- params ----------------------------------------------------------------------

fn write_file(dir: &tempfile::TempDir, params: serde_json::Value) -> std::path::PathBuf {
  let path = file(dir);
  let v = serde_json::json!({
    "version": 1,
    "data_key_id": uuid::Uuid::new_v4(),
    "slots": [{
      "id": uuid::Uuid::new_v4(),
      "provider": "fido2-hmac",
      "params": params,
      "wrapped": B64.encode([0u8; crate::wrap::WRAPPED_LEN]),
    }],
  });
  std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
  path
}

fn good() -> serde_json::Value {
  serde_json::json!({
    "credential_id": B64.encode([5u8; 64]),
    "salt": B64.encode([6u8; 32]),
    "rp_id": "spool:clipboard",
    "uv": false,
  })
}

#[test]
fn load_accepts_good_params() {
  let dir = tempfile::tempdir().unwrap();
  let ks = KeySlots::load(write_file(&dir, good())).unwrap();
  let p = fido_params(&ks, 0);
  assert_eq!(
    (p.credential_id, p.salt, p.rp_id.as_str(), p.uv),
    (vec![5; 64], [6; 32], "spool:clipboard", false)
  );
}

#[test]
fn load_rejects_bad_params() {
  let mut cases: Vec<(&str, serde_json::Value)> = Vec::new();
  let mut with = |name, k: &str, v: serde_json::Value| {
    let mut g = good();
    g[k] = v;
    cases.push((name, g));
  };
  with("empty credential", "credential_id", "".into());
  with("huge credential", "credential_id", B64.encode(vec![1u8; 1024]).into());
  with("bad b64 credential", "credential_id", "%%".into());
  with("short salt", "salt", B64.encode([1u8; 16]).into());
  with("long salt", "salt", B64.encode([1u8; 64]).into());
  with("empty rp", "rp_id", "".into());
  with("rp with space", "rp_id", "spool clipboard".into());
  with("rp non-ascii", "rp_id", "spööl".into());
  with("uv string", "uv", "false".into());
  with("unknown field", "pin", "1234".into());
  for k in ["credential_id", "salt", "rp_id", "uv"] {
    let mut g = good();
    g.as_object_mut().unwrap().remove(k);
    cases.push(("missing field", g));
  }
  for (name, params) in cases {
    let dir = tempfile::tempdir().unwrap();
    let e = KeySlots::load(write_file(&dir, params)).unwrap_err();
    assert!(matches!(e, Error::Corrupt { .. }), "{name}: {e:?}");
  }
}

#[tokio::test]
async fn unlock_rejects_foreign_params() {
  let fake = Fake::new();
  let e = provider(&fake, None).unlock(&SlotId::new(), &SlotParams::Session).await.unwrap_err();
  assert!(matches!(e, Error::ProviderCorrupt { .. }));
  assert!(fake.log().is_empty());
}

#[test]
fn debug_hides_secrets() {
  let p = Fido2Params {
    credential_id: vec![0xAB; 40],
    salt: [0xCD; 32],
    rp_id: DEFAULT_RP_ID.into(),
    uv: false,
  };
  let d = format!("{p:?}");
  assert!(!d.contains("171") && !d.contains("205") && !d.contains("q6vr"), "{d}");
  let prov = Fido2Provider::new(Some(Zeroizing::new("pin-1234-secret".into())));
  assert!(!format!("{prov:?}").contains("1234"));
}
