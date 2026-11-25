//! A phone receptionist per office (issue #32).
//!
//! Edit office → Phone: the owner connects a Twilio number (account SID,
//! auth token kept only encrypted, the number) and Airobot points the
//! number's calls at this server. A Receptionist bot answers:
//!
//! - in the office's language, with a natural (neural) voice;
//! - from the office's profile, documents-free context and apps, adding
//!   bookings and messages as records (the same `ADD` lines as in chat);
//! - handing the caller to a person (a number to forward to) when they ask
//!   for one, press 0, or the bot can't help;
//! - one short reply at a time, so it sounds like a person, not a menu.
//!
//! Every call from Twilio is checked against the auth token
//! (`X-Twilio-Signature`, HMAC-SHA1 over the URL and the sorted form
//! fields). Each call's transcript is kept (`phone_calls`), shown live in
//! the office's Phone calls room, and the office chat gets a line when a
//! call ends. Each answer is metered like any AI call. Twilio calls this
//! server, so the phone needs a public address (not the desktop app).
//!
//! TODO(air): SIP trunks other than Twilio, and buying a number from the
//! app (the reserved `workspace_wallets.dedicated_phone_number`).

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::Caller;
use crate::error::AppError;
use crate::handlers::{wants_html, JsonOrForm};
use crate::office::team::{require, Role};
use crate::state::AppState;

const RECEPTIONIST: &str = "Receptionist";
const MODEL: &str = "anthropic/claude-3.5-haiku";
/// Credits one answer costs (a short reply from a small model).
const TURN_CREDITS: i64 = 8;
/// What the receptionist keeps of a long call.
const MAX_TURNS: usize = 40;

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

fn is_phone(raw: &str) -> bool {
    let digits = raw.strip_prefix('+').unwrap_or("");
    (7..=15).contains(&digits.len())
        && digits.chars().all(|c| c.is_ascii_digit())
        && !digits.starts_with('0')
}

/// Twilio's voice and speech language for the office's language.
fn voice_for(language: Option<&str>) -> (&'static str, &'static str) {
    match language.unwrap_or("en") {
        "es" => ("es-ES", "Polly.Lucia-Neural"),
        "fr" => ("fr-FR", "Polly.Lea-Neural"),
        "de" => ("de-DE", "Polly.Vicki-Neural"),
        "pt-BR" => ("pt-BR", "Polly.Camila-Neural"),
        "pt" => ("pt-PT", "Polly.Ines-Neural"),
        "it" => ("it-IT", "Polly.Bianca-Neural"),
        "nl" => ("nl-NL", "Polly.Laura-Neural"),
        "ru" => ("ru-RU", "Polly.Tatyana"),
        "ja" => ("ja-JP", "Polly.Kazuha-Neural"),
        "ko" => ("ko-KR", "Polly.Seoyeon-Neural"),
        _ => ("en-US", "Polly.Joanna-Neural"),
    }
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn twiml(body: String) -> Response {
    (
        [(header::CONTENT_TYPE, "text/xml; charset=utf-8")],
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Response>{body}</Response>"),
    )
        .into_response()
}

/// The form fields of a body (`a=1&b=x+y`), decoded.
fn form_pairs(body: &[u8]) -> Vec<(String, String)> {
    fn decode(raw: &str) -> Option<String> {
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
    std::str::from_utf8(body)
        .unwrap_or("")
        .split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            Some((decode(k)?, decode(v)?))
        })
        .collect()
}

fn field<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// Whether Twilio signed this request with `auth_token`: base64 of
/// HMAC-SHA1 over the full URL and every form field, sorted, as key+value.
pub fn signed_by_twilio(
    auth_token: &str,
    url: &str,
    pairs: &[(String, String)],
    signature: &str,
) -> bool {
    use hmac::Mac;
    let mut sorted: Vec<&(String, String)> = pairs.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let Ok(mut mac) = <hmac::Hmac<sha1::Sha1> as Mac>::new_from_slice(auth_token.as_bytes()) else {
        return false;
    };
    mac.update(url.as_bytes());
    for (k, v) in sorted {
        mac.update(k.as_bytes());
        mac.update(v.as_bytes());
    }
    let Ok(given) = base64::engine::general_purpose::STANDARD.decode(signature.trim()) else {
        return false;
    };
    mac.verify_slice(&given).is_ok()
}

#[derive(sqlx::FromRow, Clone)]
struct LineRow {
    id: Uuid,
    workspace_id: Uuid,
    owner_user_id: Uuid,
    account_sid: String,
    auth_token_encrypted: String,
    phone_number: String,
    forward_to: Option<String>,
    greeting: Option<String>,
    agent_id: Uuid,
    thread_id: Uuid,
    last_error: Option<String>,
}

const LINE_COLUMNS: &str = "id, workspace_id, owner_user_id, account_sid, auth_token_encrypted, \
     phone_number, forward_to, greeting, agent_id, thread_id, last_error";

async fn line_of_office(pool: &PgPool, office: Uuid) -> Result<Option<LineRow>, sqlx::Error> {
    sqlx::query_as::<_, LineRow>(&format!(
        "SELECT {LINE_COLUMNS} FROM phone_lines WHERE workspace_id = $1 AND deleted_at IS NULL"
    ))
    .bind(office)
    .fetch_optional(pool)
    .await
}

fn webhook_url(state: &AppState, office: Uuid, what: &str) -> String {
    format!(
        "{}/phone/{office}/{what}",
        state.config.app_base_url.trim_end_matches('/')
    )
}

async fn twilio(
    state: &AppState,
    sid: &str,
    token: &str,
    method: reqwest::Method,
    path: &str,
    form: &[(&str, &str)],
) -> Result<Value, String> {
    let url = state.config.connectors.url(
        "api.twilio.com",
        &format!("/2010-04-01/Accounts/{sid}{path}"),
    );
    let mut req = client()
        .request(method.clone(), url)
        .basic_auth(sid, Some(token));
    req = if method == reqwest::Method::GET {
        req.query(form)
    } else {
        req.form(form)
    };
    let res = req
        .send()
        .await
        .map_err(|e| format!("Twilio couldn't be reached ({})", e.without_url()))?;
    let status = res.status();
    let data: Value = res.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        Ok(data)
    } else {
        let why = data["message"].as_str().unwrap_or("an error");
        Err(format!(
            "Twilio said: {}",
            why.chars().take(200).collect::<String>()
        ))
    }
}

// ---------- The app ----------

/// GET /api/offices/:id/phone
pub async fn status(
    State(state): State<AppState>,
    caller: Caller,
    Path(office): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    require(&state.pool, &caller, office, Role::Owner).await?;
    let line = line_of_office(&state.pool, office).await?;
    let calls: Vec<(Option<String>, i32, bool, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT from_number, records_added, transferred, created_at FROM phone_calls
         WHERE workspace_id = $1 AND deleted_at IS NULL ORDER BY created_at DESC LIMIT 5",
    )
    .bind(office)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!({
        "available": state.config.local.is_none(),
        "connected": line.is_some(),
        "phone_number": line.as_ref().map(|l| l.phone_number.clone()),
        "forward_to": line.as_ref().and_then(|l| l.forward_to.clone()),
        "greeting": line.as_ref().and_then(|l| l.greeting.clone()),
        "room_id": line.as_ref().map(|l| l.thread_id),
        "last_error": line.as_ref().and_then(|l| l.last_error.clone()),
        "recent_calls": calls.iter().map(|(from, records, transferred, at)| json!({
            "from": from, "records_added": records, "transferred": transferred, "at": at,
        })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
pub struct ConnectRequest {
    /// Absent on a change to the settings of a connected line.
    #[serde(default)]
    pub account_sid: Option<String>,
    #[serde(default)]
    pub auth_token: Option<String>,
    #[serde(default)]
    pub phone_number: Option<String>,
    #[serde(default)]
    pub forward_to: Option<String>,
    #[serde(default)]
    pub greeting: Option<String>,
    pub idempotency_key: Option<String>,
}

fn clean_settings(body: &ConnectRequest) -> Result<(Option<String>, Option<String>), AppError> {
    let forward = body
        .forward_to
        .as_deref()
        .map(|f| {
            f.chars()
                .filter(|c| !c.is_whitespace() && *c != '-' && *c != '(' && *c != ')')
                .collect::<String>()
        })
        .filter(|f| !f.is_empty());
    if forward.as_deref().is_some_and(|f| !is_phone(f)) {
        return Err(AppError::BadRequest(
            "the number to hand callers to needs the country code, like +15551234567".into(),
        ));
    }
    let greeting = body
        .greeting
        .as_deref()
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map(str::to_string);
    if greeting.as_deref().is_some_and(|g| g.chars().count() > 300) {
        return Err(AppError::BadRequest(
            "the greeting can be at most 300 characters".into(),
        ));
    }
    Ok((forward, greeting))
}

/// POST /api/offices/:id/phone -- connect a Twilio number (or, with only
/// `forward_to` and `greeting`, change a connected line's settings).
pub async fn connect(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<ConnectRequest>,
) -> Result<Response, AppError> {
    require(&state.pool, &caller, office, Role::Owner).await?;
    if state.config.local.is_some() {
        return Err(AppError::BadRequest(
            "The phone needs Airobot on a server Twilio can reach.".into(),
        ));
    }
    let key = crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let (forward, greeting) = clean_settings(&body)?;

    // Settings only.
    if body.account_sid.is_none() && body.auth_token.is_none() && body.phone_number.is_none() {
        let changed = sqlx::query(
            "UPDATE phone_lines SET forward_to = $2, greeting = $3, updated_at = now()
             WHERE workspace_id = $1 AND deleted_at IS NULL",
        )
        .bind(office)
        .bind(&forward)
        .bind(&greeting)
        .execute(&state.pool)
        .await?;
        if changed.rows_affected() == 0 {
            return Err(AppError::NotFound);
        }
        return Ok(
            Json(json!({ "connected": true, "forward_to": forward, "greeting": greeting }))
                .into_response(),
        );
    }

    if let Some(key) = key {
        let earlier: Option<String> = sqlx::query_scalar(
            "SELECT phone_number FROM phone_lines
             WHERE workspace_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL",
        )
        .bind(office)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?;
        if let Some(number) = earlier {
            return Ok(Json(
                json!({ "connected": true, "phone_number": number, "repeated": true }),
            )
            .into_response());
        }
    }

    let sid = body.account_sid.as_deref().unwrap_or("").trim().to_string();
    let token = body.auth_token.as_deref().unwrap_or("").trim().to_string();
    let number: String = body
        .phone_number
        .as_deref()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '(' && *c != ')')
        .collect();
    let hex = |s: &str, n: usize| {
        s.len() == n
            && s.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    };
    if !(sid.starts_with("AC") && hex(&sid[2.min(sid.len())..], 32)) {
        return Err(AppError::BadRequest(
            "paste the Account SID from the Twilio console (it starts with AC)".into(),
        ));
    }
    if !hex(&token, 32) {
        return Err(AppError::BadRequest(
            "paste the Auth Token from the Twilio console".into(),
        ));
    }
    if !is_phone(&number) {
        return Err(AppError::BadRequest(
            "the Twilio number needs the country code, like +15551234567".into(),
        ));
    }

    // The number must be this account's; then its calls come here.
    let found = twilio(
        &state,
        &sid,
        &token,
        reqwest::Method::GET,
        "/IncomingPhoneNumbers.json",
        &[("PhoneNumber", &number)],
    )
    .await
    .map_err(|e| AppError::BadRequest(format!("That didn't work. {e}")))?;
    let Some(number_sid) = found["incoming_phone_numbers"][0]["sid"]
        .as_str()
        .map(str::to_string)
    else {
        return Err(AppError::BadRequest(
            "that number isn't in this Twilio account; buy or port it in Twilio first".into(),
        ));
    };
    twilio(
        &state,
        &sid,
        &token,
        reqwest::Method::POST,
        &format!("/IncomingPhoneNumbers/{number_sid}.json"),
        &[
            ("VoiceUrl", &webhook_url(&state, office, "voice")),
            ("VoiceMethod", "POST"),
            ("StatusCallback", &webhook_url(&state, office, "status")),
            ("StatusCallbackMethod", "POST"),
        ],
    )
    .await
    .map_err(|e| AppError::BadRequest(format!("The number couldn't be set up. {e}")))?;

    let encrypted = crate::crypto::encrypt(&token).ok_or_else(|| {
        tracing::error!("could not encrypt a Twilio token");
        AppError::Internal
    })?;
    let owner = caller.user_id();
    let agent = crate::office::owned_bot(
        &state.pool,
        owner,
        RECEPTIONIST,
        "receptionist",
        "You answer the office's phone: friendly, brief, one question at a time.",
    )
    .await?;
    let (room, _) = crate::handlers::threads::create_thread_inner(
        &state,
        &caller,
        crate::handlers::threads::CreateThreadRequest {
            title: "Phone calls".to_string(),
            visibility: None,
            idempotency_key: Some(format!("phone-room:{office}")),
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
        "SELECT EXISTS(SELECT 1 FROM phone_lines
                       WHERE phone_number = $1 AND workspace_id <> $2 AND deleted_at IS NULL)",
    )
    .bind(&number)
    .bind(office)
    .fetch_one(&mut *tx)
    .await?;
    if taken {
        return Err(AppError::Conflict(
            "that number answers for another office".into(),
        ));
    }
    sqlx::query(
        "UPDATE phone_lines SET deleted_at = now(), updated_at = now()
         WHERE workspace_id = $1 AND deleted_at IS NULL",
    )
    .bind(office)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO phone_lines (workspace_id, owner_user_id, account_sid, auth_token_encrypted,
                                  phone_number, number_sid, forward_to, greeting, agent_id,
                                  thread_id, idempotency_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(office)
    .bind(owner)
    .bind(&sid)
    .bind(&encrypted)
    .bind(&number)
    .bind(&number_sid)
    .bind(&forward)
    .bind(&greeting)
    .bind(agent)
    .bind(room.id)
    .bind(key)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({ "connected": true, "phone_number": number, "room_id": room.id })),
    )
        .into_response())
}

/// POST /api/offices/:id/phone/delete -- stop answering. The number stays
/// in Twilio; its calls stop coming here.
pub async fn disconnect(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
) -> Result<Response, AppError> {
    require(&state.pool, &caller, office, Role::Owner).await?;
    crate::idempotency::key(&headers, None)?;
    if let Some(line) = line_of_office(&state.pool, office).await? {
        // Best effort: Twilio stops sending calls here.
        let number_sid: Option<String> =
            sqlx::query_scalar("SELECT number_sid FROM phone_lines WHERE id = $1")
                .bind(line.id)
                .fetch_optional(&state.pool)
                .await?;
        if let (Some(token), Some(number_sid)) = (
            crate::crypto::decrypt(&line.auth_token_encrypted),
            number_sid,
        ) {
            if let Err(e) = twilio(
                &state,
                &line.account_sid,
                &token,
                reqwest::Method::POST,
                &format!("/IncomingPhoneNumbers/{number_sid}.json"),
                &[("VoiceUrl", ""), ("StatusCallback", "")],
            )
            .await
            {
                tracing::warn!(error = %e, "could not clear a Twilio number's voice URL");
            }
        }
        sqlx::query("UPDATE phone_lines SET deleted_at = now(), updated_at = now() WHERE id = $1")
            .bind(line.id)
            .execute(&state.pool)
            .await?;
    }
    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    Ok(Json(json!({ "connected": false })).into_response())
}

// ---------- Calls from Twilio ----------

/// The office's line, if the request is really from Twilio for it.
async fn verified(
    state: &AppState,
    office: Uuid,
    what: &str,
    headers: &HeaderMap,
    pairs: &[(String, String)],
) -> Result<LineRow, AppError> {
    let line = line_of_office(&state.pool, office)
        .await?
        .ok_or(AppError::NotFound)?;
    let token = crate::crypto::decrypt(&line.auth_token_encrypted).ok_or(AppError::Forbidden)?;
    let signature = headers
        .get("x-twilio-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !signed_by_twilio(&token, &webhook_url(state, office, what), pairs, signature) {
        return Err(AppError::Forbidden);
    }
    // Calls to another number don't belong to this office.
    if field(pairs, "AccountSid").is_some_and(|a| a != line.account_sid) {
        return Err(AppError::Forbidden);
    }
    Ok(line)
}

async fn language_of(pool: &PgPool, office: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT language FROM workspaces WHERE id = $1")
        .bind(office)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
}

fn gather(state: &AppState, office: Uuid, lang: &str, voice: &str, say: &str) -> String {
    let say = if say.is_empty() {
        String::new()
    } else {
        format!("<Say voice=\"{voice}\">{}</Say>", xml(say))
    };
    format!(
        "<Gather input=\"speech dtmf\" numDigits=\"1\" action=\"{}\" method=\"POST\" \
         speechTimeout=\"auto\" language=\"{lang}\">{say}</Gather><Redirect method=\"POST\">{}</Redirect>",
        xml(&webhook_url(state, office, "turn")),
        xml(&webhook_url(state, office, "turn")),
    )
}

/// POST /phone/:office/voice -- a call starts: the greeting.
pub async fn voice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
    body: Bytes,
) -> Result<Response, AppError> {
    let pairs = form_pairs(&body);
    let line = verified(&state, office, "voice", &headers, &pairs).await?;
    let call_sid = field(&pairs, "CallSid").unwrap_or_default();
    if call_sid.is_empty() || call_sid.len() > 64 {
        return Err(AppError::BadRequest("no call".into()));
    }
    let from: Option<String> = field(&pairs, "From").map(|f| f.chars().take(32).collect());
    sqlx::query(
        "INSERT INTO phone_calls (line_id, workspace_id, call_sid, from_number)
         VALUES ($1, $2, $3, $4) ON CONFLICT (call_sid) DO NOTHING",
    )
    .bind(line.id)
    .bind(office)
    .bind(call_sid)
    .bind(&from)
    .execute(&state.pool)
    .await?;
    let name: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id = $1")
        .bind(office)
        .fetch_one(&state.pool)
        .await?;
    let (lang, voice) = voice_for(language_of(&state.pool, office).await.as_deref());
    let greeting = line
        .greeting
        .clone()
        .unwrap_or_else(|| format!("Hello, you've reached {name}. How can I help?"));
    Ok(twiml(gather(&state, office, lang, voice, &greeting)))
}

/// What the caller asked for a person with (or pressed 0).
fn wants_a_person(speech: &str, digits: &str) -> bool {
    if digits == "0" {
        return true;
    }
    let s = speech.to_lowercase();
    [
        "a person",
        "real person",
        "human",
        "operator",
        "someone",
        "speak to the owner",
        "talk to the owner",
        "representative",
    ]
    .iter()
    .any(|w| s.contains(w))
}

struct Turn {
    say: String,
    transfer: bool,
    end: bool,
    records: i32,
}

/// The receptionist's answer to what the caller said.
async fn answer(
    state: &AppState,
    line: &LineRow,
    transcript: &[Value],
    speech: &str,
) -> Result<Turn, &'static str> {
    let pool = &state.pool;
    let business = crate::office::context::business_of_thread(pool, line.thread_id)
        .await
        .ok_or("the office is gone")?;
    // Only the apps the Receptionist may fill (bookings, leads, messages).
    let access = crate::office::access::effective(pool, line.workspace_id, line.agent_id).await;
    let apps: Vec<_> = crate::office::context::apps_of(pool, line.workspace_id)
        .await
        .into_iter()
        .filter(|a| access.may_use_app(&a.slug))
        .collect();
    let mut system = format!(
        "You are the receptionist answering the phone for {}. You speak to the caller by voice: \
         reply in one to three short, warm sentences, plain words only (no lists, no markdown, no emoji, no links). \
         Ask one question at a time. Take bookings and messages: once you have the details (name, what, when, a phone number), \
         add a record as described below and confirm it in one sentence. Don't make up prices, times or promises the business \
         didn't give you; offer to take a message instead. \
         If the caller asks for a person, or you can't help, end your reply with a line that says only TRANSFER. \
         When the caller is done, say goodbye and end with a line that says only END_CALL.",
        business.name
    );
    system.push_str(&crate::office::context::prompt(&business, &apps));
    let mut convo = String::new();
    for t in transcript.iter().rev().take(MAX_TURNS).rev() {
        let who = if t["who"] == "caller" {
            "Caller"
        } else {
            "You"
        };
        convo.push_str(&format!("{who}: {}\n", t["text"].as_str().unwrap_or("")));
    }
    let prompt = format!("The call so far:\n{convo}Caller: {speech}\n\nYour reply:");

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
            action_type: "phone_turn".into(),
        };
        let gate = crate::office::budget_gate::check_and_deduct(
            pool,
            line.workspace_id,
            &proposal,
            None,
            true,
            crate::office::budget_gate::Spender::new(line.workspace_id, Some(line.agent_id)),
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
    let mut end = false;
    let kept: Vec<&str> = reply
        .lines()
        .filter(|l| match l.trim().trim_matches('*').trim() {
            "TRANSFER" => {
                transfer = true;
                false
            }
            "END_CALL" => {
                end = true;
                false
            }
            _ => true,
        })
        .collect();
    let text = kept.join("\n");
    // Bookings and messages it took go into the office's apps.
    let (text, _, cards) =
        crate::office::context::apply_records(pool, line.thread_id, line.agent_id, &text).await;
    let say: String = text
        .replace(['*', '#', '_', '`'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    Ok(Turn {
        say: if say.is_empty() { "Okay.".into() } else { say },
        transfer,
        end,
        records: cards.len() as i32,
    })
}

/// POST /phone/:office/turn -- the caller said something (or pressed a key).
pub async fn turn(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
    body: Bytes,
) -> Result<Response, AppError> {
    let pairs = form_pairs(&body);
    let line = verified(&state, office, "turn", &headers, &pairs).await?;
    let call_sid = field(&pairs, "CallSid").unwrap_or_default().to_string();
    let call: Option<(Uuid, Value, Option<String>, i32)> = sqlx::query_as(
        "SELECT id, transcript, from_number, silent_turns FROM phone_calls
         WHERE call_sid = $1 AND workspace_id = $2",
    )
    .bind(&call_sid)
    .bind(office)
    .fetch_optional(&state.pool)
    .await?;
    let Some((call_id, transcript, from, silent)) = call else {
        return Err(AppError::NotFound);
    };
    let (lang, voice) = voice_for(language_of(&state.pool, office).await.as_deref());
    let speech: String = field(&pairs, "SpeechResult")
        .unwrap_or("")
        .trim()
        .chars()
        .take(1000)
        .collect();
    let digits = field(&pairs, "Digits").unwrap_or("").trim().to_string();
    let say = |text: &str| format!("<Say voice=\"{voice}\">{}</Say>", xml(text));
    let hand_over = |line: &LineRow| {
        match &line.forward_to {
        Some(to) => format!(
            "{}<Dial callerId=\"{}\">{}</Dial>",
            say("One moment, I'll put you through."),
            xml(&line.phone_number),
            xml(to)
        ),
        None => say(
            "There's nobody free to take the call right now, so I've passed on that you called. Goodbye.",
        ) + "<Hangup/>",
    }
    };

    // Nothing heard: ask again once, then say goodbye.
    if speech.is_empty() && digits.is_empty() {
        sqlx::query("UPDATE phone_calls SET silent_turns = silent_turns + 1, updated_at = now() WHERE id = $1")
            .bind(call_id)
            .execute(&state.pool)
            .await?;
        if silent >= 1 {
            return Ok(twiml(format!(
                "{}<Hangup/>",
                say("I can't hear you. Please call again. Goodbye.")
            )));
        }
        return Ok(twiml(gather(
            &state,
            office,
            lang,
            voice,
            "Sorry, I didn't catch that. How can I help?",
        )));
    }

    let mut turns = transcript.as_array().cloned().unwrap_or_default();
    let caller_text = if speech.is_empty() {
        format!("(pressed {digits})")
    } else {
        speech.clone()
    };
    let (turn, failed) = if wants_a_person(&speech, &digits) {
        (
            Turn {
                say: String::new(),
                transfer: true,
                end: false,
                records: 0,
            },
            false,
        )
    } else {
        match answer(&state, &line, &turns, &caller_text).await {
            Ok(t) => (t, false),
            Err(why) => {
                tracing::warn!(office = %office, why, "a phone answer failed");
                (
                    Turn {
                        say: String::new(),
                        transfer: true,
                        end: false,
                        records: 0,
                    },
                    true,
                )
            }
        }
    };
    turns.push(json!({ "who": "caller", "text": caller_text }));
    if !turn.say.is_empty() {
        turns.push(json!({ "who": "receptionist", "text": turn.say }));
    }
    sqlx::query(
        "UPDATE phone_calls SET transcript = $2, records_added = records_added + $3,
                transferred = transferred OR $4, silent_turns = 0, updated_at = now()
         WHERE id = $1",
    )
    .bind(call_id)
    .bind(Value::Array(turns))
    .bind(turn.records)
    .bind(turn.transfer)
    .execute(&state.pool)
    .await?;

    // The call shows live in the office's Phone calls room.
    let who = from.as_deref().unwrap_or("A caller");
    let note = if turn.say.is_empty() {
        format!("📞 {who}: {caller_text}\n\n_(handing over to a person)_")
    } else {
        format!("📞 {who}: {caller_text}\n\n{}", turn.say)
    };
    crate::handlers::threads::insert_bot_note(
        &state.pool,
        &state.note_sender,
        line.thread_id,
        line.agent_id,
        &note,
        None,
    )
    .await;

    let mut out = String::new();
    if failed {
        out.push_str(&say("Sorry, I'm having trouble right now."));
    }
    if !turn.say.is_empty() {
        out.push_str(&say(&turn.say));
    }
    if turn.transfer {
        out.push_str(&hand_over(&line));
    } else if turn.end {
        out.push_str("<Hangup/>");
    } else {
        out.push_str(&gather(&state, office, lang, voice, ""));
    }
    Ok(twiml(out))
}

/// POST /phone/:office/status -- the call ended: one line in the office chat.
pub async fn status_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
    body: Bytes,
) -> Result<Response, AppError> {
    let pairs = form_pairs(&body);
    let line = verified(&state, office, "status", &headers, &pairs).await?;
    let finished = matches!(
        field(&pairs, "CallStatus"),
        Some("completed" | "busy" | "no-answer" | "failed" | "canceled")
    );
    if !finished {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let call_sid = field(&pairs, "CallSid").unwrap_or_default();
    // Once per call, however often Twilio says so.
    let ended: Option<(Option<String>, Value, i32, bool)> = sqlx::query_as(
        "UPDATE phone_calls SET ended_at = now(), updated_at = now()
         WHERE call_sid = $1 AND workspace_id = $2 AND ended_at IS NULL
         RETURNING from_number, transcript, records_added, transferred",
    )
    .bind(call_sid)
    .bind(office)
    .fetch_optional(&state.pool)
    .await?;
    if let Some((from, transcript, records, transferred)) = ended {
        let said = transcript
            .as_array()
            .map(|t| t.iter().filter(|x| x["who"] == "caller").count())
            .unwrap_or(0);
        let mut line_text = format!(
            "📞 Call from {} ended",
            from.as_deref().unwrap_or("an unknown number")
        );
        match records {
            0 => {}
            1 => line_text.push_str(": 1 record added"),
            n => line_text.push_str(&format!(": {n} records added")),
        }
        if transferred {
            line_text.push_str(", handed over to a person");
        }
        if said == 0 {
            line_text.push_str(" (the caller said nothing)");
        }
        line_text.push_str(". The conversation is in Phone calls.");
        match crate::office::group::ensure_office_chat(&state.pool, line.owner_user_id, office)
            .await
        {
            Ok(chat) => {
                crate::handlers::threads::insert_bot_note(
                    &state.pool,
                    &state.note_sender,
                    chat,
                    line.agent_id,
                    &line_text,
                    Some(json!([{ "label": "Open Phone calls", "kind": "open", "target": format!("room:{}", line.thread_id) }])),
                )
                .await;
            }
            Err(e) => tracing::warn!(error = ?e, "no office chat for a call note"),
        }
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twilios_signature_is_checked() {
        use hmac::Mac;
        let pairs = vec![
            ("To".to_string(), "+18005551212".to_string()),
            ("CallSid".to_string(), "CA1234567890ABCDE".to_string()),
            ("From".to_string(), "+12349013030".to_string()),
        ];
        let url = "https://example.com/phone/1/voice";
        // By hand: the URL, then each field sorted by name, as name+value.
        let signed = "https://example.com/phone/1/voiceCallSidCA1234567890ABCDEFrom+12349013030To+18005551212";
        let mut mac = <hmac::Hmac<sha1::Sha1> as Mac>::new_from_slice(b"12345").expect("key");
        mac.update(signed.as_bytes());
        let good = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        assert!(signed_by_twilio("12345", url, &pairs, &good));
        assert!(!signed_by_twilio("54321", url, &pairs, &good));
        assert!(
            !signed_by_twilio("12345", url, &pairs[1..], &good),
            "a field left out"
        );
        assert!(!signed_by_twilio(
            "12345",
            "https://example.com/phone/2/voice",
            &pairs,
            &good
        ));
        assert!(!signed_by_twilio("12345", url, &pairs, "not base64"));
    }

    #[test]
    fn numbers_forms_and_people() {
        assert!(is_phone("+15551234567"));
        assert!(!is_phone("15551234567") && !is_phone("+0555") && !is_phone("+1555abc4567"));
        let pairs = form_pairs(b"SpeechResult=I%27d+like+a+table&Digits=");
        assert_eq!(field(&pairs, "SpeechResult"), Some("I'd like a table"));
        assert_eq!(field(&pairs, "Digits"), Some(""));
        assert!(wants_a_person("Can I talk to a real person?", ""));
        assert!(wants_a_person("", "0"));
        assert!(!wants_a_person("A table for two at seven", ""));
        assert_eq!(xml("a<b & \"c\""), "a&lt;b &amp; &quot;c&quot;");
        assert_eq!(voice_for(Some("es")).0, "es-ES");
        assert_eq!(voice_for(None).1, "Polly.Joanna-Neural");
    }
}
