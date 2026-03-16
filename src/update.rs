use std::env;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use reqwest::Url;
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tar::Archive;
use tempfile::tempdir;
use zip::ZipArchive;

use crate::cli::OutputFormat;
use crate::error::{Result, ZocliError};
use crate::output::RenderedOutput;

const REPO_SLUG: &str = "NextStat/zocli";
const DEFAULT_VERSION: &str = "latest";
const UPDATE_BASE_URL_ENV: &str = "ZOCLI_UPDATE_BASE_URL";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArchiveKind {
    TarGz,
    Zip,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReleaseAsset {
    archive_kind: ArchiveKind,
    binary_name: &'static str,
    file_name: &'static str,
    target: &'static str,
}

#[derive(Debug)]
struct ReleasePlan {
    asset: ReleaseAsset,
    base_url: String,
    requested_version: String,
    resolved_version: Option<String>,
    checksum: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UpdateStatusReport {
    pub operation: String,
    pub status: String,
    pub current_version: String,
    pub target_version: String,
    pub requested_version: String,
    pub asset: String,
    pub target: String,
    pub base_url: String,
}

pub fn execute_update(
    format: OutputFormat,
    version: &str,
    check: bool,
    base_url: Option<&str>,
) -> Result<RenderedOutput> {
    let (current_version, plan) = resolve_release_plan(version, base_url)?;

    if check {
        let report = build_update_report("update.check", &current_version, &plan, None);
        return render_output(format, &report);
    }

    if !has_newer_release(&current_version, plan.resolved_version.as_deref()) {
        let report = build_update_report(
            "update.apply",
            &current_version,
            &plan,
            Some("already_up_to_date"),
        );
        return render_output(format, &report);
    }

    let client = release_client()?;
    let archive_bytes = download_release_archive(&client, &plan)?;
    verify_checksum(&archive_bytes, &plan.checksum, plan.asset.file_name)?;
    let staged_binary = stage_release_binary(&archive_bytes, &plan.asset)?;
    replace_current_binary(&staged_binary)?;

    let report = build_update_report("update.apply", &current_version, &plan, Some("updated"));
    render_output(format, &report)
}

pub fn check_for_update(version: Option<&str>) -> Result<UpdateStatusReport> {
    let requested_version = version.unwrap_or(DEFAULT_VERSION);
    let (current_version, plan) = resolve_release_plan(requested_version, None)?;
    Ok(build_update_report(
        "update.check",
        &current_version,
        &plan,
        None,
    ))
}

fn release_client() -> Result<Client> {
    Client::builder()
        .user_agent(format!("zocli/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(ZocliError::from)
}

fn release_probe_client() -> Result<Client> {
    Client::builder()
        .user_agent(format!("zocli/{}", env!("CARGO_PKG_VERSION")))
        .redirect(Policy::none())
        .build()
        .map_err(ZocliError::from)
}

fn normalize_version(version: &str) -> String {
    let trimmed = version.trim();
    if trimmed.is_empty() || trimmed == DEFAULT_VERSION {
        DEFAULT_VERSION.to_string()
    } else if trimmed.starts_with('v') {
        trimmed.to_string()
    } else {
        format!("v{trimmed}")
    }
}

fn configured_base_url_override(override_base_url: Option<&str>) -> Option<String> {
    if let Some(base_url) = override_base_url {
        return Some(base_url.trim_end_matches('/').to_string());
    }
    env::var(UPDATE_BASE_URL_ENV)
        .ok()
        .map(|value| value.trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn release_base_url(version: &str, override_base_url: Option<&str>) -> String {
    if let Some(base_url) = configured_base_url_override(override_base_url) {
        return base_url.trim_end_matches('/').to_string();
    }

    if version == DEFAULT_VERSION {
        format!("https://github.com/{REPO_SLUG}/releases/latest/download")
    } else {
        format!("https://github.com/{REPO_SLUG}/releases/download/{version}")
    }
}

fn current_release_asset() -> Result<ReleaseAsset> {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => Ok(ReleaseAsset {
            archive_kind: ArchiveKind::TarGz,
            binary_name: "zocli",
            file_name: "zocli-aarch64-apple-darwin.tar.gz",
            target: "aarch64-apple-darwin",
        }),
        ("macos", "x86_64") => Ok(ReleaseAsset {
            archive_kind: ArchiveKind::TarGz,
            binary_name: "zocli",
            file_name: "zocli-x86_64-apple-darwin.tar.gz",
            target: "x86_64-apple-darwin",
        }),
        ("linux", "aarch64") => Ok(ReleaseAsset {
            archive_kind: ArchiveKind::TarGz,
            binary_name: "zocli",
            file_name: "zocli-aarch64-unknown-linux-gnu.tar.gz",
            target: "aarch64-unknown-linux-gnu",
        }),
        ("linux", "x86_64") => Ok(ReleaseAsset {
            archive_kind: ArchiveKind::TarGz,
            binary_name: "zocli",
            file_name: "zocli-x86_64-unknown-linux-gnu.tar.gz",
            target: "x86_64-unknown-linux-gnu",
        }),
        ("windows", "x86_64") => Ok(ReleaseAsset {
            archive_kind: ArchiveKind::Zip,
            binary_name: "zocli.exe",
            file_name: "zocli-x86_64-pc-windows-msvc.zip",
            target: "x86_64-pc-windows-msvc",
        }),
        (os, arch) => Err(ZocliError::UnsupportedOperation(format!(
            "auto-update is not published for target {arch}-{os}"
        ))),
    }
}

fn fetch_checksums(client: &Client, base_url: &str) -> Result<(String, Option<String>)> {
    let requested_url = format!("{base_url}/SHA256SUMS");
    let probed_version = probe_latest_version(&requested_url)?;
    let response = client
        .get(&requested_url)
        .send()
        .map_err(ZocliError::from)?;
    let final_url = response.url().clone();
    let response = response.error_for_status().map_err(ZocliError::from)?;
    let body = response.text().map_err(ZocliError::from)?;
    Ok((
        body,
        version_from_download_url(&final_url).or(probed_version),
    ))
}

fn probe_latest_version(url: &str) -> Result<Option<String>> {
    if !url.contains("/releases/latest/download/") {
        return Ok(None);
    }

    let response = release_probe_client()?
        .get(url)
        .send()
        .map_err(ZocliError::from)?;
    if !response.status().is_redirection() {
        return Ok(version_from_download_url(response.url()));
    }

    let Some(location) = response.headers().get(reqwest::header::LOCATION) else {
        return Ok(None);
    };
    let location = location.to_str().map_err(|err| {
        ZocliError::Io(format!("failed to decode release redirect location: {err}"))
    })?;
    let redirected_url = response.url().join(location).map_err(|err| {
        ZocliError::Io(format!("failed to resolve release redirect location {location}: {err}"))
    })?;
    Ok(version_from_download_url(&redirected_url))
}

fn version_from_download_url(url: &Url) -> Option<String> {
    let segments = url.path_segments()?.collect::<Vec<_>>();
    let download_index = segments.iter().position(|segment| *segment == "download")?;
    let tag = segments.get(download_index + 1)?.strip_prefix('v')?;
    Some(tag.to_string())
}

fn checksum_for_asset(checksums: &str, asset_name: &str) -> Result<String> {
    for line in checksums.lines() {
        let mut parts = line.split_whitespace();
        let Some(sum) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };
        if name == asset_name {
            return Ok(sum.to_string());
        }
    }

    Err(ZocliError::Integrity(format!(
        "missing checksum for release asset {asset_name}"
    )))
}

fn download_release_archive(client: &Client, plan: &ReleasePlan) -> Result<Vec<u8>> {
    let response = client
        .get(format!("{}/{}", plan.base_url, plan.asset.file_name))
        .send()
        .map_err(ZocliError::from)?;
    let response = response.error_for_status().map_err(ZocliError::from)?;
    response
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(ZocliError::from)
}

fn resolve_release_plan(version: &str, base_url: Option<&str>) -> Result<(String, ReleasePlan)> {
    let client = release_client()?;
    let requested_version = normalize_version(version);
    let asset = current_release_asset()?;
    let base_url = release_base_url(&requested_version, base_url);
    let (checksums, resolved_version) = fetch_checksums(&client, &base_url)?;
    let checksum = checksum_for_asset(&checksums, asset.file_name)?;
    let current_version = env!("CARGO_PKG_VERSION").to_string();

    Ok((
        current_version,
        ReleasePlan {
            asset,
            base_url,
            requested_version,
            resolved_version,
            checksum,
        },
    ))
}

fn verify_checksum(bytes: &[u8], expected: &str, asset_name: &str) -> Result<()> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual == expected {
        Ok(())
    } else {
        Err(ZocliError::Integrity(format!(
            "checksum mismatch for {asset_name}: expected {expected}, got {actual}"
        )))
    }
}

fn stage_release_binary(bytes: &[u8], asset: &ReleaseAsset) -> Result<PathBuf> {
    let tmp_dir = tempdir().map_err(|err| {
        ZocliError::Io(format!(
            "failed to create temporary update directory: {err}"
        ))
    })?;
    let extract_root = tmp_dir.path().join("extract");
    fs::create_dir_all(&extract_root)
        .map_err(|err| ZocliError::Io(format!("failed to create extraction directory: {err}")))?;

    match asset.archive_kind {
        ArchiveKind::TarGz => extract_tar_gz(bytes, &extract_root)?,
        ArchiveKind::Zip => extract_zip(bytes, &extract_root)?,
    }

    let extracted_binary = find_binary(&extract_root, asset.binary_name)?;
    let staged_binary = tmp_dir.path().join(asset.binary_name);
    fs::copy(&extracted_binary, &staged_binary).map_err(|err| {
        ZocliError::Io(format!(
            "failed to stage extracted binary {}: {err}",
            extracted_binary.display()
        ))
    })?;
    make_executable_if_needed(&staged_binary)?;
    let persisted_dir = tmp_dir.keep();

    Ok(persisted_dir.join(asset.binary_name))
}

fn extract_tar_gz(bytes: &[u8], destination: &Path) -> Result<()> {
    let decoder = GzDecoder::new(Cursor::new(bytes));
    let mut archive = Archive::new(decoder);
    archive
        .unpack(destination)
        .map_err(|err| ZocliError::Io(format!("failed to unpack release archive: {err}")))
}

fn extract_zip(bytes: &[u8], destination: &Path) -> Result<()> {
    let reader = Cursor::new(bytes);
    let mut archive = ZipArchive::new(reader)
        .map_err(|err| ZocliError::Io(format!("failed to open release zip: {err}")))?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|err| ZocliError::Io(format!("failed to read release zip entry: {err}")))?;
        let Some(name) = entry.enclosed_name().map(|name| name.to_path_buf()) else {
            continue;
        };
        let output_path = destination.join(name);
        if entry.name().ends_with('/') {
            fs::create_dir_all(&output_path).map_err(|err| {
                ZocliError::Io(format!(
                    "failed to create extracted directory {}: {err}",
                    output_path.display()
                ))
            })?;
            continue;
        }

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                ZocliError::Io(format!(
                    "failed to create extracted directory {}: {err}",
                    parent.display()
                ))
            })?;
        }

        let mut output = fs::File::create(&output_path).map_err(|err| {
            ZocliError::Io(format!(
                "failed to create extracted file {}: {err}",
                output_path.display()
            ))
        })?;
        let mut buffer = Vec::new();
        entry.read_to_end(&mut buffer).map_err(|err| {
            ZocliError::Io(format!(
                "failed to read extracted file {}: {err}",
                output_path.display()
            ))
        })?;
        output.write_all(&buffer).map_err(|err| {
            ZocliError::Io(format!(
                "failed to write extracted file {}: {err}",
                output_path.display()
            ))
        })?;

        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&output_path, fs::Permissions::from_mode(mode)).map_err(|err| {
                ZocliError::Io(format!(
                    "failed to restore permissions for {}: {err}",
                    output_path.display()
                ))
            })?;
        }
    }

    Ok(())
}

fn find_binary(root: &Path, binary_name: &str) -> Result<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in fs::read_dir(&path).map_err(|err| {
            ZocliError::Io(format!(
                "failed to read extracted directory {}: {err}",
                path.display()
            ))
        })? {
            let entry = entry.map_err(|err| {
                ZocliError::Io(format!(
                    "failed to read extracted directory entry in {}: {err}",
                    path.display()
                ))
            })?;
            let entry_path = entry.path();
            if entry
                .file_type()
                .map_err(|err| {
                    ZocliError::Io(format!(
                        "failed to inspect extracted path {}: {err}",
                        entry_path.display()
                    ))
                })?
                .is_dir()
            {
                stack.push(entry_path);
                continue;
            }
            if entry_path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == binary_name)
            {
                return Ok(entry_path);
            }
        }
    }

    Err(ZocliError::Integrity(format!(
        "release archive does not contain executable {binary_name}"
    )))
}

fn make_executable_if_needed(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .map_err(|err| {
                ZocliError::Io(format!(
                    "failed to read metadata for {}: {err}",
                    path.display()
                ))
            })?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).map_err(|err| {
            ZocliError::Io(format!(
                "failed to set executable permissions for {}: {err}",
                path.display()
            ))
        })?;
    }

    Ok(())
}

fn replace_current_binary(staged_binary: &Path) -> Result<()> {
    self_replace::self_replace(staged_binary).map_err(|err| {
        ZocliError::Io(format!(
            "failed to replace current executable with {}: {err}",
            staged_binary.display()
        ))
    })
}

fn build_update_report(
    operation: &str,
    current_version: &str,
    plan: &ReleasePlan,
    explicit_status: Option<&str>,
) -> UpdateStatusReport {
    let status = explicit_status.unwrap_or_else(|| match plan.resolved_version.as_deref() {
        Some(version) if has_newer_release(current_version, Some(version)) => "update_available",
        Some(_) => "already_up_to_date",
        None => "check_complete",
    });
    let target_version = plan
        .resolved_version
        .clone()
        .unwrap_or_else(|| plan.requested_version.trim_start_matches('v').to_string());
    UpdateStatusReport {
        operation: operation.to_string(),
        status: status.to_string(),
        current_version: current_version.to_string(),
        target_version,
        requested_version: plan.requested_version.clone(),
        asset: plan.asset.file_name.to_string(),
        target: plan.asset.target.to_string(),
        base_url: plan.base_url.clone(),
    }
}

fn has_newer_release(current_version: &str, resolved_version: Option<&str>) -> bool {
    let Some(resolved_version) = resolved_version else {
        return false;
    };

    match compare_versions(current_version, resolved_version) {
        Some(std::cmp::Ordering::Less) => true,
        Some(_) => false,
        None => resolved_version != current_version,
    }
}

fn compare_versions(current_version: &str, target_version: &str) -> Option<std::cmp::Ordering> {
    let parse = |version: &str| -> Option<Vec<u64>> {
        version
            .trim_start_matches('v')
            .split('.')
            .map(|part| part.parse::<u64>().ok())
            .collect()
    };

    let current = parse(current_version)?;
    let target = parse(target_version)?;
    let max_len = current.len().max(target.len());
    for index in 0..max_len {
        let lhs = current.get(index).copied().unwrap_or(0);
        let rhs = target.get(index).copied().unwrap_or(0);
        match lhs.cmp(&rhs) {
            std::cmp::Ordering::Equal => continue,
            ordering => return Some(ordering),
        }
    }
    Some(std::cmp::Ordering::Equal)
}

fn render_output(format: OutputFormat, report: &UpdateStatusReport) -> Result<RenderedOutput> {
    let json = json!({
        "operation": report.operation,
        "status": report.status,
        "current_version": report.current_version,
        "target_version": report.target_version,
        "requested_version": report.requested_version,
        "asset": report.asset,
        "target": report.target,
        "base_url": report.base_url,
    });
    let table = match (report.operation.as_str(), report.status.as_str()) {
        ("update.check", "already_up_to_date") => {
            format!("zocli is up to date ({})", report.current_version)
        }
        ("update.check", "update_available") => format!(
            "Update available: {} -> {}\nRun: zocli update",
            report.current_version, report.target_version
        ),
        ("update.apply", "updated") => format!(
            "Updated zocli from {} to {}",
            report.current_version, report.target_version
        ),
        ("update.apply", "already_up_to_date") => {
            format!("zocli is already up to date ({})", report.current_version)
        }
        _ => [
            format!("operation\t{}", report.operation),
            format!("status\t{}", report.status),
            format!("current_version\t{}", report.current_version),
            format!("target_version\t{}", report.target_version),
            format!("asset\t{}", report.asset),
            format!("target\t{}", report.target),
            format!("base_url\t{}", report.base_url),
        ]
        .join("\n"),
    };

    Ok(RenderedOutput {
        format,
        json,
        table,
        exit_code: 0,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use std::io::Write;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::Builder;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    use super::*;

    static UPDATE_BASE_URL_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn normalize_version_accepts_latest_and_prefixed_tags() {
        assert_eq!(normalize_version("latest"), "latest");
        assert_eq!(normalize_version("0.1.32"), "v0.1.32");
        assert_eq!(normalize_version("v0.1.32"), "v0.1.32");
        assert_eq!(normalize_version("  "), "latest");
    }

    #[test]
    fn configured_base_url_override_prefers_explicit_value_over_env() {
        let _guard = UPDATE_BASE_URL_ENV_LOCK.lock().expect("env lock");
        unsafe {
            env::set_var(UPDATE_BASE_URL_ENV, "https://mirror.example.test/releases");
        }
        assert_eq!(
            configured_base_url_override(Some("https://override.example.test/download")).as_deref(),
            Some("https://override.example.test/download")
        );
        unsafe {
            env::remove_var(UPDATE_BASE_URL_ENV);
        }
    }

    #[test]
    fn version_from_download_url_extracts_tag() {
        let url =
            Url::parse("https://github.com/NextStat/zocli/releases/download/v0.2.0/SHA256SUMS")
                .expect("url");
        assert_eq!(version_from_download_url(&url).as_deref(), Some("0.2.0"));
    }

    #[test]
    fn build_update_report_marks_check_as_up_to_date_when_versions_match() {
        let asset = ReleaseAsset {
            archive_kind: ArchiveKind::TarGz,
            binary_name: "zocli",
            file_name: "zocli-aarch64-apple-darwin.tar.gz",
            target: "aarch64-apple-darwin",
        };
        let plan = ReleasePlan {
            asset,
            base_url: "https://github.com/NextStat/zocli/releases/latest/download".to_string(),
            requested_version: "latest".to_string(),
            resolved_version: Some("0.2.0".to_string()),
            checksum: "deadbeef".to_string(),
        };

        let report = build_update_report("update.check", "0.2.0", &plan, None);
        assert_eq!(report.status, "already_up_to_date");
        assert_eq!(report.target_version, "0.2.0");
    }

    #[test]
    fn build_update_report_does_not_offer_downgrade_when_current_is_newer() {
        let asset = ReleaseAsset {
            archive_kind: ArchiveKind::TarGz,
            binary_name: "zocli",
            file_name: "zocli-aarch64-apple-darwin.tar.gz",
            target: "aarch64-apple-darwin",
        };
        let plan = ReleasePlan {
            asset,
            base_url: "https://github.com/NextStat/zocli/releases/latest/download".to_string(),
            requested_version: "latest".to_string(),
            resolved_version: Some("0.2.0".to_string()),
            checksum: "deadbeef".to_string(),
        };

        let report = build_update_report("update.check", "0.2.1", &plan, None);
        assert_eq!(report.status, "already_up_to_date");
        assert_eq!(report.target_version, "0.2.0");
    }

    #[test]
    fn render_output_uses_human_message_for_up_to_date_checks() {
        let report = UpdateStatusReport {
            operation: "update.check".to_string(),
            status: "already_up_to_date".to_string(),
            current_version: "0.2.1".to_string(),
            target_version: "0.2.1".to_string(),
            requested_version: "latest".to_string(),
            asset: "zocli-aarch64-apple-darwin.tar.gz".to_string(),
            target: "aarch64-apple-darwin".to_string(),
            base_url: "https://github.com/NextStat/zocli/releases/latest/download".to_string(),
        };

        let rendered = render_output(OutputFormat::Table, &report).expect("rendered");
        assert_eq!(rendered.table, "zocli is up to date (0.2.1)");
    }

    #[test]
    fn checksum_for_asset_reads_sha256sums() {
        let checksums = "abc123  zocli-aarch64-apple-darwin.tar.gz\nxyz789  other.tar.gz\n";
        assert_eq!(
            checksum_for_asset(checksums, "zocli-aarch64-apple-darwin.tar.gz").expect("checksum"),
            "abc123"
        );
    }

    #[test]
    fn extract_tar_gz_stage_finds_unix_binary() {
        let bytes = build_tar_gz_asset("zocli", b"unix-binary");
        let asset = ReleaseAsset {
            archive_kind: ArchiveKind::TarGz,
            binary_name: "zocli",
            file_name: "zocli-x86_64-unknown-linux-gnu.tar.gz",
            target: "x86_64-unknown-linux-gnu",
        };

        let binary = stage_release_binary(&bytes, &asset).expect("staged binary");
        assert_eq!(fs::read(&binary).expect("binary bytes"), b"unix-binary");
    }

    #[test]
    fn extract_zip_stage_finds_windows_binary() {
        let bytes = build_zip_asset("zocli.exe", b"windows-binary");
        let asset = ReleaseAsset {
            archive_kind: ArchiveKind::Zip,
            binary_name: "zocli.exe",
            file_name: "zocli-x86_64-pc-windows-msvc.zip",
            target: "x86_64-pc-windows-msvc",
        };

        let binary = stage_release_binary(&bytes, &asset).expect("staged binary");
        assert_eq!(fs::read(&binary).expect("binary bytes"), b"windows-binary");
    }

    fn build_tar_gz_asset(binary_name: &str, contents: &[u8]) -> Vec<u8> {
        let mut archive = Vec::new();
        {
            let encoder = GzEncoder::new(&mut archive, Compression::default());
            let mut builder = Builder::new(encoder);
            let path = format!("zocli-test/{binary_name}");
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, path, Cursor::new(contents))
                .expect("append tar entry");
            builder.finish().expect("finish tar");
        }
        archive
    }

    fn build_zip_asset(binary_name: &str, contents: &[u8]) -> Vec<u8> {
        let temp = tempdir().expect("tempdir");
        let archive_path = temp.path().join("release.zip");
        let file = fs::File::create(&archive_path).expect("zip file");
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file(
            format!("zocli-test/{binary_name}"),
            SimpleFileOptions::default(),
        )
        .expect("start file");
        zip.write_all(contents).expect("write zip file");
        zip.finish().expect("finish zip");
        fs::read(&archive_path).expect("zip bytes")
    }
}
