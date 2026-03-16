use std::time::Duration;

use chrono::{DateTime, Days, NaiveDate, SecondsFormat, Utc};
use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::error::{Result, ZocliError};

const API_TIMEOUT_SECS: u64 = 20;
const DEFAULT_EVENT_WINDOW_DAYS: u64 = 30;
const MAX_RANGE_DAYS: i64 = 31;

// ---------------------------------------------------------------------------
// Public data types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct CalendarInfo {
    pub uid: String,
    pub name: String,
    pub color: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct CalendarEvent {
    pub uid: String,
    pub title: String,
    pub start: String,
    pub end: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct CreatedEvent {
    pub uid: String,
}

#[derive(Clone, Debug)]
pub struct CreateEventRequest {
    pub title: String,
    pub start: String,
    pub end: String,
    pub description: Option<String>,
    pub location: Option<String>,
}

/// Legacy-compat types kept for callers that haven't migrated yet.

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct CalendarCollection {
    pub id: String,
    pub name: String,
    pub href: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct CalendarEventWindow {
    pub from: String,
    pub to: String,
    pub limit: usize,
}

#[derive(Clone, Debug)]
pub struct CalendarEventsRequest {
    pub calendar: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub limit: usize,
}

#[derive(Clone, Debug)]
pub struct CalendarCreateRequest {
    pub calendar: String,
    pub summary: String,
    pub start: String,
    pub end: String,
    pub description: Option<String>,
    pub location: Option<String>,
}

// ---------------------------------------------------------------------------
// Zoho Calendar REST JSON shapes (private, for serde deserialization)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ZohoCalendarsResponse {
    #[serde(default)]
    calendars: Vec<ZohoCalendar>,
}

#[derive(Deserialize)]
struct ZohoCalendar {
    #[serde(default)]
    uid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    color: String,
    #[serde(default)]
    isdefault: bool,
}

#[derive(Deserialize)]
struct ZohoEventsResponse {
    #[serde(default)]
    events: Vec<ZohoEvent>,
}

#[derive(Deserialize)]
struct ZohoEvent {
    #[serde(default)]
    uid: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    dateandtime: Option<ZohoDateAndTime>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    etag: Option<String>,
}

#[derive(Deserialize)]
struct ZohoDateAndTime {
    #[serde(default)]
    start: String,
    #[serde(default)]
    end: String,
}

// ---------------------------------------------------------------------------
// New Zoho Calendar REST public functions
// ---------------------------------------------------------------------------

/// List all calendars for the authenticated user.
pub fn list_calendars_rest(base_url: &str, access_token: &str) -> Result<Vec<CalendarInfo>> {
    let client = zoho_client()?;
    let url = format!("{}/api/v1/calendars", base_url.trim_end_matches('/'));

    let response = client
        .get(&url)
        .header("Authorization", format!("Zoho-oauthtoken {access_token}"))
        .send()?;

    check_response_status(&response, "list calendars")?;

    let body: ZohoCalendarsResponse = response.json().map_err(|err| {
        ZocliError::Serialization(format!("failed to parse calendars JSON: {err}"))
    })?;

    let mut calendars: Vec<CalendarInfo> = body
        .calendars
        .into_iter()
        .map(|c| CalendarInfo {
            uid: c.uid,
            name: c.name,
            color: c.color,
            is_default: c.isdefault,
        })
        .collect();

    calendars.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(calendars)
}

/// List events in a calendar within a date range.
///
/// `from` and `to` are RFC 3339 timestamps (e.g. `2026-03-12T00:00:00Z`).
/// If the range exceeds 31 days, the request is automatically split into
/// multiple <=31-day windows, results merged and sorted by start time,
/// then truncated to `limit`.
pub fn list_events(
    base_url: &str,
    access_token: &str,
    calendar_uid: &str,
    from: &str,
    to: &str,
    limit: usize,
) -> Result<Vec<CalendarEvent>> {
    let from_dt = parse_rfc3339_or_date(from, "list_events <from>")?;
    let to_dt = parse_rfc3339_or_date(to, "list_events <to>")?;

    if to_dt <= from_dt {
        return Err(ZocliError::Validation(
            "list_events <to> must be after <from>".to_string(),
        ));
    }

    let client = zoho_client()?;
    let base = base_url.trim_end_matches('/');

    let mut all_events: Vec<CalendarEvent> = Vec::new();

    // Split into <=31-day chunks
    let mut chunk_start = from_dt;
    while chunk_start < to_dt {
        let chunk_end_candidate = chunk_start + chrono::Duration::days(MAX_RANGE_DAYS);
        let chunk_end = if chunk_end_candidate > to_dt {
            to_dt
        } else {
            chunk_end_candidate
        };

        let range_from = format_zoho_datetime(&chunk_start);
        let range_end = format_zoho_datetime(&chunk_end);
        let range_json = format!(r#"{{"start":"{}","end":"{}"}}"#, range_from, range_end);

        let url = format!("{base}/api/v1/calendars/{calendar_uid}/events");

        let response = client
            .get(&url)
            .header("Authorization", format!("Zoho-oauthtoken {access_token}"))
            .query(&[("range", &range_json)])
            .send()?;

        check_response_status(&response, "list events")?;

        let body: ZohoEventsResponse = response.json().map_err(|err| {
            ZocliError::Serialization(format!("failed to parse events JSON: {err}"))
        })?;

        for event in body.events {
            let (start, end) = match event.dateandtime {
                Some(dt) => (dt.start, dt.end),
                None => (String::new(), String::new()),
            };
            all_events.push(CalendarEvent {
                uid: event.uid,
                title: event.title,
                start,
                end,
                location: non_empty(event.location),
                description: non_empty(event.description),
                etag: non_empty(event.etag),
            });
        }

        chunk_start = chunk_end;
    }

    // Sort by start time, then title, then uid
    all_events.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then(a.title.cmp(&b.title))
            .then(a.uid.cmp(&b.uid))
    });

    if all_events.len() > limit {
        all_events.truncate(limit);
    }

    Ok(all_events)
}

/// Create an event in a calendar.
pub fn create_event(
    base_url: &str,
    access_token: &str,
    calendar_uid: &str,
    req: CreateEventRequest,
) -> Result<CreatedEvent> {
    let client = zoho_client()?;
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/api/v1/calendars/{calendar_uid}/events");

    let title = req.title.trim();
    if title.is_empty() {
        return Err(ZocliError::Validation(
            "create_event title must not be empty".to_string(),
        ));
    }

    let start_zoho = to_zoho_datetime_from_input(&req.start, "create_event <start>")?;
    let end_zoho = to_zoho_datetime_from_input(&req.end, "create_event <end>")?;

    let dateandtime = serde_json::json!({
        "start": start_zoho,
        "end": end_zoho,
        "timezone": "UTC",
    });

    let mut eventdata = serde_json::json!({
        "title": title,
        "dateandtime": dateandtime,
    });

    if let Some(ref desc) = req.description {
        if !desc.trim().is_empty() {
            eventdata["description"] = serde_json::Value::String(desc.trim().to_string());
        }
    }
    if let Some(ref loc) = req.location {
        if !loc.trim().is_empty() {
            eventdata["location"] = serde_json::Value::String(loc.trim().to_string());
        }
    }

    let eventdata_json = serde_json::to_string(&eventdata).map_err(|err| {
        ZocliError::Serialization(format!("failed to serialize eventdata: {err}"))
    })?;

    let response = client
        .post(&url)
        .header("Authorization", format!("Zoho-oauthtoken {access_token}"))
        .query(&[("eventdata", &eventdata_json)])
        .send()?;

    check_response_status(&response, "create event")?;

    let body: ZohoEventsResponse = response.json().map_err(|err| {
        ZocliError::Serialization(format!("failed to parse create-event JSON: {err}"))
    })?;

    let uid = body
        .events
        .into_iter()
        .next()
        .map(|e| e.uid)
        .unwrap_or_default();

    Ok(CreatedEvent { uid })
}

/// Delete an event from a calendar.
pub fn delete_event(
    base_url: &str,
    access_token: &str,
    calendar_uid: &str,
    event_uid: &str,
    etag: Option<&str>,
) -> Result<()> {
    let client = zoho_client()?;
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/api/v1/calendars/{calendar_uid}/events/{event_uid}");

    let mut request = client
        .delete(&url)
        .header("Authorization", format!("Zoho-oauthtoken {access_token}"));

    if let Some(etag_value) = etag {
        request = request.header("etag", etag_value);
    }

    let response = request.send()?;

    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(ZocliError::Auth(format!(
            "delete event failed with status {}. Check the Zoho access token.",
            status.as_u16()
        )));
    }
    // 204 No Content is the expected success response
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(ZocliError::Api(format!(
            "delete event failed with status {}: {}",
            status.as_u16(),
            truncate_body(&body)
        )));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Legacy-compat public functions (same signatures as old CalDAV code)
// ---------------------------------------------------------------------------

/// List calendars using the legacy 3-arg signature.
/// `base_url` = Zoho Calendar REST base URL, `_account` = unused (kept for compat),
/// `access_token` = Zoho OAuth access token.
pub fn list_calendars(
    base_url: &str,
    _account: &str,
    access_token: &str,
) -> Result<Vec<CalendarCollection>> {
    let infos = list_calendars_rest(base_url, access_token)?;
    Ok(infos
        .into_iter()
        .map(|info| CalendarCollection {
            id: info.uid.clone(),
            name: info.name,
            href: info.uid,
            description: None,
        })
        .collect())
}

/// List events using the legacy 4-arg signature.
pub fn list_calendar_events(
    base_url: &str,
    _account: &str,
    access_token: &str,
    request: CalendarEventsRequest,
) -> Result<(CalendarCollection, CalendarEventWindow, Vec<CalendarEvent>)> {
    let window = CalendarEventWindow {
        from: request.from.to_rfc3339_opts(SecondsFormat::Secs, true),
        to: request.to.to_rfc3339_opts(SecondsFormat::Secs, true),
        limit: request.limit,
    };

    // Resolve calendar by name or uid
    let calendars = list_calendars_rest(base_url, access_token)?;
    let cal = find_calendar_info(&calendars, &request.calendar)?;
    let calendar_uid = cal.uid.clone();

    let events = list_events(
        base_url,
        access_token,
        &calendar_uid,
        &window.from,
        &window.to,
        request.limit,
    )?;

    let collection = CalendarCollection {
        id: cal.uid.clone(),
        name: cal.name.clone(),
        href: cal.uid.clone(),
        description: None,
    };

    Ok((collection, window, events))
}

/// Create an event using the legacy signature.
pub fn create_calendar_event(
    base_url: &str,
    _account: &str,
    access_token: &str,
    request: CalendarCreateRequest,
) -> Result<(CalendarCollection, CalendarEvent)> {
    let calendars = list_calendars_rest(base_url, access_token)?;
    let cal = find_calendar_info(&calendars, &request.calendar)?;
    let calendar_uid = cal.uid.clone();

    let start_input = request.start.clone();
    let end_input = request.end.clone();

    let created = create_event(
        base_url,
        access_token,
        &calendar_uid,
        CreateEventRequest {
            title: request.summary.clone(),
            start: start_input.clone(),
            end: end_input.clone(),
            description: request.description.clone(),
            location: request.location.clone(),
        },
    )?;

    let collection = CalendarCollection {
        id: cal.uid.clone(),
        name: cal.name.clone(),
        href: cal.uid.clone(),
        description: None,
    };

    let event = CalendarEvent {
        uid: created.uid,
        title: request.summary,
        start: start_input,
        end: end_input,
        location: request.location,
        description: request.description,
        etag: None,
    };

    Ok((collection, event))
}

/// Delete an event using the legacy signature.
pub fn delete_calendar_event(
    base_url: &str,
    _account: &str,
    access_token: &str,
    calendar_ref: &str,
    uid: &str,
) -> Result<(CalendarCollection, CalendarEvent)> {
    let calendars = list_calendars_rest(base_url, access_token)?;
    let cal = find_calendar_info(&calendars, calendar_ref)?;
    let calendar_uid = cal.uid.clone();

    // Fetch the event before deleting so we can return its details
    let from = Utc::now()
        .checked_sub_signed(chrono::Duration::days(365))
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(SecondsFormat::Secs, true);
    let to = Utc::now()
        .checked_add_signed(chrono::Duration::days(365))
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(SecondsFormat::Secs, true);

    let events = list_events(base_url, access_token, &calendar_uid, &from, &to, 1000)?;
    let event = events.into_iter().find(|e| e.uid == uid).ok_or_else(|| {
        ZocliError::Validation(format!(
            "calendar event with uid `{uid}` not found in calendar `{calendar_ref}`"
        ))
    })?;

    delete_event(
        base_url,
        access_token,
        &calendar_uid,
        uid,
        event.etag.as_deref(),
    )?;

    let collection = CalendarCollection {
        id: cal.uid.clone(),
        name: cal.name.clone(),
        href: cal.uid.clone(),
        description: None,
    };

    Ok((collection, event))
}

// ---------------------------------------------------------------------------
// Event window parsing (unchanged public API)
// ---------------------------------------------------------------------------

pub fn parse_event_window(
    from: Option<&str>,
    to: Option<&str>,
    limit: usize,
) -> Result<CalendarEventWindow> {
    if limit == 0 {
        return Err(ZocliError::Validation(
            "calendar events --limit must be greater than zero".to_string(),
        ));
    }
    if limit > 100 {
        return Err(ZocliError::Validation(
            "calendar events --limit must not be greater than 100".to_string(),
        ));
    }

    let now = Utc::now();
    let default_from = now;
    let default_to = now
        .checked_add_days(Days::new(DEFAULT_EVENT_WINDOW_DAYS))
        .ok_or_else(|| {
            ZocliError::Validation("failed to compute default calendar time window".to_string())
        })?;

    let from = match from {
        Some(value) => parse_time_boundary(value, "calendar events <FROM>")?,
        None => default_from,
    };
    let to = match to {
        Some(value) => parse_time_boundary(value, "calendar events <TO>")?,
        None => default_to,
    };

    if to <= from {
        return Err(ZocliError::Validation(
            "calendar events <TO> must be after <FROM>".to_string(),
        ));
    }

    Ok(CalendarEventWindow {
        from: from.to_rfc3339_opts(SecondsFormat::Secs, true),
        to: to.to_rfc3339_opts(SecondsFormat::Secs, true),
        limit,
    })
}

// ---------------------------------------------------------------------------
// Private helpers — HTTP
// ---------------------------------------------------------------------------

fn zoho_client() -> Result<Client> {
    Client::builder()
        .user_agent(format!("zocli/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(API_TIMEOUT_SECS))
        .build()
        .map_err(|err| ZocliError::Network(format!("failed to create HTTP client: {err}")))
}

fn check_response_status(response: &reqwest::blocking::Response, action: &str) -> Result<()> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(ZocliError::Auth(format!(
            "{action} failed with status {}. Check the Zoho access token.",
            status.as_u16()
        )));
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(ZocliError::Api(format!(
            "{action} rate limited (429). Try again later."
        )));
    }
    Err(ZocliError::Api(format!(
        "{action} failed with status {}",
        status.as_u16()
    )))
}

fn find_calendar_info<'a>(
    calendars: &'a [CalendarInfo],
    calendar_ref: &str,
) -> Result<&'a CalendarInfo> {
    if calendar_ref == "default" {
        return calendars
            .iter()
            .find(|c| c.is_default)
            .or_else(|| calendars.first())
            .ok_or_else(|| ZocliError::Validation("no calendars found".to_string()));
    }
    calendars
        .iter()
        .find(|c| c.uid == calendar_ref || c.name == calendar_ref)
        .ok_or_else(|| {
            ZocliError::Validation(format!(
                "calendar `{calendar_ref}` not found; run `zocli calendar calendars` to inspect available ids"
            ))
        })
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

fn truncate_body(body: &str) -> String {
    const MAX_BODY_CHARS: usize = 240;
    if body.chars().count() <= MAX_BODY_CHARS {
        return body.to_string();
    }
    let truncated: String = body.chars().take(MAX_BODY_CHARS).collect();
    format!("{truncated}...")
}

// ---------------------------------------------------------------------------
// Private helpers — date/time formatting
// ---------------------------------------------------------------------------

/// Format a chrono DateTime<Utc> into Zoho's expected format: `yyyyMMdd'T'HHmmss'Z'`
fn format_zoho_datetime(dt: &DateTime<Utc>) -> String {
    dt.format("%Y%m%dT%H%M%SZ").to_string()
}

/// Parse an RFC 3339 string or YYYY-MM-DD date into DateTime<Utc>.
fn parse_rfc3339_or_date(value: &str, flag_name: &str) -> Result<DateTime<Utc>> {
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let datetime = date.and_hms_opt(0, 0, 0).ok_or_else(|| {
            ZocliError::Validation(format!("{flag_name} contains an invalid date"))
        })?;
        return Ok(datetime.and_utc());
    }

    DateTime::parse_from_rfc3339(value)
        .map(|v| v.with_timezone(&Utc))
        .map_err(|_| {
            ZocliError::Validation(format!(
                "{flag_name} must use YYYY-MM-DD or RFC3339, got `{value}`"
            ))
        })
}

fn parse_time_boundary(value: &str, flag_name: &str) -> Result<DateTime<Utc>> {
    parse_rfc3339_or_date(value, flag_name)
}

/// Convert a user-supplied start/end string into the Zoho datetime format.
/// Accepts RFC 3339 timestamps and YYYY-MM-DD dates.
fn to_zoho_datetime_from_input(value: &str, flag_name: &str) -> Result<String> {
    let dt = parse_rfc3339_or_date(value, flag_name)?;
    Ok(format_zoho_datetime(&dt))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_event_window_accepts_date_and_rfc3339_inputs() {
        let window = parse_event_window(Some("2026-03-12"), Some("2026-03-13T12:00:00Z"), 20)
            .expect("window");

        assert_eq!(window.from, "2026-03-12T00:00:00Z");
        assert_eq!(window.to, "2026-03-13T12:00:00Z");
        assert_eq!(window.limit, 20);
    }

    #[test]
    fn parse_event_window_rejects_zero_limit() {
        let err = parse_event_window(Some("2026-03-12"), Some("2026-03-13"), 0).unwrap_err();
        assert!(err.to_string().contains("greater than zero"));
    }

    #[test]
    fn parse_event_window_rejects_over_100_limit() {
        let err = parse_event_window(Some("2026-03-12"), Some("2026-03-13"), 101).unwrap_err();
        assert!(err.to_string().contains("greater than 100"));
    }

    #[test]
    fn parse_event_window_rejects_reversed_range() {
        let err = parse_event_window(Some("2026-03-13"), Some("2026-03-12"), 10).unwrap_err();
        assert!(err.to_string().contains("must be after"));
    }

    #[test]
    fn format_zoho_datetime_produces_expected_format() {
        let dt = chrono::DateTime::parse_from_rfc3339("2026-03-12T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(format_zoho_datetime(&dt), "20260312T090000Z");
    }

    #[test]
    fn to_zoho_datetime_from_input_handles_rfc3339() {
        let result = to_zoho_datetime_from_input("2026-03-12T09:00:00Z", "test").unwrap();
        assert_eq!(result, "20260312T090000Z");
    }

    #[test]
    fn to_zoho_datetime_from_input_handles_date() {
        let result = to_zoho_datetime_from_input("2026-03-12", "test").unwrap();
        assert_eq!(result, "20260312T000000Z");
    }

    #[test]
    fn parse_rfc3339_or_date_accepts_both_formats() {
        let dt1 = parse_rfc3339_or_date("2026-03-12", "test").unwrap();
        assert_eq!(
            dt1.to_rfc3339_opts(SecondsFormat::Secs, true),
            "2026-03-12T00:00:00Z"
        );

        let dt2 = parse_rfc3339_or_date("2026-03-12T09:30:00Z", "test").unwrap();
        assert_eq!(
            dt2.to_rfc3339_opts(SecondsFormat::Secs, true),
            "2026-03-12T09:30:00Z"
        );
    }

    #[test]
    fn parse_rfc3339_or_date_rejects_garbage() {
        let err = parse_rfc3339_or_date("not-a-date", "test").unwrap_err();
        assert!(err.to_string().contains("must use YYYY-MM-DD or RFC3339"));
    }

    #[test]
    fn truncate_body_truncates_long_strings() {
        let short = "hello";
        assert_eq!(truncate_body(short), "hello");

        let long = "a".repeat(300);
        let truncated = truncate_body(&long);
        assert!(truncated.ends_with("..."));
        assert!(truncated.len() < 300);
    }

    #[test]
    fn non_empty_filters_blank_strings() {
        assert_eq!(
            non_empty(Some("hello".to_string())),
            Some("hello".to_string())
        );
        assert_eq!(non_empty(Some("".to_string())), None);
        assert_eq!(non_empty(Some("   ".to_string())), None);
        assert_eq!(non_empty(None), None);
    }
}
