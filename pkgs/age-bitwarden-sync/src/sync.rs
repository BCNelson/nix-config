use anyhow::{Context, Result};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::process::{Command, Stdio};
use tracing::{debug, info, warn};

use crate::bitwarden::BitwardenClient;
use crate::models::{AgeSecret, BitwardenField, BitwardenItem, BitwardenLogin, BitwardenSecureNote};
use crate::nix_eval::NixEvaluator;

pub struct SecretSyncer {
    bitwarden: BitwardenClient,
    evaluator: NixEvaluator,
    folder_id: String,
    age_identities: Vec<String>,
    host_filter: Vec<String>,
}

impl SecretSyncer {
    pub async fn new(flake_path: String, age_identities: Vec<String>) -> Result<Self> {
        Self::new_with_filter(flake_path, age_identities, Vec::new()).await
    }
    
    pub async fn new_with_filter(flake_path: String, age_identities: Vec<String>, host_filter: Vec<String>) -> Result<Self> {
        let bitwarden = BitwardenClient::new().await?;
        let evaluator = NixEvaluator::new_with_filter(flake_path, host_filter.clone());
        
        Ok(Self {
            bitwarden,
            evaluator,
            folder_id: String::new(),
            age_identities,
            host_filter,
        })
    }

    pub async fn sync_all(&mut self) -> Result<()> {
        // Check Bitwarden status
        let status = self.bitwarden.status().await?;
        
        if status.status != "unlocked" {
            // Try to unlock with environment variable or prompt
            let password = std::env::var("BW_PASSWORD")
                .or_else(|_| {
                    // Try to get password from command
                    rpassword::prompt_password("Bitwarden master password: ")
                })
                .context("Failed to get Bitwarden password")?;
            
            self.bitwarden.unlock(&password).await?;
        }

        // Sync vault first
        self.bitwarden.sync().await?;

        // We'll use per-secret folder configuration, so don't set a default folder here
        // self.folder_id will remain empty unless we need a default

        // Get all secrets from Nix configurations (already filtered if host filter is set)
        let secrets = self.evaluator.get_all_secrets().await?;
        
        if secrets.is_empty() {
            warn!("No secrets with bitwarden configuration found");
            return Ok(());
        }

        info!("Processing {} secrets", secrets.len());

        let mut success_count = 0;
        let mut skip_count = 0;
        let mut error_count = 0;

        for secret in secrets {
            match self.sync_secret(&secret).await {
                Ok(SyncResult::Created) => {
                    info!("✓ Created: {} - {}", secret.hostname, secret.attribute_name);
                    success_count += 1;
                }
                Ok(SyncResult::Updated) => {
                    info!("✓ Updated: {} - {}", secret.hostname, secret.attribute_name);
                    success_count += 1;
                }
                Ok(SyncResult::Skipped) => {
                    debug!("⊘ Skipped: {} - {} (no changes)", secret.hostname, secret.attribute_name);
                    skip_count += 1;
                }
                Err(e) => {
                    warn!("✗ Failed: {} - {}: {}", secret.hostname, secret.attribute_name, e);
                    error_count += 1;
                }
            }
        }

        // Final sync to push changes
        self.bitwarden.sync().await?;

        info!(
            "\nSync complete: {} created/updated, {} skipped, {} errors",
            success_count, skip_count, error_count
        );

        if error_count > 0 {
            return Err(anyhow::anyhow!("{} secrets failed to sync", error_count));
        }

        Ok(())
    }

    async fn sync_secret(&self, secret: &AgeSecret) -> Result<SyncResult> {
        // Resolve the age file path
        let age_file_path = self.evaluator.resolve_age_file_path(secret)?;
        
        // Decrypt the secret
        let secret_value = self.decrypt_age_file(&age_file_path)?;
        
        // Calculate checksum
        let checksum = calculate_checksum(&secret_value);
        
        // Build the unique identifier for this secret
        let age_path = format!(
            "hosts/{}/{}",
            secret.hostname,
            secret.attribute_name
        );

        // Get or create the folder for this secret
        let folder_name = secret.bitwarden.folder.as_deref().unwrap_or("NixOS Secrets");
        let folder_id = self.bitwarden.get_or_create_folder(folder_name).await?;

        // Check if item already exists
        let existing_item = self
            .bitwarden
            .find_item_by_field("age_path", &age_path, &folder_id)
            .await?;

        let item_name = format!("{} - {}", secret.bitwarden.name, secret.hostname);

        if let Some(mut existing) = existing_item {
            // Check if update is needed
            if let Some(fields) = &existing.fields {
                for field in fields {
                    if field.name.as_deref() == Some("age_checksum") && field.value.as_deref() == Some(&checksum) {
                        return Ok(SyncResult::Skipped);
                    }
                }
            }

            // Update existing item
            existing.name = item_name;
            existing.folder_id = Some(folder_id.clone());
            existing.favorite = secret.bitwarden.favorite;
            existing.reprompt = if secret.bitwarden.reprompt { 1 } else { 0 };
            existing.organization_id = secret.bitwarden.organization_id.clone();
            existing.collection_ids = secret.bitwarden.collection_ids.clone();
            
            // Determine item type and set content
            if secret.bitwarden.username.is_some() {
                existing.item_type = 1; // Login
                existing.notes = secret.bitwarden.notes.clone();
                existing.secure_note = None;
                existing.login = Some(BitwardenLogin {
                    username: secret.bitwarden.username.clone(),
                    password: Some(secret_value.clone()),
                    uris: secret.bitwarden.uris.as_ref().map(|uris| uris.to_bitwarden_uris()),
                    totp: secret.bitwarden.totp.clone(),
                    password_revision_date: None,
                    fido2_credentials: None,
                });
            } else {
                existing.item_type = 2; // Secure Note
                existing.notes = Some(secret_value.clone());
                existing.login = None;
                existing.secure_note = Some(BitwardenSecureNote {
                    note_type: 0, // Generic note
                });
            }

            // Update fields
            existing.fields = Some(self.build_fields(secret, &age_path, &checksum));

            self.bitwarden.update_item(&existing).await?;
            Ok(SyncResult::Updated)
        } else {
            // Create new item
            let mut new_item = BitwardenItem {
                id: None,
                name: item_name,
                item_type: if secret.bitwarden.username.is_some() { 1 } else { 2 },
                notes: None,
                login: None,
                secure_note: None,
                fields: Some(self.build_fields(secret, &age_path, &checksum)),
                folder_id: Some(folder_id),
                favorite: secret.bitwarden.favorite,
                reprompt: if secret.bitwarden.reprompt { 1 } else { 0 },
                organization_id: secret.bitwarden.organization_id.clone(),
                collection_ids: secret.bitwarden.collection_ids.clone(),
                password_history: None,
                revision_date: None,
                creation_date: None,
                deleted_date: None,
                object: None,
            };

            // Set content based on type
            if secret.bitwarden.username.is_some() {
                new_item.notes = secret.bitwarden.notes.clone();
                new_item.login = Some(BitwardenLogin {
                    username: secret.bitwarden.username.clone(),
                    password: Some(secret_value),
                    uris: secret.bitwarden.uris.as_ref().map(|uris| uris.to_bitwarden_uris()),
                    totp: secret.bitwarden.totp.clone(),
                    password_revision_date: None,
                    fido2_credentials: None,
                });
            } else {
                // For secure notes, put the secret in notes field
                new_item.notes = Some(secret_value);
                new_item.secure_note = Some(BitwardenSecureNote {
                    note_type: 0, // Generic note
                });
            }

            self.bitwarden.create_item(&new_item).await?;
            Ok(SyncResult::Created)
        }
    }

    fn build_fields(&self, secret: &AgeSecret, age_path: &str, checksum: &str) -> Vec<BitwardenField> {
        let mut fields = vec![
            BitwardenField {
                name: Some("age_path".to_string()),
                value: Some(age_path.to_string()),
                field_type: 0,
                linked_id: None,
            },
            BitwardenField {
                name: Some("nix_attribute".to_string()),
                value: Some(format!("age.secrets.{}", secret.attribute_name)),
                field_type: 0,
                linked_id: None,
            },
            BitwardenField {
                name: Some("hostname".to_string()),
                value: Some(secret.hostname.clone()),
                field_type: 0,
                linked_id: None,
            },
            BitwardenField {
                name: Some("age_checksum".to_string()),
                value: Some(checksum.to_string()),
                field_type: 1, // Hidden
                linked_id: None,
            },
            BitwardenField {
                name: Some("last_synced".to_string()),
                value: Some(Utc::now().to_rfc3339()),
                field_type: 0,
                linked_id: None,
            },
        ];

        // Add custom fields from the configuration
        if let Some(custom_fields) = &secret.bitwarden.fields {
            for custom_field in custom_fields {
                fields.push(custom_field.to_bitwarden_field());
            }
        }

        // Add URL as a field if not used in login and if it's a secure note
        if secret.bitwarden.username.is_none() {
            if let Some(uris) = &secret.bitwarden.uris {
                // For secure notes, add the first URI as a field for backward compatibility
                let first_uri = match uris {
                    crate::models::UriConfig::Single(uri) => Some(uri.clone()),
                    crate::models::UriConfig::Multiple(entries) if !entries.is_empty() => {
                        match &entries[0] {
                            crate::models::UriEntry::Simple(uri) => Some(uri.clone()),
                            crate::models::UriEntry::WithMatch { uri, .. } => Some(uri.clone()),
                        }
                    },
                    _ => None,
                };
                
                if let Some(uri) = first_uri {
                    fields.push(BitwardenField {
                        name: Some("url".to_string()),
                        value: Some(uri),
                        field_type: 0,
                        linked_id: None,
                    });
                }
            }
        }

        fields
    }

    fn decrypt_age_file(&self, path: &str) -> Result<String> {
        debug!("Decrypting: {}", path);

        // First ensure the FIDO plugin is available
        let plugin_check = Command::new("age-plugin-fido2-hmac")
            .arg("--version")
            .output();
        
        if plugin_check.is_err() {
            warn!("age-plugin-fido2-hmac not found, FIDO key decryption may fail");
        }

        // Set up environment for age to find plugins
        let mut cmd = Command::new("age");
        cmd.args(&["-d"]);
        
        // Add all identity files that exist
        let mut has_identity = false;
        for identity in &self.age_identities {
            if !identity.is_empty() && std::path::Path::new(identity).exists() {
                cmd.args(&["-i", identity]);
                has_identity = true;
                debug!("Using identity: {}", identity);
            }
        }
        
        cmd.arg(path);
        
        // Set environment to ensure plugins are found
        cmd.env("PATH", std::env::var("PATH").unwrap_or_default());
        
        // For FIDO keys, we need to allow interactive input
        if has_identity {
            info!("Attempting to decrypt {}. Touch your FIDO key when it blinks...", path);
            cmd.stdin(Stdio::inherit());
            cmd.stderr(Stdio::inherit());
        } else {
            warn!("No valid identity files found for decryption");
        }
        
        let output = cmd
            .output()
            .context(format!("Failed to decrypt {}", path))?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            
            // Check if it's a plugin-related error
            if error.contains("plugin") || error.contains("fido") {
                return Err(anyhow::anyhow!(
                    "Failed to decrypt {} - FIDO key issue: {}. Make sure your FIDO key is connected and you touch it when prompted.",
                    path, error
                ));
            }
            
            return Err(anyhow::anyhow!("Failed to decrypt {}: {}", path, error));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

#[derive(Debug)]
enum SyncResult {
    Created,
    Updated,
    Skipped,
}

fn calculate_checksum(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    format!("{:x}", hasher.finalize())
}