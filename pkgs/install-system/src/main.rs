use std::path::PathBuf;
use clap::Parser;
use anyhow::{Result, bail, anyhow};
use cmd_lib::run_cmd;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    // Target hostname
    #[arg(required = true)]
    target_host: String,

    /// Target username
    #[arg(default_value = "bcnelson")]
    target_user: String,

    /// Target disk
    #[arg(required = true)]
    target_disk: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    
    // Using simple stdlib check for root
    if std::env::var("USER")? == "root" {
        bail!("ERROR! Program should be run as a regular user");
    }

    let home = std::env::var("HOME")?;
    let nix_config = PathBuf::from(&home).join("nix-config");

    if !nix_config.join(".git").is_dir() {
        run_cmd!(git clone "https://github.com/bcnelson/nix-config.git" $nix_config)?;
    }

    std::env::set_current_dir(&nix_config)?;

    println!("Decrypting Repository");
    run_cmd!(gpg --decrypt local.key.asc | git-crypt unlock -)?;

    let target_host_prefix = args.target_host.split('-').next().unwrap();

    println!("WARNING! The disk {} in {} is about to get wiped", 
                 args.target_disk.display(), target_host_prefix);
    println!("         NixOS will be re-installed");
    println!("         This is a destructive operation\n");

    print!("Are you sure? [y/N] ");
    std::io::Write::flush(&mut std::io::stdout())?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
        return Ok(());
    }

    print!("Would you like automatic updates enabled? [y/N] ");
    std::io::Write::flush(&mut std::io::stdout())?;

    input.clear();
    std::io::stdin().read_line(&mut input)?;

    let auto_updates = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");

    run_cmd!(sudo true)?;

    let disk_nix = if PathBuf::from(format!("nixos/{}/disks.nix", target_host_prefix)).exists() {
        format!("nixos/{}/disks.nix", target_host_prefix)
    } else {
        "disko/default.nix".to_string()
    };

    let disk_arg = format!("\"{}\"", args.target_disk.display());

    run_cmd!( sudo nix run github:nix-community/disko --extra-experimental-features "nix-command flakes" --no-write-lock-file -- --mode zap_create_mount $disk_nix --arg disk $disk_arg)?;

    let host_dir = format!("./nixos/{}", target_host_prefix);
    run_cmd!(mkdir -p $host_dir)?;

    run_cmd!(sudo nixos-generate-config --dir $host_dir --root /mnt)?;
    run_cmd!(rm -f "${host_dir}/configuration.nix")?;

    let default_nix_path = format!("{}/default.nix", host_dir);
    let default_nix_config = include_str!("../templates/default-host.nix");
    let default_with_autoupdates_nix_config = include_str!("../templates/default-host-autoupdate.nix");

    if auto_updates {
        std::fs::write(&default_nix_path, default_with_autoupdates_nix_config)?;
    } else {
        std::fs::write(&default_nix_path, default_nix_config)?;
    }

    let new_host_config = format!(
        "&\n        \"{}\" = libx.mkHost {{ hostname = \"{}\"; usernames = [ \"{}\" ]; inherit libx; version = \"unstable\"; }};",
        args.target_host, args.target_host, args.target_user
    );
    let mut flake_content = std::fs::read_to_string("flake.nix")?;
    // find end of INSERT_HOST_CONFIG
    let postition = flake_content.find("INSERT_HOST_CONFIG").ok_or_else(|| anyhow!("INSERT_HOST_CONFIG not found"))? + 18;
    flake_content.insert_str(postition, &new_host_config);
    
    std::fs::write("flake.nix", flake_content)?;

    let target_host = format!("{}", args.target_host);

    let ssh_ket_comment = format!("{}@nix-config", target_host);

    // Generate SSH keys
    run_cmd!(ssh-keygen -t ed25519 -N "\"\"" -f "$home/id_ed25519" -C $ssh_ket_comment)?;

    // read the public key
    let public_key = std::fs::read_to_string(format!("{}/id_ed25519.pub", home))?;

    let host_def_contents = include_str!("../templates/host.nix")
        .replace("INSERT_PUBLIC_KEY", &public_key);
    // write the host definition
    let host_def_path = format!("{}/hosts/data/{}.nix", nix_config.display(), args.target_host);
    println!("Writing host definition to {}", host_def_path);
    std::fs::write(host_def_path, host_def_contents)?;

    if auto_updates {
        run_cmd!(
            git add -A;
            nix run ".#agenix-rekey.x86_64-linux.rekey";
        )?;
    }

    run_cmd!(
        git add -A;
        git config user.email "admin@nel.family";
        git config user.name "Automated Installer";
        git commit -m "Install $target_host_prefix";
        git config --unset "user.email";
        git config --unset "user.name";
    )?;

    run_cmd!(sudo nixos-install --no-root-password --flake .#$target_host)?;

    let target_user = format!("{}", args.target_user);

    println!("Copying nix-config to /config and /mnt/home/{}", target_user);
    run_cmd!(
        mkdir -p /mnt/etc/ssh;
        sudo cp $home/id_ed25519 "/mnt/etc/ssh/ssh_host_ed25519_key";
        sudo cp $home/id_ed25519.pub "/mnt/etc/ssh/ssh_host_ed25519_key.pub";
        sudo rsync -a "$home/nix-config" "/mnt/config/";
        sudo rsync -a --delete "$home/nix-config" "/mnt/home/$target_user";
    )?;

    run_cmd!(sudo nixos-enter -c "passwd --expire $target_user")?;

    Ok(())
}