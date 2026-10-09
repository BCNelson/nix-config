//! The commands.

use spool_core::store::Store;
use spool_crypto::DataKey;
use spool_keys::{
  Error as KeysError, KeyProvider, KeySlots, ProviderKind, SessionProvider, Slot, SlotId,
  SlotParams,
};
use zeroize::Zeroizing;

use crate::lock::DaemonLock;
use crate::paths::{copy_durable, remove_file_durable, remove_tree, rename_durable};
use crate::providers::{Fido2Opts, refs};
use crate::ui::{confirm_word, confirm_yes};
use crate::unlock::unlock;
use crate::{CrashPoint, Ctx, KeyctlError, keys_err};

/// Minimum passphrase length (Unicode scalar values) without `--allow-short`.
pub const MIN_PASSPHRASE_CHARS: usize = 12;

/// What `add` enrolls.
#[derive(Debug, Clone)]
pub enum AddKind {
  /// A passphrase slot.
  Passphrase {
    /// Accept passphrases shorter than [`MIN_PASSPHRASE_CHARS`].
    allow_short: bool,
  },
  /// A FIDO2 security key slot.
  Fido2 {
    /// hidraw device to enroll on (`None`: ask if several are connected).
    device: Option<String>,
    /// Require user verification (PIN) on every unlock.
    uv: bool,
  },
  /// A Secret Service (KWallet / gnome-keyring) slot.
  SecretService,
}

/// State left behind by an interrupted rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
  /// Nothing.
  None,
  /// `keyslots.json.next` exists: rotation interrupted before the switch.
  Rotation,
  /// Only `keyslots.json.old` exists: switched, old secrets not yet destroyed.
  Cleanup,
}

/// Detect leftovers of an interrupted rotation.
pub fn pending(ctx: &Ctx<'_>) -> Pending {
  if ctx.paths.keyslots_next().symlink_metadata().is_ok() {
    Pending::Rotation
  } else if ctx.paths.keyslots_old().symlink_metadata().is_ok() {
    Pending::Cleanup
  } else {
    Pending::None
  }
}

fn slot_summary(s: &Slot) -> String {
  match s.params() {
    SlotParams::Passphrase(p) => {
      format!("argon2id m={} MiB t={} p={}", p.m_kib / 1024, p.t, p.p)
    }
    SlotParams::Fido2Hmac(p) => {
      format!("user verification: {}", if p.uv { "yes" } else { "no (touch only)" })
    }
    SlotParams::SecretService { .. } => "default collection (KWallet / gnome-keyring)".into(),
    SlotParams::Session => "memory only".into(),
    _ => String::new(),
  }
}

fn file_state(path: &std::path::Path) -> &'static str {
  if path.symlink_metadata().is_ok() { "present" } else { "absent" }
}

// ---- status ----------------------------------------------------------------

/// `status` / `list`: slots (no secrets), data key id, files. Read-only; does
/// not need spoold to be stopped.
pub async fn status(ctx: &Ctx<'_>) -> Result<(), KeyctlError> {
  let p = &ctx.paths;
  let ui = ctx.ui;
  ui.out(&format!("state dir:    {}", p.dir.display()));
  if p.dir.symlink_metadata().is_err() {
    ui.out("              (does not exist: spoold has not stored anything yet)");
  }
  match DaemonLock::probe(&ctx.socket, &p.dir) {
    Some(lock) => ui.out(&format!("spoold:       running (lock {} held)", lock.display())),
    None => ui.out("spoold:       not running"),
  }
  let blobs = std::fs::read_dir(p.blobs()).map(|d| d.count()).ok();
  ui.out(&format!(
    "history.db:   {}{}",
    file_state(&p.db()),
    blobs.map(|n| format!(" ({n} blob files)")).unwrap_or_default()
  ));
  ui.out(&format!("search index: {}", if p.has_index() { "present" } else { "absent" }));
  if p.plaintext_backups().iter().any(|b| b.symlink_metadata().is_ok()) {
    ui.out(&format!(
      "warning:      unencrypted M1 backup {} present; `spool-keyctl wipe` or delete it",
      p.plaintext_backups()[0].display()
    ));
  }
  match pending(ctx) {
    Pending::None => {}
    Pending::Rotation => ui.out(
      "pending:      interrupted key rotation (keyslots.json.next); run `spool-keyctl recover`",
    ),
    Pending::Cleanup => ui.out(
      "pending:      old key slots of a finished rotation (keyslots.json.old) still to destroy; run `spool-keyctl recover`",
    ),
  }
  ui.out(&format!("keyslots:     {}", p.keyslots().display()));
  match KeySlots::load(p.keyslots()) {
    Ok(ks) => {
      ui.out(&format!("data key id:  {}", ks.data_key_id()));
      ui.out(&format!("slots ({}):", ks.slots().len()));
      for s in ks.slots() {
        ui.out(&format!("  {}  {:<15} {}", s.id(), s.kind().as_str(), slot_summary(s)));
      }
    }
    Err(KeysError::NotFound(_)) => {
      ui.out("              (none: spoold creates it at first unlock)")
    }
    Err(e) => ui.out(&format!("              unreadable: {e}")),
  }
  Ok(())
}

// ---- shared helpers ----------------------------------------------------------

/// Refuse while a rotation is half done; finish a pending cleanup.
async fn settle_pending(ctx: &Ctx<'_>) -> Result<(), KeyctlError> {
  match pending(ctx) {
    Pending::None => Ok(()),
    Pending::Rotation => Err(KeyctlError::RecoveryNeeded(
      "an interrupted key rotation is pending (keyslots.json.next exists); run `spool-keyctl recover` first".into(),
    )),
    Pending::Cleanup => {
      ctx.ui.note("Finishing an earlier rotation: destroying the old slots' secrets ...");
      cleanup_old(ctx).await
    }
  }
}

fn load_main(ctx: &Ctx<'_>) -> Result<KeySlots, KeyctlError> {
  match KeySlots::load(ctx.paths.keyslots()) {
    Ok(ks) => Ok(ks),
    Err(KeysError::NotFound(p)) => Err(KeyctlError::Other(anyhow::anyhow!(
      "{} does not exist; spoold creates it when it first unlocks (nothing to manage yet)",
      p.display()
    ))),
    Err(e) => Err(KeyctlError::Other(e.into())),
  }
}

/// Prompt twice for a new passphrase.
fn new_passphrase(ctx: &Ctx<'_>, allow_short: bool) -> Result<Zeroizing<String>, KeyctlError> {
  for _ in 0..3 {
    let a = ctx.ui.secret("New passphrase: ")?;
    if a.is_empty() {
      ctx.ui.note("The passphrase must not be empty.");
      continue;
    }
    let n = a.chars().count();
    if n < MIN_PASSPHRASE_CHARS {
      if !allow_short {
        ctx.ui.note(&format!(
          "Too short: use at least {MIN_PASSPHRASE_CHARS} characters (or pass --allow-short)."
        ));
        continue;
      }
      ctx.ui.note(&format!(
        "Warning: passphrase shorter than {MIN_PASSPHRASE_CHARS} characters; an offline attacker with keyslots.json can guess it far more easily."
      ));
    }
    let b = ctx.ui.secret("Repeat the passphrase: ")?;
    if *a != *b {
      ctx.ui.note("The passphrases do not match.");
      continue;
    }
    return Ok(a);
  }
  Err(KeyctlError::Other(anyhow::anyhow!("no passphrase set")))
}

async fn pick_fido2_device(
  ctx: &Ctx<'_>,
  device: Option<String>,
) -> Result<Option<String>, KeyctlError> {
  if device.is_some() {
    return Ok(device);
  }
  let devs = ctx.providers.fido2_devices().await.map_err(|e| KeyctlError::Other(e.into()))?;
  match devs.len() {
    0 => Err(KeyctlError::Other(anyhow::anyhow!("no FIDO2 security key is connected"))),
    1 => Ok(Some(devs[0].path.clone())),
    n => {
      ctx.ui.note("Several security keys are connected:");
      for (i, d) in devs.iter().enumerate() {
        ctx.ui.note(&format!("  {}) {}  {} {}", i + 1, d.path, d.manufacturer, d.product));
      }
      let a = ctx.ui.line(&format!("Enroll which key? [1-{n}]: "))?;
      let i: usize = a.parse().ok().filter(|i| (1..=n).contains(i)).ok_or(KeyctlError::Aborted)?;
      Ok(Some(devs[i - 1].path.clone()))
    }
  }
}

/// Enroll one new slot of `kind` into `ks`, wrapping `key`.
async fn enroll_into(
  ctx: &Ctx<'_>,
  ks: &mut KeySlots,
  key: &[u8; 32],
  kind: &AddKind,
) -> Result<SlotId, KeyctlError> {
  match kind {
    AddKind::SecretService => {
      ctx
        .ui
        .note("Storing a new key in the Secret Service (the wallet may ask to be unlocked) ...");
      let p = ctx.providers.secret_service();
      ks.add_slot(p.as_ref(), key).await.map_err(keys_err)
    }
    AddKind::Passphrase { allow_short } => {
      let pw = new_passphrase(ctx, *allow_short)?;
      ctx.ui.note("Deriving the key (Argon2id, 256 MiB) ...");
      let p = ctx.providers.passphrase(Some(pw));
      ks.add_slot(p.as_ref(), key).await.map_err(keys_err)
    }
    AddKind::Fido2 { device, uv } => {
      let device = pick_fido2_device(ctx, device.clone()).await?;
      let mut pin = None;
      let mut tries = 0;
      loop {
        ctx
          .ui
          .note("Touch your security key (twice: create the credential, then derive the key) ...");
        let p = ctx.providers.fido2(Fido2Opts { pin: pin.take(), uv: *uv, device: device.clone() });
        match ks.add_slot(p.as_ref(), key).await {
          Ok(id) => return Ok(id),
          Err(e @ (KeysError::NeedsSecret(_) | KeysError::PinInvalid(_))) if tries < 3 => {
            if matches!(e, KeysError::PinInvalid(_)) {
              ctx.ui.note("Wrong PIN.");
            }
            tries += 1;
            let p = ctx.ui.secret("Security key PIN: ")?;
            if p.is_empty() {
              return Err(KeyctlError::Aborted);
            }
            pin = Some(p);
          }
          Err(e) => return Err(keys_err(e)),
        }
      }
    }
  }
}

// ---- add / remove ------------------------------------------------------------

/// `add`: unlock with an existing slot, then enroll a new one.
pub async fn add(ctx: &Ctx<'_>, kind: AddKind) -> Result<(), KeyctlError> {
  let _lock = ctx.lock()?;
  settle_pending(ctx).await?;
  let mut ks = load_main(ctx)?;
  let u = unlock(ctx, &[&ks]).await?;
  ctx.ui.note(&format!("Unlocked with slot {}.", u.slot));
  drop(u.store);
  let id = enroll_into(ctx, &mut ks, u.key.expose(), &kind).await?;
  let slot = ks.slot(&id).expect("just added");
  ctx.ui.out(&format!("added slot {id} ({})", slot.kind()));
  Ok(())
}

/// `remove`: unlock with an existing slot, then delete slot `id` and destroy
/// its provider secret. The last slot needs `force` and a typed confirmation.
pub async fn remove(ctx: &Ctx<'_>, id: SlotId, force: bool) -> Result<(), KeyctlError> {
  let _lock = ctx.lock()?;
  settle_pending(ctx).await?;
  let mut ks = load_main(ctx)?;
  if ks.slot(&id).is_none() {
    return Err(KeyctlError::Other(anyhow::anyhow!("no key slot {id}")));
  }
  let last = ks.slots().len() == 1;
  if last && !force {
    return Err(KeyctlError::Other(anyhow::anyhow!(
      "slot {id} is the only key slot; removing it makes the history undecryptable (use --force, or `spool-keyctl wipe`)"
    )));
  }
  let u = unlock(ctx, &[&ks]).await?;
  ctx.ui.note(&format!("Unlocked with slot {}.", u.slot));
  drop(u.store);
  if last {
    ctx.ui.note("This is the LAST key slot: without it the history can never be decrypted again.");
    if !confirm_word(ctx.ui, "remove")? {
      return Err(KeyctlError::Aborted);
    }
  }
  let destroyers = ctx.providers.destroyers();
  let res = ks.remove_slot(&id, &refs(&destroyers), force).await;
  match res {
    Ok(()) => {}
    Err(e @ KeysError::DestroyIncomplete { .. }) => {
      ctx.ui.out(&format!("removed slot {id} from keyslots.json"));
      return Err(keys_err(e));
    }
    Err(e) => return Err(keys_err(e)),
  }
  ctx.ui.out(&format!("removed slot {id}"));
  if last {
    ctx.ui.out(
      "keyslots.json is gone; spoold will refuse to start until the old history is deleted: run `spool-keyctl wipe`",
    );
  }
  Ok(())
}

// ---- rotate / recover --------------------------------------------------------

/// Destroy the secrets of `keyslots.json.old` and delete it.
async fn cleanup_old(ctx: &Ctx<'_>) -> Result<(), KeyctlError> {
  let path = ctx.paths.keyslots_old();
  let destroyers = ctx.providers.destroyers();
  let res = match KeySlots::load(&path) {
    Ok(old) => old.wipe(&refs(&destroyers)).await.map_err(keys_err),
    Err(KeysError::NotFound(_)) => Ok(()),
    Err(e) => {
      ctx.ui.note(&format!("{} is unreadable ({e}); deleting it", path.display()));
      remove_file_durable(&path).map(|_| ()).map_err(KeyctlError::Other)
    }
  };
  // The index was encrypted with the old key: drop it (spoold rebuilds).
  delete_index(ctx)?;
  res
}

fn delete_index(ctx: &Ctx<'_>) -> Result<(), KeyctlError> {
  let a = remove_tree(&ctx.paths.index())?;
  let b = remove_tree(&ctx.paths.index_rebuild())?;
  if a || b {
    ctx.ui.note("Deleted the search index; spoold rebuilds it at the next unlock.");
  }
  Ok(())
}

/// Give up on `.next`: destroy its new secrets and delete it.
async fn roll_back(ctx: &Ctx<'_>, next: KeySlots) -> Result<(), KeyctlError> {
  let destroyers = ctx.providers.destroyers();
  next.wipe(&refs(&destroyers)).await.map_err(keys_err)
}

/// Switch `.next` in: `keyslots.json` -> `.old` (copy), `.next` ->
/// `keyslots.json` (atomic rename), destroy `.old`'s secrets, drop the index.
async fn switch_in(ctx: &Ctx<'_>) -> Result<(), KeyctlError> {
  let p = &ctx.paths;
  copy_durable(&p.keyslots(), &p.keyslots_old())?;
  ctx.crash_point(CrashPoint::AfterOldCopied)?;
  rename_durable(&p.keyslots_next(), &p.keyslots())?;
  ctx.crash_point(CrashPoint::AfterSwitch)?;
  cleanup_old(ctx).await
}

enum Plan {
  Reenroll(AddKind),
  Drop(&'static str),
}

fn plan_for(s: &Slot, allow_short: bool) -> Plan {
  match (s.kind(), s.params()) {
    (ProviderKind::SecretService, _) => Plan::Reenroll(AddKind::SecretService),
    (ProviderKind::Passphrase, _) => Plan::Reenroll(AddKind::Passphrase { allow_short }),
    (ProviderKind::Fido2Hmac, SlotParams::Fido2Hmac(p)) => {
      Plan::Reenroll(AddKind::Fido2 { device: None, uv: p.uv })
    }
    (ProviderKind::Session, _) => Plan::Drop("session slots only live in memory"),
    _ => Plan::Drop("this provider kind cannot be enrolled by this build"),
  }
}

/// `rotate`: new data key, fresh secrets for every slot, re-encrypted
/// history, old secrets destroyed.
///
/// # Crash safety
///
/// spool-keys' `KeySlots::rotate` replaces `keyslots.json` *before* the
/// database is re-keyed, so a crash in between leaves a database that only
/// the old (already destroyed) key opens. This command instead stages the
/// new slots in a second file and keeps the old slots (and their provider
/// secrets) until the database is re-keyed:
///
/// 1. Unlock the old key `K0` with an existing slot (validated on the DB).
/// 2. `keyslots.json.next` = `KeySlots::create_new` (a fresh random `K1`,
///    new data key id) with a throw-away in-memory placeholder slot; then
///    one freshly enrolled slot per old slot (new Secret Service item, new
///    passphrase, new FIDO2 credential); then the placeholder is removed.
/// 3. `Store::rekey(K1)`: itself crash-safe, after any crash exactly one of
///    `K0` / `K1` opens the DB and `open_encrypted` finishes or undoes it.
/// 4. `keyslots.json` is copied to `keyslots.json.old`, then `.next` is
///    renamed over `keyslots.json` (atomic).
/// 5. The secrets of the `.old` slots are destroyed, `.old` is deleted, and
///    the search index (encrypted with `K0`) is deleted for spoold to
///    rebuild.
///
/// After a crash, `.next` present = before step 4 finished: `recover`
/// unlocks a slot from either file, sees which key opens the DB, and rolls
/// back (`K0`: destroy the `.next` secrets) or forward (`K1`: steps 4-5).
/// Only `.old` present = after step 4: `recover` (or any later mutating
/// command) runs step 5. At no point is there no slot for the key that
/// opens the database. spoold only reads `keyslots.json`: while `.next`
/// exists and the DB is already under `K1` it reports a key error (and wipes
/// nothing) until `recover` runs.
pub async fn rotate(ctx: &Ctx<'_>, allow_short: bool) -> Result<(), KeyctlError> {
  let _lock = ctx.lock()?;
  settle_pending(ctx).await?;
  let ks = load_main(ctx)?;

  let plan: Vec<(&Slot, Plan)> = ks.slots().iter().map(|s| (s, plan_for(s, allow_short))).collect();
  ctx.ui.note("Key rotation: a new data key, a fresh secret for every slot, history re-encrypted.");
  for (s, p) in &plan {
    match p {
      Plan::Reenroll(_) => {
        ctx.ui.note(&format!("  {} {:<15} re-enroll", s.id(), s.kind().as_str()))
      }
      Plan::Drop(why) => {
        ctx.ui.note(&format!("  {} {:<15} DROPPED ({why})", s.id(), s.kind().as_str()))
      }
    }
  }
  if !plan.iter().any(|(_, p)| matches!(p, Plan::Reenroll(_))) {
    return Err(KeyctlError::Other(anyhow::anyhow!("no slot can be re-enrolled")));
  }
  if !confirm_yes(ctx.ui, "Proceed?")? {
    return Err(KeyctlError::Aborted);
  }

  // 1.
  let u = unlock(ctx, &[&ks]).await?;
  ctx.ui.note(&format!("Unlocked with slot {}.", u.slot));
  let mut store = u.store;

  // 2.
  let placeholder = SessionProvider::new();
  let (mut next, k1) = KeySlots::create_new(ctx.paths.keyslots_next(), &placeholder)
    .await
    .map_err(|e| KeyctlError::Other(e.into()))?;
  let placeholder_id = next.slots()[0].id();
  let mut mapping: Vec<(SlotId, SlotId)> = Vec::new();
  for (s, p) in &plan {
    let Plan::Reenroll(kind) = p else { continue };
    match kind {
      AddKind::Passphrase { .. } => ctx.ui.note(&format!(
        "Passphrase for the slot replacing {} (may be the same as before):",
        s.id()
      )),
      AddKind::Fido2 { .. } => ctx
        .ui
        .note(&format!("Re-enrolling FIDO2 slot {}: use the same security key as before.", s.id())),
      AddKind::SecretService => {}
    }
    loop {
      match enroll_into(ctx, &mut next, &k1, kind).await {
        Ok(id) => {
          mapping.push((s.id(), id));
          break;
        }
        Err(KeyctlError::Aborted) => {
          roll_back(ctx, next).await?;
          return Err(KeyctlError::Aborted);
        }
        Err(e) => {
          ctx.ui.note(&format!("Could not enroll the replacement for {}: {e}", s.id()));
          let a = ctx.ui.line("[r]etry, [s]kip (drop this slot), [a]bort: ")?;
          match a.to_ascii_lowercase().as_str() {
            "r" | "retry" => continue,
            "s" | "skip" => break,
            _ => {
              roll_back(ctx, next).await?;
              return Err(KeyctlError::Aborted);
            }
          }
        }
      }
    }
    if mapping.len() == 1 {
      ctx.crash_point(CrashPoint::AfterFirstEnroll)?;
    }
  }
  if mapping.is_empty() {
    roll_back(ctx, next).await?;
    return Err(KeyctlError::Other(anyhow::anyhow!(
      "no replacement slot was enrolled; nothing changed"
    )));
  }
  next
    .remove_slot(&placeholder_id, &[&placeholder as &dyn KeyProvider], false)
    .await
    .map_err(keys_err)?;
  ctx.crash_point(CrashPoint::AfterNextWritten)?;

  // 3.
  if let Some(st) = store.as_mut() {
    ctx.ui.note("Re-encrypting the history (this can take a while) ...");
    let k1d = DataKey::from_bytes(Zeroizing::new(*k1));
    if let Err(e) = st.rekey(&k1d) {
      drop(store.take());
      ctx.ui.note(&format!("Re-encryption failed: {e}"));
      return match Store::open_encrypted(&ctx.paths.dir, &u.key) {
        Ok(s) => {
          drop(s);
          roll_back(ctx, next).await?;
          Err(KeyctlError::Other(anyhow::anyhow!(
            "re-encryption failed ({e}); rotation rolled back, nothing changed"
          )))
        }
        Err(_) => Err(KeyctlError::RecoveryNeeded(format!(
          "re-encryption failed ({e}) and the old key no longer opens the history; run `spool-keyctl recover`"
        ))),
      };
    }
  }
  drop(store);
  ctx.crash_point(CrashPoint::AfterRekey)?;

  // 4. + 5.
  let res = switch_in(ctx).await;
  for (old, new) in &mapping {
    ctx.ui.out(&format!("slot {old} -> {new}"));
  }
  ctx.ui.out(&format!("rotated: new data key id {}", next.data_key_id()));
  res
}

/// `recover`: finish or undo an interrupted rotation.
pub async fn recover(ctx: &Ctx<'_>) -> Result<(), KeyctlError> {
  let _lock = ctx.lock()?;
  match pending(ctx) {
    Pending::None => {
      ctx.ui.out("nothing to recover");
      Ok(())
    }
    Pending::Cleanup => {
      cleanup_old(ctx).await?;
      ctx.ui.out("finished the earlier rotation (old slot secrets destroyed)");
      Ok(())
    }
    Pending::Rotation => {
      // A `.old` next to `.next` is only ever a copy of `keyslots.json`
      // (crash inside step 4): delete it WITHOUT destroying its secrets.
      remove_file_durable(&ctx.paths.keyslots_old())?;
      let main = load_main(ctx)?;
      let next =
        KeySlots::load(ctx.paths.keyslots_next()).map_err(|e| KeyctlError::Other(e.into()))?;
      if !ctx.paths.has_db() {
        roll_back(ctx, next).await?;
        ctx.ui.out("no history database: rotation rolled back (run `spool-keyctl rotate` again)");
        return Ok(());
      }
      ctx.ui.note("Unlock with any old or new slot to find out which key the history uses.");
      let u = unlock(ctx, &[&main, &next]).await?;
      ctx.ui.note(&format!("Unlocked with slot {}.", u.slot));
      drop(u.store);
      if u.file == 0 {
        roll_back(ctx, next).await?;
        ctx.ui.out("the history is still under the old key: rotation rolled back (run `spool-keyctl rotate` again)");
      } else {
        let id = next.data_key_id();
        drop(next);
        switch_in(ctx).await?;
        ctx
          .ui
          .out(&format!("the history is under the new key: rotation completed (data key id {id})"));
      }
      Ok(())
    }
  }
}

// ---- wipe ----------------------------------------------------------------------

/// `wipe`: destroy every provider secret and delete the history, index and
/// slot files. Needs no unlock (it is also the way out after losing every
/// unlock method), but a typed confirmation.
pub async fn wipe(ctx: &Ctx<'_>) -> Result<(), KeyctlError> {
  let p = &ctx.paths;
  if p.dir.symlink_metadata().is_err() {
    ctx.ui.out(&format!("{} does not exist: nothing to wipe", p.dir.display()));
    return Ok(());
  }
  let _lock = ctx.lock()?;
  ctx.ui.note(&format!(
    "This permanently deletes the Spool clipboard history in {}:\n  every key slot and its secret (wallet items; FIDO2 credentials and passphrases simply become useless),\n  history.db, blobs/, the search index, and any unencrypted M1 backup.",
    p.dir.display()
  ));
  if !confirm_word(ctx.ui, "wipe")? {
    return Err(KeyctlError::Aborted);
  }
  let destroyers = ctx.providers.destroyers();
  let mut problems: Vec<String> = Vec::new();
  let mut had_secret_service = false;
  // Key material first: once it is gone the data is unreadable even if a
  // file deletion below fails.
  for path in [p.keyslots(), p.keyslots_next(), p.keyslots_old()] {
    match KeySlots::load(&path) {
      Ok(ks) => {
        had_secret_service |= ks.slots().iter().any(|s| s.kind() == ProviderKind::SecretService);
        if let Err(e) = ks.wipe(&refs(&destroyers)).await {
          problems.push(e.to_string());
          // `wipe` deletes the file even when a destroy fails; make sure.
          remove_file_durable(&path)?;
        }
      }
      Err(KeysError::NotFound(_)) => {}
      Err(e) => {
        ctx.ui.note(&format!("{} is unreadable ({e}); deleting it", path.display()));
        remove_file_durable(&path)?;
      }
    }
  }
  match ctx.providers.secret_service_destroy_all().await {
    Ok(0) => {}
    Ok(n) => ctx.ui.note(&format!("Removed {n} leftover Spool item(s) from the Secret Service.")),
    Err(e) if had_secret_service => problems.push(format!("Secret Service cleanup: {e}")),
    Err(e) => {
      ctx.ui.note(&format!("Secret Service not reachable ({e}); no Spool items checked there."))
    }
  }
  Store::wipe(&p.dir)?;
  remove_tree(&p.index())?;
  remove_tree(&p.index_rebuild())?;
  for b in p.plaintext_backups() {
    remove_file_durable(&b)?;
  }
  ctx.ui.out("wiped: key slots, history.db, blobs and search index deleted");
  ctx.ui.out(
    "note: on copy-on-write filesystems (btrfs, ZFS), in snapshots and in backups the old files may survive; they stay encrypted, and the destroyed slot secrets are what keeps them unreadable. Delete affected snapshots for a complete wipe.",
  );
  if problems.is_empty() { Ok(()) } else { Err(KeyctlError::Incomplete(problems.join("; "))) }
}
