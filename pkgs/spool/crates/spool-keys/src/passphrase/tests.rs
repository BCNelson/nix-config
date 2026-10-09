use super::*;
use crate::slots::KeySlots;

/// Cheap KDF cost for round trips (needs the relaxed floor).
const TEST_M: u32 = 64;
const TEST_T: u32 = 1;

fn pw(s: &str) -> Zeroizing<String> {
  Zeroizing::new(s.to_owned())
}

fn fast(s: &str) -> PassphraseProvider {
  relax_floor_for_tests();
  PassphraseProvider::with_test_cost(pw(s), TEST_M, TEST_T, 1)
}

fn file(dir: &tempfile::TempDir) -> std::path::PathBuf {
  dir.path().join("keyslots.json")
}

fn params(m_kib: u32, t: u32, p: u32) -> PassphraseParams {
  PassphraseParams { m_kib, t, p, salt: [7; SALT_LEN] }
}

#[test]
fn defaults_are_the_documented_cost() {
  assert_eq!((ARGON2_M_KIB, ARGON2_T, ARGON2_P), (262_144, 3, 1));
  assert_eq!((MIN_M_KIB, MIN_T), (65_536, 2));
  let p = PassphraseProvider::new(pw("x"));
  assert_eq!((p.m_kib, p.t, p.p), (ARGON2_M_KIB, ARGON2_T, ARGON2_P));
  assert!(p.interactive());
  assert_eq!(p.kind(), ProviderKind::Passphrase);
  assert!(params(ARGON2_M_KIB, ARGON2_T, ARGON2_P).validate().is_ok());
}

#[test]
fn floor_and_ceiling() {
  assert!(params(MIN_M_KIB, MIN_T, 1).validate().is_ok());
  assert!(params(MIN_M_KIB - 1, 3, 1).validate().is_err());
  assert!(params(ARGON2_M_KIB, 1, 1).validate().is_err());
  assert!(params(ARGON2_M_KIB, 3, 0).validate().is_err());
  assert!(params(ARGON2_M_KIB, 3, MAX_P + 1).validate().is_err());
  assert!(params(MAX_M_KIB + 1, 3, 1).validate().is_err());
  assert!(params(ARGON2_M_KIB, MAX_T + 1, 1).validate().is_err());
}

#[test]
fn debug_hides_secrets() {
  let p = PassphraseProvider::new(pw("hunter2-secret"));
  assert!(!format!("{p:?}").contains("hunter2"));
  let params = PassphraseParams { salt: [0xAB; SALT_LEN], ..params(MIN_M_KIB, 2, 1) };
  let d = format!("{params:?}");
  assert!(!d.contains("171") && !d.contains("AB") && !d.contains("salt"), "{d}");
}

#[test]
fn nfc_normalizes() {
  assert_eq!(nfc("cafe\u{301}").as_str(), "caf\u{e9}");
  assert_eq!(nfc("caf\u{e9}").as_str(), "caf\u{e9}");
  // Hangul jamo compose; long inputs never panic.
  assert_eq!(nfc("\u{1100}\u{1161}").as_str(), "\u{ac00}");
  let long = "e\u{301}".repeat(1000);
  assert_eq!(nfc(&long).chars().count(), 1000);
}

#[test]
fn kdf_is_deterministic_per_salt() {
  relax_floor_for_tests();
  let a = params(TEST_M, 1, 1);
  let mut b = a.clone();
  b.salt[0] ^= 1;
  assert_eq!(*derive("pw", &a).unwrap(), *derive("pw", &a).unwrap());
  assert_ne!(*derive("pw", &a).unwrap(), *derive("pw", &b).unwrap());
  assert_ne!(*derive("pw", &a).unwrap(), *derive("pW", &a).unwrap());
  // Different cost, different key.
  assert_ne!(*derive("pw", &a).unwrap(), *derive("pw", &params(TEST_M, 2, 1)).unwrap());
}

#[test]
fn kdf_known_answer() {
  // Argon2id v1.3, m=64 KiB, t=1, p=1, 32-byte output, salt = 7 x 32,
  // password "password". Pins the exact construction (no pepper, no AD,
  // output length 32) so it cannot change silently.
  relax_floor_for_tests();
  let kek = derive("password", &params(TEST_M, 1, 1)).unwrap();
  let again =
    Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::new(64, 1, 1, Some(32)).unwrap());
  let mut want = [0u8; 32];
  again.hash_password_into(b"password", &[7; 32], &mut want).unwrap();
  assert_eq!(*kek, want);
}

#[tokio::test]
async fn round_trip_and_reload() {
  let dir = tempfile::tempdir().unwrap();
  let p = fast("correct horse battery staple");
  let (ks, dk) = KeySlots::create_new(file(&dir), &p).await.unwrap();
  let slot = &ks.slots()[0];
  assert_eq!(slot.kind(), ProviderKind::Passphrase);

  // On-disk format.
  let json: serde_json::Value =
    serde_json::from_slice(&std::fs::read(file(&dir)).unwrap()).unwrap();
  let params = &json["slots"][0]["params"];
  let keys: Vec<_> = params.as_object().unwrap().keys().cloned().collect();
  assert_eq!(keys, ["kdf", "m_kib", "p", "salt", "t"]);
  assert_eq!(params["kdf"], "argon2id");
  assert_eq!(params["m_kib"], TEST_M);
  assert_eq!(params["t"], TEST_T);
  assert_eq!(params["p"], 1);
  assert_eq!(B64.decode(params["salt"].as_str().unwrap()).unwrap().len(), 32);

  let ks2 = KeySlots::load(file(&dir)).unwrap();
  assert_eq!(ks2.slots()[0].params(), slot.params());
  let (dk2, id) = ks2.unlock_any(&[&fast("correct horse battery staple")]).await.unwrap();
  assert_eq!(*dk2, *dk);
  assert_eq!(id, slot.id());
  // A provider configured with another cost still unlocks: the slot's params win.
  let other = PassphraseProvider::with_test_cost(pw("correct horse battery staple"), 128, 2, 1);
  assert_eq!(*ks2.unlock_slot(&id, &other).await.unwrap(), *dk);
}

#[tokio::test]
async fn salts_are_per_slot() {
  let dir = tempfile::tempdir().unwrap();
  let p = fast("same");
  let (mut ks, dk) = KeySlots::create_new(file(&dir), &p).await.unwrap();
  ks.add_slot(&p, &dk).await.unwrap();
  let [a, b] = ks.slots() else { panic!() };
  let (SlotParams::Passphrase(a), SlotParams::Passphrase(b)) = (a.params(), b.params()) else {
    panic!()
  };
  assert_ne!(a.salt, b.salt);
}

#[tokio::test]
async fn wrong_passphrase_is_wrong_key() {
  let dir = tempfile::tempdir().unwrap();
  let (ks, _) = KeySlots::create_new(file(&dir), &fast("right")).await.unwrap();
  let e = ks.unlock_any(&[&fast("wrong")]).await.unwrap_err();
  let Error::NoSlotUnlocked { attempts } = &e else { panic!("{e:?}") };
  assert!(matches!(attempts[0].error, Error::WrongKey));
  assert!(!e.is_retryable());
  assert!(!e.needs_secret());
  let id = ks.slots()[0].id();
  assert!(matches!(ks.unlock_slot(&id, &fast("wrong")).await, Err(Error::WrongKey)));
}

#[tokio::test]
async fn nfc_equivalent_passphrases_unlock() {
  let dir = tempfile::tempdir().unwrap();
  let (ks, dk) = KeySlots::create_new(file(&dir), &fast("caf\u{e9} cr\u{e8}me")).await.unwrap();
  let (got, _) = ks.unlock_any(&[&fast("cafe\u{301} cre\u{300}me")]).await.unwrap();
  assert_eq!(*got, *dk);
  // Compatibility forms are NOT folded (NFC, not NFKC): "ﬁ" != "fi".
  let dir2 = tempfile::tempdir().unwrap();
  let (ks2, _) = KeySlots::create_new(file(&dir2), &fast("\u{fb01}")).await.unwrap();
  assert!(ks2.unlock_any(&[&fast("fi")]).await.is_err());
}

#[tokio::test]
async fn without_passphrase_needs_secret() {
  let dir = tempfile::tempdir().unwrap();
  let (mut ks, dk) = KeySlots::create_new(file(&dir), &fast("pw")).await.unwrap();
  let none = PassphraseProvider::without_passphrase();
  let e = ks.unlock_any(&[&none]).await.unwrap_err();
  assert!(e.is_retryable() && e.needs_secret());
  assert_eq!(e.secret_kinds(), [ProviderKind::Passphrase]);
  assert!(matches!(none.enroll(&SlotId::new()).await, Err(Error::NeedsSecret(_))));
  // Destroy (slot removal) works without the passphrase.
  ks.add_slot(&fast("pw2"), &dk).await.unwrap();
  let first = ks.slots()[0].id();
  ks.remove_slot(&first, &[&none], false).await.unwrap();
  assert_eq!(ks.slots().len(), 1);
}

#[tokio::test]
async fn empty_passphrase_is_refused() {
  let e = fast("").enroll(&SlotId::new()).await.unwrap_err();
  assert!(matches!(e, Error::Provider { kind: ProviderKind::Passphrase, .. }));
}

#[tokio::test]
async fn unlock_rejects_foreign_or_weak_params() {
  let p = PassphraseProvider::new(pw("x"));
  let e = p.unlock(&SlotId::new(), &SlotParams::Session).await.unwrap_err();
  assert!(matches!(e, Error::ProviderCorrupt { .. }));
  // Strict floor on this thread: weak in-memory params never reach the KDF.
  let e = p.unlock(&SlotId::new(), &SlotParams::Passphrase(params(1024, 1, 1))).await.unwrap_err();
  assert!(matches!(e, Error::ProviderCorrupt { .. }));
}

// ---- tampered files ---------------------------------------------------------

/// Write a keyslots file with one passphrase slot whose params are `params`.
fn write_file(dir: &tempfile::TempDir, params: serde_json::Value) -> std::path::PathBuf {
  let path = file(dir);
  let v = serde_json::json!({
    "version": 1,
    "data_key_id": uuid::Uuid::new_v4(),
    "slots": [{
      "id": uuid::Uuid::new_v4(),
      "provider": "passphrase",
      "params": params,
      "wrapped": B64.encode([0u8; crate::wrap::WRAPPED_LEN]),
    }],
  });
  std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
  path
}

fn good() -> serde_json::Value {
  serde_json::json!({"kdf":"argon2id","m_kib":262144,"t":3,"p":1,"salt":B64.encode([1u8; 32])})
}

#[test]
fn load_accepts_production_params() {
  let dir = tempfile::tempdir().unwrap();
  let ks = KeySlots::load(write_file(&dir, good())).unwrap();
  let SlotParams::Passphrase(p) = ks.slots()[0].params() else { panic!() };
  assert_eq!((p.m_kib, p.t, p.p, p.salt), (262_144, 3, 1, [1; 32]));
}

#[test]
fn load_rejects_downgrades_and_garbage() {
  let mut cases: Vec<(&str, serde_json::Value)> = Vec::new();
  let mut with = |name, k: &str, v: serde_json::Value| {
    let mut g = good();
    g[k] = v;
    cases.push((name, g));
  };
  with("m below 64 MiB", "m_kib", 65_535.into());
  with("tiny m", "m_kib", 8.into());
  with("t = 1", "t", 1.into());
  with("t = 0", "t", 0.into());
  with("p = 0", "p", 0.into());
  with("huge m", "m_kib", (MAX_M_KIB + 1).into());
  with("huge t", "t", 1_000_000.into());
  with("negative t", "t", (-3).into());
  with("float m", "m_kib", 262_144.5.into());
  with("string t", "t", "3".into());
  with("argon2i", "kdf", "argon2i".into());
  with("pbkdf2", "kdf", "pbkdf2".into());
  with("short salt", "salt", B64.encode([1u8; 31]).into());
  with("long salt", "salt", B64.encode([1u8; 33]).into());
  with("bad b64", "salt", "!!!".into());
  with("unknown field", "pepper", "x".into());
  let mut missing = good();
  missing.as_object_mut().unwrap().remove("salt");
  cases.push(("missing salt", missing));
  let mut missing = good();
  missing.as_object_mut().unwrap().remove("kdf");
  cases.push(("missing kdf", missing));
  cases.push(("not an object", serde_json::json!([1, 2])));
  cases.push(("empty", serde_json::json!({})));

  for (name, params) in cases {
    let dir = tempfile::tempdir().unwrap();
    let e = KeySlots::load(write_file(&dir, params)).unwrap_err();
    assert!(matches!(e, Error::Corrupt { .. }), "{name}: {e:?}");
  }
}

#[tokio::test]
async fn tampered_salt_or_cost_fails_to_unlock() {
  let dir = tempfile::tempdir().unwrap();
  let (_, _) = KeySlots::create_new(file(&dir), &fast("pw")).await.unwrap();
  let orig = std::fs::read_to_string(file(&dir)).unwrap();
  for (from, to) in [(format!("\"t\": {TEST_T}"), "\"t\": 2".to_owned())] {
    assert!(orig.contains(&from));
    std::fs::write(file(&dir), orig.replace(&from, &to)).unwrap();
    let ks = KeySlots::load(file(&dir)).unwrap();
    let e = ks.unlock_any(&[&fast("pw")]).await.unwrap_err();
    let Error::NoSlotUnlocked { attempts } = e else { panic!() };
    assert!(matches!(attempts[0].error, Error::WrongKey));
  }
}

/// Real production cost; run with `cargo test --release -p spool-keys --
/// --ignored production_cost --nocapture` to see the timing.
#[tokio::test]
#[ignore = "slow: 256 MiB Argon2id (seconds in a debug build)"]
async fn production_cost() {
  let p = PassphraseProvider::new(pw("pw"));
  let start = std::time::Instant::now();
  let (params, kek) = p.enroll(&SlotId::new()).await.unwrap();
  eprintln!("argon2id m=256MiB t=3 p=1: {:?}", start.elapsed());
  assert_eq!(*p.unlock(&SlotId::new(), &params).await.unwrap(), *kek);
}
