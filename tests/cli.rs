use assert_cmd::Command;
use mockito::Server;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use tempfile::tempdir;

fn zocli() -> Command {
    let mut command = Command::cargo_bin("zocli").expect("binary exists");
    command.env("ZOCLI_SECRET_BACKEND", "file");
    command
}

fn current_release_update_target() -> (&'static str, &'static str) {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => ("zocli-aarch64-apple-darwin.tar.gz", "aarch64-apple-darwin"),
        ("macos", "x86_64") => ("zocli-x86_64-apple-darwin.tar.gz", "x86_64-apple-darwin"),
        ("linux", "aarch64") => (
            "zocli-aarch64-unknown-linux-gnu.tar.gz",
            "aarch64-unknown-linux-gnu",
        ),
        ("linux", "x86_64") => (
            "zocli-x86_64-unknown-linux-gnu.tar.gz",
            "x86_64-unknown-linux-gnu",
        ),
        ("windows", "x86_64") => ("zocli-x86_64-pc-windows-msvc.zip", "x86_64-pc-windows-msvc"),
        (os, arch) => panic!("unsupported published auto-update target {arch}-{os}"),
    }
}

fn write_accounts_file(config_dir: &std::path::Path, content: &str) {
    fs::write(config_dir.join("accounts.toml"), content).expect("accounts file written");
}

fn write_credentials_file(config_dir: &std::path::Path, content: &str) {
    fs::write(config_dir.join("credentials.toml"), content).expect("credentials file written");
}

/// Write a mock Zoho account config with a single credential_ref.
fn write_mock_zoho_account(
    config_dir: &std::path::Path,
    datacenter: &str,
    credential_ref: Option<&str>,
) {
    let credential_ref_line = credential_ref
        .map(|v| format!("credential_ref = \"{v}\"\n"))
        .unwrap_or_default();

    write_accounts_file(
        config_dir,
        &format!(
            r#"
version = 1

[accounts.mock]
email = "me@zoho.com"
default = true
datacenter = "{datacenter}"
account_id = "12345"
client_id = "1000.TESTCLIENTID"
{credential_ref_line}"#,
        ),
    );
}

/// Write a mock Zoho account with an oauth token in the credentials store.
fn write_mock_zoho_account_with_oauth(
    config_dir: &std::path::Path,
    access_token: &str,
    expires_at: u64,
) {
    write_mock_zoho_account(config_dir, "com", Some("store:oauth"));
    write_credentials_file(
        config_dir,
        &format!(
            r#"
version = 1

[accounts.mock.services.oauth]
kind = "oauth_pkce"
access_token = "{access_token}"
token_type = "Bearer"
expires_at_epoch_secs = {expires_at}
scope = ["ZohoMail.messages.ALL", "ZohoMail.folders.ALL", "ZohoCalendar.event.ALL", "WorkDrive.files.ALL"]
client_id = "1000.TESTCLIENTID"
"#,
        ),
    );
}

// ============================================================================
// Account add / list / use / whoami
// ============================================================================

#[test]
fn account_add_and_list_work() {
    let temp = tempdir().expect("tempdir");

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "account",
            "add",
            "personal",
            "me@zoho.com",
            "--account-id",
            "12345",
            "--client-id",
            "1000.TESTCLIENTID",
            "--use",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"operation\":\"account.add\""));

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["account", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    let items = value["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "personal");
    assert_eq!(items[0]["current"], true);
}

#[test]
fn simple_add_sets_current_account_and_derived_name() {
    let temp = tempdir().expect("tempdir");

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "add",
            "me@zoho.com",
            "--account-id",
            "12345",
            "--client-id",
            "1000.TESTCLIENTID",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"operation\":\"account.add\""));

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["whoami"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["account"], "me");
    assert_eq!(value["email"], "me@zoho.com");
}

#[test]
fn simple_add_respects_manual_account_name() {
    let temp = tempdir().expect("tempdir");

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "add",
            "me@zoho.com",
            "personal",
            "--account-id",
            "12345",
            "--client-id",
            "1000.TESTCLIENTID",
        ])
        .assert()
        .success();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["whoami"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["account"], "personal");
    assert_eq!(value["email"], "me@zoho.com");
}

#[test]
fn simple_add_uses_shared_default_oauth_client_when_client_id_is_omitted() {
    let temp = tempdir().expect("tempdir");

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .env("ZOCLI_DEFAULT_CLIENT_ID", "1000.SHAREDCLIENT")
        .args(["add", "me@zoho.com"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"operation\":\"account.add\""));

    let whoami = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["whoami"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&whoami).expect("valid json");
    assert_eq!(value["account"], "me");

    let accounts = fs::read_to_string(temp.path().join("accounts.toml")).expect("accounts file");
    assert!(accounts.contains("email = \"me@zoho.com\""));
    assert!(
        !accounts.contains("client_id ="),
        "shared client flow must not persist client_id into accounts.toml"
    );
    assert!(
        !accounts.contains("account_id ="),
        "account_id must be omitted until login auto-discovers it"
    );
}

#[test]
fn simple_add_without_client_id_behaves_consistently_with_shared_default_configuration() {
    let temp = tempdir().expect("tempdir");

    let assert = zocli()
        .env_remove("ZOCLI_DEFAULT_CLIENT_ID")
        .env_remove("ZOCLI_DEFAULT_CLIENT_SECRET")
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["add", "me@zoho.com"])
        .assert();

    if option_env!("ZOCLI_DEFAULT_CLIENT_ID").is_some() {
        assert
            .success()
            .stdout(predicate::str::contains("\"operation\":\"account.add\""));
    } else {
        assert.failure().stderr(predicate::str::contains("shared default OAuth client").or(
            predicate::str::contains("client_id must not be empty"),
        ));
    }
}

// ============================================================================
// Help output tests
// ============================================================================

#[test]
fn top_level_help_shows_commands() {
    zocli()
        .args(["--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage:"))
        .stdout(predicate::str::contains("Commands:"))
        .stdout(predicate::str::contains("Options:"))
        .stdout(predicate::str::contains("add"))
        .stdout(predicate::str::contains("login"))
        .stdout(predicate::str::contains("update"))
        .stdout(predicate::str::contains("mail"))
        .stdout(predicate::str::contains("calendar"))
        .stdout(predicate::str::contains("drive"))
        .stdout(predicate::str::contains("guide").not());
}

#[test]
fn update_help_describes_release_update_surface() {
    zocli()
        .args(["update", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Update zocli from GitHub Releases",
        ))
        .stdout(predicate::str::contains("--version"))
        .stdout(predicate::str::contains("--check"));
}

#[test]
fn update_check_uses_release_checksum_mirror_and_reports_available_target() {
    let (asset, target) = current_release_update_target();
    let mut server = Server::new();
    let base_url = format!("{}/releases/download/v9.9.9", server.url());

    let _checksums = server
        .mock("GET", "/releases/download/v9.9.9/SHA256SUMS")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_body(format!("deadbeef  {asset}\n"))
        .create();

    let output = zocli()
        .args(["update", "--check", "--base-url", &base_url])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "update.check");
    assert_eq!(value["status"], "update_available");
    assert_eq!(value["requested_version"], "latest");
    assert_eq!(value["target_version"], "9.9.9");
    assert_eq!(value["asset"], asset);
    assert_eq!(value["target"], target);
    assert_eq!(value["base_url"], base_url);
}

#[test]
fn update_check_resolves_latest_redirect_to_concrete_version() {
    let (asset, target) = current_release_update_target();
    let mut server = Server::new();
    let latest_base_url = format!("{}/releases/latest/download", server.url());
    let versioned_checksums_path = "/releases/download/v9.9.9/SHA256SUMS";
    let versioned_checksums_url = format!("{}{versioned_checksums_path}", server.url());

    let _latest_redirect = server
        .mock("GET", "/releases/latest/download/SHA256SUMS")
        .with_status(302)
        .with_header("location", &versioned_checksums_url)
        .create();
    let _checksums = server
        .mock("GET", versioned_checksums_path)
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_body(format!("deadbeef  {asset}\n"))
        .create();

    let output = zocli()
        .args(["update", "--check", "--base-url", &latest_base_url])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "update.check");
    assert_eq!(value["status"], "update_available");
    assert_eq!(value["requested_version"], "latest");
    assert_eq!(value["target_version"], "9.9.9");
    assert_eq!(value["asset"], asset);
    assert_eq!(value["target"], target);
    assert_eq!(value["base_url"], latest_base_url);
}

#[test]
fn mail_read_help_uses_positional_message_id() {
    zocli()
        .args(["mail", "read", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage:"))
        .stdout(predicate::str::contains(
            "zocli mail read [OPTIONS] --folder-id <FOLDER_ID> <MESSAGE_ID>",
        ))
        .stdout(predicate::str::contains("--uid").not());
}

#[test]
fn mail_search_help_uses_positional_query() {
    zocli()
        .args(["mail", "search", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "zocli mail search [OPTIONS] <QUERY>",
        ))
        .stdout(predicate::str::contains("--query").not());
}

#[test]
fn mail_reply_help_uses_positional_args() {
    zocli()
        .args(["mail", "reply", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "zocli mail reply [OPTIONS] --folder-id <FOLDER_ID> <MESSAGE_ID> [BODY]",
        ))
        .stdout(predicate::str::contains("--text").not());
}

#[test]
fn mail_forward_help_uses_positional_args() {
    zocli()
        .args(["mail", "forward", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "zocli mail forward [OPTIONS] --folder-id <FOLDER_ID> <MESSAGE_ID> <TO> [BODY]",
        ))
        .stdout(predicate::str::contains("--text").not());
}

#[test]
fn mail_send_help_uses_positional_args() {
    zocli()
        .args(["mail", "send", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "zocli mail send [OPTIONS] <TO> <SUBJECT> [BODY]",
        ))
        .stdout(predicate::str::contains("--to").not())
        .stdout(predicate::str::contains("--subject").not())
        .stdout(predicate::str::contains("--text").not());
}

#[test]
fn calendar_events_help_uses_positional_dates() {
    zocli()
        .args(["calendar", "events", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "zocli calendar events [OPTIONS] [FROM] [TO]",
        ))
        .stdout(predicate::str::contains("--from").not())
        .stdout(predicate::str::contains("--to").not())
        .stdout(predicate::str::contains("--calendar <CALENDAR_UID>"));
}

#[test]
fn calendar_create_help_uses_positional_args() {
    zocli()
        .args(["calendar", "create", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "zocli calendar create [OPTIONS] <TITLE> <START> <END>",
        ))
        .stdout(predicate::str::contains("--summary").not())
        .stdout(predicate::str::contains("--start").not())
        .stdout(predicate::str::contains("--end").not());
}

#[test]
fn calendar_delete_help_uses_positional_event_uid() {
    zocli()
        .args(["calendar", "delete", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "zocli calendar delete [OPTIONS] <EVENT_UID>",
        ))
        .stdout(predicate::str::contains("--id").not())
        .stdout(predicate::str::contains("--uid").not());
}

#[test]
fn drive_list_help_uses_optional_folder_id() {
    zocli()
        .args(["drive", "list", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "zocli drive list [OPTIONS] [FOLDER_ID]",
        ))
        .stdout(predicate::str::contains("--path").not());
}

#[test]
fn drive_upload_help_uses_positional_args() {
    zocli()
        .args(["drive", "upload", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "zocli drive upload [OPTIONS] <FILE> <FOLDER_ID>",
        ))
        .stdout(predicate::str::contains("--source").not())
        .stdout(predicate::str::contains("--path").not());
}

#[test]
fn drive_download_help_uses_positional_args() {
    zocli()
        .args(["drive", "download", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "zocli drive download [OPTIONS] --output <OUTPUT> <FILE_ID>",
        ))
        .stdout(predicate::str::contains("--file-id").not());
}

// ============================================================================
// Guide command tests
// ============================================================================

#[test]
fn guide_lists_stable_commands_and_workflows() {
    let output = zocli()
        .args(["guide"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "guide.show");
    assert_eq!(value["topic"], "all");
    assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));

    let commands = value["commands"].as_array().expect("commands array");
    assert!(commands.iter().any(|entry| entry["path"] == "add"));
    assert!(commands.iter().any(|entry| entry["path"] == "accounts"));
    assert!(commands.iter().any(|entry| entry["path"] == "use"));
    assert!(commands.iter().any(|entry| entry["path"] == "whoami"));
    assert!(commands.iter().any(|entry| entry["path"] == "status"));
    assert!(commands.iter().any(|entry| entry["path"] == "login"));
    assert!(commands.iter().any(|entry| entry["path"] == "logout"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail read"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail search"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail reply"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail forward"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail send"));
    assert!(commands.iter().any(|entry| entry["path"] == "drive list"));
    assert!(commands.iter().any(|entry| entry["path"] == "drive info"));
    assert!(commands.iter().any(|entry| entry["path"] == "drive upload"));
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "drive download")
    );
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "calendar events")
    );
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "calendar create")
    );
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "calendar delete")
    );

    let workflows = value["workflows"].as_array().expect("workflows array");
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_read_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_search_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_reply_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_forward_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_send_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "drive_browse_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "drive_upload_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "drive_download_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "calendar_read_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "calendar_write_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "multi_account_flow")
    );
}

#[test]
fn guide_topic_mail_filters_to_mail_commands() {
    let output = zocli()
        .args(["guide", "--topic", "mail"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["topic"], "mail");

    let commands = value["commands"].as_array().expect("commands array");
    assert!(!commands.is_empty());
    assert!(commands.iter().all(|entry| entry["topic"] == "mail"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail folders"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail search"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail reply"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail forward"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail read"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail send"));

    let workflows = value["workflows"].as_array().expect("workflows array");
    assert!(workflows.iter().all(|entry| entry["topic"] == "mail"));
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_search_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_reply_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_forward_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_send_flow")
    );
}

#[test]
fn guide_topic_auth_filters_to_auth_commands() {
    let output = zocli()
        .args(["guide", "--topic", "auth"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["topic"], "auth");

    let commands = value["commands"].as_array().expect("commands array");
    assert!(!commands.is_empty());
    assert!(commands.iter().all(|entry| entry["topic"] == "auth"));
    assert!(commands.iter().any(|entry| entry["path"] == "status"));
    assert!(commands.iter().any(|entry| entry["path"] == "login"));
    assert!(commands.iter().any(|entry| entry["path"] == "logout"));
}

#[test]
fn guide_topic_calendar_filters_to_calendar_commands() {
    let output = zocli()
        .args(["guide", "--topic", "calendar"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["topic"], "calendar");

    let commands = value["commands"].as_array().expect("commands array");
    assert!(!commands.is_empty());
    assert!(commands.iter().all(|entry| entry["topic"] == "calendar"));
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "calendar calendars")
    );
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "calendar events")
    );
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "calendar create")
    );
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "calendar delete")
    );

    let workflows = value["workflows"].as_array().expect("workflows array");
    assert!(workflows.iter().all(|entry| entry["topic"] == "calendar"));
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "calendar_read_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "calendar_write_flow")
    );
}

#[test]
fn guide_topic_drive_filters_to_drive_commands() {
    let output = zocli()
        .args(["guide", "--topic", "drive"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["topic"], "drive");

    let commands = value["commands"].as_array().expect("commands array");
    assert!(!commands.is_empty());
    assert!(commands.iter().all(|entry| entry["topic"] == "drive"));
    assert!(commands.iter().any(|entry| entry["path"] == "drive list"));
    assert!(commands.iter().any(|entry| entry["path"] == "drive info"));
    assert!(commands.iter().any(|entry| entry["path"] == "drive upload"));
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "drive download")
    );

    let workflows = value["workflows"].as_array().expect("workflows array");
    assert!(workflows.iter().all(|entry| entry["topic"] == "drive"));
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "drive_browse_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "drive_upload_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "drive_download_flow")
    );
}

// ============================================================================
// Account use / current / list
// ============================================================================

#[test]
fn account_use_switches_current_account() {
    let temp = tempdir().expect("tempdir");

    for name in ["personal", "work"] {
        zocli()
            .env("ZOCLI_CONFIG_DIR", temp.path())
            .args([
                "account",
                "add",
                name,
                "me@zoho.com",
                "--account-id",
                "12345",
                "--client-id",
                "1000.TESTCLIENTID",
            ])
            .assert()
            .success();
    }

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["account", "use", "work"])
        .assert()
        .success();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["account", "current"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["account"], "work");
}

#[test]
fn account_list_marks_single_account_as_current_without_default_flag() {
    let temp = tempdir().expect("tempdir");

    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.single]
email = "single@zoho.com"
default = false
datacenter = "com"
account_id = "12345"
client_id = "1000.TESTCLIENTID"
"#,
    );

    let list_output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["account", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let list: Value = serde_json::from_slice(&list_output).expect("valid json");
    assert_eq!(list["items"][0]["name"], "single");
    assert_eq!(list["items"][0]["current"], true);

    let current_output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["account", "current"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let current: Value = serde_json::from_slice(&current_output).expect("valid json");
    assert_eq!(current["account"], "single");
    assert_eq!(current["email"], "single@zoho.com");
}

#[test]
fn validate_reports_invalid_account() {
    let temp = tempdir().expect("tempdir");

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "account",
            "add",
            "broken",
            "not-an-email",
            "--account-id",
            "12345",
            "--client-id",
            "1000.TESTCLIENTID",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("VALIDATION_ERROR"));
}

// ============================================================================
// Auth status
// ============================================================================

#[test]
fn auth_status_reports_credential_state() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account(temp.path(), "com", Some("store:oauth"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.oauth]
kind = "oauth_pkce"
access_token = "access-123"
token_type = "Bearer"
expires_at_epoch_secs = 4102444800
scope = ["ZohoMail.messages.ALL"]
client_id = "1000.TESTCLIENTID"
"#,
    );

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["auth", "status", "--profile", "mock"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(
        value["credential_state"]["credential_state"],
        "store_present"
    );
    assert_eq!(value["credential_state"]["credential_ref"], "store:oauth");
}

#[test]
fn auth_status_reports_not_configured_when_no_credential_ref() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account(temp.path(), "com", None);

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["status", "--profile", "mock"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(
        value["credential_state"]["credential_state"],
        "not_configured"
    );
}

#[test]
fn auth_status_reports_store_expired_for_old_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account_with_oauth(temp.path(), "expired-token", 1);

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["auth", "status", "--profile", "mock"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(
        value["credential_state"]["credential_state"],
        "store_expired"
    );
}

// ============================================================================
// Auth logout
// ============================================================================

#[test]
fn auth_logout_removes_stored_oauth_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account(temp.path(), "com", Some("store:oauth"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.oauth]
kind = "oauth_pkce"
access_token = "access-123"
token_type = "Bearer"
expires_at_epoch_secs = 4102444800
scope = ["ZohoMail.messages.ALL"]
client_id = "1000.TESTCLIENTID"
"#,
    );

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["auth", "logout", "--profile", "mock", "--service", "mail"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "auth.logout");
    assert_eq!(value["service"], "mail");
    assert_eq!(value["removed"], true);

    let accounts = fs::read_to_string(temp.path().join("accounts.toml")).expect("accounts");
    assert!(!accounts.contains("credential_ref = \"store:oauth\""));

    let credentials =
        fs::read_to_string(temp.path().join("credentials.toml")).expect("credentials");
    assert!(!credentials.contains("[accounts.mock.services.oauth]"));
}

#[test]
fn logout_only_removes_target_account_token() {
    let temp = tempdir().expect("tempdir");

    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "personal@zoho.com"
default = true
datacenter = "com"
account_id = "11111"
client_id = "1000.PERSONAL"
credential_ref = "store:oauth"

[accounts.work]
email = "work@zoho.com"
default = false
datacenter = "com"
account_id = "22222"
client_id = "1000.WORK"
credential_ref = "store:oauth"
"#,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.personal.services.oauth]
kind = "oauth_pkce"
access_token = "access-personal"
token_type = "Bearer"
expires_at_epoch_secs = 4102444800
scope = ["ZohoMail.messages.ALL"]
client_id = "1000.PERSONAL"

[accounts.work.services.oauth]
kind = "oauth_pkce"
access_token = "access-work"
token_type = "Bearer"
expires_at_epoch_secs = 4102444800
scope = ["ZohoMail.messages.ALL"]
client_id = "1000.WORK"
"#,
    );

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "auth",
            "logout",
            "--profile",
            "personal",
            "--service",
            "mail",
        ])
        .assert()
        .success();

    let credentials =
        fs::read_to_string(temp.path().join("credentials.toml")).expect("credentials");
    assert!(!credentials.contains("[accounts.personal.services.oauth]"));
    assert!(credentials.contains("[accounts.work.services.oauth]"));
    assert!(credentials.contains("access_token = \"access-work\""));
}

#[test]
fn simple_logout_without_service_logs_out_all_services() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account(temp.path(), "com", Some("store:oauth"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.oauth]
kind = "oauth_pkce"
access_token = "access-123"
token_type = "Bearer"
expires_at_epoch_secs = 4102444800
scope = ["ZohoMail.messages.ALL"]
client_id = "1000.TESTCLIENTID"
"#,
    );

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["logout", "--profile", "mock"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "logout");
    let items = value["items"].as_array().expect("items array");
    assert_eq!(items.len(), 3);
}

// ============================================================================
// Expired token tests — mail commands
// ============================================================================

#[test]
fn mail_folders_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account_with_oauth(temp.path(), "expired-token", 1);

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mail", "folders", "--profile", "mock"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""));
}

#[test]
fn mail_list_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account_with_oauth(temp.path(), "expired-token", 1);

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mail", "list", "--profile", "mock"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""));
}

#[test]
fn mail_read_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account_with_oauth(temp.path(), "expired-token", 1);

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "mail",
            "read",
            "--profile",
            "mock",
            "msg-42",
            "--folder-id",
            "folder-123",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""));
}

#[test]
fn mail_search_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account_with_oauth(temp.path(), "expired-token", 1);

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mail", "search", "--profile", "mock", "Budget"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""));
}

#[test]
fn mail_reply_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account_with_oauth(temp.path(), "expired-token", 1);

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "mail",
            "reply",
            "--profile",
            "mock",
            "msg-42",
            "--folder-id",
            "folder-123",
            "Got it",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""));
}

#[test]
fn mail_forward_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account_with_oauth(temp.path(), "expired-token", 1);

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "mail",
            "forward",
            "--profile",
            "mock",
            "msg-42",
            "--folder-id",
            "folder-123",
            "person@example.com",
            "FYI",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""));
}

// ============================================================================
// Expired token tests — drive commands
// ============================================================================

#[test]
fn drive_info_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account_with_oauth(temp.path(), "expired-token", 1);

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["drive", "info", "--profile", "mock"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""));
}

#[test]
fn drive_list_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account_with_oauth(temp.path(), "expired-token", 1);

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["drive", "list", "--profile", "mock"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""));
}

// ============================================================================
// Calendar validation
// ============================================================================

#[test]
fn calendar_events_rejects_zero_limit() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account_with_oauth(temp.path(), "valid-token", 4102444800);

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["calendar", "events", "--profile", "mock", "--limit", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""));
}

// ============================================================================
// Drive upload / download validation
// ============================================================================

#[test]
fn drive_upload_rejects_missing_source_file() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account_with_oauth(temp.path(), "valid-token", 4102444800);

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "drive",
            "upload",
            "--profile",
            "mock",
            "/nonexistent/path/to/file.txt",
            "folder-123",
        ])
        .assert()
        .failure();
}

#[test]
fn drive_download_refuses_to_overwrite_without_force() {
    let temp = tempdir().expect("tempdir");
    let output_path = temp.path().join("existing.pdf");
    fs::write(&output_path, b"existing content").expect("existing file");

    write_mock_zoho_account_with_oauth(temp.path(), "valid-token", 4102444800);

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "drive",
            "download",
            "--profile",
            "mock",
            "file-123",
            "--output",
            output_path.to_str().expect("utf8 path"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"OUTPUT_EXISTS\""));

    assert_eq!(
        fs::read(&output_path).expect("existing file"),
        b"existing content"
    );
}

// ============================================================================
// Account add with datacenter
// ============================================================================

#[test]
fn add_with_datacenter_stores_correct_value() {
    let temp = tempdir().expect("tempdir");

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "add",
            "me@zoho.eu",
            "eu-account",
            "--account-id",
            "99999",
            "--client-id",
            "1000.EUCLIENT",
            "--datacenter",
            "eu",
        ])
        .assert()
        .success();

    let accounts = fs::read_to_string(temp.path().join("accounts.toml")).expect("accounts");
    assert!(accounts.contains("datacenter = \"eu\""));
    assert!(accounts.contains("account_id = \"99999\""));
    assert!(accounts.contains("client_id = \"1000.EUCLIENT\""));
}

#[test]
fn add_rejects_invalid_datacenter() {
    let temp = tempdir().expect("tempdir");

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "add",
            "me@zoho.com",
            "--account-id",
            "12345",
            "--client-id",
            "1000.TEST",
            "--datacenter",
            "invalid",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("VALIDATION_ERROR"))
        .stderr(predicate::str::contains("datacenter"));
}

// ============================================================================
// Account show / validate
// ============================================================================

#[test]
fn account_show_returns_config_details() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account(temp.path(), "com", Some("store:oauth"));

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["account", "show", "--profile", "mock"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "account.show");
    assert_eq!(value["account"], "mock");
    assert_eq!(value["config"]["email"], "me@zoho.com");
    assert_eq!(value["config"]["datacenter"], "com");
    assert_eq!(value["config"]["account_id"], "12345");
    assert_eq!(value["config"]["client_id"], "1000.TESTCLIENTID");
}

#[test]
fn account_validate_reports_valid_config() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account(temp.path(), "com", Some("store:oauth"));

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["account", "validate", "--profile", "mock"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "account.validate");
    assert_eq!(value["valid"], true);
    assert_eq!(value["errors"].as_array().expect("errors array").len(), 0);
}

// ============================================================================
// Top-level use / whoami shortcuts
// ============================================================================

#[test]
fn use_and_whoami_shortcuts_work() {
    let temp = tempdir().expect("tempdir");

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "add",
            "alice@zoho.com",
            "alice",
            "--account-id",
            "111",
            "--client-id",
            "1000.A",
        ])
        .assert()
        .success();

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "add",
            "bob@zoho.com",
            "bob",
            "--account-id",
            "222",
            "--client-id",
            "1000.B",
        ])
        .assert()
        .success();

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["use", "bob"])
        .assert()
        .success();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["whoami"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["account"], "bob");
    assert_eq!(value["email"], "bob@zoho.com");
}

// ============================================================================
// Accounts (top-level shortcut)
// ============================================================================

#[test]
fn accounts_shortcut_lists_all_accounts() {
    let temp = tempdir().expect("tempdir");

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "add",
            "a@zoho.com",
            "alpha",
            "--account-id",
            "1",
            "--client-id",
            "1000.A",
        ])
        .assert()
        .success();

    zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args([
            "add",
            "b@zoho.com",
            "beta",
            "--account-id",
            "2",
            "--client-id",
            "1000.B",
        ])
        .assert()
        .success();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["accounts"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    let items = value["items"].as_array().expect("items array");
    assert_eq!(items.len(), 2);
}

// ============================================================================
// Credential file round-trip with refresh_token and api_domain
// ============================================================================

#[test]
fn credential_file_preserves_refresh_token_and_api_domain() {
    let temp = tempdir().expect("tempdir");

    write_mock_zoho_account(temp.path(), "com", Some("store:oauth"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.oauth]
kind = "oauth_pkce"
access_token = "access-123"
refresh_token = "refresh-456"
token_type = "Bearer"
expires_at_epoch_secs = 4102444800
scope = ["ZohoMail.messages.ALL"]
client_id = "1000.TESTCLIENTID"
api_domain = "https://www.zohoapis.eu"
"#,
    );

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["auth", "status", "--profile", "mock"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(
        value["credential_state"]["credential_state"],
        "store_present"
    );

    // Verify the credential file still contains the refresh token
    let credentials =
        fs::read_to_string(temp.path().join("credentials.toml")).expect("credentials");
    assert!(credentials.contains("refresh_token = \"refresh-456\""));
    assert!(credentials.contains("api_domain = \"https://www.zohoapis.eu\""));
}
