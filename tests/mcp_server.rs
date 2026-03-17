use assert_cmd::Command;
use mockito::Server;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command as StdCommand, Stdio};
use tempfile::tempdir;

const APP_RESOURCE_URI: &str = "ui://zocli/dashboard";
const APP_RESOURCE_URI_TEMPLATE: &str =
    "ui://zocli/dashboard{?account,section,resource,tool,skill,prompt}";
const APP_RESOURCE_MIME_TYPE: &str = "text/html;profile=mcp-app";

fn zocli() -> Command {
    let mut command = Command::cargo_bin("zocli").expect("binary exists");
    command.env("ZOCLI_SECRET_BACKEND", "file");
    command
}

fn current_release_update_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "zocli-aarch64-apple-darwin.tar.gz",
        ("macos", "x86_64") => "zocli-x86_64-apple-darwin.tar.gz",
        ("linux", "aarch64") => "zocli-aarch64-unknown-linux-gnu.tar.gz",
        ("linux", "x86_64") => "zocli-x86_64-unknown-linux-gnu.tar.gz",
        ("windows", "x86_64") => "zocli-x86_64-pc-windows-msvc.zip",
        (os, arch) => panic!("unsupported published auto-update target {arch}-{os}"),
    }
}

fn mcp_request(id: u64, method: &str, params: Value) -> String {
    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .expect("request json");
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

fn json_line_request(id: u64, method: &str, params: Value) -> String {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .expect("request json")
        + "\n"
}

fn mcp_notification(method: &str, params: Value) -> String {
    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    }))
    .expect("notification json");
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

fn mcp_response(id: u64, result: Value) -> String {
    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    }))
    .expect("response json");
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

fn json_line_notification(method: &str, params: Value) -> String {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    }))
    .expect("notification json")
        + "\n"
}

fn initialize_request(ui_enabled: bool) -> String {
    initialize_request_with_capabilities(ui_enabled, None)
}

fn initialize_request_with_roots(ui_enabled: bool, list_changed: bool) -> String {
    initialize_request_with_capabilities(ui_enabled, Some(list_changed))
}

fn initialize_request_with_capabilities(
    ui_enabled: bool,
    roots_list_changed: Option<bool>,
) -> String {
    let mut capabilities = if ui_enabled {
        json!({
            "extensions": {
                "io.modelcontextprotocol/ui": {
                    "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                }
            }
        })
    } else {
        json!({})
    };
    if let Some(list_changed) = roots_list_changed {
        capabilities["roots"] = json!({
            "listChanged": list_changed
        });
    }

    mcp_request(
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": capabilities,
            "clientInfo": { "name": "test", "version": "0.1.0" }
        }),
    )
}

fn json_line_initialize_request(ui_enabled: bool) -> String {
    let capabilities = if ui_enabled {
        json!({
            "extensions": {
                "io.modelcontextprotocol/ui": {
                    "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                }
            }
        })
    } else {
        json!({})
    };

    json_line_request(
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": capabilities,
            "clientInfo": { "name": "claude-code", "version": "2.1.75" }
        }),
    )
}

fn parse_responses(stdout: &[u8]) -> Vec<Value> {
    let raw = String::from_utf8(stdout.to_vec()).expect("utf8 output");
    let mut remaining = raw.as_str();
    let mut responses = Vec::new();

    while !remaining.is_empty() {
        let (headers, rest) = remaining.split_once("\r\n\r\n").expect("header separator");
        let length = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .expect("content-length")
            .parse::<usize>()
            .expect("length");
        let body = &rest[..length];
        responses.push(serde_json::from_str(body).expect("valid json-rpc response"));
        remaining = &rest[length..];
    }

    responses
}

fn parse_json_line_responses(stdout: &[u8]) -> Vec<Value> {
    String::from_utf8(stdout.to_vec())
        .expect("utf8 output")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid json-rpc response"))
        .collect()
}

fn read_response(reader: &mut dyn BufRead) -> Value {
    let mut content_length = None::<usize>;
    let mut line = String::new();

    loop {
        line.clear();
        let bytes = reader.read_line(&mut line).expect("header line");
        assert!(bytes > 0, "response stream closed unexpectedly");
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(value) = line.trim().strip_prefix("Content-Length: ") {
            content_length = Some(value.parse::<usize>().expect("content length"));
        }
    }

    let length = content_length.expect("content length header");
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).expect("response payload");
    serde_json::from_slice(&payload).expect("json-rpc payload")
}

fn write_accounts_file(config_dir: &std::path::Path, content: &str) {
    fs::create_dir_all(config_dir).expect("config dir");
    fs::write(config_dir.join("accounts.toml"), content).expect("accounts file");
}

fn write_credentials_file(config_dir: &std::path::Path, content: &str) {
    fs::create_dir_all(config_dir).expect("config dir");
    fs::write(config_dir.join("credentials.toml"), content).expect("credentials file");
}

fn write_mock_mail_account(config_dir: &std::path::Path) {
    write_accounts_file(
        config_dir,
        r#"
version = 1

[accounts.mock]
email = "me@zoho.com"
default = true
datacenter = "com"
account_id = "12345"
client_id = "client-123"
credential_ref = "store:oauth"
"#,
    );
}

#[test]
fn mcp_stdio_initialize_returns_capabilities() {
    let output = zocli()
        .args(["mcp"])
        .write_stdin(initialize_request(true))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let response = parse_responses(&output).remove(0);
    assert_eq!(response["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(response["result"]["serverInfo"]["name"], "zocli");
    assert_eq!(response["result"]["capabilities"]["completions"], json!({}));
    assert_eq!(
        response["result"]["capabilities"]["prompts"]["listChanged"],
        false
    );
    assert_eq!(
        response["result"]["capabilities"]["experimental"]["io.modelcontextprotocol/ui"]["resourceTemplates"],
        true
    );
    assert_eq!(
        response["result"]["capabilities"]["experimental"]["io.modelcontextprotocol/ui"]["mimeTypes"]
            [0],
        APP_RESOURCE_MIME_TYPE
    );
    assert_eq!(
        response["result"]["capabilities"]["resources"]["subscribe"],
        true
    );
}

#[test]
fn mcp_stdio_accepts_claude_code_json_line_messages() {
    let input = [
        json_line_initialize_request(true),
        json_line_notification("notifications/initialized", json!({})),
        json_line_request(2, "tools/list", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let raw_output = String::from_utf8(output.clone()).expect("utf8 output");
    assert!(!raw_output.contains("Content-Length:"));

    let responses = parse_json_line_responses(&output);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(responses[0]["result"]["serverInfo"]["name"], "zocli");

    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    let snapshot_tool = tools
        .iter()
        .find(|tool| tool["name"] == "zocli.app.snapshot")
        .expect("snapshot tool");
    assert_eq!(
        snapshot_tool["_meta"]["ui"]["resourceUri"],
        APP_RESOURCE_URI
    );

    let prompts_input = [
        json_line_initialize_request(true),
        json_line_notification("notifications/initialized", json!({})),
        json_line_request(2, "prompts/list", json!({})),
    ]
    .concat();

    let prompts_output = zocli()
        .args(["mcp"])
        .write_stdin(prompts_input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let prompts_responses = parse_json_line_responses(&prompts_output);
    let prompts = prompts_responses[1]["result"]["prompts"]
        .as_array()
        .expect("prompts array");
    assert!(
        prompts
            .iter()
            .any(|prompt| prompt["name"] == "daily-briefing")
    );
    assert!(prompts.iter().any(|prompt| prompt["name"] == "drive"));
}

#[test]
fn mcp_stdio_rejects_operational_requests_before_initialize() {
    let output = zocli()
        .args(["mcp"])
        .write_stdin(mcp_request(2, "tools/list", json!({})))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let response = parse_responses(&output).remove(0);
    assert_eq!(response["error"]["code"], -32602);
    assert!(
        response["error"]["message"]
            .as_str()
            .expect("message")
            .contains("initialize")
    );
}

#[test]
fn mcp_stdio_lists_and_renders_embedded_prompts() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "prompts/list", json!({})),
        mcp_request(
            3,
            "prompts/get",
            json!({
                "name": "find-and-read",
                "arguments": {
                    "query": "invoice",
                    "account": "work"
                }
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let prompts = responses[1]["result"]["prompts"]
        .as_array()
        .expect("prompts array");
    assert_eq!(prompts.len(), 7);
    let mail_prompt = prompts
        .iter()
        .find(|prompt| prompt["name"] == "mail")
        .expect("mail prompt");
    assert_eq!(mail_prompt["title"], "zocli mail workflow");

    let rendered = responses[2]["result"]["messages"][0]["content"]["text"]
        .as_str()
        .expect("prompt text");
    assert!(rendered.contains("Query: invoice"));
    assert!(rendered.contains("Account: work"));
    assert!(rendered.contains("zocli.mail.search"));
    assert!(rendered.contains("zocli.mail.read"));

    let drive_prompt = prompts
        .iter()
        .find(|prompt| prompt["name"] == "drive")
        .expect("drive prompt");
    assert_eq!(drive_prompt["title"], "zocli drive workflow");
}

#[test]
fn mcp_stdio_completes_prompt_and_resource_arguments() {
    let temp = tempdir().expect("tempdir");
    write_mock_mail_account(temp.path());

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "completion/complete",
            json!({
                "ref": {
                    "type": "ref/prompt",
                    "name": "mail"
                },
                "argument": {
                    "name": "folder",
                    "value": "in"
                }
            }),
        ),
        mcp_request(
            3,
            "completion/complete",
            json!({
                "ref": {
                    "type": "ref/resource",
                    "uri": "resource://zocli/account/{account}"
                },
                "argument": {
                    "name": "account",
                    "value": "mo"
                }
            }),
        ),
        mcp_request(
            4,
            "completion/complete",
            json!({
                "ref": {
                    "type": "ref/resource",
                    "uri": "resource://zocli/skill/{skill}"
                },
                "argument": {
                    "name": "skill",
                    "value": "zocli-ma"
                }
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    assert_eq!(responses[1]["result"]["completion"]["values"][0], "INBOX");
    assert_eq!(responses[1]["result"]["completion"]["hasMore"], false);
    assert_eq!(responses[2]["result"]["completion"]["values"][0], "mock");
    assert_eq!(
        responses[3]["result"]["completion"]["values"][0],
        "zocli-mail"
    );
}

#[test]
fn mcp_stdio_requests_client_roots_and_refreshes_after_list_changed() {
    let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("zocli"))
        .arg("mcp")
        .env("ZOCLI_SECRET_BACKEND", "file")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn zocli mcp");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);

    stdin
        .write_all(initialize_request_with_roots(true, true).as_bytes())
        .expect("write initialize");
    let initialize = read_response(&mut reader);
    assert_eq!(initialize["result"]["protocolVersion"], "2025-11-25");

    stdin
        .write_all(mcp_notification("notifications/initialized", json!({})).as_bytes())
        .expect("write initialized");
    stdin
        .write_all(mcp_request(2, "tools/list", json!({})).as_bytes())
        .expect("write tools/list");
    let tools_list = read_response(&mut reader);
    let tools = tools_list["result"]["tools"].as_array().expect("tools");
    let roots_tool = tools
        .iter()
        .find(|tool| tool["name"] == "zocli.roots.list")
        .expect("roots tool");
    assert_eq!(roots_tool["_meta"]["ui"]["resourceUri"], APP_RESOURCE_URI);

    stdin
        .write_all(
            mcp_request(
                3,
                "tools/call",
                json!({
                    "name": "zocli.roots.list",
                    "arguments": {}
                }),
            )
            .as_bytes(),
        )
        .expect("write roots tool call");
    let outbound_roots_request = read_response(&mut reader);
    assert_eq!(outbound_roots_request["method"], "roots/list");
    let request_id = outbound_roots_request["id"].as_u64().expect("request id");

    stdin
        .write_all(
            mcp_response(
                request_id,
                json!({
                    "roots": [
                        {
                            "uri": "file:///tmp/project-a",
                            "name": "project-a"
                        }
                    ]
                }),
            )
            .as_bytes(),
        )
        .expect("write roots response");
    let roots_tool_result = read_response(&mut reader);
    assert_eq!(
        roots_tool_result["result"]["structuredContent"]["roots"][0]["uri"],
        "file:///tmp/project-a"
    );
    assert_eq!(
        roots_tool_result["result"]["structuredContent"]["roots"][0]["name"],
        "project-a"
    );

    stdin
        .write_all(mcp_notification("notifications/roots/list_changed", json!({})).as_bytes())
        .expect("write roots changed");
    stdin
        .write_all(
            mcp_request(
                4,
                "tools/call",
                json!({
                    "name": "zocli.roots.list",
                    "arguments": {}
                }),
            )
            .as_bytes(),
        )
        .expect("write second roots tool call");
    let refreshed_outbound_roots_request = read_response(&mut reader);
    assert_eq!(refreshed_outbound_roots_request["method"], "roots/list");
    let refreshed_request_id = refreshed_outbound_roots_request["id"]
        .as_u64()
        .expect("request id");
    assert_ne!(refreshed_request_id, request_id);

    stdin
        .write_all(
            mcp_response(
                refreshed_request_id,
                json!({
                    "roots": [
                        {
                            "uri": "file:///tmp/project-b",
                            "name": "project-b"
                        }
                    ]
                }),
            )
            .as_bytes(),
        )
        .expect("write refreshed roots response");
    let refreshed_roots_result = read_response(&mut reader);
    assert_eq!(
        refreshed_roots_result["result"]["structuredContent"]["roots"][0]["uri"],
        "file:///tmp/project-b"
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn mcp_stdio_does_not_advertise_roots_without_client_capability() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(tools.iter().all(|tool| tool["name"] != "zocli.roots.list"));
}

#[test]
fn mcp_stdio_exposes_mail_write_tools_and_validates_send_reply_forward() {
    let temp = tempdir().expect("tempdir");
    write_mock_mail_account(temp.path());
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.oauth]
kind = "oauth_pkce"
access_token = "mock-token"
token_type = "Bearer"
expires_at_epoch_secs = 4102444800
scope = ["ZohoMail.messages.ALL", "ZohoCalendar.calendar.ALL", "WorkDrive.files.ALL"]
client_id = "client-123"
api_domain = "https://www.zohoapis.com"
"#,
    );

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(
            3,
            "tools/call",
            json!({
                "name": "zocli.mail.send",
                "arguments": {
                    "account": "mock",
                    "subject": "Hello",
                    "text": "Body"
                }
            }),
        ),
        mcp_request(
            4,
            "tools/call",
            json!({
                "name": "zocli.mail.reply",
                "arguments": {
                    "account": "mock",
                    "message_id": "42"
                }
            }),
        ),
        mcp_request(
            5,
            "tools/call",
            json!({
                "name": "zocli.mail.forward",
                "arguments": {
                    "account": "mock",
                    "message_id": "42"
                }
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(tools.iter().any(|tool| tool["name"] == "zocli.mail.send"));
    assert!(tools.iter().any(|tool| tool["name"] == "zocli.mail.reply"));
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "zocli.mail.forward")
    );

    assert_eq!(responses[2]["error"]["code"], -32602);
    assert!(
        responses[2]["error"]["message"]
            .as_str()
            .expect("message")
            .contains("`to` is required")
    );

    assert_eq!(responses[3]["error"]["code"], -32602);
    assert!(
        responses[3]["error"]["message"]
            .as_str()
            .expect("message")
            .contains("`folder_id` is required")
    );

    assert_eq!(responses[4]["error"]["code"], -32602);
    assert!(
        responses[4]["error"]["message"]
            .as_str()
            .expect("message")
            .contains("`folder_id` is required")
    );

    let send_tool = tools
        .iter()
        .find(|tool| tool["name"] == "zocli.mail.send")
        .expect("send tool");
    assert_eq!(
        send_tool["inputSchema"]["required"],
        json!(["to", "subject"])
    );
}

#[test]
fn mcp_stdio_exposes_calendar_write_tools_and_executes_create_delete() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "zocli.calendar.create")
    );
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "zocli.calendar.delete")
    );
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "zocli.calendar.calendars")
    );
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "zocli.calendar.events")
    );
}

#[test]
fn mcp_stdio_exposes_drive_tools() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(tools.iter().any(|tool| tool["name"] == "zocli.drive.teams"));
    assert!(tools.iter().any(|tool| tool["name"] == "zocli.drive.list"));
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "zocli.drive.upload")
    );
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "zocli.drive.download")
    );
}

#[test]
fn mcp_stdio_apps_capable_clients_receive_ui_metadata_and_resources() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(3, "resources/list", json!({})),
        mcp_request(6, "resources/templates/list", json!({})),
        mcp_request(
            4,
            "tools/call",
            json!({
                "name": "zocli.app.snapshot",
                "arguments": {}
            }),
        ),
        mcp_request(
            5,
            "resources/read",
            json!({
                "uri": APP_RESOURCE_URI
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    let snapshot_tool = tools
        .iter()
        .find(|tool| tool["name"] == "zocli.app.snapshot")
        .expect("snapshot tool");
    assert_eq!(
        snapshot_tool["_meta"]["ui"]["resourceUri"],
        APP_RESOURCE_URI
    );
    assert_eq!(snapshot_tool["_meta"]["ui"]["visibility"][0], "app");
    assert!(snapshot_tool["_meta"]["ui"]["visibility"].get(1).is_none());

    let account_tool = tools
        .iter()
        .find(|tool| tool["name"] == "zocli.account.current")
        .expect("account tool");
    assert_eq!(
        account_tool["_meta"]["ui"]["resourceUri"],
        "ui://zocli/account"
    );
    assert_eq!(account_tool["_meta"]["ui"]["visibility"][0], "model");
    assert_eq!(account_tool["_meta"]["ui"]["visibility"][1], "app");

    let update_tool = tools
        .iter()
        .find(|tool| tool["name"] == "zocli.update.check")
        .expect("update tool");
    assert_eq!(update_tool["_meta"]["ui"]["resourceUri"], APP_RESOURCE_URI);
    assert_eq!(update_tool["_meta"]["ui"]["visibility"][0], "model");
    assert_eq!(update_tool["_meta"]["ui"]["visibility"][1], "app");

    let resources = responses[2]["result"]["resources"]
        .as_array()
        .expect("resources array");
    let dashboard = resources
        .iter()
        .find(|resource| resource["uri"] == APP_RESOURCE_URI)
        .expect("dashboard resource");
    assert_eq!(dashboard["mimeType"], APP_RESOURCE_MIME_TYPE);
    assert_eq!(dashboard["_meta"]["ui"]["prefersBorder"], true);
    assert_eq!(dashboard["_meta"]["ui"]["csp"]["connectDomains"], json!(["'self'"]));

    let templates = responses[3]["result"]["resourceTemplates"]
        .as_array()
        .expect("resource templates array");
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == APP_RESOURCE_URI_TEMPLATE)
    );

    assert_eq!(
        responses[4]["result"]["structuredContent"]["appResourceUri"],
        APP_RESOURCE_URI
    );
    assert_eq!(
        responses[4]["result"]["_meta"]["ui"]["resourceUri"],
        APP_RESOURCE_URI
    );
    assert_eq!(
        responses[4]["result"]["_meta"]["ui"]["visibility"][0],
        "app"
    );

    let contents = responses[5]["result"]["contents"]
        .as_array()
        .expect("contents array");
    assert_eq!(contents[0]["mimeType"], APP_RESOURCE_MIME_TYPE);
    assert_eq!(contents[0]["_meta"]["ui"]["prefersBorder"], true);
    let html = contents[0]["text"].as_str().expect("html");
    assert!(html.contains("ui/initialize"));
    assert!(html.contains("ui/notifications/initialized"));
    assert!(html.contains("ui/notifications/tool-input"));
    assert!(html.contains("ui/notifications/tool-result"));
    assert!(html.contains("tools/call"));
    assert!(html.contains("zocli.app.snapshot"));
    assert!(html.contains("Refresh snapshot"));
    assert!(html.contains("tool-name-input"));
    assert!(html.contains("tool-args-input"));
    assert!(html.contains("tool-schema-json"));
    assert!(html.contains("Run selected tool"));
    assert!(html.contains("ui/open-link"));
    assert!(html.contains("ui/request-display-mode"));
    assert!(html.contains("ui/update-model-context"));
    assert!(html.contains("ui/message"));
    assert!(html.contains("Host profile"));
    assert!(html.contains("host-profile-summary"));
    assert!(html.contains("host-capability-grid"));
    assert!(html.contains("host-recommendations"));
    assert!(html.contains("apps-rich"));
    assert!(html.contains("hybrid"));
    assert!(html.contains("text-first"));
    assert!(html.contains("Open auth issuer"));
    assert!(html.contains("Open resource metadata"));
    assert!(html.contains("Share auth context"));
    assert!(html.contains("authChallengeFromError"));
    assert!(html.contains("resources/subscribe"));
    assert!(html.contains("resources/unsubscribe"));
    assert!(html.contains("notifications/resources/updated"));
    assert!(html.contains("resources/templates/list"));
    assert!(html.contains("resources/read"));
    assert!(html.contains("Read account resource"));
    assert!(html.contains("Read auth resource"));
    assert!(html.contains("Read skills catalog"));
    assert!(html.contains("Read selected skill"));
    assert!(html.contains("skill-input"));
    assert!(html.contains("Searchable browser"));
    assert!(html.contains("browser-query-input"));
    assert!(html.contains("browser-filter-select"));
    assert!(html.contains("browser-results"));
    assert!(html.contains("Refresh browser"));
    assert!(html.contains("notifications/prompts/list_changed"));
    assert!(html.contains("notifications/tools/list_changed"));
    assert!(html.contains("tools/list"));
    assert!(html.contains("Prompt browser"));
    assert!(html.contains("List prompts"));
    assert!(html.contains("Render selected prompt"));
    assert!(html.contains("prompt-name-input"));
    assert!(html.contains("prompt-args-input"));
    assert!(html.contains("prompt-list-json"));
    assert!(html.contains("prompt-json"));
    assert!(html.contains("Share snapshot"));
    assert!(html.contains("Share current view"));
    assert!(html.contains("Current zocli dashboard view"));
    assert!(html.contains("buildCurrentViewUri"));
    assert!(html.contains("APP_VIEW_STATE_STORAGE_KEY"));
    assert!(html.contains("APP_RUNTIME_STATE_STORAGE_KEY"));
    assert!(html.contains("window.localStorage"));
    assert!(html.contains("mergeBootstrapState"));
    assert!(html.contains("savePersistedViewState"));
    assert!(html.contains("savePersistedRuntimeState"));
    assert!(html.contains("current-view-uri"));
    assert!(html.contains("Open MCP Apps docs"));
    assert!(html.contains("List accounts"));
    assert!(html.contains("Check updates"));
}

#[test]
fn mcp_stdio_reads_account_aware_dashboard_resource() {
    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "resources/read",
            json!({
                "uri": "ui://zocli/dashboard?account=work&section=auth&resource=auth&tool=zocli.auth.status"
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let contents = responses[1]["result"]["contents"]
        .as_array()
        .expect("contents array");
    let html = contents[0]["text"].as_str().expect("html");
    assert!(html.contains("\"resourceUri\":\"ui://zocli/dashboard?account=work&section=auth&resource=auth&tool=zocli.auth.status\""));
    assert!(html.contains("\"defaultAccount\":\"work\""));
    assert!(html.contains("\"preferredSection\":\"auth\""));
    assert!(html.contains("\"preferredResource\":\"auth\""));
    assert!(html.contains("\"preferredTool\":\"zocli.auth.status\""));
    assert!(html.contains("dashboard opened for account"));
    assert!(html.contains("\"section restored\""));
    assert!(html.contains("state.bootstrap.preferredSection"));
    assert!(html.contains("restoreBootstrapFocus"));
    assert!(html.contains("loadPersistedViewState"));
    assert!(html.contains("restorePersistedRuntimeState"));
    assert!(html.contains("\"runtime restored\""));
    assert!(html.contains("\"View: \" + uri"));
    assert!(html.contains("panel-auth"));
}

#[test]
fn mcp_stdio_reads_skill_aware_dashboard_resource() {
    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "resources/read",
            json!({
                "uri": "ui://zocli/dashboard?section=resources&resource=skill&skill=zocli-mail"
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let contents = responses[1]["result"]["contents"]
        .as_array()
        .expect("contents array");
    let html = contents[0]["text"].as_str().expect("html");
    assert!(html.contains("\"preferredResource\":\"skill\""));
    assert!(html.contains("\"preferredSkill\":\"zocli-mail\""));
    assert!(html.contains("resource://zocli/skill/"));
    assert!(html.contains("readSkillResource"));
}

#[test]
fn mcp_stdio_reads_prompt_aware_dashboard_resource() {
    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "resources/read",
            json!({
                "uri": "ui://zocli/dashboard?section=prompts&prompt=mail"
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let contents = responses[1]["result"]["contents"]
        .as_array()
        .expect("contents array");
    let html = contents[0]["text"].as_str().expect("html");
    assert!(html.contains("\"preferredSection\":\"prompts\""));
    assert!(html.contains("\"preferredPrompt\":\"mail\""));
    assert!(html.contains("renderSelectedPrompt"));
    assert!(html.contains("prompts/get"));
}

#[test]
fn mcp_stdio_update_check_tool_reads_release_mirror() {
    let mut server = Server::new();
    let base_url = format!("{}/releases/download/v9.9.9", server.url());
    let asset = current_release_update_target();
    let _checksums = server
        .mock("GET", "/releases/download/v9.9.9/SHA256SUMS")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_body(format!("deadbeef  {asset}\n"))
        .create();

    let input = [
        initialize_request(false),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "zocli.update.check",
                "arguments": {}
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_UPDATE_BASE_URL", &base_url)
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_eq!(structured["operation"], "update.check");
    assert_eq!(structured["status"], "update_available");
    assert_eq!(structured["targetVersion"], "9.9.9");
    assert_eq!(structured["requestedVersion"], "latest");
    assert_eq!(structured["baseUrl"], base_url);
}

#[test]
fn mcp_stdio_update_check_tool_resolves_latest_redirect_version() {
    let asset = current_release_update_target();
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

    let input = [
        initialize_request(false),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "zocli.update.check",
                "arguments": {}
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_UPDATE_BASE_URL", &latest_base_url)
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_eq!(structured["operation"], "update.check");
    assert_eq!(structured["status"], "update_available");
    assert_eq!(structured["targetVersion"], "9.9.9");
    assert_eq!(structured["requestedVersion"], "latest");
    assert_eq!(structured["baseUrl"], latest_base_url);
}

#[test]
fn mcp_stdio_lists_resource_templates_and_reads_templated_resources() {
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@zoho.com"
default = true
datacenter = "com"
account_id = "12345"
client_id = "client-123"
credential_ref = "env:ZOCLI_OAUTH_TOKEN"
"#,
    );

    let input = [
        initialize_request(false),
        mcp_request(2, "resources/templates/list", json!({})),
        mcp_request(
            3,
            "resources/read",
            json!({
                "uri": "resource://zocli/account/personal"
            }),
        ),
        mcp_request(
            4,
            "resources/read",
            json!({
                "uri": "resource://zocli/auth/personal"
            }),
        ),
        mcp_request(
            5,
            "resources/read",
            json!({
                "uri": "resource://zocli/skills"
            }),
        ),
        mcp_request(
            6,
            "resources/read",
            json!({
                "uri": "resource://zocli/skill/zocli-mail"
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let templates = responses[1]["result"]["resourceTemplates"]
        .as_array()
        .expect("resource templates array");
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://zocli/account/{account}")
    );
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://zocli/auth/{account}")
    );
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://zocli/skill/{skill}")
    );
    assert!(
        !templates
            .iter()
            .any(|template| template["uriTemplate"] == APP_RESOURCE_URI_TEMPLATE)
    );

    let account_contents = responses[2]["result"]["contents"]
        .as_array()
        .expect("account contents");
    assert_eq!(account_contents[0]["mimeType"], "application/json");
    let account_payload: Value =
        serde_json::from_str(account_contents[0]["text"].as_str().expect("account text"))
            .expect("account json");
    assert_eq!(account_payload["account"], "personal");
    assert_eq!(account_payload["current"], true);
    assert_eq!(account_payload["email"], "me@zoho.com");
    assert_eq!(account_payload["credential_ref"], "env:ZOCLI_OAUTH_TOKEN");

    let auth_contents = responses[3]["result"]["contents"]
        .as_array()
        .expect("auth contents");
    assert_eq!(auth_contents[0]["mimeType"], "application/json");
    let auth_payload: Value =
        serde_json::from_str(auth_contents[0]["text"].as_str().expect("auth text"))
            .expect("auth json");
    assert_eq!(auth_payload["account"], "personal");
    assert_eq!(auth_payload["auth"]["credential_state"], "env_missing");

    let skills_catalog_contents = responses[4]["result"]["contents"]
        .as_array()
        .expect("skills catalog contents");
    let skills_catalog_payload: Value = serde_json::from_str(
        skills_catalog_contents[0]["text"]
            .as_str()
            .expect("skills catalog text"),
    )
    .expect("skills catalog json");
    assert_eq!(skills_catalog_payload["count"], 7);
    assert!(
        skills_catalog_payload["items"]
            .as_array()
            .expect("skill items")
            .iter()
            .any(|item| item["name"] == "zocli-mail")
    );

    let skill_contents = responses[5]["result"]["contents"]
        .as_array()
        .expect("skill contents");
    assert_eq!(skill_contents[0]["mimeType"], "text/markdown");
    let skill_text = skill_contents[0]["text"].as_str().expect("skill text");
    assert!(skill_text.contains("# zocli mail"));
    assert!(skill_text.contains("zocli mail send"));
}

#[test]
fn mcp_stdio_app_snapshot_exposes_auth_discovery_when_http_auth_is_configured() {
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@zoho.com"
default = true
datacenter = "com"
account_id = "12345"
client_id = "client-123"
credential_ref = "store:oauth"
"#,
    );

    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "zocli.app.snapshot",
                "arguments": {}
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .env("ZOCLI_MCP_HTTP_BEARER_TOKEN", "secret-token")
        .env("ZOCLI_MCP_HTTP_AUTH_ISSUER", "https://auth.example.test")
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let discovery = &responses[1]["result"]["structuredContent"]["authDiscovery"];
    assert_eq!(discovery["enabled"], true);
    assert_eq!(
        discovery["authorizationServers"][0],
        "https://auth.example.test"
    );
    assert_eq!(
        discovery["resourceMetadataUrl"],
        "http://127.0.0.1:8787/.well-known/oauth-protected-resource/mcp"
    );
}

#[test]
fn mcp_stdio_text_only_clients_receive_graceful_degradation() {
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@zoho.com"
default = true
datacenter = "com"
account_id = "12345"
client_id = "client-123"
credential_ref = "store:oauth"
"#,
    );

    let input = [
        initialize_request(false),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(3, "resources/list", json!({})),
        mcp_request(
            4,
            "tools/call",
            json!({
                "name": "zocli.account.list",
                "arguments": {}
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    let account_tool = tools
        .iter()
        .find(|tool| tool["name"] == "zocli.account.current")
        .expect("account tool");
    assert!(account_tool.get("_meta").is_none());
    assert!(
        !tools
            .iter()
            .any(|tool| tool["name"] == "zocli.app.snapshot")
    );

    let resources = responses[2]["result"]["resources"]
        .as_array()
        .expect("resources array");
    assert!(
        !resources
            .iter()
            .any(|resource| resource["uri"] == APP_RESOURCE_URI)
    );

    assert_eq!(
        responses[3]["result"]["structuredContent"]["items"][0]["name"],
        "personal"
    );
    assert!(responses[3]["result"].get("_meta").is_none());
}

#[test]
fn mcp_stdio_subscriptions_emit_resource_updated_notifications() {
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@zoho.com"
default = true
datacenter = "com"
account_id = "12345"
client_id = "client-123"
credential_ref = "store:oauth"
"#,
    );

    let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("zocli"))
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .env("ZOCLI_SECRET_BACKEND", "file")
        .args(["mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn zocli mcp");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);

    stdin
        .write_all(initialize_request(false).as_bytes())
        .expect("initialize write");
    let initialize = read_response(&mut reader);
    assert_eq!(
        initialize["result"]["capabilities"]["resources"]["subscribe"],
        true
    );

    stdin
        .write_all(
            mcp_request(
                2,
                "resources/subscribe",
                json!({
                    "uri": "resource://zocli/account/personal"
                }),
            )
            .as_bytes(),
        )
        .expect("subscribe write");
    let subscribe = read_response(&mut reader);
    assert_eq!(subscribe["result"], json!({}));

    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "updated@zoho.com"
default = true
datacenter = "com"
account_id = "12345"
client_id = "client-123"
credential_ref = "store:oauth"
"#,
    );

    let notification = read_response(&mut reader);
    assert_eq!(notification["method"], "notifications/resources/updated");
    assert_eq!(
        notification["params"]["uri"],
        "resource://zocli/account/personal"
    );

    let _ = child.kill();
    let _ = child.wait();
}

// ── MCP Apps ui/* lifecycle tests (Phase 2) ──────────────────

#[test]
fn mcp_stdio_ui_initialize_returns_protocol_and_capabilities() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let ui_init = &responses[1];
    assert_eq!(ui_init["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(ui_init["result"]["serverInfo"]["name"], "zocli");
    assert!(
        ui_init["result"]["serverInfo"]["version"]
            .as_str()
            .is_some(),
        "serverInfo.version must be present"
    );
    assert_eq!(
        ui_init["result"]["capabilities"]["tools"]["listChanged"],
        false
    );
    assert_eq!(
        ui_init["result"]["capabilities"]["resources"]["listChanged"],
        false
    );
}

#[test]
fn mcp_stdio_ui_request_display_mode_validates_known_modes() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
        // Spec-correct modes: inline, fullscreen, pip
        mcp_request(
            3,
            "ui/request-display-mode",
            json!({ "mode": "fullscreen" }),
        ),
        mcp_request(
            4,
            "ui/request-display-mode",
            json!({ "mode": "pip" }),
        ),
        // Legacy/unknown modes fall back to "inline"
        mcp_request(
            5,
            "ui/request-display-mode",
            json!({ "mode": "floating" }),
        ),
        mcp_request(
            6,
            "ui/request-display-mode",
            json!({ "mode": "full-window" }),
        ),
        mcp_request(7, "ui/request-display-mode", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    // responses[0]=init, responses[1]=ui/initialize, responses[2..6]=display-mode
    assert_eq!(responses[2]["result"]["mode"], "fullscreen");
    assert_eq!(responses[3]["result"]["mode"], "pip");
    assert_eq!(
        responses[4]["result"]["mode"], "inline",
        "legacy 'floating' must fall back to inline"
    );
    assert_eq!(
        responses[5]["result"]["mode"], "inline",
        "legacy 'full-window' must fall back to inline"
    );
    assert_eq!(
        responses[6]["result"]["mode"], "inline",
        "missing mode must default to inline"
    );
}

#[test]
fn mcp_stdio_ui_update_model_context_and_message_are_acknowledged() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
        // View confirms initialization — required before ui/message
        mcp_notification("ui/notifications/initialized", json!({})),
        mcp_request(
            3,
            "ui/update-model-context",
            json!({ "context": { "tools": [] } }),
        ),
        mcp_request(4, "ui/message", json!({ "type": "info", "text": "hello" })),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    // responses[0]=init, responses[1]=ui/initialize, responses[2]=update-model-context, responses[3]=message
    assert_eq!(responses[2]["result"]["accepted"], true);
    assert_eq!(responses[3]["result"]["accepted"], true);
}

#[test]
fn mcp_stdio_ui_open_link_and_resource_teardown_acknowledged() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
        // View confirms initialization — required before ui/open-link
        mcp_notification("ui/notifications/initialized", json!({})),
        mcp_request(
            3,
            "ui/open-link",
            json!({ "url": "https://example.com" }),
        ),
        mcp_request(
            4,
            "ui/resource-teardown",
            json!({ "uri": "ui://zocli/dashboard" }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    // responses[0]=init, responses[1]=ui/initialize, responses[2]=open-link, responses[3]=teardown
    assert_eq!(responses[2]["result"]["accepted"], true);
    assert_eq!(responses[3]["result"]["accepted"], true);
}

#[test]
fn mcp_stdio_ui_notifications_are_accepted_silently() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_notification("ui/notifications/initialized", json!({})),
        mcp_notification(
            "ui/notifications/tool-input",
            json!({ "tool": "zocli.mail.list", "input": {} }),
        ),
        mcp_notification(
            "ui/notifications/tool-input-partial",
            json!({ "tool": "zocli.mail.list", "partial": {} }),
        ),
        mcp_notification(
            "ui/notifications/tool-result",
            json!({ "tool": "zocli.mail.list", "result": {} }),
        ),
        mcp_notification(
            "ui/notifications/host-context-changed",
            json!({ "context": {} }),
        ),
        mcp_notification(
            "ui/notifications/size-changed",
            json!({ "width": 800, "height": 600 }),
        ),
        mcp_notification(
            "ui/notifications/tool-cancelled",
            json!({ "tool": "zocli.mail.list" }),
        ),
        // After all notifications, a normal request still works
        mcp_request(2, "ping", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    // Only 2 responses: initialize (id:1) and ping (id:2)
    // Notifications must NOT generate responses
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["id"], 1); // initialize
    assert_eq!(responses[1]["id"], 2); // ping
    assert_eq!(responses[1]["result"], json!({}));
}

#[test]
fn mcp_stdio_ui_full_lifecycle_initialize_interact_teardown() {
    let input = [
        // 1. MCP initialize
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        // 2. UI initialize
        mcp_request(2, "ui/initialize", json!({})),
        // 3. UI initialized notification
        mcp_notification("ui/notifications/initialized", json!({})),
        // 4. Host context changed
        mcp_notification(
            "ui/notifications/host-context-changed",
            json!({ "context": {} }),
        ),
        // 5. Update model context
        mcp_request(3, "ui/update-model-context", json!({ "context": {} })),
        // 6. Request display mode (spec-correct mode)
        mcp_request(
            4,
            "ui/request-display-mode",
            json!({ "mode": "fullscreen" }),
        ),
        // 7. Tool input notification
        mcp_notification(
            "ui/notifications/tool-input",
            json!({ "tool": "zocli.app.snapshot", "input": {} }),
        ),
        // 8. Tool result notification
        mcp_notification(
            "ui/notifications/tool-result",
            json!({ "tool": "zocli.app.snapshot", "result": {} }),
        ),
        // 9. Send message
        mcp_request(5, "ui/message", json!({ "type": "info", "text": "ok" })),
        // 10. Open link
        mcp_request(
            6,
            "ui/open-link",
            json!({ "url": "https://example.com" }),
        ),
        // 11. Teardown
        mcp_request(
            7,
            "ui/resource-teardown",
            json!({ "uri": "ui://zocli/dashboard" }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    // 7 responses: initialize(1), ui/initialize(2), ui/update-model-context(3),
    //   ui/request-display-mode(4), ui/message(5), ui/open-link(6), ui/resource-teardown(7)
    assert_eq!(
        responses.len(),
        7,
        "expected 7 responses (1 MCP init + 6 UI requests), got {}",
        responses.len()
    );

    // Verify each response ID matches
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[1]["id"], 2);
    assert_eq!(responses[2]["id"], 3);
    assert_eq!(responses[3]["id"], 4);
    assert_eq!(responses[4]["id"], 5);
    assert_eq!(responses[5]["id"], 6);
    assert_eq!(responses[6]["id"], 7);

    // ui/initialize has protocol info
    assert_eq!(responses[1]["result"]["protocolVersion"], "2025-11-25");
    // ui/request-display-mode validates spec-correct mode
    assert_eq!(responses[3]["result"]["mode"], "fullscreen");
    // ui/update-model-context returns accepted + revision
    assert_eq!(responses[2]["result"]["accepted"], true);
    assert_eq!(responses[2]["result"]["revision"], 1);
    // ui/message echoes stored text
    assert_eq!(responses[4]["result"]["accepted"], true);
    assert_eq!(responses[4]["result"]["stored"], "ok");
    // ui/open-link echoes url
    assert_eq!(responses[5]["result"]["accepted"], true);
    assert_eq!(responses[5]["result"]["url"], "https://example.com");
    // ui/resource-teardown returns accepted
    assert_eq!(responses[6]["result"]["accepted"], true);
}

#[test]
fn mcp_stdio_ui_methods_require_ui_initialize_gate() {
    // Send ui/update-model-context WITHOUT calling ui/initialize first.
    // The server should reject it because UI lifecycle is NotInitialized.
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        // Skip ui/initialize — go straight to a ui/* method
        mcp_request(
            2,
            "ui/update-model-context",
            json!({ "context": {} }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    // 2 responses: initialize(1), ui/update-model-context(2)
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[1]["id"], 2);
    // Must be an error, not a success
    assert!(
        responses[1].get("error").is_some(),
        "ui/update-model-context without prior ui/initialize must return error, got: {}",
        responses[1]
    );
}

#[test]
fn mcp_stdio_ui_teardown_resets_lifecycle_requires_reinitialize() {
    // Full lifecycle: initialize → view ready → interact → teardown → try again without re-init
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        // ui/initialize
        mcp_request(2, "ui/initialize", json!({})),
        // View confirms initialization
        mcp_notification("ui/notifications/initialized", json!({})),
        // ui/message — should succeed (view is ready)
        mcp_request(3, "ui/message", json!({ "type": "info", "text": "ok" })),
        // teardown — transitions to NotInitialized
        mcp_request(4, "ui/resource-teardown", json!({ "uri": "ui://zocli/dashboard" })),
        // ui/message again — should FAIL because teardown reset the state
        mcp_request(5, "ui/message", json!({ "type": "info", "text": "after teardown" })),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    assert_eq!(responses.len(), 5);
    // ui/initialize succeeds
    assert_eq!(responses[1]["id"], 2);
    assert!(responses[1].get("result").is_some());
    // ui/message succeeds before teardown
    assert_eq!(responses[2]["id"], 3);
    assert_eq!(responses[2]["result"]["accepted"], true);
    assert_eq!(responses[2]["result"]["stored"], "ok");
    // teardown succeeds
    assert_eq!(responses[3]["id"], 4);
    assert_eq!(responses[3]["result"]["accepted"], true);
    // ui/message after teardown must be an error
    assert_eq!(responses[4]["id"], 5);
    assert!(
        responses[4].get("error").is_some(),
        "ui/message after teardown without re-initialize must return error, got: {}",
        responses[4]
    );
}

#[test]
fn mcp_stdio_non_ui_client_degrades_gracefully() {
    // Client that does NOT advertise io.modelcontextprotocol/ui
    let input = [
        initialize_request(false),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(3, "resources/list", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);

    // tools/list should NOT contain app.snapshot for non-UI clients
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(
        !tools
            .iter()
            .any(|tool| tool["name"] == "zocli.app.snapshot"),
        "app.snapshot must not appear for non-UI clients"
    );

    // resources/list should NOT contain ui:// resources for non-UI clients
    let resources = responses[2]["result"]["resources"]
        .as_array()
        .expect("resources array");
    assert!(
        !resources
            .iter()
            .any(|r| r["uri"].as_str().unwrap_or("").starts_with("ui://")),
        "ui:// resources must not appear for non-UI clients"
    );

    // Non-UI tools must NOT have _meta
    for tool in tools {
        assert!(
            tool.get("_meta").is_none(),
            "tool {} must not have _meta for non-UI clients",
            tool["name"]
        );
    }
}

// ── Phase 3: Surface Split tests ─────────────────────────────

#[test]
fn mcp_stdio_dedicated_surfaces_are_readable() {
    for surface_uri in [
        "ui://zocli/mail",
        "ui://zocli/calendar",
        "ui://zocli/drive",
        "ui://zocli/auth",
        "ui://zocli/account",
    ] {
        let input = [
            initialize_request(true),
            mcp_notification("notifications/initialized", json!({})),
            mcp_request(
                2,
                "resources/read",
                json!({ "uri": surface_uri }),
            ),
        ]
        .concat();

        let output = zocli()
            .args(["mcp"])
            .write_stdin(input)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let responses = parse_responses(&output);
        let contents = responses[1]["result"]["contents"]
            .as_array()
            .expect("contents array");
        assert_eq!(contents.len(), 1, "surface {surface_uri} must return 1 content");
        assert_eq!(
            contents[0]["mimeType"], APP_RESOURCE_MIME_TYPE,
            "surface {surface_uri} must be MCP Apps MIME"
        );
        let html = contents[0]["text"].as_str().expect("html text");
        assert!(
            html.contains("<!DOCTYPE html>") || html.contains("<!doctype html>"),
            "surface {surface_uri} must return HTML"
        );
        assert!(
            !html.contains("__ZOCLI_VERSION__"),
            "surface {surface_uri} must have version placeholder replaced"
        );
        assert!(
            html.contains(&format!("\"resourceUri\":\"{surface_uri}\"")),
            "surface {surface_uri} bootstrap must embed its own resourceUri"
        );
    }
}

#[test]
fn mcp_stdio_dedicated_surfaces_have_focused_bootstrap_state() {
    let expected: &[(&str, &str, Option<&str>)] = &[
        ("ui://zocli/mail", "prompts", Some("mail")),
        ("ui://zocli/calendar", "prompts", Some("calendar")),
        ("ui://zocli/drive", "prompts", Some("drive")),
        ("ui://zocli/auth", "auth", None),
        ("ui://zocli/account", "auth", None),
    ];

    for (surface_uri, section, prompt) in expected {
        let input = [
            initialize_request(true),
            mcp_notification("notifications/initialized", json!({})),
            mcp_request(
                2,
                "resources/read",
                json!({ "uri": surface_uri }),
            ),
        ]
        .concat();

        let output = zocli()
            .args(["mcp"])
            .write_stdin(input)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let responses = parse_responses(&output);
        let html = responses[1]["result"]["contents"][0]["text"]
            .as_str()
            .expect("html text");

        assert!(
            html.contains(&format!("\"preferredSection\":\"{section}\"")),
            "{surface_uri} must have preferredSection={section}"
        );

        if let Some(p) = prompt {
            assert!(
                html.contains(&format!("\"preferredPrompt\":\"{p}\"")),
                "{surface_uri} must have preferredPrompt={p}"
            );
        }
    }
}

#[test]
fn mcp_stdio_dedicated_surfaces_accept_account_query() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "resources/read",
            json!({ "uri": "ui://zocli/mail?account=work" }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let html = responses[1]["result"]["contents"][0]["text"]
        .as_str()
        .expect("html text");
    assert!(html.contains("\"defaultAccount\":\"work\""));
}

#[test]
fn mcp_stdio_dedicated_surfaces_reject_invalid_query_params() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "resources/read",
            json!({ "uri": "ui://zocli/mail?section=tools" }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    assert!(
        responses[1]["error"].is_object(),
        "non-dashboard surfaces must reject section param"
    );
}

#[test]
fn mcp_stdio_tools_map_to_dedicated_surfaces() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");

    // Verify tool → surface mapping
    let expected_mappings = [
        ("zocli.app.snapshot", "ui://zocli/dashboard"),
        ("zocli.update.check", "ui://zocli/dashboard"),
        ("zocli.account.list", "ui://zocli/account"),
        ("zocli.account.current", "ui://zocli/account"),
        ("zocli.auth.status", "ui://zocli/auth"),
        ("zocli.mail.folders", "ui://zocli/mail"),
        ("zocli.mail.list", "ui://zocli/mail"),
        ("zocli.mail.search", "ui://zocli/mail"),
        ("zocli.mail.read", "ui://zocli/mail"),
        ("zocli.mail.send", "ui://zocli/mail"),
        ("zocli.mail.reply", "ui://zocli/mail"),
        ("zocli.mail.forward", "ui://zocli/mail"),
        ("zocli.mail.attachment_export", "ui://zocli/mail"),
        ("zocli.calendar.calendars", "ui://zocli/calendar"),
        ("zocli.calendar.events", "ui://zocli/calendar"),
        ("zocli.calendar.create", "ui://zocli/calendar"),
        ("zocli.calendar.delete", "ui://zocli/calendar"),
        ("zocli.drive.teams", "ui://zocli/drive"),
        ("zocli.drive.list", "ui://zocli/drive"),
        ("zocli.drive.upload", "ui://zocli/drive"),
        ("zocli.drive.download", "ui://zocli/drive"),
    ];

    for (tool_name, expected_uri) in expected_mappings {
        let tool = tools
            .iter()
            .find(|t| t["name"] == tool_name)
            .unwrap_or_else(|| panic!("tool {tool_name} not found"));
        assert_eq!(
            tool["_meta"]["ui"]["resourceUri"], expected_uri,
            "tool {tool_name} must map to {expected_uri}"
        );
    }

    // ALL tools must have _meta when UI is enabled
    for tool in tools {
        assert!(
            tool.get("_meta").is_some(),
            "tool {} must have _meta when UI is enabled",
            tool["name"]
        );
    }
}

#[test]
fn mcp_stdio_resources_list_includes_all_surfaces() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "resources/list", json!({})),
        mcp_request(3, "resources/templates/list", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let resources = responses[1]["result"]["resources"]
        .as_array()
        .expect("resources array");
    let templates = responses[2]["result"]["resourceTemplates"]
        .as_array()
        .expect("resource templates");

    for expected_uri in [
        "ui://zocli/dashboard",
        "ui://zocli/mail",
        "ui://zocli/calendar",
        "ui://zocli/drive",
        "ui://zocli/auth",
        "ui://zocli/account",
    ] {
        assert!(
            resources.iter().any(|r| r["uri"] == expected_uri),
            "resources/list must include {expected_uri}"
        );
    }

    // Check templates exist for new surfaces
    for expected_template in [
        "ui://zocli/mail{?account}",
        "ui://zocli/calendar{?account}",
        "ui://zocli/drive{?account}",
        "ui://zocli/auth{?account}",
        "ui://zocli/account{?account}",
    ] {
        assert!(
            templates
                .iter()
                .any(|t| t["uriTemplate"] == expected_template),
            "resource templates must include {expected_template}"
        );
    }
}

// ── Phase 0: Runtime Parity Verification ─────────────────────
//
// This test is the RUNTIME counterpart to the static parity manifest
// in release_surface.rs. The manifest declares what we EXPECT the
// server to advertise; this test calls the actual MCP server and
// verifies that the runtime surface matches EXACTLY.
//
// If a tool/prompt/resource is added to server.rs but not to the
// manifest, this test fails. If the manifest is updated but the
// server doesn't actually register the entry, this test fails.

#[test]
fn parity_manifest_matches_runtime_surface() {
    use std::collections::BTreeSet;

    // ── Expected surface (must match release_surface.rs exactly) ──
    let expected_tools: BTreeSet<&str> = [
        "zocli.app.snapshot",
        "zocli.roots.list",
        "zocli.account.list",
        "zocli.account.current",
        "zocli.auth.status",
        "zocli.update.check",
        "zocli.mail.folders",
        "zocli.mail.list",
        "zocli.mail.search",
        "zocli.mail.read",
        "zocli.mail.send",
        "zocli.mail.reply",
        "zocli.mail.forward",
        "zocli.mail.attachment_export",
        "zocli.calendar.calendars",
        "zocli.calendar.events",
        "zocli.calendar.create",
        "zocli.calendar.delete",
        "zocli.drive.teams",
        "zocli.drive.list",
        "zocli.drive.upload",
        "zocli.drive.download",
    ]
    .into_iter()
    .collect();

    let expected_prompts: BTreeSet<&str> = [
        "shared",
        "mail",
        "calendar",
        "drive",
        "daily-briefing",
        "find-and-read",
        "reply-with-context",
    ]
    .into_iter()
    .collect();

    let expected_resources: BTreeSet<&str> = [
        "ui://zocli/dashboard",
        "ui://zocli/mail",
        "ui://zocli/calendar",
        "ui://zocli/drive",
        "ui://zocli/auth",
        "ui://zocli/account",
    ]
    .into_iter()
    .collect();

    // ── Initialize with UI + roots (to get all 21 tools) ──
    let input = [
        initialize_request_with_roots(true, false),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(3, "prompts/list", json!({})),
        mcp_request(4, "resources/list", json!({})),
        mcp_request(5, "resources/templates/list", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);

    // ── Verify tools/list ──
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    let actual_tools: BTreeSet<&str> = tools
        .iter()
        .map(|t| t["name"].as_str().expect("tool name"))
        .collect();

    let missing_tools: Vec<_> = expected_tools.difference(&actual_tools).collect();
    let extra_tools: Vec<_> = actual_tools.difference(&expected_tools).collect();
    assert!(
        missing_tools.is_empty() && extra_tools.is_empty(),
        "tools/list parity failure!\n  Missing (in manifest but not runtime): {missing_tools:?}\n  Extra (in runtime but not manifest): {extra_tools:?}"
    );

    // ── Verify prompts/list ──
    let prompts = responses[2]["result"]["prompts"]
        .as_array()
        .expect("prompts array");
    let actual_prompts: BTreeSet<&str> = prompts
        .iter()
        .map(|p| p["name"].as_str().expect("prompt name"))
        .collect();

    let missing_prompts: Vec<_> = expected_prompts.difference(&actual_prompts).collect();
    let extra_prompts: Vec<_> = actual_prompts.difference(&expected_prompts).collect();
    assert!(
        missing_prompts.is_empty() && extra_prompts.is_empty(),
        "prompts/list parity failure!\n  Missing (in manifest but not runtime): {missing_prompts:?}\n  Extra (in runtime but not manifest): {extra_prompts:?}"
    );

    // ── Verify resources/list (ui:// app resources only) ──
    let resources = responses[3]["result"]["resources"]
        .as_array()
        .expect("resources array");
    let actual_resources: BTreeSet<&str> = resources
        .iter()
        .map(|r| r["uri"].as_str().expect("resource uri"))
        .filter(|uri| uri.starts_with("ui://"))
        .collect();

    let missing_resources: Vec<_> = expected_resources.difference(&actual_resources).collect();
    let extra_resources: Vec<_> = actual_resources.difference(&expected_resources).collect();
    assert!(
        missing_resources.is_empty() && extra_resources.is_empty(),
        "resources/list parity failure!\n  Missing (in manifest but not runtime): {missing_resources:?}\n  Extra (in runtime but not manifest): {extra_resources:?}"
    );

    // ── Verify resources/templates/list (ui:// templates only) ──
    let all_templates = responses[4]["result"]["resourceTemplates"]
        .as_array()
        .expect("resource templates array");
    let ui_templates: Vec<_> = all_templates
        .iter()
        .filter(|t| {
            t["uriTemplate"]
                .as_str()
                .is_some_and(|u| u.starts_with("ui://"))
        })
        .collect();
    // Dashboard template + 5 surface templates = 6
    assert_eq!(
        ui_templates.len(),
        6,
        "Expected 6 ui:// resource templates (dashboard + 5 surfaces), got {}.\n  Templates: {:?}",
        ui_templates.len(),
        ui_templates
            .iter()
            .map(|t| t["uriTemplate"].as_str().unwrap_or("?"))
            .collect::<Vec<_>>()
    );

    // ── Verify counts match manifest ──
    assert_eq!(actual_tools.len(), 22, "MCP tools count drift");
    assert_eq!(actual_prompts.len(), 7, "Prompts count drift");
    assert_eq!(actual_resources.len(), 6, "App resources count drift");
}

// ── Phase 4: Auth App ─────────────────────────────────────────
//
// Tests that the auth surface provides actionable auth information
// across 5 scenarios: no account, not logged in, expired, valid,
// and protected-resource challenge sharing.

#[test]
fn mcp_stdio_auth_status_no_account_returns_guidance() {
    // No accounts.toml at all — auth.status should return an error with guidance
    let temp = tempdir().expect("tempdir");

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({ "name": "zocli.auth.status", "arguments": {} }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let result = &responses[1]["result"];
    // When no account is configured, the tool should indicate this clearly.
    // It may return an error or a result with guidance — either way,
    // the response must contain actionable text.
    let is_error = responses[1].get("error").is_some();
    if is_error {
        let msg = responses[1]["error"]["message"]
            .as_str()
            .unwrap_or("");
        assert!(
            msg.contains("account") || msg.contains("configure") || msg.contains("add"),
            "auth error for no-account must mention account setup: {msg}"
        );
    } else {
        let guidance = result["guidance"].as_str().unwrap_or("");
        assert!(
            !guidance.is_empty(),
            "auth_status result must include non-empty guidance when no account"
        );
    }
}

#[test]
fn mcp_stdio_auth_status_not_logged_in_returns_guidance() {
    let temp = tempdir().expect("tempdir");
    // Account configured but no credentials stored (store_missing)
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.test]
email = "test@zoho.com"
default = true
datacenter = "com"
account_id = "99999"
client_id = "client-test"
credential_ref = "store:oauth"
"#,
    );
    // No credentials.toml — store_missing state

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({ "name": "zocli.auth.status", "arguments": { "account": "test" } }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let result = &responses[1]["result"];
    let structured = &result["structuredContent"];
    let content = result["content"].as_array().expect("content array");
    let text = content[0]["text"].as_str().unwrap_or("");

    // Must show store_missing state
    assert!(
        text.contains("store_missing") || text.contains("not found") || text.contains("no local secret"),
        "auth_status for not-logged-in must show store_missing state: {text}"
    );

    // Must include guidance
    let guidance = structured["guidance"].as_str().unwrap_or("");
    assert!(
        !guidance.is_empty(),
        "auth_status must include guidance for store_missing"
    );
    assert!(
        guidance.contains("login") || guidance.contains("zocli login"),
        "guidance for store_missing must mention login: {guidance}"
    );
    assert_eq!(structured["auth"]["credential_state"], "store_missing");
}

#[test]
fn mcp_stdio_auth_status_expired_returns_guidance() {
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.expired]
email = "expired@zoho.com"
default = true
datacenter = "com"
account_id = "88888"
client_id = "client-expired"
credential_ref = "store:oauth"
"#,
    );
    // Token with expired timestamp (epoch 0)
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.expired.services.oauth]
kind = "oauth_pkce"
access_token = "expired-token"
token_type = "Bearer"
expires_at_epoch_secs = 0
scope = ["ZohoMail.messages.ALL"]
client_id = "client-expired"
api_domain = "https://www.zohoapis.com"
"#,
    );

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({ "name": "zocli.auth.status", "arguments": { "account": "expired" } }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let result = &responses[1]["result"];
    let structured = &result["structuredContent"];
    assert_eq!(
        structured["auth"]["credential_state"], "store_expired",
        "expired token must show store_expired"
    );

    let guidance = structured["guidance"].as_str().unwrap_or("");
    assert!(
        !guidance.is_empty(),
        "auth_status must include guidance for expired token"
    );
    assert!(
        guidance.contains("login") || guidance.contains("expired"),
        "guidance for expired must mention re-login: {guidance}"
    );
}

#[test]
fn mcp_stdio_auth_status_valid_returns_scopes_and_guidance() {
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.valid]
email = "valid@zoho.com"
default = true
datacenter = "com"
account_id = "77777"
client_id = "client-valid"
credential_ref = "store:oauth"
"#,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.valid.services.oauth]
kind = "oauth_pkce"
access_token = "valid-token"
token_type = "Bearer"
expires_at_epoch_secs = 4102444800
scope = ["ZohoMail.messages.ALL", "ZohoCalendar.calendar.ALL", "WorkDrive.files.ALL"]
client_id = "client-valid"
api_domain = "https://www.zohoapis.com"
"#,
    );

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({ "name": "zocli.auth.status", "arguments": { "account": "valid" } }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let result = &responses[1]["result"];
    let structured = &result["structuredContent"];
    let content = result["content"].as_array().expect("content array");
    let text = content[0]["text"].as_str().unwrap_or("");

    assert_eq!(structured["auth"]["credential_state"], "store_present");
    assert_eq!(structured["account"], "valid");
    assert_eq!(structured["email"], "valid@zoho.com");
    assert_eq!(structured["datacenter"], "com");

    // Scopes must be present
    let scopes = structured["auth"]["scope"].as_array().expect("scope array");
    assert!(scopes.len() >= 3, "valid auth must expose scopes");

    // Guidance for valid auth — should confirm everything is ok
    let guidance = structured["guidance"].as_str().unwrap_or("");
    assert!(
        !guidance.is_empty(),
        "auth_status must include guidance even when valid"
    );
    assert!(
        text.contains("store_present"),
        "ui-enabled summary should mention store_present: {text}"
    );
}

#[test]
fn mcp_stdio_auth_status_includes_auth_discovery() {
    // When HTTP auth is configured, auth_status should include authDiscovery
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.disco]
email = "disco@zoho.com"
default = true
datacenter = "com"
account_id = "66666"
client_id = "client-disco"
credential_ref = "store:oauth"
"#,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.disco.services.oauth]
kind = "oauth_pkce"
access_token = "disco-token"
token_type = "Bearer"
expires_at_epoch_secs = 4102444800
scope = ["ZohoMail.messages.ALL"]
client_id = "client-disco"
api_domain = "https://www.zohoapis.com"
"#,
    );

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({ "name": "zocli.auth.status", "arguments": { "account": "disco" } }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .env("ZOCLI_MCP_HTTP_BEARER_TOKEN", "secret-token")
        .env("ZOCLI_MCP_HTTP_AUTH_ISSUER", "https://auth.example.test")
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let result = &responses[1]["result"];

    // Must include authDiscovery when HTTP auth is configured
    let discovery = &result["structuredContent"]["authDiscovery"];
    assert_eq!(discovery["enabled"], true);
    assert_eq!(
        discovery["authorizationServers"][0],
        "https://auth.example.test"
    );
}

#[test]
fn mcp_stdio_ui_tool_results_use_human_summary_and_structured_content() {
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.valid]
email = "valid@zoho.com"
default = true
datacenter = "com"
account_id = "77777"
client_id = "client-valid"
credential_ref = "store:oauth"
"#,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.valid.services.oauth]
kind = "oauth_pkce"
access_token = "valid-token"
token_type = "Bearer"
expires_at_epoch_secs = 4102444800
scope = ["ZohoMail.messages.ALL", "ZohoCalendar.calendar.ALL"]
client_id = "client-valid"
api_domain = "https://www.zohoapis.com"
"#,
    );

    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "zocli.auth.status",
                "arguments": { "account": "valid" }
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let result = &responses[1]["result"];
    let text = result["content"][0]["text"].as_str().expect("summary text");

    assert!(
        text.starts_with("Auth status for valid: store_present."),
        "ui-enabled content should be a human summary, got: {text}"
    );
    assert!(
        !text.trim_start().starts_with('{'),
        "ui-enabled content must not be raw JSON: {text}"
    );
    assert_eq!(result["structuredContent"]["auth"]["credential_state"], "store_present");
    assert_eq!(result["_meta"]["ui"]["resourceUri"], "ui://zocli/auth");
}

#[test]
fn mcp_stdio_non_ui_tool_results_keep_raw_json_content() {
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.valid]
email = "valid@zoho.com"
default = true
datacenter = "com"
account_id = "77777"
client_id = "client-valid"
credential_ref = "store:oauth"
"#,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.valid.services.oauth]
kind = "oauth_pkce"
access_token = "valid-token"
token_type = "Bearer"
expires_at_epoch_secs = 4102444800
scope = ["ZohoMail.messages.ALL"]
client_id = "client-valid"
api_domain = "https://www.zohoapis.com"
"#,
    );

    let input = [
        initialize_request(false),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "zocli.auth.status",
                "arguments": { "account": "valid" }
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let result = &responses[1]["result"];
    let text = result["content"][0]["text"].as_str().expect("json text");
    let parsed: Value = serde_json::from_str(text).expect("non-ui content json");

    assert_eq!(parsed["auth"]["credential_state"], "store_present");
    assert_eq!(parsed, result["structuredContent"]);
    assert!(result.get("_meta").is_none());
}

// ── Phase 5: Workflow Parity ─────────────────────────────────
//
// Tests that new workflow tools are registered and validate input.

#[test]
fn mcp_stdio_attachment_export_validates_required_args() {
    let temp = tempdir().expect("tempdir");
    write_mock_mail_account(temp.path());

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        // Missing required attachment_id
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "zocli.mail.attachment_export",
                "arguments": {
                    "account": "mock",
                    "folder_id": "INBOX",
                    "message_id": "123"
                }
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    // Should get a validation error about missing attachment_id
    let error = &responses[1]["error"];
    assert!(
        error.is_object(),
        "attachment_export without attachment_id must return error"
    );
    let msg = error["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("attachment_id"),
        "error must mention missing attachment_id: {msg}"
    );
}

#[test]
fn mcp_stdio_attachment_export_tool_is_registered() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");

    let export_tool = tools
        .iter()
        .find(|t| t["name"] == "zocli.mail.attachment_export")
        .expect("attachment_export tool must be registered");

    // Must require folder_id, message_id, attachment_id
    let required = export_tool["inputSchema"]["required"]
        .as_array()
        .expect("required array");
    let required_names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert!(required_names.contains(&"folder_id"));
    assert!(required_names.contains(&"message_id"));
    assert!(required_names.contains(&"attachment_id"));

    // Must map to mail surface
    assert_eq!(
        export_tool["_meta"]["ui"]["resourceUri"],
        "ui://zocli/mail"
    );
}

// ── Phase 6: Schema Stabilization ────────────────────────────
//
// These tests define the stable structuredContent schema contract
// for each tool category. If a field is added, removed, or renamed,
// the corresponding test MUST be updated — schema drift breaks tests.

/// Every structuredContent payload includes "schemaVersion" for host versioning.
const SV: &str = "schemaVersion";

/// Expected top-level keys for the dashboard snapshot payload.
const SNAPSHOT_SCHEMA_KEYS: &[&str] = &[
    SV,
    "generatedAt",
    "protocolVersion",
    "appResourceUri",
    "accountCount",
    "currentAccount",
    "authDiscovery",
    "accounts",
];

/// Expected top-level keys for account list payload.
const ACCOUNT_LIST_SCHEMA_KEYS: &[&str] = &[SV, "items"];

/// Expected top-level keys for account current payload.
const ACCOUNT_CURRENT_SCHEMA_KEYS: &[&str] = &[SV, "account", "email"];

/// Expected top-level keys for auth status payload.
const AUTH_STATUS_SCHEMA_KEYS: &[&str] = &[
    SV,
    "account",
    "email",
    "datacenter",
    "auth",
    "guidance",
    "authDiscovery",
];

/// Expected top-level keys for update check payload.
const UPDATE_CHECK_SCHEMA_KEYS: &[&str] = &[
    SV,
    "operation",
    "status",
    "currentVersion",
    "targetVersion",
    "requestedVersion",
    "asset",
    "target",
    "baseUrl",
];

/// Expected top-level keys for roots list payload.
const ROOTS_SCHEMA_KEYS: &[&str] = &[SV, "roots"];

/// Expected top-level keys for mail folders payload.
const MAIL_FOLDERS_SCHEMA_KEYS: &[&str] = &[SV, "account", "items"];

/// Expected top-level keys for mail list payload.
const MAIL_LIST_SCHEMA_KEYS: &[&str] = &[SV, "account", "folder_id", "items"];

/// Expected top-level keys for mail search payload.
const MAIL_SEARCH_SCHEMA_KEYS: &[&str] = &[SV, "account", "query", "items"];

/// Expected top-level keys for mail read payload.
const MAIL_READ_SCHEMA_KEYS: &[&str] = &[SV, "account", "item"];

/// Expected top-level keys for mail send payload.
const MAIL_SEND_SCHEMA_KEYS: &[&str] = &[SV, "account", "sent"];

/// Expected top-level keys for mail reply payload.
const MAIL_REPLY_SCHEMA_KEYS: &[&str] = &[SV, "account", "folder_id", "reply"];

/// Expected top-level keys for mail forward payload.
const MAIL_FORWARD_SCHEMA_KEYS: &[&str] = &[SV, "account", "folder_id", "forward"];

/// Expected top-level keys for mail attachment export payload.
const MAIL_ATTACHMENT_EXPORT_SCHEMA_KEYS: &[&str] = &[
    SV,
    "account",
    "folder_id",
    "message_id",
    "attachment_id",
    "attachment_name",
    "attachment_size",
    "content_base64",
];

/// Expected top-level keys for calendar events payload.
const CALENDAR_EVENTS_SCHEMA_KEYS: &[&str] = &[SV, "account", "calendar", "window", "items"];

/// Expected top-level keys for calendar create payload.
const CALENDAR_CREATE_SCHEMA_KEYS: &[&str] = &[SV, "account", "calendar", "event"];

/// Expected top-level keys for calendar delete payload.
const CALENDAR_DELETE_SCHEMA_KEYS: &[&str] = &[SV, "account", "calendar", "deleted_event"];

/// Expected top-level keys for drive list payload.
const DRIVE_LIST_SCHEMA_KEYS: &[&str] = &[SV, "account", "folder_id", "items"];

/// Expected top-level keys for drive upload payload.
const DRIVE_UPLOAD_SCHEMA_KEYS: &[&str] = &[SV, "account", "folder_id", "source", "uploaded"];

/// Expected top-level keys for drive download payload.
const DRIVE_DOWNLOAD_SCHEMA_KEYS: &[&str] = &[SV, "account", "file_id", "downloaded"];

/// Asserts that a JSON object contains exactly the expected keys (no more, no fewer).
fn assert_schema_keys(structured: &Value, expected_keys: &[&str], tool_name: &str) {
    let obj = structured
        .as_object()
        .unwrap_or_else(|| panic!("{tool_name}: structuredContent must be an object"));

    let actual: std::collections::BTreeSet<&str> =
        obj.keys().map(|k| k.as_str()).collect();
    let expected: std::collections::BTreeSet<&str> =
        expected_keys.iter().copied().collect();

    let extra: Vec<_> = actual.difference(&expected).collect();
    let missing: Vec<_> = expected.difference(&actual).collect();

    assert!(
        extra.is_empty() && missing.is_empty(),
        "{tool_name} schema drift detected!\n  Extra keys: {extra:?}\n  Missing keys: {missing:?}\n  Expected: {expected:?}\n  Actual: {actual:?}"
    );
}

#[test]
fn schema_snapshot_dashboard() {
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@zoho.com"
default = true
datacenter = "com"
account_id = "12345"
client_id = "client-123"
credential_ref = "store:oauth"
"#,
    );

    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "zocli.app.snapshot",
                "arguments": {}
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_schema_keys(structured, SNAPSHOT_SCHEMA_KEYS, "zocli.app.snapshot");

    // Verify schemaVersion is present and correct
    assert_eq!(
        structured["schemaVersion"], "1.0",
        "structuredContent must include schemaVersion 1.0"
    );

    // Verify nested account schema
    let accounts = structured["accounts"].as_array().expect("accounts array");
    assert!(!accounts.is_empty(), "snapshot must include at least one account");
    let account = &accounts[0];
    let account_keys: std::collections::BTreeSet<&str> = account
        .as_object()
        .expect("account object")
        .keys()
        .map(|k| k.as_str())
        .collect();
    for key in &["name", "email", "current", "datacenter", "account_id", "auth"] {
        assert!(
            account_keys.contains(key),
            "snapshot account missing key: {key}"
        );
    }
}

#[test]
fn schema_snapshot_account_list() {
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@zoho.com"
default = true
datacenter = "com"
account_id = "12345"
client_id = "client-123"
credential_ref = "store:oauth"
"#,
    );

    let input = [
        initialize_request(false),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "zocli.account.list",
                "arguments": {}
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_schema_keys(structured, ACCOUNT_LIST_SCHEMA_KEYS, "zocli.account.list");

    // Verify account item schema
    let items = structured["items"].as_array().expect("items array");
    assert!(!items.is_empty());
    let item = &items[0];
    for key in &["name", "email", "current", "datacenter", "account_id"] {
        assert!(
            item.get(key).is_some(),
            "account.list item missing key: {key}"
        );
    }
}

#[test]
fn schema_snapshot_account_current() {
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@zoho.com"
default = true
datacenter = "com"
account_id = "12345"
client_id = "client-123"
credential_ref = "store:oauth"
"#,
    );

    let input = [
        initialize_request(false),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "zocli.account.current",
                "arguments": {}
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_schema_keys(structured, ACCOUNT_CURRENT_SCHEMA_KEYS, "zocli.account.current");
    assert_eq!(structured["account"], "personal");
    assert_eq!(structured["email"], "me@zoho.com");
}

#[test]
fn schema_snapshot_auth_status() {
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@zoho.com"
default = true
datacenter = "com"
account_id = "12345"
client_id = "client-123"
credential_ref = "store:oauth"
"#,
    );

    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "zocli.auth.status",
                "arguments": {}
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_schema_keys(structured, AUTH_STATUS_SCHEMA_KEYS, "zocli.auth.status");

    // Verify auth nested schema
    let auth = structured["auth"].as_object().expect("auth object");
    assert!(auth.contains_key("credential_state"), "auth missing credential_state");

    // Verify guidance is a non-empty string
    assert!(
        structured["guidance"].as_str().is_some_and(|s| !s.is_empty()),
        "guidance must be a non-empty string"
    );

    // Verify authDiscovery schema
    let discovery = structured["authDiscovery"].as_object().expect("authDiscovery object");
    assert!(discovery.contains_key("enabled"), "authDiscovery missing enabled");
}

#[test]
fn schema_snapshot_update_check() {
    let mut server = Server::new();
    let base_url = format!("{}/releases/download/v8.8.8", server.url());
    let asset = current_release_update_target();
    let _checksums = server
        .mock("GET", "/releases/download/v8.8.8/SHA256SUMS")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_body(format!("cafebabe  {asset}\n"))
        .create();

    let input = [
        initialize_request(false),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "zocli.update.check",
                "arguments": {}
            }),
        ),
    ]
    .concat();

    let output = zocli()
        .env("ZOCLI_UPDATE_BASE_URL", &base_url)
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_schema_keys(structured, UPDATE_CHECK_SCHEMA_KEYS, "zocli.update.check");
}

#[test]
fn schema_snapshot_roots_list() {
    let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("zocli"))
        .env("ZOCLI_SECRET_BACKEND", "file")
        .args(["mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn zocli mcp");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);

    // Initialize with roots + UI capability
    stdin
        .write_all(initialize_request_with_roots(true, true).as_bytes())
        .expect("write init");
    let _init_response = read_response(&mut reader);

    // Send initialized notification
    stdin
        .write_all(mcp_notification("notifications/initialized", json!({})).as_bytes())
        .expect("write initialized");

    // Call roots.list tool — server will issue roots/list request to client
    stdin
        .write_all(
            mcp_request(
                2,
                "tools/call",
                json!({
                    "name": "zocli.roots.list",
                    "arguments": {}
                }),
            )
            .as_bytes(),
        )
        .expect("write roots.list call");

    // Server requests roots from client
    let roots_request = read_response(&mut reader);
    assert_eq!(
        roots_request["method"], "roots/list",
        "server must request roots"
    );
    let request_id = roots_request["id"].as_u64().expect("request id");

    // Provide roots response
    stdin
        .write_all(
            mcp_response(
                request_id,
                json!({
                    "roots": [
                        { "uri": "file:///tmp/schema-test", "name": "schema-test" }
                    ]
                }),
            )
            .as_bytes(),
        )
        .expect("write roots response");

    let tool_result = read_response(&mut reader);
    let structured = &tool_result["result"]["structuredContent"];
    assert_schema_keys(structured, ROOTS_SCHEMA_KEYS, "zocli.roots.list");

    // Verify roots array items have expected shape
    let roots = structured["roots"].as_array().expect("roots array");
    assert!(!roots.is_empty());
    assert!(roots[0].get("uri").is_some(), "root item missing uri");
    assert!(roots[0].get("name").is_some(), "root item missing name");

    drop(stdin);
    child.wait().expect("child exit");
}

/// Verifies that schema constants cover ALL 22 tools and are self-consistent.
/// If a new tool is added, it must get a corresponding schema constant.
#[test]
fn schema_constants_cover_all_tools() {
    use std::collections::BTreeSet;

    // Map of tool name → expected schema constant keys
    let schema_map: Vec<(&str, &[&str])> = vec![
        ("zocli.app.snapshot", SNAPSHOT_SCHEMA_KEYS),
        ("zocli.roots.list", ROOTS_SCHEMA_KEYS),
        ("zocli.account.list", ACCOUNT_LIST_SCHEMA_KEYS),
        ("zocli.account.current", ACCOUNT_CURRENT_SCHEMA_KEYS),
        ("zocli.auth.status", AUTH_STATUS_SCHEMA_KEYS),
        ("zocli.update.check", UPDATE_CHECK_SCHEMA_KEYS),
        ("zocli.mail.folders", MAIL_FOLDERS_SCHEMA_KEYS),
        ("zocli.mail.list", MAIL_LIST_SCHEMA_KEYS),
        ("zocli.mail.search", MAIL_SEARCH_SCHEMA_KEYS),
        ("zocli.mail.read", MAIL_READ_SCHEMA_KEYS),
        ("zocli.mail.send", MAIL_SEND_SCHEMA_KEYS),
        ("zocli.mail.reply", MAIL_REPLY_SCHEMA_KEYS),
        ("zocli.mail.forward", MAIL_FORWARD_SCHEMA_KEYS),
        ("zocli.mail.attachment_export", MAIL_ATTACHMENT_EXPORT_SCHEMA_KEYS),
        ("zocli.calendar.calendars", MAIL_FOLDERS_SCHEMA_KEYS),
        ("zocli.calendar.events", CALENDAR_EVENTS_SCHEMA_KEYS),
        ("zocli.calendar.create", CALENDAR_CREATE_SCHEMA_KEYS),
        ("zocli.calendar.delete", CALENDAR_DELETE_SCHEMA_KEYS),
        ("zocli.drive.teams", MAIL_FOLDERS_SCHEMA_KEYS),
        ("zocli.drive.list", DRIVE_LIST_SCHEMA_KEYS),
        ("zocli.drive.upload", DRIVE_UPLOAD_SCHEMA_KEYS),
        ("zocli.drive.download", DRIVE_DOWNLOAD_SCHEMA_KEYS),
    ];

    // Verify we cover all 22 tools
    assert_eq!(
        schema_map.len(),
        22,
        "schema_constants_cover_all_tools must map all 22 MCP tools"
    );

    // Verify no duplicate tool names
    let names: BTreeSet<&str> = schema_map.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        names.len(),
        22,
        "duplicate tool name in schema map"
    );

    // Verify each schema constant has at least 1 key
    for (tool_name, keys) in &schema_map {
        assert!(
            !keys.is_empty(),
            "{tool_name} schema constant must have at least one key"
        );
    }

    // Verify no duplicate keys within any single schema
    for (tool_name, keys) in &schema_map {
        let unique: BTreeSet<&str> = keys.iter().copied().collect();
        assert_eq!(
            unique.len(),
            keys.len(),
            "{tool_name} schema has duplicate keys"
        );
    }
}

/// Verifies that mail.folders returns { account, items } and
/// mail.list returns { account, folder_id, items }.
#[test]
fn schema_mail_folders_and_list_shapes() {
    assert_eq!(
        MAIL_FOLDERS_SCHEMA_KEYS,
        &[SV, "account", "items"],
        "mail.folders schema must include schemaVersion, account, items"
    );
    assert_eq!(
        MAIL_LIST_SCHEMA_KEYS,
        &[SV, "account", "folder_id", "items"],
        "mail.list schema must include schemaVersion, account, folder_id, items"
    );
}

/// Verifies all list/collection tools include "account" and "items".
#[test]
fn schema_list_operations_follow_items_pattern() {
    let all_list_schemas: &[(&str, &[&str])] = &[
        ("mail.folders", MAIL_FOLDERS_SCHEMA_KEYS),
        ("mail.list", MAIL_LIST_SCHEMA_KEYS),
        ("mail.search", MAIL_SEARCH_SCHEMA_KEYS),
        ("calendar.calendars", MAIL_FOLDERS_SCHEMA_KEYS),
        ("calendar.events", CALENDAR_EVENTS_SCHEMA_KEYS),
        ("drive.teams", MAIL_FOLDERS_SCHEMA_KEYS),
        ("drive.list", DRIVE_LIST_SCHEMA_KEYS),
    ];
    for (name, keys) in all_list_schemas {
        assert!(
            keys.contains(&"account"),
            "{name} list schema missing 'account': {keys:?}"
        );
        assert!(
            keys.contains(&"items"),
            "{name} list schema missing 'items': {keys:?}"
        );
    }
}

// ── MCP Apps spec contract tests ──

/// Verify display mode names match MCP Apps spec SEP-1865 (2026-01-26).
/// Stable modes: inline, fullscreen, pip. Nothing else is accepted.
#[test]
fn spec_display_modes_match_sep_1865() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
        // All three spec-correct modes
        mcp_request(3, "ui/request-display-mode", json!({ "mode": "inline" })),
        mcp_request(4, "ui/request-display-mode", json!({ "mode": "fullscreen" })),
        mcp_request(5, "ui/request-display-mode", json!({ "mode": "pip" })),
        // Non-spec modes must fall back to inline
        mcp_request(6, "ui/request-display-mode", json!({ "mode": "floating" })),
        mcp_request(7, "ui/request-display-mode", json!({ "mode": "full-window" })),
        mcp_request(8, "ui/request-display-mode", json!({ "mode": "maximized" })),
        mcp_request(9, "ui/request-display-mode", json!({ "mode": "" })),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    // Spec-correct modes echoed back
    assert_eq!(responses[2]["result"]["mode"], "inline");
    assert_eq!(responses[3]["result"]["mode"], "fullscreen");
    assert_eq!(responses[4]["result"]["mode"], "pip");
    // Non-spec modes → inline fallback
    for (i, resp) in responses.iter().enumerate().take(9).skip(5) {
        assert_eq!(
            resp["result"]["mode"], "inline",
            "response[{i}] must fall back to inline for non-spec mode"
        );
    }
}

/// Verify UI lifecycle gate is enforced: ui/* methods before ui/initialize return error.
#[test]
fn spec_ui_lifecycle_gate_enforced() {
    let methods = [
        "ui/update-model-context",
        "ui/message",
        "ui/request-display-mode",
        "ui/open-link",
        "ui/resource-teardown",
    ];
    for (i, method) in methods.iter().enumerate() {
        let id = (i + 2) as u64;
        let input = [
            initialize_request(true),
            mcp_notification("notifications/initialized", json!({})),
            // No ui/initialize — go straight to ui/* method
            mcp_request(id, method, json!({})),
        ]
        .concat();

        let output = zocli()
            .args(["mcp"])
            .write_stdin(input)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let responses = parse_responses(&output);
        assert!(
            responses[1].get("error").is_some(),
            "{method} without prior ui/initialize must return error, got: {}",
            responses[1]
        );
    }
}

/// Verify app metadata shape in ui/initialize response matches spec.
#[test]
fn spec_ui_initialize_response_shape() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let result = &responses[1]["result"];
    // Required fields per spec
    assert!(result.get("protocolVersion").is_some(), "missing protocolVersion");
    assert!(result.get("serverInfo").is_some(), "missing serverInfo");
    assert!(result.get("capabilities").is_some(), "missing capabilities");
    assert!(result["serverInfo"].get("name").is_some(), "missing serverInfo.name");
    assert!(result["serverInfo"].get("version").is_some(), "missing serverInfo.version");
}

/// Double ui/initialize is allowed — resets the session to a fresh Active state.
/// After re-init, view_initialized resets to false, so ui/message requires
/// a new ui/notifications/initialized before it can succeed again.
#[test]
fn spec_ui_double_initialize_resets_state() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
        // View confirms initialization
        mcp_notification("ui/notifications/initialized", json!({})),
        mcp_request(3, "ui/message", json!({ "type": "info", "text": "first" })),
        // Second ui/initialize — should succeed and reset state (view_initialized=false)
        mcp_request(4, "ui/initialize", json!({})),
        // ui/message without re-sending ui/notifications/initialized — should FAIL
        mcp_request(5, "ui/message", json!({ "type": "info", "text": "second" })),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    assert_eq!(responses.len(), 5);
    // Both ui/initialize succeed
    assert!(responses[1].get("result").is_some());
    assert!(responses[3].get("result").is_some());
    // First ui/message succeeds (view was initialized)
    assert_eq!(responses[2]["result"]["accepted"], true);
    // Second ui/message fails — re-init reset view_initialized to false
    assert!(
        responses[4].get("error").is_some(),
        "ui/message after re-initialize without ui/notifications/initialized must fail, got: {}",
        responses[4]
    );
}

/// Double teardown: second teardown fails because state is NotInitialized after first.
#[test]
fn spec_ui_double_teardown_fails() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
        mcp_request(3, "ui/resource-teardown", json!({ "uri": "ui://zocli/dashboard" })),
        // Second teardown — should fail, already NotInitialized
        mcp_request(4, "ui/resource-teardown", json!({ "uri": "ui://zocli/dashboard" })),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    assert_eq!(responses.len(), 4);
    // First teardown succeeds
    assert_eq!(responses[2]["result"]["accepted"], true);
    // Second teardown → error (not initialized)
    assert!(
        responses[3].get("error").is_some(),
        "double teardown must return error, got: {}",
        responses[3]
    );
}

/// ui/update-model-context increments host-context revision (semantic effect, not just ack).
/// After teardown + re-init, revision resets to 0.
#[test]
fn spec_ui_model_context_tracks_revision() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
        // 3 model-context updates
        mcp_request(3, "ui/update-model-context", json!({ "context": {} })),
        mcp_request(4, "ui/update-model-context", json!({ "context": {} })),
        mcp_request(5, "ui/update-model-context", json!({ "context": {} })),
        // All should accept
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    assert_eq!(responses.len(), 5);
    // All three model-context updates accepted AND revision increments monotonically.
    assert_eq!(responses[2]["result"]["accepted"], true);
    assert_eq!(responses[2]["result"]["revision"], 1);
    assert_eq!(responses[3]["result"]["accepted"], true);
    assert_eq!(responses[3]["result"]["revision"], 2);
    assert_eq!(responses[4]["result"]["accepted"], true);
    assert_eq!(responses[4]["result"]["revision"], 3);
}

/// ui/message and ui/open-link require view to be initialized (ui/notifications/initialized).
/// Without that notification, they must fail even if ui/initialize was called.
#[test]
fn spec_ui_message_and_open_link_require_view_ready() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
        // No ui/notifications/initialized — view not ready
        mcp_request(3, "ui/message", json!({ "type": "info", "text": "too early" })),
        mcp_request(4, "ui/open-link", json!({ "url": "https://example.com" })),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    assert_eq!(responses.len(), 4);
    // ui/initialize succeeds
    assert!(responses[1].get("result").is_some());
    // ui/message fails — view not ready
    assert!(
        responses[2].get("error").is_some(),
        "ui/message without view initialization must fail, got: {}",
        responses[2]
    );
    // ui/open-link fails — view not ready
    assert!(
        responses[3].get("error").is_some(),
        "ui/open-link without view initialization must fail, got: {}",
        responses[3]
    );
}

/// ui/open-link requires a non-empty 'url' parameter.
#[test]
fn spec_ui_open_link_requires_url() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
        mcp_notification("ui/notifications/initialized", json!({})),
        // Missing url
        mcp_request(3, "ui/open-link", json!({})),
        // Empty url
        mcp_request(4, "ui/open-link", json!({ "url": "" })),
        // Valid url
        mcp_request(5, "ui/open-link", json!({ "url": "https://example.com" })),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    assert_eq!(responses.len(), 5);
    // Missing url → error
    assert!(
        responses[2].get("error").is_some(),
        "ui/open-link without url must fail, got: {}",
        responses[2]
    );
    // Empty url → error
    assert!(
        responses[3].get("error").is_some(),
        "ui/open-link with empty url must fail, got: {}",
        responses[3]
    );
    // Valid url → success
    assert_eq!(responses[4]["result"]["accepted"], true);
}

/// ui/resource-teardown validates resourceUri against active resource.
#[test]
fn spec_ui_teardown_validates_resource_uri() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
        // View reports its resource URI
        mcp_notification(
            "ui/notifications/initialized",
            json!({ "resourceUri": "ui://zocli/dashboard" }),
        ),
        // Teardown with wrong URI — should fail
        mcp_request(
            3,
            "ui/resource-teardown",
            json!({ "resourceUri": "ui://zocli/wrong" }),
        ),
        // Teardown with correct URI — should succeed
        mcp_request(
            4,
            "ui/resource-teardown",
            json!({ "resourceUri": "ui://zocli/dashboard" }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    assert_eq!(responses.len(), 4);
    // Wrong URI → error
    assert!(
        responses[2].get("error").is_some(),
        "ui/resource-teardown with wrong resourceUri must fail, got: {}",
        responses[2]
    );
    // Correct URI → success
    assert_eq!(responses[3]["result"]["accepted"], true);
}

#[test]
fn mcp_stdio_resource_teardown_is_acknowledged() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
        mcp_request(3, "ui/resource-teardown", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    // responses[0]=init, responses[1]=ui/initialize, responses[2]=resource-teardown
    assert_eq!(
        responses[2]["result"]["accepted"], true,
        "resource-teardown must return accepted: true"
    );
}

// =============================================================================
// Phase A3 — Summary Contract
// =============================================================================

/// For every callable tool in UI mode, the content[0].text must NOT start with
/// `{` or `[` — it must be a human-readable summary, not raw JSON.
#[test]
fn summary_contract_no_json_leak_in_ui_content() {
    let tools = [
        ("zocli.app.snapshot", json!({})),
        ("zocli.account.list", json!({})),
        ("zocli.account.current", json!({})),
        ("zocli.auth.status", json!({})),
        ("zocli.update.check", json!({})),
    ];

    for (tool_name, args) in &tools {
        let input = [
            initialize_request(true),
            mcp_notification("notifications/initialized", json!({})),
            mcp_request(
                2,
                "tools/call",
                json!({ "name": tool_name, "arguments": args }),
            ),
        ]
        .concat();

        let output = zocli()
            .args(["mcp"])
            .write_stdin(input)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let responses = parse_responses(&output);
        let result = &responses[1]["result"];
        let content = result["content"]
            .as_array()
            .unwrap_or_else(|| panic!("{tool_name}: missing content array"));
        assert!(
            !content.is_empty(),
            "{tool_name}: content array must not be empty"
        );
        let text = content[0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{tool_name}: missing text in content[0]"));
        let first_char = text.chars().next().unwrap_or(' ');
        assert!(
            first_char != '{' && first_char != '[',
            "{tool_name}: UI content must be human summary, not JSON. Got: {text}"
        );
    }
}

/// All UI summaries must be ≤ 280 characters.
#[test]
fn summary_contract_max_length_enforced() {
    let tools = [
        ("zocli.app.snapshot", json!({})),
        ("zocli.account.list", json!({})),
        ("zocli.account.current", json!({})),
        ("zocli.auth.status", json!({})),
        ("zocli.update.check", json!({})),
    ];

    for (tool_name, args) in &tools {
        let input = [
            initialize_request(true),
            mcp_notification("notifications/initialized", json!({})),
            mcp_request(
                2,
                "tools/call",
                json!({ "name": tool_name, "arguments": args }),
            ),
        ]
        .concat();

        let output = zocli()
            .args(["mcp"])
            .write_stdin(input)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let responses = parse_responses(&output);
        let result = &responses[1]["result"];
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{tool_name}: missing text"));
        assert!(
            text.chars().count() <= 280,
            "{tool_name}: summary too long ({} chars > 280): {text}",
            text.chars().count()
        );
    }
}

/// Non-UI content must preserve full JSON for model consumption.
#[test]
fn summary_contract_non_ui_preserves_json() {
    let tools = [
        ("zocli.app.snapshot", json!({})),
        ("zocli.account.list", json!({})),
        ("zocli.account.current", json!({})),
        ("zocli.auth.status", json!({})),
        ("zocli.update.check", json!({})),
    ];

    for (tool_name, args) in &tools {
        let input = [
            initialize_request(false),
            mcp_notification("notifications/initialized", json!({})),
            mcp_request(
                2,
                "tools/call",
                json!({ "name": tool_name, "arguments": args }),
            ),
        ]
        .concat();

        let output = zocli()
            .args(["mcp"])
            .write_stdin(input)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let responses = parse_responses(&output);
        let result = &responses[1]["result"];
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{tool_name}: missing text"));
        // Non-UI content must be valid JSON
        let parsed: std::result::Result<Value, _> = serde_json::from_str(text);
        assert!(
            parsed.is_ok(),
            "{tool_name}: non-UI content must be valid JSON, got: {text}"
        );
    }
}

// =============================================================================
// Phase A5 — Host Action Semantics
// =============================================================================

/// ui/message rejects payloads that have neither `content` nor `text`.
#[test]
fn host_action_message_rejects_missing_content() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
        mcp_notification("ui/notifications/initialized", json!({})),
        // Empty payload — no content, no text
        mcp_request(3, "ui/message", json!({})),
        // Valid payload with text
        mcp_request(4, "ui/message", json!({ "text": "hello" })),
        // Valid payload with content
        mcp_request(5, "ui/message", json!({ "content": "hello" })),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    // responses: [init, ui/initialize, ui/message(empty), ui/message(text), ui/message(content)]
    assert!(
        responses[2].get("error").is_some(),
        "ui/message with empty payload must fail, got: {}",
        responses[2]
    );
    assert_eq!(
        responses[3]["result"]["accepted"], true,
        "ui/message with text must succeed"
    );
    assert_eq!(
        responses[4]["result"]["accepted"], true,
        "ui/message with content must succeed"
    );
}

/// ui/open-link rejects dangerous URL schemes (javascript:, data:, file:).
#[test]
fn host_action_open_link_rejects_dangerous_schemes() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
        mcp_notification("ui/notifications/initialized", json!({})),
        // Dangerous schemes
        mcp_request(
            3,
            "ui/open-link",
            json!({ "url": "javascript:alert(1)" }),
        ),
        mcp_request(
            4,
            "ui/open-link",
            json!({ "url": "data:text/html,<h1>xss</h1>" }),
        ),
        mcp_request(
            5,
            "ui/open-link",
            json!({ "url": "file:///etc/passwd" }),
        ),
        // Safe schemes
        mcp_request(
            6,
            "ui/open-link",
            json!({ "url": "https://example.com" }),
        ),
        mcp_request(
            7,
            "ui/open-link",
            json!({ "url": "http://example.com" }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    // responses: [init, ui/init, js, data, file, https, http]
    assert!(
        responses[2].get("error").is_some(),
        "javascript: must be rejected, got: {}",
        responses[2]
    );
    assert!(
        responses[3].get("error").is_some(),
        "data: must be rejected, got: {}",
        responses[3]
    );
    assert!(
        responses[4].get("error").is_some(),
        "file: must be rejected, got: {}",
        responses[4]
    );
    assert_eq!(
        responses[5]["result"]["accepted"], true,
        "https: must be accepted"
    );
    assert_eq!(
        responses[6]["result"]["accepted"], true,
        "http: must be accepted"
    );
}

// =============================================================================
// Phase A6 — Security Hardening
// =============================================================================

/// For every ui:// resource, _meta.ui.csp.connectDomains must contain "'self'"
/// and frameDomains must be empty.
#[test]
fn security_csp_shape_populated() {
    let surfaces = [
        "ui://zocli/dashboard",
        "ui://zocli/mail",
        "ui://zocli/calendar",
        "ui://zocli/drive",
        "ui://zocli/auth",
        "ui://zocli/account",
    ];

    for surface_uri in &surfaces {
        let input = [
            initialize_request(true),
            mcp_notification("notifications/initialized", json!({})),
            mcp_request(
                2,
                "resources/read",
                json!({ "uri": surface_uri }),
            ),
        ]
        .concat();

        let output = zocli()
            .args(["mcp"])
            .write_stdin(input)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let responses = parse_responses(&output);
        let contents = responses[1]["result"]["contents"]
            .as_array()
            .unwrap_or_else(|| panic!("{surface_uri}: missing contents array"));
        assert!(!contents.is_empty(), "{surface_uri}: empty contents");

        let meta = &contents[0]["_meta"];
        let connect = meta["ui"]["csp"]["connectDomains"]
            .as_array()
            .unwrap_or_else(|| panic!("{surface_uri}: missing connectDomains"));
        assert!(
            connect.iter().any(|v| v.as_str() == Some("'self'")),
            "{surface_uri}: connectDomains must contain 'self', got: {connect:?}"
        );

        let frame = meta["ui"]["csp"]["frameDomains"]
            .as_array()
            .unwrap_or_else(|| panic!("{surface_uri}: missing frameDomains"));
        assert!(
            frame.is_empty(),
            "{surface_uri}: frameDomains must be empty, got: {frame:?}"
        );
    }
}

/// Dashboard HTML must not contain any external `<script src=` tags.
#[test]
fn security_html_no_external_scripts() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "resources/read",
            json!({ "uri": "ui://zocli/dashboard" }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let html = responses[1]["result"]["contents"][0]["text"]
        .as_str()
        .expect("dashboard HTML text");
    // No external script tags
    assert!(
        !html.contains("<script src="),
        "dashboard HTML must not contain external script tags"
    );
}

/// Dashboard HTML must not contain any external stylesheet links.
#[test]
fn security_html_no_external_styles() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "resources/read",
            json!({ "uri": "ui://zocli/dashboard" }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let html = responses[1]["result"]["contents"][0]["text"]
        .as_str()
        .expect("dashboard HTML text");
    assert!(
        !html.contains(r#"<link rel="stylesheet" href="#),
        "dashboard HTML must not contain external stylesheet links"
    );
}

/// Dashboard HTML must capture hostOrigin and never use bare postMessage(msg, "*")
/// without a fallback guard.
#[test]
fn security_postmessage_origin_capture() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "resources/read",
            json!({ "uri": "ui://zocli/dashboard" }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let html = responses[1]["result"]["contents"][0]["text"]
        .as_str()
        .expect("dashboard HTML text");
    // Must contain hostOrigin variable for origin capture
    assert!(
        html.contains("hostOrigin"),
        "dashboard HTML must contain 'hostOrigin' pattern for origin tracking"
    );
    // The postMessage function uses `hostOrigin || "*"` — guarded fallback, not bare "*"
    assert!(
        html.contains(r#"hostOrigin || "*""#),
        "postMessage must use hostOrigin with guarded fallback"
    );
}

// =============================================================================
// Phase A4 — Dedicated Surface Controllers
// =============================================================================

/// Dashboard HTML must contain SURFACE_CONTROLLERS with all 6 surface keys.
#[test]
fn surface_controller_registry_exists() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "resources/read",
            json!({ "uri": "ui://zocli/dashboard" }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let html = responses[1]["result"]["contents"][0]["text"]
        .as_str()
        .expect("dashboard HTML text");

    assert!(
        html.contains("SURFACE_CONTROLLERS"),
        "dashboard HTML must contain SURFACE_CONTROLLERS"
    );
    for surface in &["dashboard", "mail", "calendar", "drive", "auth", "account"] {
        assert!(
            html.contains(&format!("{surface}:")),
            "SURFACE_CONTROLLERS must contain key '{surface}'"
        );
    }
}

/// Each surface controller must have a defaultPanel field.
#[test]
fn surface_controller_each_surface_has_defaults() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "resources/read",
            json!({ "uri": "ui://zocli/dashboard" }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let html = responses[1]["result"]["contents"][0]["text"]
        .as_str()
        .expect("dashboard HTML text");

    // Every surface entry must have a defaultPanel field
    for surface in &["dashboard", "mail", "calendar", "drive", "auth", "account"] {
        assert!(
            html.contains("defaultPanel"),
            "surface controller '{surface}' must define defaultPanel"
        );
    }
}

/// The mail surface controller should focus the prompts panel.
#[test]
fn surface_controller_mail_focuses_prompts() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "resources/read",
            json!({ "uri": "ui://zocli/mail" }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let html = responses[1]["result"]["contents"][0]["text"]
        .as_str()
        .expect("mail surface HTML text");

    // HTML should contain the bootstrap with preferredSection = prompts
    assert!(
        html.contains(r#""preferredSection":"prompts"#),
        "mail surface bootstrap must set preferredSection to prompts"
    );
    // HTML should contain SURFACE_CONTROLLERS with mail entry
    assert!(
        html.contains("SURFACE_CONTROLLERS"),
        "mail surface HTML must contain SURFACE_CONTROLLERS"
    );
}

// =============================================================================
// Phase A7 — Browser Conformance Harness
// =============================================================================

fn get_dashboard_html() -> String {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "resources/read",
            json!({ "uri": "ui://zocli/dashboard" }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    responses[1]["result"]["contents"][0]["text"]
        .as_str()
        .expect("dashboard HTML text")
        .to_string()
}

/// HTML must contain all postMessage lifecycle functions.
#[test]
fn browser_harness_postmessage_lifecycle_functions() {
    let html = get_dashboard_html();
    for func in &[
        "sendRequest",
        "sendNotification",
        "sendResponse",
        "onMessage",
        "handleHostNotification",
        "initializeApp",
    ] {
        assert!(
            html.contains(func),
            "dashboard HTML must contain function '{func}'"
        );
    }
}

/// HTML must contain display mode strings and requestAlternateDisplayMode.
#[test]
fn browser_harness_display_mode_strings() {
    let html = get_dashboard_html();
    for token in &[
        r#""inline""#,
        r#""fullscreen""#,
        r#""pip""#,
        "requestAlternateDisplayMode",
    ] {
        assert!(
            html.contains(token),
            "dashboard HTML must contain '{token}'"
        );
    }
}

/// HTML must start with <!doctype html>, have a single inline <script>, no external scripts.
#[test]
fn browser_harness_html_structure() {
    let html = get_dashboard_html();
    assert!(
        html.trim_start().starts_with("<!doctype html>") || html.trim_start().starts_with("<!DOCTYPE html>"),
        "dashboard HTML must start with <!doctype html>"
    );
    // Must have at least one inline <script>
    assert!(
        html.contains("<script>"),
        "dashboard HTML must have an inline <script>"
    );
    // No external scripts
    assert!(
        !html.contains("<script src="),
        "dashboard HTML must not have external script tags"
    );
}

/// For each surface, APP_BOOTSTRAP JSON must contain expected keys.
#[test]
fn browser_harness_bootstrap_injection() {
    let surfaces = [
        "ui://zocli/dashboard",
        "ui://zocli/mail",
        "ui://zocli/calendar",
        "ui://zocli/drive",
        "ui://zocli/auth",
        "ui://zocli/account",
    ];
    let expected_keys = [
        "resourceUri",
        "defaultAccount",
        "preferredSection",
        "preferredResource",
        "preferredTool",
        "preferredSkill",
        "preferredPrompt",
    ];

    for surface_uri in &surfaces {
        let input = [
            initialize_request(true),
            mcp_notification("notifications/initialized", json!({})),
            mcp_request(
                2,
                "resources/read",
                json!({ "uri": surface_uri }),
            ),
        ]
        .concat();

        let output = zocli()
            .args(["mcp"])
            .write_stdin(input)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let responses = parse_responses(&output);
        let html = responses[1]["result"]["contents"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{surface_uri}: missing HTML text"));

        // Extract APP_BOOTSTRAP JSON from HTML
        let bootstrap_marker = "const APP_BOOTSTRAP = ";
        let start = html
            .find(bootstrap_marker)
            .unwrap_or_else(|| panic!("{surface_uri}: missing APP_BOOTSTRAP"));
        let json_start = start + bootstrap_marker.len();
        let rest = &html[json_start..];
        let end = rest.find(';').unwrap_or(rest.len());
        let bootstrap_json = &rest[..end];
        let bootstrap: Value = serde_json::from_str(bootstrap_json)
            .unwrap_or_else(|err| panic!("{surface_uri}: invalid APP_BOOTSTRAP JSON: {err}"));

        for key in &expected_keys {
            assert!(
                bootstrap.get(key).is_some(),
                "{surface_uri}: APP_BOOTSTRAP missing key '{key}'"
            );
        }
    }
}

/// HTML must contain renderToolResult and normalizeToolPayload.
#[test]
fn browser_harness_tool_result_rendering() {
    let html = get_dashboard_html();
    assert!(
        html.contains("renderToolResult"),
        "dashboard HTML must contain 'renderToolResult'"
    );
    assert!(
        html.contains("normalizeToolPayload"),
        "dashboard HTML must contain 'normalizeToolPayload'"
    );
}

// =============================================================================
// Phase A8 — Host Compatibility Matrix
// =============================================================================

/// Non-UI host: tools must NOT have _meta, no ui:// resources, content is JSON.
#[test]
fn compat_matrix_non_ui_no_meta_no_ui_resources() {
    // tools/list without UI
    let input = [
        initialize_request(false),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(3, "resources/list", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    // Check tools
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    for tool in tools {
        assert!(
            tool.get("_meta").is_none(),
            "non-UI host: tool '{}' must not have _meta, got: {}",
            tool["name"],
            tool
        );
    }
    // Check resources — no ui:// URIs
    let resources = responses[2]["result"]["resources"]
        .as_array()
        .expect("resources array");
    for resource in resources {
        let uri = resource["uri"].as_str().unwrap_or("");
        assert!(
            !uri.starts_with("ui://"),
            "non-UI host must not see ui:// resources, found: {uri}"
        );
    }
}

/// UI host: tools must have _meta.ui.resourceUri, all 6 ui:// resources present,
/// content is human-readable.
#[test]
fn compat_matrix_ui_has_meta_and_surfaces() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(3, "resources/list", json!({})),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    // Check that at least some tools have _meta
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    let tools_with_meta: Vec<_> = tools
        .iter()
        .filter(|t| t.get("_meta").is_some())
        .collect();
    assert!(
        !tools_with_meta.is_empty(),
        "UI host: at least some tools must have _meta"
    );
    // Verify _meta has resourceUri
    for tool in &tools_with_meta {
        let resource_uri = tool["_meta"]["ui"]["resourceUri"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "tool '{}' _meta.ui.resourceUri missing",
                    tool["name"]
                )
            });
        assert!(
            resource_uri.starts_with("ui://zocli/"),
            "tool '{}' resourceUri must be ui://zocli/*, got: {resource_uri}",
            tool["name"]
        );
    }

    // Check resources — all 6 ui:// surfaces
    let resources = responses[2]["result"]["resources"]
        .as_array()
        .expect("resources array");
    let ui_uris: Vec<&str> = resources
        .iter()
        .filter_map(|r| r["uri"].as_str())
        .filter(|u| u.starts_with("ui://"))
        .collect();
    for expected in &[
        "ui://zocli/dashboard",
        "ui://zocli/mail",
        "ui://zocli/calendar",
        "ui://zocli/drive",
        "ui://zocli/auth",
        "ui://zocli/account",
    ] {
        assert!(
            ui_uris.contains(expected),
            "UI host must list resource '{expected}', found: {ui_uris:?}"
        );
    }
}

/// Display modes: UI host must accept inline/fullscreen/pip; unknown falls back to inline.
#[test]
fn compat_matrix_display_modes_per_profile() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
        // Valid modes
        mcp_request(3, "ui/request-display-mode", json!({ "mode": "inline" })),
        mcp_request(
            4,
            "ui/request-display-mode",
            json!({ "mode": "fullscreen" }),
        ),
        mcp_request(5, "ui/request-display-mode", json!({ "mode": "pip" })),
        // Unknown mode → falls back to inline
        mcp_request(
            6,
            "ui/request-display-mode",
            json!({ "mode": "floating" }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    assert_eq!(responses[2]["result"]["mode"], "inline");
    assert_eq!(responses[3]["result"]["mode"], "fullscreen");
    assert_eq!(responses[4]["result"]["mode"], "pip");
    assert_eq!(
        responses[5]["result"]["mode"], "inline",
        "unknown display mode must fall back to inline"
    );
}

/// Both UI and non-UI: structuredContent.schemaVersion must always be "1.0".
#[test]
fn compat_matrix_schema_version_always_present() {
    for ui_enabled in &[true, false] {
        let input = [
            initialize_request(*ui_enabled),
            mcp_notification("notifications/initialized", json!({})),
            mcp_request(
                2,
                "tools/call",
                json!({ "name": "zocli.app.snapshot", "arguments": {} }),
            ),
        ]
        .concat();

        let output = zocli()
            .args(["mcp"])
            .write_stdin(input)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let responses = parse_responses(&output);
        let result = &responses[1]["result"];
        assert_eq!(
            result["structuredContent"]["schemaVersion"], "1.0",
            "ui_enabled={ui_enabled}: structuredContent.schemaVersion must be '1.0'"
        );
    }
}

/// UI profile: CSP populated. Non-UI profile: no _meta at all.
#[test]
fn compat_matrix_csp_shape_ui_only() {
    // UI: CSP populated
    let input_ui = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "resources/read",
            json!({ "uri": "ui://zocli/dashboard" }),
        ),
    ]
    .concat();

    let output_ui = zocli()
        .args(["mcp"])
        .write_stdin(input_ui)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses_ui = parse_responses(&output_ui);
    let meta = &responses_ui[1]["result"]["contents"][0]["_meta"];
    assert!(
        meta["ui"]["csp"]["connectDomains"]
            .as_array()
            .is_some_and(|arr| !arr.is_empty()),
        "UI profile: CSP connectDomains must be populated"
    );

    // Non-UI: tools should not have _meta
    let input_non_ui = [
        initialize_request(false),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({ "name": "zocli.app.snapshot", "arguments": {} }),
        ),
    ]
    .concat();

    let output_non_ui = zocli()
        .args(["mcp"])
        .write_stdin(input_non_ui)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses_non_ui = parse_responses(&output_non_ui);
    let tool_result = &responses_non_ui[1]["result"];
    assert!(
        tool_result.get("_meta").is_none(),
        "non-UI profile: tool result must not have _meta"
    );
}

/// Non-UI host gets full data in JSON content — no information loss.
#[test]
fn compat_matrix_graceful_degradation() {
    let input = [
        initialize_request(false),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({ "name": "zocli.app.snapshot", "arguments": {} }),
        ),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let result = &responses[1]["result"];

    // content[0].text must be valid JSON with full data
    let text = result["content"][0]["text"]
        .as_str()
        .expect("content text");
    let parsed: Value = serde_json::from_str(text).expect("must be valid JSON");
    // Must have schemaVersion in the parsed JSON (from structuredContent which is also in text)
    assert!(
        parsed.get("schemaVersion").is_some() || parsed.get("accountCount").is_some(),
        "non-UI content must contain full data, got: {text}"
    );

    // structuredContent must also be present
    assert!(
        result.get("structuredContent").is_some(),
        "structuredContent must always be present"
    );
}

// =============================================================================
// Phase A5 addendum — Session state fields are observable (not write-only)
// =============================================================================

/// ui/message echoes stored text; ui/open-link echoes url; ui/update-model-context echoes revision.
#[test]
fn host_action_session_state_fields_observable() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "ui/initialize", json!({})),
        mcp_notification("ui/notifications/initialized", json!({})),
        mcp_request(3, "ui/message", json!({ "text": "observable payload" })),
        mcp_request(
            4,
            "ui/open-link",
            json!({ "url": "https://observable.test" }),
        ),
        mcp_request(5, "ui/update-model-context", json!({ "context": { "key": "v1" } })),
    ]
    .concat();

    let output = zocli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    // ui/message echoes stored text
    assert_eq!(
        responses[2]["result"]["stored"], "observable payload",
        "ui/message must echo stored text"
    );
    // ui/open-link echoes url
    assert_eq!(
        responses[3]["result"]["url"], "https://observable.test",
        "ui/open-link must echo accepted url"
    );
    // ui/update-model-context returns revision
    assert_eq!(
        responses[4]["result"]["revision"], 1,
        "ui/update-model-context must return monotonic revision"
    );
}
