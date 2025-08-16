use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::models::{BitwardenFolder, BitwardenItem, BitwardenStatus};

pub struct BitwardenClient {
    client: Client,
    base_url: String,
    serve_process: Option<Child>,
}

impl BitwardenClient {
    pub async fn new() -> Result<Self> {
        // Start bw serve
        info!("Starting Bitwarden CLI in serve mode...");
        let mut serve_process = Command::new("bw")
            .args(&["serve", "--port", "8087"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to start 'bw serve'")?;

        // Wait for serve to be ready
        let client = Client::new();
        let base_url = "http://localhost:8087".to_string();
        
        for _ in 0..30 {
            if let Ok(response) = client.get(&format!("{}/status", base_url)).send().await {
                if response.status().is_success() {
                    info!("Bitwarden serve is ready");
                    break;
                }
            }
            sleep(Duration::from_millis(500)).await;
        }

        // Check if process is still running
        match serve_process.try_wait() {
            Ok(Some(status)) => {
                return Err(anyhow!("bw serve exited with status: {}", status));
            }
            Ok(None) => {
                // Process is still running
            }
            Err(e) => {
                return Err(anyhow!("Failed to check bw serve status: {}", e));
            }
        }

        Ok(Self {
            client,
            base_url,
            serve_process: Some(serve_process),
        })
    }

    pub async fn status(&self) -> Result<BitwardenStatus> {
        let response = self
            .client
            .get(&format!("{}/status", self.base_url))
            .send()
            .await
            .context("Failed to get status")?;

        // Get the response text first for debugging
        let text = response.text().await
            .context("Failed to get status response text")?;
        
        debug!("Status response: {}", text);
        
        // Handle potential error response or different format
        if text.contains("\"success\":false") || text.contains("\"error\"") {
            // It's an error response, extract the message
            return Err(anyhow!("Bitwarden API error: {}", text));
        }
        
        // For the serve API, the status endpoint might just return a simple object
        // Let's try to parse whatever we get
        if text.trim() == "\"locked\"" || text.trim() == "\"unlocked\"" {
            // Simple string response
            return Ok(BitwardenStatus {
                status: text.trim().trim_matches('"').to_string(),
                user_email: None,
                user_id: None,
                server_url: None,
            });
        }
        
        // Try to parse as full object
        let status: BitwardenStatus = serde_json::from_str(&text)
            .context(format!("Failed to parse status response: {}", text))?;

        Ok(status)
    }

    pub async fn unlock(&self, password: &str) -> Result<()> {
        let response = self
            .client
            .post(&format!("{}/unlock", self.base_url))
            .json(&json!({ "password": password }))
            .send()
            .await
            .context("Failed to unlock vault")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to unlock vault: {}", error_text));
        }

        info!("Vault unlocked successfully");
        Ok(())
    }

    pub async fn sync(&self) -> Result<()> {
        let response = self
            .client
            .post(&format!("{}/sync", self.base_url))
            .send()
            .await
            .context("Failed to sync vault")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to sync vault: {}", error_text));
        }

        info!("Vault synced successfully");
        Ok(())
    }

    pub async fn get_or_create_folder(&self, name: &str) -> Result<String> {
        // List existing folders
        let response = self
            .client
            .get(&format!("{}/list/object/folders", self.base_url))
            .send()
            .await
            .context("Failed to list folders")?;

        // Get response text for debugging
        let text = response.text().await
            .context("Failed to get folders response text")?;
        
        debug!("Folders response: {}", text);
        
        // Parse the response - Bitwarden serve API has nested structure
        let folders: Vec<BitwardenFolder> = if text.contains("\"success\":") {
            // Response from bw serve API: {"success":true,"data":{"object":"list","data":[...]}}
            #[derive(Deserialize)]
            struct ListData {
                object: String,
                data: Vec<BitwardenFolder>,
            }
            #[derive(Deserialize)]
            struct ApiResponse {
                success: bool,
                data: ListData,
            }
            let response: ApiResponse = serde_json::from_str(&text)
                .context(format!("Failed to parse API folders response"))?;
            if !response.success {
                return Err(anyhow!("API returned success: false"));
            }
            response.data.data
        } else if text.starts_with("{\"data\":") {
            // Response is wrapped in simple data object
            #[derive(Deserialize)]
            struct FoldersResponse {
                data: Vec<BitwardenFolder>,
            }
            let wrapped: FoldersResponse = serde_json::from_str(&text)
                .context(format!("Failed to parse wrapped folders response"))?;
            wrapped.data
        } else if text.starts_with("[") {
            // Direct array response
            serde_json::from_str(&text)
                .context(format!("Failed to parse folders array response"))?
        } else {
            // Unknown format
            return Err(anyhow!("Unexpected folders response format"));
        };

        // Check if folder exists
        for folder in folders {
            if folder.name == name {
                if let Some(id) = folder.id {
                    debug!("Found existing folder: {} ({})", name, id);
                    return Ok(id);
                }
            }
        }

        // Create new folder
        info!("Creating new folder: {}", name);
        
        // According to API spec, just send name
        let folder_data = json!({
            "name": name
        });
        
        debug!("Creating folder with data: {}", folder_data);
        
        let response = self
            .client
            .post(&format!("{}/object/folder", self.base_url))
            .json(&folder_data)
            .send()
            .await
            .context("Failed to create folder")?;

        let response_text = response.text().await
            .context("Failed to get create folder response")?;
        
        debug!("Create folder response: {}", response_text);
        
        // Parse the response according to the API spec
        // Expected: {"success":true,"data":{"object":"folder","id":"...","name":"..."}}
        if response_text.contains("\"success\":false") {
            // Extract error message if available
            let error_msg = if let Ok(err_json) = serde_json::from_str::<serde_json::Value>(&response_text) {
                err_json.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or(&response_text)
                    .to_string()
            } else {
                response_text.clone()
            };
            
            warn!("Failed to create folder '{}': {}. Using no folder for now.", name, error_msg);
            warn!("Please create the 'NixOS Secrets' folder manually in Bitwarden");
            // Return empty string to indicate no folder
            return Ok(String::new());
        }
        
        // Parse successful response
        #[derive(Deserialize)]
        struct CreateFolderResponse {
            success: bool,
            data: BitwardenFolder,
        }
        
        let response: CreateFolderResponse = serde_json::from_str(&response_text)
            .context(format!("Failed to parse create folder response: {}", response_text))?;
        
        if !response.success {
            return Err(anyhow!("Folder creation returned success: false"));
        }
        
        response.data.id.ok_or_else(|| anyhow!("Created folder has no ID"))
    }

    pub async fn search_items(&self, search: &str) -> Result<Vec<BitwardenItem>> {
        let response = self
            .client
            .get(&format!("{}/list/object/items", self.base_url))
            .query(&[("search", search)])
            .send()
            .await
            .context("Failed to search items")?;

        // Get response text for debugging
        let text = response.text().await
            .context("Failed to get items response text")?;
        
        debug!("Items response (first 500 chars): {}", &text[..text.len().min(500)]);
        
        // Parse the response - Bitwarden serve API has nested structure
        let items: Vec<BitwardenItem> = if text.contains("\"success\":") {
            // Response from bw serve API: {"success":true,"data":{"object":"list","data":[...]}}
            // Try to parse the outer structure first
            let response_value: serde_json::Value = serde_json::from_str(&text)
                .context("Failed to parse items JSON")?;
            
            // Extract success field
            let success = response_value.get("success")
                .and_then(|v| v.as_bool())
                .ok_or_else(|| anyhow!("Missing success field in response"))?;
            
            if !success {
                return Err(anyhow!("API returned success: false"));
            }
            
            // Extract the items array
            let items_array = response_value
                .get("data")
                .and_then(|d| d.get("data"))
                .ok_or_else(|| anyhow!("Missing data.data in response"))?;
            
            // Try to parse each item individually to find the problematic one
            let mut items = Vec::new();
            if let Some(arr) = items_array.as_array() {
                for (index, item_value) in arr.iter().enumerate() {
                    match serde_json::from_value::<BitwardenItem>(item_value.clone()) {
                        Ok(item) => items.push(item),
                        Err(e) => {
                            // Log the problematic item but continue
                            debug!("Skipping item {} due to parse error: {}", index, e);
                            debug!("Problematic item: {}", serde_json::to_string_pretty(item_value).unwrap_or_default());
                        }
                    }
                }
            }
            
            items
        } else if text.starts_with("{\"data\":") {
            // Response is wrapped in simple data object
            #[derive(Deserialize)]
            struct ItemsResponse {
                data: Vec<BitwardenItem>,
            }
            let wrapped: ItemsResponse = serde_json::from_str(&text)
                .context(format!("Failed to parse wrapped items response"))?;
            wrapped.data
        } else if text.starts_with("[") {
            // Direct array response
            serde_json::from_str(&text)
                .context(format!("Failed to parse items array response"))?
        } else {
            // Unknown format
            return Err(anyhow!("Unexpected items response format"));
        };

        Ok(items)
    }

    pub async fn find_item_by_field(
        &self,
        field_name: &str,
        field_value: &str,
        folder_id: &str,
    ) -> Result<Option<BitwardenItem>> {
        // Get all items in folder
        let items = self.search_items("").await?;
        
        for item in items {
            // Check if item is in the right folder (or no folder if folder_id is empty)
            if !folder_id.is_empty() {
                if item.folder_id.as_ref() != Some(&folder_id.to_string()) {
                    continue;
                }
            } else {
                // If we couldn't create a folder, only match items without a folder
                if item.folder_id.is_some() {
                    continue;
                }
            }

            // Check fields
            if let Some(fields) = &item.fields {
                for field in fields {
                    if field.name.as_deref() == Some(field_name) && field.value.as_deref() == Some(field_value) {
                        return Ok(Some(item));
                    }
                }
            }
        }

        Ok(None)
    }

    pub async fn create_item(&self, item: &BitwardenItem) -> Result<BitwardenItem> {
        let response = self
            .client
            .post(&format!("{}/object/item", self.base_url))
            .json(item)
            .send()
            .await
            .context("Failed to create item")?;

        let response_text = response.text().await
            .context("Failed to get create item response")?;
        
        debug!("Create item response: {}", &response_text[..response_text.len().min(500)]);

        // Parse response with success wrapper
        #[derive(Deserialize)]
        struct CreateItemResponse {
            success: bool,
            data: BitwardenItem,
        }
        
        let response: CreateItemResponse = serde_json::from_str(&response_text)
            .context(format!("Failed to parse create item response"))?;
        
        if !response.success {
            return Err(anyhow!("Failed to create item: success=false"));
        }

        Ok(response.data)
    }

    pub async fn update_item(&self, item: &BitwardenItem) -> Result<BitwardenItem> {
        let item_id = item
            .id
            .as_ref()
            .ok_or_else(|| anyhow!("Item ID is required for update"))?;

        let response = self
            .client
            .put(&format!("{}/object/item/{}", self.base_url, item_id))
            .json(item)
            .send()
            .await
            .context("Failed to update item")?;

        let response_text = response.text().await
            .context("Failed to get update item response")?;
        
        debug!("Update item response: {}", &response_text[..response_text.len().min(500)]);

        // Parse response with success wrapper
        #[derive(Deserialize)]
        struct UpdateItemResponse {
            success: bool,
            data: BitwardenItem,
        }
        
        let response: UpdateItemResponse = serde_json::from_str(&response_text)
            .context(format!("Failed to parse update item response"))?;
        
        if !response.success {
            return Err(anyhow!("Failed to update item: success=false"));
        }

        Ok(response.data)
    }

    pub async fn lock(&self) -> Result<()> {
        let response = self
            .client
            .post(&format!("{}/lock", self.base_url))
            .send()
            .await
            .context("Failed to lock vault")?;

        if !response.status().is_success() {
            warn!("Failed to lock vault");
        }

        Ok(())
    }
}

impl Drop for BitwardenClient {
    fn drop(&mut self) {
        if let Some(mut process) = self.serve_process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}