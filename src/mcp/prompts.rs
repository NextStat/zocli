use chrono::{Days, Utc};
use serde_json::{Value, json};

use super::skills;
use crate::account_store::AccountStore;
use crate::error::{Result, ZocliError};

struct PromptArgument {
    name: &'static str,
    description: &'static str,
    required: bool,
}

struct PromptDefinition {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    arguments: &'static [PromptArgument],
}

const SHARED_ARGUMENTS: &[PromptArgument] = &[
    PromptArgument {
        name: "goal",
        description: "What you want to achieve with zocli. Что вы хотите сделать через zocli.",
        required: false,
    },
    PromptArgument {
        name: "account",
        description: "Optional zocli account alias to prefer. Необязательный alias аккаунта zocli.",
        required: false,
    },
];

const MAIL_ARGUMENTS: &[PromptArgument] = &[
    PromptArgument {
        name: "request",
        description: "What mail task to perform. Какую почтовую задачу нужно выполнить.",
        required: true,
    },
    PromptArgument {
        name: "account",
        description: "Optional zocli account alias. Необязательный alias аккаунта zocli.",
        required: false,
    },
    PromptArgument {
        name: "folder",
        description: "Mailbox folder, defaults to INBOX when omitted. Папка почты; если не указана, используется INBOX.",
        required: false,
    },
];

const CALENDAR_ARGUMENTS: &[PromptArgument] = &[
    PromptArgument {
        name: "request",
        description: "What calendar task to perform. Какую задачу по календарю нужно выполнить.",
        required: true,
    },
    PromptArgument {
        name: "account",
        description: "Optional zocli account alias. Необязательный alias аккаунта zocli.",
        required: false,
    },
    PromptArgument {
        name: "calendar",
        description: "Calendar name, defaults to default. Имя календаря; по умолчанию используется default.",
        required: false,
    },
    PromptArgument {
        name: "from",
        description: "Optional RFC3339 or YYYY-MM-DD start boundary. Необязательная начальная граница RFC3339 или YYYY-MM-DD.",
        required: false,
    },
    PromptArgument {
        name: "to",
        description: "Optional RFC3339 or YYYY-MM-DD end boundary. Необязательная конечная граница RFC3339 или YYYY-MM-DD.",
        required: false,
    },
];

const DRIVE_ARGUMENTS: &[PromptArgument] = &[
    PromptArgument {
        name: "request",
        description: "What WorkDrive task to perform. Какую задачу по WorkDrive нужно выполнить.",
        required: true,
    },
    PromptArgument {
        name: "account",
        description: "Optional zocli account alias. Необязательный alias аккаунта zocli.",
        required: false,
    },
    PromptArgument {
        name: "folder_id",
        description: "Optional WorkDrive folder ID to focus on. Необязательный folder ID WorkDrive для фокуса.",
        required: false,
    },
];

const DAILY_BRIEFING_ARGUMENTS: &[PromptArgument] = &[
    PromptArgument {
        name: "account",
        description: "Optional zocli account alias. Необязательный alias аккаунта zocli.",
        required: false,
    },
    PromptArgument {
        name: "from",
        description: "Optional start date for the schedule window. Необязательная дата начала окна.",
        required: false,
    },
    PromptArgument {
        name: "to",
        description: "Optional end date for the schedule window. Необязательная дата конца окна.",
        required: false,
    },
    PromptArgument {
        name: "mail_limit",
        description: "How many recent inbox messages to inspect. Сколько последних писем из inbox нужно посмотреть.",
        required: false,
    },
];

const FIND_AND_READ_ARGUMENTS: &[PromptArgument] = &[
    PromptArgument {
        name: "query",
        description: "Search phrase to locate the email. Поисковая фраза для поиска письма.",
        required: true,
    },
    PromptArgument {
        name: "account",
        description: "Optional zocli account alias. Необязательный alias аккаунта zocli.",
        required: false,
    },
    PromptArgument {
        name: "limit",
        description: "How many matching messages to inspect. Сколько совпавших писем нужно проверить.",
        required: false,
    },
];

const REPLY_WITH_CONTEXT_ARGUMENTS: &[PromptArgument] = &[
    PromptArgument {
        name: "message_id",
        description: "The message ID to read and respond to. Идентификатор письма, которое нужно прочитать и обработать.",
        required: true,
    },
    PromptArgument {
        name: "account",
        description: "Optional zocli account alias. Необязательный alias аккаунта zocli.",
        required: false,
    },
    PromptArgument {
        name: "folder_id",
        description: "Folder ID of the message when already known. Folder ID письма, если он уже известен.",
        required: false,
    },
    PromptArgument {
        name: "from",
        description: "Optional schedule start boundary to inspect. Необязательная начальная граница расписания.",
        required: false,
    },
    PromptArgument {
        name: "to",
        description: "Optional schedule end boundary to inspect. Необязательная конечная граница расписания.",
        required: false,
    },
];

const PROMPTS: &[PromptDefinition] = &[
    PromptDefinition {
        name: "shared",
        title: "zocli shared context",
        description: "Understand the account, auth state, and base MCP context before using mail, calendar, or WorkDrive tools. Общий контекст аккаунта, авторизации и MCP перед работой с mail, calendar и WorkDrive.",
        arguments: SHARED_ARGUMENTS,
    },
    PromptDefinition {
        name: "mail",
        title: "zocli mail workflow",
        description: "Work with Zoho Mail through MCP: folders, search, read, send, reply, and forward. Работа с Zoho Mail через MCP: папки, поиск, чтение, отправка, reply и forward.",
        arguments: MAIL_ARGUMENTS,
    },
    PromptDefinition {
        name: "calendar",
        title: "zocli calendar workflow",
        description: "Work with Zoho Calendar through MCP: calendars, upcoming events, event creation, and event deletion. Работа с Zoho Calendar через MCP: календари, события, создание и удаление событий.",
        arguments: CALENDAR_ARGUMENTS,
    },
    PromptDefinition {
        name: "drive",
        title: "zocli drive workflow",
        description: "Work with Zoho WorkDrive through MCP: teams, folders, uploads, and downloads. Работа с Zoho WorkDrive через MCP: teams, папки, загрузки и скачивания.",
        arguments: DRIVE_ARGUMENTS,
    },
    PromptDefinition {
        name: "daily-briefing",
        title: "Mail and calendar briefing",
        description: "Build a short summary of important mail, upcoming events, deadlines, and actions that need attention. Собери короткую сводку по важным письмам, событиям и действиям.",
        arguments: DAILY_BRIEFING_ARGUMENTS,
    },
    PromptDefinition {
        name: "find-and-read",
        title: "Find and read a message",
        description: "Search for a message, choose the best result, and read its contents. Найди письмо, выбери лучший результат и прочитай содержимое.",
        arguments: FIND_AND_READ_ARGUMENTS,
    },
    PromptDefinition {
        name: "reply-with-context",
        title: "Reply with calendar context",
        description: "Read a message, inspect the schedule, and draft or send a reply with the right calendar context. Прочитай письмо, проверь расписание и подготовь ответ с правильным календарным контекстом.",
        arguments: REPLY_WITH_CONTEXT_ARGUMENTS,
    },
];

pub fn prompt_names() -> Vec<&'static str> {
    PROMPTS.iter().map(|prompt| prompt.name).collect()
}

pub fn prompt_definitions() -> Vec<Value> {
    PROMPTS
        .iter()
        .map(|prompt| {
            json!({
                "name": prompt.name,
                "title": prompt.title,
                "description": prompt.description,
                "arguments": prompt.arguments.iter().map(argument_json).collect::<Vec<_>>(),
            })
        })
        .collect()
}

pub fn get_prompt(params: Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ZocliError::Validation("prompts/get requires `name`".to_string()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));

    let prompt = PROMPTS
        .iter()
        .find(|prompt| prompt.name == name)
        .ok_or_else(|| ZocliError::Validation(format!("unknown MCP prompt: {name}")))?;

    Ok(json!({
        "description": prompt.description,
        "messages": prompt_messages(name, &arguments)?,
    }))
}

pub fn complete(params: Value) -> Result<Value> {
    let reference = params
        .get("ref")
        .ok_or_else(|| ZocliError::Validation("completion/complete requires `ref`".to_string()))?;
    let argument = params.get("argument").ok_or_else(|| {
        ZocliError::Validation("completion/complete requires `argument`".to_string())
    })?;
    let argument_name = argument
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ZocliError::Validation("completion/complete requires `argument.name`".to_string())
        })?;
    let argument_value = argument
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let context_arguments = params
        .get("context")
        .and_then(|context| context.get("arguments"));

    let suggestions = match reference.get("type").and_then(Value::as_str) {
        Some("ref/prompt") => complete_prompt_reference(
            reference
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ZocliError::Validation(
                        "prompt completion requires `ref.name` for ref/prompt".to_string(),
                    )
                })?,
            argument_name,
            argument_value,
            context_arguments,
        )?,
        Some("ref/resource") => complete_resource_reference(
            reference
                .get("uri")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ZocliError::Validation(
                        "resource completion requires `ref.uri` for ref/resource".to_string(),
                    )
                })?,
            argument_name,
            argument_value,
        )?,
        Some(other) => {
            return Err(ZocliError::Validation(format!(
                "unsupported completion reference type: {other}"
            )));
        }
        None => {
            return Err(ZocliError::Validation(
                "completion/complete requires `ref.type`".to_string(),
            ));
        }
    };

    let total = suggestions.len();
    let values = suggestions.into_iter().take(100).collect::<Vec<_>>();
    Ok(json!({
        "completion": {
            "values": values,
            "total": total,
            "hasMore": total > 100
        }
    }))
}

fn argument_json(argument: &PromptArgument) -> Value {
    json!({
        "name": argument.name,
        "description": argument.description,
        "required": argument.required,
    })
}

fn prompt_messages(name: &str, arguments: &Value) -> Result<Vec<Value>> {
    let text = match name {
        "shared" => render_shared_prompt(arguments),
        "mail" => render_mail_prompt(arguments)?,
        "calendar" => render_calendar_prompt(arguments)?,
        "drive" => render_drive_prompt(arguments)?,
        "daily-briefing" => render_daily_briefing_prompt(arguments),
        "find-and-read" => render_find_and_read_prompt(arguments)?,
        "reply-with-context" => render_reply_with_context_prompt(arguments)?,
        _ => {
            return Err(ZocliError::Validation(format!(
                "unknown MCP prompt: {name}"
            )));
        }
    };

    Ok(vec![json!({
        "role": "user",
        "content": {
            "type": "text",
            "text": enrich_with_canonical_skill(name, &text),
        }
    })])
}

fn complete_prompt_reference(
    name: &str,
    argument_name: &str,
    current_value: &str,
    context_arguments: Option<&Value>,
) -> Result<Vec<String>> {
    let suggestions = match (name, argument_name) {
        (_, "account") => configured_accounts()?,
        ("mail", "folder") | ("find-and-read", "folder") | ("reply-with-context", "folder") => {
            ["INBOX", "Sent", "Drafts", "Archive", "Trash", "Spam"]
                .into_iter()
                .map(str::to_string)
                .collect()
        }
        ("calendar", "calendar") => {
            vec!["default".to_string()]
        }
        ("calendar", "from")
        | ("calendar", "to")
        | ("daily-briefing", "from")
        | ("daily-briefing", "to")
        | ("reply-with-context", "from")
        | ("reply-with-context", "to") => date_suggestions(),
        ("daily-briefing", "mail_limit") | ("find-and-read", "limit") => {
            vec!["5", "10", "20", "50"]
                .into_iter()
                .map(str::to_string)
                .collect()
        }
        ("reply-with-context", "message_id") => context_arguments
            .and_then(|arguments| arguments.get("message_id"))
            .and_then(Value::as_str)
            .map(|message_id| vec![message_id.to_string()])
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    Ok(rank_and_filter(suggestions, current_value))
}

fn complete_resource_reference(
    uri: &str,
    argument_name: &str,
    current_value: &str,
) -> Result<Vec<String>> {
    let suggestions = match argument_name {
        "account"
            if uri == "resource://zocli/account/{account}"
                || uri == "resource://zocli/auth/{account}"
                || uri.starts_with("ui://zocli/dashboard") =>
        {
            configured_accounts()?
        }
        "skill" if uri == "resource://zocli/skill/{skill}" => skills::skill_names()
            .into_iter()
            .map(str::to_string)
            .collect(),
        "section" if uri.starts_with("ui://zocli/dashboard") => ["tools", "resources", "auth"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        "resource" if uri.starts_with("ui://zocli/dashboard") => {
            ["account", "auth", "skills", "skill"]
                .into_iter()
                .map(str::to_string)
                .collect()
        }
        "skill" if uri.starts_with("ui://zocli/dashboard") => skills::skill_names()
            .into_iter()
            .map(str::to_string)
            .collect(),
        "tool" if uri.starts_with("ui://zocli/dashboard") => vec![
            "zocli.app.snapshot".to_string(),
            "zocli.account.list".to_string(),
            "zocli.account.current".to_string(),
            "zocli.auth.status".to_string(),
            "zocli.update.check".to_string(),
        ],
        "prompt" if uri.starts_with("ui://zocli/dashboard") => {
            prompt_names().into_iter().map(str::to_string).collect()
        }
        _ => Vec::new(),
    };

    Ok(rank_and_filter(suggestions, current_value))
}

fn configured_accounts() -> Result<Vec<String>> {
    let store = AccountStore::load()?;
    let mut accounts = store.summaries().into_keys().collect::<Vec<_>>();
    accounts.sort();
    Ok(accounts)
}

fn date_suggestions() -> Vec<String> {
    let today = Utc::now().date_naive();
    let tomorrow = today.checked_add_days(Days::new(1)).unwrap_or(today);
    let next_week = today.checked_add_days(Days::new(7)).unwrap_or(today);
    vec![
        today.format("%Y-%m-%d").to_string(),
        tomorrow.format("%Y-%m-%d").to_string(),
        next_week.format("%Y-%m-%d").to_string(),
    ]
}

fn rank_and_filter(candidates: Vec<String>, current_value: &str) -> Vec<String> {
    let needle = current_value.trim().to_ascii_lowercase();
    let mut prefix = Vec::new();
    let mut contains = Vec::new();
    let mut rest = Vec::new();

    for candidate in dedupe_preserve_order(candidates) {
        let haystack = candidate.to_ascii_lowercase();
        if needle.is_empty() {
            rest.push(candidate);
        } else if haystack.starts_with(&needle) {
            prefix.push(candidate);
        } else if haystack.contains(&needle) {
            contains.push(candidate);
        }
    }

    prefix.extend(contains);
    if needle.is_empty() {
        rest.sort();
        rest
    } else {
        prefix
    }
}

fn dedupe_preserve_order(candidates: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.clone()))
        .collect()
}

fn enrich_with_canonical_skill(prompt_name: &str, body: &str) -> String {
    let Some(skill_name) = skills::prompt_skill_name(prompt_name) else {
        return body.to_string();
    };
    format!(
        "{body}\n\nCanonical embedded skill resource: `resource://zocli/skill/{skill_name}`.\nRead it with `resources/read` if you need the exact SKILL.md workflow text mirrored from Claude Code."
    )
}

fn render_shared_prompt(arguments: &Value) -> String {
    let goal = optional_string(arguments, "goal").unwrap_or(
        "Determine the right zocli account, verify auth, and choose the correct service surface before taking action.",
    );
    let account = optional_string(arguments, "account");

    format!(
        "You are working with the zocli MCP server.\n\
Goal: {goal}\n\
Preferred account: {}\n\
\n\
Use this order of work:\n\
1. Determine the account through `zocli.account.current` or `zocli.account.list`.\n\
2. Check auth state through `zocli.auth.status`.\n\
3. If you need structured account context, read `resource://zocli/account/{{account}}` and `resource://zocli/auth/{{account}}`.\n\
4. Only then move to mail, calendar, or drive tools.\n\
\n\
If the required account does not exist or a service is not authenticated, state that gap explicitly before proceeding.",
        account.unwrap_or("current")
    )
}

fn render_mail_prompt(arguments: &Value) -> Result<String> {
    let request = required_string(arguments, "request")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let folder = optional_string(arguments, "folder").unwrap_or("INBOX");

    Ok(format!(
        "Help with Zoho Mail through the zocli MCP server.\n\
Task: {request}\n\
Account: {account}\n\
Mailbox hint: {folder}\n\
\n\
Use this workflow:\n\
1. Confirm the account and auth state when needed through `zocli.account.current` and `zocli.auth.status`.\n\
2. If the right folder is unclear, call `zocli.mail.folders` first.\n\
3. Use `zocli.mail.list` for quick context and `zocli.mail.search` when the user provides a keyword.\n\
4. For a specific message, use `zocli.mail.read` with both `folder_id` and `message_id`.\n\
5. For write actions, use `zocli.mail.send`, `zocli.mail.reply`, or `zocli.mail.forward` with the smallest sufficient payload.\n\
6. In the final answer, record the sender, subject, relevant dates, and the exact result of any write action.\n\
\n\
Do not imply attachment export or invite import support because those tools are not part of the current stable MCP surface."
    ))
}

fn render_calendar_prompt(arguments: &Value) -> Result<String> {
    let request = required_string(arguments, "request")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let calendar = optional_string(arguments, "calendar").unwrap_or("default");
    let from = optional_string(arguments, "from").unwrap_or("auto");
    let to = optional_string(arguments, "to").unwrap_or("auto");

    Ok(format!(
        "Help with Zoho Calendar through the zocli MCP server.\n\
Task: {request}\n\
Account: {account}\n\
Calendar: {calendar}\n\
Window: from={from}, to={to}\n\
\n\
Use this workflow:\n\
1. Confirm the account and auth state when needed.\n\
2. If the right calendar is unclear, call `zocli.calendar.calendars` first.\n\
3. Use `zocli.calendar.events` with the narrowest useful time window.\n\
4. For write actions, use `zocli.calendar.create` or `zocli.calendar.delete` with explicit calendar IDs, timestamps, or event UIDs.\n\
5. Return a short summary of the schedule or the write result, including title, time, location, and visible conflicts.\n\
\n\
Editing an existing event in place is not part of the current stable MCP surface. Say that explicitly if the user asks for it."
    ))
}

fn render_drive_prompt(arguments: &Value) -> Result<String> {
    let request = required_string(arguments, "request")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let folder_id = optional_string(arguments, "folder_id").unwrap_or("auto");

    Ok(format!(
        "Help with Zoho WorkDrive through the zocli MCP server.\n\
Task: {request}\n\
Account: {account}\n\
Folder hint: {folder_id}\n\
\n\
Use this workflow:\n\
1. Confirm the account and auth state when needed.\n\
2. Use `zocli.drive.teams` to discover teams or workspaces.\n\
3. Use `zocli.drive.list` when you already know the target folder ID.\n\
4. For mutations, use `zocli.drive.upload` or `zocli.drive.download` with explicit folder IDs, file IDs, and local paths.\n\
5. Return a short summary of files, folders, or transfer results.\n\
\n\
Do not imply path-based WorkDrive support such as `disk:/...` because the stable surface uses real team, folder, and file IDs."
    ))
}

fn render_daily_briefing_prompt(arguments: &Value) -> String {
    let account = optional_string(arguments, "account").unwrap_or("current");
    let from = optional_string(arguments, "from").unwrap_or("today");
    let to = optional_string(arguments, "to").unwrap_or("tomorrow");
    let mail_limit = optional_string(arguments, "mail_limit").unwrap_or("10");

    format!(
        "Prepare a mail and calendar briefing through zocli.\n\
Account: {account}\n\
Mail limit: {mail_limit}\n\
Calendar window: from={from}, to={to}\n\
\n\
Use this workflow:\n\
1. Clarify the account if needed.\n\
2. Call `zocli.mail.list` with the requested limit.\n\
3. Call `zocli.calendar.events` for the target window.\n\
4. Summarize important or fresh mail, urgent topics, upcoming events, deadlines, and obvious conflicts.\n\
\n\
Keep the final answer short, operational, and action-oriented."
    )
}

fn render_find_and_read_prompt(arguments: &Value) -> Result<String> {
    let query = required_string(arguments, "query")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let limit = optional_string(arguments, "limit").unwrap_or("5");

    Ok(format!(
        "Find and read the right message through the zocli MCP server.\n\
Query: {query}\n\
Account: {account}\n\
Limit: {limit}\n\
\n\
Use this workflow:\n\
1. Call `zocli.mail.search` with the given query and limit.\n\
2. Choose the most relevant hit by sender, subject, date, and folder.\n\
3. Call `zocli.mail.read` with that hit's `folder_id` and `message_id`.\n\
4. Return the key message contents and explicitly say which folder ID and message ID you selected."
    ))
}

fn render_reply_with_context_prompt(arguments: &Value) -> Result<String> {
    let message_id = required_string(arguments, "message_id")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let folder_id = optional_string(arguments, "folder_id").unwrap_or("auto");
    let from = optional_string(arguments, "from").unwrap_or("auto");
    let to = optional_string(arguments, "to").unwrap_or("auto");

    Ok(format!(
        "Prepare or send a reply with calendar context through the zocli MCP server.\n\
Message ID: {message_id}\n\
Account: {account}\n\
Folder ID: {folder_id}\n\
Calendar window: from={from}, to={to}\n\
\n\
Use this workflow:\n\
1. Read the source message through `zocli.mail.read`.\n\
2. Derive the relevant calendar window from the message, or use the explicit one provided here.\n\
3. Check availability through `zocli.calendar.events`.\n\
4. If a real reply is needed, use `zocli.mail.reply`; if only a draft is needed, return the proposed draft text explicitly.\n\
\n\
State clearly whether the reply was actually sent or whether you only produced a draft."
    ))
}

fn required_string<'a>(arguments: &'a Value, key: &str) -> Result<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ZocliError::Validation(format!("prompt argument `{key}` is required")))
}

fn optional_string<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_definitions_expose_all_embedded_prompts() {
        let prompts = prompt_definitions();
        assert_eq!(prompts.len(), 7);
        assert!(prompts.iter().any(|prompt| prompt["name"] == "shared"));
        assert!(prompts.iter().any(|prompt| prompt["name"] == "mail"));
        assert!(prompts.iter().any(|prompt| prompt["name"] == "calendar"));
        assert!(prompts.iter().any(|prompt| prompt["name"] == "drive"));
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt["name"] == "daily-briefing")
        );
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt["name"] == "find-and-read")
        );
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt["name"] == "reply-with-context")
        );
    }

    #[test]
    fn get_prompt_requires_prompt_name() {
        let err = get_prompt(json!({})).expect_err("missing name should fail");
        assert!(err.to_string().contains("requires `name`"));
    }

    #[test]
    fn get_prompt_renders_daily_briefing_prompt() {
        let prompt = get_prompt(json!({
            "name": "daily-briefing",
            "arguments": {
                "account": "work",
                "mail_limit": "15"
            }
        }))
        .expect("prompt");

        let text = prompt["messages"][0]["content"]["text"]
            .as_str()
            .expect("prompt text");
        assert!(text.contains("Account: work"));
        assert!(text.contains("Mail limit: 15"));
        assert!(text.contains("zocli.mail.list"));
        assert!(text.contains("zocli.calendar.events"));
    }

    #[test]
    fn get_prompt_validates_required_arguments() {
        let err = get_prompt(json!({
            "name": "find-and-read",
            "arguments": {}
        }))
        .expect_err("missing query should fail");
        assert!(
            err.to_string()
                .contains("prompt argument `query` is required")
        );
    }

    #[test]
    fn reply_with_context_prompt_mentions_draft_boundary() {
        let prompt = get_prompt(json!({
            "name": "reply-with-context",
            "arguments": {
                "message_id": "42"
            }
        }))
        .expect("prompt");

        let text = prompt["messages"][0]["content"]["text"]
            .as_str()
            .expect("prompt text");
        assert!(text.contains("use `zocli.mail.reply`"));
        assert!(text.contains("only produced a draft"));
    }

    #[test]
    fn completion_for_mail_folder_filters_by_prefix() {
        let result = complete(json!({
            "ref": {
                "type": "ref/prompt",
                "name": "mail"
            },
            "argument": {
                "name": "folder",
                "value": "in"
            }
        }))
        .expect("completion");

        assert_eq!(result["completion"]["values"][0], "INBOX");
        assert_eq!(result["completion"]["hasMore"], false);
    }

    #[test]
    fn completion_for_calendar_filters_by_prefix() {
        let result = complete(json!({
            "ref": {
                "type": "ref/prompt",
                "name": "calendar"
            },
            "argument": {
                "name": "calendar",
                "value": "de"
            }
        }))
        .expect("completion");

        let values = result["completion"]["values"].as_array().expect("values");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0], "default");
    }

    #[test]
    fn completion_for_dashboard_resource_section_filters_by_prefix() {
        let result = complete(json!({
            "ref": {
                "type": "ref/resource",
                "uri": "ui://zocli/dashboard{?account,section,resource,tool}"
            },
            "argument": {
                "name": "section",
                "value": "re"
            }
        }))
        .expect("completion");

        let values = result["completion"]["values"].as_array().expect("values");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0], "resources");
    }

    #[test]
    fn completion_for_skill_resource_filters_by_prefix() {
        let result = complete(json!({
            "ref": {
                "type": "ref/resource",
                "uri": "resource://zocli/skill/{skill}"
            },
            "argument": {
                "name": "skill",
                "value": "zocli-ma"
            }
        }))
        .expect("completion");

        let values = result["completion"]["values"].as_array().expect("values");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0], "zocli-mail");
    }

    #[test]
    fn completion_for_dashboard_skill_filters_by_prefix() {
        let result = complete(json!({
            "ref": {
                "type": "ref/resource",
                "uri": "ui://zocli/dashboard{?account,section,resource,tool,skill}"
            },
            "argument": {
                "name": "skill",
                "value": "zocli-ca"
            }
        }))
        .expect("completion");

        let values = result["completion"]["values"].as_array().expect("values");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0], "zocli-calendar");
    }

    #[test]
    fn prompt_messages_include_canonical_skill_resource() {
        let prompt = get_prompt(json!({
            "name": "mail",
            "arguments": {
                "request": "reply to the latest invoice"
            }
        }))
        .expect("prompt");
        let text = prompt["messages"][0]["content"]["text"]
            .as_str()
            .expect("prompt text");
        assert!(text.contains("resource://zocli/skill/zocli-mail"));
    }

    #[test]
    fn prompt_definitions_are_bilingual() {
        let definitions = prompt_definitions();
        for definition in definitions {
            let description = definition["description"]
                .as_str()
                .expect("prompt description");
            assert!(
                description
                    .chars()
                    .any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                "prompt description is missing Cyrillic text: {description}"
            );
        }
    }
}
