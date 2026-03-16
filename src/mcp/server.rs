use std::collections::{BTreeSet, HashMap, hash_map::DefaultHasher};
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header as http_header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rand::{Rng, distr::Alphanumeric};
use serde_json::{Map, Value, json};
use tokio::sync::{Mutex, broadcast, mpsc as tokio_mpsc, oneshot};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};
use url::Url;

use crate::account_store::AccountStore;
use crate::credential_store::CredentialStore;
use crate::error::{Result, ZocliError};
use crate::runtime_context::{auth_state, resolve_zoho_context};
use crate::update::check_for_update;
use crate::{
    calendar::{
        CalendarCreateRequest, CalendarEventsRequest, create_calendar_event, delete_calendar_event,
        list_calendar_events, list_calendars, parse_event_window,
    },
    disk::{download_file, list_files, list_teams, upload_file},
    mail::{
        download_attachment, forward_mail_message, get_attachment_info, list_mail_folders,
        list_mail_messages, read_mail_message, reply_to_mail_message, search_mail_messages,
        send_mail_message, upload_attachment,
    },
};
use chrono::{DateTime, Utc};

use super::{prompts, skills};

const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const APP_RESOURCE_URI: &str = "ui://zocli/dashboard";
const APP_RESOURCE_URI_TEMPLATE: &str =
    "ui://zocli/dashboard{?account,section,resource,tool,skill,prompt}";
const APP_MAIL_RESOURCE_URI: &str = "ui://zocli/mail";
const APP_CALENDAR_RESOURCE_URI: &str = "ui://zocli/calendar";
const APP_DRIVE_RESOURCE_URI: &str = "ui://zocli/drive";
const APP_AUTH_RESOURCE_URI: &str = "ui://zocli/auth";
const APP_ACCOUNT_RESOURCE_URI: &str = "ui://zocli/account";
const APP_RESOURCE_MIME_TYPE: &str = "text/html;profile=mcp-app";
const APP_EXTENSION_ID: &str = "io.modelcontextprotocol/ui";
const APP_SURFACES: &[&str] = &["dashboard", "mail", "calendar", "drive", "auth", "account"];
const APP_ACCOUNT_QUERY_PARAM: &str = "account";
const APP_SECTION_QUERY_PARAM: &str = "section";
const APP_RESOURCE_QUERY_PARAM: &str = "resource";
const APP_TOOL_QUERY_PARAM: &str = "tool";
const APP_SKILL_QUERY_PARAM: &str = "skill";
const APP_PROMPT_QUERY_PARAM: &str = "prompt";
const HTTP_MCP_PATH: &str = "/mcp";
const MCP_SESSION_HEADER: &str = "Mcp-Session-Id";
const HTTP_AUTH_TOKEN_ENV: &str = "ZOCLI_MCP_HTTP_BEARER_TOKEN";
const HTTP_AUTH_ISSUER_ENV: &str = "ZOCLI_MCP_HTTP_AUTH_ISSUER";
const MODEL_AND_APP_VISIBILITY: &[&str] = &["model", "app"];
const APP_ONLY_VISIBILITY: &[&str] = &["app"];
const RESOURCE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const SSE_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const PROTECTED_RESOURCE_METADATA_PATH: &str = "/.well-known/oauth-protected-resource";
const PROTECTED_RESOURCE_MCP_METADATA_PATH: &str = "/.well-known/oauth-protected-resource/mcp";
const DASHBOARD_SECTION_TOOLS: &str = "tools";
const DASHBOARD_SECTION_PROMPTS: &str = "prompts";
const DASHBOARD_SECTION_RESOURCES: &str = "resources";
const DASHBOARD_SECTION_AUTH: &str = "auth";
const DASHBOARD_RESOURCE_ACCOUNT: &str = "account";
const DASHBOARD_RESOURCE_AUTH: &str = "auth";
const DASHBOARD_RESOURCE_SKILLS: &str = "skills";
const DASHBOARD_RESOURCE_SKILL: &str = "skill";
const DASHBOARD_TOOL_APP_SNAPSHOT: &str = "zocli.app.snapshot";
const DASHBOARD_TOOL_ACCOUNT_LIST: &str = "zocli.account.list";
const DASHBOARD_TOOL_ACCOUNT_CURRENT: &str = "zocli.account.current";
const DASHBOARD_TOOL_AUTH_STATUS: &str = "zocli.auth.status";
const DASHBOARD_TOOL_UPDATE_CHECK: &str = "zocli.update.check";

struct DashboardResourceState {
    default_account: Option<String>,
    preferred_section: Option<String>,
    preferred_resource: Option<String>,
    preferred_tool: Option<String>,
    preferred_skill: Option<String>,
    preferred_prompt: Option<String>,
}

struct MailForwardToolRequest {
    folder_id: String,
    message_id: String,
    to: String,
    cc: Vec<String>,
    bcc: Vec<String>,
    content: Option<String>,
}

struct MailSendToolRequest {
    from_address: String,
    to: String,
    cc: Vec<String>,
    bcc: Vec<String>,
    subject: String,
    text: Option<String>,
    html: Option<String>,
    attachments: Vec<String>,
}

struct MailReplyToolRequest {
    folder_id: String,
    message_id: String,
    content: Option<String>,
    cc: Vec<String>,
}

struct SessionState {
    initialized: bool,
    ui_enabled: bool,
    ui_initialized: bool,
    ui_active_resource: Option<String>,
    supports_resource_subscriptions: bool,
    supports_roots_requests: bool,
    roots_list_changed_supported: bool,
    roots_dirty: bool,
    cached_roots: Option<Vec<Value>>,
    next_outbound_request_id: u64,
    resource_subscriptions: BTreeSet<String>,
    stdio_message_format: StdioMessageFormat,
}

impl SessionState {
    fn stdio() -> Self {
        Self {
            initialized: false,
            ui_enabled: false,
            ui_initialized: false,
            ui_active_resource: None,
            supports_resource_subscriptions: true,
            supports_roots_requests: false,
            roots_list_changed_supported: false,
            roots_dirty: false,
            cached_roots: None,
            next_outbound_request_id: 1,
            resource_subscriptions: BTreeSet::new(),
            stdio_message_format: StdioMessageFormat::ContentLength,
        }
    }

    fn http() -> Self {
        Self {
            initialized: false,
            ui_enabled: false,
            ui_initialized: false,
            ui_active_resource: None,
            supports_resource_subscriptions: true,
            supports_roots_requests: false,
            roots_list_changed_supported: false,
            roots_dirty: false,
            cached_roots: None,
            next_outbound_request_id: 1,
            resource_subscriptions: BTreeSet::new(),
            stdio_message_format: StdioMessageFormat::ContentLength,
        }
    }
}

#[derive(Default)]
struct ResourceSubscriptionPoller {
    digests: HashMap<String, u64>,
}

impl ResourceSubscriptionPoller {
    fn collect_notifications(&mut self, subscriptions: &BTreeSet<String>) -> Result<Vec<Value>> {
        self.digests
            .retain(|uri, _| subscriptions.contains(uri.as_str()));

        let mut notifications = Vec::new();
        for uri in subscriptions {
            let digest = resource_digest(uri)?;
            match self.digests.insert(uri.clone(), digest) {
                None => {}
                Some(previous) if previous != digest => {
                    notifications.push(resource_updated_notification(uri));
                }
                Some(_) => {}
            }
        }

        Ok(notifications)
    }

    fn track(&mut self, uri: &str) -> Result<()> {
        self.digests.insert(uri.to_string(), resource_digest(uri)?);
        Ok(())
    }

    fn untrack(&mut self, uri: &str) {
        self.digests.remove(uri);
    }
}

struct HttpSession {
    state: SessionState,
    poller: ResourceSubscriptionPoller,
    notifications: broadcast::Sender<Value>,
    pending_client_requests: HashMap<u64, oneshot::Sender<Value>>,
}

impl HttpSession {
    fn new() -> Self {
        let (notifications, _) = broadcast::channel(64);
        Self {
            state: SessionState::http(),
            poller: ResourceSubscriptionPoller::default(),
            notifications,
            pending_client_requests: HashMap::new(),
        }
    }
}

enum InputEvent {
    Message(Value, StdioMessageFormat),
    Eof,
    Error(ZocliError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StdioMessageFormat {
    ContentLength,
    JsonLine,
}

#[derive(Clone)]
struct HttpAppState {
    sessions: Arc<Mutex<HashMap<String, HttpSession>>>,
    auth: HttpAuthConfig,
    auth_discovery: Option<HttpAuthDiscovery>,
}

#[derive(Clone, Default)]
struct HttpAuthConfig {
    bearer_token: Option<String>,
}

#[derive(Clone)]
struct HttpAuthDiscovery {
    authorization_servers: Vec<String>,
    canonical_server_url: String,
    protected_resource_metadata_url: String,
}

pub fn serve_http(listen: &str, public_url: Option<&str>) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| ZocliError::Io(format!("failed to start HTTP runtime: {err}")))?;

    runtime.block_on(async move {
        let auth = HttpAuthConfig::from_env();
        let auth_discovery = HttpAuthDiscovery::from_config(listen, public_url, &auth)?;
        let state = HttpAppState {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            auth,
            auth_discovery,
        };
        spawn_http_resource_poller(state.clone());

        let app = Router::new()
            .route(
                PROTECTED_RESOURCE_METADATA_PATH,
                get(handle_http_protected_resource_metadata),
            )
            .route(
                PROTECTED_RESOURCE_MCP_METADATA_PATH,
                get(handle_http_protected_resource_metadata),
            )
            .route(
                HTTP_MCP_PATH,
                post(handle_http_post)
                    .get(handle_http_get)
                    .delete(handle_http_delete)
                    .options(handle_http_options),
            )
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(listen).await.map_err(|err| {
            ZocliError::Io(format!("failed to bind MCP HTTP server at {listen}: {err}"))
        })?;
        axum::serve(listener, app)
            .await
            .map_err(|err| ZocliError::Io(format!("MCP HTTP server failed: {err}")))
    })
}

pub fn serve_stdio() -> Result<()> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    let mut session = SessionState::stdio();
    let mut poller = ResourceSubscriptionPoller::default();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin.lock());

        loop {
            match read_message(&mut reader) {
                Ok(Some((message, format))) => {
                    if tx.send(InputEvent::Message(message, format)).is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = tx.send(InputEvent::Eof);
                    break;
                }
                Err(err) => {
                    let _ = tx.send(InputEvent::Error(err));
                    break;
                }
            }
        }
    });

    loop {
        match rx.recv_timeout(RESOURCE_POLL_INTERVAL) {
            Ok(InputEvent::Message(message, format)) => {
                session.stdio_message_format = format;
                if message.get("method").is_none() {
                    continue;
                }
                let method = message
                    .get("method")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ZocliError::Serialization("missing JSON-RPC method".to_string())
                    })?;
                let params = message.get("params").cloned().unwrap_or(Value::Null);

                if message.get("id").is_none() {
                    handle_notification(method, &params, &mut session);
                    continue;
                }

                let id = message
                    .get("id")
                    .cloned()
                    .ok_or_else(|| ZocliError::Serialization("missing JSON-RPC id".to_string()))?;

                let response = match execute_stdio_request(
                    method,
                    params,
                    &mut session,
                    &mut writer,
                    &rx,
                    &mut poller,
                ) {
                    Ok(result) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": result,
                    }),
                    Err(err) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": error_code(&err),
                            "message": err.to_string(),
                            "data": err.as_json(),
                        }
                    }),
                };
                write_message(&mut writer, &response, session.stdio_message_format)?;
            }
            Ok(InputEvent::Eof) => break,
            Ok(InputEvent::Error(err)) => return Err(err),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        for notification in poller.collect_notifications(&session.resource_subscriptions)? {
            write_message(&mut writer, &notification, session.stdio_message_format)?;
        }
    }

    Ok(())
}

async fn handle_http_post(
    State(state): State<HttpAppState>,
    headers: HeaderMap,
    Json(message): Json<Value>,
) -> Response {
    if let Err(err) = request_origin_allowed(&headers) {
        return http_error_response(StatusCode::FORBIDDEN, &err.to_string(), None);
    }

    let messages = match normalize_messages(message) {
        Ok(messages) => messages,
        Err(err) => return http_error_response(StatusCode::BAD_REQUEST, &err.to_string(), None),
    };

    if messages.iter().any(is_client_response_message) {
        if !messages.iter().all(is_client_response_message) {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "JSON-RPC responses cannot be mixed with requests in the same HTTP payload",
                None,
            );
        }
        return handle_http_client_responses(&state, &headers, messages).await;
    }

    if needs_http_auth(&messages) && !state.auth.authorized(&headers) {
        return http_unauthorized_response(
            required_scopes(&messages),
            state.auth_discovery.as_ref(),
        );
    }

    let method = match messages[0].get("method").and_then(Value::as_str) {
        Some(method) => method,
        None => {
            return http_error_response(StatusCode::BAD_REQUEST, "missing JSON-RPC method", None);
        }
    };

    let session_header = header_value(&headers, MCP_SESSION_HEADER);
    let session_id = if method == "initialize" {
        session_header.unwrap_or_else(new_session_id)
    } else if let Some(session_id) = session_header {
        session_id
    } else {
        return http_error_response(
            StatusCode::BAD_REQUEST,
            "missing Mcp-Session-Id header; call initialize first",
            None,
        );
    };

    if method == "tools/call"
        && requested_tool_name(messages[0].get("params").unwrap_or(&Value::Null))
            == Some("zocli.roots.list")
    {
        if messages.len() != 1 {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "zocli.roots.list must be called as a single JSON-RPC request over HTTP",
                Some(&session_id),
            );
        }
        return handle_http_roots_tool_call(
            state,
            headers,
            session_id,
            messages.into_iter().next().unwrap_or(Value::Null),
        )
        .await;
    }

    let mut responses = Vec::new();
    {
        let mut sessions = state.sessions.lock().await;
        let session = sessions
            .entry(session_id.clone())
            .or_insert_with(HttpSession::new);
        for message in messages {
            match tokio::task::block_in_place(|| {
                execute_message(message, &mut session.state, Some(&mut session.poller))
            }) {
                Ok(Some(response)) => responses.push(response),
                Ok(None) => {}
                Err(err) => responses.push(json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": {
                        "code": error_code(&err),
                        "message": err.to_string(),
                        "data": err.as_json(),
                    }
                })),
            }
        }
    }

    if responses.is_empty() {
        return http_empty_response(StatusCode::ACCEPTED, Some(&session_id));
    }

    let payload = if responses.len() == 1 {
        responses.into_iter().next().unwrap_or(Value::Null)
    } else {
        Value::Array(responses)
    };
    http_json_response(StatusCode::OK, payload, Some(&session_id))
}

async fn handle_http_client_responses(
    state: &HttpAppState,
    headers: &HeaderMap,
    messages: Vec<Value>,
) -> Response {
    let session_id = match header_value(headers, MCP_SESSION_HEADER) {
        Some(session_id) => session_id,
        None => {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "missing Mcp-Session-Id header; call initialize first",
                None,
            );
        }
    };

    let mut sessions = state.sessions.lock().await;
    let Some(session) = sessions.get_mut(&session_id) else {
        return http_error_response(
            StatusCode::BAD_REQUEST,
            "unknown Mcp-Session-Id session",
            None,
        );
    };

    for message in messages {
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "HTTP client response requires numeric JSON-RPC `id`",
                Some(&session_id),
            );
        };
        let Some(sender) = session.pending_client_requests.remove(&id) else {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "unknown pending HTTP client request id",
                Some(&session_id),
            );
        };
        let _ = sender.send(message);
    }

    http_empty_response(StatusCode::ACCEPTED, Some(&session_id))
}

async fn handle_http_roots_tool_call(
    state: HttpAppState,
    headers: HeaderMap,
    session_id: String,
    message: Value,
) -> Response {
    if !request_accepts_sse(&headers) {
        return http_error_response(
            StatusCode::NOT_ACCEPTABLE,
            "zocli.roots.list over HTTP requires Accept: text/event-stream",
            Some(&session_id),
        );
    }

    let original_id = message.get("id").cloned().unwrap_or(Value::Null);
    let mut sessions = state.sessions.lock().await;
    let Some(session) = sessions.get_mut(&session_id) else {
        return http_error_response(
            StatusCode::BAD_REQUEST,
            "unknown Mcp-Session-Id session",
            Some(&session_id),
        );
    };
    if !session.state.initialized {
        return http_error_response(
            StatusCode::BAD_REQUEST,
            "MCP session is not initialized; call initialize first",
            Some(&session_id),
        );
    }
    if !session.state.supports_roots_requests {
        let payload = jsonrpc_error_payload(
            original_id,
            &ZocliError::UnsupportedOperation(
                "zocli.roots.list requires client roots capability".to_string(),
            ),
        );
        return http_json_response(StatusCode::OK, payload, Some(&session_id));
    }

    let outbound_request_id = session.state.next_outbound_request_id;
    session.state.next_outbound_request_id += 1;
    let ui_enabled = session.state.ui_enabled;
    let (roots_response_tx, roots_response_rx) = oneshot::channel();
    session
        .pending_client_requests
        .insert(outbound_request_id, roots_response_tx);
    drop(sessions);

    let (event_tx, event_rx) = tokio_mpsc::channel::<Value>(4);
    let state_for_task = state.clone();
    let session_id_for_task = session_id.clone();
    tokio::spawn(async move {
        let roots_request = json!({
            "jsonrpc": "2.0",
            "id": outbound_request_id,
            "method": "roots/list"
        });
        if event_tx.send(roots_request).await.is_err() {
            let mut sessions = state_for_task.sessions.lock().await;
            if let Some(session) = sessions.get_mut(&session_id_for_task) {
                session.pending_client_requests.remove(&outbound_request_id);
            }
            return;
        }

        let final_payload =
            match tokio::time::timeout(Duration::from_secs(30), roots_response_rx).await {
                Ok(Ok(client_response)) => match parse_roots_list_response(&client_response)
                    .and_then(|roots| {
                        let response = roots_tool_response(roots.clone(), ui_enabled)?;
                        Ok((roots, response))
                    }) {
                    Ok((roots, response)) => {
                        let mut sessions = state_for_task.sessions.lock().await;
                        if let Some(session) = sessions.get_mut(&session_id_for_task) {
                            session.state.cached_roots = Some(roots);
                            session.state.roots_dirty = false;
                        }
                        json!({
                            "jsonrpc": "2.0",
                            "id": original_id,
                            "result": response,
                        })
                    }
                    Err(err) => jsonrpc_error_payload(original_id, &err),
                },
                Ok(Err(_)) => jsonrpc_error_payload(
                    original_id,
                    &ZocliError::Io(
                        "HTTP client closed roots/list response channel before replying"
                            .to_string(),
                    ),
                ),
                Err(_) => {
                    let mut sessions = state_for_task.sessions.lock().await;
                    if let Some(session) = sessions.get_mut(&session_id_for_task) {
                        session.pending_client_requests.remove(&outbound_request_id);
                    }
                    jsonrpc_error_payload(
                        original_id,
                        &ZocliError::Io(
                            "timed out waiting for HTTP client roots/list response".to_string(),
                        ),
                    )
                }
            };

        let _ = event_tx.send(final_payload).await;
    });

    let stream =
        ReceiverStream::new(event_rx).filter_map(|payload| match serde_json::to_string(&payload) {
            Ok(data) => Some(Ok::<Event, Infallible>(
                Event::default().event("message").data(data),
            )),
            Err(_) => None,
        });
    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(SSE_KEEPALIVE_INTERVAL)
                .text("keepalive"),
        )
        .into_response();
    insert_common_http_headers(response.headers_mut(), Some(&session_id));
    response.headers_mut().insert(
        http_header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    response
}

async fn handle_http_get(State(state): State<HttpAppState>, headers: HeaderMap) -> Response {
    if let Err(err) = request_origin_allowed(&headers) {
        return http_error_response(StatusCode::FORBIDDEN, &err.to_string(), None);
    }

    let session_id = match header_value(&headers, MCP_SESSION_HEADER) {
        Some(session_id) => session_id,
        None => {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "missing Mcp-Session-Id header; call initialize first",
                None,
            );
        }
    };
    let accept = header_value(&headers, "Accept").unwrap_or_default();
    if !accept.contains("text/event-stream") {
        return http_error_response(
            StatusCode::NOT_ACCEPTABLE,
            "SSE stream requires Accept: text/event-stream",
            None,
        );
    }

    let receiver = {
        let mut sessions = state.sessions.lock().await;
        let Some(session) = sessions.get_mut(&session_id) else {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "unknown Mcp-Session-Id session",
                None,
            );
        };
        if !session.state.initialized {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "MCP session is not initialized; call initialize first",
                None,
            );
        }
        session.notifications.subscribe()
    };

    let stream = BroadcastStream::new(receiver).filter_map(|result| match result {
        Ok(payload) => match serde_json::to_string(&payload) {
            Ok(data) => Some(Ok::<Event, Infallible>(
                Event::default().event("message").data(data),
            )),
            Err(_) => None,
        },
        Err(_) => None,
    });
    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(SSE_KEEPALIVE_INTERVAL)
                .text("keepalive"),
        )
        .into_response();
    insert_common_http_headers(response.headers_mut(), Some(&session_id));
    response.headers_mut().insert(
        http_header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    response
}

async fn handle_http_delete(State(state): State<HttpAppState>, headers: HeaderMap) -> Response {
    if let Err(err) = request_origin_allowed(&headers) {
        return http_error_response(StatusCode::FORBIDDEN, &err.to_string(), None);
    }

    let session_id = match header_value(&headers, MCP_SESSION_HEADER) {
        Some(session_id) => session_id,
        None => {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "missing Mcp-Session-Id header; call initialize first",
                None,
            );
        }
    };

    let mut sessions = state.sessions.lock().await;
    sessions.remove(&session_id);
    http_empty_response(StatusCode::NO_CONTENT, Some(&session_id))
}

async fn handle_http_options(headers: HeaderMap) -> Response {
    if let Err(err) = request_origin_allowed(&headers) {
        return http_error_response(StatusCode::FORBIDDEN, &err.to_string(), None);
    }
    http_empty_response(StatusCode::NO_CONTENT, None)
}

async fn handle_http_protected_resource_metadata(State(state): State<HttpAppState>) -> Response {
    let Some(auth_discovery) = state.auth_discovery.as_ref() else {
        return http_error_response(
            StatusCode::NOT_FOUND,
            "HTTP auth discovery is not configured",
            None,
        );
    };

    http_json_response(
        StatusCode::OK,
        json!({
            "resource": auth_discovery.canonical_server_url,
            "authorization_servers": auth_discovery.authorization_servers,
            "scopes_supported": [
                "zocli.auth.read",
                "zocli.mail.read",
                "zocli.mail.write",
                "zocli.calendar.read",
                "zocli.calendar.write",
                "zocli.drive.read",
                "zocli.drive.write"
            ],
            "bearer_methods_supported": ["header"]
        }),
        None,
    )
}

fn spawn_http_resource_poller(state: HttpAppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(RESOURCE_POLL_INTERVAL).await;

            let mut sessions = state.sessions.lock().await;
            for session in sessions.values_mut() {
                let notifications = match session
                    .poller
                    .collect_notifications(&session.state.resource_subscriptions)
                {
                    Ok(notifications) => notifications,
                    Err(_) => continue,
                };

                for notification in notifications {
                    let _ = session.notifications.send(notification);
                }
            }
        }
    });
}

fn resource_updated_notification(uri: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/resources/updated",
        "params": {
            "uri": uri
        }
    })
}

fn execute_message(
    message: Value,
    session: &mut SessionState,
    poller: Option<&mut ResourceSubscriptionPoller>,
) -> Result<Option<Value>> {
    if message.get("method").is_none() {
        return Ok(None);
    }

    let method = message
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| ZocliError::Serialization("missing JSON-RPC method".to_string()))?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    if message.get("id").is_none() {
        handle_notification(method, &params, session);
        return Ok(None);
    }

    let id = message
        .get("id")
        .cloned()
        .ok_or_else(|| ZocliError::Serialization("missing JSON-RPC id".to_string()))?;

    let response = match handle_request(method, params, session, poller) {
        Ok(result) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }),
        Err(err) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": error_code(&err),
                "message": err.to_string(),
                "data": err.as_json(),
            }
        }),
    };

    Ok(Some(response))
}

fn handle_notification(method: &str, params: &Value, session: &mut SessionState) {
    match method {
        "notifications/initialized" => {
            session.initialized = true;
        }
        "notifications/roots/list_changed" => {
            session.roots_dirty = true;
            session.cached_roots = None;
        }
        // MCP Apps lifecycle notifications (View → Host, host may proxy).
        // Track state transitions — these are fire-and-forget but observable.
        "ui/notifications/initialized" => {
            session.ui_initialized = true;
            if let Some(uri) = params.get("resourceUri").and_then(Value::as_str) {
                session.ui_active_resource = Some(uri.to_string());
            }
        }
        "ui/notifications/tool-input"
        | "ui/notifications/tool-input-partial"
        | "ui/notifications/tool-result"
        | "ui/notifications/host-context-changed"
        | "ui/notifications/size-changed"
        | "ui/notifications/tool-cancelled" => {
            // Valid lifecycle notifications — accepted silently per MCP Apps spec.
        }
        _ => {}
    }
}

fn handle_request(
    method: &str,
    params: Value,
    session: &mut SessionState,
    poller: Option<&mut ResourceSubscriptionPoller>,
) -> Result<Value> {
    if method != "initialize" && method != "ping" && !session.initialized {
        return Err(ZocliError::Validation(
            "MCP session is not initialized; call `initialize` first".to_string(),
        ));
    }

    match method {
        "initialize" => {
            session.ui_enabled = client_supports_ui(&params);
            session.roots_list_changed_supported = client_supports_roots_list_changed(&params);
            session.supports_roots_requests = client_supports_roots(&params);
            session.roots_dirty = session.supports_roots_requests;
            session.cached_roots = None;
            session.initialized = true;

            Ok(json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {
                    "completions": {},
                    "prompts": { "listChanged": false },
                    "tools": { "listChanged": false },
                    "resources": {
                        "listChanged": false,
                        "subscribe": session.supports_resource_subscriptions
                    },
                    "experimental": {
                        APP_EXTENSION_ID: {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE],
                            "resourceTemplates": true
                        }
                    }
                },
                "serverInfo": {
                    "name": "zocli",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }))
        }
        "ping" => Ok(json!({})),
        "completion/complete" => prompts::complete(params),
        "prompts/list" => Ok(json!({ "prompts": prompts::prompt_definitions() })),
        "prompts/get" => prompts::get_prompt(params),
        "tools/list" => Ok(json!({
            "tools": tool_definitions(session.ui_enabled, session.supports_roots_requests)
        })),
        "tools/call" => {
            if requested_tool_name(&params) == Some("zocli.roots.list") {
                return Err(ZocliError::UnsupportedOperation(
                    "zocli.roots.list requires stdio transport with client roots capability"
                        .to_string(),
                ));
            }
            call_tool(params, session.ui_enabled)
        }
        "resources/list" => Ok(json!({ "resources": resource_definitions(session.ui_enabled) })),
        "resources/templates/list" => {
            Ok(json!({ "resourceTemplates": resource_templates(session.ui_enabled) }))
        }
        "resources/subscribe" => subscribe_resource(params, session, poller),
        "resources/unsubscribe" => unsubscribe_resource(params, session, poller),
        "resources/read" => read_resource(params),

        // ── MCP Apps lifecycle (View ↔ Host, host may proxy to server) ──
        "ui/initialize" => {
            session.ui_initialized = true;
            Ok(json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "serverInfo": {
                    "name": "zocli",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "tools": { "listChanged": false },
                    "resources": { "listChanged": false },
                }
            }))
        }
        // All remaining ui/* methods require a prior ui/initialize call.
        "ui/update-model-context"
        | "ui/message"
        | "ui/request-display-mode"
        | "ui/open-link"
        | "ui/resource-teardown"
            if !session.ui_initialized =>
        {
            Err(ZocliError::UnsupportedOperation(
                "ui/initialize must be called before other ui/* methods".to_string(),
            ))
        }
        "ui/update-model-context" => {
            Ok(json!({ "accepted": true }))
        }
        "ui/message" => {
            Ok(json!({ "accepted": true }))
        }
        "ui/request-display-mode" => {
            let mode = params
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("inline");
            let validated = match mode {
                "inline" | "floating" | "full-window" => mode,
                _ => "inline",
            };
            Ok(json!({ "mode": validated }))
        }
        "ui/open-link" => {
            Ok(json!({ "accepted": true }))
        }
        "ui/resource-teardown" => {
            session.ui_active_resource = None;
            session.ui_initialized = false;
            Ok(json!({ "accepted": true }))
        }

        _ => Err(ZocliError::UnsupportedOperation(format!(
            "unsupported MCP method: {method}"
        ))),
    }
}

fn tool_definitions(ui_enabled: bool, roots_enabled: bool) -> Vec<Value> {
    let mut tools = Vec::new();
    if ui_enabled {
        tools.push(tool(
            "zocli.app.snapshot",
            "Return the current zocli dashboard snapshot for the hosted app.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            ui_enabled,
        ));
    }

    if roots_enabled {
        tools.push(tool(
            "zocli.roots.list",
            "Request the current MCP client filesystem roots for this session.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            ui_enabled,
        ));
    }

    tools.extend([
        tool(
            "zocli.account.list",
            "List configured zocli accounts.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.account.current",
            "Return the current zocli account.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.auth.status",
            "Return auth status for one account or the current account.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" }
                },
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            DASHBOARD_TOOL_UPDATE_CHECK,
            "Check whether a newer published zocli release is available for this target.",
            json!({
                "type": "object",
                "properties": {
                    "version": { "type": "string" }
                },
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.mail.folders",
            "List folders in the configured mailbox.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" }
                },
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.mail.list",
            "List messages from a folder.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder_id": { "type": "string" },
                    "unread_only": { "type": "boolean" },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.mail.search",
            "Search messages across the account.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.mail.read",
            "Read one message by folder and message ID.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder_id": { "type": "string" },
                    "message_id": { "type": "string" }
                },
                "required": ["folder_id", "message_id"],
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.mail.send",
            "Send one mail message. Optionally attach files by providing local file paths in the attachments array.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "from_address": { "type": "string" },
                    "to": { "type": "string" },
                    "cc": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "bcc": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "subject": { "type": "string" },
                    "text": { "type": "string" },
                    "html": { "type": "string" },
                    "attachments": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Local file paths to attach"
                    }
                },
                "required": ["to", "subject"],
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.mail.reply",
            "Reply to one message by folder and message ID.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder_id": { "type": "string" },
                    "message_id": { "type": "string" },
                    "text": { "type": "string" },
                    "html": { "type": "string" },
                    "cc": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                },
                "required": ["folder_id", "message_id"],
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.mail.attachment_export",
            "Export one attachment from a message as base64. Use get_attachment_info from mail.read to find attachment IDs.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder_id": { "type": "string" },
                    "message_id": { "type": "string" },
                    "attachment_id": { "type": "string" }
                },
                "required": ["folder_id", "message_id", "attachment_id"],
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.mail.forward",
            "Forward one message by folder and message ID.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder_id": { "type": "string" },
                    "message_id": { "type": "string" },
                    "to": { "type": "string" },
                    "text": { "type": "string" },
                    "html": { "type": "string" },
                    "cc": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "bcc": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                },
                "required": ["folder_id", "message_id", "to"],
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.calendar.calendars",
            "List calendars for the selected account.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" }
                },
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.calendar.events",
            "List upcoming calendar events.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "calendar": { "type": "string" },
                    "from": { "type": "string" },
                    "to": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.calendar.create",
            "Create one calendar event.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "calendar": { "type": "string" },
                    "summary": { "type": "string" },
                    "start": { "type": "string" },
                    "end": { "type": "string" },
                    "description": { "type": "string" },
                    "location": { "type": "string" }
                },
                "required": ["summary", "start", "end"],
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.calendar.delete",
            "Delete one calendar event by UID.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "calendar": { "type": "string" },
                    "uid": { "type": "string" }
                },
                "required": ["uid"],
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.drive.teams",
            "List Zoho WorkDrive teams (workspaces).",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" }
                },
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.drive.list",
            "List files in a Zoho WorkDrive folder.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder_id": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 },
                    "offset": { "type": "integer", "minimum": 0 }
                },
                "required": ["folder_id"],
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.drive.upload",
            "Upload one local file to a Zoho WorkDrive folder.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder_id": { "type": "string" },
                    "source": { "type": "string" },
                    "overwrite": { "type": "boolean" }
                },
                "required": ["folder_id", "source"],
                "additionalProperties": false
            }),
            ui_enabled,
        ),
        tool(
            "zocli.drive.download",
            "Download one file from Zoho WorkDrive to a local path.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "file_id": { "type": "string" },
                    "output_path": { "type": "string" },
                    "force": { "type": "boolean" }
                },
                "required": ["file_id", "output_path"],
                "additionalProperties": false
            }),
            ui_enabled,
        ),
    ]);

    tools
}

fn tool(name: &str, description: &str, input_schema: Value, ui_enabled: bool) -> Value {
    let mut object = Map::new();
    object.insert("name".to_string(), Value::String(name.to_string()));
    object.insert(
        "description".to_string(),
        Value::String(description.to_string()),
    );
    object.insert("inputSchema".to_string(), input_schema);
    if ui_enabled {
        object.insert(
            "_meta".to_string(),
            app_meta(tool_surface(name), tool_visibility(name)),
        );
    }
    Value::Object(object)
}

fn call_tool(params: Value, ui_enabled: bool) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ZocliError::Validation("tools/call requires `name`".to_string()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Object(Map::new()));

    let mut structured = match name {
        "zocli.app.snapshot" => app_snapshot()?,
        "zocli.account.list" => account_list()?,
        "zocli.account.current" => account_current()?,
        "zocli.auth.status" => auth_status(arguments.get("account").and_then(Value::as_str))?,
        DASHBOARD_TOOL_UPDATE_CHECK => {
            update_check(arguments.get("version").and_then(Value::as_str))?
        }
        "zocli.mail.folders" => mail_folders(arguments.get("account").and_then(Value::as_str))?,
        "zocli.mail.list" => mail_list(
            arguments.get("account").and_then(Value::as_str),
            optional_string(&arguments, "folder_id"),
            optional_bool(&arguments, "unread_only").unwrap_or(false),
            optional_usize(&arguments, "limit").unwrap_or(20),
        )?,
        "zocli.mail.search" => mail_search(
            arguments.get("account").and_then(Value::as_str),
            required_string(&arguments, "query")?,
            optional_usize(&arguments, "limit").unwrap_or(20),
        )?,
        "zocli.mail.read" => mail_read(
            arguments.get("account").and_then(Value::as_str),
            required_string(&arguments, "folder_id")?,
            required_string(&arguments, "message_id")?,
        )?,
        "zocli.mail.attachment_export" => mail_attachment_export(
            arguments.get("account").and_then(Value::as_str),
            required_string(&arguments, "folder_id")?,
            required_string(&arguments, "message_id")?,
            required_string(&arguments, "attachment_id")?,
        )?,
        "zocli.mail.send" => mail_send(
            arguments.get("account").and_then(Value::as_str),
            MailSendToolRequest {
                from_address: optional_string(&arguments, "from_address")
                    .unwrap_or("")
                    .to_string(),
                to: required_string(&arguments, "to")?.to_string(),
                cc: string_list(&arguments, "cc")?,
                bcc: string_list(&arguments, "bcc")?,
                subject: required_string(&arguments, "subject")?.to_string(),
                text: optional_string_owned(&arguments, "text"),
                html: optional_string_owned(&arguments, "html"),
                attachments: string_list(&arguments, "attachments")?,
            },
        )?,
        "zocli.mail.reply" => mail_reply(
            arguments.get("account").and_then(Value::as_str),
            MailReplyToolRequest {
                folder_id: required_string(&arguments, "folder_id")?.to_string(),
                message_id: required_string(&arguments, "message_id")?.to_string(),
                content: optional_string_owned(&arguments, "text")
                    .or_else(|| optional_string_owned(&arguments, "html")),
                cc: string_list(&arguments, "cc")?,
            },
        )?,
        "zocli.mail.forward" => mail_forward(
            arguments.get("account").and_then(Value::as_str),
            MailForwardToolRequest {
                folder_id: required_string(&arguments, "folder_id")?.to_string(),
                message_id: required_string(&arguments, "message_id")?.to_string(),
                to: required_string(&arguments, "to")?.to_string(),
                cc: string_list(&arguments, "cc")?,
                bcc: string_list(&arguments, "bcc")?,
                content: optional_string_owned(&arguments, "text")
                    .or_else(|| optional_string_owned(&arguments, "html")),
            },
        )?,
        "zocli.calendar.calendars" => {
            calendar_calendars(arguments.get("account").and_then(Value::as_str))?
        }
        "zocli.calendar.events" => calendar_events(
            arguments.get("account").and_then(Value::as_str),
            optional_string(&arguments, "calendar").unwrap_or("default"),
            optional_string(&arguments, "from"),
            optional_string(&arguments, "to"),
            optional_usize(&arguments, "limit").unwrap_or(20),
        )?,
        "zocli.calendar.create" => calendar_create(
            arguments.get("account").and_then(Value::as_str),
            optional_string(&arguments, "calendar").unwrap_or("default"),
            required_string(&arguments, "summary")?,
            required_string(&arguments, "start")?,
            required_string(&arguments, "end")?,
            optional_string_owned(&arguments, "description"),
            optional_string_owned(&arguments, "location"),
        )?,
        "zocli.calendar.delete" => calendar_delete(
            arguments.get("account").and_then(Value::as_str),
            optional_string(&arguments, "calendar").unwrap_or("default"),
            required_string(&arguments, "uid")?,
        )?,
        "zocli.drive.teams" => drive_teams(arguments.get("account").and_then(Value::as_str))?,
        "zocli.drive.list" => drive_list(
            arguments.get("account").and_then(Value::as_str),
            required_string(&arguments, "folder_id")?,
            optional_usize(&arguments, "limit").unwrap_or(100),
            optional_u64(&arguments, "offset").unwrap_or(0),
        )?,
        "zocli.drive.upload" => drive_upload(
            arguments.get("account").and_then(Value::as_str),
            required_string(&arguments, "folder_id")?,
            required_string(&arguments, "source")?,
            optional_bool(&arguments, "overwrite").unwrap_or(false),
        )?,
        "zocli.drive.download" => drive_download(
            arguments.get("account").and_then(Value::as_str),
            required_string(&arguments, "file_id")?,
            required_string(&arguments, "output_path")?,
            optional_bool(&arguments, "force").unwrap_or(false),
        )?,
        _ => {
            return Err(ZocliError::UnsupportedOperation(format!(
                "unsupported MCP tool: {name}"
            )));
        }
    };

    // Stamp every structuredContent payload with a schema version
    // so hosts can detect breaking changes.
    if let Value::Object(ref mut map) = structured {
        map.insert("schemaVersion".to_string(), json!("1.0"));
    }

    let text = serde_json::to_string_pretty(&structured)
        .map_err(|err| ZocliError::Serialization(err.to_string()))?;

    let mut response = Map::new();
    response.insert("structuredContent".to_string(), structured);
    response.insert(
        "content".to_string(),
        json!([
            {
                "type": "text",
                "text": text
            }
        ]),
    );
    if ui_enabled {
        response.insert(
            "_meta".to_string(),
            app_meta(tool_surface(name), tool_visibility(name)),
        );
    }

    Ok(Value::Object(response))
}

fn resource_definitions(ui_enabled: bool) -> Vec<Value> {
    let mut resources = vec![
        json!({
            "uri": "resource://zocli/getting-started",
            "name": "zocli MCP Getting Started",
            "description": "Text guide for the stable zocli MCP surface",
            "mimeType": "text/markdown"
        }),
        json!({
            "uri": "resource://zocli/skills",
            "name": "zocli Embedded Skills",
            "description": "Catalog of embedded zocli SKILL.md workflows mirrored into MCP resources",
            "mimeType": "application/json"
        }),
    ];
    if ui_enabled {
        resources.push(json!({
            "uri": APP_RESOURCE_URI,
            "name": "zocli MCP Dashboard",
            "description": "Minimal MCP Apps-compatible HTML surface for zocli",
            "mimeType": APP_RESOURCE_MIME_TYPE,
            "_meta": app_resource_meta()
        }));
        resources.push(json!({
            "uri": APP_MAIL_RESOURCE_URI,
            "name": "zocli Mail",
            "description": "Mail workflow surface for Zoho Mail",
            "mimeType": APP_RESOURCE_MIME_TYPE,
            "_meta": app_resource_meta()
        }));
        resources.push(json!({
            "uri": APP_CALENDAR_RESOURCE_URI,
            "name": "zocli Calendar",
            "description": "Calendar workflow surface for Zoho Calendar",
            "mimeType": APP_RESOURCE_MIME_TYPE,
            "_meta": app_resource_meta()
        }));
        resources.push(json!({
            "uri": APP_DRIVE_RESOURCE_URI,
            "name": "zocli Drive",
            "description": "Drive workflow surface for Zoho WorkDrive",
            "mimeType": APP_RESOURCE_MIME_TYPE,
            "_meta": app_resource_meta()
        }));
        resources.push(json!({
            "uri": APP_AUTH_RESOURCE_URI,
            "name": "zocli Auth",
            "description": "Auth status and remediation surface",
            "mimeType": APP_RESOURCE_MIME_TYPE,
            "_meta": app_resource_meta()
        }));
        resources.push(json!({
            "uri": APP_ACCOUNT_RESOURCE_URI,
            "name": "zocli Account",
            "description": "Account management surface",
            "mimeType": APP_RESOURCE_MIME_TYPE,
            "_meta": app_resource_meta()
        }));
    }
    resources
}

fn resource_templates(_ui_enabled: bool) -> Vec<Value> {
    let mut templates = vec![
        json!({
            "uriTemplate": "resource://zocli/account/{account}",
            "name": "zocli Account Resource",
            "description": "Read one configured zocli account summary as JSON",
            "mimeType": "application/json"
        }),
        json!({
            "uriTemplate": "resource://zocli/auth/{account}",
            "name": "zocli Auth Resource",
            "description": "Read auth posture for one configured zocli account as JSON",
            "mimeType": "application/json"
        }),
        json!({
            "uriTemplate": "resource://zocli/skill/{skill}",
            "name": "zocli Embedded Skill Resource",
            "description": "Read one embedded zocli SKILL.md workflow as Markdown",
            "mimeType": "text/markdown"
        }),
    ];

    if _ui_enabled {
        templates.push(json!({
            "uriTemplate": APP_RESOURCE_URI_TEMPLATE,
            "name": "zocli MCP Dashboard",
            "description": "Load the hosted zocli dashboard with an optional restored account alias",
            "mimeType": APP_RESOURCE_MIME_TYPE,
            "_meta": app_resource_meta()
        }));
        for (uri, name, description) in [
            ("ui://zocli/mail{?account}", "zocli Mail", "Load the mail workflow surface"),
            ("ui://zocli/calendar{?account}", "zocli Calendar", "Load the calendar workflow surface"),
            ("ui://zocli/drive{?account}", "zocli Drive", "Load the drive workflow surface"),
            ("ui://zocli/auth{?account}", "zocli Auth", "Load the auth status surface"),
            ("ui://zocli/account{?account}", "zocli Account", "Load the account management surface"),
        ] {
            templates.push(json!({
                "uriTemplate": uri,
                "name": name,
                "description": description,
                "mimeType": APP_RESOURCE_MIME_TYPE,
                "_meta": app_resource_meta()
            }));
        }
    }

    templates
}

fn read_resource(params: Value) -> Result<Value> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| ZocliError::Validation("resources/read requires `uri`".to_string()))?;

    let contents = resource_contents(uri)?;

    Ok(json!({ "contents": contents }))
}

fn subscribe_resource(
    params: Value,
    session: &mut SessionState,
    poller: Option<&mut ResourceSubscriptionPoller>,
) -> Result<Value> {
    if !session.supports_resource_subscriptions {
        return Err(ZocliError::UnsupportedOperation(
            "resource subscriptions are not available on this transport".to_string(),
        ));
    }

    let uri = resource_uri_param(&params, "resources/subscribe")?;
    validate_subscribable_resource_uri(uri)?;
    session.resource_subscriptions.insert(uri.to_string());
    if let Some(poller) = poller {
        poller.track(uri)?;
    }
    Ok(json!({}))
}

fn unsubscribe_resource(
    params: Value,
    session: &mut SessionState,
    poller: Option<&mut ResourceSubscriptionPoller>,
) -> Result<Value> {
    if !session.supports_resource_subscriptions {
        return Err(ZocliError::UnsupportedOperation(
            "resource subscriptions are not available on this transport".to_string(),
        ));
    }

    let uri = resource_uri_param(&params, "resources/unsubscribe")?;
    session.resource_subscriptions.remove(uri);
    if let Some(poller) = poller {
        poller.untrack(uri);
    }
    Ok(json!({}))
}

fn account_list() -> Result<Value> {
    let store = AccountStore::load()?;
    let items: Vec<_> = store
        .summaries()
        .into_iter()
        .map(|(name, account)| {
            json!({
                "name": name,
                "email": account.email,
                "current": store.is_current_account(&name),
                "datacenter": account.datacenter,
                "account_id": account.account_id,
            })
        })
        .collect();
    Ok(json!({ "items": items }))
}

fn account_current() -> Result<Value> {
    let store = AccountStore::load()?;
    let name = store.current_account_name()?;
    let account = store.get_account(&name)?;
    Ok(json!({
        "account": name,
        "email": account.email,
    }))
}

fn auth_status(account: Option<&str>) -> Result<Value> {
    let account_store = AccountStore::load()?;
    let name = account_store.resolved_account_name(account)?;
    let account = account_store.get_account(&name)?;
    let credential_store = CredentialStore::load()?;

    let state = auth_state(
        &credential_store,
        &name,
        account.credential_ref.as_deref(),
        "oauth",
    );
    let guidance = auth_guidance(state.credential_state, &name);
    let auth_discovery = app_auth_discovery();

    Ok(json!({
        "account": name,
        "email": account.email,
        "datacenter": account.datacenter,
        "auth": state,
        "guidance": guidance,
        "authDiscovery": auth_discovery,
    }))
}

fn auth_guidance(credential_state: &str, _account_name: &str) -> &'static str {
    match credential_state {
        "not_configured" => "No credential reference configured. Run `zocli add` to set up an account with OAuth.",
        "store_missing" => "Account is configured but not logged in. Run `zocli login` to authenticate with Zoho.",
        "store_expired" => "OAuth token has expired. Run `zocli login` to re-authenticate with Zoho.",
        "store_present" => "Authenticated and ready. All Zoho API tools are available.",
        "env_present" => "Using environment variable for authentication. Ready.",
        "env_missing" => "Environment variable for authentication is not set. Set the required variable or switch to OAuth.",
        "store_mismatch" => "Credential reference points to a different service. Check your account configuration.",
        _ => "Unknown auth state. Run `zocli auth status` for details.",
    }
}

fn update_check(version: Option<&str>) -> Result<Value> {
    let report = check_for_update(version)?;
    Ok(json!({
        "operation": report.operation,
        "status": report.status,
        "currentVersion": report.current_version,
        "targetVersion": report.target_version,
        "requestedVersion": report.requested_version,
        "asset": report.asset,
        "target": report.target,
        "baseUrl": report.base_url,
    }))
}

fn account_resource(uri: &str) -> Result<Value> {
    let account_name = templated_account_name(uri, "account")?;
    let account_store = AccountStore::load()?;
    let account = account_store.get_account(&account_name)?;

    Ok(json!({
        "account": account_name,
        "email": account.email,
        "current": account_store.is_current_account(&account_name),
        "datacenter": account.datacenter,
        "account_id": account.account_id,
        "org_id": account.org_id,
        "credential_ref": account.credential_ref,
    }))
}

fn auth_resource(uri: &str) -> Result<Value> {
    let account_name = templated_account_name(uri, "auth")?;
    auth_status(Some(&account_name))
}

fn skills_catalog_resource() -> Value {
    let items = skills::skill_names()
        .into_iter()
        .map(|name| {
            json!({
                "name": name,
                "prompt": skills::skill_prompt_name(name),
                "uri": format!("resource://zocli/skill/{name}"),
                "description": skills::skill_description(name).unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "count": items.len(),
        "items": items,
    })
}

fn skill_resource_contents(uri: &str) -> Result<Vec<Value>> {
    let skill_name = templated_account_name(uri, "skill")?;
    let content = skills::skill_content(&skill_name).ok_or_else(|| {
        ZocliError::UnsupportedOperation(format!("unknown embedded skill resource: {uri}"))
    })?;
    Ok(vec![json!({
        "uri": uri,
        "mimeType": "text/markdown",
        "text": content,
    })])
}

fn app_snapshot() -> Result<Value> {
    let account_store = AccountStore::load()?;
    let credential_store = CredentialStore::load()?;
    let current_account = account_store.current_account_name().ok();
    let auth_discovery = app_auth_discovery();

    let accounts = account_store
        .summaries()
        .into_iter()
        .map(|(name, account)| {
            json!({
                "name": name,
                "email": account.email,
                "current": current_account.as_deref() == Some(name.as_str()),
                "datacenter": account.datacenter,
                "account_id": account.account_id,
                "auth": auth_state(
                    &credential_store,
                    &name,
                    account.credential_ref.as_deref(),
                    "oauth",
                ),
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "generatedAt": Utc::now().to_rfc3339(),
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "appResourceUri": APP_RESOURCE_URI,
        "accountCount": accounts.len(),
        "currentAccount": current_account,
        "authDiscovery": auth_discovery,
        "accounts": accounts,
    }))
}

fn mail_folders(account: Option<&str>) -> Result<Value> {
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let base_url = config.mail_api_url();
    let folders = list_mail_folders(&base_url, &config.account_id, &access_token)?;
    Ok(json!({
        "account": resolved_account,
        "items": folders,
    }))
}

fn mail_list(
    account: Option<&str>,
    folder_id: Option<&str>,
    unread_only: bool,
    limit: usize,
) -> Result<Value> {
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let base_url = config.mail_api_url();
    let messages = list_mail_messages(
        &base_url,
        &config.account_id,
        &access_token,
        folder_id,
        unread_only,
        limit,
    )?;
    Ok(json!({
        "account": resolved_account,
        "folder_id": folder_id,
        "items": messages,
    }))
}

fn mail_search(account: Option<&str>, query: &str, limit: usize) -> Result<Value> {
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let base_url = config.mail_api_url();
    let messages =
        search_mail_messages(&base_url, &config.account_id, &access_token, query, limit)?;
    Ok(json!({
        "account": resolved_account,
        "query": query,
        "items": messages,
    }))
}

fn mail_read(account: Option<&str>, folder_id: &str, message_id: &str) -> Result<Value> {
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let base_url = config.mail_api_url();
    let message = read_mail_message(
        &base_url,
        &config.account_id,
        &access_token,
        folder_id,
        message_id,
    )?;
    Ok(json!({
        "account": resolved_account,
        "item": message,
    }))
}

fn mail_attachment_export(
    account: Option<&str>,
    folder_id: &str,
    message_id: &str,
    attachment_id: &str,
) -> Result<Value> {
    use base64::Engine;
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let base_url = config.mail_api_url();

    let attachments = get_attachment_info(
        &base_url,
        &config.account_id,
        &access_token,
        folder_id,
        message_id,
    )?;

    let info = attachments
        .iter()
        .find(|a| a.attachment_id == attachment_id)
        .ok_or_else(|| {
            ZocliError::Validation(format!(
                "attachment {attachment_id} not found on message {message_id}"
            ))
        })?;

    let bytes = download_attachment(
        &base_url,
        &config.account_id,
        &access_token,
        folder_id,
        message_id,
        attachment_id,
    )?;

    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);

    Ok(json!({
        "account": resolved_account,
        "folder_id": folder_id,
        "message_id": message_id,
        "attachment_id": attachment_id,
        "attachment_name": info.attachment_name,
        "attachment_size": info.attachment_size,
        "content_base64": encoded,
    }))
}

fn mail_send(account: Option<&str>, request: MailSendToolRequest) -> Result<Value> {
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let base_url = config.mail_api_url();

    let from_address = if request.from_address.is_empty() {
        config.email.clone()
    } else {
        request.from_address
    };

    let content = if let Some(html) = &request.html {
        html.clone()
    } else {
        request.text.unwrap_or_default()
    };

    let mail_format = if request.html.is_some() {
        "html".to_string()
    } else {
        "plaintext".to_string()
    };

    // Upload attachments from local file paths
    let uploaded_attachments = request
        .attachments
        .iter()
        .map(|path| {
            let file_path = std::path::Path::new(path);
            let file_name = file_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("attachment")
                .to_string();
            let file_bytes = std::fs::read(file_path).map_err(|e| {
                ZocliError::Validation(format!("cannot read attachment {path}: {e}"))
            })?;
            upload_attachment(&base_url, &config.account_id, &access_token, &file_name, file_bytes)
        })
        .collect::<Result<Vec<_>>>()?;

    let sent = send_mail_message(
        &base_url,
        &config.account_id,
        &access_token,
        crate::mail::MailSendRequest {
            from_address,
            to_address: request.to,
            cc_address: request.cc.join(","),
            bcc_address: request.bcc.join(","),
            subject: request.subject,
            content,
            mail_format,
            attachments: uploaded_attachments,
        },
    )?;
    Ok(json!({
        "account": resolved_account,
        "sent": sent,
    }))
}

fn mail_reply(account: Option<&str>, request: MailReplyToolRequest) -> Result<Value> {
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let base_url = config.mail_api_url();
    let reply = reply_to_mail_message(
        &base_url,
        &config.account_id,
        &access_token,
        crate::mail::MailReplyRequest {
            message_id: request.message_id,
            content: request.content.unwrap_or_default(),
            cc_address: request.cc.join(","),
            mail_format: "plaintext".to_string(),
            from_address: None,
        },
    )?;
    Ok(json!({
        "account": resolved_account,
        "folder_id": request.folder_id,
        "reply": reply,
    }))
}

fn mail_forward(account: Option<&str>, request: MailForwardToolRequest) -> Result<Value> {
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let base_url = config.mail_api_url();
    let forward = forward_mail_message(
        &base_url,
        &config.account_id,
        &access_token,
        crate::mail::MailForwardRequest {
            message_id: request.message_id,
            folder_id: request.folder_id.clone(),
            from_address: config.email.clone(),
            to_address: request.to,
            content: request.content.unwrap_or_default(),
            cc_address: request.cc.join(","),
            bcc_address: request.bcc.join(","),
        },
    )?;
    Ok(json!({
        "account": resolved_account,
        "folder_id": request.folder_id,
        "forward": forward,
    }))
}

fn calendar_calendars(account: Option<&str>) -> Result<Value> {
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let base_url = config.calendar_api_url();
    let calendars = list_calendars(&base_url, &config.account_id, &access_token)?;
    Ok(json!({
        "account": resolved_account,
        "items": calendars,
    }))
}

fn calendar_events(
    account: Option<&str>,
    calendar: &str,
    from: Option<&str>,
    to: Option<&str>,
    limit: usize,
) -> Result<Value> {
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let base_url = config.calendar_api_url();
    let window = parse_event_window(from, to, limit)?;
    let request = CalendarEventsRequest {
        calendar: calendar.to_string(),
        from: parse_rfc3339(&window.from)?,
        to: parse_rfc3339(&window.to)?,
        limit,
    };
    let (calendar, window, items) =
        list_calendar_events(&base_url, &config.account_id, &access_token, request)?;
    Ok(json!({
        "account": resolved_account,
        "calendar": calendar,
        "window": window,
        "items": items,
    }))
}

fn calendar_create(
    account: Option<&str>,
    calendar: &str,
    summary: &str,
    start: &str,
    end: &str,
    description: Option<String>,
    location: Option<String>,
) -> Result<Value> {
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let base_url = config.calendar_api_url();
    let (calendar, event) = create_calendar_event(
        &base_url,
        &config.account_id,
        &access_token,
        CalendarCreateRequest {
            calendar: calendar.to_string(),
            summary: summary.to_string(),
            start: start.to_string(),
            end: end.to_string(),
            description,
            location,
        },
    )?;
    Ok(json!({
        "account": resolved_account,
        "calendar": calendar,
        "event": event,
    }))
}

fn calendar_delete(account: Option<&str>, calendar: &str, uid: &str) -> Result<Value> {
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let base_url = config.calendar_api_url();
    let (calendar, deleted_event) =
        delete_calendar_event(&base_url, &config.account_id, &access_token, calendar, uid)?;
    Ok(json!({
        "account": resolved_account,
        "calendar": calendar,
        "deleted_event": deleted_event,
    }))
}

fn drive_teams(account: Option<&str>) -> Result<Value> {
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let base_url = config.drive_api_url();
    let drive_user_id = config.zuid.as_deref().unwrap_or(&config.account_id);
    let teams = list_teams(&base_url, drive_user_id, &access_token)?;
    Ok(json!({
        "account": resolved_account,
        "items": teams,
    }))
}

fn drive_list(account: Option<&str>, folder_id: &str, limit: usize, offset: u64) -> Result<Value> {
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let base_url = config.drive_api_url();
    let files = list_files(&base_url, &access_token, folder_id, limit, offset)?;
    Ok(json!({
        "account": resolved_account,
        "folder_id": folder_id,
        "items": files,
    }))
}

fn drive_upload(
    account: Option<&str>,
    folder_id: &str,
    source: &str,
    overwrite: bool,
) -> Result<Value> {
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let upload_url = config.drive_upload_url();
    let uploaded = upload_file(
        &upload_url,
        &access_token,
        folder_id,
        &PathBuf::from(source),
        overwrite,
    )?;
    Ok(json!({
        "account": resolved_account,
        "folder_id": folder_id,
        "source": source,
        "uploaded": uploaded,
    }))
}

fn drive_download(
    account: Option<&str>,
    file_id: &str,
    output_path: &str,
    force: bool,
) -> Result<Value> {
    let (resolved_account, config, access_token) = resolve_zoho_context(account)?;
    let download_url = config.drive_download_url(file_id);
    let downloaded = download_file(
        &download_url,
        &access_token,
        &PathBuf::from(output_path),
        force,
    )?;
    Ok(json!({
        "account": resolved_account,
        "file_id": file_id,
        "downloaded": downloaded,
    }))
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ZocliError::Validation(format!("`{key}` is required")))
}

fn optional_string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn optional_string_owned(value: &Value, key: &str) -> Option<String> {
    optional_string(value, key).map(ToString::to_string)
}

fn string_list(value: &Value, key: &str) -> Result<Vec<String>> {
    let Some(values) = value.get(key) else {
        return Ok(Vec::new());
    };

    let array = values
        .as_array()
        .ok_or_else(|| ZocliError::Validation(format!("`{key}` must be an array of strings")))?;

    array
        .iter()
        .map(|item| {
            item.as_str().map(ToString::to_string).ok_or_else(|| {
                ZocliError::Validation(format!("`{key}` must be an array of strings"))
            })
        })
        .collect()
}

fn optional_usize(value: &Value, key: &str) -> Option<usize> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .map(|number| number as usize)
}

fn optional_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

fn optional_bool(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

fn parse_rfc3339(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|err| {
            ZocliError::Validation(format!("invalid RFC3339 timestamp `{value}`: {err}"))
        })
}

fn app_html(uri: &str) -> Result<String> {
    let bootstrap_state = parse_app_resource_state(uri)?;
    let bootstrap = serde_json::to_string(&json!({
        "resourceUri": uri,
        "defaultAccount": bootstrap_state.default_account,
        "preferredSection": bootstrap_state.preferred_section,
        "preferredResource": bootstrap_state.preferred_resource,
        "preferredTool": bootstrap_state.preferred_tool,
        "preferredSkill": bootstrap_state.preferred_skill,
        "preferredPrompt": bootstrap_state.preferred_prompt,
    }))
    .map_err(|err| ZocliError::Serialization(err.to_string()))?;

    Ok(include_str!("dashboard_app.html")
        .replace("__ZOCLI_VERSION__", env!("CARGO_PKG_VERSION"))
        .replace("__ZOCLI_BOOTSTRAP__", &bootstrap))
}

fn resource_uri_param<'a>(params: &'a Value, method: &str) -> Result<&'a str> {
    params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| ZocliError::Validation(format!("{method} requires `uri`")))
}

fn validate_subscribable_resource_uri(uri: &str) -> Result<()> {
    if !is_subscribable_resource_uri(uri) {
        return Err(ZocliError::UnsupportedOperation(format!(
            "resource does not support subscriptions: {uri}"
        )));
    }

    let _ = resource_digest(uri)?;
    Ok(())
}

fn resource_contents(uri: &str) -> Result<Vec<Value>> {
    match uri {
        "resource://zocli/getting-started" => Ok(vec![json!({
            "uri": uri,
            "mimeType": "text/markdown",
            "text": "# zocli MCP\n\nStable read-only tools are available for accounts, auth status, mail, calendar, and drive.\n\nApps-ready clients can also load `ui://zocli/dashboard`."
        })]),
        "resource://zocli/skills" => json_resource_contents(uri, skills_catalog_resource()),
        app_uri if is_app_resource_uri(app_uri) => Ok(vec![json!({
            "uri": app_uri,
            "mimeType": APP_RESOURCE_MIME_TYPE,
            "text": app_html(app_uri)?,
            "_meta": app_resource_meta()
        })]),
        account_uri if account_uri.starts_with("resource://zocli/account/") => {
            json_resource_contents(account_uri, account_resource(account_uri)?)
        }
        auth_uri if auth_uri.starts_with("resource://zocli/auth/") => {
            json_resource_contents(auth_uri, auth_resource(auth_uri)?)
        }
        skill_uri if skill_uri.starts_with("resource://zocli/skill/") => {
            skill_resource_contents(skill_uri)
        }
        _ => Err(ZocliError::UnsupportedOperation(format!(
            "unknown MCP resource: {uri}"
        ))),
    }
}

fn is_app_resource_uri(uri: &str) -> bool {
    parse_app_resource_state(uri).is_ok()
}

fn parse_app_resource_state(uri: &str) -> Result<DashboardResourceState> {
    let parsed = Url::parse(uri)
        .map_err(|err| ZocliError::Validation(format!("invalid resource URI `{uri}`: {err}")))?;

    if parsed.scheme() != "ui" || parsed.host_str() != Some("zocli") {
        return Err(ZocliError::UnsupportedOperation(format!(
            "unknown MCP resource: {uri}"
        )));
    }

    let surface = parsed
        .path()
        .strip_prefix('/')
        .unwrap_or(parsed.path());

    if !APP_SURFACES.contains(&surface) {
        return Err(ZocliError::UnsupportedOperation(format!(
            "unknown MCP resource: {uri}"
        )));
    }

    if surface == "dashboard" {
        return parse_dashboard_resource_state(uri);
    }

    // Non-dashboard surfaces only accept ?account
    let mut account = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            APP_ACCOUNT_QUERY_PARAM => {
                if value.trim().is_empty() {
                    return Err(ZocliError::Validation(format!(
                        "{surface} resource query `{APP_ACCOUNT_QUERY_PARAM}` cannot be empty"
                    )));
                }
                if account.replace(value.into_owned()).is_some() {
                    return Err(ZocliError::Validation(format!(
                        "{surface} resource query `{APP_ACCOUNT_QUERY_PARAM}` cannot appear more than once"
                    )));
                }
            }
            _ => {
                return Err(ZocliError::UnsupportedOperation(format!(
                    "unknown MCP resource: {uri}"
                )));
            }
        }
    }

    // Map each surface to a focused section+prompt so the app shell
    // renders the relevant view, not a generic empty dashboard.
    let (section, prompt) = match surface {
        "mail" => (Some(DASHBOARD_SECTION_PROMPTS), Some("mail")),
        "calendar" => (Some(DASHBOARD_SECTION_PROMPTS), Some("calendar")),
        "drive" => (Some(DASHBOARD_SECTION_PROMPTS), Some("drive")),
        "auth" => (Some(DASHBOARD_SECTION_AUTH), None),
        "account" => (Some(DASHBOARD_SECTION_AUTH), None),
        _ => (None, None),
    };

    Ok(DashboardResourceState {
        default_account: account,
        preferred_section: section.map(String::from),
        preferred_resource: None,
        preferred_tool: None,
        preferred_skill: None,
        preferred_prompt: prompt.map(String::from),
    })
}

fn is_subscribable_resource_uri(uri: &str) -> bool {
    uri.starts_with("resource://zocli/account/") || uri.starts_with("resource://zocli/auth/")
}

fn templated_account_name(uri: &str, namespace: &str) -> Result<String> {
    let parsed = Url::parse(uri)
        .map_err(|err| ZocliError::Validation(format!("invalid resource URI `{uri}`: {err}")))?;
    let segments = parsed
        .path_segments()
        .ok_or_else(|| ZocliError::Validation(format!("invalid resource URI `{uri}`")))?;
    let parts = segments.collect::<Vec<_>>();
    if parsed.scheme() != "resource"
        || parsed.host_str() != Some("zocli")
        || parts.len() != 2
        || parts[0] != namespace
        || parts[1].trim().is_empty()
    {
        return Err(ZocliError::UnsupportedOperation(format!(
            "unknown MCP resource: {uri}"
        )));
    }

    Ok(parts[1].to_string())
}

fn parse_dashboard_resource_state(uri: &str) -> Result<DashboardResourceState> {
    let parsed = Url::parse(uri)
        .map_err(|err| ZocliError::Validation(format!("invalid resource URI `{uri}`: {err}")))?;

    if parsed.scheme() != "ui"
        || parsed.host_str() != Some("zocli")
        || parsed.path() != "/dashboard"
    {
        return Err(ZocliError::UnsupportedOperation(format!(
            "unknown MCP resource: {uri}"
        )));
    }

    let mut account = None;
    let mut section = None;
    let mut resource = None;
    let mut tool = None;
    let mut skill = None;
    let mut prompt = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            APP_ACCOUNT_QUERY_PARAM => {
                if value.trim().is_empty() {
                    return Err(ZocliError::Validation(format!(
                        "dashboard resource query `{APP_ACCOUNT_QUERY_PARAM}` cannot be empty"
                    )));
                }
                if account.replace(value.into_owned()).is_some() {
                    return Err(ZocliError::Validation(format!(
                        "dashboard resource query `{APP_ACCOUNT_QUERY_PARAM}` cannot appear more than once"
                    )));
                }
            }
            APP_SECTION_QUERY_PARAM => {
                let value = value.into_owned();
                if !matches!(
                    value.as_str(),
                    DASHBOARD_SECTION_TOOLS
                        | DASHBOARD_SECTION_PROMPTS
                        | DASHBOARD_SECTION_RESOURCES
                        | DASHBOARD_SECTION_AUTH
                ) {
                    return Err(ZocliError::Validation(format!(
                        "dashboard resource query `{APP_SECTION_QUERY_PARAM}` must be one of: {DASHBOARD_SECTION_TOOLS}, {DASHBOARD_SECTION_PROMPTS}, {DASHBOARD_SECTION_RESOURCES}, {DASHBOARD_SECTION_AUTH}"
                    )));
                }
                if section.replace(value).is_some() {
                    return Err(ZocliError::Validation(format!(
                        "dashboard resource query `{APP_SECTION_QUERY_PARAM}` cannot appear more than once"
                    )));
                }
            }
            APP_RESOURCE_QUERY_PARAM => {
                let value = value.into_owned();
                if !matches!(
                    value.as_str(),
                    DASHBOARD_RESOURCE_ACCOUNT
                        | DASHBOARD_RESOURCE_AUTH
                        | DASHBOARD_RESOURCE_SKILLS
                        | DASHBOARD_RESOURCE_SKILL
                ) {
                    return Err(ZocliError::Validation(format!(
                        "dashboard resource query `{APP_RESOURCE_QUERY_PARAM}` must be one of: {DASHBOARD_RESOURCE_ACCOUNT}, {DASHBOARD_RESOURCE_AUTH}, {DASHBOARD_RESOURCE_SKILLS}, {DASHBOARD_RESOURCE_SKILL}"
                    )));
                }
                if resource.replace(value).is_some() {
                    return Err(ZocliError::Validation(format!(
                        "dashboard resource query `{APP_RESOURCE_QUERY_PARAM}` cannot appear more than once"
                    )));
                }
            }
            APP_TOOL_QUERY_PARAM => {
                let value = value.into_owned();
                if !matches!(
                    value.as_str(),
                    DASHBOARD_TOOL_APP_SNAPSHOT
                        | DASHBOARD_TOOL_ACCOUNT_LIST
                        | DASHBOARD_TOOL_ACCOUNT_CURRENT
                        | DASHBOARD_TOOL_AUTH_STATUS
                ) {
                    return Err(ZocliError::Validation(format!(
                        "dashboard resource query `{APP_TOOL_QUERY_PARAM}` must be one of: {DASHBOARD_TOOL_APP_SNAPSHOT}, {DASHBOARD_TOOL_ACCOUNT_LIST}, {DASHBOARD_TOOL_ACCOUNT_CURRENT}, {DASHBOARD_TOOL_AUTH_STATUS}"
                    )));
                }
                if tool.replace(value).is_some() {
                    return Err(ZocliError::Validation(format!(
                        "dashboard resource query `{APP_TOOL_QUERY_PARAM}` cannot appear more than once"
                    )));
                }
            }
            APP_SKILL_QUERY_PARAM => {
                if value.trim().is_empty() {
                    return Err(ZocliError::Validation(format!(
                        "dashboard resource query `{APP_SKILL_QUERY_PARAM}` cannot be empty"
                    )));
                }
                if skill.replace(value.into_owned()).is_some() {
                    return Err(ZocliError::Validation(format!(
                        "dashboard resource query `{APP_SKILL_QUERY_PARAM}` cannot appear more than once"
                    )));
                }
            }
            APP_PROMPT_QUERY_PARAM => {
                let value = value.into_owned();
                if !prompts::prompt_names().contains(&value.as_str()) {
                    return Err(ZocliError::Validation(format!(
                        "dashboard resource query `{APP_PROMPT_QUERY_PARAM}` must be one of the embedded MCP prompts"
                    )));
                }
                if prompt.replace(value).is_some() {
                    return Err(ZocliError::Validation(format!(
                        "dashboard resource query `{APP_PROMPT_QUERY_PARAM}` cannot appear more than once"
                    )));
                }
            }
            _ => {
                return Err(ZocliError::UnsupportedOperation(format!(
                    "unknown MCP resource: {uri}"
                )));
            }
        }
    }

    match resource.as_deref() {
        Some(DASHBOARD_RESOURCE_SKILL) if skill.is_none() => {
            return Err(ZocliError::Validation(format!(
                "dashboard resource query `{APP_SKILL_QUERY_PARAM}` is required when `{APP_RESOURCE_QUERY_PARAM}=skill`"
            )));
        }
        Some(DASHBOARD_RESOURCE_ACCOUNT | DASHBOARD_RESOURCE_AUTH | DASHBOARD_RESOURCE_SKILLS)
            if skill.is_some() =>
        {
            return Err(ZocliError::Validation(format!(
                "dashboard resource query `{APP_SKILL_QUERY_PARAM}` is only valid when `{APP_RESOURCE_QUERY_PARAM}=skill`"
            )));
        }
        None if skill.is_some() => {
            return Err(ZocliError::Validation(format!(
                "dashboard resource query `{APP_SKILL_QUERY_PARAM}` requires `{APP_RESOURCE_QUERY_PARAM}=skill`"
            )));
        }
        _ => {}
    }

    if section.is_none() {
        if resource.is_some() {
            section = Some(DASHBOARD_SECTION_RESOURCES.to_string());
        } else if tool.is_some() {
            section = Some(DASHBOARD_SECTION_TOOLS.to_string());
        } else if prompt.is_some() {
            section = Some(DASHBOARD_SECTION_PROMPTS.to_string());
        }
    }

    if prompt.is_some() && section.as_deref() != Some(DASHBOARD_SECTION_PROMPTS) {
        return Err(ZocliError::Validation(format!(
            "dashboard resource query `{APP_PROMPT_QUERY_PARAM}` requires `{APP_SECTION_QUERY_PARAM}=prompts`"
        )));
    }

    Ok(DashboardResourceState {
        default_account: account,
        preferred_section: section,
        preferred_resource: resource,
        preferred_tool: tool,
        preferred_skill: skill,
        preferred_prompt: prompt,
    })
}

fn json_resource_contents(uri: &str, payload: Value) -> Result<Vec<Value>> {
    let text = serde_json::to_string_pretty(&payload)
        .map_err(|err| ZocliError::Serialization(err.to_string()))?;
    Ok(vec![json!({
        "uri": uri,
        "mimeType": "application/json",
        "text": text,
    })])
}

fn app_auth_discovery() -> Value {
    let auth = HttpAuthConfig::from_env();
    match HttpAuthDiscovery::from_config("127.0.0.1:8787", None, &auth) {
        Ok(Some(discovery)) => json!({
            "enabled": true,
            "authorizationServers": discovery.authorization_servers,
            "resourceMetadataUrl": discovery.protected_resource_metadata_url,
            "serverUrl": discovery.canonical_server_url,
        }),
        _ => json!({
            "enabled": false
        }),
    }
}

fn resource_digest(uri: &str) -> Result<u64> {
    let mut hasher = DefaultHasher::new();
    match resource_contents(uri) {
        Ok(contents) => {
            "ok".hash(&mut hasher);
            let encoded = serde_json::to_vec(&contents)
                .map_err(|err| ZocliError::Serialization(err.to_string()))?;
            encoded.hash(&mut hasher);
        }
        Err(err) => {
            "err".hash(&mut hasher);
            err.code().hash(&mut hasher);
            err.to_string().hash(&mut hasher);
        }
    }
    Ok(hasher.finish())
}

fn tool_visibility(tool_name: &str) -> &'static [&'static str] {
    match tool_name {
        "zocli.app.snapshot" => APP_ONLY_VISIBILITY,
        _ => MODEL_AND_APP_VISIBILITY,
    }
}

fn tool_surface(tool_name: &str) -> &'static str {
    match tool_name {
        name if name.starts_with("zocli.mail.") => APP_MAIL_RESOURCE_URI,
        name if name.starts_with("zocli.calendar.") => APP_CALENDAR_RESOURCE_URI,
        name if name.starts_with("zocli.drive.") => APP_DRIVE_RESOURCE_URI,
        "zocli.auth.status" => APP_AUTH_RESOURCE_URI,
        "zocli.account.list" | "zocli.account.current" => APP_ACCOUNT_RESOURCE_URI,
        _ => APP_RESOURCE_URI,
    }
}

fn jsonrpc_error_payload(id: Value, err: &ZocliError) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": error_code(err),
            "message": err.to_string(),
            "data": err.as_json(),
        }
    })
}

fn execute_stdio_request(
    method: &str,
    params: Value,
    session: &mut SessionState,
    writer: &mut dyn Write,
    rx: &mpsc::Receiver<InputEvent>,
    poller: &mut ResourceSubscriptionPoller,
) -> Result<Value> {
    if method == "tools/call" && requested_tool_name(&params) == Some("zocli.roots.list") {
        return roots_list_tool_stdio(session, writer, rx, poller);
    }

    handle_request(method, params, session, Some(poller))
}

fn roots_list_tool_stdio(
    session: &mut SessionState,
    writer: &mut dyn Write,
    rx: &mpsc::Receiver<InputEvent>,
    poller: &mut ResourceSubscriptionPoller,
) -> Result<Value> {
    if !session.supports_roots_requests {
        return Err(ZocliError::UnsupportedOperation(
            "zocli.roots.list requires stdio transport with client roots capability".to_string(),
        ));
    }

    if let Some(cached_roots) = session.cached_roots.clone()
        && !session.roots_dirty
    {
        return roots_tool_response(cached_roots, session.ui_enabled);
    }

    let request_id = session.next_outbound_request_id;
    session.next_outbound_request_id += 1;
    let request = json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "roots/list"
    });
    write_message(writer, &request, session.stdio_message_format)?;

    loop {
        match rx.recv_timeout(RESOURCE_POLL_INTERVAL) {
            Ok(InputEvent::Message(message, format)) => {
                session.stdio_message_format = format;

                if message.get("id") == Some(&json!(request_id)) && message.get("method").is_none()
                {
                    if let Some(error) = message.get("error") {
                        let code = error.get("code").and_then(Value::as_i64).unwrap_or(-32000);
                        let message = error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("roots/list failed");
                        let err = if code == -32601 {
                            ZocliError::UnsupportedOperation(message.to_string())
                        } else {
                            ZocliError::Validation(message.to_string())
                        };
                        return Err(err);
                    }

                    let roots = parse_roots_list_response(&message)?;
                    session.cached_roots = Some(roots.clone());
                    session.roots_dirty = false;
                    return roots_tool_response(roots, session.ui_enabled);
                }

                if message.get("method").is_none() {
                    continue;
                }

                let nested_method =
                    message
                        .get("method")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            ZocliError::Serialization("missing JSON-RPC method".to_string())
                        })?;
                let nested_params = message.get("params").cloned().unwrap_or(Value::Null);

                if message.get("id").is_none() {
                    handle_notification(nested_method, &nested_params, session);
                    continue;
                }

                let nested_id = message
                    .get("id")
                    .cloned()
                    .ok_or_else(|| ZocliError::Serialization("missing JSON-RPC id".to_string()))?;
                let response = match execute_stdio_request(
                    nested_method,
                    nested_params,
                    session,
                    writer,
                    rx,
                    poller,
                ) {
                    Ok(result) => json!({
                        "jsonrpc": "2.0",
                        "id": nested_id,
                        "result": result,
                    }),
                    Err(err) => json!({
                        "jsonrpc": "2.0",
                        "id": nested_id,
                        "error": {
                            "code": error_code(&err),
                            "message": err.to_string(),
                            "data": err.as_json(),
                        }
                    }),
                };
                write_message(writer, &response, session.stdio_message_format)?;
            }
            Ok(InputEvent::Eof) => {
                return Err(ZocliError::Io(
                    "stdio stream closed while waiting for roots/list response".to_string(),
                ));
            }
            Ok(InputEvent::Error(err)) => return Err(err),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(ZocliError::Io(
                    "stdio reader disconnected while waiting for roots/list response".to_string(),
                ));
            }
        }

        for notification in poller.collect_notifications(&session.resource_subscriptions)? {
            write_message(writer, &notification, session.stdio_message_format)?;
        }
    }
}

fn roots_tool_response(roots: Vec<Value>, ui_enabled: bool) -> Result<Value> {
    let structured = json!({
        "roots": roots,
        "schemaVersion": "1.0",
    });
    let text = serde_json::to_string_pretty(&structured)
        .map_err(|err| ZocliError::Serialization(err.to_string()))?;

    let mut response = Map::new();
    response.insert("structuredContent".to_string(), structured);
    response.insert(
        "content".to_string(),
        json!([
            {
                "type": "text",
                "text": text
            }
        ]),
    );
    if ui_enabled {
        response.insert(
            "_meta".to_string(),
            app_meta(
                tool_surface("zocli.roots.list"),
                tool_visibility("zocli.roots.list"),
            ),
        );
    }
    Ok(Value::Object(response))
}

fn parse_roots_list_response(message: &Value) -> Result<Vec<Value>> {
    let roots = message
        .get("result")
        .and_then(|result| result.get("roots"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ZocliError::Serialization("roots/list response missing `result.roots`".to_string())
        })?;

    roots.iter().map(validate_root).collect::<Result<Vec<_>>>()
}

fn validate_root(root: &Value) -> Result<Value> {
    let uri = root
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| ZocliError::Validation("roots/list item requires `uri`".to_string()))?;
    if !uri.starts_with("file://") {
        return Err(ZocliError::Validation(format!(
            "roots/list item `uri` must be a file:// URI, got `{uri}`"
        )));
    }

    let mut normalized = Map::new();
    normalized.insert("uri".to_string(), Value::String(uri.to_string()));
    if let Some(name) = root.get("name").and_then(Value::as_str) {
        normalized.insert("name".to_string(), Value::String(name.to_string()));
    }
    Ok(Value::Object(normalized))
}

fn requested_tool_name(params: &Value) -> Option<&str> {
    params.get("name").and_then(Value::as_str)
}

fn app_meta(resource_uri: &str, visibility: &[&str]) -> Value {
    json!({
        "ui": {
            "resourceUri": resource_uri,
            "visibility": visibility
        }
    })
}

fn app_resource_meta() -> Value {
    json!({
        "ui": {
            "prefersBorder": true,
            "csp": {
                "connectDomains": [],
                "resourceDomains": [],
                "frameDomains": []
            }
        }
    })
}

fn client_supports_ui(params: &Value) -> bool {
    capability_mime_types(params, "extensions")
        .into_iter()
        .chain(capability_mime_types(params, "experimental"))
        .any(|mime_type| mime_type == APP_RESOURCE_MIME_TYPE)
}

fn client_supports_roots(params: &Value) -> bool {
    params
        .get("capabilities")
        .and_then(|capabilities| capabilities.get("roots"))
        .is_some()
}

fn client_supports_roots_list_changed(params: &Value) -> bool {
    params
        .get("capabilities")
        .and_then(|capabilities| capabilities.get("roots"))
        .and_then(|roots| roots.get("listChanged"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn capability_mime_types<'a>(params: &'a Value, branch: &str) -> Vec<&'a str> {
    params
        .get("capabilities")
        .and_then(|capabilities| capabilities.get(branch))
        .and_then(|branch| branch.get(APP_EXTENSION_ID))
        .and_then(|ui| ui.get("mimeTypes"))
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

fn normalize_messages(payload: Value) -> Result<Vec<Value>> {
    match payload {
        Value::Array(messages) => {
            if messages.is_empty() {
                return Err(ZocliError::Validation(
                    "JSON-RPC batch payload must not be empty".to_string(),
                ));
            }
            Ok(messages)
        }
        single => Ok(vec![single]),
    }
}

fn is_client_response_message(message: &Value) -> bool {
    message.get("method").is_none()
        && message.get("id").is_some()
        && (message.get("result").is_some() || message.get("error").is_some())
}

fn request_accepts_sse(headers: &HeaderMap) -> bool {
    header_value(headers, "Accept")
        .map(|accept| accept.contains("text/event-stream"))
        .unwrap_or(false)
}

fn needs_http_auth(messages: &[Value]) -> bool {
    messages.iter().any(message_requires_http_auth)
}

fn message_requires_http_auth(message: &Value) -> bool {
    message.get("method").and_then(Value::as_str) == Some("tools/call")
        && message
            .get("params")
            .and_then(|params| params.get("name"))
            .and_then(Value::as_str)
            .map(tool_requires_http_auth)
            .unwrap_or(false)
}

fn tool_requires_http_auth(tool_name: &str) -> bool {
    matches!(tool_name, "zocli.auth.status")
        || tool_name.starts_with("zocli.mail.")
        || tool_name.starts_with("zocli.calendar.")
        || tool_name.starts_with("zocli.drive.")
}

fn read_message(reader: &mut dyn BufRead) -> Result<Option<(Value, StdioMessageFormat)>> {
    let mut content_length = None::<usize>;
    let mut line = String::new();

    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }

        if content_length.is_none() && matches!(trimmed.as_bytes().first(), Some(b'{' | b'[')) {
            let payload = serde_json::from_str::<Value>(trimmed).map_err(|err| {
                ZocliError::Serialization(format!("invalid JSON-RPC payload: {err}"))
            })?;
            return Ok(Some((payload, StdioMessageFormat::JsonLine)));
        }

        if let Some(value) = trimmed
            .split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("Content-Length"))
            .map(|(_, value)| value)
        {
            let parsed = value.trim().parse::<usize>().map_err(|err| {
                ZocliError::Serialization(format!("invalid Content-Length header: {err}"))
            })?;
            content_length = Some(parsed);
        }
    }

    let length = content_length
        .ok_or_else(|| ZocliError::Serialization("missing Content-Length header".to_string()))?;
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice::<Value>(&payload)
        .map(|value| Some((value, StdioMessageFormat::ContentLength)))
        .map_err(|err| ZocliError::Serialization(format!("invalid JSON-RPC payload: {err}")))
}

fn write_message(
    writer: &mut dyn Write,
    payload: &Value,
    format: StdioMessageFormat,
) -> Result<()> {
    let encoded =
        serde_json::to_vec(payload).map_err(|err| ZocliError::Serialization(err.to_string()))?;
    match format {
        StdioMessageFormat::ContentLength => {
            write!(writer, "Content-Length: {}\r\n\r\n", encoded.len())?;
            writer.write_all(&encoded)?;
        }
        StdioMessageFormat::JsonLine => {
            writer.write_all(&encoded)?;
            writer.write_all(b"\n")?;
        }
    }
    writer.flush()?;
    Ok(())
}

fn error_code(err: &ZocliError) -> i64 {
    match err {
        ZocliError::Validation(_) => -32602,
        ZocliError::UnsupportedOperation(_) => -32601,
        _ => -32000,
    }
}

fn new_session_id() -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(24)
        .map(char::from)
        .collect()
}

fn request_origin_allowed(headers: &HeaderMap) -> Result<()> {
    let Some(origin) = header_value(headers, "Origin") else {
        return Ok(());
    };

    let parsed = Url::parse(&origin).map_err(|err| {
        ZocliError::Validation(format!("invalid Origin header `{origin}`: {err}"))
    })?;
    let Some(host) = parsed.host_str() else {
        return Err(ZocliError::Validation(
            "origin is not allowed for local MCP HTTP transport".to_string(),
        ));
    };
    if matches!(host, "localhost" | "127.0.0.1" | "::1") {
        Ok(())
    } else {
        Err(ZocliError::Validation(
            "origin is not allowed for local MCP HTTP transport".to_string(),
        ))
    }
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|header| header.to_str().ok())
        .map(str::to_string)
}

impl HttpAuthConfig {
    fn from_env() -> Self {
        let bearer_token = std::env::var(HTTP_AUTH_TOKEN_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        Self { bearer_token }
    }

    fn authorized(&self, headers: &HeaderMap) -> bool {
        let Some(expected) = &self.bearer_token else {
            return true;
        };
        let Some(authorization) = header_value(headers, "Authorization") else {
            return false;
        };
        authorization
            .strip_prefix("Bearer ")
            .map(|token| token == expected)
            .unwrap_or(false)
    }
}

impl HttpAuthDiscovery {
    fn from_config(
        listen: &str,
        public_url: Option<&str>,
        auth: &HttpAuthConfig,
    ) -> Result<Option<Self>> {
        if auth.bearer_token.is_none() {
            return Ok(None);
        }

        let authorization_server = std::env::var(HTTP_AUTH_ISSUER_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let Some(authorization_server) = authorization_server else {
            return Ok(None);
        };

        let canonical_server_url = canonical_server_url(listen, public_url)?;
        let protected_resource_metadata_url = format!(
            "{}{}",
            canonical_base_url(&canonical_server_url)?,
            PROTECTED_RESOURCE_MCP_METADATA_PATH
        );

        Ok(Some(Self {
            authorization_servers: vec![authorization_server],
            canonical_server_url,
            protected_resource_metadata_url,
        }))
    }
}

fn http_json_response(status: StatusCode, payload: Value, session_id: Option<&str>) -> Response {
    let mut response = (status, Json(payload)).into_response();
    insert_common_http_headers(response.headers_mut(), session_id);
    response
}

fn http_error_response(status: StatusCode, message: &str, session_id: Option<&str>) -> Response {
    http_json_response(
        status,
        json!({
            "ok": false,
            "error": message,
        }),
        session_id,
    )
}

fn http_unauthorized_response(
    scopes: Vec<String>,
    auth_discovery: Option<&HttpAuthDiscovery>,
) -> Response {
    let mut payload = json!({
        "ok": false,
        "error": "invalid_token",
        "error_description": "protected MCP tool requires Authorization: Bearer",
    });
    if let Some(auth_discovery) = auth_discovery {
        payload["resource_metadata"] =
            Value::String(auth_discovery.protected_resource_metadata_url.clone());
    }
    if !scopes.is_empty() {
        payload["scope"] = Value::String(scopes.join(" "));
    }

    let mut response = http_json_response(StatusCode::UNAUTHORIZED, payload, None);
    let mut challenge = String::from("Bearer error=\"invalid_token\"");
    if let Some(auth_discovery) = auth_discovery {
        challenge.push_str(&format!(
            ", resource_metadata=\"{}\"",
            auth_discovery.protected_resource_metadata_url
        ));
    }
    if !scopes.is_empty() {
        challenge.push_str(&format!(", scope=\"{}\"", scopes.join(" ")));
    }
    response.headers_mut().insert(
        http_header::WWW_AUTHENTICATE,
        HeaderValue::from_str(&challenge)
            .unwrap_or_else(|_| HeaderValue::from_static("Bearer error=\"invalid_token\"")),
    );
    response
}

fn http_empty_response(status: StatusCode, session_id: Option<&str>) -> Response {
    let mut response = status.into_response();
    insert_common_http_headers(response.headers_mut(), session_id);
    response
}

fn insert_common_http_headers(headers: &mut HeaderMap, session_id: Option<&str>) {
    headers.insert(
        http_header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type, Mcp-Session-Id, Origin, Authorization, Accept"),
    );
    headers.insert(
        http_header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, DELETE, OPTIONS"),
    );
    if let Some(session_id) = session_id
        && let Ok(value) = HeaderValue::from_str(session_id)
    {
        headers.insert(HeaderName::from_static("mcp-session-id"), value);
    }
}

fn required_scopes(messages: &[Value]) -> Vec<String> {
    let mut scopes = BTreeSet::new();
    for message in messages {
        if message.get("method").and_then(Value::as_str) != Some("tools/call") {
            continue;
        }
        let Some(name) = message
            .get("params")
            .and_then(|params| params.get("name"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if let Some(scope) = tool_scope(name) {
            scopes.insert(scope.to_string());
        }
    }
    scopes.into_iter().collect()
}

fn tool_scope(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "zocli.auth.status" => Some("zocli.auth.read"),
        // Mail: read vs write
        "zocli.mail.folders"
        | "zocli.mail.list"
        | "zocli.mail.search"
        | "zocli.mail.read"
        | "zocli.mail.attachment_export" => Some("zocli.mail.read"),
        "zocli.mail.send" | "zocli.mail.reply" | "zocli.mail.forward" => Some("zocli.mail.write"),
        // Calendar: read vs write
        "zocli.calendar.calendars" | "zocli.calendar.events" => Some("zocli.calendar.read"),
        "zocli.calendar.create" | "zocli.calendar.delete" => Some("zocli.calendar.write"),
        // Drive: read vs write
        "zocli.drive.teams" | "zocli.drive.list" | "zocli.drive.download" => {
            Some("zocli.drive.read")
        }
        "zocli.drive.upload" => Some("zocli.drive.write"),
        _ => None,
    }
}

fn canonical_server_url(listen: &str, public_url: Option<&str>) -> Result<String> {
    let raw = public_url
        .map(str::to_string)
        .unwrap_or_else(|| format!("http://{listen}{HTTP_MCP_PATH}"));
    let parsed = Url::parse(&raw)
        .map_err(|err| ZocliError::Config(format!("invalid public MCP URL `{raw}`: {err}")))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(ZocliError::Config(format!(
            "public MCP URL must use http or https: {raw}"
        )));
    }
    Ok(parsed.to_string().trim_end_matches('/').to_string())
}

fn canonical_base_url(server_url: &str) -> Result<String> {
    let mut parsed = Url::parse(server_url).map_err(|err| {
        ZocliError::Config(format!("invalid public MCP URL `{server_url}`: {err}"))
    })?;
    parsed.set_path("");
    parsed.set_query(None);
    parsed.set_fragment(None);
    Ok(parsed.to_string().trim_end_matches('/').to_string())
}
