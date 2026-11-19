pub mod secret;

use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::{Agent, Session, User};
use crate::state::AppState;

fn authentication_error(parts: &Parts) -> AppError {
    let browser_request = parts
        .headers
        .get(axum::http::header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/html"));
    if browser_request {
        AppError::SignInRequired
    } else {
        AppError::Unauthorized
    }
}

pub const SESSION_COOKIE: &str = "air_session";
pub const SESSION_TTL: Duration = Duration::days(30);
pub const EMAIL_TOKEN_TTL: Duration = Duration::hours(3);
pub const DEVICE_CODE_TTL: Duration = Duration::minutes(10);
const AGENT_KEY_PREFIX: &str = "air_ag_";

/// Who's making the request, resolved from either a session cookie (a
/// signed-in human) or a `Bearer` agent key.
#[derive(Debug, Clone)]
pub enum Caller {
    User(User),
    Agent(Agent),
}

impl Caller {
    /// The owning human either way: the user themselves, or the human who
    /// owns the calling agent. Threads and notes are always attributed to
    /// this id.
    pub fn user_id(&self) -> Uuid {
        match self {
            Caller::User(u) => u.id,
            Caller::Agent(a) => a.user_id,
        }
    }
}

impl FromRequestParts<AppState> for Caller {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(header) = parts.headers.get(AUTHORIZATION) {
            let value = header.to_str().map_err(|_| authentication_error(parts))?;
            let raw_key = value
                .strip_prefix("Bearer ")
                .ok_or_else(|| authentication_error(parts))?;
            let agent = resolve_agent_key(&state.pool, raw_key)
                .await?
                .ok_or_else(|| authentication_error(parts))?;
            touch_agent_last_seen(&state.pool, agent.id).await?;
            return Ok(Caller::Agent(agent));
        }

        let jar = CookieJar::from_headers(&parts.headers);
        let raw = jar
            .get(SESSION_COOKIE)
            .map(|c| c.value().to_string())
            .ok_or_else(|| authentication_error(parts))?;
        let user = resolve_session(&state.pool, &raw)
            .await?
            .ok_or_else(|| authentication_error(parts))?;
        Ok(Caller::User(user))
    }
}

/// Same resolution as `Caller`, but never rejects -- `None` when there's no
/// cookie/Bearer key, or it doesn't resolve to anything live. Public and
/// unlisted threads (see handlers::threads) are readable without being
/// signed in at all, so their GET handlers need to know who's asking
/// without requiring it.
pub struct OptionalCaller(pub Option<Caller>);

impl FromRequestParts<AppState> for OptionalCaller {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(OptionalCaller(
            Caller::from_request_parts(parts, state).await.ok(),
        ))
    }
}

/// Creates a session row and returns the raw cookie value. The raw value is
/// only ever handed back here -- the DB only ever holds its argon2 hash.
pub async fn create_session(
    pool: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: Uuid,
    user_agent: Option<&str>,
    ip: Option<&str>,
) -> Result<String, AppError> {
    let id = Uuid::new_v4();
    let raw_secret = secret::random_hex_token(32);
    let token_hash = secret::hash_secret(&raw_secret).map_err(|_| AppError::Internal)?;
    let expires_at = Utc::now() + SESSION_TTL;

    sqlx::query(
        "INSERT INTO sessions (id, user_id, token_hash, expires_at, user_agent, ip)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(user_id)
    .bind(&token_hash)
    .bind(expires_at)
    .bind(user_agent)
    .bind(ip)
    .execute(pool)
    .await?;

    Ok(secret::encode_keyed_secret("", id, &raw_secret))
}

async fn resolve_session(pool: &PgPool, raw: &str) -> Result<Option<User>, AppError> {
    let Some((session_id, raw_secret)) = secret::decode_keyed_secret("", raw) else {
        return Ok(None);
    };

    let session = sqlx::query_as::<_, Session>(
        "SELECT * FROM sessions WHERE id = $1 AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;

    let Some(session) = session else {
        return Ok(None);
    };
    if !secret::verify_secret_memoized(&raw_secret, &session.token_hash) {
        return Ok(None);
    }

    let user =
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1 AND deleted_at IS NULL")
            .bind(session.user_id)
            .fetch_optional(pool)
            .await?;
    Ok(user)
}

pub async fn revoke_session(pool: &PgPool, raw: &str) -> Result<(), AppError> {
    let Some((session_id, _)) = secret::decode_keyed_secret("", raw) else {
        return Ok(());
    };
    sqlx::query("UPDATE sessions SET revoked_at = now(), updated_at = now() WHERE id = $1")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Mints a fresh agent key for a newly-created agent row. The raw key is
/// returned exactly once by the caller (the device-flow poll) and never
/// stored -- only its argon2 hash is.
pub fn generate_agent_key(agent_id: Uuid) -> (String, String, String) {
    let raw_secret = secret::random_hex_token(32);
    let raw_key = secret::encode_keyed_secret(AGENT_KEY_PREFIX, agent_id, &raw_secret);
    let prefix_for_display = raw_key.chars().take(16).collect::<String>();
    (raw_key, raw_secret, prefix_for_display)
}

async fn resolve_agent_key(pool: &PgPool, raw: &str) -> Result<Option<Agent>, AppError> {
    let Some((agent_id, raw_secret)) = secret::decode_keyed_secret(AGENT_KEY_PREFIX, raw) else {
        return Ok(None);
    };

    let agent = sqlx::query_as::<_, Agent>(
        "SELECT * FROM agents WHERE id = $1 AND revoked_at IS NULL AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await?;

    let Some(agent) = agent else {
        return Ok(None);
    };
    if !secret::verify_secret_memoized(&raw_secret, &agent.key_hash) {
        return Ok(None);
    }
    Ok(Some(agent))
}

/// Presence only needs ~30-second resolution ("quiet" means hours), so a
/// polling agent writes this row at most twice a minute instead of on every
/// request.
async fn touch_agent_last_seen(pool: &PgPool, agent_id: Uuid) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE agents SET last_seen_at = now()
         WHERE id = $1 AND (last_seen_at IS NULL OR last_seen_at < now() - interval '30 seconds')",
    )
    .bind(agent_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub fn build_session_cookie(raw_value: String, secure: bool) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, raw_value))
        .path("/")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(cookie_duration(SESSION_TTL))
        .build()
}

pub fn build_logout_cookie(secure: bool) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, ""))
        .path("/")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(cookie_duration(Duration::zero()))
        .build()
}

fn cookie_duration(d: Duration) -> time::Duration {
    time::Duration::seconds(d.num_seconds())
}
