use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{Result, ZocliError};

/// Zoho sometimes returns booleans as `"0"`/`"1"` strings.
fn bool_from_zoho<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BoolOrString {
        Bool(bool),
        Str(String),
        Num(u64),
    }
    match BoolOrString::deserialize(d)? {
        BoolOrString::Bool(b) => Ok(b),
        BoolOrString::Str(s) => Ok(s != "0" && !s.is_empty() && s != "false"),
        BoolOrString::Num(n) => Ok(n != 0),
    }
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct MailFolder {
    #[serde(alias = "folderId")]
    pub folder_id: String,
    #[serde(alias = "folderName")]
    pub folder_name: String,
    #[serde(alias = "messageCount", default)]
    pub message_count: u64,
    #[serde(alias = "unreadCount", default)]
    pub unread_count: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct MailMessageSummary {
    #[serde(alias = "messageId")]
    pub message_id: String,
    #[serde(alias = "folderId", default)]
    pub folder_id: String,
    #[serde(default)]
    pub sender: String,
    #[serde(alias = "toAddress", default)]
    pub to_address: String,
    #[serde(default)]
    pub subject: String,
    #[serde(alias = "receivedTime", default)]
    pub received_time: String,
    #[serde(alias = "isRead", default, deserialize_with = "bool_from_zoho")]
    pub is_read: bool,
    #[serde(alias = "hasAttachment", default, deserialize_with = "bool_from_zoho")]
    pub has_attachment: bool,
    #[serde(default)]
    pub summary: String,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct MailMessage {
    pub message_id: String,
    pub folder_id: String,
    pub sender: String,
    pub to_address: String,
    pub subject: String,
    pub received_time: String,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct SentMail {
    pub message_id: String,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct RepliedMail {
    pub message_id: String,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct ForwardedMail {
    pub message_id: String,
}

// ---------------------------------------------------------------------------
// Request structures
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct MailSendRequest {
    pub from_address: String,
    pub to_address: String,
    pub cc_address: String,
    pub bcc_address: String,
    pub subject: String,
    pub content: String,
    pub mail_format: String,
    pub attachments: Vec<UploadedAttachment>,
}

#[derive(Clone, Debug)]
pub struct MailReplyRequest {
    pub message_id: String,
    pub content: String,
    pub cc_address: String,
    pub mail_format: String,
    pub from_address: Option<String>,
}

#[derive(Clone, Debug)]
pub struct MailForwardRequest {
    pub message_id: String,
    pub folder_id: String,
    pub from_address: String,
    pub to_address: String,
    pub content: String,
    pub cc_address: String,
    pub bcc_address: String,
}

// ---------------------------------------------------------------------------
// Zoho API JSON response wrappers (internal)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ZohoDataResponse<T> {
    data: Option<T>,
    #[serde(default)]
    status: Option<ZohoStatus>,
}

#[derive(Deserialize)]
struct ZohoStatus {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct ZohoContentData {
    content: String,
}

#[derive(Deserialize)]
struct ZohoSentData {
    #[serde(alias = "messageId", default)]
    message_id: Option<String>,
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

fn build_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ZocliError::Network(format!("failed to create HTTP client: {e}")))
}

fn auth_header(access_token: &str) -> String {
    format!("Zoho-oauthtoken {access_token}")
}

/// Check for Zoho-level error in the response status object.
fn check_zoho_error(status: &Option<ZohoStatus>, context: &str) -> Result<()> {
    if let Some(s) = status {
        // Zoho uses code 200 for success; anything else is an error.
        if s.code != 200 && s.code != 0 {
            return Err(ZocliError::Api(format!(
                "{context}: Zoho error {}: {}",
                s.code, s.description
            )));
        }
    }
    Ok(())
}

/// Check HTTP status and return the body text or an error.
fn checked_response_text(resp: reqwest::blocking::Response, context: &str) -> Result<String> {
    let status = resp.status();
    let body = resp.text().map_err(|e| {
        ZocliError::Network(format!("{context}: failed to read response body: {e}"))
    })?;

    if !status.is_success() {
        return Err(ZocliError::Api(format!(
            "{context}: HTTP {status} — {body}"
        )));
    }
    Ok(body)
}

// ---------------------------------------------------------------------------
// Public API functions
// ---------------------------------------------------------------------------

/// Discovered account info from Zoho Mail API.
pub struct DiscoveredAccount {
    pub account_id: String,
    pub zuid: Option<String>,
}

/// Discover the Zoho Mail account ID and ZUID by calling GET /api/accounts.
/// Returns the first account's accountId and zuid.
pub fn discover_mail_account(base_url: &str, access_token: &str) -> Result<DiscoveredAccount> {
    let client = build_client()?;
    let url = format!("{base_url}/api/accounts");
    let resp = client
        .get(&url)
        .header("Authorization", auth_header(access_token))
        .send()?;
    let body = checked_response_text(resp, "discover_account")?;

    #[derive(Deserialize)]
    struct Account {
        #[serde(alias = "accountId")]
        account_id: String,
        #[serde(default)]
        zuid: Option<u64>,
    }
    #[derive(Deserialize)]
    struct Resp {
        data: Vec<Account>,
    }

    let parsed: Resp = serde_json::from_str(&body)
        .map_err(|e| ZocliError::Api(format!("discover_account: bad JSON: {e}")))?;
    let acct = parsed
        .data
        .into_iter()
        .next()
        .ok_or_else(|| ZocliError::Api("no Zoho Mail accounts found".to_string()))?;

    Ok(DiscoveredAccount {
        account_id: acct.account_id,
        zuid: acct.zuid.map(|z| z.to_string()),
    })
}

/// List all mail folders for the account.
pub fn list_mail_folders(
    base_url: &str,
    account_id: &str,
    access_token: &str,
) -> Result<Vec<MailFolder>> {
    let client = build_client()?;
    let url = format!("{base_url}/api/accounts/{account_id}/folders");

    let resp = client
        .get(&url)
        .header("Authorization", auth_header(access_token))
        .send()?;

    let body = checked_response_text(resp, "list_mail_folders")?;
    let parsed: ZohoDataResponse<Vec<MailFolder>> = serde_json::from_str(&body)
        .map_err(|e| ZocliError::Api(format!("list_mail_folders: bad JSON: {e}")))?;

    check_zoho_error(&parsed.status, "list_mail_folders")?;

    Ok(parsed.data.unwrap_or_default())
}

/// List messages in a folder (optionally filtered to unread-only).
pub fn list_mail_messages(
    base_url: &str,
    account_id: &str,
    access_token: &str,
    folder_id: Option<&str>,
    unread_only: bool,
    limit: usize,
) -> Result<Vec<MailMessageSummary>> {
    let client = build_client()?;
    let url = format!("{base_url}/api/accounts/{account_id}/messages/view");

    let mut query: Vec<(&str, String)> =
        vec![("limit", limit.to_string()), ("start", "0".to_string())];
    if let Some(fid) = folder_id {
        query.push(("folderId", fid.to_string()));
    }
    if unread_only {
        query.push(("status", "unread".to_string()));
    }

    let resp = client
        .get(&url)
        .header("Authorization", auth_header(access_token))
        .query(&query)
        .send()?;

    let body = checked_response_text(resp, "list_mail_messages")?;
    let parsed: ZohoDataResponse<Vec<MailMessageSummary>> = serde_json::from_str(&body)
        .map_err(|e| ZocliError::Api(format!("list_mail_messages: bad JSON: {e}")))?;

    check_zoho_error(&parsed.status, "list_mail_messages")?;

    Ok(parsed.data.unwrap_or_default())
}

/// Read the full content of a specific message.
pub fn read_mail_message(
    base_url: &str,
    account_id: &str,
    access_token: &str,
    folder_id: &str,
    message_id: &str,
) -> Result<MailMessage> {
    // First, we need the message metadata. Fetch it from the messages/view endpoint
    // filtered to the specific folder so we have sender/subject/etc.
    let summary = fetch_message_summary(base_url, account_id, access_token, folder_id, message_id)?;

    // Now fetch the content.
    let client = build_client()?;
    let url = format!(
        "{base_url}/api/accounts/{account_id}/folders/{folder_id}/messages/{message_id}/content"
    );

    let resp = client
        .get(&url)
        .header("Authorization", auth_header(access_token))
        .send()?;

    let body = checked_response_text(resp, "read_mail_message")?;
    let parsed: ZohoDataResponse<ZohoContentData> = serde_json::from_str(&body)
        .map_err(|e| ZocliError::Api(format!("read_mail_message: bad JSON: {e}")))?;

    check_zoho_error(&parsed.status, "read_mail_message")?;

    let content_data = parsed.data.ok_or_else(|| {
        ZocliError::Api("read_mail_message: response contained no data".to_string())
    })?;

    Ok(MailMessage {
        message_id: summary.message_id,
        folder_id: summary.folder_id,
        sender: summary.sender,
        to_address: summary.to_address,
        subject: summary.subject,
        received_time: summary.received_time,
        content: content_data.content,
    })
}

/// Search messages across the account.
pub fn search_mail_messages(
    base_url: &str,
    account_id: &str,
    access_token: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<MailMessageSummary>> {
    if query.trim().is_empty() {
        return Err(ZocliError::Validation(
            "search query must not be empty".to_string(),
        ));
    }

    let client = build_client()?;
    let url = format!("{base_url}/api/accounts/{account_id}/messages/search");

    let params: Vec<(&str, String)> = vec![
        ("searchKey", query.to_string()),
        ("limit", limit.to_string()),
        ("start", "0".to_string()),
    ];

    let resp = client
        .get(&url)
        .header("Authorization", auth_header(access_token))
        .query(&params)
        .send()?;

    let body = checked_response_text(resp, "search_mail_messages")?;
    let parsed: ZohoDataResponse<Vec<MailMessageSummary>> = serde_json::from_str(&body)
        .map_err(|e| ZocliError::Api(format!("search_mail_messages: bad JSON: {e}")))?;

    check_zoho_error(&parsed.status, "search_mail_messages")?;

    Ok(parsed.data.unwrap_or_default())
}

/// Attachment metadata returned by Zoho's attachmentinfo endpoint.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttachmentInfo {
    #[serde(alias = "attachmentId")]
    pub attachment_id: String,
    #[serde(alias = "attachmentName")]
    pub attachment_name: String,
    #[serde(alias = "attachmentSize", default)]
    pub attachment_size: u64,
}

/// An uploaded attachment ready to be referenced in a send/forward payload.
#[derive(Clone, Debug, Serialize)]
pub struct UploadedAttachment {
    #[serde(rename = "storeName")]
    pub store_name: String,
    #[serde(rename = "attachmentPath")]
    pub attachment_path: String,
    #[serde(rename = "attachmentName")]
    pub attachment_name: String,
}

/// List attachments for a message.
pub fn get_attachment_info(
    base_url: &str,
    account_id: &str,
    access_token: &str,
    folder_id: &str,
    message_id: &str,
) -> Result<Vec<AttachmentInfo>> {
    let client = build_client()?;
    let url = format!(
        "{base_url}/api/accounts/{account_id}/folders/{folder_id}/messages/{message_id}/attachmentinfo"
    );
    let resp = client
        .get(&url)
        .header("Authorization", auth_header(access_token))
        .send()?;
    let body = checked_response_text(resp, "get_attachment_info")?;
    let parsed: ZohoDataResponse<ZohoAttachmentInfoData> = serde_json::from_str(&body)
        .map_err(|e| ZocliError::Api(format!("get_attachment_info: bad JSON: {e}")))?;
    check_zoho_error(&parsed.status, "get_attachment_info")?;
    Ok(parsed
        .data
        .map(|d| d.attachments)
        .unwrap_or_default())
}

/// Download an attachment's raw bytes.
pub fn download_attachment(
    base_url: &str,
    account_id: &str,
    access_token: &str,
    folder_id: &str,
    message_id: &str,
    attachment_id: &str,
) -> Result<Vec<u8>> {
    let client = build_client()?;
    let url = format!(
        "{base_url}/api/accounts/{account_id}/folders/{folder_id}/messages/{message_id}/attachments/{attachment_id}"
    );
    let resp = client
        .get(&url)
        .header("Authorization", auth_header(access_token))
        .send()?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(ZocliError::Api(format!(
            "download_attachment: HTTP {status} — {text}"
        )));
    }
    resp.bytes()
        .map(|b| b.to_vec())
        .map_err(|e| ZocliError::Network(format!("download_attachment: {e}")))
}

/// Upload a file as an attachment and return the metadata needed for send.
pub fn upload_attachment(
    base_url: &str,
    account_id: &str,
    access_token: &str,
    file_name: &str,
    file_bytes: Vec<u8>,
) -> Result<UploadedAttachment> {
    let client = build_client()?;
    let url = format!(
        "{base_url}/api/accounts/{account_id}/messages/attachments?uploadType=multipart"
    );
    let mime = mime_from_filename(file_name);
    let part = reqwest::blocking::multipart::Part::bytes(file_bytes)
        .file_name(file_name.to_string())
        .mime_str(&mime)
        .map_err(|e| ZocliError::Network(format!("upload_attachment: MIME error: {e}")))?;
    let form = reqwest::blocking::multipart::Form::new().part("attach", part);

    let resp = client
        .post(&url)
        .header("Authorization", auth_header(access_token))
        .multipart(form)
        .send()?;
    let body = checked_response_text(resp, "upload_attachment")?;
    let parsed: ZohoDataResponse<Vec<ZohoUploadData>> = serde_json::from_str(&body)
        .map_err(|e| ZocliError::Api(format!("upload_attachment: bad JSON: {e}")))?;
    check_zoho_error(&parsed.status, "upload_attachment")?;
    let items = parsed.data.ok_or_else(|| {
        ZocliError::Api("upload_attachment: response contained no data".to_string())
    })?;
    let data = items.into_iter().next().ok_or_else(|| {
        ZocliError::Api("upload_attachment: response contained empty array".to_string())
    })?;
    Ok(UploadedAttachment {
        store_name: data.store_name,
        attachment_path: data.attachment_path,
        attachment_name: data.attachment_name,
    })
}

/// Send a new mail message.
pub fn send_mail_message(
    base_url: &str,
    account_id: &str,
    access_token: &str,
    req: MailSendRequest,
) -> Result<SentMail> {
    let client = build_client()?;
    let url = format!("{base_url}/api/accounts/{account_id}/messages");

    let mut payload = serde_json::json!({
        "fromAddress": req.from_address,
        "toAddress": req.to_address,
        "ccAddress": req.cc_address,
        "bccAddress": req.bcc_address,
        "subject": req.subject,
        "content": req.content,
        "mailFormat": req.mail_format,
    });
    if !req.attachments.is_empty() {
        payload["attachments"] = serde_json::to_value(&req.attachments)
            .map_err(|e| ZocliError::Api(format!("send_mail_message: attachment serialize: {e}")))?;
    }

    let resp = client
        .post(&url)
        .header("Authorization", auth_header(access_token))
        .json(&payload)
        .send()?;

    let body = checked_response_text(resp, "send_mail_message")?;
    let parsed: ZohoDataResponse<ZohoSentData> = serde_json::from_str(&body)
        .map_err(|e| ZocliError::Api(format!("send_mail_message: bad JSON: {e}")))?;

    check_zoho_error(&parsed.status, "send_mail_message")?;

    let sent_data = parsed.data.ok_or_else(|| {
        ZocliError::Api("send_mail_message: response contained no data".to_string())
    })?;

    Ok(SentMail {
        message_id: sent_data.message_id.unwrap_or_default(),
    })
}

/// Reply to a message.
pub fn reply_to_mail_message(
    base_url: &str,
    account_id: &str,
    access_token: &str,
    req: MailReplyRequest,
) -> Result<RepliedMail> {
    let client = build_client()?;
    let url = format!(
        "{base_url}/api/accounts/{account_id}/messages/{}",
        req.message_id
    );

    let mut payload = serde_json::json!({
        "action": "reply",
        "content": req.content,
        "mailFormat": req.mail_format,
    });
    if let Some(ref from) = req.from_address {
        payload["fromAddress"] = serde_json::Value::String(from.clone());
    }
    if !req.cc_address.is_empty() {
        payload["ccAddress"] = serde_json::Value::String(req.cc_address.clone());
    }

    let resp = client
        .post(&url)
        .header("Authorization", auth_header(access_token))
        .json(&payload)
        .send()?;

    let body = checked_response_text(resp, "reply_to_mail_message")?;
    let parsed: ZohoDataResponse<ZohoSentData> = serde_json::from_str(&body)
        .map_err(|e| ZocliError::Api(format!("reply_to_mail_message: bad JSON: {e}")))?;

    check_zoho_error(&parsed.status, "reply_to_mail_message")?;

    let data = parsed.data.ok_or_else(|| {
        ZocliError::Api("reply_to_mail_message: response contained no data".to_string())
    })?;

    Ok(RepliedMail {
        message_id: data.message_id.unwrap_or_default(),
    })
}

/// Forward a message to another recipient.
///
/// Zoho Mail API has no native "forward" action, so this:
/// 1. Reads the original message content
/// 2. Downloads and re-uploads any attachments
/// 3. Sends a new message with forwarded content + attachments
pub fn forward_mail_message(
    base_url: &str,
    account_id: &str,
    access_token: &str,
    req: MailForwardRequest,
) -> Result<ForwardedMail> {
    // 1. Read original message
    let original = read_mail_message(
        base_url,
        account_id,
        access_token,
        &req.folder_id,
        &req.message_id,
    )?;

    // 2. Re-upload attachments from the original message
    let attachment_infos = get_attachment_info(
        base_url,
        account_id,
        access_token,
        &req.folder_id,
        &req.message_id,
    )?;
    let mut uploaded: Vec<UploadedAttachment> = Vec::new();
    for info in &attachment_infos {
        let bytes = download_attachment(
            base_url,
            account_id,
            access_token,
            &req.folder_id,
            &req.message_id,
            &info.attachment_id,
        )?;
        let att = upload_attachment(
            base_url,
            account_id,
            access_token,
            &info.attachment_name,
            bytes,
        )?;
        uploaded.push(att);
    }

    // 3. Format forwarded content
    let user_note = if req.content.is_empty() {
        String::new()
    } else {
        format!("<p>{}</p><br/>", req.content)
    };
    let forward_content = format!(
        "{user_note}\
         <p>---------- Forwarded message ----------</p>\
         <p><b>From:</b> {from}</p>\
         <p><b>Date:</b> {date}</p>\
         <p><b>Subject:</b> {subject}</p>\
         <p><b>To:</b> {to}</p>\
         <br/>{body}",
        from = original.sender,
        date = original.received_time,
        subject = original.subject,
        to = original.to_address,
        body = original.content,
    );

    // 4. Send as new message
    let fwd_subject = if original
        .subject
        .to_lowercase()
        .starts_with("fwd:")
    {
        original.subject.clone()
    } else {
        format!("Fwd: {}", original.subject)
    };

    let sent = send_mail_message(
        base_url,
        account_id,
        access_token,
        MailSendRequest {
            from_address: req.from_address,
            to_address: req.to_address,
            cc_address: req.cc_address,
            bcc_address: req.bcc_address,
            subject: fwd_subject,
            content: forward_content,
            mail_format: "html".to_string(),
            attachments: uploaded,
        },
    )?;

    Ok(ForwardedMail {
        message_id: sent.message_id,
    })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ZohoAttachmentInfoData {
    #[serde(default)]
    attachments: Vec<AttachmentInfo>,
}

#[derive(Deserialize)]
struct ZohoUploadData {
    #[serde(alias = "storeName")]
    store_name: String,
    #[serde(alias = "attachmentPath")]
    attachment_path: String,
    #[serde(alias = "attachmentName")]
    attachment_name: String,
}

fn mime_from_filename(name: &str) -> String {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "csv" => "text/csv",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "zip" => "application/zip",
        "gz" | "gzip" => "application/gzip",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Fetch a single message's metadata using the direct details endpoint.
fn fetch_message_summary(
    base_url: &str,
    account_id: &str,
    access_token: &str,
    folder_id: &str,
    message_id: &str,
) -> Result<MailMessageSummary> {
    let client = build_client()?;
    let url = format!(
        "{base_url}/api/accounts/{account_id}/folders/{folder_id}/messages/{message_id}/details"
    );

    let resp = client
        .get(&url)
        .header("Authorization", auth_header(access_token))
        .send()?;

    let body = checked_response_text(resp, "fetch_message_summary")?;
    let parsed: ZohoDataResponse<MailMessageSummary> = serde_json::from_str(&body)
        .map_err(|e| ZocliError::Api(format!("fetch_message_summary: bad JSON: {e}")))?;

    check_zoho_error(&parsed.status, "fetch_message_summary")?;

    parsed.data.ok_or_else(|| {
        ZocliError::Api(format!(
            "message {message_id} not found in folder {folder_id}"
        ))
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_folder_response() {
        let json = r#"{ "data": [
            { "folderId": "123", "folderName": "Inbox", "messageCount": 42, "unreadCount": 5 },
            { "folderId": "456", "folderName": "Sent", "messageCount": 10, "unreadCount": 0 }
        ]}"#;
        let parsed: ZohoDataResponse<Vec<MailFolder>> = serde_json::from_str(json).unwrap();
        let folders = parsed.data.unwrap();
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].folder_id, "123");
        assert_eq!(folders[0].folder_name, "Inbox");
        assert_eq!(folders[0].message_count, 42);
        assert_eq!(folders[0].unread_count, 5);
        assert_eq!(folders[1].folder_id, "456");
    }

    #[test]
    fn deserialize_message_summary_response() {
        let json = r#"{ "data": [{
            "messageId": "msg1",
            "folderId": "f1",
            "sender": "alice@example.com",
            "toAddress": "bob@example.com",
            "subject": "Hello",
            "receivedTime": "1710345600000",
            "isRead": false,
            "hasAttachment": true,
            "summary": "Preview text"
        }]}"#;
        let parsed: ZohoDataResponse<Vec<MailMessageSummary>> = serde_json::from_str(json).unwrap();
        let msgs = parsed.data.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].message_id, "msg1");
        assert_eq!(msgs[0].sender, "alice@example.com");
        assert!(!msgs[0].is_read);
        assert!(msgs[0].has_attachment);
    }

    #[test]
    fn deserialize_content_response() {
        let json = r#"{ "data": { "content": "<p>Hello world</p>" } }"#;
        let parsed: ZohoDataResponse<ZohoContentData> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.data.unwrap().content, "<p>Hello world</p>");
    }

    #[test]
    fn deserialize_sent_response() {
        let json = r#"{ "data": { "messageId": "sent123" } }"#;
        let parsed: ZohoDataResponse<ZohoSentData> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.data.unwrap().message_id, Some("sent123".to_string()));
    }

    #[test]
    fn search_rejects_empty_query() {
        let result = search_mail_messages("http://x", "1", "tok", "", 10);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn mail_folder_serializes_correctly() {
        let folder = MailFolder {
            folder_id: "100".to_string(),
            folder_name: "INBOX".to_string(),
            message_count: 7,
            unread_count: 2,
        };
        let json = serde_json::to_value(&folder).unwrap();
        assert_eq!(json["folder_id"], "100");
        assert_eq!(json["folder_name"], "INBOX");
        assert_eq!(json["message_count"], 7);
        assert_eq!(json["unread_count"], 2);
    }

    #[test]
    fn sent_mail_serializes() {
        let sent = SentMail {
            message_id: "abc".to_string(),
        };
        let json = serde_json::to_value(&sent).unwrap();
        assert_eq!(json["message_id"], "abc");
    }

    #[test]
    fn replied_mail_serializes() {
        let replied = RepliedMail {
            message_id: "def".to_string(),
        };
        let json = serde_json::to_value(&replied).unwrap();
        assert_eq!(json["message_id"], "def");
    }

    #[test]
    fn forwarded_mail_serializes() {
        let forwarded = ForwardedMail {
            message_id: "ghi".to_string(),
        };
        let json = serde_json::to_value(&forwarded).unwrap();
        assert_eq!(json["message_id"], "ghi");
    }

    #[test]
    fn deserialize_message_summary_with_defaults() {
        // Minimal JSON — most fields should default gracefully.
        let json = r#"{ "data": [{ "messageId": "m1" }] }"#;
        let parsed: ZohoDataResponse<Vec<MailMessageSummary>> = serde_json::from_str(json).unwrap();
        let msgs = parsed.data.unwrap();
        assert_eq!(msgs[0].message_id, "m1");
        assert_eq!(msgs[0].folder_id, "");
        assert_eq!(msgs[0].sender, "");
        assert!(!msgs[0].is_read);
        assert!(!msgs[0].has_attachment);
    }

    #[test]
    fn check_zoho_error_passes_on_success_code() {
        let status = Some(ZohoStatus {
            code: 200,
            description: "OK".to_string(),
        });
        assert!(check_zoho_error(&status, "test").is_ok());
    }

    #[test]
    fn check_zoho_error_fails_on_error_code() {
        let status = Some(ZohoStatus {
            code: 400,
            description: "Bad request".to_string(),
        });
        let result = check_zoho_error(&status, "test");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Bad request"));
    }

    #[test]
    fn check_zoho_error_passes_on_none() {
        assert!(check_zoho_error(&None, "test").is_ok());
    }
}
