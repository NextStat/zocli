mod browser_harness;

use browser_harness::BrowserHarness;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create harness, respecting BROWSER_TESTS_REQUIRED env var.
/// When set to "1", browser init failures panic instead of skipping.
fn require_harness(surface: &str) -> Option<BrowserHarness> {
    match BrowserHarness::new(surface) {
        Ok(h) => Some(h),
        Err(e) => {
            if std::env::var("BROWSER_TESTS_REQUIRED").as_deref() == Ok("1") {
                panic!("browser harness required but failed: {e}");
            }
            eprintln!("skipping browser test: {e}");
            None
        }
    }
}

/// Auto-respond to catalog requests (tools/list, prompts/list, resources/list,
/// resources/templates/list, resources/read) with empty results until a
/// tools/call request for `tool_name` appears. Returns that tools/call message.
///
/// This mirrors what a real MCP host does: the app's refreshBrowserCatalogs()
/// sends these sequentially (each awaited), so we must answer each one before
/// the next appears.
fn drain_until_tool_call(
    harness: &BrowserHarness,
    tool_name: &str,
    timeout: Duration,
) -> Value {
    let start = Instant::now();
    let mut responded: HashSet<u64> = HashSet::new();

    loop {
        for msg in harness.sent_messages() {
            let Some(id) = msg.get("id").and_then(|v| v.as_u64()) else {
                continue;
            };
            if responded.contains(&id) {
                continue;
            }
            let method = match msg.get("method").and_then(|m| m.as_str()) {
                Some(m) => m,
                None => continue,
            };

            // Is this the tools/call we're waiting for?
            if method == "tools/call" {
                if let Some(name) = msg.pointer("/params/name").and_then(|n| n.as_str()) {
                    if name == tool_name {
                        return msg;
                    }
                }
            }

            // Auto-respond to catalog requests with empty results
            responded.insert(id);
            let result = match method {
                "tools/list" => json!({"tools": []}),
                "prompts/list" => json!({"prompts": []}),
                "resources/list" => json!({"resources": []}),
                "resources/templates/list" => json!({"resourceTemplates": []}),
                "resources/read" => json!({"contents": []}),
                _ => json!({}),
            };
            harness.inject(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            }));
        }

        assert!(
            start.elapsed() < timeout,
            "timed out waiting for tools/call {tool_name}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Drive the harness through the full init lifecycle so tests can focus on
/// their specific concern. Leaves the harness in "ready" state.
///
/// Handles: ui/initialize → notifications/initialized → catalog drain →
/// tools/call(snapshot) → mock response → ready.
fn complete_lifecycle(harness: &BrowserHarness) {
    // 1. ui/initialize
    let init_msg = harness
        .wait_for_message("ui/initialize", TIMEOUT)
        .expect("ui/initialize");
    let init_id = init_msg["id"].as_u64().unwrap();
    harness.inject(&json!({
        "jsonrpc": "2.0",
        "id": init_id,
        "result": {
            "hostCapabilities": {
                "openLinks": true,
                "message": { "text": true },
                "updateModelContext": true,
                "serverResources": { "subscribe": true }
            },
            "hostContext": {
                "displayMode": "inline",
                "theme": "light"
            }
        }
    }));

    // 2. notifications/initialized
    harness
        .wait_for_message("ui/notifications/initialized", TIMEOUT)
        .expect("notifications/initialized");

    // 3. Drain catalog requests, then respond to snapshot tool call
    let tool_call = drain_until_tool_call(harness, "zocli.app.snapshot", TIMEOUT);
    let tool_id = tool_call["id"].as_u64().unwrap();
    harness.inject(&json!({
        "jsonrpc": "2.0",
        "id": tool_id,
        "result": {
            "structuredContent": {
                "schemaVersion": "1.0",
                "data": { "mock": true }
            },
            "content": [{ "type": "text", "text": "mock snapshot" }]
        }
    }));

    // 4. Wait for app to reach ready state
    std::thread::sleep(Duration::from_millis(500));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Full MCP Apps lifecycle: ui/initialize → notifications/initialized →
/// catalog refresh → tools/call → ready.
///
/// This test runs the REAL shipped dashboard HTML in a REAL browser.
/// No string-contains fakery — actual postMessage JSON-RPC over an iframe.
#[test]
fn browser_lifecycle_happy_path() {
    let Some(harness) = require_harness("dashboard") else {
        return;
    };

    // 1. App should have sent ui/initialize
    let init_msg = harness
        .wait_for_message("ui/initialize", TIMEOUT)
        .expect("app must send ui/initialize");

    // Verify protocol fields (APP_PROTOCOL_VERSION = "2025-11-25")
    assert_eq!(init_msg["params"]["protocolVersion"], "2025-11-25");
    let modes = &init_msg["params"]["appCapabilities"]["availableDisplayModes"];
    assert!(modes.is_array(), "availableDisplayModes must be array");
    assert!(
        modes.as_array().unwrap().contains(&json!("inline")),
        "must support inline"
    );

    // 2. Host responds to ui/initialize with capabilities + context
    let init_id = init_msg["id"].as_u64().expect("init has id");
    harness.inject(&json!({
        "jsonrpc": "2.0",
        "id": init_id,
        "result": {
            "hostCapabilities": {
                "openLinks": true,
                "message": { "text": true },
                "updateModelContext": true,
                "serverResources": { "subscribe": true }
            },
            "hostContext": {
                "displayMode": "inline",
                "theme": "light"
            }
        }
    }));

    // 3. App should send notifications/initialized
    let initialized = harness
        .wait_for_message("ui/notifications/initialized", TIMEOUT)
        .expect("app must send ui/notifications/initialized");
    assert_eq!(initialized["method"], "ui/notifications/initialized");

    // 4. App sends catalog requests then tools/call.
    //    drain_until_tool_call auto-responds to catalogs with empty results.
    let tool_call = drain_until_tool_call(&harness, "zocli.app.snapshot", TIMEOUT);
    assert_eq!(tool_call["params"]["name"], "zocli.app.snapshot");

    // 5. Host responds with mock snapshot
    let tool_id = tool_call["id"].as_u64().expect("tool call has id");
    harness.inject(&json!({
        "jsonrpc": "2.0",
        "id": tool_id,
        "result": {
            "structuredContent": {
                "schemaVersion": "1.0",
                "data": { "mock": true }
            },
            "content": [{ "type": "text", "text": "mock snapshot" }]
        }
    }));

    // 6. Give app time to process and reach ready state
    std::thread::sleep(Duration::from_millis(500));

    let status_text = harness.app_status();
    let status_state = harness.app_status_state();
    assert!(
        status_state == "ready"
            || status_text.to_lowercase().contains("snapshot")
            || status_text.to_lowercase().contains("ready"),
        "app should reach ready state, got state={status_state:?} text={status_text:?}"
    );
}

/// Host pushes display mode change via ui/notifications/host-context-changed.
/// App must update its internal state.displayMode to "fullscreen".
#[test]
fn browser_display_mode_change() {
    let Some(harness) = require_harness("dashboard") else {
        return;
    };

    complete_lifecycle(&harness);

    // Host sends display mode change (real method: ui/notifications/host-context-changed)
    harness.inject(&json!({
        "jsonrpc": "2.0",
        "method": "ui/notifications/host-context-changed",
        "params": {
            "hostContext": {
                "displayMode": "fullscreen",
                "theme": "light"
            }
        }
    }));

    // Give app time to process
    std::thread::sleep(Duration::from_millis(300));

    let display_mode_text = harness.display_mode_text();
    let body_attr = harness.body_display_mode();

    assert!(
        display_mode_text.contains("fullscreen") || body_attr.contains("fullscreen"),
        "app should reflect fullscreen display mode. #display-mode={display_mode_text:?}, body[data-display-mode]={body_attr:?}"
    );
}

/// Teardown: host sends ui/resource-teardown (request-style), app responds
/// and stops listening. Post-teardown messages must have no effect.
#[test]
fn browser_teardown_and_reinit() {
    let Some(harness) = require_harness("dashboard") else {
        return;
    };

    complete_lifecycle(&harness);

    // Host sends teardown as a request (with id) for observable proof
    harness.inject(&json!({
        "jsonrpc": "2.0",
        "id": 9999,
        "method": "ui/resource-teardown",
        "params": {}
    }));

    // App should respond with {jsonrpc: "2.0", id: 9999, result: {}}
    let teardown_response = harness
        .wait_for_response(9999, TIMEOUT)
        .expect("app must respond to teardown request");
    assert_eq!(teardown_response["id"], 9999);
    assert!(
        teardown_response.get("result").is_some(),
        "teardown response must have result"
    );

    // Status should indicate teardown
    let status = harness.app_status();
    assert!(
        status.to_lowercase().contains("teardown"),
        "app status should mention teardown, got: {status}"
    );

    // Record count after teardown response
    let count_after_teardown = harness.sent_messages().len();

    // Behavioral proof: inject another message after teardown.
    // App removed its message listener (line 2969), so it should NOT respond.
    harness.inject(&json!({
        "jsonrpc": "2.0",
        "id": 10000,
        "method": "ui/notifications/host-context-changed",
        "params": {
            "hostContext": { "displayMode": "pip" }
        }
    }));

    // Wait a bit for any potential (unwanted) reaction
    std::thread::sleep(Duration::from_millis(500));

    let count_after_ghost = harness.sent_messages().len();

    // No new messages should have been sent after teardown
    assert_eq!(
        count_after_teardown, count_after_ghost,
        "app must not send messages after teardown (sent {count_after_teardown} before, {count_after_ghost} after)"
    );

    // The status should NOT have changed to something other than teardown
    let final_status = harness.app_status();
    assert!(
        final_status.to_lowercase().contains("teardown"),
        "status must remain teardown after ghost message, got: {final_status}"
    );
}
