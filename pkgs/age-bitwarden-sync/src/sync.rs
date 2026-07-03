use age::secrecy::SecretString;
use anyhow::{Context, Result};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::sync::{Arc, Mutex};
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
    /// FIDO PIN cached for the duration of a run. It is prompted for lazily (only
    /// when the plugin actually asks) and reused across every secret, so the user
    /// types it at most once per run and never when no decryption is needed.
    pin: Arc<Mutex<Option<String>>>,
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
            pin: Arc::new(Mutex::new(None)),
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

        // Checksum the *encrypted* bytes so we can detect changes without decrypting
        // (and therefore without triggering a FIDO PIN/touch prompt).
        let encrypted = std::fs::read(&age_file_path)
            .with_context(|| format!("Failed to read {}", age_file_path))?;
        let source_checksum = calculate_checksum_bytes(&encrypted);

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

        // If the encrypted source is unchanged, skip without decrypting (no FIDO prompt).
        if let Some(existing) = &existing_item {
            if let Some(fields) = &existing.fields {
                for field in fields {
                    if field.name.as_deref() == Some("age_source_checksum")
                        && field.value.as_deref() == Some(&source_checksum)
                    {
                        return Ok(SyncResult::Skipped);
                    }
                }
            }
        }

        // Source changed (or new item): decrypt now. This is the only path that
        // touches the FIDO key; the PIN is prompted once per run and cached.
        let secret_value = self.decrypt_age_file(&age_file_path)?;
        let checksum = calculate_checksum(&secret_value);

        let item_name = format!("{} - {}", secret.bitwarden.name, secret.hostname);

        if let Some(mut existing) = existing_item {
            // Update existing item (source changed, so always refresh)
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
            existing.fields = Some(self.build_fields(secret, &age_path, &checksum, &source_checksum));

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
                fields: Some(self.build_fields(secret, &age_path, &checksum, &source_checksum)),
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

    fn build_fields(
        &self,
        secret: &AgeSecret,
        age_path: &str,
        checksum: &str,
        source_checksum: &str,
    ) -> Vec<BitwardenField> {
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
                // Checksum of the encrypted source file; used to skip unchanged
                // secrets without decrypting them (avoids a FIDO prompt).
                name: Some("age_source_checksum".to_string()),
                value: Some(source_checksum.to_string()),
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
                    crate::models::UriConfig::SingleWithMatch { uri, .. } => Some(uri.clone()),
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

        // Parse the plugin identities (e.g. AGE-PLUGIN-FIDO2-HMAC-...) out of the
        // configured identity files.
        let mut plugin_identities: Vec<age::plugin::Identity> = Vec::new();
        for id_path in &self.age_identities {
            if id_path.is_empty() || !std::path::Path::new(id_path).exists() {
                continue;
            }
            let content = std::fs::read_to_string(id_path)
                .with_context(|| format!("Failed to read identity file {}", id_path))?;
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                match line.parse::<age::plugin::Identity>() {
                    Ok(identity) => plugin_identities.push(identity),
                    Err(e) => debug!("Skipping non-plugin identity in {}: {}", id_path, e),
                }
            }
        }
        if plugin_identities.is_empty() {
            return Err(anyhow::anyhow!(
                "No plugin identities found for decryption of {}",
                path
            ));
        }

        // Decrypt in-process via the age crate. It speaks the age-plugin protocol
        // to the plugin binary, and routes the plugin's PIN request to our
        // PinCallbacks, which prompts once and caches — so the PIN is entered at
        // most once per run even across many secrets. The physical touch still
        // happens per secret (the plugin performs a fresh FIDO2 assertion).
        let callbacks = PinCallbacks {
            pin: Arc::clone(&self.pin),
        };

        let mut plugin_names: Vec<String> =
            plugin_identities.iter().map(|i| i.plugin().to_string()).collect();
        plugin_names.sort();
        plugin_names.dedup();

        let mut plugins: Vec<age::plugin::IdentityPluginV1<PinCallbacks>> = Vec::new();
        for name in &plugin_names {
            let plugin =
                age::plugin::IdentityPluginV1::new(name, &plugin_identities, callbacks.clone())
                    .with_context(|| format!("Failed to initialise age plugin '{}'", name))?;
            plugins.push(plugin);
        }

        info!("Decrypting {} (touch your FIDO key when prompted)...", path);

        let file =
            std::fs::File::open(path).with_context(|| format!("Failed to open {}", path))?;
        let decryptor = age::Decryptor::new(std::io::BufReader::new(file))
            .with_context(|| format!("Failed to parse age header of {}", path))?;
        if decryptor.is_scrypt() {
            return Err(anyhow::anyhow!(
                "{} is passphrase-encrypted; expected identity-encrypted",
                path
            ));
        }

        let mut reader = decryptor
            .decrypt(plugins.iter().map(|p| p as &dyn age::Identity))
            .with_context(|| {
                format!(
                    "Failed to decrypt {} — check your FIDO key is connected, the PIN is correct, \
                     and you touch it when prompted",
                    path
                )
            })?;

        // Reaching here means the plugin completed its FIDO2 assertion, i.e. the
        // touch was accepted. Confirm it immediately.
        eprintln!("  ✅ Touch confirmed.");

        let mut plaintext = String::new();
        reader
            .read_to_string(&mut plaintext)
            .with_context(|| format!("Failed to read plaintext of {}", path))?;
        Ok(plaintext)
    }
}

/// age plugin callbacks that answer the plugin's PIN request from a per-run
/// cache, so the FIDO PIN is entered at most once even across many secrets.
#[derive(Clone)]
struct PinCallbacks {
    pin: Arc<Mutex<Option<String>>>,
}

impl age::Callbacks for PinCallbacks {
    fn display_message(&self, message: &str) {
        // Surfaces plugin prompts such as "Please touch your token...".
        eprintln!("{}", message.trim_end());
    }

    fn confirm(&self, message: &str, yes_string: &str, _no_string: Option<&str>) -> Option<bool> {
        // No confirmations are expected during decryption; default to "yes".
        debug!("Auto-confirming plugin prompt: {} [{}]", message, yes_string);
        Some(true)
    }

    fn request_public_string(&self, _description: &str) -> Option<String> {
        None
    }

    fn request_passphrase(&self, description: &str) -> Option<SecretString> {
        let mut guard = self.pin.lock().unwrap();
        if guard.is_none() {
            match rpassword::prompt_password(format!("{} ", description.trim())) {
                Ok(p) => *guard = Some(p),
                Err(e) => {
                    warn!("Failed to read PIN: {}", e);
                    return None;
                }
            }
        }
        guard.clone().map(SecretString::from)
    }
}

#[derive(Debug)]
enum SyncResult {
    Created,
    Updated,
    Skipped,
}

fn calculate_checksum(data: &str) -> String {
    calculate_checksum_bytes(data.as_bytes())
}

fn calculate_checksum_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}