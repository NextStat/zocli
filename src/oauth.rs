use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::credential_store::StoredOauthCredential;
use crate::error::{Result, ZocliError};

/// All available Zoho OAuth scopes for Mail, Calendar, and WorkDrive.
pub const MAIL_SCOPES: &[&str] = &[
    "ZohoMail.messages.ALL",
    "ZohoMail.folders.ALL",
    "ZohoMail.accounts.READ",
];
pub const CALENDAR_SCOPES: &[&str] = &["ZohoCalendar.event.ALL", "ZohoCalendar.calendar.ALL"];
pub const DRIVE_SCOPES: &[&str] = &[
    "WorkDrive.files.ALL",
    "WorkDrive.workspace.ALL",
    "WorkDrive.team.ALL",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OauthService {
    Mail,
    Calendar,
    Drive,
}

impl OauthService {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mail => "mail",
            Self::Calendar => "calendar",
            Self::Drive => "drive",
        }
    }

    pub fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::Mail => MAIL_SCOPES,
            Self::Calendar => CALENDAR_SCOPES,
            Self::Drive => DRIVE_SCOPES,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AuthorizationRequest {
    pub authorization_url: String,
    pub redirect_uri: String,
    pub state: String,
    pub code_challenge_method: &'static str,
}

#[derive(Clone, Debug)]
pub struct AuthorizationSession {
    pub request: AuthorizationRequest,
    client_id: String,
    client_secret: Option<String>,
    code_verifier: String,
    auth_base_url: String,
}

#[derive(Clone, Debug)]
pub struct LoginResult {
    pub authorization: AuthorizationRequest,
    pub credential: StoredOauthCredential,
}

/// The local redirect URI used for the OAuth callback.
pub const REDIRECT_URI: &str = "http://127.0.0.1:9004/callback";

pub fn start_pkce_authorization(
    services: &[OauthService],
    client_id: &str,
    client_secret: Option<&str>,
    auth_base_url: &str,
    login_hint: Option<&str>,
) -> Result<AuthorizationSession> {
    if services.is_empty() {
        return Err(ZocliError::Config(
            "OAuth authorization requires at least one target service".to_string(),
        ));
    }
    let code_verifier = random_token(32);
    let code_challenge = pkce_challenge(&code_verifier);
    let state = random_token(16);
    let scopes = deduped_scopes(services);
    let request = AuthorizationRequest {
        authorization_url: build_authorize_url(
            auth_base_url,
            &scopes,
            client_id,
            &state,
            &code_challenge,
            login_hint,
        )?,
        redirect_uri: REDIRECT_URI.to_string(),
        state,
        code_challenge_method: "S256",
    };

    Ok(AuthorizationSession {
        request,
        client_id: client_id.to_string(),
        client_secret: client_secret.map(ToString::to_string),
        code_verifier,
        auth_base_url: auth_base_url.to_string(),
    })
}

pub fn exchange_authorization_code(
    session: AuthorizationSession,
    code: &str,
) -> Result<LoginResult> {
    let client = build_http_client()?;
    let token_url = oauth_endpoint(&session.auth_base_url, "/oauth/v2/token")?;

    let mut params = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", session.client_id.as_str()),
        ("redirect_uri", REDIRECT_URI),
        ("code_verifier", session.code_verifier.as_str()),
    ];
    let secret_owned;
    if let Some(ref secret) = session.client_secret {
        secret_owned = secret.clone();
        params.push(("client_secret", &secret_owned));
    }

    let response = client.post(token_url).form(&params).send()?;
    let status = response.status();
    let body = response.text()?;

    if !status.is_success() {
        return Err(oauth_error(status, &body, "authorization code exchange"));
    }

    let token = serde_json::from_str::<ZohoTokenResponse>(&body)
        .map_err(|err| ZocliError::Serialization(format!("invalid OAuth token response: {err}")))?;

    Ok(LoginResult {
        authorization: session.request,
        credential: token.into_stored(&session.client_id),
    })
}

/// Refresh an expired access token using the stored refresh token.
pub fn refresh_access_token(
    auth_base_url: &str,
    client_id: &str,
    client_secret: Option<&str>,
    refresh_token: &str,
) -> Result<StoredOauthCredential> {
    let client = build_http_client()?;
    let token_url = oauth_endpoint(auth_base_url, "/oauth/v2/token")?;

    let mut params = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    if let Some(secret) = client_secret {
        params.push(("client_secret", secret));
    }

    let response = client.post(token_url).form(&params).send()?;
    let status = response.status();
    let body = response.text()?;

    if !status.is_success() {
        return Err(oauth_error(status, &body, "token refresh"));
    }

    let token = serde_json::from_str::<ZohoRefreshResponse>(&body)
        .map_err(|err| ZocliError::Serialization(format!("invalid refresh response: {err}")))?;

    Ok(StoredOauthCredential {
        kind: "oauth_pkce".to_string(),
        access_token: token.access_token,
        refresh_token: Some(refresh_token.to_string()),
        token_type: token.token_type,
        expires_at_epoch_secs: unix_timestamp_now().saturating_add(token.expires_in),
        scope: token
            .scope
            .split(',')
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect(),
        client_id: client_id.to_string(),
        api_domain: token.api_domain,
    })
}

pub fn unix_timestamp_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn build_authorize_url(
    base_url: &str,
    scopes: &[&str],
    client_id: &str,
    state: &str,
    code_challenge: &str,
    login_hint: Option<&str>,
) -> Result<String> {
    let mut url = oauth_endpoint(base_url, "/oauth/v2/auth")?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", client_id);
        query.append_pair("redirect_uri", REDIRECT_URI);
        query.append_pair("scope", &scopes.join(","));
        query.append_pair("state", state);
        query.append_pair("code_challenge", code_challenge);
        query.append_pair("code_challenge_method", "S256");
        query.append_pair("access_type", "offline");
        query.append_pair("prompt", "consent");
        if let Some(login_hint) = login_hint {
            query.append_pair("login_hint", login_hint);
        }
    }

    Ok(url.to_string())
}

fn build_http_client() -> Result<Client> {
    Client::builder()
        .user_agent(format!("zocli/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(Into::into)
}

fn oauth_endpoint(base_url: &str, path: &str) -> Result<Url> {
    Url::parse(base_url)
        .map_err(|err| ZocliError::Config(format!("invalid OAuth base URL: {err}")))?
        .join(path)
        .map_err(|err| ZocliError::Config(format!("invalid OAuth endpoint: {err}")))
}

fn deduped_scopes(services: &[OauthService]) -> Vec<&'static str> {
    let mut scopes = Vec::new();
    for service in services {
        for scope in service.scopes() {
            if !scopes.contains(scope) {
                scopes.push(*scope);
            }
        }
    }
    scopes
}

fn pkce_challenge(code_verifier: &str) -> String {
    let digest = Sha256::digest(code_verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn random_token(byte_len: usize) -> String {
    let mut bytes = vec![0_u8; byte_len];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn oauth_error(status: StatusCode, body: &str, action: &str) -> ZocliError {
    let parsed = serde_json::from_str::<OauthErrorBody>(body).ok();
    let code = parsed
        .as_ref()
        .and_then(|value| value.error.as_deref())
        .unwrap_or("unknown_oauth_error");
    let description = parsed
        .as_ref()
        .and_then(|value| value.error_description.as_deref())
        .unwrap_or("No provider message returned.");

    ZocliError::Auth(format!(
        "Zoho OAuth {} failed with status {} ({}): {}",
        action,
        status.as_u16(),
        code,
        description
    ))
}

/// Zoho token response includes refresh_token and api_domain.
#[derive(Debug, Deserialize)]
struct ZohoTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    token_type: String,
    expires_in: u64,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    api_domain: Option<String>,
}

impl ZohoTokenResponse {
    fn into_stored(self, client_id: &str) -> StoredOauthCredential {
        StoredOauthCredential {
            kind: "oauth_pkce".to_string(),
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            token_type: self.token_type,
            expires_at_epoch_secs: unix_timestamp_now().saturating_add(self.expires_in),
            scope: self
                .scope
                .split(',')
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect(),
            client_id: client_id.to_string(),
            api_domain: self.api_domain,
        }
    }
}

/// Zoho refresh response (no new refresh_token).
#[derive(Debug, Deserialize)]
struct ZohoRefreshResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    api_domain: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OauthErrorBody {
    error: Option<String>,
    #[serde(alias = "error_description")]
    error_description: Option<String>,
}
