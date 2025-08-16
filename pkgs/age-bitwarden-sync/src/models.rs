use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BitwardenConfig {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgeSecret {
    pub hostname: String,
    pub attribute_name: String,
    pub rekey_file: String,
    pub bitwarden: BitwardenConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BitwardenItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub item_type: i32, // 1 = Login, 2 = Secure Note, 3 = Card, 4 = Identity
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<BitwardenLogin>,
    #[serde(rename = "secureNote", skip_serializing_if = "Option::is_none")]
    pub secure_note: Option<BitwardenSecureNote>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<BitwardenField>>,
    #[serde(rename = "folderId", skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub reprompt: i32,
    // Additional fields that may be in responses but we don't use
    #[serde(rename = "organizationId", skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    #[serde(rename = "collectionIds", skip_serializing_if = "Option::is_none")]
    pub collection_ids: Option<Vec<String>>,
    #[serde(rename = "passwordHistory", skip_serializing_if = "Option::is_none")]
    pub password_history: Option<serde_json::Value>,
    #[serde(rename = "revisionDate", skip_serializing_if = "Option::is_none")]
    pub revision_date: Option<String>,
    #[serde(rename = "creationDate", skip_serializing_if = "Option::is_none")]
    pub creation_date: Option<String>,
    #[serde(rename = "deletedDate", skip_serializing_if = "Option::is_none")]
    pub deleted_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BitwardenSecureNote {
    #[serde(rename = "type")]
    pub note_type: i32, // 0 = Generic
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BitwardenLogin {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uris: Option<Vec<BitwardenUri>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub totp: Option<String>,
    #[serde(rename = "passwordRevisionDate", skip_serializing_if = "Option::is_none")]
    pub password_revision_date: Option<String>,
    #[serde(rename = "fido2Credentials", skip_serializing_if = "Option::is_none")]
    pub fido2_credentials: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BitwardenUri {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    pub match_type: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BitwardenField {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(rename = "type")]
    pub field_type: i32, // 0 = Text, 1 = Hidden, 2 = Boolean
    #[serde(rename = "linkedId", skip_serializing_if = "Option::is_none")]
    pub linked_id: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BitwardenFolder {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BitwardenStatus {
    #[serde(default)]
    pub status: String,
    #[serde(rename = "userEmail", skip_serializing_if = "Option::is_none", default)]
    pub user_email: Option<String>,
    #[serde(rename = "userId", skip_serializing_if = "Option::is_none", default)]
    pub user_id: Option<String>,
    #[serde(rename = "serverUrl", skip_serializing_if = "Option::is_none", default)]
    pub server_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NixEvalResult {
    pub secrets: HashMap<String, SecretConfig>,
}

#[derive(Debug, Deserialize)]
pub struct SecretConfig {
    #[serde(rename = "rekeyFile")]
    pub rekey_file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitwarden: Option<BitwardenConfig>,
}