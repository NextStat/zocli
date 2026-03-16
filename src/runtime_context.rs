use serde::Serialize;

use crate::account_store::AccountStore;
use crate::credential_store::{CredentialStore, StoredCredential};
use crate::error::{Result, ZocliError};
use crate::model::{AccountConfig, datacenter_auth_url};
use crate::oauth::{refresh_access_token, unix_timestamp_now};

#[derive(Clone, Debug, Serialize)]
pub struct CredentialState {
    pub credential_ref: Option<String>,
    pub credential_state: &'static str,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope: Vec<String>,
}

#[derive(Clone, Copy)]
enum CredentialReference<'a> {
    Env(&'a str),
    Store(&'a str),
}

pub fn auth_state(
    credential_store: &CredentialStore,
    account_name: &str,
    reference: Option<&str>,
    service: &str,
) -> CredentialState {
    match reference {
        None => CredentialState {
            credential_ref: None,
            credential_state: "not_configured",
            detail: "service not yet connected".to_string(),
            scope: vec![],
        },
        Some(raw) => match parse_credential_ref(raw) {
            Some(CredentialReference::Env(var_name)) => match std::env::var_os(var_name) {
                Some(_) => CredentialState {
                    credential_ref: Some(raw.to_string()),
                    credential_state: "env_present",
                    detail: format!("using environment variable {var_name}"),
                    scope: vec![],
                },
                None => CredentialState {
                    credential_ref: Some(raw.to_string()),
                    credential_state: "env_missing",
                    detail: format!("environment variable {var_name} is not set"),
                    scope: vec![],
                },
            },
            Some(CredentialReference::Store(store_service)) => {
                if store_service != service {
                    return CredentialState {
                        credential_ref: Some(raw.to_string()),
                        credential_state: "store_mismatch",
                        detail: format!("reference points to store:{store_service}"),
                        scope: vec![],
                    };
                }

                match credential_store.get_service(account_name, service) {
                    Some(StoredCredential::Oauth(credential)) => {
                        let scopes = credential.scope.clone();
                        let now = unix_timestamp_now();
                        if credential.expires_at_epoch_secs > now.saturating_add(60) {
                            CredentialState {
                                credential_ref: Some(raw.to_string()),
                                credential_state: "store_present",
                                detail: format!(
                                    "stored OAuth token valid until {}",
                                    credential.expires_at_epoch_secs
                                ),
                                scope: scopes,
                            }
                        } else {
                            CredentialState {
                                credential_ref: Some(raw.to_string()),
                                credential_state: "store_expired",
                                detail: "stored OAuth token expired or expiring soon; run `zocli login` again".to_string(),
                                scope: scopes,
                            }
                        }
                    }
                    Some(StoredCredential::AppPassword(_)) => CredentialState {
                        credential_ref: Some(raw.to_string()),
                        credential_state: "store_present",
                        detail: "app password stored locally".to_string(),
                        scope: vec![],
                    },
                    None => CredentialState {
                        credential_ref: Some(raw.to_string()),
                        credential_state: "store_missing",
                        detail: format!("no local secret found for {service}"),
                        scope: vec![],
                    },
                }
            }
            None => CredentialState {
                credential_ref: Some(raw.to_string()),
                credential_state: "unsupported_reference",
                detail: "only env:NAME and store:SERVICE are supported".to_string(),
                scope: vec![],
            },
        },
    }
}

/// Resolve a Zoho API context for the given profile. Returns:
/// - account name (resolved from profile or default)
/// - account config (cloned)
/// - valid OAuth access token (auto-refreshed if expired and refresh_token is available)
pub fn resolve_zoho_context(profile: Option<&str>) -> Result<(String, AccountConfig, String)> {
    let account_store = AccountStore::load()?;
    let name = account_store.resolved_account_name(profile)?;
    let account = account_store.get_account(&name)?.clone();

    let reference = account.credential_ref.as_deref().ok_or_else(|| {
        ZocliError::Auth(format!(
            "{name} has no credential_ref configured; run `zocli login` first"
        ))
    })?;

    let access_token = match parse_credential_ref(reference) {
        Some(CredentialReference::Env(var_name)) => required_env(var_name)?,
        Some(CredentialReference::Store(store_service)) => {
            resolve_store_oauth_token(&name, &account, store_service)?
        }
        None => {
            return Err(ZocliError::Config(format!(
                "unsupported credential_ref format: {reference}"
            )));
        }
    };

    Ok((name, account, access_token))
}

/// Resolve an OAuth access token from the credential store, auto-refreshing if
/// the token is expired and a refresh_token is available.
fn resolve_store_oauth_token(
    account_name: &str,
    account: &AccountConfig,
    store_service: &str,
) -> Result<String> {
    let mut credential_store = CredentialStore::load()?;
    let stored = credential_store
        .get_oauth(account_name, store_service)
        .ok_or_else(|| {
            ZocliError::Auth(format!(
                "no stored OAuth credential for account {account_name} service {store_service}; run `zocli login` first"
            ))
        })?;

    let now = unix_timestamp_now();

    // Token is still valid (with 60-second buffer)
    if stored.expires_at_epoch_secs > now.saturating_add(60) {
        return Ok(stored.access_token.clone());
    }

    // Token is expired — try to refresh
    let refresh_token = stored.refresh_token.as_deref().ok_or_else(|| {
        ZocliError::Auth(format!(
            "OAuth token expired for account {account_name} and no refresh_token is available; run `zocli login --profile {account_name}` to re-authenticate"
        ))
    })?;

    let auth_base_url = datacenter_auth_url(&account.datacenter);
    let client_id = account.oauth_client_id().ok_or_else(|| {
        ZocliError::Config(format!(
            "no OAuth client is configured for account {account_name}; configure the shared/default zocli OAuth app or re-run `zocli add --client-id ...`"
        ))
    })?;
    let client_secret = account.oauth_client_secret();
    let new_credential = refresh_access_token(
        &auth_base_url,
        &client_id,
        client_secret.as_deref(),
        refresh_token,
    )?;

    let new_token = new_credential.access_token.clone();

    credential_store.set_oauth(
        account_name.to_string(),
        store_service.to_string(),
        new_credential,
    );
    credential_store.save()?;

    Ok(new_token)
}

fn required_env(name: &str) -> Result<String> {
    std::env::var(name).map_err(|_| {
        ZocliError::Config(format!("required environment variable is missing: {name}"))
    })
}

fn parse_credential_ref(raw: &str) -> Option<CredentialReference<'_>> {
    if let Some(value) = raw.strip_prefix("env:") {
        return Some(CredentialReference::Env(value));
    }
    if let Some(value) = raw.strip_prefix("store:") {
        return Some(CredentialReference::Store(value));
    }
    None
}
