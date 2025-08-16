use anyhow::{Context, Result};
use serde_json::Value;
use std::process::Command;
use tracing::{debug, info, warn};

use crate::models::{AgeSecret, BitwardenConfig};

pub struct NixEvaluator {
    flake_path: String,
    host_filter: Vec<String>,
}

impl NixEvaluator {
    pub fn new(flake_path: String) -> Self {
        Self::new_with_filter(flake_path, Vec::new())
    }
    
    pub fn new_with_filter(flake_path: String, host_filter: Vec<String>) -> Self {
        // Convert to absolute path
        let absolute_path = if flake_path.starts_with("/") {
            flake_path
        } else {
            // Convert relative path to absolute
            std::env::current_dir()
                .ok()
                .and_then(|cwd| {
                    let path = if flake_path == "." {
                        cwd
                    } else {
                        cwd.join(&flake_path)
                    };
                    path.canonicalize().ok()
                })
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| {
                    // Fallback: use pwd command
                    Command::new("pwd")
                        .output()
                        .ok()
                        .and_then(|output| String::from_utf8(output.stdout).ok())
                        .map(|pwd| pwd.trim().to_string())
                        .unwrap_or_else(|| "/home/bcnelson/nix-config".to_string())
                })
        };
        
        info!("Using flake path: {}", absolute_path);
        if !host_filter.is_empty() {
            info!("Filtering to hosts: {:?}", host_filter);
        }
        Self { flake_path: absolute_path, host_filter }
    }

    pub async fn get_all_secrets(&self) -> Result<Vec<AgeSecret>> {
        info!("Evaluating NixOS configurations to find secrets...");
        
        // Get all secrets in a single Nix evaluation
        let hosts_expr = if self.host_filter.is_empty() {
            "builtins.attrNames flake.nixosConfigurations".to_string()
        } else {
            // Only evaluate specified hosts
            format!("{:?}", self.host_filter)
        };
        
        let nix_expr = format!(
            r#"
            let
              flake = builtins.getFlake "{}";
              allHosts = builtins.attrNames flake.nixosConfigurations;
              hosts = {};
              
              # Function to extract secrets from a host
              getHostSecrets = hostname:
                let
                  config = flake.nixosConfigurations.${{hostname}}.config;
                  secrets = config.age.secrets or {{}};
                  
                  # Filter secrets with bitwarden attribute
                  secretsWithBitwarden = builtins.filter 
                    (name: (secrets.${{name}}.bitwarden or null) != null)
                    (builtins.attrNames secrets);
                  
                  # Build result for this host
                  hostSecrets = map (name: {{
                    hostname = hostname;
                    attribute_name = name;
                    rekey_file = builtins.unsafeDiscardStringContext (toString (secrets.${{name}}.rekeyFile or ""));
                    bitwarden = secrets.${{name}}.bitwarden;
                  }}) secretsWithBitwarden;
                in hostSecrets;
              
              # Get secrets from all hosts and flatten
              allSecrets = builtins.concatLists (map getHostSecrets hosts);
            in allSecrets
            "#,
            self.flake_path, hosts_expr
        );

        let output = Command::new("nix")
            .args(&["eval", "--json", "--impure", "--expr", &nix_expr])
            .output()
            .context("Failed to evaluate all secrets at once")?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("Failed to evaluate secrets: {}", error));
        }

        let secrets_json: Vec<Value> = serde_json::from_slice(&output.stdout)
            .context("Failed to parse secrets")?;

        let mut all_secrets = Vec::new();
        
        for secret_value in secrets_json {
            if let Value::Object(secret_obj) = secret_value {
                let hostname = secret_obj.get("hostname")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                    
                let attribute_name = secret_obj.get("attribute_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                    
                let rekey_file = secret_obj.get("rekey_file")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                
                if let Some(bw_value) = secret_obj.get("bitwarden") {
                    if let Ok(bw_config) = serde_json::from_value::<BitwardenConfig>(bw_value.clone()) {
                        all_secrets.push(AgeSecret {
                            hostname,
                            attribute_name,
                            rekey_file,
                            bitwarden: bw_config,
                        });
                    }
                }
            }
        }

        info!("Found {} total secrets with bitwarden config", all_secrets.len());
        Ok(all_secrets)
    }

    async fn get_hosts(&self) -> Result<Vec<String>> {
        let output = Command::new("nix")
            .args(&[
                "eval",
                &format!("{}#nixosConfigurations", self.flake_path),
                "--apply",
                "builtins.attrNames",
                "--json",
                "--impure",
            ])
            .output()
            .context("Failed to get host list from flake")?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("Failed to evaluate hosts: {}", error));
        }

        let hosts: Vec<String> = serde_json::from_slice(&output.stdout)
            .context("Failed to parse host list")?;

        Ok(hosts)
    }

    async fn get_host_secrets(&self, hostname: &str) -> Result<Vec<AgeSecret>> {
        // Build the Nix expression to extract age.secrets with bitwarden attributes
        let nix_expr = format!(
            r#"
            let
              config = (builtins.getFlake "{}").nixosConfigurations.{}.config;
              secrets = config.age.secrets or {{}};
              
              # Filter secrets with bitwarden attribute
              secretsWithBitwarden = builtins.filter 
                (name: (secrets.${{name}}.bitwarden or null) != null)
                (builtins.attrNames secrets);
              
              # Build result
              result = builtins.listToAttrs (map (name: {{
                name = name;
                value = {{
                  # Store the original rekeyFile path expression as a string
                  rekeyFile = builtins.unsafeDiscardStringContext (toString (secrets.${{name}}.rekeyFile or ""));
                  bitwarden = secrets.${{name}}.bitwarden;
                }};
              }}) secretsWithBitwarden);
            in result
            "#,
            self.flake_path, hostname
        );

        let output = Command::new("nix")
            .args(&["eval", "--json", "--impure", "--expr", &nix_expr])
            .output()
            .context(format!("Failed to evaluate secrets for host {}", hostname))?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            // Some hosts might not have age.secrets at all, that's okay
            if error.contains("attribute 'age'") || error.contains("attribute 'secrets'") {
                debug!("Host {} has no age.secrets", hostname);
                return Ok(Vec::new());
            }
            return Err(anyhow::anyhow!("Failed to evaluate {}: {}", hostname, error));
        }

        let secrets_json: Value = serde_json::from_slice(&output.stdout)
            .context(format!("Failed to parse secrets for {}", hostname))?;

        let mut secrets = Vec::new();

        if let Value::Object(map) = secrets_json {
            for (attr_name, value) in map {
                if let Value::Object(secret_obj) = value {
                    // Extract rekeyFile
                    let rekey_file = secret_obj.get("rekeyFile")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    // Extract bitwarden config
                    if let Some(bw_value) = secret_obj.get("bitwarden") {
                        if let Ok(bw_config) = serde_json::from_value::<BitwardenConfig>(bw_value.clone()) {
                            secrets.push(AgeSecret {
                                hostname: hostname.to_string(),
                                attribute_name: attr_name.clone(),
                                rekey_file,
                                bitwarden: bw_config,
                            });
                        }
                    }
                }
            }
        }

        debug!("Found {} secrets with bitwarden config in {}", secrets.len(), hostname);
        Ok(secrets)
    }

    pub async fn get_master_identities(&self) -> Result<Vec<String>> {
        // Get master identities from the NixOS configuration
        let nix_expr = format!(
            r#"
            let
              flake = builtins.getFlake "{}";
              # Get the first host to extract the master identities from
              # They should be the same across all hosts
              firstHost = builtins.head (builtins.attrNames flake.nixosConfigurations);
              config = flake.nixosConfigurations.${{firstHost}}.config;
              masterIdentities = config.age.rekey.masterIdentities or [];
              
              # Extract the identity paths - these will be store paths
              identityPaths = map (id: toString id.identity) masterIdentities;
            in identityPaths
            "#,
            self.flake_path
        );

        let output = Command::new("nix")
            .args(&["eval", "--json", "--impure", "--expr", &nix_expr])
            .output()
            .context("Failed to get master identities from config")?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("Failed to evaluate master identities: {}", error));
        }

        let store_paths: Vec<String> = serde_json::from_slice(&output.stdout)
            .context("Failed to parse master identities")?;

        // Convert store paths to actual file paths
        let mut identities = Vec::new();
        for store_path in store_paths {
            // Extract the filename from the store path
            if let Some(filename) = std::path::Path::new(&store_path).file_name() {
                let filename_str = filename.to_string_lossy();
                // Construct the actual path
                let actual_path = format!("{}/secrets/masterKeys/{}", self.flake_path, filename_str);
                if std::path::Path::new(&actual_path).exists() {
                    identities.push(actual_path);
                } else {
                    // Try without extension changes in case the store mangles names
                    warn!("Master identity not found at expected path: {}", actual_path);
                }
            }
        }

        info!("Found {} master identities in config", identities.len());
        for identity in &identities {
            debug!("Master identity: {}", identity);
        }

        Ok(identities)
    }

    pub fn resolve_age_file_path(&self, secret: &AgeSecret) -> Result<String> {
        // For agenix-rekey, we need to find the MASTER-encrypted file, not the rekeyed one
        // The rekeyFile path points to the original source file
        
        let store_path = &secret.rekey_file;
        if store_path.is_empty() {
            return Err(anyhow::anyhow!("No rekeyFile specified for {}", secret.attribute_name));
        }
        
        // First, try to evaluate the actual rekeyFile path from the config
        let nix_expr = format!(
            r#"
            let
              config = (builtins.getFlake "{}").nixosConfigurations.{}.config;
              secret = config.age.secrets.{} or {{}};
              rekeyFile = secret.rekeyFile or null;
              # Get the path relative to the flake root
              path = if rekeyFile != null then 
                builtins.unsafeDiscardStringContext (toString rekeyFile)
              else "";
            in path
            "#,
            self.flake_path, secret.hostname, secret.attribute_name
        );

        let output = Command::new("nix")
            .args(&["eval", "--raw", "--impure", "--expr", &nix_expr])
            .output()
            .context("Failed to resolve rekeyFile path")?;

        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).to_string();
            if !path.is_empty() {
                // This might be a store path, extract the relative path
                if path.starts_with("/nix/store/") {
                    // Find the actual source file by pattern matching
                    // The store path contains the hash-prefixed source
                    // We need to find the original file in the repo
                    
                    // For files like nixos/romeo/services/secrets/rom_auth_secret_key.age
                    // Try to find it based on the hostname and secret name
                    let possible_paths = vec![
                        format!("{}/nixos/{}/services/secrets/{}.age", 
                            self.flake_path, 
                            secret.hostname.split('-').next().unwrap_or(&secret.hostname),
                            secret.attribute_name.replace('-', "_")),
                        format!("{}/nixos/{}/services/secrets/{}.age", 
                            self.flake_path, 
                            secret.hostname,
                            secret.attribute_name.replace('-', "_")),
                        format!("{}/secrets/store/{}/{}.age", 
                            self.flake_path,
                            secret.hostname.split('-').next().unwrap_or(&secret.hostname),
                            secret.attribute_name.replace('-', "_")),
                        format!("{}/secrets/store/{}.age", 
                            self.flake_path,
                            secret.attribute_name.replace('-', "_")),
                    ];
                    
                    for test_path in &possible_paths {
                        if std::path::Path::new(test_path).exists() {
                            debug!("Found master-encrypted age file at: {}", test_path);
                            return Ok(test_path.clone());
                        }
                    }
                    
                    return Err(anyhow::anyhow!(
                        "Could not find master-encrypted file for secret '{}'. Tried: {:?}",
                        secret.attribute_name,
                        possible_paths
                    ));
                } else if std::path::Path::new(&path).exists() {
                    // Direct path that exists
                    debug!("Found age file at: {}", path);
                    return Ok(path);
                }
            }
        }
        
        // Fallback: construct the path manually
        Err(anyhow::anyhow!(
            "Could not resolve master-encrypted file for secret '{}'",
            secret.attribute_name
        ))
    }
}