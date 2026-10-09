//! `spool-keyctl`: offline, interactive management of Spool's key slots.
//! See the crate docs (`lib.rs`) and the README.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use spool_keyctl::ops::{self, AddKind};
use spool_keyctl::{Ctx, KeyctlError, SystemProviders, TtyUi, exit};
use spool_keys::SlotId;

/// Manage Spool's encryption key slots while spoold is stopped.
///
/// Exit codes: 0 ok, 1 error, 2 usage, 3 spoold is running, 4 no slot could
/// be unlocked, 5 done but some provider secrets could not be destroyed,
/// 6 an interrupted rotation needs `recover`, 7 aborted.
#[derive(Debug, Parser)]
#[command(name = "spool-keyctl", version)]
struct Cli {
  /// Spool state directory (default: $XDG_STATE_HOME/spool).
  #[arg(long, env = "SPOOL_STATE_DIR", global = true, value_name = "DIR")]
  state_dir: Option<PathBuf>,
  /// Unlock only with this existing slot.
  #[arg(long, global = true, value_name = "SLOT_ID")]
  unlock_slot: Option<SlotId>,
  #[command(subcommand)]
  command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
  /// Show key slots (no secrets), data key id and state files. Works while
  /// spoold runs.
  #[command(visible_alias = "list")]
  Status,
  /// Add a key slot (unlocks with an existing slot first).
  #[command(subcommand)]
  Add(AddCmd),
  /// Remove a key slot and destroy its secret.
  Remove {
    /// Slot id (see `status`).
    slot: SlotId,
    /// Allow removing the last slot (the history becomes undecryptable).
    #[arg(long)]
    force: bool,
  },
  /// New data key: re-enroll every slot, re-encrypt the history, destroy the
  /// old secrets.
  Rotate {
    /// Accept passphrases shorter than 12 characters.
    #[arg(long)]
    allow_short: bool,
  },
  /// Finish or undo an interrupted rotation.
  Recover,
  /// Destroy every key slot secret and delete the history.
  Wipe,
}

#[derive(Debug, Subcommand)]
enum AddCmd {
  /// A passphrase slot (Argon2id).
  Passphrase {
    /// Accept passphrases shorter than 12 characters (not recommended).
    #[arg(long)]
    allow_short: bool,
  },
  /// A FIDO2 security key slot (hmac-secret).
  Fido2 {
    /// Enroll on this device (e.g. /dev/hidraw3); asked if several keys are
    /// connected.
    #[arg(long, value_name = "PATH")]
    device: Option<String>,
    /// Require user verification (PIN) on every unlock.
    #[arg(long)]
    uv: bool,
  },
  /// A Secret Service (KWallet / gnome-keyring) slot.
  SecretService,
}

fn harden() {
  // Holds the data key: no core dumps, no ptrace by other same-uid processes.
  let _ = rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable);
  rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
}

fn main() -> ExitCode {
  let cli = match Cli::try_parse() {
    Ok(c) => c,
    Err(e) => {
      let _ = e.print();
      return ExitCode::from(if e.use_stderr() { exit::USAGE as u8 } else { exit::OK as u8 });
    }
  };
  harden();
  let code = match run(cli) {
    Ok(()) => exit::OK,
    Err(e) => {
      eprintln!("spool-keyctl: {e}");
      e.exit_code()
    }
  };
  ExitCode::from(code as u8)
}

fn run(cli: Cli) -> Result<(), KeyctlError> {
  let state_dir = spool_keyctl::paths::state_dir(cli.state_dir)?;
  let socket = spool_proto::paths::public_socket_path().map_err(anyhow::Error::from)?;
  let ui = TtyUi;
  let providers = SystemProviders;
  let mut ctx = Ctx::new(state_dir, socket, &ui, &providers);
  ctx.unlock_slot = cli.unlock_slot;
  let rt = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .map_err(anyhow::Error::from)?;
  rt.block_on(async {
    match cli.command {
      Command::Status => ops::status(&ctx).await,
      Command::Add(a) => {
        let kind = match a {
          AddCmd::Passphrase { allow_short } => AddKind::Passphrase { allow_short },
          AddCmd::Fido2 { device, uv } => AddKind::Fido2 { device, uv },
          AddCmd::SecretService => AddKind::SecretService,
        };
        ops::add(&ctx, kind).await
      }
      Command::Remove { slot, force } => ops::remove(&ctx, slot, force).await,
      Command::Rotate { allow_short } => ops::rotate(&ctx, allow_short).await,
      Command::Recover => ops::recover(&ctx).await,
      Command::Wipe => ops::wipe(&ctx).await,
    }
  })
}
