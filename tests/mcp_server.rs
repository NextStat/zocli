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
    assert_eq!(account_tool["_meta"]["ui"]["resourceUri"], APP_RESOURCE_URI);
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
    assert_eq!(dashboard["_meta"]["ui"]["csp"]["connectDomains"], json!([]));

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
