//! WhatsApp for an office, on Meta's official WhatsApp Business Cloud API
//! (issue #33).
//!
//! Edit office → Channels → WhatsApp: the owner connects their own Meta
//! app's business number (its id, an access token and the app secret, both
//! kept only encrypted) and points the app's webhook at this server, with
//! the verify token shown there. Then customers write to the business on
//! WhatsApp and a Receptionist bot answers:
//!
//! - from that customer's own conversation only (never another customer's),
//!   the office's profile and apps, in the office's language;
//! - taking bookings, leads and requests into the office's apps (the same
//!   `ADD` lines as in chat), and adding the customer to Contacts;
//! - handing over to a person when the customer asks or it can't help, with
//!   a line in the office chat.
//!
//! Every message shows in the office's WhatsApp room. A person answers a
//! customer there by starting a message with their number
//! (`+15551234567 See you at 3!`); the bot then leaves that customer to
//! them for a while. Anything else in the room is a normal chat with the
//! Receptionist.
//!
//! Each webhook call is checked against the app secret
//! (`X-Hub-Signature-256`, HMAC-SHA256 of the body), and a message
//! delivered twice is handled once (WhatsApp's message id). WhatsApp only
//! lets a business write freely within 24 hours of the customer's last
//! message, so nothing is sent outside that window. Meta calls this server,
//! so WhatsApp needs a public address (not the desktop app).
//!
//! TODO(air): messages outside the 24-hour window need templates approved
//! by Meta (reminders, follow-ups); bots and workflows can't start a
//! WhatsApp conversation yet. Photos, voice notes and files are noted, not
//! read.

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::Caller;
use crate::error::AppError;
use crate::handlers::{wants_html, JsonOrForm};
use crate::models::Thread;
use crate::office::team::{require_person, Role};
use crate::state::AppState;

const GRAPH_HOST: &str = "graph.facebook.com";
/// The Graph API version called (released July 2026).
const GRAPH_VERSION: &str = "v26.0";
const RECEPTIONIST: &str = "Receptionist";
const MODEL: &str = "anthropic/claude-3.5-haiku";
/// Credits one answer costs (a short reply from a small model).
const TURN_CREDITS: i64 = 8;
/// Messages of a conversation the receptionist reads.
const HISTORY: i64 = 30;
/// WhatsApp lets a business write freely this long after the customer's
/// last message.
const WINDOW_HOURS: i32 = 24;
/// After a person answers a customer, the bot leaves them to it this long.
const PERSON_HOURS: i32 = 12;
/// WhatsApp's limit for one text message.
const MAX_TEXT: usize = 4096;

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

/// One Graph API call. Errors carry Meta's message, never the token.
async fn graph(
    state: &AppState,
    token: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let url = state
        .config
        .connectors
        .url(GRAPH_HOST, &format!("/{GRAPH_VERSION}{path}"));
    let mut req = client().request(method, url).bearer_auth(token);
    if let Some(body) = body {
        req = req.json(&body);
    }
    let res = req
        .send()
        .await
        .map_err(|e| format!("Meta couldn't be reached ({})", e.without_url()))?;
    let status = res.status();
    let data: Value = res.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        Ok(data)
    } else {
        let why = data["error"]["message"].as_str().unwrap_or("an error");
        Err(format!(
            "Meta said: {}",
            why.chars().take(200).collect::<String>()
        ))
    }
}

#[derive(sqlx::FromRow, Clone)]
struct NumberRow {
    id: Uuid,
    workspace_id: Uuid,
    owner_user_id: Uuid,
    phone_number_id: String,
    display_number: String,
    verified_name: Option<String>,
    access_token_encrypted: String,
    app_secret_encrypted: String,
    agent_id: Uuid,
    thread_id: Uuid,
    auto_reply: bool,
    last_error: Option<String>,
}

const NUMBER_COLUMNS: &str = "id, workspace_id, owner_user_id, phone_number_id, display_number, \
     verified_name, access_token_encrypted, app_secret_encrypted, agent_id, thread_id, \
     auto_reply, last_error";

async fn number_of_office(pool: &PgPool, office: Uuid) -> Result<Option<NumberRow>, sqlx::Error> {
    sqlx::query_as::<_, NumberRow>(&format!(
        "SELECT {NUMBER_COLUMNS} FROM whatsapp_numbers WHERE workspace_id = $1 AND deleted_at IS NULL"
    ))
    .bind(office)
    .fetch_optional(pool)
    .await
}

async fn number_of_room(pool: &PgPool, room: Uuid) -> Result<Option<NumberRow>, sqlx::Error> {
    sqlx::query_as::<_, NumberRow>(&format!(
        "SELECT {NUMBER_COLUMNS} FROM whatsapp_numbers WHERE thread_id = $1 AND deleted_at IS NULL"
    ))
    .bind(room)
    .fetch_optional(pool)
    .await
}

async fn note_error(pool: &PgPool, number: Uuid, error: Option<&str>) {
    let error = error.map(|e| e.chars().take(300).collect::<String>());
    let _ = sqlx::query(
        "UPDATE whatsapp_numbers SET last_error = $2, updated_at = now() WHERE id = $1",
    )
    .bind(number)
    .bind(error)
    .execute(pool)
    .await;
}

fn webhook_url(state: &AppState, office: Uuid) -> String {
    format!(
        "{}/whatsapp/{office}/webhook",
        state.config.app_base_url.trim_end_matches('/')
    )
}

/// Whether `body` was signed by Meta with the app secret
/// (`X-Hub-Signature-256: sha256=<hex>`).
pub fn signed_by_meta(app_secret: &str, body: &[u8], signature: Option<&str>) -> bool {
    use hmac::Mac;
    let Some(hex) = signature.and_then(|s| s.trim().strip_prefix("sha256=")) else {
        return false;
    };
    if hex.len() != 64 {
        return false;
    }
    let Some(given) = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect::<Option<Vec<u8>>>()
    else {
        return false;
    };
    let Ok(mut mac) = <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(app_secret.as_bytes())
    else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&given).is_ok()
}

/// A person's message to a customer: `+<number> <text>` (or with a colon).
fn addressed(text: &str) -> Option<(String, &str)> {
    let rest = text.trim_start().strip_prefix('+')?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if !(6..=20).contains(&digits.len()) {
        return None;
    }
    let message = rest[digits.len()..].trim_start_matches(':').trim();
    (!message.is_empty()).then_some((digits, message))
}

fn cut(text: &str) -> String {
    text.chars().take(MAX_TEXT).collect()
}

// ---------- The app ----------

/// GET /api/offices/:id/whatsapp
pub async fn status(
    State(state): State<AppState>,
    caller: Caller,
    Path(office): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    require_person(&state.pool, &caller, office, Role::Owner).await?;
    let number = number_of_office(&state.pool, office).await?;
    let conversations: Vec<(String, Option<String>, chrono::DateTime<chrono::Utc>, i64)> =
        match &number {
            Some(n) => {
                sqlx::query_as(
                    "SELECT customer, max(customer_name), max(created_at), COUNT(*)
                 FROM whatsapp_messages
                 WHERE number_id = $1 AND deleted_at IS NULL
                 GROUP BY customer ORDER BY max(created_at) DESC LIMIT 5",
                )
                .bind(n.id)
                .fetch_all(&state.pool)
                .await?
            }
            None => Vec::new(),
        };
    Ok(Json(json!({
        "available": state.config.local.is_none(),
        "connected": number.is_some(),
        "display_number": number.as_ref().map(|n| n.display_number.clone()),
        "verified_name": number.as_ref().and_then(|n| n.verified_name.clone()),
        "webhook_url": number.as_ref().map(|_| webhook_url(&state, office)),
        "verify_token": number.as_ref().map(|n| crate::crypto::whatsapp_verify_token(n.id)),
        "auto_reply": number.as_ref().is_none_or(|n| n.auto_reply),
        "room_id": number.as_ref().map(|n| n.thread_id),
        "last_error": number.as_ref().and_then(|n| n.last_error.clone()),
        "recent_conversations": conversations.iter().map(|(customer, name, at, count)| json!({
            "customer": format!("+{customer}"), "name": name, "last_at": at, "messages": count,
        })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
pub struct ConnectRequest {
    /// Absent on a change to a connected number's settings.
    #[serde(default)]
    pub phone_number_id: Option<String>,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub app_secret: Option<String>,
    #[serde(default)]
    pub auto_reply: Option<bool>,
    pub idempotency_key: Option<String>,
}

/// POST /api/offices/:id/whatsapp -- connect a WhatsApp business number
/// (or, with only `auto_reply`, change whether the bot answers).
pub async fn connect(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<ConnectRequest>,
) -> Result<Response, AppError> {
    require_person(&state.pool, &caller, office, Role::Owner).await?;
    if state.config.local.is_some() {
        return Err(AppError::BadRequest(
            "WhatsApp needs Airobot on a server Meta can reach.".into(),
        ));
    }
    let key = crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;

    // Settings only.
    if body.phone_number_id.is_none() && body.access_token.is_none() && body.app_secret.is_none() {
        let auto_reply = body.auto_reply.unwrap_or(true);
        let changed = sqlx::query(
            "UPDATE whatsapp_numbers SET auto_reply = $2, updated_at = now()
             WHERE workspace_id = $1 AND deleted_at IS NULL",
        )
        .bind(office)
        .bind(auto_reply)
        .execute(&state.pool)
        .await?;
        if changed.rows_affected() == 0 {
            return Err(AppError::NotFound);
        }
        return Ok(Json(json!({ "connected": true, "auto_reply": auto_reply })).into_response());
    }

    if let Some(key) = key {
        let earlier: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT id, display_number FROM whatsapp_numbers
             WHERE workspace_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL",
        )
        .bind(office)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?;
        if let Some((id, display)) = earlier {
            return Ok(Json(json!({
                "connected": true,
                "display_number": display,
                "webhook_url": webhook_url(&state, office),
                "verify_token": crate::crypto::whatsapp_verify_token(id),
                "repeated": true,
            }))
            .into_response());
        }
    }

    let number_id = body
        .phone_number_id
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    let token = body
        .access_token
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    let secret = body.app_secret.as_deref().unwrap_or("").trim().to_string();
    if !(5..=32).contains(&number_id.len()) || !number_id.chars().all(|c| c.is_ascii_digit()) {
        return Err(AppError::BadRequest(
            "paste the Phone number ID from your Meta app's WhatsApp settings (only digits)".into(),
        ));
    }
    if token.is_empty() || token.len() > 1000 || token.chars().any(char::is_whitespace) {
        return Err(AppError::BadRequest(
            "paste the access token from your Meta app".into(),
        ));
    }
    if secret.len() != 32 || !secret.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest(
            "paste the App secret from your Meta app's basic settings".into(),
        ));
    }

    // The number must answer to this token.
    let found = graph(
        &state,
        &token,
        reqwest::Method::GET,
        &format!("/{number_id}?fields=display_phone_number,verified_name"),
        None,
    )
    .await
    .map_err(|e| AppError::BadRequest(format!("That didn't work. {e}")))?;
    let display: String = found["display_phone_number"]
        .as_str()
        .unwrap_or(&number_id)
        .chars()
        .take(40)
        .collect();
    let verified_name: Option<String> = found["verified_name"]
        .as_str()
        .map(|n| n.chars().take(200).collect());

    let (Some(token_sealed), Some(secret_sealed)) = (
        crate::crypto::encrypt(&token),
        crate::crypto::encrypt(&secret),
    ) else {
        tracing::error!("could not encrypt WhatsApp credentials");
        return Err(AppError::Internal);
    };
    let owner = caller.user_id();
    let agent = crate::office::owned_bot(
        &state.pool,
        owner,
        RECEPTIONIST,
        "receptionist",
        "You answer the office's phone and WhatsApp: friendly, brief, one question at a time.",
    )
    .await?;
    let (room, _) = crate::handlers::threads::create_thread_inner(
        &state,
        &caller,
        crate::handlers::threads::CreateThreadRequest {
            title: "WhatsApp".to_string(),
            visibility: None,
            idempotency_key: Some(format!("whatsapp-room:{office}")),
            board_id: None,
            board_name: None,
            command: None,
            workspace_id: Some(office),
            agent_ids: vec![agent],
        },
    )
    .await?;

    let mut tx = state.pool.begin().await?;
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM whatsapp_numbers
                       WHERE phone_number_id = $1 AND workspace_id <> $2 AND deleted_at IS NULL)",
    )
    .bind(&number_id)
    .bind(office)
    .fetch_one(&mut *tx)
    .await?;
    if taken {
        return Err(AppError::Conflict(
            "that number answers for another office".into(),
        ));
    }
    sqlx::query(
        "UPDATE whatsapp_numbers SET deleted_at = now(), updated_at = now()
         WHERE workspace_id = $1 AND deleted_at IS NULL",
    )
    .bind(office)
    .execute(&mut *tx)
    .await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO whatsapp_numbers (workspace_id, owner_user_id, phone_number_id, display_number,
                                       verified_name, access_token_encrypted, app_secret_encrypted,
                                       agent_id, thread_id, auto_reply, idempotency_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
         RETURNING id",
    )
    .bind(office)
    .bind(owner)
    .bind(&number_id)
    .bind(&display)
    .bind(&verified_name)
    .bind(&token_sealed)
    .bind(&secret_sealed)
    .bind(agent)
    .bind(room.id)
    .bind(body.auto_reply.unwrap_or(true))
    .bind(key)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "connected": true,
            "display_number": display,
            "verified_name": verified_name,
            "webhook_url": webhook_url(&state, office),
            "verify_token": crate::crypto::whatsapp_verify_token(id),
            "room_id": room.id,
        })),
    )
        .into_response())
}

/// POST /api/offices/:id/whatsapp/delete -- stop answering. The number
/// stays in the owner's Meta app; its messages stop being answered here.
pub async fn disconnect(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
) -> Result<Response, AppError> {
    require_person(&state.pool, &caller, office, Role::Owner).await?;
    sqlx::query(
        "UPDATE whatsapp_numbers SET deleted_at = now(), updated_at = now()
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

// ---------- Meta ----------

#[derive(Deserialize)]
pub struct VerifyQuery {
    #[serde(rename = "hub.mode")]
    mode: Option<String>,
    #[serde(rename = "hub.verify_token")]
    verify_token: Option<String>,
    #[serde(rename = "hub.challenge")]
    challenge: Option<String>,
}

/// GET /whatsapp/:office/webhook -- Meta checks the webhook: echo its
/// challenge when the verify token is this office's.
pub async fn verify(
    State(state): State<AppState>,
    Path(office): Path<Uuid>,
    Query(q): Query<VerifyQuery>,
) -> Result<Response, AppError> {
    let number = number_of_office(&state.pool, office)
        .await?
        .ok_or(AppError::NotFound)?;
    let token_ok = q
        .verify_token
        .as_deref()
        .is_some_and(|t| crate::crypto::verify_whatsapp_token(number.id, t));
    let challenge = q.challenge.unwrap_or_default();
    if q.mode.as_deref() != Some("subscribe")
        || !token_ok
        || challenge.is_empty()
        || challenge.len() > 200
        || !challenge
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(AppError::Forbidden);
    }
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        challenge,
    )
        .into_response())
}

/// One message from a customer, as Meta delivers it.
struct Incoming {
    wamid: String,
    customer: String,
    name: Option<String>,
    text: String,
}

/// The customers' messages for `number` in a webhook body. Statuses of
/// messages sent (delivered, read, failed) come back as the last failure.
fn read_webhook(body: &Value, phone_number_id: &str) -> (Vec<Incoming>, Option<String>) {
    let mut messages = Vec::new();
    let mut failure = None;
    for change in body["entry"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|e| e["changes"].as_array().into_iter().flatten())
    {
        let value = &change["value"];
        if change["field"] != "messages"
            || value["metadata"]["phone_number_id"].as_str() != Some(phone_number_id)
        {
            continue;
        }
        for status in value["statuses"].as_array().into_iter().flatten() {
            if status["status"] == "failed" {
                let why = status["errors"][0]["message"]
                    .as_str()
                    .or(status["errors"][0]["title"].as_str())
                    .unwrap_or("a message couldn't be delivered");
                failure = Some(format!("WhatsApp couldn't deliver a message: {why}"));
            }
        }
        for m in value["messages"].as_array().into_iter().flatten() {
            let (Some(wamid), Some(customer)) = (m["id"].as_str(), m["from"].as_str()) else {
                continue;
            };
            if !(6..=20).contains(&customer.len()) || !customer.chars().all(|c| c.is_ascii_digit())
            {
                continue;
            }
            let text = match m["type"].as_str() {
                Some("text") => m["text"]["body"].as_str().unwrap_or("").to_string(),
                Some("button") => m["button"]["text"].as_str().unwrap_or("").to_string(),
                Some("interactive") => m["interactive"]["button_reply"]["title"]
                    .as_str()
                    .or(m["interactive"]["list_reply"]["title"].as_str())
                    .unwrap_or("")
                    .to_string(),
                Some(
                    kind @ ("image" | "audio" | "video" | "document" | "sticker" | "location"),
                ) => {
                    let caption = m[kind]["caption"].as_str().unwrap_or("");
                    format!("(sent a {kind}) {caption}").trim().to_string()
                }
                _ => "(sent a message the office can't read)".to_string(),
            };
            let text = cut(text.trim());
            if text.is_empty() {
                continue;
            }
            let name = value["contacts"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|c| c["wa_id"].as_str() == Some(customer))
                .and_then(|c| c["profile"]["name"].as_str())
                .map(|n| n.chars().take(200).collect::<String>());
            messages.push(Incoming {
                wamid: wamid.chars().take(200).collect(),
                customer: customer.to_string(),
                name,
                text,
            });
        }
    }
    (messages, failure)
}

/// POST /whatsapp/:office/webhook -- messages and delivery statuses from
/// Meta. Answers at once (Meta retries slow webhooks); the bot's replies
/// are sent in the background.
pub async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
    body: Bytes,
) -> Result<Response, AppError> {
    let number = number_of_office(&state.pool, office)
        .await?
        .ok_or(AppError::NotFound)?;
    let secret = crate::crypto::decrypt(&number.app_secret_encrypted).ok_or_else(|| {
        tracing::error!(office = %office, "a WhatsApp app secret couldn't be decrypted");
        AppError::Internal
    })?;
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok());
    if !signed_by_meta(&secret, &body, signature) {
        return Err(AppError::Unauthorized);
    }
    let payload: Value = serde_json::from_slice(&body)
        .map_err(|_| AppError::BadRequest("not a WhatsApp webhook".into()))?;
    let (messages, failure) = read_webhook(&payload, &number.phone_number_id);
    if let Some(why) = failure {
        note_error(&state.pool, number.id, Some(&why)).await;
    }
    for m in messages {
        // A message delivered twice is kept (and answered) once.
        let new: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO whatsapp_messages (number_id, workspace_id, customer, customer_name,
                                            direction, sent_by, body, wamid)
             VALUES ($1, $2, $3, $4, 'in', 'customer', $5, $6)
             ON CONFLICT (number_id, wamid) WHERE wamid IS NOT NULL DO NOTHING
             RETURNING id",
        )
        .bind(number.id)
        .bind(number.workspace_id)
        .bind(&m.customer)
        .bind(&m.name)
        .bind(&m.text)
        .bind(&m.wamid)
        .fetch_optional(&state.pool)
        .await?;
        if new.is_some() {
            tokio::spawn(handle_message(state.clone(), number.clone(), m));
        }
    }
    Ok(StatusCode::OK.into_response())
}

/// Sends `text` to `customer`; WhatsApp's id for it.
async fn send_text(
    state: &AppState,
    number: &NumberRow,
    customer: &str,
    text: &str,
) -> Result<Option<String>, String> {
    let token = crate::crypto::decrypt(&number.access_token_encrypted)
        .ok_or("the access token couldn't be read; connect WhatsApp again")?;
    let sent = graph(
        state,
        &token,
        reqwest::Method::POST,
        &format!("/{}/messages", number.phone_number_id),
        Some(json!({
            "messaging_product": "whatsapp",
            "recipient_type": "individual",
            "to": customer,
            "type": "text",
            "text": { "preview_url": false, "body": cut(text) },
        })),
    )
    .await?;
    Ok(sent["messages"][0]["id"].as_str().map(str::to_string))
}

/// Keeps a message sent to `customer`.
async fn keep_sent(
    pool: &PgPool,
    number: &NumberRow,
    customer: &str,
    sent_by: &str,
    text: &str,
    wamid: Option<String>,
) {
    let _ = sqlx::query(
        "INSERT INTO whatsapp_messages (number_id, workspace_id, customer, direction, sent_by, body, wamid)
         VALUES ($1, $2, $3, 'out', $4, $5, $6)
         ON CONFLICT (number_id, wamid) WHERE wamid IS NOT NULL DO NOTHING",
    )
    .bind(number.id)
    .bind(number.workspace_id)
    .bind(customer)
    .bind(sent_by)
    .bind(cut(text))
    .bind(wamid)
    .execute(pool)
    .await;
}

/// Whether the customer wrote within WhatsApp's 24-hour window.
async fn window_open(pool: &PgPool, number: Uuid, customer: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM whatsapp_messages
                       WHERE number_id = $1 AND customer = $2 AND direction = 'in'
                         AND deleted_at IS NULL
                         AND created_at > now() - make_interval(hours => $3))",
    )
    .bind(number)
    .bind(customer)
    .bind(WINDOW_HOURS)
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

/// The customer's last name WhatsApp gave.
async fn name_of(pool: &PgPool, number: Uuid, customer: &str) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT customer_name FROM whatsapp_messages
         WHERE number_id = $1 AND customer = $2 AND customer_name IS NOT NULL
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(number)
    .bind(customer)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .flatten()
}

async fn room_note(state: &AppState, number: &NumberRow, text: &str) {
    crate::handlers::threads::insert_bot_note(
        &state.pool,
        &state.note_sender,
        number.thread_id,
        number.agent_id,
        text,
        None,
    )
    .await;
}

/// A line in the office chat, with a button to the WhatsApp room.
async fn office_note(state: &AppState, number: &NumberRow, text: &str) {
    match crate::office::group::ensure_office_chat(
        &state.pool,
        number.owner_user_id,
        number.workspace_id,
    )
    .await
    {
        Ok(chat) => {
            crate::handlers::threads::insert_bot_note(
                &state.pool,
                &state.note_sender,
                chat,
                number.agent_id,
                text,
                Some(json!([{ "label": "Open WhatsApp", "kind": "open", "target": format!("room:{}", number.thread_id) }])),
            )
            .await;
        }
        Err(e) => tracing::warn!(error = ?e, "no office chat for a WhatsApp note"),
    }
}

/// A customer's message: shown in the room, added to Contacts, and answered
/// by the Receptionist unless a person has the conversation.
async fn handle_message(state: AppState, number: NumberRow, m: Incoming) {
    let pool = &state.pool;
    let who = match &m.name {
        Some(name) => format!("{name} (+{})", m.customer),
        None => format!("+{}", m.customer),
    };
    let contact_name = m.name.clone().unwrap_or_else(|| format!("+{}", m.customer));
    let phone = crate::office::contacts::normalize_phone(&format!("+{}", m.customer));
    crate::office::contacts::find_or_create(
        pool,
        number.workspace_id,
        &contact_name,
        None,
        phone.as_deref(),
        None,
        "whatsapp",
        Some(number.agent_id),
    )
    .await;

    // Blue ticks for the customer, best effort.
    if let Some(token) = crate::crypto::decrypt(&number.access_token_encrypted) {
        let _ = graph(
            &state,
            &token,
            reqwest::Method::POST,
            &format!("/{}/messages", number.phone_number_id),
            Some(
                json!({ "messaging_product": "whatsapp", "status": "read", "message_id": m.wamid }),
            ),
        )
        .await;
    }

    // A new conversation (nothing from them in the last day) is news.
    let earlier: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM whatsapp_messages
         WHERE number_id = $1 AND customer = $2 AND direction = 'in' AND deleted_at IS NULL
           AND created_at > now() - make_interval(hours => $3)",
    )
    .bind(number.id)
    .bind(&m.customer)
    .bind(WINDOW_HOURS)
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let person_has_it: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM whatsapp_messages
                       WHERE number_id = $1 AND customer = $2 AND sent_by = 'person'
                         AND deleted_at IS NULL
                         AND created_at > now() - make_interval(hours => $3))",
    )
    .bind(number.id)
    .bind(&m.customer)
    .bind(PERSON_HOURS)
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    let shown = format!("💬 {who}: {}", m.text);
    if !number.auto_reply || person_has_it {
        room_note(&state, &number, &shown).await;
        if earlier <= 1 {
            office_note(
                &state,
                &number,
                &format!(
                    "New WhatsApp message from {who}. Answer in the WhatsApp room, starting with +{}.",
                    m.customer
                ),
            )
            .await;
        }
        return;
    }

    match answer(&state, &number, &m).await {
        Ok(turn) => {
            let mut note = shown.clone();
            if !turn.reply.is_empty() {
                match send_text(&state, &number, &m.customer, &turn.reply).await {
                    Ok(wamid) => {
                        keep_sent(pool, &number, &m.customer, "bot", &turn.reply, wamid).await;
                        note.push_str(&format!("\n\n{}", turn.reply));
                    }
                    Err(why) => {
                        note_error(pool, number.id, Some(&why)).await;
                        note.push_str(&format!("\n\n_(the reply couldn't be sent: {why})_"));
                    }
                }
            }
            if turn.records > 0 {
                note.push_str(&format!(
                    "\n\n_({} added to the office's apps)_",
                    turn.records
                ));
            }
            if turn.transfer {
                note.push_str("\n\n_(waiting for a person)_");
            }
            room_note(&state, &number, &note).await;
            if turn.transfer {
                office_note(
                    &state,
                    &number,
                    &format!(
                        "{who} on WhatsApp would like a person: \"{}\". Answer in the WhatsApp room, starting with +{}.",
                        m.text.chars().take(200).collect::<String>(),
                        m.customer
                    ),
                )
                .await;
            } else if earlier <= 1 {
                office_note(
                    &state,
                    &number,
                    &format!(
                        "New WhatsApp conversation with {who}; the Receptionist is answering."
                    ),
                )
                .await;
            }
        }
        Err(why) => {
            tracing::warn!(office = %number.workspace_id, why, "a WhatsApp answer failed");
            room_note(
                &state,
                &number,
                &format!("{shown}\n\n_(no answer was sent: {why})_"),
            )
            .await;
            office_note(
                &state,
                &number,
                &format!(
                    "{who} wrote on WhatsApp and the Receptionist couldn't answer ({why}). Answer in the WhatsApp room, starting with +{}.",
                    m.customer
                ),
            )
            .await;
        }
    }
}

struct Turn {
    reply: String,
    transfer: bool,
    records: usize,
}

/// The Receptionist's reply to a customer, from this customer's own
/// conversation only.
async fn answer(state: &AppState, number: &NumberRow, m: &Incoming) -> Result<Turn, &'static str> {
    let pool = &state.pool;
    let business = crate::office::context::business_of_thread(pool, number.thread_id)
        .await
        .ok_or("the office is gone")?;
    // Only the apps the Receptionist may fill (bookings, leads, messages).
    let access = crate::office::access::effective(pool, number.workspace_id, number.agent_id).await;
    let apps: Vec<_> = crate::office::context::apps_of(pool, number.workspace_id)
        .await
        .into_iter()
        .filter(|a| access.may_use_app(&a.slug))
        .collect();
    let mut history: Vec<(String, String)> = sqlx::query_as(
        "SELECT sent_by, body FROM whatsapp_messages
         WHERE number_id = $1 AND customer = $2 AND deleted_at IS NULL
         ORDER BY created_at DESC LIMIT $3",
    )
    .bind(number.id)
    .bind(&m.customer)
    .bind(HISTORY)
    .fetch_all(pool)
    .await
    .map_err(|_| "the conversation couldn't be read")?;
    history.reverse();
    let customer = match &m.name {
        Some(name) => format!("{name} (+{})", m.customer),
        None => format!("+{}", m.customer),
    };
    let mut system = format!(
        "You are the receptionist answering WhatsApp messages for {}. You write to one customer, {customer}, in a chat: \
         reply in one to four short, warm sentences, plain text (no markdown headings or tables, no links you weren't given). \
         Ask one question at a time. Take bookings, orders and messages: once you have the details (name, what, when, a phone number), \
         add a record as described below and confirm it in one sentence. Don't make up prices, times or promises the business \
         didn't give you; offer to take a message instead. \
         If the customer asks for a person, or you can't help, say a person from the team will reply here soon, \
         and end your reply with a line that says only TRANSFER.",
        business.name
    );
    system.push_str(&crate::office::context::prompt(&business, &apps));
    let mut convo = String::new();
    for (by, body) in &history {
        let who = if by == "customer" { "Customer" } else { "You" };
        convo.push_str(&format!("{who}: {body}\n"));
    }
    system.push_str(
        &crate::office::contacts::context_block(
            pool,
            number.workspace_id,
            &format!("{customer}\n{convo}"),
        )
        .await,
    );
    let prompt =
        format!("The conversation so far:\n{convo}\nYour reply to the customer's last message:");

    let reply = if let Some(ai) = state.local_ai.clone() {
        ai(crate::local::LocalAiRequest {
            system,
            prompt,
            engine: None,
            model: None,
            web: false,
        })
        .await
        .map_err(|_| "no answer")?
    } else {
        let key = state
            .config
            .openrouter_api_key
            .clone()
            .filter(|k| !k.trim().is_empty())
            .ok_or("no AI")?;
        let proposal = crate::office::budget_gate::ActionProposal {
            provider: "openrouter".into(),
            model_or_tool: MODEL.into(),
            estimated_credits: TURN_CREDITS,
            raw_cost_micros: 80,
            requires_manual_approval: false,
            action_type: "whatsapp_turn".into(),
        };
        let gate = crate::office::budget_gate::check_and_deduct(
            pool,
            number.workspace_id,
            &proposal,
            None,
            true,
            crate::office::budget_gate::Spender::new(number.workspace_id, Some(number.agent_id)),
        )
        .await
        .map_err(|_| "no credits")?;
        if !gate.passed {
            return Err("no credits");
        }
        crate::openrouter::call_openrouter(&key, MODEL, &system, &prompt)
            .await
            .map_err(|_| "no answer")?
    };

    let mut transfer = false;
    let kept: Vec<&str> = reply
        .lines()
        .filter(|l| {
            if l.trim().trim_matches('*').trim() == "TRANSFER" {
                transfer = true;
                false
            } else {
                true
            }
        })
        .collect();
    // Bookings and leads it took go into the office's apps.
    let (text, _, cards) = crate::office::context::apply_records(
        pool,
        number.thread_id,
        number.agent_id,
        &kept.join("\n"),
    )
    .await;
    // The customer gets the reply, not the "Added ..." line for the office.
    let mut reply = crate::office::context::customer_text(&text)
        .trim()
        .to_string();
    if reply.is_empty() && !cards.is_empty() {
        reply = "Thanks, that's noted.".to_string();
    }
    Ok(Turn {
        reply,
        transfer,
        records: cards.len(),
    })
}

/// A person wrote in the WhatsApp room. `+<number> <text>` goes to that
/// customer on WhatsApp and the bot leaves them to the person; returns
/// false for anything else, which the Receptionist answers as in any room.
pub async fn on_person_message(state: &AppState, thread: &Thread, text: &str) -> bool {
    let Some((customer, message)) = addressed(text) else {
        return false;
    };
    let Ok(Some(number)) = number_of_room(&state.pool, thread.id).await else {
        return false;
    };
    if !window_open(&state.pool, number.id, &customer).await {
        room_note(
            state,
            &number,
            &format!(
                "Not sent: WhatsApp only lets a business reply within 24 hours of the customer's last message, and +{customer} hasn't written in that time."
            ),
        )
        .await;
        return true;
    }
    let name = name_of(&state.pool, number.id, &customer)
        .await
        .unwrap_or_else(|| format!("+{customer}"));
    match send_text(state, &number, &customer, message).await {
        Ok(wamid) => {
            keep_sent(&state.pool, &number, &customer, "person", message, wamid).await;
            room_note(
                state,
                &number,
                &format!("Sent to {name} on WhatsApp. I'll leave this conversation to you for {PERSON_HOURS} hours."),
            )
            .await;
        }
        Err(why) => {
            note_error(&state.pool, number.id, Some(&why)).await;
            room_note(state, &number, &format!("Not sent: {why}")).await;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metas_signature_is_checked() {
        use hmac::Mac;
        let body = br#"{"object":"whatsapp_business_account"}"#;
        let mut mac =
            <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(b"0123456789abcdef0123456789abcdef")
                .expect("key");
        mac.update(body);
        let hex: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let secret = "0123456789abcdef0123456789abcdef";
        assert!(signed_by_meta(secret, body, Some(&format!("sha256={hex}"))));
        assert!(!signed_by_meta(
            secret,
            b"{}",
            Some(&format!("sha256={hex}"))
        ));
        assert!(!signed_by_meta(
            "another-secret",
            body,
            Some(&format!("sha256={hex}"))
        ));
        assert!(!signed_by_meta(secret, body, Some(&hex)));
        assert!(!signed_by_meta(secret, body, None));
        assert!(!signed_by_meta(secret, body, Some("sha256=zz")));
    }

    #[test]
    fn a_person_addresses_a_customer_by_number() {
        assert_eq!(
            addressed("+15551234567 See you at 3!"),
            Some(("15551234567".to_string(), "See you at 3!"))
        );
        assert_eq!(
            addressed("+15551234567: Thanks"),
            Some(("15551234567".to_string(), "Thanks"))
        );
        assert_eq!(addressed("What did customers ask today?"), None);
        assert_eq!(addressed("+15551234567"), None);
        assert_eq!(addressed("+123 too short"), None);
    }

    #[test]
    fn only_this_numbers_messages_are_read() {
        let body = json!({ "entry": [{ "changes": [
            { "field": "messages", "value": {
                "metadata": { "phone_number_id": "1111" },
                "contacts": [{ "wa_id": "15550002222", "profile": { "name": "Ana" } }],
                "messages": [
                    { "from": "15550002222", "id": "wamid.1", "type": "text", "text": { "body": " Hi! " } },
                    { "from": "15550002222", "id": "wamid.2", "type": "image", "image": { "caption": "my cake" } },
                    { "from": "not-a-number", "id": "wamid.3", "type": "text", "text": { "body": "x" } }
                ],
                "statuses": [{ "status": "failed", "errors": [{ "message": "Re-engagement message" }] }]
            }},
            { "field": "messages", "value": {
                "metadata": { "phone_number_id": "9999" },
                "messages": [{ "from": "15550003333", "id": "wamid.4", "type": "text", "text": { "body": "other" } }]
            }}
        ]}]});
        let (messages, failure) = read_webhook(&body, "1111");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].text, "Hi!");
        assert_eq!(messages[0].name.as_deref(), Some("Ana"));
        assert_eq!(messages[1].text, "(sent a image) my cake");
        assert!(failure.is_some_and(|f| f.contains("Re-engagement")));
    }
}
