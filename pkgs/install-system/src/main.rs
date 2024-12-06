use std::path::PathBuf;
use clap::Parser;
use anyhow::{Result, bail};
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

    run_cmd!(sudo true)?;

    let disk_nix = if PathBuf::from(format!("nixos/{}/disks.nix", target_host_prefix)).exists() {
        format!("nixos/{}/disks.nix", target_host_prefix)
    } else {
        "disko/default.nix".to_string()
    };

    let disk_arg = format!("--arg disk \"{}\"", args.target_disk.to_string_lossy());

    run_cmd!( sudo nix run github:nix-community/disko --extra-experimental-features "nix-command flakes" --no-write-lock-file -- --mode zap_create_mount $disk_nix $disk_arg)?;

    let host_dir = format!("./nixos/{}", target_host_prefix);
    run_cmd!(mkdir -p $host_dir)?;

    run_cmd!(sudo nixos-generate-config --dir $host_dir --root /mnt)?;
    run_cmd!(rm -f "${host_dir}/configuration.nix")?;

    let default_nix_path = format!("{}/default.nix", host_dir);
    if !std::path::Path::new(&default_nix_path).exists() {
        run_cmd!(echo "{ ... }:\n{\n  imports = [\n    ./hardware-configuration.nix\n  ];\n}" > $default_nix_path)?;
    }

    let new_host_config = format!(
        "&\n        \"{}\" = libx.mkHost {{ hostname = \"{}\"; usernames = [ \"{}\" ]; inherit libx; version = \"unstable\"; }};",
        args.target_host, args.target_host, args.target_user
    );
    let flake_content = std::fs::read_to_string("flake.nix")?;
    let updated_flake = flake_content.replace("INSERT_HOST_CONFIG", &new_host_config);
    std::fs::write("flake.nix", updated_flake)?;

    let target_host = format!("{}", args.target_host);

    run_cmd!(sudo nixos-install --no-root-password --flake .#$target_host)?;

    let target_user = format!("{}", args.target_user);

    run_cmd!(
        sudo rsync -a "$home/nix-config" /config;
        sudo rsync -a --delete "$home/nix-config" "/mnt/home/$target_user"
    )?;

    run_cmd!(sudo nixos-enter -c "passwd --expire $target_user")?;

    Ok(())
}