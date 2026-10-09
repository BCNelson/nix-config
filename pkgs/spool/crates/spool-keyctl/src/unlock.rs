//! Interactive unlock of the data key through an existing slot, validated
//! against the history database.

use spool_core::store::Store;
use spool_crypto::DataKey;
use spool_keys::{Error as KeysError, KeySlots, ProviderKind, Slot, SlotId};

use crate::providers::Fido2Opts;
use crate::{Ctx, KeyctlError};

/// How many times a passphrase / PIN is asked for.
const SECRET_TRIES: usize = 3;

/// A data key proven to open the store.
pub(crate) struct Unlocked {
  /// The data key.
  pub key: DataKey,
  /// Index into the `files` passed to [`unlock`] whose slot matched.
  pub file: usize,
  /// The slot that unlocked.
  pub slot: SlotId,
  /// The opened store (`None` if there is no `history.db` yet).
  pub store: Option<Store>,
}

enum Verdict {
  Opened(Box<Unlocked>),
  /// The slot's key does not open the database.
  Stale,
}

/// Turn off SQLCipher's own logging on this connection (process-global in
/// practice), as spoold does.
fn quiet_sqlcipher(store: &Store) {
  let _ = store.conn().execute_batch("PRAGMA cipher_log_level = NONE;");
}

/// Check a candidate key against `history.db`.
fn validate(
  ctx: &Ctx<'_>,
  key: spool_keys::DataKey,
  file: usize,
  slot: &Slot,
) -> Result<Verdict, KeyctlError> {
  let key = DataKey::from_bytes(key);
  if !ctx.paths.has_db() {
    return Ok(Verdict::Opened(Box::new(Unlocked { key, file, slot: slot.id(), store: None })));
  }
  match Store::open_encrypted(&ctx.paths.dir, &key) {
    Ok(store) => {
      quiet_sqlcipher(&store);
      Ok(Verdict::Opened(Box::new(Unlocked { key, file, slot: slot.id(), store: Some(store) })))
    }
    Err(spool_core::Error::WrongKey) => {
      ctx.ui.note(&format!(
        "  slot {} unlocked, but its key does not open {}",
        slot.id(),
        ctx.paths.db().display()
      ));
      Ok(Verdict::Stale)
    }
    Err(spool_core::Error::NotEncrypted) => Err(KeyctlError::Other(anyhow::anyhow!(
      "{} is an unencrypted (M1 development) database; start spoold once to quarantine it, or delete it",
      ctx.paths.db().display()
    ))),
    Err(e) => Err(KeyctlError::Other(
      anyhow::Error::new(e).context(format!("opening {}", ctx.paths.db().display())),
    )),
  }
}

fn describe(e: &KeysError) -> String {
  // spool-keys errors never contain secrets.
  e.to_string()
}

/// Unlock the data key with a slot from any of `files` (tried in order of
/// kind: Secret Service, FIDO2, passphrase), validating each candidate by
/// opening `history.db`. A key that unwraps but does not open the database
/// (stale slot, or the other side of an interrupted rotation) moves on to
/// the next slot.
pub(crate) async fn unlock(ctx: &Ctx<'_>, files: &[&KeySlots]) -> Result<Unlocked, KeyctlError> {
  let mut slots: Vec<(usize, &Slot)> =
    files.iter().enumerate().flat_map(|(i, ks)| ks.slots().iter().map(move |s| (i, s))).collect();
  if let Some(only) = ctx.unlock_slot {
    slots.retain(|(_, s)| s.id() == only);
    if slots.is_empty() {
      return Err(KeyctlError::Other(anyhow::anyhow!("no key slot {only}")));
    }
  }
  let of_kind = |k: ProviderKind| -> Vec<(usize, &Slot)> {
    slots.iter().copied().filter(|(_, s)| s.kind() == k).collect()
  };

  for (_, s) in &slots {
    if !matches!(
      s.kind(),
      ProviderKind::SecretService | ProviderKind::Fido2Hmac | ProviderKind::Passphrase
    ) {
      ctx.ui.note(&format!("  slot {} ({}) cannot be used here; skipped", s.id(), s.kind()));
    }
  }

  // 1. Secret Service (may show the wallet's unlock dialog).
  for (f, s) in of_kind(ProviderKind::SecretService) {
    ctx.ui.note(&format!("Trying Secret Service slot {} ...", s.id()));
    let p = ctx.providers.secret_service();
    match files[f].unlock_slot(&s.id(), p.as_ref()).await {
      Ok(k) => {
        if let Verdict::Opened(u) = validate(ctx, k, f, s)? {
          return Ok(*u);
        }
      }
      Err(e) => ctx.ui.note(&format!("  {}", describe(&e))),
    }
  }

  // 2. FIDO2 security keys.
  let fido = of_kind(ProviderKind::Fido2Hmac);
  if !fido.is_empty() {
    match ctx.providers.fido2_devices().await {
      Ok(d) if d.is_empty() => ctx.ui.note("No security key connected; skipping FIDO2 slots."),
      Err(e) => {
        ctx.ui.note(&format!("Cannot list security keys ({}); skipping FIDO2 slots.", describe(&e)))
      }
      Ok(_) => {
        for (f, s) in fido {
          if let Some(u) = unlock_fido2(ctx, files[f], f, s).await? {
            return Ok(u);
          }
        }
      }
    }
  }

  // 3. Passphrases: one prompt is tried against every passphrase slot.
  let pass = of_kind(ProviderKind::Passphrase);
  if !pass.is_empty() {
    for _ in 0..SECRET_TRIES {
      let pw = ctx.ui.secret("Spool passphrase (empty to skip): ")?;
      if pw.is_empty() {
        break;
      }
      ctx.ui.note("Deriving the key (Argon2id) ...");
      let p = ctx.providers.passphrase(Some(pw));
      let mut any_stale = false;
      for &(f, s) in &pass {
        match files[f].unlock_slot(&s.id(), p.as_ref()).await {
          Ok(k) => match validate(ctx, k, f, s)? {
            Verdict::Opened(u) => return Ok(*u),
            Verdict::Stale => any_stale = true,
          },
          Err(KeysError::WrongKey) => {}
          Err(e) => ctx.ui.note(&format!("  slot {}: {}", s.id(), describe(&e))),
        }
      }
      if !any_stale {
        ctx.ui.note("Wrong passphrase.");
      }
    }
  }

  Err(KeyctlError::Unlock("no existing key slot could be unlocked".into()))
}

async fn unlock_fido2(
  ctx: &Ctx<'_>,
  ks: &KeySlots,
  f: usize,
  s: &Slot,
) -> Result<Option<Unlocked>, KeyctlError> {
  let mut pin = None;
  let mut tries = 0;
  loop {
    ctx.ui.note(&format!("Trying FIDO2 slot {}: touch your security key if it blinks ...", s.id()));
    let p = ctx.providers.fido2(Fido2Opts { pin: pin.take(), ..Default::default() });
    match ks.unlock_slot(&s.id(), p.as_ref()).await {
      Ok(k) => {
        return Ok(match validate(ctx, k, f, s)? {
          Verdict::Opened(u) => Some(*u),
          Verdict::Stale => None,
        });
      }
      Err(e @ (KeysError::NeedsSecret(_) | KeysError::PinInvalid(_))) if tries < SECRET_TRIES => {
        if matches!(e, KeysError::PinInvalid(_)) {
          ctx.ui.note("  Wrong PIN.");
        }
        tries += 1;
        let p = ctx.ui.secret("Security key PIN (empty to skip this slot): ")?;
        if p.is_empty() {
          return Ok(None);
        }
        pin = Some(p);
      }
      Err(e) => {
        ctx.ui.note(&format!("  {}", describe(&e)));
        return Ok(None);
      }
    }
  }
}
