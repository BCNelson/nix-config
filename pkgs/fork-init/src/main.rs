use anyhow::{bail, Context, Result};
use clap::Parser;
use cmd_lib::run_cmd;
use std::fs;
use std::path::PathBuf;

// Embed templates at compile time
const FLAKE_TEMPLATE: &str = include_str!("../templates/flake.nix");
const DEVENV_TEMPLATE: &str = include_str!("../templates/devenv.nix");
const ENVRC_CONFIG: &str = include_str!("../templates/envrc-config");
const ENVRC_PROJECT: &str = include_str!("../templates/envrc-project");

#[derive(Parser)]
#[command(name = "fork-init")]
#[command(about = "Initialize external devenv for forked repos")]
struct Cli {}

fn main() -> Result<()> {
    let _cli = Cli::parse();

    // 1. Get project name from current directory
    let current_dir = std::env::current_dir()?;
    let project_name = current_dir
        .file_name()
        .and_then(|n| n.to_str())
        .context("Could not determine project name from current directory")?;

    // 2. Expand ~/dev-configs path
    let home = std::env::var("HOME")?;
    let config_dir = PathBuf::from(&home).join("dev-configs").join(project_name);

    // 3. Check for existing config directory
    if config_dir.exists() {
        bail!("Config directory already exists: {}", config_dir.display());
    }

    // 4. Check for existing .envrc in current directory
    if current_dir.join(".envrc").exists() {
        bail!(".envrc already exists in current directory");
    }

    // 5. Create config directory and write files (using templates)
    fs::create_dir_all(&config_dir)?;
    fs::write(config_dir.join("flake.nix"), FLAKE_TEMPLATE)?;
    fs::write(config_dir.join("devenv.nix"), DEVENV_TEMPLATE)?;
    fs::write(config_dir.join(".envrc"), ENVRC_CONFIG)?;

    // 6. Create .envrc in current directory (replace placeholder)
    let project_envrc = ENVRC_PROJECT.replace("PROJECT_NAME", project_name);
    fs::write(current_dir.join(".envrc"), project_envrc)?;

    // 7. Append to .git/info/exclude
    let exclude_path = current_dir.join(".git/info/exclude");
    let mut exclude_content = fs::read_to_string(&exclude_path).unwrap_or_default();
    let mut modified = false;

    for entry in [".envrc", ".devenv", ".direnv"] {
        if !exclude_content.contains(entry) {
            exclude_content.push_str(&format!("\n{}\n", entry));
            modified = true;
        }
    }

    if modified {
        fs::write(&exclude_path, exclude_content)?;
    }

    // 8. Run `direnv allow`
    run_cmd!(direnv allow)?;

    // 9. Print success message
    println!("Created devenv config at: {}", config_dir.display());
    println!(
        "Edit {} to add your development tools",
        config_dir.join("devenv.nix").display()
    );

    Ok(())
}
