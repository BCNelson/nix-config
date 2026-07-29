//! Thin-client install flow.
//!
//! A thin client (Dell box, ~2 GB RAM) cannot evaluate its own configuration,
//! let alone build it. So the installer never runs `nixos-install --flake`.
//! Instead it prepares the host in the repo, pushes a branch, and then waits
//! for CI to check it and for the builder to publish a signed closure. The
//! closure is fetched straight into `/mnt` -- never into the live ISO's
//! RAM-backed store, which a full system closure would not fit in -- and
//! `nixos-install --system` installs it without evaluating anything.
//!
//! Because the wait spans a human merging a PR and a builder chewing through a
//! full closure, every long step is checkpointed so an interrupted run can be
//! resumed rather than restarted.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub const CACHE_URL: &str = "https://nixcache.nel.family";
pub const CACHE_PUBKEY_FILE: &str = "secrets/store/romeo/nix_cache_key.pub";
pub const THIN_CLIENTS_FILE: &str = "hosts/thin-clients.nix";

/// Where the installer's own progress is recorded on the target disk. Kept
/// under /mnt so it survives the live ISO being rebooted, which is the whole
/// point of the resume path.
const MNT_STATE_DIR: &str = "/mnt/var/lib/install-system";

/// How far the install got. Each variant is only written after the step it
/// names has fully succeeded, so resuming always redoes at most one step.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Disk partitioned and mounted, host written into the repo, nothing pushed.
    Prepared,
    /// Branch pushed. From here on the work is waiting on CI and the builder.
    Pushed,
    /// Closure fetched into /mnt.
    Copied,
    /// nixos-install finished; only post-install fixups remain.
    Installed,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct State {
    pub hostname: String,
    pub stage: Stage,
    pub disk: String,
    pub disk_nix: String,
    pub swap_size: u64,
    pub users: Vec<String>,
    pub branch: String,
    pub commit: String,
    /// Commit the manifest already advertised before we pushed, if any. A
    /// reinstall of an existing thin client finds a manifest already sitting
    /// there; without this the installer would accept that stale closure
    /// immediately instead of waiting for one built from the new config.
    #[serde(default)]
    pub baseline_commit: Option<String>,
    #[serde(default)]
    pub store_path: Option<String>,
}

/// The builder's per-host manifest, served from the binary cache vhost.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub hostname: String,
    pub commit: String,
    #[serde(default)]
    pub store_path: String,
    pub status: String,
    #[serde(default)]
    pub built_at: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

pub fn manifest_url(hostname: &str) -> String {
    format!("{}/thin-clients/{}.json", CACHE_URL, hostname)
}

// ---------------------------------------------------------------------------
// State persistence
// ---------------------------------------------------------------------------

fn local_state_path(home: &str, hostname: &str) -> PathBuf {
    PathBuf::from(home)
        .join(".local/state/install-system")
        .join(format!("{}.json", hostname))
}

fn mnt_state_path(hostname: &str) -> PathBuf {
    PathBuf::from(MNT_STATE_DIR).join(format!("{}.json", hostname))
}

/// Write the checkpoint to the live session *and* to the target disk. The
/// on-disk copy is the one that matters after a reboot; the session copy is
/// what lets a plain re-run resume without re-mounting anything.
pub fn save_state(home: &str, state: &State) -> Result<()> {
    let json = serde_json::to_string_pretty(state)?;

    let local = local_state_path(home, &state.hostname);
    if let Some(parent) = local.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&local, &json)
        .with_context(|| format!("writing {}", local.display()))?;

    if Path::new("/mnt/nix").exists() {
        // /mnt is owned by root, so these two go through sudo.
        let status = Command::new("sudo")
            .args(["install", "-d", "-m", "0755", MNT_STATE_DIR])
            .status()?;
        if status.success() {
            let target = mnt_state_path(&state.hostname);
            let mut child = Command::new("sudo")
                .args(["tee", target.to_string_lossy().as_ref()])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .spawn()?;
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .context("failed to open stdin for tee")?
                .write_all(json.as_bytes())?;
            child.wait()?;
        }
    }

    Ok(())
}

pub fn load_state(home: &str, hostname: &str) -> Option<State> {
    for path in [local_state_path(home, hostname), mnt_state_path(hostname)] {
        if let Ok(contents) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<State>(&contents) {
                Ok(state) => return Some(state),
                Err(e) => println!("Ignoring unreadable state at {}: {}", path.display(), e),
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Repo edits
// ---------------------------------------------------------------------------

/// Add the host to the registry the builder reads and the thin-client role
/// asserts against. Idempotent, so a resumed run does not duplicate the entry.
pub fn register_thin_client(hostname: &str) -> Result<()> {
    let contents = std::fs::read_to_string(THIN_CLIENTS_FILE)
        .with_context(|| format!("reading {}", THIN_CLIENTS_FILE))?;

    if contents
        .lines()
        .any(|line| line.trim() == format!("\"{}\"", hostname))
    {
        println!("{} is already listed in {}", hostname, THIN_CLIENTS_FILE);
        return Ok(());
    }

    let close = contents.rfind(']').with_context(|| {
        format!(
            "{} does not contain a closing ']' -- is it still a nix list?",
            THIN_CLIENTS_FILE
        )
    })?;
    let (head, tail) = contents.split_at(close);
    let updated = format!("{}  \"{}\"\n{}", head, hostname, tail);
    std::fs::write(THIN_CLIENTS_FILE, updated)?;
    println!("Registered {} in {}", hostname, THIN_CLIENTS_FILE);
    Ok(())
}

/// The builder signs every closure it publishes, and the client verifies that
/// signature at install time and on every later update. Without the public half
/// committed, the host we are about to add would fail evaluation in CI and the
/// installer could not verify what it fetched -- so refuse early and loudly
/// rather than halfway through a destructive install.
pub fn read_cache_public_key() -> Result<String> {
    let key = std::fs::read_to_string(CACHE_PUBKEY_FILE).map_err(|_| {
        anyhow::anyhow!(
            "{} is missing. The binary cache signing key has not been generated yet.\n\
             Run `just generate-secrets`, then `just rekey`, commit both halves, and\n\
             deploy romeo before installing a thin client. See docs/thin-clients.md.",
            CACHE_PUBKEY_FILE
        )
    })?;
    let key = key.trim().to_string();
    if key.is_empty() {
        bail!("{} is empty", CACHE_PUBKEY_FILE);
    }
    Ok(key)
}

// ---------------------------------------------------------------------------
// Manifest polling
// ---------------------------------------------------------------------------

/// Fetch the manifest once. `Ok(None)` means "not published yet", which during
/// a fresh install is the normal state for the first hour or so.
pub fn fetch_manifest(hostname: &str) -> Result<Option<Manifest>> {
    let url = manifest_url(hostname);
    let output = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--max-time",
            "30",
            &url,
        ])
        .output()
        .context("failed to run curl")?;

    if !output.status.success() {
        // 404 until the builder has ever built this host; anything else is
        // also just "try again later" from the installer's point of view.
        return Ok(None);
    }

    let manifest: Manifest = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("parsing manifest from {}", url))?;

    if manifest.hostname != hostname {
        bail!(
            "manifest at {} is for host '{}', not '{}'",
            url,
            manifest.hostname,
            hostname
        );
    }

    Ok(Some(manifest))
}

/// Block until the builder publishes a closure built from a commit that
/// contains our change.
///
/// The signal is deliberately not "the manifest names our commit": the branch
/// lands on main through a merge or a squash, so our SHA generally never
/// appears anywhere. What does hold is that the manifest's commit must have
/// *moved* from whatever it advertised before we pushed -- and for a brand new
/// host, that no manifest existed at all.
pub fn wait_for_closure(hostname: &str, baseline_commit: Option<&str>) -> Result<Manifest> {
    let poll_interval = Duration::from_secs(60);
    let started = Instant::now();
    let mut last_reported: Option<String> = None;

    println!();
    println!("Waiting for the builder to publish a closure for {}.", hostname);
    println!("  manifest: {}", manifest_url(hostname));
    match baseline_commit {
        Some(c) => println!("  waiting for a commit other than {}", c),
        None => println!("  waiting for the first manifest to appear"),
    }
    println!("This is safe to interrupt -- re-run with --thin --resume to pick up here.");
    println!();

    loop {
        match fetch_manifest(hostname)? {
            Some(manifest) => {
                let is_new = match baseline_commit {
                    Some(base) => manifest.commit != base,
                    None => true,
                };

                if is_new && manifest.status == "ready" && !manifest.store_path.is_empty() {
                    println!(
                        "Closure ready: {} (commit {}, built {})",
                        manifest.store_path,
                        manifest.commit,
                        manifest.built_at.as_deref().unwrap_or("unknown")
                    );
                    return Ok(manifest);
                }

                if is_new && manifest.status == "failed" {
                    println!(
                        "!! Builder reported a FAILED build for {} at commit {}",
                        hostname, manifest.commit
                    );
                    if let Some(error) = &manifest.error {
                        println!("---");
                        println!("{}", error);
                        println!("---");
                    }
                    println!("Fix the configuration, push, and this will pick up the next build.");
                }

                let key = format!("{}:{}", manifest.commit, manifest.status);
                if last_reported.as_deref() != Some(key.as_str()) {
                    println!(
                        "[{}] manifest at commit {} status {} -- not ours yet",
                        elapsed(started),
                        manifest.commit,
                        manifest.status
                    );
                    last_reported = Some(key);
                }
            }
            None => {
                if last_reported.as_deref() != Some("absent") {
                    println!("[{}] no manifest published yet", elapsed(started));
                    last_reported = Some("absent".to_string());
                }
            }
        }

        std::thread::sleep(poll_interval);
    }
}

fn elapsed(started: Instant) -> String {
    let secs = started.elapsed().as_secs();
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

// ---------------------------------------------------------------------------
// Fetch and install
// ---------------------------------------------------------------------------

/// Copy the closure from the cache directly into the target filesystem.
///
/// `--to local?root=/mnt` is the important part: the live ISO's store is a
/// tmpfs sized to RAM, and a full system closure does not fit in 2 GB. Writing
/// straight to /mnt also means `nixos-install --system` afterwards finds
/// everything already present and does almost no work.
pub fn copy_closure_to_target(store_path: &str, cache_public_key: &str) -> Result<()> {
    println!("Fetching {} into /mnt", store_path);
    let status = Command::new("sudo")
        .args([
            "nix",
            "copy",
            "--extra-experimental-features",
            "nix-command flakes",
            "--from",
            CACHE_URL,
            "--to",
            "local?root=/mnt",
            // The live ISO only trusts cache.nixos.org out of the box; the
            // closure we are fetching is signed by the builder's key.
            "--extra-trusted-public-keys",
            cache_public_key,
            store_path,
        ])
        .status()
        .context("failed to run nix copy")?;

    if !status.success() {
        bail!("nix copy of {} into /mnt failed", store_path);
    }
    Ok(())
}

/// Install without evaluating: `--system` takes a prebuilt closure, so nix
/// never loads the flake, never instantiates nixpkgs, and never needs the
/// memory this machine does not have.
pub fn install_closure(store_path: &str) -> Result<()> {
    println!("Installing {}", store_path);
    let status = Command::new("sudo")
        .args([
            "nixos-install",
            "--root",
            "/mnt",
            "--system",
            store_path,
            "--no-root-password",
            "--no-channel-copy",
        ])
        .status()
        .context("failed to run nixos-install")?;

    if !status.success() {
        bail!("nixos-install of {} failed", store_path);
    }
    Ok(())
}

/// Re-mount an already-partitioned disk so an interrupted install can be
/// resumed after the live ISO has been rebooted. `--mode mount` touches no
/// partition tables, unlike the `zap_create_mount` used on a fresh install.
pub fn remount_target(disk_nix: &str, disk: &str, swap_size: u64) -> Result<()> {
    println!("Re-mounting {} (no partitioning) so the install can resume", disk);
    let status = Command::new("sudo")
        .args([
            "nix",
            "run",
            "github:nix-community/disko",
            "--extra-experimental-features",
            "nix-command flakes",
            "--no-write-lock-file",
            "--",
            "--mode",
            "mount",
            disk_nix,
            "--arg",
            "disk",
            &format!("\"{}\"", disk),
            "--arg",
            "swapSize",
            &format!("\"{}G\"", swap_size),
        ])
        .status()
        .context("failed to run disko")?;

    if !status.success() {
        bail!("disko --mode mount failed for {}", disk);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_thin_client_inserts_before_the_closing_bracket() {
        let original = "# comment\n[\n]\n";
        let close = original.rfind(']').unwrap();
        let (head, tail) = original.split_at(close);
        let updated = format!("{}  \"{}\"\n{}", head, "delta-1", tail);
        assert_eq!(updated, "# comment\n[\n  \"delta-1\"\n]\n");
    }

    #[test]
    fn register_thin_client_appends_to_a_populated_list() {
        let original = "[\n  \"delta-1\"\n]\n";
        let close = original.rfind(']').unwrap();
        let (head, tail) = original.split_at(close);
        let updated = format!("{}  \"{}\"\n{}", head, "delta-2", tail);
        assert_eq!(updated, "[\n  \"delta-1\"\n  \"delta-2\"\n]\n");
    }

    #[test]
    fn manifest_parses_the_builder_output() {
        let json = r#"{
          "hostname": "delta-1",
          "commit": "5053fef",
          "storePath": "/nix/store/xxx-nixos-system-delta-1-25.11",
          "status": "ready",
          "builtAt": "2026-07-28T18:04:11Z"
        }"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.hostname, "delta-1");
        assert_eq!(manifest.status, "ready");
        assert_eq!(
            manifest.store_path,
            "/nix/store/xxx-nixos-system-delta-1-25.11"
        );
        assert!(manifest.error.is_none());
    }

    #[test]
    fn manifest_parses_a_failed_build() {
        let json = r#"{
          "hostname": "delta-1",
          "commit": "5053fef",
          "storePath": "",
          "status": "failed",
          "builtAt": "2026-07-28T18:04:11Z",
          "error": "error: attribute 'foo' missing"
        }"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.status, "failed");
        assert_eq!(manifest.error.unwrap(), "error: attribute 'foo' missing");
    }

    #[test]
    fn stages_order_so_resume_can_compare_them() {
        assert!(Stage::Prepared < Stage::Pushed);
        assert!(Stage::Pushed < Stage::Copied);
        assert!(Stage::Copied < Stage::Installed);
    }

    #[test]
    fn state_round_trips_through_json() {
        let state = State {
            hostname: "delta-1".to_string(),
            stage: Stage::Pushed,
            disk: "/dev/sda".to_string(),
            disk_nix: "disko/default.nix".to_string(),
            swap_size: 8,
            users: vec!["bcnelson".to_string()],
            branch: "install-delta-1".to_string(),
            commit: "abc123".to_string(),
            baseline_commit: None,
            store_path: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: State = serde_json::from_str(&json).unwrap();
        assert_eq!(back.hostname, "delta-1");
        assert_eq!(back.stage, Stage::Pushed);
        assert_eq!(back.swap_size, 8);
    }
}
