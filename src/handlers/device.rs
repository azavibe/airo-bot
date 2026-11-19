use askama::Template;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use axum::{Form, Json};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{self, secret, Caller, DEVICE_CODE_TTL};
use crate::error::AppError;
use crate::models::LoginCode;
use crate::state::AppState;

use super::auth::is_unique_violation;
use super::{message_response, render, wants_html};

const CODE_LEN: usize = 8;

#[derive(Serialize)]
pub struct DeviceCodeResponse {
    code: String,
    device_code: String,
    verification_url: String,
    expires_in: i64,
}

#[derive(Deserialize, Default)]
pub struct CreateCodeRequest {
    pub idempotency_key: Option<String>,
}

/// No auth required -- this is the first call the CLI makes.
pub async fn create_code(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<CreateCodeRequest>>,
) -> Result<Json<DeviceCodeResponse>, AppError> {
    let body = body.map(|b| b.0).unwrap_or_default();
    let key = crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let expires_at = Utc::now() + DEVICE_CODE_TTL;

    for _ in 0..5 {
        let id = Uuid::new_v4();
        let code = secret::random_code(CODE_LEN);
        let raw_device_secret = secret::random_hex_token(32);
        let device_code = secret::encode_keyed_secret("air_device_", id, &raw_device_secret);
        let device_secret_hash =
            secret::hash_secret(&raw_device_secret).map_err(|_| AppError::Internal)?;
        let result = sqlx::query(
            "INSERT INTO login_codes (id, code, device_secret_hash, expires_at, idempotency_key)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(&code)
        .bind(device_secret_hash)
        .bind(expires_at)
        .bind(key)
        .execute(&state.pool)
        .await;

        match result {
            Ok(_) => {
                let verification_url = format!("{}/device?code={code}", state.config.app_base_url);
                return Ok(Json(DeviceCodeResponse {
                    code,
                    device_code,
                    verification_url,
                    expires_in: DEVICE_CODE_TTL.num_seconds(),
                }));
            }
            // Collision on the (tiny-probability) 8-char code; try again
            // with a fresh one rather than fail the CLI's request.
            Err(ref err) if is_unique_violation(err) => {
                if let Some(key) = key {
                    let exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM login_codes WHERE idempotency_key = $1)",
                    )
                    .bind(key)
                    .fetch_one(&state.pool)
                    .await?;
                    if exists {
                        // One-time secrets cannot be replayed from their Argon2 hashes.
                        return Err(AppError::Conflict("device request already issued; start a new login with a new idempotency key".into()));
                    }
                }
                continue;
            }
            Err(err) => return Err(err.into()),
        }
    }
    Err(AppError::Internal)
}

fn normalize_code(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(char::to_uppercase)
        .collect()
}

#[derive(Template)]
#[template(path = "device.html")]
struct DeviceTemplate<'a> {
    display_code: String,
    suggested_name: String,
    error: Option<&'a str>,
    active_tab: &'static str,
}

/// "XXXX-XXXX" per the UI spec's device screen -- CODE_LEN is 8, so the
/// hyphen always lands in the middle.
fn format_code_for_display(code: &str) -> String {
    if code.len() == CODE_LEN {
        format!("{}-{}", &code[..CODE_LEN / 2], &code[CODE_LEN / 2..])
    } else {
        code.to_string()
    }
}

#[derive(Deserialize)]
pub struct DeviceQuery {
    // Optional: the CLI's verification_url includes it, but per the UI
    // spec this page also works with a bare visit -- the user types the
    // code their terminal is showing.
    pub code: Option<String>,
}

pub async fn verify_form(
    State(state): State<AppState>,
    caller: Caller,
    Query(query): Query<DeviceQuery>,
) -> Result<Html<String>, AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };

    let (display_code, error) = match query.code.filter(|c| !c.trim().is_empty()) {
        Some(raw_code) => {
            let code = normalize_code(&raw_code);
            let row = sqlx::query_as::<_, LoginCode>("SELECT * FROM login_codes WHERE code = $1")
                .bind(&code)
                .fetch_optional(&state.pool)
                .await?;
            (
                format_code_for_display(&code),
                code_state_error(row.as_ref()),
            )
        }
        None => (String::new(), None),
    };

    let suggested_name = next_agent_name(&state.pool, user.id).await?;
    render(DeviceTemplate {
        display_code,
        suggested_name,
        error,
        active_tab: "agents",
    })
}

fn code_state_error(row: Option<&LoginCode>) -> Option<&'static str> {
    match row {
        None => Some("this code is invalid"),
        Some(r) if r.expires_at < Utc::now() => {
            Some("That code has expired. Run `air login` again.")
        }
        Some(r) if r.denied_at.is_some() => Some("this code was already denied"),
        Some(r) if r.approved_at.is_some() => Some("this code was already approved"),
        _ => None,
    }
}

async fn next_agent_name(pool: &PgPool, user_id: Uuid) -> Result<String, AppError> {
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM agents WHERE user_id = $1 AND deleted_at IS NULL")
            .bind(user_id)
            .fetch_one(pool)
            .await?;
    Ok(format!("agent-{}", count + 1))
}

#[derive(Deserialize)]
pub struct ApproveForm {
    pub code: String,
    pub agent_name: String,
    pub idempotency_key: Option<String>,
}

pub async fn approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    caller: Caller,
    Form(form): Form<ApproveForm>,
) -> Result<Response, AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };
    let code = normalize_code(&form.code);
    crate::idempotency::key(&headers, form.idempotency_key.as_deref())?;
    let agent_name = form.agent_name.trim();
    if agent_name.is_empty() || agent_name.chars().count() > 64 {
        return Err(AppError::BadRequest(
            "agent name must be 1-64 characters".to_string(),
        ));
    }

    let already_approved: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM login_codes WHERE code = $1 AND user_id = $2 AND agent_name = $3 AND approved_at IS NOT NULL)")
        .bind(&code).bind(user.id).bind(agent_name).fetch_one(&state.pool).await?;
    if already_approved {
        return device_result_response(&headers, "Connected", "Go back to your terminal.");
    }
    let already_taken: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE user_id = $1 AND name = $2 AND deleted_at IS NULL)",
    )
    .bind(user.id)
    .bind(agent_name)
    .fetch_one(&state.pool)
    .await?;
    if already_taken {
        let msg = "you already have an agent with that name -- pick another";
        if wants_html(&headers) {
            return Ok(render(DeviceTemplate {
                display_code: format_code_for_display(&code),
                suggested_name: agent_name.to_string(),
                error: Some(msg),
                active_tab: "agents",
            })?
            .into_response());
        }
        return Err(AppError::Conflict(msg.to_string()));
    }

    let result = sqlx::query(
        "UPDATE login_codes SET user_id = $1, agent_name = $2, approved_at = now(), updated_at = now()
         WHERE code = $3 AND approved_at IS NULL AND denied_at IS NULL AND expires_at > now()",
    )
    .bind(user.id)
    .bind(agent_name)
    .bind(&code)
    .execute(&state.pool)
    .await?;

    if result.rows_affected() == 0 {
        return device_result_response(
            &headers,
            "Code not available",
            "this code is invalid, expired, or already used",
        );
    }
    device_result_response(&headers, "Connected", "Go back to your terminal.")
}

#[derive(Deserialize)]
pub struct DenyForm {
    pub code: String,
    pub idempotency_key: Option<String>,
}

pub async fn deny(
    State(state): State<AppState>,
    headers: HeaderMap,
    caller: Caller,
    Form(form): Form<DenyForm>,
) -> Result<Response, AppError> {
    let Caller::User(_) = caller else {
        return Err(AppError::Forbidden);
    };
    crate::idempotency::key(&headers, form.idempotency_key.as_deref())?;
    let code = normalize_code(&form.code);

    sqlx::query(
        "UPDATE login_codes SET denied_at = now(), updated_at = now()
         WHERE code = $1 AND approved_at IS NULL AND denied_at IS NULL AND expires_at > now()",
    )
    .bind(&code)
    .execute(&state.pool)
    .await?;

    device_result_response(&headers, "Denied", "The agent was not approved.")
}

fn device_result_response(
    headers: &HeaderMap,
    title: &str,
    body: &str,
) -> Result<Response, AppError> {
    if wants_html(headers) {
        Ok(message_response(title, body)?.into_response())
    } else {
        Ok(Json(json!({ "message": body })).into_response())
    }
}

#[derive(Deserialize)]
pub struct TokenRequest {
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub device_code: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "status")]
pub enum TokenResponse {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "denied")]
    Denied,
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "approved")]
    Approved {
        agent_key: String,
        agent_name: String,
    },
}

/// No auth required -- polled by the CLI every 2 seconds. Rate-limited by
/// IP (see rate_limit.rs) since it has no bearer key to key on yet.
pub async fn poll_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TokenRequest>,
) -> Result<Json<TokenResponse>, AppError> {
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let mut tx = state.pool.begin().await?;
    let row = if let Some(device_code) = body.device_code.as_deref() {
        let Some((id, raw_secret)) = secret::decode_keyed_secret("air_device_", device_code) else {
            return Ok(Json(TokenResponse::Expired));
        };
        let row =
            sqlx::query_as::<_, LoginCode>("SELECT * FROM login_codes WHERE id = $1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        row.filter(|row| {
            row.device_secret_hash
                .as_deref()
                .is_some_and(|hash| secret::verify_secret(&raw_secret, hash))
        })
    } else if let Some(code) = body.code.as_deref() {
        let code = normalize_code(code);
        sqlx::query_as::<_, LoginCode>(
            "SELECT * FROM login_codes
             WHERE code = $1 AND device_secret_hash IS NULL FOR UPDATE",
        )
        .bind(&code)
        .fetch_optional(&mut *tx)
        .await?
    } else {
        None
    };

    let Some(row) = row else {
        return Ok(Json(TokenResponse::Expired));
    };
    if row.expires_at < Utc::now() {
        return Ok(Json(TokenResponse::Expired));
    }
    if row.denied_at.is_some() {
        return Ok(Json(TokenResponse::Denied));
    }
    if row.token_delivered_at.is_some() {
        // Already delivered once -- a repeat poll is treated as expired so
        // a leaked code can't be replayed to fetch the key again.
        return Ok(Json(TokenResponse::Expired));
    }
    let (Some(user_id), Some(agent_name)) = (row.user_id, row.agent_name.clone()) else {
        return Ok(Json(TokenResponse::Pending));
    };

    let agent_id = Uuid::new_v4();
    let (raw_key, raw_secret, key_prefix) = auth::generate_agent_key(agent_id);
    let key_hash = secret::hash_secret(&raw_secret).map_err(|_| AppError::Internal)?;

    let agent_handle = crate::handlers::unique_handle(&state.pool, &agent_name).await;

    let insert = sqlx::query(
        "INSERT INTO agents (id, user_id, name, handle, key_hash, key_prefix) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(agent_id)
    .bind(user_id)
    .bind(&agent_name)
    .bind(&agent_handle)
    .bind(&key_hash)
    .bind(&key_prefix)
    .execute(&mut *tx)
    .await;

    if let Err(err) = &insert {
        if is_unique_violation(err) {
            // The name was taken between approve-time's check and now.
            // Nothing to hand back; the CLI can retry the whole flow.
            return Ok(Json(TokenResponse::Expired));
        }
    }
    insert?;

    sqlx::query(
        "UPDATE login_codes SET agent_id = $1, token_delivered_at = now(), updated_at = now() WHERE id = $2",
    )
    .bind(agent_id)
    .bind(row.id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(TokenResponse::Approved {
        agent_key: raw_key,
        agent_name,
    }))
}
