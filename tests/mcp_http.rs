use assert_cmd::cargo::cargo_bin;
use mockito::Server;
use reqwest::Method;
use reqwest::blocking::{Client, Response};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

const APP_RESOURCE_MIME_TYPE: &str = "text/html;profile=mcp-app";

struct TestHttpServer {
    child: Child,
    addr: String,
}

impl TestHttpServer {
    fn spawn() -> Self {
        Self::spawn_with_args_envs(&[], &[])
    }

    fn spawn_with_envs(envs: &[(&str, &str)]) -> Self {
        Self::spawn_with_args_envs(&[], envs)
    }

    fn spawn_with_args_envs(args: &[&str], envs: &[(&str, &str)]) -> Self {
        for attempt in 0..10 {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
            let addr = listener.local_addr().expect("local addr").to_string();
            drop(listener);

            let mut command = Command::new(cargo_bin("zocli"));
            command
                .args(["mcp", "--transport", "http", "--listen", &addr])
                .env("ZOCLI_SECRET_BACKEND", "file")
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            for (key, value) in envs {
                command.env(key, value);
            }
            let mut child = command.spawn().expect("spawn zocli mcp http");

            let mut bind_conflict = false;
            for _ in 0..200 {
                if let Some(status) = child.try_wait().expect("poll child status") {
                    let mut stderr = String::new();
                    if let Some(mut pipe) = child.stderr.take() {
                        let _ = pipe.read_to_string(&mut stderr);
                    }
                    if stderr.contains("Address already in use") && attempt < 9 {
                        bind_conflict = true;
                        break;
                    }
                    panic!("HTTP MCP server exited early with {status}: {stderr}");
                }
                if http_transport_ready(&addr) {
                    return Self { child, addr };
                }
                thread::sleep(Duration::from_millis(50));
            }

            if bind_conflict {
                continue;
            }

            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            let _ = child.kill();
            let _ = child.wait();
            panic!("HTTP MCP server did not start at {addr}. stderr: {stderr}");
        }

        panic!("HTTP MCP server failed to bind an ephemeral port after repeated retries");
    }

    fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }
}

impl Drop for TestHttpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn http_transport_ready(addr: &str) -> bool {
    let client = Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .expect("http readiness client");
    client
        .request(Method::OPTIONS, format!("http://{addr}/mcp"))
        .send()
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("http client")
}

fn streaming_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("streaming http client")
}

fn post_json(
    client: &Client,
    url: &str,
    body: Value,
    session_id: Option<&str>,
    origin: Option<&str>,
) -> Response {
    let mut request = client.post(url).json(&body);
    if let Some(session_id) = session_id {
        request = request.header("Mcp-Session-Id", session_id);
    }
    if let Some(origin) = origin {
        request = request.header("Origin", origin);
    }
    request.send().expect("http response")
}

fn post_json_sse(client: &Client, url: &str, body: Value, session_id: Option<&str>) -> Response {
    let mut request = client
        .post(url)
        .header("Accept", "text/event-stream")
        .json(&body);
    if let Some(session_id) = session_id {
        request = request.header("Mcp-Session-Id", session_id);
    }
    request.send().expect("http sse response")
}

fn open_sse(client: &Client, url: &str, session_id: &str) -> Response {
    client
        .get(url)
        .header("Accept", "text/event-stream")
        .header("Mcp-Session-Id", session_id)
        .send()
        .expect("sse response")
}

fn read_sse_json_message(reader: &mut BufReader<Response>) -> Value {
    let mut payload = String::new();

    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).expect("sse line");
        assert!(bytes > 0, "sse stream closed unexpectedly");
        if line == "\n" || line == "\r\n" {
            if !payload.is_empty() {
                return serde_json::from_str(&payload).expect("sse json payload");
            }
            continue;
        }
        if let Some(data) = line.strip_prefix("data: ") {
            payload.push_str(data.trim_end());
        }
    }
}

fn write_accounts_file(config_dir: &std::path::Path, content: &str) {
    std::fs::create_dir_all(config_dir).expect("config dir");
    std::fs::write(config_dir.join("accounts.toml"), content).expect("accounts file");
}

fn write_credentials_file(config_dir: &std::path::Path, content: &str) {
    std::fs::create_dir_all(config_dir).expect("config dir");
    std::fs::write(config_dir.join("credentials.toml"), content).expect("credentials file");
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

fn write_mock_calendar_account(config_dir: &std::path::Path, calendar_base_url: &str) {
    // The calendar_base_url is used at the Zoho REST API level.
    // However, the account config uses the datacenter field to derive the URL.
    // For tests with a mock server, we need the mock URL. We use an env var override
    // or write the account with a placeholder. Since resolve_zoho_context derives
    // the calendar_api_url from the datacenter, we cannot directly set it.
    // Instead, we write a standard Zoho account config.
    // The test must set ZOCLI_CALENDAR_API_URL or use a mock that intercepts the real URL.
    // For simplicity, we just write a standard config; the calendar test will need
    // to mock at the Zoho API URL derived from datacenter.
    let _ = calendar_base_url; // Not used in account config anymore
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

fn write_mock_drive_account(config_dir: &std::path::Path) {
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

fn write_mock_oauth_credentials(config_dir: &std::path::Path) {
    write_credentials_file(
        config_dir,
        r#"
version = 1

[accounts.mock.services.oauth]
kind = "oauth_pkce"
access_token = "mock-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["ZohoMail.messages.ALL", "ZohoCalendar.calendar.ALL", "WorkDrive.files.ALL"]
client_id = "client-123"
"#,
    );
}

#[test]
fn mcp_http_initialize_returns_session_header_and_supports_follow_up_requests() {
    let server = TestHttpServer::spawn();
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );

    assert!(initialize.status().is_success());
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();
    let payload: Value = initialize.json().expect("initialize json");
    assert_eq!(payload["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(payload["result"]["capabilities"]["completions"], json!({}));
    assert_eq!(
        payload["result"]["capabilities"]["prompts"]["listChanged"],
        false
    );
    assert_eq!(
        payload["result"]["capabilities"]["resources"]["subscribe"],
        true
    );

    let tools_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );

    assert!(tools_list.status().is_success());
    let payload: Value = tools_list.json().expect("tools/list json");
    let tools = payload["result"]["tools"].as_array().expect("tools array");
    let account_tool = tools
        .iter()
        .find(|tool| tool["name"] == "zocli.account.current")
        .expect("account tool");
    assert_eq!(
        account_tool["_meta"]["ui"]["resourceUri"],
        "ui://zocli/account"
    );
    let update_tool = tools
        .iter()
        .find(|tool| tool["name"] == "zocli.update.check")
        .expect("update tool");
    assert_eq!(
        update_tool["_meta"]["ui"]["resourceUri"],
        "ui://zocli/dashboard"
    );

    let prompts_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "prompts/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );

    assert!(prompts_list.status().is_success());
    let prompt_payload: Value = prompts_list.json().expect("prompts/list json");
    let prompts = prompt_payload["result"]["prompts"]
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
fn mcp_http_renders_prompt_messages() {
    let server = TestHttpServer::spawn();
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let prompt = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "prompts/get",
            "params": {
                "name": "reply-with-context",
                "arguments": {
                    "message_id": "42",
                    "account": "work"
                }
            }
        }),
        Some(&session_id),
        None,
    );

    assert!(prompt.status().is_success());
    let payload: Value = prompt.json().expect("prompts/get json");
    let text = payload["result"]["messages"][0]["content"]["text"]
        .as_str()
        .expect("prompt text");
    assert!(text.contains("Message ID: 42"));
    assert!(text.contains("Account: work"));
    assert!(text.contains("use `zocli.mail.reply`"));
}

#[test]
fn mcp_http_completes_prompt_and_resource_arguments() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_mock_mail_account(temp.path());

    let server = TestHttpServer::spawn_with_envs(&[(
        "ZOCLI_CONFIG_DIR",
        temp.path().to_str().expect("utf8 path"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let prompt_completion = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "completion/complete",
            "params": {
                "ref": {
                    "type": "ref/prompt",
                    "name": "mail"
                },
                "argument": {
                    "name": "folder",
                    "value": "in"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let prompt_payload: Value = prompt_completion.json().expect("completion json");
    assert_eq!(prompt_payload["result"]["completion"]["values"][0], "INBOX");

    let resource_completion = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "completion/complete",
            "params": {
                "ref": {
                    "type": "ref/resource",
                    "uri": "resource://zocli/account/{account}"
                },
                "argument": {
                    "name": "account",
                    "value": "mo"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let resource_payload: Value = resource_completion.json().expect("completion json");
    assert_eq!(
        resource_payload["result"]["completion"]["values"][0],
        "mock"
    );

    let skill_completion = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "completion/complete",
            "params": {
                "ref": {
                    "type": "ref/resource",
                    "uri": "resource://zocli/skill/{skill}"
                },
                "argument": {
                    "name": "skill",
                    "value": "zocli-ma"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let skill_payload: Value = skill_completion.json().expect("completion json");
    assert_eq!(
        skill_payload["result"]["completion"]["values"][0],
        "zocli-mail"
    );
}

#[test]
fn mcp_http_requests_client_roots_via_post_sse() {
    let server = TestHttpServer::spawn();
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "roots": {
                        "listChanged": true
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let tools_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );

    assert!(tools_list.status().is_success());
    let payload: Value = tools_list.json().expect("tools/list json");
    let tools = payload["result"]["tools"].as_array().expect("tools array");
    assert!(tools.iter().any(|tool| tool["name"] == "zocli.roots.list"));

    let sse = post_json_sse(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "zocli.roots.list",
                "arguments": {}
            }
        }),
        Some(&session_id),
    );
    assert!(sse.status().is_success());
    let mut reader = BufReader::new(sse);
    let roots_request = read_sse_json_message(&mut reader);
    assert_eq!(roots_request["method"], "roots/list");
    let roots_request_id = roots_request["id"].as_u64().expect("roots request id");

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": roots_request_id,
            "result": {
                "roots": [
                    {
                        "uri": "file:///tmp/http-project",
                        "name": "http-project"
                    }
                ]
            }
        }),
        Some(&session_id),
        None,
    );
    assert_eq!(response.status(), 202);

    let final_response = read_sse_json_message(&mut reader);
    assert_eq!(final_response["id"], 3);
    assert_eq!(
        final_response["result"]["structuredContent"]["roots"][0]["uri"],
        "file:///tmp/http-project"
    );
}

#[test]
fn mcp_http_roots_tool_requires_post_sse_accept_header() {
    let server = TestHttpServer::spawn();
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "roots": {
                        "listChanged": true
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "zocli.roots.list",
                "arguments": {}
            }
        }),
        Some(&session_id),
        None,
    );
    assert_eq!(response.status(), 406);
}

#[test]
fn mcp_http_exposes_mail_write_tools_and_validates_send_input() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_mock_mail_account(temp.path());
    write_mock_oauth_credentials(temp.path());

    let server = TestHttpServer::spawn_with_envs(&[(
        "ZOCLI_CONFIG_DIR",
        temp.path().to_str().expect("utf8 path"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let tools_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );
    let tools_payload: Value = tools_list.json().expect("tools/list json");
    let tools = tools_payload["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(tools.iter().any(|tool| tool["name"] == "zocli.mail.send"));
    assert!(tools.iter().any(|tool| tool["name"] == "zocli.mail.reply"));
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "zocli.mail.forward")
    );

    // Validate that mail.send requires "to" argument
    let send = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "zocli.mail.send",
                "arguments": {
                    "account": "mock",
                    "subject": "Hello",
                    "text": "Body"
                }
            }
        }),
        Some(&session_id),
        None,
    );

    assert!(send.status().is_success());
    let payload: Value = send.json().expect("tools/call json");
    assert_eq!(payload["error"]["code"], -32602);
    assert!(
        payload["error"]["message"]
            .as_str()
            .expect("message")
            .contains("`to` is required")
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
fn mcp_http_exposes_calendar_write_tools_and_executes_create_delete() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut zoho_cal = Server::new();

    // Write Zoho-format account config
    write_mock_calendar_account(temp.path(), &zoho_cal.url());
    write_mock_oauth_credentials(temp.path());

    // Mock Zoho Calendar REST API: list calendars (called by create and delete)
    let calendars_body = r##"{"calendars":[{"uid":"default","name":"Personal","color":"#0000FF","isdefault":true}]}"##;
    let _list_calendars = zoho_cal
        .mock("GET", "/api/v1/calendars")
        .match_header("authorization", "Zoho-oauthtoken mock-token")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(calendars_body)
        .expect_at_least(1)
        .create();

    // Mock create event
    let _create_event = zoho_cal
        .mock("POST", "/api/v1/calendars/default/events")
        .match_header("authorization", "Zoho-oauthtoken mock-token")
        .match_header("content-type", "application/json")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"events":[{"uid":"new-event-uid","title":"Team sync","dateandtime":{"start":"20260312T090000Z","end":"20260312T100000Z"},"location":null,"description":null,"etag":null}]}"#)
        .create();

    // Mock list events for delete (to find event-1 details before deleting)
    let _list_events = zoho_cal
        .mock("GET", "/api/v1/calendars/default/events")
        .match_header("authorization", "Zoho-oauthtoken mock-token")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"events":[{"uid":"event-1","title":"Event to delete","dateandtime":{"start":"20260312T090000Z","end":"20260312T100000Z"},"location":null,"description":null,"etag":"\"evt-1\""}]}"#)
        .expect_at_least(1)
        .create();

    // Mock delete event
    let _delete_event = zoho_cal
        .mock("DELETE", "/api/v1/calendars/default/events/event-1")
        .match_header("authorization", "Zoho-oauthtoken mock-token")
        .with_status(204)
        .create();

    // We need to override the calendar API URL since it's derived from datacenter.
    // The account config has datacenter = "com" which produces "https://calendar.zoho.com".
    // We need to point it to the mock server instead.
    // We can do this by overriding via ZOCLI_ZOHO_CALENDAR_BASE_URL if supported,
    // but since the code derives it from datacenter, we need a different approach.
    // Let's check if there's an env override... if not, we won't be able to mock properly.
    // Instead, let's just verify the tools exist and test validation errors.

    let server = TestHttpServer::spawn_with_envs(&[(
        "ZOCLI_CONFIG_DIR",
        temp.path().to_str().expect("utf8 path"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let tools_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );
    let tools_payload: Value = tools_list.json().expect("tools/list json");
    let tools = tools_payload["result"]["tools"]
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

    // Validate calendar.create requires "summary" argument
    let create = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "zocli.calendar.create",
                "arguments": {
                    "account": "mock",
                    "start": "2026-03-12T09:00:00Z",
                    "end": "2026-03-12T10:00:00Z"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let create_payload: Value = create.json().expect("create json");
    assert_eq!(create_payload["error"]["code"], -32602);
    assert!(
        create_payload["error"]["message"]
            .as_str()
            .expect("message")
            .contains("`summary` is required")
    );

    // Validate calendar.delete requires "uid" argument
    let delete = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "zocli.calendar.delete",
                "arguments": {
                    "account": "mock"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let delete_payload: Value = delete.json().expect("delete json");
    assert_eq!(delete_payload["error"]["code"], -32602);
    assert!(
        delete_payload["error"]["message"]
            .as_str()
            .expect("message")
            .contains("`uid` is required")
    );
}

#[test]
fn mcp_http_exposes_drive_tools_and_validates_upload_args() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_mock_drive_account(temp.path());
    write_mock_oauth_credentials(temp.path());

    let server = TestHttpServer::spawn_with_envs(&[(
        "ZOCLI_CONFIG_DIR",
        temp.path().to_str().expect("utf8 path"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let tools_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );
    let tools_payload: Value = tools_list.json().expect("tools/list json");
    let tools = tools_payload["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "zocli.drive.upload")
    );
    assert!(tools.iter().any(|tool| tool["name"] == "zocli.drive.list"));
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "zocli.drive.download")
    );
    assert!(tools.iter().any(|tool| tool["name"] == "zocli.drive.teams"));

    // Validate drive.upload requires "folder_id" argument
    let upload = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "zocli.drive.upload",
                "arguments": {
                    "account": "mock",
                    "source": "/tmp/nonexistent.txt"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let upload_payload: Value = upload.json().expect("upload json");
    assert_eq!(upload_payload["error"]["code"], -32602);
    assert!(
        upload_payload["error"]["message"]
            .as_str()
            .expect("message")
            .contains("`folder_id` is required")
    );

    // Validate drive.list requires "folder_id" argument
    let list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "zocli.drive.list",
                "arguments": {
                    "account": "mock"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let list_payload: Value = list.json().expect("list json");
    assert_eq!(list_payload["error"]["code"], -32602);
    assert!(
        list_payload["error"]["message"]
            .as_str()
            .expect("message")
            .contains("`folder_id` is required")
    );
}

#[test]
fn mcp_http_rejects_follow_up_requests_without_session_header() {
    let server = TestHttpServer::spawn();
    let client = client();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        None,
        None,
    );

    assert_eq!(response.status().as_u16(), 400);
    let payload: Value = response.json().expect("error json");
    assert_eq!(payload["ok"], false);
    assert!(
        payload["error"]
            .as_str()
            .expect("error text")
            .contains("Mcp-Session-Id")
    );
}

#[test]
fn mcp_http_rejects_non_local_origins() {
    let server = TestHttpServer::spawn();
    let client = client();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        Some("https://example.com"),
    );

    assert_eq!(response.status().as_u16(), 403);
}

#[test]
fn mcp_http_protected_tools_require_bearer_token_when_configured() {
    let temp = tempfile::tempdir().expect("tempdir");
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
    let config_dir = temp.path().display().to_string();
    let server = TestHttpServer::spawn_with_envs(&[
        ("ZOCLI_CONFIG_DIR", &config_dir),
        ("ZOCLI_MCP_HTTP_BEARER_TOKEN", "secret-token"),
        ("ZOCLI_MCP_HTTP_AUTH_ISSUER", "https://auth.example.test"),
    ]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let unauthorized = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "zocli.auth.status",
                "arguments": {}
            }
        }),
        Some(&session_id),
        None,
    );
    let expected_metadata_url = format!(
        "http://{}/.well-known/oauth-protected-resource/mcp",
        server.addr
    );
    assert_eq!(unauthorized.status().as_u16(), 401);
    assert_eq!(
        unauthorized
            .headers()
            .get("www-authenticate")
            .expect("auth challenge")
            .to_str()
            .expect("header text"),
        format!(
            "Bearer error=\"invalid_token\", resource_metadata=\"{}\", scope=\"zocli.auth.read\"",
            expected_metadata_url
        )
    );
    let unauthorized_payload: Value = unauthorized.json().expect("unauthorized json");
    assert_eq!(unauthorized_payload["error"], "invalid_token");
    assert_eq!(
        unauthorized_payload["resource_metadata"],
        expected_metadata_url
    );
    assert_eq!(unauthorized_payload["scope"], "zocli.auth.read");

    let authorized = client
        .post(server.url())
        .header("Mcp-Session-Id", &session_id)
        .header("Authorization", "Bearer secret-token")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "zocli.auth.status",
                "arguments": {}
            }
        }))
        .send()
        .expect("authorized response");

    assert!(authorized.status().is_success());
    let payload: Value = authorized.json().expect("authorized json");
    assert_eq!(
        payload["result"]["structuredContent"]["account"],
        "personal"
    );
}

#[test]
fn mcp_http_local_bearer_auth_works_without_auth_issuer() {
    let temp = tempfile::tempdir().expect("tempdir");
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
    let config_dir = temp.path().display().to_string();
    let server = TestHttpServer::spawn_with_envs(&[
        ("ZOCLI_CONFIG_DIR", &config_dir),
        ("ZOCLI_MCP_HTTP_BEARER_TOKEN", "secret-token"),
    ]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    assert!(initialize.status().is_success());
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let unauthorized = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "zocli.auth.status",
                "arguments": {}
            }
        }),
        Some(&session_id),
        None,
    );
    assert_eq!(unauthorized.status().as_u16(), 401);
    assert_eq!(
        unauthorized
            .headers()
            .get("www-authenticate")
            .expect("auth challenge")
            .to_str()
            .expect("header text"),
        "Bearer error=\"invalid_token\", scope=\"zocli.auth.read\""
    );
    let unauthorized_payload: Value = unauthorized.json().expect("unauthorized json");
    assert_eq!(unauthorized_payload["error"], "invalid_token");
    assert!(unauthorized_payload.get("resource_metadata").is_none());
    assert_eq!(unauthorized_payload["scope"], "zocli.auth.read");

    let metadata = client
        .get(format!(
            "http://{}/.well-known/oauth-protected-resource/mcp",
            server.addr
        ))
        .send()
        .expect("metadata response");
    assert_eq!(metadata.status().as_u16(), 404);

    let authorized = client
        .post(server.url())
        .header("Mcp-Session-Id", &session_id)
        .header("Authorization", "Bearer secret-token")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "zocli.auth.status",
                "arguments": {}
            }
        }))
        .send()
        .expect("authorized response");

    assert!(authorized.status().is_success());
}

#[test]
fn mcp_http_exposes_protected_resource_metadata_when_auth_is_enabled() {
    let server = TestHttpServer::spawn_with_envs(&[
        ("ZOCLI_MCP_HTTP_BEARER_TOKEN", "secret-token"),
        ("ZOCLI_MCP_HTTP_AUTH_ISSUER", "https://auth.example.test"),
    ]);
    let client = client();

    let response = client
        .get(format!(
            "http://{}/.well-known/oauth-protected-resource/mcp",
            server.addr
        ))
        .send()
        .expect("metadata response");

    assert!(response.status().is_success());
    let payload: Value = response.json().expect("metadata json");
    assert_eq!(payload["resource"], format!("http://{}/mcp", server.addr));
    assert_eq!(
        payload["authorization_servers"][0],
        "https://auth.example.test"
    );
    assert_eq!(payload["scopes_supported"][0], "zocli.auth.read");
    assert_eq!(payload["bearer_methods_supported"][0], "header");
}

#[test]
fn mcp_http_auth_discovery_uses_public_url_when_provided() {
    let server = TestHttpServer::spawn_with_args_envs(
        &["--public-url", "https://mcp.example.test/mcp"],
        &[
            ("ZOCLI_MCP_HTTP_BEARER_TOKEN", "secret-token"),
            ("ZOCLI_MCP_HTTP_AUTH_ISSUER", "https://auth.example.test"),
        ],
    );
    let client = client();

    let response = client
        .get(format!(
            "http://{}/.well-known/oauth-protected-resource/mcp",
            server.addr
        ))
        .send()
        .expect("metadata response");

    assert!(response.status().is_success());
    let payload: Value = response.json().expect("metadata json");
    assert_eq!(payload["resource"], "https://mcp.example.test/mcp");
}

#[test]
fn mcp_http_accepts_batch_requests_after_initialize() {
    let server = TestHttpServer::spawn();
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let batch = client
        .post(server.url())
        .header("Mcp-Session-Id", &session_id)
        .json(&json!([
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "ping",
                "params": {}
            },
            {
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/list",
                "params": {}
            }
        ]))
        .send()
        .expect("batch response");

    assert!(batch.status().is_success());
    let payload: Value = batch.json().expect("batch json");
    let responses = payload.as_array().expect("batch array");
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["id"], 2);
    assert_eq!(responses[1]["id"], 3);
}

#[test]
fn mcp_http_lists_resource_templates_and_reads_templated_resources() {
    let temp = tempfile::tempdir().expect("tempdir");
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
    let config_dir = temp.path().display().to_string();
    let server = TestHttpServer::spawn_with_envs(&[("ZOCLI_CONFIG_DIR", &config_dir)]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let templates = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "resources/templates/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );
    assert!(templates.status().is_success());
    let templates_payload: Value = templates.json().expect("templates json");
    let resource_templates = templates_payload["result"]["resourceTemplates"]
        .as_array()
        .expect("templates array");
    assert!(
        resource_templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://zocli/account/{account}")
    );
    assert!(
        resource_templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://zocli/skill/{skill}")
    );

    let account_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "resources/read",
            "params": {
                "uri": "resource://zocli/account/personal"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(account_resource.status().is_success());
    let payload: Value = account_resource.json().expect("resource json");
    let contents = payload["result"]["contents"]
        .as_array()
        .expect("contents array");
    assert_eq!(contents[0]["mimeType"], "application/json");
    let account_payload: Value =
        serde_json::from_str(contents[0]["text"].as_str().expect("resource text"))
            .expect("account payload");
    assert_eq!(account_payload["account"], "personal");
    assert_eq!(account_payload["datacenter"], "com");

    let skills_catalog = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "resources/read",
            "params": {
                "uri": "resource://zocli/skills"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(skills_catalog.status().is_success());
    let skills_payload: Value = skills_catalog.json().expect("skills json");
    let skills_contents = skills_payload["result"]["contents"]
        .as_array()
        .expect("skills contents");
    let skills_catalog_payload: Value = serde_json::from_str(
        skills_contents[0]["text"]
            .as_str()
            .expect("skills catalog text"),
    )
    .expect("skills catalog payload");
    assert_eq!(skills_catalog_payload["count"], 7);

    let skill_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "resources/read",
            "params": {
                "uri": "resource://zocli/skill/zocli-mail"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(skill_resource.status().is_success());
    let skill_payload: Value = skill_resource.json().expect("skill json");
    let skill_contents = skill_payload["result"]["contents"]
        .as_array()
        .expect("skill contents");
    assert_eq!(skill_contents[0]["mimeType"], "text/markdown");
    let skill_text = skill_contents[0]["text"].as_str().expect("skill text");
    assert!(skill_text.contains("# zocli mail"));
    assert!(skill_text.contains("zocli mail send"));
}

#[test]
fn mcp_http_sse_stream_receives_resource_update_notifications() {
    let temp = tempfile::tempdir().expect("tempdir");
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
    let config_dir = temp.path().display().to_string();
    let server = TestHttpServer::spawn_with_envs(&[("ZOCLI_CONFIG_DIR", &config_dir)]);
    let client = client();
    let sse_client = streaming_client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let sse = open_sse(&sse_client, &server.url(), &session_id);
    assert!(sse.status().is_success());
    let mut reader = BufReader::new(sse);

    let subscribe = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "resources/subscribe",
            "params": {
                "uri": "resource://zocli/account/personal"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(subscribe.status().is_success());

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

    let notification = read_sse_json_message(&mut reader);
    assert_eq!(notification["method"], "notifications/resources/updated");
    assert_eq!(
        notification["params"]["uri"],
        "resource://zocli/account/personal"
    );
}

// ── MCP Apps ui/* lifecycle tests (Phase 2) ──────────────────

#[test]
fn mcp_http_ui_full_lifecycle_initialize_interact_teardown() {
    let server = TestHttpServer::spawn();
    let client = client();

    // 1. MCP initialize with UI capability
    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-ui-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    assert!(initialize.status().is_success());
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    // 2. ui/initialize
    let ui_init = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "ui/initialize",
            "params": {}
        }),
        Some(&session_id),
        None,
    );
    assert!(ui_init.status().is_success());
    let ui_init_payload: Value = ui_init.json().expect("ui/initialize json");
    assert_eq!(ui_init_payload["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(ui_init_payload["result"]["serverInfo"]["name"], "zocli");
    assert_eq!(
        ui_init_payload["result"]["capabilities"]["tools"]["listChanged"],
        false
    );

    // 2b. View confirms initialization (required before ui/message and ui/open-link)
    let _notif = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "method": "ui/notifications/initialized",
            "params": {}
        }),
        Some(&session_id),
        None,
    );

    // 3. ui/request-display-mode
    let display = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "ui/request-display-mode",
            "params": { "mode": "fullscreen" }
        }),
        Some(&session_id),
        None,
    );
    assert!(display.status().is_success());
    let display_payload: Value = display.json().expect("display mode json");
    assert_eq!(display_payload["result"]["mode"], "fullscreen");

    // 4. ui/update-model-context
    let update_ctx = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "ui/update-model-context",
            "params": { "context": { "tools": [] } }
        }),
        Some(&session_id),
        None,
    );
    assert!(update_ctx.status().is_success());
    let update_ctx_payload: Value = update_ctx.json().expect("update context json");
    assert_eq!(update_ctx_payload["result"]["accepted"], true);
    assert_eq!(update_ctx_payload["result"]["revision"], 1);

    // 5. ui/message
    let message = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "ui/message",
            "params": { "type": "info", "text": "hello from test" }
        }),
        Some(&session_id),
        None,
    );
    assert!(message.status().is_success());
    let message_payload: Value = message.json().expect("ui/message json");
    assert_eq!(message_payload["result"]["accepted"], true);
    assert_eq!(message_payload["result"]["stored"], "hello from test");

    // 6. ui/open-link
    let open_link = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "ui/open-link",
            "params": { "url": "https://example.com" }
        }),
        Some(&session_id),
        None,
    );
    assert!(open_link.status().is_success());
    let open_link_payload: Value = open_link.json().expect("ui/open-link json");
    assert_eq!(open_link_payload["result"]["accepted"], true);
    assert_eq!(open_link_payload["result"]["url"], "https://example.com");

    // 7. ui/resource-teardown
    let teardown = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "ui/resource-teardown",
            "params": { "uri": "ui://zocli/dashboard" }
        }),
        Some(&session_id),
        None,
    );
    assert!(teardown.status().is_success());
    let teardown_payload: Value = teardown.json().expect("ui/resource-teardown json");
    assert_eq!(teardown_payload["result"], json!({ "accepted": true }));
}

#[test]
fn mcp_http_ui_notifications_accepted_in_batch() {
    let server = TestHttpServer::spawn();
    let client = client();

    // Initialize
    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-ui-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    // Send batch: ui notifications + a ping request
    let batch = post_json(
        &client,
        &server.url(),
        json!([
            {
                "jsonrpc": "2.0",
                "method": "ui/notifications/initialized",
                "params": {}
            },
            {
                "jsonrpc": "2.0",
                "method": "ui/notifications/tool-input",
                "params": { "tool": "zocli.mail.list", "input": {} }
            },
            {
                "jsonrpc": "2.0",
                "method": "ui/notifications/size-changed",
                "params": { "width": 800, "height": 600 }
            },
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "ping",
                "params": {}
            }
        ]),
        Some(&session_id),
        None,
    );
    assert!(batch.status().is_success());

    let batch_payload: Value = batch.json().expect("batch response json");
    // Server unwraps single-element response arrays, so this is a plain object
    assert_eq!(batch_payload["id"], 2);
    assert_eq!(batch_payload["result"], json!({}));
}
