//! Talk to an office from Slack (issue #33).
//!
//! Edit office → Slack shows a manifest: the owner creates a Slack app from
//! it (its event and button URLs point at this server), installs it to
//! their Slack workspace and pastes the bot token and signing secret here.
//! Both are kept only encrypted (`crypto::encrypt`). The owner then sends
//! the app a direct message with a one-time code (an Argon2 hash, valid an
//! hour), which ties their Slack account to the office. Then, as with
//! Telegram (`crate::telegram`):
//!
//! - their direct messages to the app are posted in the office's group
//!   chat as them, and the bots answer as they do in the app;
//! - the bots' messages in the group chat come back to that conversation;
//! - approvals arrive with Approve and Reject buttons.
//!
//! Slack calls this server (`/slack/events/:office`, `/slack/actions/:office`)
//! and every call is checked against the signing secret (HMAC-SHA256 over
//! `v0:<timestamp>:<body>`, at most five minutes old). That needs a public
//! address, so Slack isn't offered in the desktop app; Telegram is.
//!
//! TODO(air): only the owner's direct messages for now; a channel shared
//! with the team (and invited members' own Slack accounts) could follow.

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{secret, Caller};
use crate::error::AppError;
use crate::handlers::{wants_html, JsonOrForm};
use crate::office::team::{require_person, Role};
use crate::state::{AppState, NoteEvent};

const LINK_MINUTES: i64 = 60;
/// Slack's limit for a message's text is far higher; long ones are cut.
const MAX_TEXT: usize = 3500;
/// How old a signed request from Slack may be.
const MAX_AGE_SECONDS: i64 = 300;

fn client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// One Web API call with the bot token.
async fn call(state: &AppState, token: &str, method: &str, body: Value) -> Result<Value, String> {
    let url = state
        .config
        .connectors
        .url("slack.com", &format!("/api/{method}"));
    let res = client()
        .post(url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Slack couldn't be reached ({})", e.without_url()))?;
    let status = res.status();
    let data: Value = res
        .json()
        .await
        .map_err(|_| format!("Slack answered with an error ({status})"))?;
    if data["ok"].as_bool() == Some(true) {
        Ok(data)
    } else {
        let why = data["error"].as_str().unwrap_or("unknown_error");
        Err(format!(
            "Slack said: {}",
            why.chars().take(100).collect::<String>()
        ))
    }
}

fn cut(text: &str) -> String {
    if text.chars().count() <= MAX_TEXT {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(MAX_TEXT).collect::<String>())
    }
}

#[derive(sqlx::FromRow, Clone)]
struct AppRow {
    id: Uuid,
    workspace_id: Uuid,
    owner_user_id: Uuid,
    team_name: Option<String>,
    token_encrypted: String,
    signing_secret_encrypted: String,
    link_code_hash: Option<String>,
    link_expires_at: Option<DateTime<Utc>>,
    slack_user_id: Option<String>,
    channel_id: Option<String>,
    approvals_sent_until: DateTime<Utc>,
    last_error: Option<String>,
}

const APP_COLUMNS: &str = "id, workspace_id, owner_user_id, team_name, token_encrypted, \
     signing_secret_encrypted, link_code_hash, link_expires_at, slack_user_id, channel_id, \
     approvals_sent_until, last_error";

async fn app_of_office(pool: &PgPool, office: Uuid) -> Result<Option<AppRow>, sqlx::Error> {
    sqlx::query_as::<_, AppRow>(&format!(
        "SELECT {APP_COLUMNS} FROM slack_apps WHERE workspace_id = $1 AND deleted_at IS NULL"
    ))
    .bind(office)
    .fetch_optional(pool)
    .await
}

async fn note_error(pool: &PgPool, app: Uuid, error: Option<&str>) {
    let error = error.map(|e| e.chars().take(300).collect::<String>());
    if let Err(e) =
        sqlx::query("UPDATE slack_apps SET last_error = $2, updated_at = now() WHERE id = $1")
            .bind(app)
            .bind(error)
            .execute(pool)
            .await
    {
        tracing::warn!(error = %e, "could not save a Slack error");
    }
}

fn new_code() -> Result<(String, String), AppError> {
    // Short enough to type: 8 hex characters, an hour, once.
    let code = secret::random_hex_token(4);
    let hash = secret::hash_secret(&code).map_err(|e| {
        tracing::error!(error = %e, "could not hash a Slack link code");
        AppError::Internal
    })?;
    Ok((code, hash))
}

fn urls(state: &AppState, office: Uuid) -> (String, String) {
    let base = state.config.app_base_url.trim_end_matches('/');
    (
        format!("{base}/slack/events/{office}"),
        format!("{base}/slack/actions/{office}"),
    )
}

/// The app manifest to create the Slack app from.
fn manifest(name: &str, events_url: &str, actions_url: &str) -> Value {
    // "Corner Bakery office", but not "My office office".
    let full = if name.trim().to_lowercase().ends_with("office") {
        name.trim().to_string()
    } else {
        format!("{} office", name.trim())
    };
    let name: String = full.chars().take(35).collect();
    json!({
        "display_information": { "name": name },
        "features": {
            "bot_user": { "display_name": name, "always_online": true },
            "app_home": { "messages_tab_enabled": true, "messages_tab_read_only_enabled": false }
        },
        "oauth_config": { "scopes": { "bot": ["chat:write", "im:history", "im:read", "im:write"] } },
        "settings": {
            "event_subscriptions": { "request_url": events_url, "bot_events": ["message.im"] },
            "interactivity": { "is_enabled": true, "request_url": actions_url },
            "org_deploy_enabled": false,
            "socket_mode_enabled": false
        }
    })
}

fn not_on_this_computer(state: &AppState) -> Result<(), AppError> {
    if state.config.local.is_some() {
        return Err(AppError::BadRequest(
            "Slack needs Airobot on a server Slack can reach. On this computer, use Telegram."
                .into(),
        ));
    }
    Ok(())
}

// ---------- HTTP (the app) ----------

/// GET /api/offices/:id/slack -- whether Slack is connected and linked,
/// and the manifest to create the Slack app from.
pub async fn status(
    State(state): State<AppState>,
    caller: Caller,
    Path(office): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    require_person(&state.pool, &caller, office, Role::Owner).await?;
    let app = app_of_office(&state.pool, office).await?;
    let name: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id = $1")
        .bind(office)
        .fetch_one(&state.pool)
        .await?;
    let (events_url, actions_url) = urls(&state, office);
    Ok(Json(json!({
        "available": state.config.local.is_none(),
        "connected": app.is_some(),
        "team_name": app.as_ref().and_then(|a| a.team_name.clone()),
        "linked": app.as_ref().is_some_and(|a| a.channel_id.is_some()),
        "last_error": app.as_ref().and_then(|a| a.last_error.clone()),
        "manifest": serde_json::to_string_pretty(&manifest(&name, &events_url, &actions_url))
            .unwrap_or_default(),
    })))
}

#[derive(Deserialize)]
pub struct ConnectRequest {
    pub bot_token: String,
    pub signing_secret: String,
    pub idempotency_key: Option<String>,
}

/// POST /api/offices/:id/slack -- connect the installed app. Answers with
/// the code to send it in a direct message.
pub async fn connect(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<ConnectRequest>,
) -> Result<Response, AppError> {
    require_person(&state.pool, &caller, office, Role::Owner).await?;
    not_on_this_computer(&state)?;
    let key = crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    if let Some(key) = key {
        let earlier: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM slack_apps WHERE workspace_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL",
        )
        .bind(office)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?;
        if earlier.is_some() {
            return Ok(Json(json!({ "connected": true, "repeated": true })).into_response());
        }
    }
    let token = body.bot_token.trim().to_string();
    let signing = body.signing_secret.trim().to_string();
    if !token.starts_with("xoxb-") || token.len() > 200 {
        return Err(AppError::BadRequest(
            "paste the Bot User OAuth Token (it starts with xoxb-)".into(),
        ));
    }
    if signing.len() < 16 || signing.len() > 100 || !signing.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(AppError::BadRequest(
            "paste the Signing Secret from Basic Information".into(),
        ));
    }
    let me = call(&state, &token, "auth.test", json!({}))
        .await
        .map_err(|e| AppError::BadRequest(format!("That token didn't work. {e}")))?;
    let (Some(team_id), Some(bot_user)) = (me["team_id"].as_str(), me["user_id"].as_str()) else {
        return Err(AppError::BadRequest(
            "Slack didn't say which app that is".into(),
        ));
    };
    let encrypt = |plain: &str| {
        crate::crypto::encrypt(plain).ok_or_else(|| {
            tracing::error!("could not encrypt a Slack credential");
            AppError::Internal
        })
    };
    let (token_enc, signing_enc) = (encrypt(&token)?, encrypt(&signing)?);
    let (code, hash) = new_code()?;
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "UPDATE slack_apps SET deleted_at = now(), updated_at = now()
         WHERE workspace_id = $1 AND deleted_at IS NULL",
    )
    .bind(office)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO slack_apps (workspace_id, owner_user_id, team_id, team_name, bot_user_id,
                                 token_encrypted, signing_secret_encrypted, link_code_hash,
                                 link_expires_at, idempotency_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now() + make_interval(mins => $9), $10)",
    )
    .bind(office)
    .bind(caller.user_id())
    .bind(team_id)
    .bind(me["team"].as_str())
    .bind(bot_user)
    .bind(&token_enc)
    .bind(&signing_enc)
    .bind(&hash)
    .bind(LINK_MINUTES as i32)
    .bind(key)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "connected": true,
            "team_name": me["team"].as_str(),
            "linked": false,
            "link_code": code,
        })),
    )
        .into_response())
}

/// POST /api/offices/:id/slack/link -- a new code (the old one stops).
pub async fn new_link(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
) -> Result<Response, AppError> {
    require_person(&state.pool, &caller, office, Role::Owner).await?;
    crate::idempotency::key(&headers, None)?;
    let app = app_of_office(&state.pool, office)
        .await?
        .ok_or(AppError::NotFound)?;
    let (code, hash) = new_code()?;
    sqlx::query(
        "UPDATE slack_apps SET link_code_hash = $2,
                link_expires_at = now() + make_interval(mins => $3), updated_at = now()
         WHERE id = $1",
    )
    .bind(app.id)
    .bind(&hash)
    .bind(LINK_MINUTES as i32)
    .execute(&state.pool)
    .await?;
    Ok(Json(json!({ "link_code": code })).into_response())
}

/// POST /api/offices/:id/slack/delete -- disconnect (the Slack app itself
/// stays in Slack until removed there).
pub async fn disconnect(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
) -> Result<Response, AppError> {
    require_person(&state.pool, &caller, office, Role::Owner).await?;
    crate::idempotency::key(&headers, None)?;
    sqlx::query(
        "UPDATE slack_apps SET deleted_at = now(), updated_at = now()
         WHERE workspace_id = $1 AND deleted_at IS NULL",
    )
    .bind(office)
    .execute(&state.pool)
    .await?;
    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    Ok(Json(json!({ "connected": false })).into_response())
}

// ---------- Calls from Slack ----------

/// Whether `body` was signed by Slack with `signing_secret`, recently.
pub fn signed_by_slack(signing_secret: &str, headers: &HeaderMap, body: &[u8], now: i64) -> bool {
    use hmac::Mac;
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let (Some(ts), Some(sig)) = (
        header("x-slack-request-timestamp"),
        header("x-slack-signature"),
    ) else {
        return false;
    };
    let Ok(at) = ts.parse::<i64>() else {
        return false;
    };
    if (now - at).abs() > MAX_AGE_SECONDS {
        return false;
    }
    let Some(given) = sig.strip_prefix("v0=").and_then(from_hex) else {
        return false;
    };
    let Ok(mut mac) = <hmac::Hmac<Sha256> as Mac>::new_from_slice(signing_secret.as_bytes()) else {
        return false;
    };
    mac.update(format!("v0:{ts}:").as_bytes());
    mac.update(body);
    mac.verify_slice(&given).is_ok()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// `payload=...` from a form body, decoded.
fn form_value(body: &[u8], name: &str) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    let raw = text
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{name}=")))?;
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = bytes.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// The office's app, if the request is really from its Slack app.
async fn verified(
    state: &AppState,
    office: Uuid,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<AppRow, AppError> {
    let app = app_of_office(&state.pool, office)
        .await?
        .ok_or(AppError::NotFound)?;
    let secret =
        crate::crypto::decrypt(&app.signing_secret_encrypted).ok_or(AppError::Forbidden)?;
    if !signed_by_slack(&secret, headers, body, Utc::now().timestamp()) {
        return Err(AppError::Forbidden);
    }
    Ok(app)
}

/// POST /slack/events/:office -- Slack's Events API.
pub async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
    body: Bytes,
) -> Result<Response, AppError> {
    let event: Value =
        serde_json::from_slice(&body).map_err(|_| AppError::BadRequest("not JSON".into()))?;
    // Setting the URL in Slack checks it before the app is connected here;
    // echoing the challenge reveals nothing.
    if event["type"] == "url_verification" {
        return Ok(Json(json!({ "challenge": event["challenge"] })).into_response());
    }
    let app = verified(&state, office, &headers, &body).await?;
    // Slack retries when an answer is slow; the first delivery is handled.
    if headers.contains_key("x-slack-retry-num") {
        return Ok(StatusCode::OK.into_response());
    }
    if event["type"] == "event_callback" {
        let inner = event["event"].clone();
        let event_id = event["event_id"].as_str().unwrap_or_default().to_string();
        // Answered at once (Slack wants a reply within 3 seconds); handled after.
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_message(&state, &app, &event_id, &inner).await {
                tracing::warn!(error = %e, "a Slack message wasn't handled");
                note_error(&state.pool, app.id, Some(&e)).await;
            }
        });
    }
    Ok(StatusCode::OK.into_response())
}

/// A direct message to the app: the link code, or the owner talking to
/// the office.
async fn handle_message(
    state: &AppState,
    app: &AppRow,
    event_id: &str,
    event: &Value,
) -> Result<(), String> {
    // Only people's direct messages (not the app's own, not edits).
    if event["type"] != "message"
        || event["channel_type"] != "im"
        || event.get("bot_id").is_some_and(|v| !v.is_null())
        || event.get("subtype").is_some_and(|v| !v.is_null())
    {
        return Ok(());
    }
    let (Some(user), Some(channel)) = (event["user"].as_str(), event["channel"].as_str()) else {
        return Ok(());
    };
    let text = event["text"].as_str().unwrap_or("").trim();
    let token =
        crate::crypto::decrypt(&app.token_encrypted).ok_or("the saved token can't be read")?;
    let reply = |text: String| {
        let token = token.clone();
        let channel = channel.to_string();
        async move {
            call(
                state,
                &token,
                "chat.postMessage",
                json!({ "channel": channel, "text": cut(&text) }),
            )
            .await
        }
    };

    let code = text
        .strip_prefix("link ")
        .or_else(|| text.strip_prefix("Link "))
        .map(str::trim);
    if let Some(code) = code {
        let valid = app.link_expires_at.is_some_and(|t| t > Utc::now())
            && app
                .link_code_hash
                .as_deref()
                .is_some_and(|hash| secret::verify_secret(code, hash));
        if valid {
            sqlx::query(
                "UPDATE slack_apps SET slack_user_id = $2, channel_id = $3, linked_at = now(),
                        link_code_hash = NULL, link_expires_at = NULL, last_error = NULL,
                        updated_at = now()
                 WHERE id = $1",
            )
            .bind(app.id)
            .bind(user)
            .bind(channel)
            .execute(&state.pool)
            .await
            .map_err(|e| e.to_string())?;
            let name: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id = $1")
                .bind(app.workspace_id)
                .fetch_one(&state.pool)
                .await
                .map_err(|e| e.to_string())?;
            reply(format!(
                "Linked to {name}. Write here to talk to the office: the Architect answers or hands it to the right bot. Approvals come here with buttons."
            ))
            .await?;
            return Ok(());
        }
    }
    if app.slack_user_id.as_deref() != Some(user) {
        reply("This app belongs to an Airobot office and only talks to its owner.".to_string())
            .await?;
        return Ok(());
    }
    if text.is_empty() {
        return Ok(());
    }
    crate::chat_apps::post_in_office_chat(
        state,
        app.owner_user_id,
        app.workspace_id,
        text,
        &format!("slack:{}:{event_id}", app.id),
    )
    .await
}

/// POST /slack/actions/:office -- Approve or Reject pressed.
pub async fn actions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
    body: Bytes,
) -> Result<Response, AppError> {
    let app = verified(&state, office, &headers, &body).await?;
    let payload: Value = form_value(&body, "payload")
        .and_then(|p| serde_json::from_str(&p).ok())
        .ok_or_else(|| AppError::BadRequest("no payload".into()))?;
    if payload["type"] != "block_actions" {
        return Ok(StatusCode::OK.into_response());
    }
    let action = &payload["actions"][0];
    let parsed = action["value"]
        .as_str()
        .and_then(|v| v.split_once(':'))
        .and_then(|(yes, id)| Some((yes == "y", id.parse::<Uuid>().ok()?)));
    let reply_text = if payload["user"]["id"].as_str() != app.slack_user_id.as_deref() {
        "Only this office's owner can decide its approvals.".to_string()
    } else if let Some((approve, approval)) = parsed {
        match crate::chat_apps::decide_as_owner(
            &state,
            app.owner_user_id,
            app.workspace_id,
            approval,
            approve,
        )
        .await
        {
            Ok(()) if approve => "✓ Approved".to_string(),
            Ok(()) => "✗ Rejected".to_string(),
            Err(why) => why.to_string(),
        }
    } else {
        "That button is out of date.".to_string()
    };
    // The message shows the outcome instead of the buttons.
    if let (Some(token), Some(channel), Some(ts)) = (
        crate::crypto::decrypt(&app.token_encrypted),
        payload["container"]["channel_id"].as_str(),
        payload["container"]["message_ts"].as_str(),
    ) {
        let original = payload["message"]["text"].as_str().unwrap_or("Approval");
        if let Err(e) = call(
            &state,
            &token,
            "chat.update",
            json!({ "channel": channel, "ts": ts, "text": cut(&format!("{original}\n\n{reply_text}")), "blocks": [] }),
        )
        .await
        {
            note_error(&state.pool, app.id, Some(&e)).await;
        }
    }
    Ok(StatusCode::OK.into_response())
}

// ---------- Messages to Slack ----------

/// A bot's message in an office group chat goes to the owner's direct
/// messages with the office's Slack app.
pub async fn relay_note(state: &AppState, event: &NoteEvent) {
    if event.note["author_kind"].as_str() != Some("agent") {
        return;
    }
    let Some(office) = crate::chat_apps::office_of_chat(&state.pool, event.thread_id).await else {
        return;
    };
    let Ok(Some(app)) = app_of_office(&state.pool, office).await else {
        return;
    };
    let (Some(channel), Some(token), Some(body)) = (
        app.channel_id.clone(),
        crate::crypto::decrypt(&app.token_encrypted),
        event.note["body"].as_str(),
    ) else {
        return;
    };
    let who = event.note["author_name"].as_str().unwrap_or("Your bot");
    if let Err(e) = call(
        state,
        &token,
        "chat.postMessage",
        json!({ "channel": channel, "text": cut(&format!("*{who}:* {body}")) }),
    )
    .await
    {
        note_error(&state.pool, app.id, Some(&e)).await;
    }
}

/// New approvals in linked offices, with Approve and Reject buttons.
pub async fn send_approvals(state: &AppState) -> usize {
    let apps: Vec<AppRow> = sqlx::query_as::<_, AppRow>(&format!(
        "SELECT {APP_COLUMNS} FROM slack_apps WHERE channel_id IS NOT NULL AND deleted_at IS NULL"
    ))
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    let mut sent = 0;
    for app in apps {
        let (Some(token), Some(channel)) = (
            crate::crypto::decrypt(&app.token_encrypted),
            app.channel_id.clone(),
        ) else {
            continue;
        };
        let mut until = app.approvals_sent_until;
        for item in crate::chat_apps::approvals_since(&state.pool, app.workspace_id, until).await {
            let text = cut(&format!(
                "Approval needed\n{}\nAsked by {}",
                item.action, item.asked_by
            ));
            let res = call(
                state,
                &token,
                "chat.postMessage",
                json!({
                    "channel": channel,
                    "text": text,
                    "blocks": [
                        { "type": "section", "text": { "type": "mrkdwn", "text": text } },
                        { "type": "actions", "elements": [
                            { "type": "button", "action_id": "approve", "style": "primary",
                              "text": { "type": "plain_text", "text": "Approve" }, "value": format!("y:{}", item.id) },
                            { "type": "button", "action_id": "reject", "style": "danger",
                              "text": { "type": "plain_text", "text": "Reject" }, "value": format!("n:{}", item.id) }
                        ]}
                    ],
                }),
            )
            .await;
            match res {
                Ok(_) => {
                    sent += 1;
                    until = item.created_at;
                }
                Err(e) => {
                    note_error(&state.pool, app.id, Some(&e)).await;
                    break;
                }
            }
        }
        if until != app.approvals_sent_until {
            let _ = sqlx::query(
                "UPDATE slack_apps SET approvals_sent_until = $2, updated_at = now() WHERE id = $1",
            )
            .bind(app.id)
            .bind(until)
            .execute(&state.pool)
            .await;
        }
    }
    sent
}

/// The bots' messages to Slack as they come, and new approvals every 20
/// seconds.
pub async fn run_loop(state: AppState) {
    let relay_state = state.clone();
    tokio::spawn(async move {
        let mut events = relay_state.note_sender.subscribe();
        loop {
            match events.recv().await {
                Ok(event) => relay_note(&relay_state, &event).await,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
    loop {
        send_approvals(&state).await;
        tokio::time::sleep(Duration::from_secs(20)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::Mac;

    fn sign(secret: &str, ts: i64, body: &str) -> String {
        let mut mac = <hmac::Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).expect("key");
        mac.update(format!("v0:{ts}:{body}").as_bytes());
        let bytes = mac.finalize().into_bytes();
        format!(
            "v0={}",
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        )
    }

    fn headers(ts: i64, sig: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            "x-slack-request-timestamp",
            ts.to_string().parse().expect("ts"),
        );
        h.insert("x-slack-signature", sig.parse().expect("sig"));
        h
    }

    #[test]
    fn only_fresh_requests_signed_with_the_secret_pass() {
        let body = r#"{"type":"event_callback"}"#;
        let now = 1_800_000_000;
        let good = sign("8f742231b10e8888abcd99yyyzzz85a5", now, body);
        assert!(signed_by_slack(
            "8f742231b10e8888abcd99yyyzzz85a5",
            &headers(now, &good),
            body.as_bytes(),
            now
        ));
        assert!(!signed_by_slack(
            "another-secret",
            &headers(now, &good),
            body.as_bytes(),
            now
        ));
        assert!(!signed_by_slack(
            "8f742231b10e8888abcd99yyyzzz85a5",
            &headers(now, &good),
            b"{}",
            now
        ));
        assert!(
            !signed_by_slack(
                "8f742231b10e8888abcd99yyyzzz85a5",
                &headers(now, &good),
                body.as_bytes(),
                now + 301
            ),
            "too old"
        );
        assert!(!signed_by_slack(
            "x",
            &HeaderMap::new(),
            body.as_bytes(),
            now
        ));
    }

    #[test]
    fn form_payloads_are_decoded() {
        assert_eq!(
            form_value(b"payload=%7B%22a%22%3A+1%7D&x=2", "payload").as_deref(),
            Some(r#"{"a": 1}"#)
        );
        assert_eq!(form_value(b"x=2", "payload"), None);
        assert_eq!(
            form_value(b"payload=%7", "payload"),
            None,
            "a cut-off escape"
        );
    }

    #[test]
    fn the_manifest_points_at_this_server() {
        let m = manifest(
            "Corner Bakery",
            "https://a.example/slack/events/1",
            "https://a.example/slack/actions/1",
        );
        assert_eq!(
            m["settings"]["event_subscriptions"]["request_url"],
            "https://a.example/slack/events/1"
        );
        assert_eq!(
            m["settings"]["interactivity"]["request_url"],
            "https://a.example/slack/actions/1"
        );
        assert_eq!(m["display_information"]["name"], "Corner Bakery office");
        let m = manifest("My office", "e", "a");
        assert_eq!(m["display_information"]["name"], "My office");
    }
}
