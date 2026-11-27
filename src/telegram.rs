//! Talk to an office from Telegram (issue #33).
//!
//! The owner makes a bot with @BotFather and pastes its token in Edit office
//! → Telegram. The token is kept only encrypted (`crypto::encrypt`): the
//! server must present it to Telegram. A one-time link (`/start <code>`,
//! the code kept as an Argon2 hash, valid an hour) ties the owner's own
//! Telegram chat to the office. Then:
//!
//! - what they write there is posted in the office's group chat as them,
//!   and the bots answer as they do in the app;
//! - the bots' messages in the group chat (answers, the morning plan) are
//!   sent to Telegram;
//! - approvals arrive with Approve and Reject buttons.
//!
//! The server asks Telegram for new messages itself (long polling), so it
//! needs no public address and works in the desktop app too. Other chats
//! that find the bot are told it is private.
//!
//! TODO(air): messages arriving while the server is down are fetched when
//! it starts again, but bot messages posted then are not sent (the relay
//! follows the live stream). Slack and WhatsApp come later.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{secret, Caller};
use crate::error::AppError;
use crate::handlers::{wants_html, JsonOrForm};
use crate::office::team::{require_person, Role};
use crate::state::{AppState, NoteEvent};

/// How long a link to the bot works.
const LINK_MINUTES: i64 = 60;
/// How long one request for new messages waits at Telegram.
const POLL_SECONDS: u64 = 25;
/// Telegram's limit for one message.
const MAX_TEXT: usize = 4000;

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            // Longer than a long poll.
            .timeout(Duration::from_secs(POLL_SECONDS + 15))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// One Bot API call. Errors never carry the URL: it holds the token.
async fn call(state: &AppState, token: &str, method: &str, body: Value) -> Result<Value, String> {
    let url = state
        .config
        .connectors
        .url("api.telegram.org", &format!("/bot{token}/{method}"));
    let res = client()
        .post(url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Telegram couldn't be reached ({})", e.without_url()))?;
    let status = res.status();
    let data: Value = res
        .json()
        .await
        .map_err(|_| format!("Telegram answered with an error ({status})"))?;
    if data["ok"].as_bool() == Some(true) {
        Ok(data["result"].clone())
    } else {
        let why = data["description"].as_str().unwrap_or("unknown error");
        Err(format!(
            "Telegram said: {}",
            why.chars().take(200).collect::<String>()
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
struct BotRow {
    id: Uuid,
    workspace_id: Uuid,
    owner_user_id: Uuid,
    bot_username: String,
    token_encrypted: String,
    link_code_hash: Option<String>,
    link_expires_at: Option<DateTime<Utc>>,
    chat_id: Option<i64>,
    update_offset: i64,
    approvals_sent_until: DateTime<Utc>,
}

const BOT_COLUMNS: &str = "id, workspace_id, owner_user_id, bot_username, token_encrypted, \
     link_code_hash, link_expires_at, chat_id, update_offset, approvals_sent_until";

async fn bot_by_id(pool: &PgPool, id: Uuid) -> Result<Option<BotRow>, sqlx::Error> {
    sqlx::query_as::<_, BotRow>(&format!(
        "SELECT {BOT_COLUMNS} FROM telegram_bots WHERE id = $1 AND deleted_at IS NULL"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
}

async fn bot_of_office(pool: &PgPool, office: Uuid) -> Result<Option<BotRow>, sqlx::Error> {
    sqlx::query_as::<_, BotRow>(&format!(
        "SELECT {BOT_COLUMNS} FROM telegram_bots WHERE workspace_id = $1 AND deleted_at IS NULL"
    ))
    .bind(office)
    .fetch_optional(pool)
    .await
}

fn token_of(bot: &BotRow) -> Option<String> {
    crate::crypto::decrypt(&bot.token_encrypted)
}

async fn note_error(pool: &PgPool, bot: Uuid, error: Option<&str>) {
    let error = error.map(|e| e.chars().take(300).collect::<String>());
    if let Err(e) =
        sqlx::query("UPDATE telegram_bots SET last_error = $2, updated_at = now() WHERE id = $1")
            .bind(bot)
            .bind(error)
            .execute(pool)
            .await
    {
        tracing::warn!(error = %e, "could not save a Telegram error");
    }
}

/// A fresh one-time link code: (code, its hash).
fn new_code() -> Result<(String, String), AppError> {
    let code = secret::random_hex_token(16);
    let hash = secret::hash_secret(&code).map_err(|e| {
        tracing::error!(error = %e, "could not hash a Telegram link code");
        AppError::Internal
    })?;
    Ok((code, hash))
}

fn link_url(username: &str, code: &str) -> String {
    format!("https://t.me/{username}?start={code}")
}

// ---------- HTTP ----------

fn status_json(bot: Option<&BotRow>, last_error: Option<String>) -> Value {
    match bot {
        None => json!({ "connected": false }),
        Some(b) => json!({
            "connected": true,
            "bot_username": b.bot_username,
            "linked": b.chat_id.is_some(),
            "last_error": last_error,
        }),
    }
}

/// GET /api/offices/:id/telegram
pub async fn status(
    State(state): State<AppState>,
    caller: Caller,
    Path(office): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    require_person(&state.pool, &caller, office, Role::Owner).await?;
    let bot = bot_of_office(&state.pool, office).await?;
    let last_error: Option<String> = match &bot {
        Some(b) => {
            sqlx::query_scalar("SELECT last_error FROM telegram_bots WHERE id = $1")
                .bind(b.id)
                .fetch_one(&state.pool)
                .await?
        }
        None => None,
    };
    Ok(Json(status_json(bot.as_ref(), last_error)))
}

#[derive(Deserialize)]
pub struct ConnectRequest {
    pub token: String,
    pub idempotency_key: Option<String>,
}

/// POST /api/offices/:id/telegram -- connect a bot by its token. Answers
/// with the link that ties the owner's Telegram chat to the office.
pub async fn connect(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<ConnectRequest>,
) -> Result<Response, AppError> {
    require_person(&state.pool, &caller, office, Role::Owner).await?;
    let key = crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    if let Some(key) = key {
        let earlier: Option<BotRow> = sqlx::query_as::<_, BotRow>(&format!(
            "SELECT {BOT_COLUMNS} FROM telegram_bots
             WHERE workspace_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL"
        ))
        .bind(office)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?;
        if let Some(bot) = earlier {
            return Ok(Json(status_json(Some(&bot), None)).into_response());
        }
    }
    // "123456789:AA..." as @BotFather gives it.
    let token = body.token.trim().to_string();
    let well_formed = token.len() <= 100
        && token.split_once(':').is_some_and(|(id, rest)| {
            !id.is_empty()
                && id.chars().all(|c| c.is_ascii_digit())
                && rest.len() >= 20
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        });
    if !well_formed {
        return Err(AppError::BadRequest(
            "paste the token @BotFather gave you (it looks like 123456789:AA...)".into(),
        ));
    }
    let me = call(&state, &token, "getMe", json!({}))
        .await
        .map_err(|e| AppError::BadRequest(format!("That token didn't work. {e}")))?;
    let (Some(bot_id), Some(username)) = (me["id"].as_i64(), me["username"].as_str()) else {
        return Err(AppError::BadRequest(
            "Telegram didn't say which bot that is".into(),
        ));
    };
    let encrypted = crate::crypto::encrypt(&token).ok_or_else(|| {
        tracing::error!("could not encrypt a Telegram token");
        AppError::Internal
    })?;
    let (code, hash) = new_code()?;
    let mut tx = state.pool.begin().await?;
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM telegram_bots
                       WHERE bot_id = $1 AND workspace_id <> $2 AND deleted_at IS NULL)",
    )
    .bind(bot_id)
    .bind(office)
    .fetch_one(&mut *tx)
    .await?;
    if taken {
        return Err(AppError::Conflict(
            "that bot is connected to another office; make a new bot with @BotFather".into(),
        ));
    }
    // A new token replaces the office's bot.
    sqlx::query(
        "UPDATE telegram_bots SET deleted_at = now(), updated_at = now()
         WHERE workspace_id = $1 AND deleted_at IS NULL",
    )
    .bind(office)
    .execute(&mut *tx)
    .await?;
    let user_id = caller.user_id();
    let bot = sqlx::query_as::<_, BotRow>(&format!(
        "INSERT INTO telegram_bots (workspace_id, owner_user_id, bot_id, bot_username,
                                    token_encrypted, link_code_hash, link_expires_at,
                                    idempotency_key)
         VALUES ($1, $2, $3, $4, $5, $6, now() + make_interval(mins => $7), $8)
         RETURNING {BOT_COLUMNS}"
    ))
    .bind(office)
    .bind(user_id)
    .bind(bot_id)
    .bind(username)
    .bind(&encrypted)
    .bind(&hash)
    .bind(LINK_MINUTES as i32)
    .bind(key)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    let mut out = status_json(Some(&bot), None);
    out["link_url"] = json!(link_url(username, &code));
    Ok((StatusCode::CREATED, Json(out)).into_response())
}

/// POST /api/offices/:id/telegram/link -- a new link (the old one stops
/// working), e.g. to link a new phone.
pub async fn new_link(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
) -> Result<Response, AppError> {
    require_person(&state.pool, &caller, office, Role::Owner).await?;
    crate::idempotency::key(&headers, None)?;
    let bot = bot_of_office(&state.pool, office)
        .await?
        .ok_or(AppError::NotFound)?;
    let (code, hash) = new_code()?;
    sqlx::query(
        "UPDATE telegram_bots SET link_code_hash = $2,
                link_expires_at = now() + make_interval(mins => $3), updated_at = now()
         WHERE id = $1",
    )
    .bind(bot.id)
    .bind(&hash)
    .bind(LINK_MINUTES as i32)
    .execute(&state.pool)
    .await?;
    Ok(Json(json!({ "link_url": link_url(&bot.bot_username, &code) })).into_response())
}

/// POST /api/offices/:id/telegram/delete -- disconnect. Telegram keeps the
/// bot itself; @BotFather can delete it.
pub async fn disconnect(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
) -> Result<Response, AppError> {
    require_person(&state.pool, &caller, office, Role::Owner).await?;
    crate::idempotency::key(&headers, None)?;
    sqlx::query(
        "UPDATE telegram_bots SET deleted_at = now(), updated_at = now()
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

// ---------- Messages from Telegram ----------

async fn say(state: &AppState, token: &str, chat: i64, text: &str) -> Result<Value, String> {
    call(
        state,
        token,
        "sendMessage",
        json!({ "chat_id": chat, "text": cut(text) }),
    )
    .await
}

async fn office_name(pool: &PgPool, office: Uuid) -> String {
    sqlx::query_scalar("SELECT name FROM workspaces WHERE id = $1")
        .bind(office)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "your office".to_string())
}

/// Asks Telegram once for new messages to this bot and handles them;
/// `wait` is the long-poll time. Returns how many were handled.
pub async fn poll_once(state: &AppState, bot_row: Uuid, wait: u64) -> Result<usize, String> {
    let bot = bot_by_id(&state.pool, bot_row)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("disconnected")?;
    let token = token_of(&bot).ok_or("the saved token can't be read")?;
    let updates = call(
        state,
        &token,
        "getUpdates",
        json!({
            "offset": bot.update_offset,
            "timeout": wait,
            "allowed_updates": ["message", "callback_query"],
        }),
    )
    .await?;
    let list = updates.as_array().cloned().unwrap_or_default();
    let mut next = bot.update_offset;
    for update in &list {
        let Some(id) = update["update_id"].as_i64() else {
            continue;
        };
        next = next.max(id + 1);
        // Re-read: a /start in this batch may have linked the chat.
        let Ok(Some(current)) = bot_by_id(&state.pool, bot.id).await else {
            break;
        };
        if let Err(e) = handle_update(state, &current, &token, id, update).await {
            tracing::warn!(error = %e, "a Telegram message wasn't handled");
        }
    }
    if next != bot.update_offset {
        sqlx::query(
            "UPDATE telegram_bots SET update_offset = $2, updated_at = now() WHERE id = $1",
        )
        .bind(bot.id)
        .bind(next)
        .execute(&state.pool)
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(list.len())
}

async fn handle_update(
    state: &AppState,
    bot: &BotRow,
    token: &str,
    update_id: i64,
    update: &Value,
) -> Result<(), String> {
    if let Some(query) = update.get("callback_query") {
        return handle_button(state, bot, token, query).await;
    }
    let Some(message) = update.get("message") else {
        return Ok(());
    };
    let Some(chat) = message["chat"]["id"].as_i64() else {
        return Ok(());
    };
    // Only one-to-one chats with the bot; groups are told it's private.
    let private = message["chat"]["type"].as_str() == Some("private");
    let text = message["text"].as_str().unwrap_or("").trim();

    if let Some(code) = text.strip_prefix("/start") {
        let code = code.trim();
        let valid = private
            && !code.is_empty()
            && bot.link_expires_at.is_some_and(|t| t > Utc::now())
            && bot
                .link_code_hash
                .as_deref()
                .is_some_and(|hash| secret::verify_secret(code, hash));
        if valid {
            sqlx::query(
                "UPDATE telegram_bots SET chat_id = $2, linked_at = now(), link_code_hash = NULL,
                        link_expires_at = NULL, last_error = NULL, updated_at = now()
                 WHERE id = $1",
            )
            .bind(bot.id)
            .bind(chat)
            .execute(&state.pool)
            .await
            .map_err(|e| e.to_string())?;
            let name = office_name(&state.pool, bot.workspace_id).await;
            say(
                state,
                token,
                chat,
                &format!(
                    "Linked to {name}. Write here to talk to the office: the Architect answers or hands it to the right bot. Approvals come here with buttons."
                ),
            )
            .await?;
            return Ok(());
        }
        if bot.chat_id == Some(chat) {
            say(
                state,
                token,
                chat,
                "You're already linked. Write to talk to the office.",
            )
            .await?;
            return Ok(());
        }
        say(
            state,
            token,
            chat,
            "This bot belongs to an Airobot office. Its owner links it from the app.",
        )
        .await?;
        return Ok(());
    }

    if bot.chat_id != Some(chat) || !private {
        say(
            state,
            token,
            chat,
            "This bot belongs to an Airobot office and only talks to its owner.",
        )
        .await?;
        return Ok(());
    }
    if text.is_empty() {
        say(state, token, chat, "I can read text messages for now.").await?;
        return Ok(());
    }
    // Posted in the office chat as the owner; the bots answer there and
    // their answers come back here.
    crate::chat_apps::post_in_office_chat(
        state,
        bot.owner_user_id,
        bot.workspace_id,
        text,
        &format!("telegram:{}:{update_id}", bot.id),
    )
    .await?;
    Ok(())
}

/// Approve or Reject pressed under an approval.
async fn handle_button(
    state: &AppState,
    bot: &BotRow,
    token: &str,
    query: &Value,
) -> Result<(), String> {
    let query_id = query["id"].as_str().unwrap_or_default().to_string();
    let chat = query["message"]["chat"]["id"].as_i64();
    let answer = |text: &'static str| {
        call(
            state,
            token,
            "answerCallbackQuery",
            json!({ "callback_query_id": query_id, "text": text }),
        )
    };
    if chat.is_none() || chat != bot.chat_id {
        answer("This isn't your office.").await?;
        return Ok(());
    }
    let data = query["data"].as_str().unwrap_or_default();
    let parsed = data
        .strip_prefix("ap:")
        .and_then(|rest| rest.split_once(':'))
        .and_then(|(id, yes)| Some((id.parse::<Uuid>().ok()?, yes == "y")));
    let Some((approval, approve)) = parsed else {
        answer("That button is out of date.").await?;
        return Ok(());
    };
    // The same decision the app makes, as the owner.
    let decided = crate::chat_apps::decide_as_owner(
        state,
        bot.owner_user_id,
        bot.workspace_id,
        approval,
        approve,
    )
    .await;
    let (toast, line) = match decided {
        Ok(_) if approve => ("Approved", "✓ Approved"),
        Ok(_) => ("Rejected", "✗ Rejected"),
        Err(why) => (why, ""),
    };
    answer(toast).await?;
    if !line.is_empty() {
        if let (Some(message_id), Some(text)) = (
            query["message"]["message_id"].as_i64(),
            query["message"]["text"].as_str(),
        ) {
            call(
                state,
                token,
                "editMessageText",
                json!({
                    "chat_id": chat,
                    "message_id": message_id,
                    "text": cut(&format!("{text}\n\n{line}")),
                }),
            )
            .await?;
        }
    }
    Ok(())
}

// ---------- Messages to Telegram ----------

/// A bot's message in an office group chat goes to that office's linked
/// Telegram chat. People's messages don't (they wrote them).
pub async fn relay_note(state: &AppState, event: &NoteEvent) {
    if event.note["author_kind"].as_str() != Some("agent") {
        return;
    }
    let bot: Option<BotRow> = sqlx::query_as::<_, BotRow>(&format!(
        "SELECT {} FROM telegram_bots b
         JOIN threads t ON t.workspace_id = b.workspace_id AND t.kind = 'office'
                       AND t.deleted_at IS NULL
         WHERE t.id = $1 AND b.chat_id IS NOT NULL AND b.deleted_at IS NULL",
        BOT_COLUMNS
            .split(", ")
            .map(|c| format!("b.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ")
    ))
    .bind(event.thread_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    let (Some(bot), Some(body)) = (bot, event.note["body"].as_str()) else {
        return;
    };
    let (Some(token), Some(chat)) = (token_of(&bot), bot.chat_id) else {
        return;
    };
    let who = event.note["author_name"].as_str().unwrap_or("Your bot");
    match say(state, &token, chat, &format!("{who}: {body}")).await {
        Ok(_) => {}
        Err(e) => note_error(&state.pool, bot.id, Some(&e)).await,
    }
}

/// New approvals in linked offices, sent with Approve and Reject buttons.
/// Returns how many were sent.
pub async fn send_approvals(state: &AppState) -> usize {
    let bots: Vec<BotRow> = sqlx::query_as::<_, BotRow>(&format!(
        "SELECT {BOT_COLUMNS} FROM telegram_bots WHERE chat_id IS NOT NULL AND deleted_at IS NULL"
    ))
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    let mut sent = 0;
    for bot in bots {
        let (Some(token), Some(chat)) = (token_of(&bot), bot.chat_id) else {
            continue;
        };
        let pending = crate::chat_apps::approvals_since(
            &state.pool,
            bot.workspace_id,
            bot.approvals_sent_until,
        )
        .await;
        let mut until = bot.approvals_sent_until;
        for item in pending {
            let (id, action, who, at) = (item.id, item.action, item.asked_by, item.created_at);
            let res = call(
                state,
                &token,
                "sendMessage",
                json!({
                    "chat_id": chat,
                    "text": cut(&format!("Approval needed\n{action}\nAsked by {who}")),
                    "reply_markup": { "inline_keyboard": [[
                        { "text": "Approve", "callback_data": format!("ap:{id}:y") },
                        { "text": "Reject", "callback_data": format!("ap:{id}:n") },
                    ]]},
                }),
            )
            .await;
            match res {
                Ok(_) => {
                    sent += 1;
                    until = at;
                }
                Err(e) => {
                    // Tried again next round.
                    note_error(&state.pool, bot.id, Some(&e)).await;
                    break;
                }
            }
        }
        if until != bot.approvals_sent_until {
            let _ = sqlx::query(
                "UPDATE telegram_bots SET approvals_sent_until = $2, updated_at = now() WHERE id = $1",
            )
            .bind(bot.id)
            .bind(until)
            .execute(&state.pool)
            .await;
        }
    }
    sent
}

// ---------- The loop ----------

/// One task per connected bot asks Telegram for messages; another sends
/// the bots' messages; every 20 seconds new approvals go out and the bot
/// list is refreshed.
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
    let mut pollers: HashMap<Uuid, tokio::task::JoinHandle<()>> = HashMap::new();
    loop {
        let ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM telegram_bots WHERE deleted_at IS NULL")
                .fetch_all(&state.pool)
                .await
                .unwrap_or_default();
        pollers.retain(|id, task| {
            let keep = ids.contains(id) && !task.is_finished();
            if !keep {
                task.abort();
            }
            keep
        });
        for id in ids {
            pollers.entry(id).or_insert_with(|| {
                let state = state.clone();
                tokio::spawn(async move {
                    let mut failures = 0u32;
                    loop {
                        match poll_once(&state, id, POLL_SECONDS).await {
                            Ok(_) => {
                                if failures > 0 {
                                    note_error(&state.pool, id, None).await;
                                }
                                failures = 0;
                            }
                            Err(e) if e == "disconnected" => return,
                            Err(e) => {
                                failures += 1;
                                note_error(&state.pool, id, Some(&e)).await;
                                // 10s, 20s, ... up to five minutes.
                                let wait = (10 * failures as u64).min(300);
                                tokio::time::sleep(Duration::from_secs(wait)).await;
                            }
                        }
                    }
                })
            });
        }
        send_approvals(&state).await;
        tokio::time::sleep(Duration::from_secs(20)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_is_cut_to_telegrams_limit() {
        assert_eq!(cut("hi"), "hi");
        let long = "a".repeat(MAX_TEXT + 10);
        assert_eq!(cut(&long).chars().count(), MAX_TEXT + 1);
    }
}
