use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::blocking::multipart;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use url::Url;

use crate::error::{Result, ZocliError};

// ---------------------------------------------------------------------------
// Public data structures
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct DriveTeam {
    pub id: String,
    pub name: String,
    pub storage_limit: u64,
    pub storage_used: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DriveFile {
    pub id: String,
    pub name: String,
    pub file_type: String,
    pub size: u64,
    pub modified_time: String,
    pub created_time: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct UploadedFile {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DownloadedFile {
    pub path: PathBuf,
    pub size: u64,
}

// ---------------------------------------------------------------------------
// Public API functions
// ---------------------------------------------------------------------------

/// List all WorkDrive teams (workspaces) visible to the given Zoho account.
///
/// `base_url` — e.g. `https://www.zohoapis.eu/workdrive`
/// `account_id` — the Zoho numeric user / account ID.
pub fn list_teams(base_url: &str, account_id: &str, access_token: &str) -> Result<Vec<DriveTeam>> {
    let client = build_http_client()?;
    let endpoint = build_url(base_url, &format!("/api/v1/users/{account_id}/teams"))?;

    let response = client
        .get(endpoint)
        .header("Authorization", format!("Zoho-oauthtoken {access_token}"))
        .header("Accept", "application/vnd.api+json")
        .send()?;

    let status = response.status();
    let body = response.text()?;

    if !status.is_success() {
        return Err(api_error(status, &body, "list teams"));
    }

    let raw: JsonApiListResponse<RawTeamAttributes> = serde_json::from_str(&body)
        .map_err(|err| ZocliError::Serialization(format!("invalid WorkDrive response: {err}")))?;

    Ok(raw
        .data
        .into_iter()
        .map(|entry| DriveTeam {
            id: entry.id,
            name: entry.attributes.name,
            storage_limit: entry.attributes.storage_limit.unwrap_or(0),
            storage_used: entry.attributes.storage_used.unwrap_or(0),
        })
        .collect())
}

/// List files and folders inside a WorkDrive folder.
///
/// `base_url` — e.g. `https://www.zohoapis.eu/workdrive`
/// `folder_id` — the WorkDrive folder resource ID.
pub fn list_files(
    base_url: &str,
    access_token: &str,
    folder_id: &str,
    limit: usize,
    offset: u64,
) -> Result<Vec<DriveFile>> {
    if limit == 0 {
        return Err(ZocliError::Validation(
            "list_files: limit must be greater than zero".to_string(),
        ));
    }
    if folder_id.trim().is_empty() {
        return Err(ZocliError::Validation(
            "list_files: folder_id must not be empty".to_string(),
        ));
    }

    let client = build_http_client()?;
    let endpoint = build_url(base_url, &format!("/api/v1/files/{folder_id}/files"))?;

    let response = client
        .get(endpoint)
        .header("Authorization", format!("Zoho-oauthtoken {access_token}"))
        .header("Accept", "application/vnd.api+json")
        .query(&[
            ("page[limit]", limit.to_string()),
            ("page[offset]", offset.to_string()),
        ])
        .send()?;

    let status = response.status();
    let body = response.text()?;

    if !status.is_success() {
        return Err(api_error(status, &body, "list files"));
    }

    let raw: JsonApiListResponse<RawFileAttributes> = serde_json::from_str(&body)
        .map_err(|err| ZocliError::Serialization(format!("invalid WorkDrive response: {err}")))?;

    Ok(raw
        .data
        .into_iter()
        .map(|entry| DriveFile {
            id: entry.id,
            name: entry.attributes.name,
            file_type: entry.attributes.r#type.unwrap_or_default(),
            size: entry.attributes.size.unwrap_or(0),
            modified_time: entry.attributes.modified_time.unwrap_or_default(),
            created_time: entry.attributes.created_time.unwrap_or_default(),
        })
        .collect())
}

/// Upload a local file to a WorkDrive folder.
///
/// `upload_url` — e.g. `https://upload.zoho.eu/workdrive-api/v1/upload`
///                (use `AccountConfig::drive_upload_url()`).
/// `folder_id` — the WorkDrive folder resource ID.
/// `source` — path to the local file.
/// `overwrite` — when true, overwrites an existing file with the same name.
pub fn upload_file(
    upload_url: &str,
    access_token: &str,
    folder_id: &str,
    source: &Path,
    overwrite: bool,
) -> Result<UploadedFile> {
    validate_upload_source(source)?;

    let file_name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| {
            ZocliError::Validation(format!(
                "upload_file: cannot extract filename from path: {}",
                source.display()
            ))
        })?
        .to_string();

    let client = build_upload_client()?;
    let mut endpoint = Url::parse(upload_url)
        .map_err(|err| ZocliError::Config(format!("invalid WorkDrive upload URL: {err}")))?;

    if overwrite {
        endpoint
            .query_pairs_mut()
            .append_pair("override-name-exist", "true");
    }

    let file_bytes = fs::read(source)
        .map_err(|err| ZocliError::Io(format!("failed to read {}: {err}", source.display())))?;

    let file_part = multipart::Part::bytes(file_bytes)
        .file_name(file_name.clone())
        .mime_str(
            mime_guess::from_path(source)
                .first_raw()
                .unwrap_or("application/octet-stream"),
        )
        .map_err(|err| ZocliError::Network(format!("multipart MIME error: {err}")))?;

    let form = multipart::Form::new()
        .text("parent_id", folder_id.to_string())
        .text("filename", file_name.clone())
        .part("content", file_part);

    let response = client
        .post(endpoint)
        .header("Authorization", format!("Zoho-oauthtoken {access_token}"))
        .header("x-filename", &file_name)
        .multipart(form)
        .send()?;

    let status = response.status();
    let body = response.text()?;

    if !status.is_success() {
        return Err(api_error(status, &body, "upload file"));
    }

    let raw: JsonApiListResponse<RawUploadAttributes> = serde_json::from_str(&body)
        .map_err(|err| ZocliError::Serialization(format!("invalid WorkDrive response: {err}")))?;

    let entry =
        raw.data.into_iter().next().ok_or_else(|| {
            ZocliError::Api("WorkDrive upload returned empty data array".to_string())
        })?;

    Ok(UploadedFile {
        id: entry.id,
        name: entry.attributes.name,
    })
}

/// Download a WorkDrive file to a local path.
///
/// `download_url` — e.g. `https://download.zoho.eu/v1/workdrive/download/{fileId}`
///                  (use `AccountConfig::drive_download_url(file_id)`).
/// `access_token` — OAuth access token.
/// `output` — local destination path.
/// `force` — when true, overwrites the output if it already exists.
pub fn download_file(
    download_url: &str,
    access_token: &str,
    output: &Path,
    force: bool,
) -> Result<DownloadedFile> {
    if output.exists() && !force {
        return Err(ZocliError::OutputExists(output.display().to_string()));
    }

    if output.is_dir() {
        return Err(ZocliError::UnsupportedOperation(format!(
            "output path points to a directory: {}",
            output.display()
        )));
    }

    let temp_dir = match output.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            fs::create_dir_all(parent)?;
            parent.to_path_buf()
        }
        _ => std::env::current_dir()?,
    };

    let client = build_download_client()?;
    let endpoint = Url::parse(download_url)
        .map_err(|err| ZocliError::Config(format!("invalid WorkDrive download URL: {err}")))?;

    let mut response = client
        .get(endpoint)
        .header("Authorization", format!("Zoho-oauthtoken {access_token}"))
        .send()?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(api_error(status, &body, "download file"));
    }

    let mut temp_file = NamedTempFile::new_in(&temp_dir)?;
    let mut bytes_written = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        let read = response.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        temp_file.write_all(&buffer[..read])?;
        bytes_written += read as u64;
    }

    temp_file.as_file_mut().sync_all()?;

    if force && output.exists() {
        fs::remove_file(output)?;
    }

    temp_file
        .persist(output)
        .map_err(|err| ZocliError::Io(err.error.to_string()))?;

    Ok(DownloadedFile {
        path: output.to_path_buf(),
        size: bytes_written,
    })
}

// ---------------------------------------------------------------------------
// Internal: JSON:API response shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct JsonApiListResponse<A> {
    data: Vec<JsonApiResource<A>>,
}

#[derive(Debug, Deserialize)]
struct JsonApiResource<A> {
    id: String,
    #[allow(dead_code)]
    #[serde(default)]
    r#type: Option<String>,
    attributes: A,
}

// -- Teams --

#[derive(Debug, Deserialize)]
struct RawTeamAttributes {
    name: String,
    #[serde(default)]
    storage_limit: Option<u64>,
    #[serde(default)]
    storage_used: Option<u64>,
}

// -- Files / Folders --

#[derive(Debug, Deserialize)]
struct RawFileAttributes {
    name: String,
    #[serde(default, rename = "type")]
    r#type: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    modified_time: Option<String>,
    #[serde(default)]
    created_time: Option<String>,
}

// -- Upload response --

#[derive(Debug, Deserialize)]
struct RawUploadAttributes {
    name: String,
}

// ---------------------------------------------------------------------------
// Internal: HTTP clients
// ---------------------------------------------------------------------------

fn build_http_client() -> Result<Client> {
    Client::builder()
        .user_agent(format!("zocli/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(Into::into)
}

/// Upload client: longer timeout, no default body timeout.
fn build_upload_client() -> Result<Client> {
    Client::builder()
        .user_agent(format!("zocli/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(Into::into)
}

/// Download client: longer timeout for large files.
fn build_download_client() -> Result<Client> {
    Client::builder()
        .user_agent(format!("zocli/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// Internal: URL construction
// ---------------------------------------------------------------------------

fn build_url(base_url: &str, path: &str) -> Result<Url> {
    let base_with_slash = if base_url.ends_with('/') {
        base_url.to_string()
    } else {
        format!("{base_url}/")
    };
    let base = Url::parse(&base_with_slash)
        .map_err(|err| ZocliError::Config(format!("invalid WorkDrive base URL: {err}")))?;
    let relative_path = path.strip_prefix('/').unwrap_or(path);
    base.join(relative_path)
        .map_err(|err| ZocliError::Config(format!("invalid WorkDrive endpoint: {err}")))
}

// ---------------------------------------------------------------------------
// Internal: error handling
// ---------------------------------------------------------------------------

/// Build a structured API error from a non-success HTTP response.
fn api_error(status: StatusCode, body: &str, action: &str) -> ZocliError {
    // Zoho WorkDrive error responses can vary.  Try to extract a JSON error
    // structure; fall back to the raw body if parsing fails.
    let detail = match serde_json::from_str::<serde_json::Value>(body) {
        Ok(value) => {
            // Zoho errors are sometimes {"errors":[{"title":"..."}]} or
            // {"data":null,"errors":[...]} or {"error":{"code":...,"message":...}}.
            if let Some(errors) = value.get("errors").and_then(|v| v.as_array()) {
                errors
                    .iter()
                    .filter_map(|e| {
                        e.get("title")
                            .or_else(|| e.get("message"))
                            .or_else(|| e.get("detail"))
                            .and_then(|v| v.as_str())
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            } else if let Some(msg) = value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
            {
                msg.to_string()
            } else {
                body.chars().take(500).collect::<String>()
            }
        }
        Err(_) => body.chars().take(500).collect::<String>(),
    };

    ZocliError::Api(format!(
        "Zoho WorkDrive {action} failed with status {}: {detail}",
        status.as_u16()
    ))
}

// ---------------------------------------------------------------------------
// Internal: upload validation
// ---------------------------------------------------------------------------

fn validate_upload_source(path: &Path) -> Result<()> {
    let metadata = fs::metadata(path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            ZocliError::Validation(format!(
                "upload_file: source file not found: {}",
                path.display()
            ))
        } else {
            ZocliError::Io(err.to_string())
        }
    })?;

    if !metadata.is_file() {
        return Err(ZocliError::Validation(format!(
            "upload_file: path must point to a file: {}",
            path.display()
        )));
    }

    if metadata.len() == 0 {
        return Err(ZocliError::Validation(
            "upload_file: file must not be empty".to_string(),
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_url_basic() {
        let url = build_url("https://www.zohoapis.eu/workdrive", "/api/v1/teams/abc123").unwrap();
        assert_eq!(
            url.as_str(),
            "https://www.zohoapis.eu/workdrive/api/v1/teams/abc123"
        );
    }

    #[test]
    fn build_url_trailing_slash() {
        let url = build_url("https://www.zohoapis.eu/workdrive/", "/api/v1/teams/abc123").unwrap();
        assert_eq!(
            url.as_str(),
            "https://www.zohoapis.eu/workdrive/api/v1/teams/abc123"
        );
    }

    #[test]
    fn build_url_invalid_base() {
        let result = build_url("not-a-url", "/api/v1/teams/abc");
        assert!(result.is_err());
    }

    #[test]
    fn api_error_with_json_errors_array() {
        let body = r#"{"errors":[{"title":"Not Found","detail":"Resource does not exist"}]}"#;
        let err = api_error(StatusCode::NOT_FOUND, body, "list files");
        let msg = err.to_string();
        assert!(msg.contains("404"), "should include status code");
        assert!(msg.contains("Not Found"), "should include error title");
    }

    #[test]
    fn api_error_with_plain_body() {
        let body = "Internal Server Error";
        let err = api_error(StatusCode::INTERNAL_SERVER_ERROR, body, "upload file");
        let msg = err.to_string();
        assert!(msg.contains("500"));
        assert!(msg.contains("Internal Server Error"));
    }

    #[test]
    fn api_error_with_nested_error_object() {
        let body = r#"{"error":{"code":"INVALID_TOKEN","message":"Token expired"}}"#;
        let err = api_error(StatusCode::UNAUTHORIZED, body, "download file");
        let msg = err.to_string();
        assert!(msg.contains("401"));
        assert!(msg.contains("Token expired"));
    }

    #[test]
    fn list_files_rejects_zero_limit() {
        let result = list_files("https://example.com/workdrive", "token", "folder1", 0, 0);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("limit"));
    }

    #[test]
    fn list_files_rejects_empty_folder_id() {
        let result = list_files("https://example.com/workdrive", "token", "  ", 10, 0);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("folder_id"));
    }

    #[test]
    fn download_file_rejects_existing_output_without_force() {
        // Use a path that actually exists on every system.
        let existing = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let result = download_file(
            "https://download.zoho.eu/v1/workdrive/download/file123",
            "token",
            &existing,
            false,
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Cargo.toml") || msg.contains("exists"));
    }

    #[test]
    fn validate_upload_source_rejects_missing_file() {
        let result = validate_upload_source(Path::new("/nonexistent/file.txt"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not found"));
    }

    #[test]
    fn validate_upload_source_rejects_directory() {
        let result = validate_upload_source(Path::new(env!("CARGO_MANIFEST_DIR")));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("file"));
    }

    #[test]
    fn deserialize_team_list_response() {
        let json = r#"{
            "data": [
                {
                    "id": "team1",
                    "type": "team",
                    "attributes": {
                        "name": "Engineering",
                        "storage_limit": 107374182400,
                        "storage_used": 53687091200
                    }
                },
                {
                    "id": "team2",
                    "type": "team",
                    "attributes": {
                        "name": "Design",
                        "storage_limit": 53687091200
                    }
                }
            ]
        }"#;

        let parsed: JsonApiListResponse<RawTeamAttributes> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.data.len(), 2);
        assert_eq!(parsed.data[0].id, "team1");
        assert_eq!(parsed.data[0].attributes.name, "Engineering");
        assert_eq!(parsed.data[0].attributes.storage_limit, Some(107374182400));
        assert_eq!(parsed.data[0].attributes.storage_used, Some(53687091200));
        assert_eq!(parsed.data[1].attributes.storage_used, None);
    }

    #[test]
    fn deserialize_file_list_response() {
        let json = r#"{
            "data": [
                {
                    "id": "file1",
                    "type": "files",
                    "attributes": {
                        "name": "report.pdf",
                        "type": "file",
                        "size": 1048576,
                        "modified_time": "2025-01-15T10:30:00Z",
                        "created_time": "2025-01-10T08:00:00Z"
                    }
                },
                {
                    "id": "folder1",
                    "type": "files",
                    "attributes": {
                        "name": "Documents",
                        "type": "folder"
                    }
                }
            ]
        }"#;

        let parsed: JsonApiListResponse<RawFileAttributes> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.data.len(), 2);
        assert_eq!(parsed.data[0].attributes.name, "report.pdf");
        assert_eq!(parsed.data[0].attributes.r#type.as_deref(), Some("file"));
        assert_eq!(parsed.data[0].attributes.size, Some(1048576));
        assert_eq!(parsed.data[1].attributes.name, "Documents");
        assert_eq!(parsed.data[1].attributes.r#type.as_deref(), Some("folder"));
        assert_eq!(parsed.data[1].attributes.size, None);
    }

    #[test]
    fn deserialize_upload_response() {
        let json = r#"{
            "data": [
                {
                    "id": "uploaded123",
                    "type": "files",
                    "attributes": {
                        "name": "photo.jpg"
                    }
                }
            ]
        }"#;

        let parsed: JsonApiListResponse<RawUploadAttributes> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.data.len(), 1);
        assert_eq!(parsed.data[0].id, "uploaded123");
        assert_eq!(parsed.data[0].attributes.name, "photo.jpg");
    }
}
