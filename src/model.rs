use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountsFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub accounts: BTreeMap<String, AccountConfig>,
}

impl Default for AccountsFile {
    fn default() -> Self {
        Self {
            version: default_version(),
            accounts: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountConfig {
    pub email: String,
    #[serde(default)]
    pub default: bool,
    /// Zoho datacenter suffix: "com", "eu", "in", "com.au", "jp", "zohocloud.ca", "sa", "uk"
    pub datacenter: String,
    /// Zoho account ID (numeric, from Zoho admin panel)
    pub account_id: String,
    /// Zoho User ID (ZUID) — used for WorkDrive API
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zuid: Option<String>,
    /// Zoho organization ID (for WorkDrive team operations)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    /// OAuth2 client ID from Zoho API Console
    pub client_id: String,
    /// OAuth2 client secret (plain string, optional for PKCE-only flows)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// Credential store reference
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

pub struct NewAccountInput {
    pub email: String,
    pub default: bool,
    pub datacenter: String,
    pub account_id: String,
    pub org_id: Option<String>,
    pub client_id: String,
    pub client_secret: Option<String>,
}

impl AccountConfig {
    pub fn new(input: NewAccountInput) -> Self {
        Self {
            email: input.email,
            default: input.default,
            datacenter: input.datacenter,
            account_id: input.account_id,
            zuid: None,
            org_id: input.org_id,
            client_id: input.client_id,
            client_secret: input.client_secret,
            credential_ref: Some("store:oauth".to_string()),
        }
    }

    /// OAuth2 authorization base URL for this account's datacenter.
    pub fn auth_base_url(&self) -> String {
        datacenter_auth_url(&self.datacenter)
    }

    /// Zoho Mail REST API base URL.
    pub fn mail_api_url(&self) -> String {
        format!("https://mail.zoho.{}", self.datacenter)
    }

    /// Zoho Calendar REST API base URL.
    pub fn calendar_api_url(&self) -> String {
        format!("https://calendar.zoho.{}", self.datacenter)
    }

    /// Zoho WorkDrive API base URL.
    pub fn drive_api_url(&self) -> String {
        format!("https://www.zohoapis.{}/workdrive", self.datacenter)
    }

    /// Zoho WorkDrive upload URL (dedicated upload host).
    pub fn drive_upload_url(&self) -> String {
        format!(
            "https://upload.zoho.{}/workdrive-api/v1/upload",
            self.datacenter
        )
    }

    /// Zoho WorkDrive download URL for a specific file (dedicated download host).
    pub fn drive_download_url(&self, file_id: &str) -> String {
        format!(
            "https://download.zoho.{}/v1/workdrive/download/{}",
            self.datacenter, file_id
        )
    }
}

/// Valid Zoho datacenter suffixes.
pub const VALID_DATACENTERS: &[&str] = &[
    "com",
    "eu",
    "in",
    "com.au",
    "jp",
    "zohocloud.ca",
    "sa",
    "uk",
];

pub fn validate_datacenter(dc: &str) -> bool {
    VALID_DATACENTERS.contains(&dc)
}

pub fn datacenter_auth_url(dc: &str) -> String {
    format!("https://accounts.zoho.{dc}")
}

const fn default_version() -> u32 {
    1
}
