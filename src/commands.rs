use std::io;

use serde::Serialize;
use serde_json::json;

use crate::account_store::{AccountStore, validate_account};
use crate::calendar::{
    CalendarCollection, CalendarCreateRequest, CalendarEvent, CalendarEventWindow,
    CalendarEventsRequest, create_calendar_event, delete_calendar_event, list_calendar_events,
    list_calendars, parse_event_window,
};
use crate::cli::{
    AccountCommand, AuthCommand, AuthServiceArg, CalendarCommand, Cli, Command, DriveCommand,
    GuideTopicArg, MailCommand, McpCommand, OutputFormat,
};
use crate::credential_store::CredentialStore;
use crate::disk::{
    DownloadedFile, DriveFile, DriveTeam, UploadedFile, download_file, list_files, list_teams,
    upload_file,
};
use crate::error::{Result, ZocliError};
use crate::mail::{
    ForwardedMail, MailFolder, MailForwardRequest, MailMessage, MailMessageSummary,
    MailReplyRequest, MailSendRequest, RepliedMail, SentMail, forward_mail_message,
    list_mail_folders, list_mail_messages, read_mail_message, reply_to_mail_message,
    search_mail_messages, send_mail_message, upload_attachment,
};
use crate::mcp::install::execute_install;
use crate::model::{AccountConfig, NewAccountInput};
use crate::oauth::{OauthService, exchange_authorization_code, start_pkce_authorization};
use crate::output::RenderedOutput;
use crate::runtime_context::{auth_state, resolve_zoho_context};
use crate::update::execute_update;

// ---------------------------------------------------------------------------
// Top-level dispatch
// ---------------------------------------------------------------------------

pub fn execute(cli: Cli) -> Result<RenderedOutput> {
    match cli.command {
        Command::Guide { topic } => execute_guide(cli.format, topic),
        Command::Add {
            email,
            name,
            datacenter,
            account_id,
            org_id,
            client_id,
            client_secret,
        } => execute_simple_add(
            cli.format,
            email,
            name,
            datacenter,
            account_id,
            org_id,
            client_id,
            client_secret,
        ),
        Command::Accounts => execute_account(cli.format, AccountCommand::List),
        Command::Use { name } => execute_account(cli.format, AccountCommand::Use { name }),
        Command::Whoami => execute_account(cli.format, AccountCommand::Current),
        Command::Status { profile } => execute_auth(cli.format, AuthCommand::Status { profile }),
        Command::Login {
            service,
            profile,
            code,
            login_hint,
        } => execute_auth(
            cli.format,
            AuthCommand::Login {
                profile,
                service,
                code,
                login_hint,
            },
        ),
        Command::Logout { service, profile } => execute_simple_logout(cli.format, profile, service),
        Command::Account { action } => execute_account(cli.format, action),
        Command::Auth { action } => execute_auth(cli.format, action),
        Command::Drive { action } => execute_drive(cli.format, action),
        Command::Calendar { action } => execute_calendar(cli.format, action),
        Command::Mail { action } => execute_mail(cli.format, action),
        Command::Mcp {
            action:
                Some(McpCommand::Install {
                    client,
                    transport,
                    url,
                }),
            ..
        } => execute_install(cli.format, client, transport, url),
        Command::Mcp { action: None, .. } => Err(ZocliError::UnsupportedOperation(
            "mcp server mode is handled in main".to_string(),
        )),
        Command::Update {
            version,
            check,
            base_url,
        } => execute_update(cli.format, &version, check, base_url.as_deref()),
    }
}

// ---------------------------------------------------------------------------
// Account commands
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn execute_simple_add(
    format: OutputFormat,
    email: String,
    name: Option<String>,
    datacenter: String,
    account_id: Option<String>,
    org_id: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
) -> Result<RenderedOutput> {
    let name = name.unwrap_or_else(|| derive_account_name_from_email(&email));
    execute_account(
        format,
        AccountCommand::Add {
            name,
            email,
            use_as_current: true,
            datacenter,
            account_id,
            org_id,
            client_id,
            client_secret,
        },
    )
}

fn execute_account(format: OutputFormat, action: AccountCommand) -> Result<RenderedOutput> {
    match action {
        AccountCommand::Add {
            name,
            email,
            use_as_current,
            datacenter,
            account_id,
            org_id,
            client_id,
            client_secret,
        } => {
            let account_config = AccountConfig::new(NewAccountInput {
                email,
                default: use_as_current,
                datacenter,
                account_id,
                org_id,
                client_id,
                client_secret,
            });
            let report = validate_account(&name, &account_config);
            if !report.valid {
                return Err(ZocliError::Validation(report.errors.join("; ")));
            }

            let mut store = AccountStore::load()?;
            store.add_account(name.clone(), account_config.clone())?;
            let current = store.current_account_name()? == name;
            store.save()?;

            ok_output(
                format,
                "account.add",
                json!({
                    "account": name,
                    "config": account_config,
                    "current": current,
                }),
                render_key_value_table(&[
                    ("operation", "account.add".to_string()),
                    ("account", name),
                    ("email", account_config.email),
                    ("current", current.to_string()),
                ]),
            )
        }
        AccountCommand::List => {
            let store = AccountStore::load()?;
            let items: Vec<_> = store
                .summaries()
                .into_iter()
                .map(|(name, account)| {
                    let current = store.is_current_account(&name);
                    json!({
                        "name": name,
                        "email": account.email,
                        "datacenter": account.datacenter,
                        "current": current,
                    })
                })
                .collect();

            let table = if items.is_empty() {
                "No accounts configured".to_string()
            } else {
                let mut lines = vec!["NAME\tEMAIL\tDATACENTER\tCURRENT".to_string()];
                for item in &items {
                    lines.push(format!(
                        "{}\t{}\t{}\t{}",
                        item["name"].as_str().unwrap_or_default(),
                        item["email"].as_str().unwrap_or_default(),
                        item["datacenter"].as_str().unwrap_or_default(),
                        item["current"].as_bool().unwrap_or(false)
                    ));
                }
                lines.join("\n")
            };

            ok_output(format, "account.list", json!({ "items": items }), table)
        }
        AccountCommand::Show { profile } => {
            let store = AccountStore::load()?;
            let name = store.resolved_account_name(profile.as_deref())?;
            let account = store.get_account(&name)?;
            let current = store.is_current_account(&name);
            ok_output(
                format,
                "account.show",
                json!({
                    "account": name,
                    "current": current,
                    "config": account,
                }),
                render_account_table(&name, account, current),
            )
        }
        AccountCommand::Validate { profile } => {
            let store = AccountStore::load()?;
            let name = store.resolved_account_name(profile.as_deref())?;
            let account = store.get_account(&name)?;
            let report = validate_account(&name, account);
            ok_output(
                format,
                "account.validate",
                json!({
                    "account": name,
                    "valid": report.valid,
                    "errors": report.errors,
                }),
                render_account_validation_table(&name, &report.errors),
            )
        }
        AccountCommand::Use { name } => {
            let mut store = AccountStore::load()?;
            store.set_current(&name)?;
            store.save()?;
            ok_output(
                format,
                "account.use",
                json!({
                    "account": name,
                    "current": true,
                }),
                render_key_value_table(&[
                    ("operation", "account.use".to_string()),
                    ("account", name),
                    ("current", "true".to_string()),
                ]),
            )
        }
        AccountCommand::Current => {
            let store = AccountStore::load()?;
            let name = store.current_account_name()?;
            let account = store.get_account(&name)?;
            ok_output(
                format,
                "account.current",
                json!({
                    "account": name,
                    "email": account.email,
                }),
                render_key_value_table(&[
                    ("operation", "account.current".to_string()),
                    ("account", name),
                    ("email", account.email.clone()),
                ]),
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Auth commands
// ---------------------------------------------------------------------------

fn execute_simple_logout(
    format: OutputFormat,
    profile: Option<String>,
    service: Option<AuthServiceArg>,
) -> Result<RenderedOutput> {
    let services = match service {
        Some(service) => vec![service],
        None => vec![
            AuthServiceArg::Mail,
            AuthServiceArg::Calendar,
            AuthServiceArg::Drive,
        ],
    };

    let mut items = Vec::with_capacity(services.len());
    for service in services {
        let output = execute_auth(
            OutputFormat::Json,
            AuthCommand::Logout {
                profile: profile.clone(),
                service: Some(service),
            },
        )?;
        items.push(output.json);
    }

    let account_name = items
        .first()
        .and_then(|item| item.get("account"))
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let table = if items.is_empty() {
        "No services logged out".to_string()
    } else {
        let mut lines = vec!["SERVICE\tREMOVED".to_string()];
        for item in &items {
            lines.push(format!(
                "{}\t{}",
                item["service"].as_str().unwrap_or_default(),
                item["removed"].as_bool().unwrap_or(false),
            ));
        }
        lines.join("\n")
    };

    ok_output(
        format,
        "logout",
        json!({
            "account": account_name,
            "items": items,
        }),
        table,
    )
}

fn execute_auth(format: OutputFormat, action: AuthCommand) -> Result<RenderedOutput> {
    match action {
        AuthCommand::Status { profile } => {
            let account_store = AccountStore::load()?;
            let name = account_store.resolved_account_name(profile.as_deref())?;
            let account = account_store.get_account(&name)?;
            let credential_store = CredentialStore::load()?;

            let state = auth_state(
                &credential_store,
                &name,
                account.credential_ref.as_deref(),
                "oauth",
            );

            let table = [
                format!("ACCOUNT\t{}", name),
                format!("EMAIL\t{}", account.email),
                format!("DATACENTER\t{}", account.datacenter),
                "STATUS\tDETAIL".to_string(),
                format!(
                    "{}\t{}",
                    credential_state_label(state.credential_state),
                    state.detail
                ),
            ]
            .join("\n");

            ok_output(
                format,
                "auth.status",
                json!({
                    "account": name,
                    "email": account.email,
                    "datacenter": account.datacenter,
                    "credential_state": state,
                }),
                table,
            )
        }
        AuthCommand::Login {
            profile,
            service,
            code,
            login_hint,
        } => {
            let mut account_store = AccountStore::load()?;
            let account_name = account_store.resolved_account_name(profile.as_deref())?;
            let account = account_store.get_account(&account_name)?.clone();
            let client_id = account.oauth_client_id().ok_or_else(|| {
                ZocliError::Config(
                    "no OAuth client is configured for this account. Use the shared/default zocli OAuth app or re-run `zocli add --client-id ...` as an advanced override."
                        .to_string(),
                )
            })?;
            let client_secret = account.oauth_client_secret();

            let services = oauth_login_services(service)?;

            let resolved_login_hint = login_hint.as_deref().or(Some(account.email.as_str()));
            let auth_base_url = account.auth_base_url();

            let session = start_pkce_authorization(
                &services,
                &client_id,
                client_secret.as_deref(),
                &auth_base_url,
                resolved_login_hint,
            )?;

            let code = match code {
                Some(code) => code,
                None => read_confirmation_code(&session.request.authorization_url)?,
            };

            let login = exchange_authorization_code(session, &code)?;

            let mut credential_store = CredentialStore::load()?;
            credential_store.set_oauth(
                account_name.clone(),
                "oauth".to_string(),
                login.credential.clone(),
            );
            credential_store.save()?;

            account_store.set_credential_ref(&account_name, Some("store:oauth".to_string()))?;

            // Auto-discover account_id and ZUID
            {
                let mail_base = account.mail_api_url();
                match crate::mail::discover_mail_account(&mail_base, &login.credential.access_token)
                {
                    Ok(discovered) => {
                        if account.account_id == "0" || account.account_id.is_empty() {
                            eprintln!("Discovered account_id: {}", discovered.account_id);
                            account_store.set_account_id(&account_name, &discovered.account_id)?;
                        }
                        if let Some(ref zuid) = discovered.zuid {
                            eprintln!("Discovered ZUID: {zuid}");
                            account_store.set_zuid(&account_name, zuid)?;
                        }
                    }
                    Err(err) => {
                        eprintln!("Warning: could not auto-discover account info: {err}");
                    }
                }
            }

            account_store.save()?;

            ok_output(
                format,
                "auth.login",
                json!({
                    "account": account_name,
                    "services": services.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    "credential_ref": "store:oauth",
                    "authorization": login.authorization,
                    "token": {
                        "token_type": login.credential.token_type,
                        "expires_at_epoch_secs": login.credential.expires_at_epoch_secs,
                        "scope": login.credential.scope,
                    }
                }),
                render_key_value_table(&[
                    ("operation", "auth.login".to_string()),
                    ("account", account_name),
                    (
                        "services",
                        services
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(","),
                    ),
                    ("credential_ref", "store:oauth".to_string()),
                    (
                        "expires_at_epoch_secs",
                        login.credential.expires_at_epoch_secs.to_string(),
                    ),
                ]),
            )
        }
        AuthCommand::Logout { profile, service } => {
            let service = service.ok_or_else(|| {
                ZocliError::Validation(
                    "logout without service is handled by the top-level `zocli logout`; internal `auth logout` still requires an explicit service".to_string(),
                )
            })?;

            let mut account_store = AccountStore::load()?;
            let account_name = account_store.resolved_account_name(profile.as_deref())?;

            let service_name = match service {
                AuthServiceArg::Mail => "mail",
                AuthServiceArg::Calendar => "calendar",
                AuthServiceArg::Drive => "drive",
            };

            // All services share the single "oauth" credential. Removing it
            // will affect all services, but per-service logout is kept for
            // reporting purposes.
            let mut credential_store = CredentialStore::load()?;
            let removed = credential_store.remove_service(&account_name, "oauth");
            if removed {
                credential_store.save()?;
            }

            // Clear the account credential_ref when logging out of any service.
            let had_ref = account_store
                .get_account(&account_name)?
                .credential_ref
                .is_some();
            if had_ref {
                account_store.set_credential_ref(&account_name, None)?;
                account_store.save()?;
            }

            ok_output(
                format,
                "auth.logout",
                json!({
                    "account": account_name,
                    "service": service_name,
                    "removed": removed,
                }),
                render_key_value_table(&[
                    ("operation", "auth.logout".to_string()),
                    ("account", account_name),
                    ("service", service_name.to_string()),
                    ("removed", removed.to_string()),
                ]),
            )
        }
    }
}

fn oauth_login_services(requested: Option<AuthServiceArg>) -> Result<Vec<OauthService>> {
    match requested {
        Some(AuthServiceArg::Mail) => Ok(vec![OauthService::Mail]),
        Some(AuthServiceArg::Calendar) => Ok(vec![OauthService::Calendar]),
        Some(AuthServiceArg::Drive) => Ok(vec![OauthService::Drive]),
        None => Ok(vec![
            OauthService::Mail,
            OauthService::Calendar,
            OauthService::Drive,
        ]),
    }
}

// ---------------------------------------------------------------------------
// Drive commands
// ---------------------------------------------------------------------------

fn execute_drive(format: OutputFormat, action: DriveCommand) -> Result<RenderedOutput> {
    match action {
        DriveCommand::List {
            profile,
            folder_id,
            limit,
            offset,
        } => {
            let (account_name, account, access_token) = resolve_zoho_context(profile.as_deref())?;

            match folder_id {
                Some(folder_id) => {
                    let base_url = account.drive_api_url();
                    let files = list_files(&base_url, &access_token, &folder_id, limit, offset)?;

                    ok_output(
                        format,
                        "drive.list",
                        json!({
                            "account": account_name,
                            "folder_id": folder_id,
                            "limit": limit,
                            "offset": offset,
                            "files": files,
                        }),
                        render_drive_files_table(&account_name, &files),
                    )
                }
                None => {
                    // No folder_id specified: list teams as an entry point
                    let base_url = account.drive_api_url();
                    let teams = list_teams(&base_url, &account.account_id, &access_token)?;

                    ok_output(
                        format,
                        "drive.list",
                        json!({
                            "account": account_name,
                            "teams": teams,
                        }),
                        render_drive_teams_table(&account_name, &teams),
                    )
                }
            }
        }
        DriveCommand::Info { profile } => {
            let (account_name, account, access_token) = resolve_zoho_context(profile.as_deref())?;
            let base_url = account.drive_api_url();
            let drive_user_id = account.zuid.as_deref().unwrap_or(&account.account_id);
            let teams = list_teams(&base_url, drive_user_id, &access_token)?;

            ok_output(
                format,
                "drive.info",
                json!({
                    "account": account_name,
                    "teams": teams,
                }),
                render_drive_teams_table(&account_name, &teams),
            )
        }
        DriveCommand::Upload {
            profile,
            source,
            folder_id,
            overwrite,
        } => {
            let (account_name, account, access_token) = resolve_zoho_context(profile.as_deref())?;
            let upload_url = account.drive_upload_url();
            let uploaded = upload_file(&upload_url, &access_token, &folder_id, &source, overwrite)?;

            ok_output(
                format,
                "drive.upload",
                json!({
                    "account": account_name,
                    "folder_id": folder_id,
                    "source": source.display().to_string(),
                    "overwrite": overwrite,
                    "uploaded": uploaded,
                }),
                render_drive_upload_table(&account_name, &uploaded),
            )
        }
        DriveCommand::Download {
            profile,
            file_id,
            output,
            force,
        } => {
            let (account_name, account, access_token) = resolve_zoho_context(profile.as_deref())?;
            let download_url = account.drive_download_url(&file_id);
            let downloaded = download_file(&download_url, &access_token, &output, force)?;

            ok_output(
                format,
                "drive.download",
                json!({
                    "account": account_name,
                    "file_id": file_id,
                    "output": output.display().to_string(),
                    "force": force,
                    "downloaded": downloaded,
                }),
                render_drive_download_table(&account_name, &downloaded),
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Calendar commands
// ---------------------------------------------------------------------------

fn execute_calendar(format: OutputFormat, action: CalendarCommand) -> Result<RenderedOutput> {
    match action {
        CalendarCommand::Calendars { profile } => {
            let (account_name, account, access_token) = resolve_zoho_context(profile.as_deref())?;
            let base_url = account.calendar_api_url();
            let calendars = list_calendars(&base_url, "", &access_token)?;

            ok_output(
                format,
                "calendar.calendars",
                json!({
                    "account": account_name,
                    "calendars": calendars,
                }),
                render_calendar_collections_table(&account_name, &calendars),
            )
        }
        CalendarCommand::Events {
            profile,
            calendar,
            from,
            to,
            limit,
        } => {
            let window = parse_event_window(from.as_deref(), to.as_deref(), limit)?;
            let (account_name, account, access_token) = resolve_zoho_context(profile.as_deref())?;
            let base_url = account.calendar_api_url();

            let calendar_ref = calendar.unwrap_or_else(|| "default".to_string());
            let request = CalendarEventsRequest {
                calendar: calendar_ref,
                from: chrono::DateTime::parse_from_rfc3339(&window.from)
                    .map_err(|err| {
                        ZocliError::Serialization(format!(
                            "failed to rebuild calendar --from boundary: {err}"
                        ))
                    })?
                    .with_timezone(&chrono::Utc),
                to: chrono::DateTime::parse_from_rfc3339(&window.to)
                    .map_err(|err| {
                        ZocliError::Serialization(format!(
                            "failed to rebuild calendar --to boundary: {err}"
                        ))
                    })?
                    .with_timezone(&chrono::Utc),
                limit: window.limit,
            };

            let (calendar, window, events) =
                list_calendar_events(&base_url, "", &access_token, request)?;

            ok_output(
                format,
                "calendar.events",
                json!({
                    "account": account_name,
                    "calendar": calendar,
                    "window": window,
                    "events": events.iter().map(calendar_event_json).collect::<Vec<_>>(),
                }),
                render_calendar_events_table(&account_name, &calendar, &window, &events),
            )
        }
        CalendarCommand::Create {
            profile,
            calendar,
            title,
            start,
            end,
            description,
            location,
        } => {
            let (account_name, account, access_token) = resolve_zoho_context(profile.as_deref())?;
            let base_url = account.calendar_api_url();

            let calendar_ref = calendar.unwrap_or_else(|| "default".to_string());
            let (calendar, event) = create_calendar_event(
                &base_url,
                "",
                &access_token,
                CalendarCreateRequest {
                    calendar: calendar_ref,
                    summary: title,
                    start,
                    end,
                    description,
                    location,
                },
            )?;

            ok_output(
                format,
                "calendar.create",
                json!({
                    "account": account_name,
                    "calendar": calendar,
                    "event": calendar_event_json(&event),
                }),
                render_calendar_create_table(&account_name, &calendar, &event),
            )
        }
        CalendarCommand::Delete {
            profile,
            calendar,
            event_uid,
            etag: _,
        } => {
            let (account_name, account, access_token) = resolve_zoho_context(profile.as_deref())?;
            let base_url = account.calendar_api_url();

            let calendar_ref = calendar.unwrap_or_else(|| "default".to_string());
            let (calendar, event) =
                delete_calendar_event(&base_url, "", &access_token, &calendar_ref, &event_uid)?;

            ok_output(
                format,
                "calendar.delete",
                json!({
                    "account": account_name,
                    "calendar": calendar,
                    "deleted_event": calendar_event_json(&event),
                }),
                render_calendar_delete_table(&account_name, &calendar, &event),
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Mail commands
// ---------------------------------------------------------------------------

fn execute_mail(format: OutputFormat, action: MailCommand) -> Result<RenderedOutput> {
    match action {
        MailCommand::Folders { profile } => {
            let (account_name, account, access_token) = resolve_zoho_context(profile.as_deref())?;
            let base_url = account.mail_api_url();
            let folders = list_mail_folders(&base_url, &account.account_id, &access_token)?;

            ok_output(
                format,
                "mail.folders",
                json!({
                    "account": account_name,
                    "folders": folders,
                }),
                render_mail_folders_table(&account_name, &folders),
            )
        }
        MailCommand::List {
            profile,
            folder_id,
            unread,
            limit,
        } => {
            let (account_name, account, access_token) = resolve_zoho_context(profile.as_deref())?;
            let base_url = account.mail_api_url();
            let messages = list_mail_messages(
                &base_url,
                &account.account_id,
                &access_token,
                folder_id.as_deref(),
                unread,
                limit,
            )?;

            ok_output(
                format,
                "mail.list",
                json!({
                    "account": account_name,
                    "folder_id": folder_id,
                    "limit": limit,
                    "messages": messages,
                }),
                render_mail_list_table(&account_name, &messages),
            )
        }
        MailCommand::Search {
            profile,
            query,
            limit,
        } => {
            let (account_name, account, access_token) = resolve_zoho_context(profile.as_deref())?;
            let base_url = account.mail_api_url();
            let messages =
                search_mail_messages(&base_url, &account.account_id, &access_token, &query, limit)?;

            ok_output(
                format,
                "mail.search",
                json!({
                    "account": account_name,
                    "query": query,
                    "limit": limit,
                    "messages": messages,
                }),
                render_mail_search_table(&account_name, &query, &messages),
            )
        }
        MailCommand::Read {
            profile,
            message_id,
            folder_id,
        } => {
            let (account_name, account, access_token) = resolve_zoho_context(profile.as_deref())?;
            let base_url = account.mail_api_url();
            let message = read_mail_message(
                &base_url,
                &account.account_id,
                &access_token,
                &folder_id,
                &message_id,
            )?;

            ok_output(
                format,
                "mail.read",
                json!({
                    "account": account_name,
                    "folder_id": folder_id,
                    "message_id": message_id,
                    "message": message,
                }),
                render_mail_read_table(&account_name, &message),
            )
        }
        MailCommand::Send {
            profile,
            to,
            subject,
            body,
            cc,
            bcc,
            html,
            attachments,
        } => {
            let (account_name, account, access_token) = resolve_zoho_context(profile.as_deref())?;
            let base_url = account.mail_api_url();

            let content = html.unwrap_or_else(|| body.unwrap_or_default());
            let mail_format = if content.contains('<') {
                "html".to_string()
            } else {
                "plaintext".to_string()
            };

            let uploaded = attachments
                .iter()
                .map(|path| {
                    let file_path = std::path::Path::new(path);
                    let file_name = file_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("attachment")
                        .to_string();
                    let file_bytes = std::fs::read(file_path)
                        .map_err(|e| ZocliError::Validation(format!("cannot read attachment {path}: {e}")))?;
                    upload_attachment(
                        &base_url,
                        &account.account_id,
                        &access_token,
                        &file_name,
                        file_bytes,
                    )
                })
                .collect::<Result<Vec<_>>>()?;

            let sent = send_mail_message(
                &base_url,
                &account.account_id,
                &access_token,
                MailSendRequest {
                    from_address: account.email.clone(),
                    to_address: to.clone(),
                    cc_address: cc.join(","),
                    bcc_address: bcc.join(","),
                    subject: subject.clone(),
                    content,
                    mail_format,
                    attachments: uploaded,
                },
            )?;

            ok_output(
                format,
                "mail.send",
                json!({
                    "account": account_name,
                    "to": to,
                    "subject": subject,
                    "sent": sent,
                }),
                render_mail_send_table(&account_name, &sent),
            )
        }
        MailCommand::Reply {
            profile,
            message_id,
            folder_id,
            body,
            cc,
            html,
        } => {
            let (account_name, account, access_token) = resolve_zoho_context(profile.as_deref())?;
            let base_url = account.mail_api_url();

            let content = html.unwrap_or_else(|| body.unwrap_or_default());
            let mail_format = if content.contains('<') {
                "html".to_string()
            } else {
                "plaintext".to_string()
            };

            let replied = reply_to_mail_message(
                &base_url,
                &account.account_id,
                &access_token,
                MailReplyRequest {
                    message_id: message_id.clone(),
                    content,
                    cc_address: cc.join(","),
                    mail_format,
                    from_address: None,
                },
            )?;

            ok_output(
                format,
                "mail.reply",
                json!({
                    "account": account_name,
                    "folder_id": folder_id,
                    "message_id": message_id,
                    "reply": replied,
                }),
                render_mail_reply_table(&account_name, &replied),
            )
        }
        MailCommand::Forward {
            profile,
            message_id,
            folder_id,
            to,
            body,
            cc,
            bcc,
            html,
        } => {
            let (account_name, account, access_token) = resolve_zoho_context(profile.as_deref())?;
            let base_url = account.mail_api_url();

            let content = html.unwrap_or_else(|| body.unwrap_or_default());

            let forwarded = forward_mail_message(
                &base_url,
                &account.account_id,
                &access_token,
                MailForwardRequest {
                    message_id: message_id.clone(),
                    folder_id: folder_id.clone(),
                    from_address: account.email.clone(),
                    to_address: to.clone(),
                    content,
                    cc_address: cc.join(","),
                    bcc_address: bcc.join(","),
                },
            )?;

            ok_output(
                format,
                "mail.forward",
                json!({
                    "account": account_name,
                    "folder_id": folder_id,
                    "message_id": message_id,
                    "to": to,
                    "forward": forwarded,
                }),
                render_mail_forward_table(&account_name, &forwarded),
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Guide
// ---------------------------------------------------------------------------

fn execute_guide(format: OutputFormat, topic: GuideTopicArg) -> Result<RenderedOutput> {
    let topic_name = guide_topic_name(topic);
    let commands = guide_commands(topic);
    let workflows = guide_workflows(topic);

    ok_output(
        format,
        "guide.show",
        json!({
            "topic": topic_name,
            "version": env!("CARGO_PKG_VERSION"),
            "commands": commands,
            "workflows": workflows,
        }),
        render_guide_table(topic_name, &commands, &workflows),
    )
}

// ---------------------------------------------------------------------------
// Output helpers
// ---------------------------------------------------------------------------

fn ok_output(
    format: OutputFormat,
    operation: &'static str,
    payload: serde_json::Value,
    table: String,
) -> Result<RenderedOutput> {
    let Some(mut object) = payload.as_object().cloned() else {
        return Err(ZocliError::Serialization(
            "expected object payload".to_string(),
        ));
    };
    object.insert("ok".to_string(), json!(true));
    object.insert("operation".to_string(), json!(operation));

    Ok(RenderedOutput {
        format,
        json: serde_json::Value::Object(object),
        table,
        exit_code: 0,
    })
}

fn render_key_value_table(items: &[(&str, String)]) -> String {
    items
        .iter()
        .map(|(key, value)| format!("{key}\t{value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn sanitize_table_cell(value: &str) -> String {
    let normalized = value.replace(['\r', '\n', '\t'], " ");
    let collapsed = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        "-".to_string()
    } else {
        collapsed
    }
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn derive_account_name_from_email(email: &str) -> String {
    let local = email.split('@').next().unwrap_or(email);
    let mut derived = String::with_capacity(local.len());
    let mut last_was_dash = false;

    for character in local.chars() {
        let normalized = match character {
            'a'..='z' | '0'..='9' => Some(character),
            'A'..='Z' => Some(character.to_ascii_lowercase()),
            '.' | '_' | '-' => Some(character),
            _ => Some('-'),
        };

        if let Some(value) = normalized {
            if value == '-' {
                if !last_was_dash {
                    derived.push(value);
                }
                last_was_dash = true;
            } else {
                derived.push(value);
                last_was_dash = false;
            }
        }
    }

    let trimmed = derived.trim_matches('-');
    if trimmed.is_empty() {
        "account".to_string()
    } else {
        trimmed.to_string()
    }
}

fn read_confirmation_code(authorization_url: &str) -> Result<String> {
    use std::io::Read as _;
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:9004").map_err(|err| {
        ZocliError::Io(format!(
            "failed to bind OAuth callback server at 127.0.0.1:9004: {err}"
        ))
    })?;

    // Open browser automatically
    eprintln!("Opening browser for authorization...");
    let open_result = std::process::Command::new("open")
        .arg(authorization_url)
        .spawn();
    if open_result.is_err() {
        // Fallback: print URL for manual opening
        eprintln!("Open this URL in your browser:\n{}", authorization_url);
    }

    eprintln!("Waiting for OAuth callback on http://127.0.0.1:9004/callback ...");

    let (mut stream, _) = listener.accept().map_err(|err| {
        ZocliError::Io(format!("failed to accept OAuth callback connection: {err}"))
    })?;

    let mut buf = [0u8; 4096];
    let n = stream
        .read(&mut buf)
        .map_err(|err| ZocliError::Io(format!("failed to read OAuth callback request: {err}")))?;
    let request = String::from_utf8_lossy(&buf[..n]);

    // Extract code from GET /callback?code=...&...
    let code = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|path| url::Url::parse(&format!("http://localhost{path}")).ok())
        .and_then(|url| {
            url.query_pairs()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.to_string())
        });

    // Send response to browser
    let (status, body) = match &code {
        Some(_) => (
            "200 OK",
            "<html><body style='font-family:system-ui;text-align:center;padding:60px'><h1>&#10004; Authorization successful</h1><p>You can close this tab.</p></body></html>",
        ),
        None => (
            "400 Bad Request",
            "<html><body style='font-family:system-ui;text-align:center;padding:60px'><h1>&#10008; Authorization failed</h1><p>No code received.</p></body></html>",
        ),
    };
    let response =
        format!("HTTP/1.1 {status}\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n{body}");
    let _ = io::Write::write_all(&mut stream, response.as_bytes());

    code.ok_or_else(|| {
        ZocliError::Auth("no authorization code received in OAuth callback".to_string())
    })
}

fn credential_state_label(state: &str) -> String {
    match state {
        "not_configured" => "Not configured".to_string(),
        "env_present" | "store_present" => "Connected".to_string(),
        "env_missing" | "store_missing" => "Not found".to_string(),
        "store_expired" => "Login required".to_string(),
        "store_mismatch" | "unsupported_reference" => "Config error".to_string(),
        _ => state.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Guide data
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct GuideCommandEntry {
    path: &'static str,
    topic: &'static str,
    summary: &'static str,
    requires_account: bool,
    examples: Vec<&'static str>,
}

#[derive(Serialize)]
struct GuideWorkflowEntry {
    id: &'static str,
    topic: &'static str,
    title: &'static str,
    summary: &'static str,
    steps: Vec<&'static str>,
}

fn guide_topic_name(topic: GuideTopicArg) -> &'static str {
    match topic {
        GuideTopicArg::All => "all",
        GuideTopicArg::Account => "account",
        GuideTopicArg::Auth => "auth",
        GuideTopicArg::Mail => "mail",
        GuideTopicArg::Calendar => "calendar",
        GuideTopicArg::Drive => "drive",
    }
}

fn guide_commands(topic: GuideTopicArg) -> Vec<GuideCommandEntry> {
    all_guide_commands()
        .into_iter()
        .filter(|entry| topic == GuideTopicArg::All || guide_topic_name(topic) == entry.topic)
        .collect()
}

fn guide_workflows(topic: GuideTopicArg) -> Vec<GuideWorkflowEntry> {
    all_guide_workflows()
        .into_iter()
        .filter(|entry| topic == GuideTopicArg::All || guide_topic_name(topic) == entry.topic)
        .collect()
}

fn all_guide_commands() -> Vec<GuideCommandEntry> {
    vec![
        GuideCommandEntry {
            path: "add",
            topic: "account",
            summary: "Add a Zoho account and set it as current.",
            requires_account: false,
            examples: vec![
                "zocli add user@zoho.com",
                "zocli add user@company.com --datacenter eu",
                "zocli add user@zoho.com work",
            ],
        },
        GuideCommandEntry {
            path: "accounts",
            topic: "account",
            summary: "Show all configured accounts.",
            requires_account: false,
            examples: vec!["zocli accounts"],
        },
        GuideCommandEntry {
            path: "use",
            topic: "account",
            summary: "Set an account as current for subsequent commands.",
            requires_account: false,
            examples: vec!["zocli use work"],
        },
        GuideCommandEntry {
            path: "whoami",
            topic: "account",
            summary: "Show the current active account.",
            requires_account: false,
            examples: vec!["zocli whoami"],
        },
        GuideCommandEntry {
            path: "update",
            topic: "account",
            summary: "Check for a new release or update the installed zocli binary.",
            requires_account: false,
            examples: vec!["zocli update --check", "zocli update"],
        },
        GuideCommandEntry {
            path: "status",
            topic: "auth",
            summary: "Show the auth status of the current account.",
            requires_account: true,
            examples: vec!["zocli status"],
        },
        GuideCommandEntry {
            path: "login",
            topic: "auth",
            summary: "Authenticate with Zoho OAuth2 for Mail, Calendar, and WorkDrive.",
            requires_account: true,
            examples: vec!["zocli login", "zocli login mail", "zocli login drive"],
        },
        GuideCommandEntry {
            path: "logout",
            topic: "auth",
            summary: "Revoke credentials for all or a specific service.",
            requires_account: true,
            examples: vec!["zocli logout", "zocli logout mail"],
        },
        GuideCommandEntry {
            path: "mail folders",
            topic: "mail",
            summary: "List available mail folders.",
            requires_account: true,
            examples: vec!["zocli mail folders"],
        },
        GuideCommandEntry {
            path: "mail list",
            topic: "mail",
            summary: "List messages in a folder. Defaults to Inbox.",
            requires_account: true,
            examples: vec!["zocli mail list --limit 10"],
        },
        GuideCommandEntry {
            path: "mail search",
            topic: "mail",
            summary: "Search messages across the mailbox.",
            requires_account: true,
            examples: vec!["zocli mail search \"invoice\""],
        },
        GuideCommandEntry {
            path: "mail read",
            topic: "mail",
            summary: "Read a message by its ID.",
            requires_account: true,
            examples: vec!["zocli mail read <message-id> --folder-id <folder-id>"],
        },
        GuideCommandEntry {
            path: "mail send",
            topic: "mail",
            summary: "Send a new message via Zoho Mail API.",
            requires_account: true,
            examples: vec!["zocli mail send person@example.com \"Subject\" \"Body text\""],
        },
        GuideCommandEntry {
            path: "mail reply",
            topic: "mail",
            summary: "Reply to a message by its ID.",
            requires_account: true,
            examples: vec!["zocli mail reply <message-id> --folder-id <folder-id> \"Reply body\""],
        },
        GuideCommandEntry {
            path: "mail forward",
            topic: "mail",
            summary: "Forward a message to another recipient.",
            requires_account: true,
            examples: vec![
                "zocli mail forward <message-id> --folder-id <folder-id> person@example.com",
            ],
        },
        GuideCommandEntry {
            path: "calendar calendars",
            topic: "calendar",
            summary: "List available calendars.",
            requires_account: true,
            examples: vec!["zocli calendar calendars"],
        },
        GuideCommandEntry {
            path: "calendar events",
            topic: "calendar",
            summary: "List events in a date range. Defaults to next 30 days.",
            requires_account: true,
            examples: vec![
                "zocli calendar events",
                "zocli calendar events 2026-03-12 2026-03-19 --limit 20",
            ],
        },
        GuideCommandEntry {
            path: "calendar create",
            topic: "calendar",
            summary: "Create a new calendar event.",
            requires_account: true,
            examples: vec![
                "zocli calendar create \"Meeting\" 2026-03-12T09:00:00Z 2026-03-12T10:00:00Z",
            ],
        },
        GuideCommandEntry {
            path: "calendar delete",
            topic: "calendar",
            summary: "Delete a calendar event by UID.",
            requires_account: true,
            examples: vec!["zocli calendar delete <event-uid>"],
        },
        GuideCommandEntry {
            path: "drive list",
            topic: "drive",
            summary: "List files in a WorkDrive folder. Without folder_id, lists teams.",
            requires_account: true,
            examples: vec![
                "zocli drive list",
                "zocli drive list <folder-id> --limit 50",
            ],
        },
        GuideCommandEntry {
            path: "drive info",
            topic: "drive",
            summary: "Show WorkDrive team info (storage quotas).",
            requires_account: true,
            examples: vec!["zocli drive info"],
        },
        GuideCommandEntry {
            path: "drive upload",
            topic: "drive",
            summary: "Upload a local file to a WorkDrive folder.",
            requires_account: true,
            examples: vec!["zocli drive upload ./report.pdf <folder-id>"],
        },
        GuideCommandEntry {
            path: "drive download",
            topic: "drive",
            summary: "Download a WorkDrive file to a local path.",
            requires_account: true,
            examples: vec!["zocli drive download <file-id> --output ./report.pdf"],
        },
    ]
}

fn all_guide_workflows() -> Vec<GuideWorkflowEntry> {
    vec![
        GuideWorkflowEntry {
            id: "mail_read_flow",
            topic: "mail",
            title: "Read a message from Zoho Mail",
            summary: "Full flow from account setup to reading a message by ID.",
            steps: vec![
                "zocli add user@zoho.com",
                "zocli login",
                "zocli mail list --limit 10",
                "zocli mail read <message-id> --folder-id <folder-id>",
            ],
        },
        GuideWorkflowEntry {
            id: "mail_search_flow",
            topic: "mail",
            title: "Search for a message",
            summary: "Flow from login to searching and reading a message.",
            steps: vec![
                "zocli login",
                "zocli mail search \"invoice\"",
                "zocli mail read <message-id> --folder-id <folder-id>",
            ],
        },
        GuideWorkflowEntry {
            id: "mail_reply_flow",
            topic: "mail",
            title: "Reply to a message",
            summary: "Flow from finding a message to sending a reply.",
            steps: vec![
                "zocli login",
                "zocli mail search \"invoice\"",
                "zocli mail reply <message-id> --folder-id <folder-id> \"Got it, thanks\"",
            ],
        },
        GuideWorkflowEntry {
            id: "mail_forward_flow",
            topic: "mail",
            title: "Forward a message",
            summary: "Flow from finding a message to forwarding it.",
            steps: vec![
                "zocli login",
                "zocli mail search \"invoice\"",
                "zocli mail forward <message-id> --folder-id <folder-id> person@example.com \"FYI\"",
            ],
        },
        GuideWorkflowEntry {
            id: "mail_send_flow",
            topic: "mail",
            title: "Send a new message",
            summary: "Flow from login to composing and sending a new message.",
            steps: vec![
                "zocli login",
                "zocli mail send person@example.com \"Meeting\" \"See you at 3pm\"",
            ],
        },
        GuideWorkflowEntry {
            id: "calendar_read_flow",
            topic: "calendar",
            title: "Browse calendars and events",
            summary: "Flow from login to listing calendars and events.",
            steps: vec![
                "zocli login",
                "zocli calendar calendars",
                "zocli calendar events",
            ],
        },
        GuideWorkflowEntry {
            id: "calendar_write_flow",
            topic: "calendar",
            title: "Create and delete an event",
            summary: "Flow from login to creating and removing a calendar event.",
            steps: vec![
                "zocli login",
                "zocli calendar create \"Meeting\" 2026-03-12T09:00:00Z 2026-03-12T10:00:00Z",
                "zocli calendar delete <event-uid>",
            ],
        },
        GuideWorkflowEntry {
            id: "drive_browse_flow",
            topic: "drive",
            title: "Browse WorkDrive",
            summary: "Flow from login to listing teams and folder contents.",
            steps: vec![
                "zocli login",
                "zocli drive info",
                "zocli drive list <folder-id>",
            ],
        },
        GuideWorkflowEntry {
            id: "drive_upload_flow",
            topic: "drive",
            title: "Upload a file to WorkDrive",
            summary: "Flow from login to uploading a local file.",
            steps: vec!["zocli login", "zocli drive upload ./report.pdf <folder-id>"],
        },
        GuideWorkflowEntry {
            id: "drive_download_flow",
            topic: "drive",
            title: "Download a file from WorkDrive",
            summary: "Flow from login to downloading a file to disk.",
            steps: vec![
                "zocli login",
                "zocli drive download <file-id> --output ./report.pdf",
            ],
        },
        GuideWorkflowEntry {
            id: "multi_account_flow",
            topic: "mail",
            title: "Work with multiple accounts",
            summary: "Independent access to multiple mailboxes via named accounts.",
            steps: vec![
                "zocli add personal@zoho.com",
                "zocli add work@zoho.com",
                "zocli login",
                "zocli use work",
                "zocli login",
                "zocli use personal",
                "zocli mail list --limit 5",
                "zocli use work",
                "zocli mail list --limit 5",
            ],
        },
    ]
}

// ---------------------------------------------------------------------------
// Table renderers — Account
// ---------------------------------------------------------------------------

fn render_account_table(name: &str, account: &AccountConfig, current: bool) -> String {
    render_key_value_table(&[
        ("account", name.to_string()),
        ("email", account.email.clone()),
        ("current", current.to_string()),
        ("datacenter", account.datacenter.clone()),
        (
            "account_id",
            display_account_identity(account.account_id.as_str(), "auto-discover after login"),
        ),
        (
            "org_id",
            account.org_id.as_deref().unwrap_or("-").to_string(),
        ),
        (
            "client_id",
            display_account_identity(account.client_id.as_str(), "shared/default OAuth app"),
        ),
        (
            "credential_ref",
            account.credential_ref.as_deref().unwrap_or("-").to_string(),
        ),
    ])
}

fn display_account_identity(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn render_account_validation_table(name: &str, errors: &[String]) -> String {
    if errors.is_empty() {
        return render_key_value_table(&[
            ("account", name.to_string()),
            ("valid", "true".to_string()),
        ]);
    }

    let mut lines = vec![
        format!("account\t{name}"),
        "valid\tfalse".to_string(),
        "errors".to_string(),
    ];
    lines.extend(errors.iter().map(|error| format!("- {error}")));
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Table renderers — Guide
// ---------------------------------------------------------------------------

fn render_guide_table(
    topic: &str,
    commands: &[GuideCommandEntry],
    workflows: &[GuideWorkflowEntry],
) -> String {
    let mut lines = vec![
        format!("topic\t{topic}"),
        format!("version\t{}", env!("CARGO_PKG_VERSION")),
        "commands".to_string(),
        "PATH\tTOPIC\tREQUIRES_ACCOUNT\tSUMMARY".to_string(),
    ];
    lines.extend(commands.iter().map(|command| {
        format!(
            "{}\t{}\t{}\t{}",
            command.path, command.topic, command.requires_account, command.summary
        )
    }));

    lines.push("workflows".to_string());
    lines.push("ID\tTOPIC\tTITLE\tSUMMARY".to_string());
    lines.extend(workflows.iter().map(|workflow| {
        format!(
            "{}\t{}\t{}\t{}",
            workflow.id, workflow.topic, workflow.title, workflow.summary
        )
    }));

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Table renderers — Mail
// ---------------------------------------------------------------------------

fn render_mail_folders_table(account: &str, folders: &[MailFolder]) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("count\t{}", folders.len()),
        "FOLDER_ID\tNAME\tMESSAGES\tUNREAD".to_string(),
    ];
    lines.extend(folders.iter().map(|folder| {
        format!(
            "{}\t{}\t{}\t{}",
            sanitize_table_cell(&folder.folder_id),
            sanitize_table_cell(&folder.folder_name),
            folder.message_count,
            folder.unread_count,
        )
    }));
    lines.join("\n")
}

fn render_mail_list_table(account: &str, messages: &[MailMessageSummary]) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("count\t{}", messages.len()),
        "MESSAGE_ID\tFROM\tSUBJECT\tRECEIVED\tREAD\tATTACH".to_string(),
    ];
    lines.extend(messages.iter().map(|message| {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            sanitize_table_cell(&message.message_id),
            sanitize_table_cell(&message.sender),
            sanitize_table_cell(&message.subject),
            sanitize_table_cell(&message.received_time),
            message.is_read,
            message.has_attachment,
        )
    }));
    lines.join("\n")
}

fn render_mail_search_table(account: &str, query: &str, messages: &[MailMessageSummary]) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("query\t{query}"),
        format!("count\t{}", messages.len()),
        "MESSAGE_ID\tFROM\tSUBJECT\tRECEIVED\tREAD".to_string(),
    ];
    lines.extend(messages.iter().map(|message| {
        format!(
            "{}\t{}\t{}\t{}\t{}",
            sanitize_table_cell(&message.message_id),
            sanitize_table_cell(&message.sender),
            sanitize_table_cell(&message.subject),
            sanitize_table_cell(&message.received_time),
            message.is_read,
        )
    }));
    lines.join("\n")
}

fn render_mail_read_table(account: &str, message: &MailMessage) -> String {
    let lines = vec![
        format!("account\t{account}"),
        format!("message_id\t{}", message.message_id),
        format!("folder_id\t{}", message.folder_id),
        format!("from\t{}", message.sender),
        format!("to\t{}", message.to_address),
        format!("subject\t{}", message.subject),
        format!("received\t{}", message.received_time),
        "content".to_string(),
        message.content.trim_end().to_string(),
    ];
    lines.join("\n")
}

fn render_mail_send_table(account: &str, sent: &SentMail) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("message_id", sent.message_id.clone()),
    ])
}

fn render_mail_reply_table(account: &str, replied: &RepliedMail) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("message_id", replied.message_id.clone()),
    ])
}

fn render_mail_forward_table(account: &str, forwarded: &ForwardedMail) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("message_id", forwarded.message_id.clone()),
    ])
}

// ---------------------------------------------------------------------------
// Table renderers — Calendar
// ---------------------------------------------------------------------------

fn calendar_event_json(event: &CalendarEvent) -> serde_json::Value {
    json!({
        "uid": event.uid,
        "title": event.title,
        "start": event.start,
        "end": event.end,
        "location": event.location,
        "description": event.description,
        "etag": event.etag,
    })
}

fn render_calendar_collections_table(account: &str, calendars: &[CalendarCollection]) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("count\t{}", calendars.len()),
        "ID\tNAME\tDESCRIPTION".to_string(),
    ];
    lines.extend(calendars.iter().map(|calendar| {
        format!(
            "{}\t{}\t{}",
            calendar.id,
            calendar.name,
            calendar.description.as_deref().unwrap_or("-")
        )
    }));
    lines.join("\n")
}

fn render_calendar_events_table(
    account: &str,
    calendar: &CalendarCollection,
    window: &CalendarEventWindow,
    events: &[CalendarEvent],
) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("calendar.id\t{}", calendar.id),
        format!("calendar.name\t{}", calendar.name),
        format!("window.from\t{}", window.from),
        format!("window.to\t{}", window.to),
        format!("count\t{}", events.len()),
        "UID\tSTART\tEND\tTITLE\tLOCATION".to_string(),
    ];
    lines.extend(events.iter().map(|event| {
        format!(
            "{}\t{}\t{}\t{}\t{}",
            event.uid,
            event.start,
            event.end,
            event.title,
            event.location.as_deref().unwrap_or("-"),
        )
    }));
    lines.join("\n")
}

fn render_calendar_create_table(
    account: &str,
    calendar: &CalendarCollection,
    event: &CalendarEvent,
) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("calendar.id", calendar.id.clone()),
        ("calendar.name", calendar.name.clone()),
        ("event.uid", event.uid.clone()),
        ("event.title", event.title.clone()),
        ("event.start", event.start.clone()),
        ("event.end", event.end.clone()),
        (
            "event.location",
            event.location.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "event.description",
            event.description.clone().unwrap_or_else(|| "-".to_string()),
        ),
    ])
}

fn render_calendar_delete_table(
    account: &str,
    calendar: &CalendarCollection,
    event: &CalendarEvent,
) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("calendar.id", calendar.id.clone()),
        ("calendar.name", calendar.name.clone()),
        ("deleted.uid", event.uid.clone()),
        ("deleted.title", event.title.clone()),
        ("deleted.start", event.start.clone()),
        ("deleted.end", event.end.clone()),
    ])
}

// ---------------------------------------------------------------------------
// Table renderers — Drive
// ---------------------------------------------------------------------------

fn render_drive_teams_table(account: &str, teams: &[DriveTeam]) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("count\t{}", teams.len()),
        "ID\tNAME\tSTORAGE_USED\tSTORAGE_LIMIT".to_string(),
    ];
    lines.extend(teams.iter().map(|team| {
        format!(
            "{}\t{}\t{}\t{}",
            team.id, team.name, team.storage_used, team.storage_limit,
        )
    }));
    lines.join("\n")
}

fn render_drive_files_table(account: &str, files: &[DriveFile]) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("count\t{}", files.len()),
        "ID\tNAME\tTYPE\tSIZE\tMODIFIED".to_string(),
    ];
    lines.extend(files.iter().map(|file| {
        format!(
            "{}\t{}\t{}\t{}\t{}",
            file.id, file.name, file.file_type, file.size, file.modified_time,
        )
    }));
    lines.join("\n")
}

fn render_drive_upload_table(account: &str, uploaded: &UploadedFile) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("uploaded.id", uploaded.id.clone()),
        ("uploaded.name", uploaded.name.clone()),
    ])
}

fn render_drive_download_table(account: &str, downloaded: &DownloadedFile) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("output.path", downloaded.path.display().to_string()),
        ("bytes_written", downloaded.size.to_string()),
    ])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::sanitize_table_cell;

    #[test]
    fn sanitize_table_cell_collapses_newlines_tabs_and_empty_values() {
        assert_eq!(sanitize_table_cell("one\ttwo\nthree"), "one two three");
        assert_eq!(sanitize_table_cell("   \r\n\t  "), "-");
    }
}
