//! Opt-in hardware test for the FIDO2 provider. Talks to REAL security keys,
//! so it does nothing unless `SPOOL_FIDO2_HW_TESTS=1`.
//!
//! Manual run (from the dev shell, exactly one key plugged in, or set
//! `SPOOL_FIDO2_DEVICE=/dev/hidrawN`):
//!
//! ```sh
//! SPOOL_FIDO2_HW_TESTS=1 cargo test -p spool-keys --test fido2_hw -- --nocapture --test-threads=1
//! ```
//!
//! Touch the key when it blinks: twice to enroll, once to unlock. Optional:
//! `SPOOL_FIDO2_PIN=<pin>` if the key wants its PIN to create credentials,
//! `SPOOL_FIDO2_UV=1` to enroll with user verification (needs the PIN, or a
//! bio key). The test creates a non-resident credential (nothing is stored
//! on the key) in a temporary keyslots file and never prints key material.
#![cfg(feature = "fido2")]

use spool_keys::{Fido2Provider, KeySlots};
use zeroize::Zeroizing;

fn enabled() -> bool {
  if std::env::var("SPOOL_FIDO2_HW_TESTS").as_deref() == Ok("1") {
    return true;
  }
  eprintln!("skipping: set SPOOL_FIDO2_HW_TESTS=1 to run against a real security key");
  false
}

fn provider() -> Fido2Provider {
  let pin = std::env::var("SPOOL_FIDO2_PIN").ok().map(Zeroizing::new);
  let mut p =
    Fido2Provider::new(pin).require_uv(std::env::var("SPOOL_FIDO2_UV").as_deref() == Ok("1"));
  if let Ok(dev) = std::env::var("SPOOL_FIDO2_DEVICE") {
    p = p.with_device_path(dev);
  }
  p
}

#[tokio::test(flavor = "multi_thread")]
async fn hw_enroll_unlock() {
  if !enabled() {
    return;
  }
  let p = provider();
  let devices = p.devices().await.expect("enumerate security keys");
  for d in &devices {
    eprintln!("found: {} {} at {}", d.manufacturer, d.product, d.path);
  }
  assert!(!devices.is_empty(), "no FIDO2 security key connected");

  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("keyslots.json");
  eprintln!(">>> enrolling: touch the key (twice)");
  let (ks, dk) = KeySlots::create_new(&path, &p).await.expect("enroll");

  let ks = KeySlots::load(ks.path()).expect("reload");
  eprintln!(">>> unlocking: touch the key (once)");
  let (got, _) = ks.unlock_any(&[&provider()]).await.expect("unlock");
  assert!(*got == *dk, "unlocked data key differs");
  eprintln!("ok: enroll + reload + unlock round trip");
}
