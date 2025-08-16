mod bitwarden;
mod models;
mod nix_eval;
mod sync;

use anyhow::Result;
use clap::Parser;
use std::env;
use std::path::PathBuf;
use tracing::{error, info, warn};
use tracing_subscriber;

#[derive(Parser, Debug)]
#[clap(author, version, about = "Sync age-encrypted secrets to Bitwarden")]
struct Args {
    /// Path to the flake (defaults to current directory)
    #[clap(short, long, default_value = ".")]
    flake: String,

    /// Path to age identity file (optional when using FIDO keys)
    #[clap(short, long)]
    identity: Option<PathBuf>,
    
    /// Use FIDO key for decryption (no identity file needed)
    #[clap(short = 'f', long)]
    fido: bool,

    /// Bitwarden password (can also use BW_PASSWORD env var)
    #[clap(short = 'p', long, env = "BW_PASSWORD")]
    password: Option<String>,

    /// Only sync secrets from specific host(s) (can be specified multiple times)
    #[clap(short = 'H', long = "host")]
    hosts: Vec<String>,

    /// Verbose output
    #[clap(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Setup logging
    let filter = if args.verbose {
        "age_bitwarden_sync=debug,info"
    } else {
        "age_bitwarden_sync=info"
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();

    info!("Starting age-bitwarden-sync");

    // Get master identities from NixOS config
    let master_identities = if args.fido {
        info!("Getting master identities from NixOS configuration...");
        let evaluator = nix_eval::NixEvaluator::new(args.flake.clone());
        match evaluator.get_master_identities().await {
            Ok(identities) => {
                if identities.is_empty() {
                    warn!("No master identities found in config");
                    vec![]
                } else {
                    info!("Found {} master identities in config", identities.len());
                    identities
                }
            }
            Err(e) => {
                warn!("Failed to get master identities from config: {}", e);
                vec![]
            }
        }
    } else {
        vec![]
    };

    // Determine age identities to use
    let age_identities = if args.fido {
        // Using FIDO key - use all master identities from config
        info!("Using FIDO key for decryption");
        info!("Make sure your FIDO key is connected");
        
        let mut valid_identities = Vec::new();
        for identity in &master_identities {
            let path = PathBuf::from(identity);
            if path.exists() {
                info!("Found master identity: {}", path.display());
                valid_identities.push(path.to_string_lossy().to_string());
            } else {
                warn!("Master identity not found: {}", identity);
            }
        }
        
        if valid_identities.is_empty() {
            warn!("No valid master identity files found");
        }
        valid_identities
    } else if let Some(path) = args.identity {
        info!("Using age identity: {}", path.display());
        vec![path.to_string_lossy().to_string()]
    } else {
        // Try common locations
        let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let default_paths = vec![
            PathBuf::from(&home).join(".config/age/keys.txt"),
            PathBuf::from(&home).join(".age/keys.txt"),
            PathBuf::from("./secrets/keys.txt"),
        ];

        let mut found_identities = Vec::new();
        for path in default_paths {
            if path.exists() {
                info!("Found age identity at: {}", path.display());
                found_identities.push(path.to_string_lossy().to_string());
                break; // Use first found
            }
        }
        
        if found_identities.is_empty() {
            return Err(anyhow::anyhow!(
                "No age identity found. Please provide one with -i or use --fido for FIDO keys"
            ));
        }
        found_identities
    };

    info!("Using flake: {}", args.flake);

    // Set password in environment if provided
    if let Some(password) = args.password {
        env::set_var("BW_PASSWORD", password);
    }

    // Create syncer and run
    let mut syncer = if args.hosts.is_empty() {
        sync::SecretSyncer::new(
            args.flake,
            age_identities,
        )
        .await?
    } else {
        info!("Filtering to hosts: {:?}", args.hosts);
        sync::SecretSyncer::new_with_filter(
            args.flake,
            age_identities,
            args.hosts,
        )
        .await?
    };

    match syncer.sync_all().await {
        Ok(_) => {
            info!("Sync completed successfully!");
            Ok(())
        }
        Err(e) => {
            error!("Sync failed: {}", e);
            std::process::exit(1);
        }
    }
}