//! Browser-backed MCP Apps conformance harness.
//!
//! Launches headless Chrome with an ephemeral HTTP server to test the real
//! shipped dashboard HTML in a browser context via postMessage JSON-RPC.

use anyhow::{Context, Result};
use assert_cmd::cargo::CommandCargoExt;
use headless_chrome::{Browser, LaunchOptions};
use serde_json::{Value, json};
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// MCP stdio protocol helpers (minimal copies from mcp_server.rs)
// ---------------------------------------------------------------------------

/// Format a JSON-RPC request with Content-Length framing.
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

/// Format a JSON-RPC notification with Content-Length framing.
fn mcp_notification(method: &str, params: Value) -> String {
    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    }))
    .expect("notification json");
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

/// Parse Content-Length-framed JSON-RPC responses from raw stdout bytes.
fn parse_responses(stdout: &[u8]) -> Vec<Value> {
    let raw = String::from_utf8(stdout.to_vec()).expect("utf8 output");
    let mut remaining = raw.as_str();
    let mut responses = Vec::new();
    while !remaining.is_empty() {
        let (headers, rest) = remaining
            .split_once("\r\n\r\n")
            .expect("header separator");
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

// ---------------------------------------------------------------------------
// Runtime HTML extraction via MCP stdio
// ---------------------------------------------------------------------------

/// Extract the runtime HTML for a given MCP Apps surface by spawning
/// `zocli mcp` and reading the resource via stdio JSON-RPC.
///
/// This is the same path a real MCP host uses — no source template shortcuts.
fn extract_runtime_html(surface: &str) -> Result<String> {
    let uri = format!("ui://zocli/{surface}");

    let init = mcp_request(
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {
                "extensions": {
                    "io.modelcontextprotocol/ui": {
                        "mimeTypes": ["text/html;profile=mcp-app"]
                    }
                }
            },
            "clientInfo": { "name": "browser-harness", "version": "0.1.0" }
        }),
    );
    let init_notif = mcp_notification("notifications/initialized", json!({}));
    let read = mcp_request(2, "resources/read", json!({ "uri": uri }));

    let input = format!("{init}{init_notif}{read}");

    let output = Command::cargo_bin("zocli")
        .context("cargo bin")?
        .args(["mcp"])
        .env("ZOCLI_SECRET_BACKEND", "file")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(input.as_bytes())?;
            drop(child.stdin.take());
            child.wait_with_output()
        })
        .context("spawn zocli mcp")?;

    let responses = parse_responses(&output.stdout);
    // Find the resources/read response by id (not position — server may
    // emit notifications between responses).
    let read_result = responses
        .iter()
        .find(|r| r.get("id").and_then(|v| v.as_u64()) == Some(2))
        .context("expected resources/read response with id=2")?;

    read_result["result"]["contents"][0]["text"]
        .as_str()
        .map(|s| s.to_string())
        .context("no HTML text in resources/read response")
}

// ---------------------------------------------------------------------------
// Ephemeral HTTP server
// ---------------------------------------------------------------------------

/// Spawn an ephemeral HTTP server on 127.0.0.1:0 that serves:
/// - `GET /host.html` → the test host page
/// - `GET /app.html`  → the runtime dashboard HTML
///
/// Returns the bound address. The server thread runs until the browser/test
/// process exits; no global/shared server is used.
fn spawn_http_server(host_html: String, app_html: String) -> Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind ephemeral port")?;
    let addr = listener.local_addr()?;
    let host = Arc::new(host_html);
    let app = Arc::new(app_html);

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let host = host.clone();
            let app = app.clone();
            std::thread::spawn(move || {
                let _ = handle_http_request(stream, &host, &app);
            });
        }
    });

    Ok(addr)
}

fn handle_http_request(
    mut stream: std::net::TcpStream,
    host_html: &str,
    app_html: &str,
) -> Result<()> {
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf)?;
    let request = String::from_utf8_lossy(&buf[..n]);

    let body = if request.contains("GET /app.html") {
        app_html
    } else {
        host_html
    };

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// BrowserHarness
// ---------------------------------------------------------------------------

const HOST_HTML: &str = include_str!("host.html");

/// Real browser-backed harness for MCP Apps conformance testing.
///
/// Extracts runtime HTML from `zocli mcp`, serves it via ephemeral HTTP,
/// launches headless Chrome, and provides methods to observe and drive
/// the postMessage JSON-RPC protocol.
pub struct BrowserHarness {
    tab: Arc<headless_chrome::Tab>,
    _browser: Browser,
    addr: SocketAddr,
}

impl BrowserHarness {
    /// Launch the harness for a given surface (e.g. "dashboard").
    ///
    /// Returns `Err` if Chrome is not available (graceful skip in tests).
    pub fn new(surface: &str) -> Result<Self> {
        // 1. Extract runtime HTML via MCP stdio
        let app_html =
            extract_runtime_html(surface).context("extract runtime HTML")?;

        // 2. Spawn ephemeral HTTP server
        let addr = spawn_http_server(HOST_HTML.to_string(), app_html)?;

        // 3. Launch headless Chrome
        let options = LaunchOptions::default_builder()
            .headless(true)
            .sandbox(false)
            .window_size(Some((1280, 720)))
            .idle_browser_timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let browser = Browser::new(options)
            .context("launch headless Chrome — is chromium installed?")?;
        let tab = browser.new_tab().context("open new tab")?;

        // 4. Navigate to host page (which loads app in iframe)
        let url = format!("http://{addr}/host.html");
        tab.navigate_to(&url)?.wait_until_navigated()?;

        // 5. Wait for iframe to load
        std::thread::sleep(Duration::from_millis(500));

        Ok(Self {
            tab,
            _browser: browser,
            addr,
        })
    }

    /// Read all JSON-RPC messages the app has sent via postMessage.
    pub fn sent_messages(&self) -> Vec<Value> {
        let result = self
            .tab
            .evaluate("JSON.stringify(window.__HARNESS__.sent)", false)
            .ok()
            .and_then(|r| r.value)
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| "[]".to_string());
        serde_json::from_str(&result).unwrap_or_default()
    }

    /// Send a JSON-RPC message into the app iframe (as if from the host).
    pub fn inject(&self, message: &Value) {
        let js = format!(
            "window.__HARNESS__.inject({})",
            serde_json::to_string(message).unwrap()
        );
        let _ = self.tab.evaluate(&js, false);
    }

    /// Evaluate a JS expression and return its string value.
    pub fn eval_string(&self, expression: &str) -> String {
        self.tab
            .evaluate(expression, false)
            .ok()
            .and_then(|r| r.value)
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default()
    }

    /// Poll `sent_messages()` until a message with the given method appears.
    pub fn wait_for_message(&self, method: &str, timeout: Duration) -> Option<Value> {
        let start = Instant::now();
        loop {
            for msg in self.sent_messages() {
                if msg.get("method").and_then(|m| m.as_str()) == Some(method) {
                    return Some(msg);
                }
            }
            if start.elapsed() > timeout {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Poll `sent_messages()` until a response with the given `id` appears.
    pub fn wait_for_response(&self, id: u64, timeout: Duration) -> Option<Value> {
        let start = Instant::now();
        loop {
            for msg in self.sent_messages() {
                let matches_id = msg.get("id").and_then(|v| v.as_u64()) == Some(id);
                let is_response = msg.get("result").is_some() || msg.get("error").is_some();
                if matches_id && is_response {
                    return Some(msg);
                }
            }
            if start.elapsed() > timeout {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Evaluate JS inside the app iframe's document.
    fn eval_in_app(&self, expression: &str) -> String {
        let js = format!(
            "(function() {{ var d = document.getElementById('app').contentDocument; return {expression}; }})()"
        );
        self.eval_string(&js)
    }

    /// Read the app's current status text from the DOM.
    pub fn app_status(&self) -> String {
        self.eval_in_app(
            "d.getElementById('status-text') \
             ? d.getElementById('status-text').textContent : ''",
        )
    }

    /// Read the status badge state attribute (ready/running/error/booting).
    pub fn app_status_state(&self) -> String {
        self.eval_in_app(
            "d.getElementById('status-badge') \
             ? (d.getElementById('status-badge').dataset.state || '') : ''",
        )
    }

    /// Read the current display mode from `#display-mode` span.
    pub fn display_mode_text(&self) -> String {
        self.eval_in_app(
            "d.getElementById('display-mode') \
             ? d.getElementById('display-mode').textContent : ''",
        )
    }

    /// Read the display mode from body's data-display-mode attribute.
    pub fn body_display_mode(&self) -> String {
        self.eval_in_app(
            "d.body ? (d.body.dataset.displayMode || '') : ''",
        )
    }

    /// Convenience: get the server address (for debug logging).
    #[allow(dead_code)]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}
