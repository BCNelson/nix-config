use std::path::PathBuf;
use clap::Parser;
use anyhow::{Context, Result, bail};
use cmd_lib::run_cmd;
use regex::Regex;
use std::process::Command;
use std::collections::HashMap;
use std::fs;
use serde::Deserialize;

/// Configuration for repository unlock methods
#[derive(Deserialize)]
struct UnlockConfig {
    methods: UnlockMethods,
}

#[derive(Deserialize)]
struct UnlockMethods {
    fido2: Option<Fido2Config>,
    gpg: Option<GpgConfig>,
}

#[derive(Deserialize)]
struct Fido2Config {
    enabled: bool,
    #[serde(rename = "keyFile")]
    key_file: String,
    identities: Vec<Fido2Identity>,
}

#[derive(Deserialize, Clone)]
struct Fido2Identity {
    name: String,
    path: String,
    default: bool,
}

#[derive(Deserialize)]
struct GpgConfig {
    enabled: bool,
    #[serde(rename = "keyFile")]
    key_file: String,
}

mod wifi;
mod swap;
mod thin;

use thin::{Stage, State};

/// Lists available disks in the system
fn list_available_disks() -> Result<Vec<String>> {
    let mut disks = Vec::new();
    
    // Read from /sys/block to get all block devices
    for entry in fs::read_dir("/sys/block")? {
        let entry = entry?;
        let path = entry.path();
        
        // Get the disk name
        if let Some(disk_name) = path.file_name().and_then(|n| n.to_str()) {
            // Filter out devices that are never a valid install target.
            // zram and fd are here because a thin client install offered both:
            // zram0 is the compressed swap this very installer depends on, and
            // fd0 is a floppy QEMU advertises. Note "ram" does not cover
            // "zram" -- the prefix check is on the whole name.
            if !disk_name.starts_with("loop") &&
               !disk_name.starts_with("ram") &&
               !disk_name.starts_with("zram") &&
               !disk_name.starts_with("fd") &&
               !disk_name.starts_with("sr") &&
               !disk_name.starts_with("dm-") {
                
                // Read size to make sure it's a real disk
                if let Ok(size_str) = fs::read_to_string(path.join("size")) {
                    if let Ok(size) = size_str.trim().parse::<u64>() {
                        // Only include disks with non-zero size
                        if size > 0 {
                            let dev_path = format!("/dev/{}", disk_name);
                            disks.push(dev_path);
                        }
                    }
                }
            }
        }
    }
    
    Ok(disks)
}

/// Retrieves detailed information about a disk
fn get_disk_info(disk_path: &PathBuf) -> Result<HashMap<String, String>> {
    let mut disk_info = HashMap::new();
    
    // Extract the base disk name (e.g., /dev/sda -> sda)
    let disk_name = disk_path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    
    // Read disk vendor, model, size from sysfs
    let sysfs_path = PathBuf::from("/sys/block").join(disk_name);
    
    // Check if device exists
    if !sysfs_path.exists() {
        bail!("Disk {} not found in system", disk_path.display());
    }
    
    // Get disk size
    if let Ok(size_bytes) = fs::read_to_string(sysfs_path.join("size")) {
        if let Ok(sectors) = size_bytes.trim().parse::<u64>() {
            // Sectors are typically 512 bytes
            let size_gb = (sectors * 512) as f64 / 1_073_741_824.0;
            disk_info.insert("size".to_string(), format!("{:.1} GB", size_gb));
        }
    }
    
    // Get vendor and model information
    if let Ok(vendor) = fs::read_to_string(sysfs_path.join("device/vendor")) {
        disk_info.insert("vendor".to_string(), vendor.trim().to_string());
    }
    
    if let Ok(model) = fs::read_to_string(sysfs_path.join("device/model")) {
        disk_info.insert("model".to_string(), model.trim().to_string());
    }
    
    // Check if removable
    if let Ok(removable) = fs::read_to_string(sysfs_path.join("removable")) {
        let is_removable = removable.trim() == "1";
        disk_info.insert("removable".to_string(), is_removable.to_string());
    }
    
    // Try to detect if it's a USB device
    let is_usb = fs::read_dir(sysfs_path.join("device"))
        .ok()
        .and_then(|entries| {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_symlink() && path.file_name().and_then(|n| n.to_str()) == Some("driver") {
                    if let Ok(target) = fs::read_link(path) {
                        let target_str = target.to_string_lossy();
                        if target_str.contains("usb") {
                            return Some(true);
                        }
                    }
                }
            }
            Some(false)
        })
        .unwrap_or(false);
    
    disk_info.insert("is_usb".to_string(), is_usb.to_string());
    
    // Try to get current mount points
    if let Ok(mounts) = fs::read_to_string("/proc/mounts") {
        let relevant_mounts: Vec<&str> = mounts.lines()
            .filter(|line| line.contains(disk_path.to_string_lossy().as_ref()))
            .collect();
        
        if !relevant_mounts.is_empty() {
            disk_info.insert("mounted".to_string(), "true".to_string());
            disk_info.insert("mount_points".to_string(), 
                relevant_mounts.iter()
                    .map(|line| {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() > 1 { parts[1] } else { "unknown" }
                    })
                    .collect::<Vec<&str>>()
                    .join(", "));
        } else {
            disk_info.insert("mounted".to_string(), "false".to_string());
        }
    }
    
    Ok(disk_info)
}

/// List the system's disks with enough detail to tell them apart, and let the
/// user pick one.
fn select_disk(prompt: &str) -> Result<PathBuf> {
    println!("\nAvailable disks on this system:");
    let disks = match list_available_disks() {
        Ok(disks) => disks,
        Err(e) => {
            bail!("Error listing disks: {}", e);
        }
    };

    if disks.is_empty() {
        bail!("No suitable disks found on the system");
    }

    // Create disk display info for selection
    let mut disk_display_info = Vec::new();
    let mut disk_paths = Vec::new();

    for disk_path in &disks {
        let disk_pathbuf = PathBuf::from(disk_path);
        match get_disk_info(&disk_pathbuf) {
            Ok(info) => {
                let unknown_str = "Unknown".to_string();
                let size = info.get("size").unwrap_or(&unknown_str);
                let model = info.get("model").unwrap_or(&unknown_str);
                let is_usb = info.get("is_usb").map_or(false, |v| v == "true");
                let is_removable = info.get("removable").map_or(false, |v| v == "true");

                let mut disk_type = "";
                if is_removable {
                    disk_type = " [REMOVABLE]";
                }

                if is_usb {
                    disk_type = " [USB]";
                }

                let display = format!("{} - {} - {}{}",
                    disk_path,
                    model,
                    size,
                    disk_type);

                disk_display_info.push(display);
                disk_paths.push(disk_path.clone());
            },
            Err(_) => {
                let display = format!("{} - No information available", disk_path);
                disk_display_info.push(display);
                disk_paths.push(disk_path.clone());
            }
        }
    }

    // Let user select the disk
    let selected_disk_display = inquire::Select::new(prompt, disk_display_info.clone())
        .prompt()?;

    // Find the selected disk path
    let selected_index = disk_display_info.iter().position(|d| d == &selected_disk_display).unwrap();
    Ok(PathBuf::from(&disk_paths[selected_index]))
}

/// Unlock the git-crypt'd parts of the checkout, if they are not already.
fn ensure_repo_unlocked() -> Result<()> {
    // check .git/config for git-crypt
    let git_config = std::fs::read_to_string(".git/config")?;
    if git_config.contains("git-crypt") {
        println!("Repository already decrypted");
        return Ok(());
    }

    println!("Decrypting Repository");

    // Load unlock configuration
    let unlock_config: UnlockConfig = serde_json::from_str(
        &std::fs::read_to_string("secrets/unlock-config.json")
            .unwrap_or_else(|_| {
                // Fallback to GPG-only if config doesn't exist
                r#"{"methods":{"gpg":{"enabled":true,"keyFile":"local.key.asc"}}}"#.to_string()
            })
    )?;

    // Build list of available unlock methods
    let mut method_names: Vec<&str> = Vec::new();
    if let Some(ref fido2) = unlock_config.methods.fido2 {
        if fido2.enabled && std::path::Path::new(&fido2.key_file).exists() {
            method_names.push("FIDO2 (security key)");
        }
    }
    if let Some(ref gpg) = unlock_config.methods.gpg {
        if gpg.enabled && std::path::Path::new(&gpg.key_file).exists() {
            method_names.push("GPG");
        }
    }

    if method_names.is_empty() {
        bail!("No unlock methods available. Ensure local.key.asc or local.key.age exists.");
    }

    // If only one method available, use it automatically
    let selected_method = if method_names.len() == 1 {
        method_names[0]
    } else {
        inquire::Select::new("Select unlock method", method_names).prompt()?
    };

    match selected_method {
        "FIDO2 (security key)" => {
            let fido2 = unlock_config.methods.fido2.as_ref().unwrap();
            let key_file = &fido2.key_file;

            // Sort identities with default first
            let mut identities = fido2.identities.clone();
            identities.sort_by(|a, b| b.default.cmp(&a.default));

            let mut unlocked = false;
            for identity in &identities {
                println!("Trying {} (touch your security key)...", identity.name);
                let identity_path = &identity.path;

                if run_cmd!(age --decrypt -i $identity_path $key_file | git-crypt unlock -).is_ok() {
                    println!("Unlocked with {}", identity.name);
                    unlocked = true;
                    break;
                }
            }

            if !unlocked {
                bail!("Failed to unlock with any FIDO2 identity");
            }
        },
        _ => {
            let gpg = unlock_config.methods.gpg.as_ref().unwrap();
            let key_file = &gpg.key_file;
            run_cmd!(gpg --decrypt $key_file | git-crypt unlock -)?;
        }
    }

    Ok(())
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    // Target hostname
    #[arg(required = true)]
    host: String,

    /// Target username
    #[arg(short, long, default_value = "bcnelson", value_delimiter = ',')]
    users: Vec<String>,

    /// Install a thin client: a host with too little RAM to evaluate or build
    /// its own configuration. Nothing is built locally -- the host is pushed to
    /// CI, the builder produces a signed closure, and this installs that.
    #[arg(long)]
    thin: bool,

    /// Resume an interrupted --thin install from its last checkpoint. Re-mounts
    /// the target disk if needed; never repartitions it.
    #[arg(long, requires = "thin")]
    resume: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    
    // Using simple stdlib check for root
    if std::env::var("USER")? == "root" {
        bail!("ERROR! Program should be run as a regular user");
    }

    wifi::ensure_connectivity()?;

    let home = std::env::var("HOME")?;
    let nix_config = PathBuf::from(&home).join("nix-config");

    if !nix_config.join(".git").is_dir() {
        run_cmd!(git clone "https://github.com/bcnelson/nix-config.git" $nix_config)?;
    }

    std::env::set_current_dir(&nix_config)?;

    // Fail before anything destructive if the thin-client prerequisites are not
    // in place: the closure this host will run is signed, and without the public
    // half committed neither CI nor this installer can verify it.
    let cache_public_key = if args.thin {
        Some(thin::read_cache_public_key()?)
    } else {
        None
    };

    ensure_repo_unlocked()?;

    // A thin client install spans a PR merge and a full closure build, so
    // picking up where a previous run left off is the normal case rather than
    // the exception. Take that path before the "host already in flake.nix"
    // check below, which a half-finished install would always trip.
    if args.thin {
        let existing = thin::load_state(&home, &args.host);
        if args.resume || existing.is_some() {
            return resume_thin_install(&args, &home, cache_public_key.as_ref().unwrap(), existing);
        }
    }

    let mut flake_content = std::fs::read_to_string("flake.nix")?;

    //check if the host is already in the flake
    if flake_content.contains(args.host.as_str()) {
        bail!("Host already exists in flake.nix");
    }

    let target_host_parts: Vec<&str> = args.host.split('-').collect();
    
    // Ensure the hostname format is correct (prefix-number)
    if target_host_parts.len() != 2 {
        bail!("Invalid hostname format: Expected format is <name>-<number> (e.g., sierra-2)");
    }
    
    let target_host_prefix = target_host_parts[0];
    let target_host_suffix = target_host_parts[1];
    
    // Ensure the suffix is a number
    if target_host_suffix.parse::<u32>().is_err() {
        bail!("Invalid hostname format: The suffix part after the dash must be a number (e.g., 'sierra-2')");
    }

    let selected_disk = select_disk("Select the disk to install NixOS on")?;

    // Gather detailed disk information
    println!("\nAnalyzing disk {}...", selected_disk.display());
    
    let disk_info = match get_disk_info(&selected_disk) {
        Ok(info) => info,
        Err(e) => {
            println!("WARNING: Could not get detailed disk information: {}", e);
            bail!("Failed to get disk information. Aborting for safety.");
        }
    };

    // Print detailed disk information
    println!("\nDisk information:");
    println!("  Path: {}", selected_disk.display());
    
    if let Some(model) = disk_info.get("model") {
        println!("  Model: {}", model);
    }
    
    if let Some(vendor) = disk_info.get("vendor") {
        println!("  Vendor: {}", vendor);
    }
    
    if let Some(size) = disk_info.get("size") {
        println!("  Size: {}", size);
    }
    
    // Generate warnings based on disk characteristics
    let mut warnings = Vec::new();
    
    if disk_info.get("removable").map_or(false, |v| v == "true") {
        warnings.push("⚠️ This appears to be a REMOVABLE device!".to_string());
    }
    
    if disk_info.get("is_usb").map_or(false, |v| v == "true") {
        warnings.push("⚠️ This appears to be a USB device! USB drives are typically NOT suitable for system installation.".to_string());
    }
    
    if let Some(size) = disk_info.get("size") {
        if let Ok(size_str) = size.split_whitespace().next().unwrap_or("0").parse::<f64>() {
            if size_str < 20.0 {
                warnings.push("⚠️ This disk is unusually small for a system installation (< 20GB)!".to_string());
            }
        }
    }
    
    if disk_info.get("mounted").map_or(false, |v| v == "true") {
        if let Some(mounts) = disk_info.get("mount_points") {
            warnings.push(format!("⚠️ This disk is currently mounted at: {}", mounts));
            
            // Check if this disk contains the boot media
            if mounts.contains("/run/initramfs") || mounts.contains("/nix/store") || mounts.contains("/iso") {
                warnings.push("⚠️ WARNING: This appears to be the BOOT MEDIA you're currently running from!".to_string());
                warnings.push("⚠️ Installing to this disk will likely FAIL and could brick your installation media!".to_string());
            }
        } else {
            warnings.push("⚠️ This disk is currently mounted!".to_string());
        }
    }
    
    // Check if this appears to be a system disk with existing partitions
    if let Ok(output) = Command::new("lsblk")
        .arg("-no")
        .arg("NAME,MOUNTPOINT")
        .arg(selected_disk.to_string_lossy().as_ref())
        .output() 
    {
        if let Ok(output_str) = String::from_utf8(output.stdout) {
            if output_str.contains("/boot") || output_str.contains("/ ") || output_str.contains("/home") {
                warnings.push("⚠️ This disk appears to contain an existing operating system!".to_string());
            }
        }
    }

    println!("\n========== WARNING ==========");
    println!("The disk {} is about to get COMPLETELY WIPED!", selected_disk.display());
    println!("NixOS will be installed on this disk for {}", target_host_prefix);
    println!("This is a DESTRUCTIVE operation that CANNOT be undone!");
    
    if !warnings.is_empty() {
        println!("\nADDITIONAL WARNINGS:");
        for warning in &warnings {
            println!("  {}", warning);
        }
    }
    println!("==============================\n");

    // Extra confirmation if there are warnings
    if !warnings.is_empty() {
        if !inquire::Confirm::new("This disk has warnings. Do you REALLY want to continue?")
            .with_default(false)
            .prompt()?
        {
            bail!("Installation aborted by user");
        }
    }

    if !inquire::Confirm::new("Are you sure you want to install on this disk?")
        .with_default(false)
        .prompt()? 
    {
        return Ok(());
    }

    let host_dir = format!("./nixos/{}", target_host_prefix);

    // check if the default.nix file exists
    let default_nix_path = format!("{}/default.nix", host_dir);
    let default_nix_exists = std::fs::metadata(&default_nix_path).is_ok();


    // A thin client's update path is fixed by its role -- it pulls a prebuilt
    // closure rather than rebuilding -- so there is nothing to ask here.
    let mut auto_updates = false;
    if !default_nix_exists && !args.thin {
        auto_updates = inquire::Confirm::new("Would you like automatic updates enabled?")
            .with_default(false)
            .prompt()?;
    }

    run_cmd!(sudo true)?;

    let disk_nix = if PathBuf::from(format!("nixos/{}/disks.nix", target_host_prefix)).exists() {
        format!("nixos/{}/disks.nix", target_host_prefix)
        //TODO: Check if it is a luks disk
    } else {
        // Check if luks is needed
        let luks = inquire::Confirm::new("Would you like to encrypt the disk?").with_default(false).prompt()?;
        match luks {
            true => {
                let passphrase = inquire::Password::new("Enter passphrase for disk encryption:").prompt()?;
                std::fs::write("/tmp/luks-password", passphrase)?;
                "disko/luks.nix".to_string()
            },
            false => {
                "disko/default.nix".to_string()
            }
        }
    };


    let desktop_config = if args.thin {
        // A full desktop is both too heavy for the hardware and beside the
        // point; whatever the host runs is decided by its own default.nix.
        String::new()
    } else {
        let desktop_options = vec![ "kde6", "None", "hyperland", "kde" ];
        let desktop = inquire::Select::new("Select a desktop environment", desktop_options)
            .prompt()?;

        match desktop {
            "kde6" => format!(" desktop = \"kde6\";"),
            "hyperland" => format!(" desktop = \"hyperland\";"),
            "kde" => format!(" desktop = \"kde\";"),
            _ => format!(""),
        }
    };

    // Swap has to be chosen against the disk, not just RAM: "twice RAM" on an
    // 8 GB eMMC does not fit, and finding that out inside disko means finding
    // out after the partition table has been written.
    let disk_gb = swap::disk_size_gb(&selected_disk)?;
    let swap_size = swap::select_swap_size(disk_gb)?;
    
    // Provide relevant feedback based on swap size
    if swap_size == 0 {
        println!("No swap partition will be created");
    } else if swap_size > 32 {
        println!("Creating a large swap partition ({}GB) - this may take a while", swap_size);
    } else {
        println!("Creating a {}GB swap partition", swap_size);
    }
    
    // Prefer a disko already on PATH -- the ISO ships one built against our own
    // nixpkgs. Fetching it instead pulls a second stdenv into the live store,
    // which on a 2 GB machine fills the tmpfs before partitioning even starts.
    let disko_status = thin::disko_command(
        "zap_create_mount",
        &disk_nix,
        selected_disk.to_string_lossy().as_ref(),
        swap_size,
    )
    .status()
    .context("failed to run disko")?;
    if !disko_status.success() {
        bail!("disko failed to partition {}", selected_disk.display());
    }

    run_cmd!(mkdir -p $host_dir)?;

    run_cmd!(sudo nixos-generate-config --dir "${host_dir}/generate" --root /mnt)?;
    run_cmd!(sudo mv "${host_dir}/generate/hardware-configuration.nix" "${host_dir}/${target_host_suffix}.hardware-configuration.nix")?;
    run_cmd!(sudo rm -rf "${host_dir}/generate")?;

    if !default_nix_exists {
        let default_nix_config = include_str!("../templates/default-host.nix");
        let default_with_autoupdates_nix_config = include_str!("../templates/default-host-autoupdate.nix");
        let default_thin_nix_config = include_str!("../templates/default-host-thin.nix");

        if args.thin {
            std::fs::write(&default_nix_path, default_thin_nix_config)?;
        } else if auto_updates {
            std::fs::write(&default_nix_path, default_with_autoupdates_nix_config)?;
        } else {
            std::fs::write(&default_nix_path, default_nix_config)?;
        }
    }

    // The registry the builder reads and the thin-client role asserts against.
    // Doing this after the host directory exists keeps a failed disko run from
    // leaving a hostname listed that has no configuration behind it.
    if args.thin {
        thin::register_thin_client(&args.host)?;
    }

    let users = format!("\"{}\"", args.users.join("\" \""));

    let new_host_config = format!(
        "\"{}\" = libx.mkHost {{ hostname = \"{}\"; usernames = [ {} ];{} }};",
        args.host, args.host, users, desktop_config
    );

    let hosts_regex = Regex::new(r"(?m)(\s*# INSERT_NEW_HOST_CONFIG_HERE\n)").unwrap();
    
    flake_content = hosts_regex.replace(&flake_content, |caps: &regex::Captures| {
        format!("{}        {}\n", &caps[0], new_host_config)
    }).to_string();
    
    std::fs::write("flake.nix", flake_content)?;

    let target_host = format!("{}", args.host);

    let ssh_ket_comment = format!("{}@nix-config", target_host);

    // ssh-keygen has no force flag: if the key exists it asks "Overwrite (y/n)?"
    // on stdin, which hangs a re-run or an unattended retry. Clear it first --
    // a leftover key from an abandoned attempt is never the one we want, since
    // its public half has to match what goes into hosts/data/<host>.nix.
    for stale in [format!("{}/id_ed25519", home), format!("{}/id_ed25519.pub", home)] {
        if std::path::Path::new(&stale).exists() {
            println!("Removing stale key from a previous run: {}", stale);
            std::fs::remove_file(&stale)?;
        }
    }

    // Generate SSH keys using Rust's native process library so that we can pass an empty passphrase
    let ssh_keygen_status = Command::new("ssh-keygen")
        .arg("-t")
        .arg("ed25519")
        .arg("-N")
        .arg("")
        .arg("-f")
        .arg(format!("{}/id_ed25519", home))
        .arg("-C")
        .arg(&ssh_ket_comment)
        .status()?;

    if !ssh_keygen_status.success() {
        bail!("Failed to generate SSH keys");
    }

    // read the public key
    let public_key = std::fs::read_to_string(format!("{}/id_ed25519.pub", home))?;

    let host_def_contents = include_str!("../templates/host.nix")
        .replace("INSERT_PUBLIC_KEY", &public_key);
    // write the host definition
    let host_def_path = format!("{}/hosts/data/{}.nix", nix_config.display(), args.host);
    println!("Writing host definition to {}", host_def_path);
    std::fs::write(host_def_path, host_def_contents)?;

    // `nix fmt` evaluates the formatter out of the flake, which pulls in
    // nixpkgs. On a thin client that is exactly the memory spike this whole
    // flow exists to avoid, and the files written above are already formatted.
    if !args.thin {
        run_cmd!(ignore sudo nix fmt)?;
    }

    if args.thin {
        // Place the host key now rather than after the install: the private half
        // only exists in this live session, and the resume path has to survive
        // the session going away entirely.
        install_host_key(&home)?;
        announce_deferred_rekey(&args.host);
    } else {
        //TODO: Better chack to see if this is needed
        run_cmd!(
            git add -A;
            nix run ".#agenix-rekey.x86_64-linux.rekey" -- --dummy;
        )?;
    }

    let branch = format!("install-{}", target_host);

    run_cmd!(
        git checkout -b "install-$target_host";
        git add -A;
        git config user.email "admin@nel.family";
        git config user.name "Automated Installer";
        git commit -m "Install $target_host_prefix";
        git config --unset "user.email";
        git config --unset "user.name";
    )?;

    if args.thin {
        let commit = git_head()?;

        // Read the manifest before pushing. If this host has been installed
        // before there is already one sitting there, and accepting it would
        // install the closure from the *previous* configuration.
        let baseline_commit = thin::fetch_manifest(&args.host)?.map(|m| m.commit);

        let mut state = State {
            hostname: args.host.clone(),
            stage: Stage::Prepared,
            disk: selected_disk.display().to_string(),
            disk_nix: disk_nix.clone(),
            swap_size,
            users: args.users.clone(),
            branch: branch.clone(),
            commit: commit.clone(),
            baseline_commit,
            store_path: None,
        };
        thin::save_state(&home, &state)?;

        run_cmd!(
            git config push.autoSetupRemote true;
            just push;
            git config --unset push.autoSetupRemote;
        )?;

        state.stage = Stage::Pushed;
        thin::save_state(&home, &state)?;

        println!();
        println!("Pushed branch {} ({}).", branch, commit);
        println!("Open a PR and merge it into main. CI checks every host, then the");
        println!("auto-update branch advances and romeo builds this host's closure.");
        println!();

        return finish_thin_install(&home, cache_public_key.as_ref().unwrap(), state);
    }

    run_cmd!(sudo nixos-install --no-root-password --flake .#$target_host)?;

    run_cmd!(
        git config push.autoSetupRemote true;
        just push;
        git config --unset push.autoSetupRemote;
        git switch auto-update;
    )?;

    println!("Copying nix-config to /config");
    run_cmd!(
        mkdir -p /mnt/etc/ssh;
        sudo cp $home/id_ed25519 "/mnt/etc/ssh/ssh_host_ed25519_key";
        sudo cp $home/id_ed25519.pub "/mnt/etc/ssh/ssh_host_ed25519_key.pub";
        sudo rsync -a "$home/nix-config/" "/mnt/config/";
    )?;

    for target_user in args.users {
        run_cmd!(sudo nixos-enter -c "passwd --expire $target_user")?;
    }

    Ok(())
}

fn git_head() -> Result<String> {
    let output = Command::new("git").args(["rev-parse", "HEAD"]).output()?;
    if !output.status.success() {
        bail!("failed to read HEAD");
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Copy the freshly generated SSH host key onto the target filesystem. Its
/// public half is what `hosts/data/<host>.nix` pins for agenix, so the two have
/// to stay together or every secret on the host fails to decrypt.
fn install_host_key(home: &str) -> Result<()> {
    let private = format!("{}/id_ed25519", home);
    let public = format!("{}/id_ed25519.pub", home);
    run_cmd!(
        sudo mkdir -p /mnt/etc/ssh;
        sudo cp $private "/mnt/etc/ssh/ssh_host_ed25519_key";
        sudo cp $public "/mnt/etc/ssh/ssh_host_ed25519_key.pub";
        sudo chmod 600 "/mnt/etc/ssh/ssh_host_ed25519_key";
    )?;
    println!("Installed SSH host key to /mnt/etc/ssh");
    Ok(())
}

/// Explain that this host's secrets have deliberately NOT been rekeyed.
///
/// A thin client cannot rekey its own secrets, and pretending otherwise wasted
/// everyone's time. agenix-rekey evaluates every host in the flake, which forces
/// a local build of nixpkgs-patched (applyPatches over patches/, in no binary
/// cache). On the installer ISO the nix store *is* RAM -- /nix/store is an
/// overlay whose upperdir is a tmpfs -- so that means pushing a whole nixpkgs
/// tree through the memory of a 2 GB machine. Measured in a VM: it livelocks,
/// with kswapd0 and nix's GC marker both blocked, 26 minutes of no progress and
/// no OOM kill to end it.
///
/// The dummy secrets this used to generate were worthless regardless: you always
/// had to rekey from a workstation afterwards. So skip it and say so loudly --
/// including that CI cannot pass until someone does, because a half-truth here
/// turns into a confusing red build later.
fn announce_deferred_rekey(hostname: &str) {
    let bar = "=".repeat(66);
    println!();
    println!("{}", bar);
    println!(" {} has not had agenix secrets rekeyed.", hostname);
    println!();
    println!(" This machine cannot rekey: agenix-rekey evaluates the whole flake,");
    println!(" which does not fit in 2 GB of RAM.");
    println!();
    println!(" Normally that is fine -- a thin client declares no agenix secrets,");
    println!(" so there is nothing to rekey and CI will pass as-is. If you have");
    println!(" given this host something that does declare one, its build will");
    println!(" fail until you rekey from a workstation with the security key:");
    println!();
    println!("     git fetch && git checkout install-{}", hostname);
    println!("     just rekey");
    println!("     git add secrets/hosts && git commit -m 'rekey {}'", hostname);
    println!("     git push");
    println!("{}", bar);
    println!();
}

/// Entry point for `--resume`: get back to a state where the closure can be
/// fetched and installed, then hand off to the normal tail of the install.
fn resume_thin_install(
    args: &Args,
    home: &str,
    cache_public_key: &str,
    existing: Option<State>,
) -> Result<()> {
    let state = match existing {
        Some(state) => {
            println!(
                "Resuming install of {} from stage {:?}",
                state.hostname, state.stage
            );
            state
        }
        None => {
            // No checkpoint in this session, so the live ISO was rebooted. The
            // real checkpoint is on the target disk; mount it and read it back.
            println!(
                "No install state for {} in this session. It should be on the target disk.",
                args.host
            );
            let disk = select_disk("Select the disk the interrupted install was targeting")?;
            let disk_nix = select_disko_config(&args.host)?;
            ensure_luks_password(&disk_nix)?;
            // Only used for partition sizing at create time; `--mode mount`
            // matches existing partitions by label and ignores it.
            thin::remount_target(&disk_nix, &disk.display().to_string(), 0)?;

            thin::load_state(home, &args.host).ok_or_else(|| {
                anyhow::anyhow!(
                    "No install state found for {} on {} either. \
                     If the disk was never partitioned, run without --resume.",
                    args.host,
                    disk.display()
                )
            })?
        }
    };

    if !PathBuf::from("/mnt/nix").exists() {
        println!("/mnt does not look mounted; re-mounting {}", state.disk);
        ensure_luks_password(&state.disk_nix)?;
        thin::remount_target(&state.disk_nix, &state.disk, state.swap_size)?;
    }

    finish_thin_install(home, cache_public_key, state)
}

/// Which disko config the interrupted install partitioned the disk with. Only
/// asked for when the checkpoint that would have recorded it is unreachable,
/// i.e. the live ISO was rebooted before the disk could be mounted.
fn select_disko_config(hostname: &str) -> Result<String> {
    let prefix = hostname.split('-').next().unwrap_or(hostname);
    let host_specific = format!("nixos/{}/disks.nix", prefix);

    let mut options = Vec::new();
    if PathBuf::from(&host_specific).exists() {
        options.push(host_specific);
    }
    options.push("disko/default.nix".to_string());
    options.push("disko/luks.nix".to_string());

    Ok(inquire::Select::new("disko config used for that install", options).prompt()?)
}

/// disko reads the passphrase for an encrypted disk from a file the install
/// wrote to /tmp. On the resume path that file is gone with the live session,
/// so ask again -- otherwise the re-mount fails with nothing useful to say.
fn ensure_luks_password(disk_nix: &str) -> Result<()> {
    if !disk_nix.contains("luks") || PathBuf::from("/tmp/luks-password").exists() {
        return Ok(());
    }

    let passphrase =
        inquire::Password::new("Disk encryption passphrase (needed to re-mount):").prompt()?;
    std::fs::write("/tmp/luks-password", passphrase)?;
    Ok(())
}

/// The part of a thin install that is worth resuming: wait for the builder,
/// fetch the closure, install it, and hand the machine over.
fn finish_thin_install(home: &str, cache_public_key: &str, mut state: State) -> Result<()> {
    if state.stage < Stage::Copied {
        let manifest = thin::wait_for_closure(&state.hostname, state.baseline_commit.as_deref())?;
        thin::copy_closure_to_target(&manifest.store_path, cache_public_key)?;
        state.store_path = Some(manifest.store_path);
        state.stage = Stage::Copied;
        thin::save_state(home, &state)?;
    }

    let store_path = state
        .store_path
        .clone()
        .context("state reached the copy stage without recording a store path")?;

    if state.stage < Stage::Installed {
        thin::install_closure(&store_path)?;
        state.stage = Stage::Installed;
        thin::save_state(home, &state)?;
    }

    for target_user in &state.users {
        run_cmd!(sudo nixos-enter -c "passwd --expire $target_user")?;
    }

    println!();
    println!("{} is installed and running {}", state.hostname, store_path);
    println!("It will poll {} for future updates.", thin::manifest_url(&state.hostname));
    println!();

    if inquire::Confirm::new("Reboot into the installed system now?")
        .with_default(true)
        .prompt()?
    {
        run_cmd!(sudo reboot)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flake_nix_host_insertion() {
        let flake_content = r#"{
  outputs = inputs@{ self, flake-utils-plus, ... }:
    flake-utils-plus.lib.mkFlake {
      hosts = let
        libx = import ./lib { inherit inputs; stateVersion = "23.05"; outputs = self; };
      in {
        # INSERT_NEW_HOST_CONFIG_HERE
        "existing-host" = libx.mkHost { hostname = "existing-host"; usernames = [ "user" ]; };
      };
    };
}"#;

        let expected_result = r#"{
  outputs = inputs@{ self, flake-utils-plus, ... }:
    flake-utils-plus.lib.mkFlake {
      hosts = let
        libx = import ./lib { inherit inputs; stateVersion = "23.05"; outputs = self; };
      in {
        # INSERT_NEW_HOST_CONFIG_HERE
        "test-host" = libx.mkHost { hostname = "test-host"; usernames = [ "testuser" ]; };
        "existing-host" = libx.mkHost { hostname = "existing-host"; usernames = [ "user" ]; };
      };
    };
}"#;

        let new_host_config = r#""test-host" = libx.mkHost { hostname = "test-host"; usernames = [ "testuser" ]; };"#;
        let hosts_regex = Regex::new(r"(?m)(\s*# INSERT_NEW_HOST_CONFIG_HERE\n)").unwrap();
        
        let result = hosts_regex.replace(&flake_content, |caps: &regex::Captures| {
            format!("{}        {}\n", &caps[0], new_host_config)
        }).to_string();

        assert_eq!(result, expected_result);
    }

    #[test]
    fn test_flake_nix_host_insertion_flexible_whitespace() {
        let flake_content = r#"{
      in {
    # INSERT_NEW_HOST_CONFIG_HERE
        "existing-host" = libx.mkHost { hostname = "existing-host"; usernames = [ "user" ]; };
      };
}"#;

        let new_host_config = r#""test-host" = libx.mkHost { hostname = "test-host"; usernames = [ "testuser" ]; };"#;
        let hosts_regex = Regex::new(r"(?m)(\s*# INSERT_NEW_HOST_CONFIG_HERE\n)").unwrap();
        
        let result = hosts_regex.replace(&flake_content, |caps: &regex::Captures| {
            format!("{}        {}\n", &caps[0], new_host_config)
        }).to_string();

        // Should find and replace the comment regardless of indentation
        assert!(result.contains(r#""test-host" = libx.mkHost { hostname = "test-host"; usernames = [ "testuser" ]; };"#));
        assert!(result.contains(r#"# INSERT_NEW_HOST_CONFIG_HERE"#));
    }
}