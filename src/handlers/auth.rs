use askama::Template;
use axum::extract::{Query, State};
use axum::http::header::USER_AGENT;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Form, Json};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::auth::{self, secret, Caller, OptionalCaller, EMAIL_TOKEN_TTL};
use crate::error::AppError;
use crate::models::{EmailToken, User};
use crate::state::AppState;

use super::{render, wants_html};

#[derive(Template)]
#[template(path = "signin.html")]
struct LoginTemplate<'a> {
    error: Option<&'a str>,
}

#[derive(Template)]
#[template(path = "handle.html")]
struct HandleTemplate<'a> {
    error: Option<&'a str>,
}

pub async fn login_form(OptionalCaller(caller): OptionalCaller) -> Result<Response, AppError> {
    if let Some(Caller::User(user)) = caller {
        return Ok(Redirect::to(if user.handle.is_some() {
            "/inbox"
        } else {
            "/onboarding/handle"
        })
        .into_response());
    }
    Ok(render(LoginTemplate { error: None })?.into_response())
}

#[derive(Template)]
#[template(path = "check_email.html")]
struct CheckEmailTemplate;

#[derive(Deserialize)]
pub struct LoginForm {
    pub email: String,
    pub idempotency_key: Option<String>,
}

/// Human sign-in and sign-up are the same flow: request a magic link for an
/// email, and if that email has no user yet, the callback below creates
/// one. This is a browser-only endpoint (the device flow is how agents
/// authenticate), so it only accepts a form post.
pub async fn request_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Result<Response, AppError> {
    let email = form.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') || email.len() > 254 {
        let msg = "enter a valid email address";
        if wants_html(&headers) {
            return Ok(render(LoginTemplate { error: Some(msg) })?.into_response());
        }
        return Err(AppError::BadRequest(msg.to_string()));
    }

    let key = crate::idempotency::key(&headers, form.idempotency_key.as_deref())?;
    let mut tx = state.pool.begin().await?;
    if let Some(key) = key {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("login:{email}:{key}"))
            .execute(&mut *tx)
            .await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM email_tokens WHERE email = $1 AND idempotency_key = $2)",
        )
        .bind(&email)
        .bind(key)
        .fetch_one(&mut *tx)
        .await?;
        if exists {
            return login_sent_response(&headers);
        }
    }
    let id = Uuid::new_v4();
    let raw_secret = secret::random_hex_token(32);
    let token_hash = secret::hash_secret(&raw_secret).map_err(|_| AppError::Internal)?;
    let expires_at = Utc::now() + EMAIL_TOKEN_TTL;

    sqlx::query(
        "INSERT INTO email_tokens (id, email, token_hash, purpose, expires_at, idempotency_key)
         VALUES ($1, $2, $3, 'login', $4, $5)",
    )
    .bind(id)
    .bind(&email)
    .bind(&token_hash)
    .bind(expires_at)
    .bind(key)
    .execute(&mut *tx)
    .await?;

    // The token's alphabet (hex digits, a hyphenated UUID, one '.') is all
    // URL-unreserved, so it needs no percent-encoding in the query string.
    let raw_token = secret::encode_keyed_secret("", id, &raw_secret);
    let link = format!(
        "{}/auth/callback?token={raw_token}",
        state.config.app_base_url
    );

    if let Err(err) = state.mailer.send_magic_link(&email, &link).await {
        tracing::error!(error = %err, "failed to send magic link email");
        let message = "We couldn't send your sign-in email. Please try again shortly.";
        if wants_html(&headers) {
            return Ok((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                render(LoginTemplate {
                    error: Some(message),
                })?,
            )
                .into_response());
        }
        return Err(AppError::ServiceUnavailable(message.to_string()));
    }
    // SMTP and Postgres cannot commit atomically. A crash after provider acceptance
    // but before commit can require a fresh link; never persist the raw token.
    tx.commit().await?;
    tracing::info!(token_id = %id, "magic link accepted by configured mail transport");
    login_sent_response(&headers)
}

fn login_sent_response(headers: &HeaderMap) -> Result<Response, AppError> {
    // Sign-in and sign-up are the same flow (an unknown email just gets a
    // user row at callback time), so this response is truthful either way
    // -- no enumeration hedge needed.
    let message = "The link works once and expires in 3 hours.";
    if wants_html(headers) {
        Ok(render(CheckEmailTemplate)?.into_response())
    } else {
        Ok(Json(json!({ "message": message })).into_response())
    }
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub token: String,
}

pub async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, AppError> {
    let Some((token_id, raw_secret)) = secret::decode_keyed_secret("", &query.token) else {
        return invalid_link_response(&headers);
    };

    let token = sqlx::query_as::<_, EmailToken>("SELECT * FROM email_tokens WHERE id = $1")
        .bind(token_id)
        .fetch_optional(&state.pool)
        .await?;

    let Some(token) = token else {
        return invalid_link_response(&headers);
    };

    let valid = token.consumed_at.is_none()
        && token.expires_at > Utc::now()
        && secret::verify_secret(&raw_secret, &token.token_hash);
    if !valid {
        return invalid_link_response(&headers);
    }

    let mut tx = state.pool.begin().await?;
    let claimed = sqlx::query("UPDATE email_tokens SET consumed_at = now(), updated_at = now() WHERE id = $1 AND consumed_at IS NULL AND expires_at > now() AND purpose = 'login'")
        .bind(token_id)
        .execute(&mut *tx)
        .await?;
    if claimed.rows_affected() != 1 {
        return invalid_link_response(&headers);
    }

    let user = find_or_create_user_by_email(&mut tx, &token.email).await?;

    let user_agent = headers.get(USER_AGENT).and_then(|v| v.to_str().ok());
    let ip = client_ip(&headers);
    let raw_session = auth::create_session(&mut *tx, user.id, user_agent, ip.as_deref()).await?;
    tx.commit().await?;
    crate::office::ensure_starter_bots_logged(&state.pool, user.id).await;
    let secure = state.config.app_base_url.starts_with("https");
    let jar = CookieJar::new().add(auth::build_session_cookie(raw_session, secure));

    if user.handle.is_none() {
        if wants_html(&headers) {
            return Ok((jar, Redirect::to("/onboarding/handle")).into_response());
        }
        return Ok((jar, Json(json!({ "next": "/onboarding/handle" }))).into_response());
    }

    if wants_html(&headers) {
        Ok((jar, Redirect::to("/inbox")).into_response())
    } else {
        Ok((
            jar,
            Json(json!({ "message": "signed in", "handle": user.handle })),
        )
            .into_response())
    }
}

fn invalid_link_response(headers: &HeaderMap) -> Result<Response, AppError> {
    let msg = "this link is invalid or has expired";
    if wants_html(headers) {
        Ok(render(LoginTemplate { error: Some("This link has expired or was already used. Enter your email to get a new sign-in link.") })?.into_response())
    } else {
        Err(AppError::BadRequest(msg.to_string()))
    }
}

async fn find_or_create_user_by_email(
    pool: &mut sqlx::PgConnection,
    email: &str,
) -> Result<User, AppError> {
    // ON CONFLICT DO NOTHING + a follow-up SELECT handles the (very rare)
    // race of the same brand-new email being verified twice concurrently,
    // within the callback's token-consumption transaction.
    sqlx::query(
        "INSERT INTO users (email) VALUES ($1)
         ON CONFLICT (email) WHERE deleted_at IS NULL DO NOTHING",
    )
    .bind(email)
    .execute(&mut *pool)
    .await?;

    let user =
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE email = $1 AND deleted_at IS NULL")
            .bind(email)
            .fetch_one(&mut *pool)
            .await?;
    Ok(user)
}

pub async fn handle_form(caller: Caller) -> Result<Response, AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };
    if user.handle.is_some() {
        return Ok(Redirect::to("/inbox").into_response());
    }
    Ok(render(HandleTemplate { error: None })?.into_response())
}

#[derive(Deserialize)]
pub struct HandleForm {
    pub handle: String,
    pub idempotency_key: Option<String>,
}

pub async fn set_handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    caller: Caller,
    Form(form): Form<HandleForm>,
) -> Result<Response, AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };

    let handle = form.handle.trim();
    crate::idempotency::key(&headers, form.idempotency_key.as_deref())?;
    if !valid_handle(handle) {
        let msg = "handles are 3-32 characters: letters, numbers, underscore";
        if wants_html(&headers) {
            return Ok(render(HandleTemplate { error: Some(msg) })?.into_response());
        }
        return Err(AppError::BadRequest(msg.to_string()));
    }

    let result = sqlx::query("UPDATE users SET handle = $1, updated_at = now() WHERE id = $2")
        .bind(handle)
        .bind(user.id)
        .execute(&state.pool)
        .await;

    if let Err(err) = &result {
        if is_unique_violation(err) {
            let msg = "that handle is taken";
            if wants_html(&headers) {
                return Ok(render(HandleTemplate { error: Some(msg) })?.into_response());
            }
            return Err(AppError::Conflict(msg.to_string()));
        }
    }
    result?;

    if wants_html(&headers) {
        Ok(Redirect::to("/inbox").into_response())
    } else {
        Ok(Json(json!({ "handle": handle })).into_response())
    }
}

fn valid_handle(h: &str) -> bool {
    (3..=32).contains(&h.len()) && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(super) fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .is_some_and(|e| e.is_unique_violation())
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, AppError> {
    if let Some(cookie) = jar.get(auth::SESSION_COOKIE) {
        crate::idempotency::key(&headers, None)?;
        auth::revoke_session(&state.pool, cookie.value()).await?;
    }
    let secure = state.config.app_base_url.starts_with("https");
    let jar = CookieJar::new().add(auth::build_logout_cookie(secure));

    if wants_html(&headers) {
        Ok((jar, Redirect::to("/signin")).into_response())
    } else {
        Ok((jar, Json(json!({ "message": "signed out" }))).into_response())
    }
}

pub(super) fn client_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_string())
}

pub async fn current_user(
    State(state): State<AppState>,
    OptionalCaller(caller): OptionalCaller,
) -> Result<Response, AppError> {
    match caller {
        Some(Caller::User(user)) => {
            // Effective, not stored: the bootstrap admin (earliest account
            // while nobody is admin) must see the Admin screen in the app.
            let is_admin = super::admin::is_effective_admin(&state.pool, &user).await?;
            let ui_theme: Option<serde_json::Value> =
                sqlx::query_scalar("SELECT ui_theme FROM users WHERE id = $1")
                    .bind(user.id)
                    .fetch_one(&state.pool)
                    .await?;
            Ok(Json(json!({
            "authenticated": true,
            "id": user.id,
            "handle": user.handle,
            "email": user.email,
            "display_name": user.display_name,
            "avatar_url": user.avatar_url,
            "is_admin": is_admin,
            "is_agent": false,
            "ui_theme": ui_theme,
            "local_mode": state.config.local.is_some(),
            }))
            .into_response())
        }
        Some(Caller::Agent(agent)) => Ok(Json(json!({
            "authenticated": true,
            "id": agent.id,
            "handle": agent.handle,
            "email": null,
            "display_name": Some(agent.name),
            "avatar_url": null,
            "is_admin": false,
            "is_agent": true,
        }))
        .into_response()),
        None => Ok(Json(json!({
            "authenticated": false,
            "id": null,
            "handle": null,
            "email": null,
            "display_name": null,
            "avatar_url": null,
            "is_admin": false,
            "is_agent": false,
        }))
        .into_response()),
    }
}

#[derive(Deserialize)]
pub struct SaveThemeRequest {
    /// The theme to keep, or `null` to go back to the default.
    pub theme: Option<serde_json::Value>,
    pub idempotency_key: Option<String>,
}

fn is_hex_colour(value: &str) -> bool {
    value.len() == 7 && value.starts_with('#') && value[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Checks a theme field by field and returns only the known keys, so the
/// stored value can never carry arbitrary data back into the app's CSS.
fn normalise_theme(raw: &serde_json::Value) -> Result<serde_json::Value, AppError> {
    let bad = |what: &str| AppError::BadRequest(format!("theme: {what}"));
    let obj = raw.as_object().ok_or_else(|| bad("must be an object"))?;
    if raw.to_string().len() > 2048 {
        return Err(bad("too large"));
    }
    let mut out = serde_json::Map::new();
    for (key, value) in obj {
        let normalised = match key.as_str() {
            "accent" | "background" | "surface" | "border" | "text" | "userBubble" => {
                let colour = value
                    .as_str()
                    .filter(|v| is_hex_colour(v))
                    .ok_or_else(|| bad(&format!("{key} must be a #rrggbb colour")))?;
                json!(colour.to_ascii_lowercase())
            }
            "mode" => match value.as_str() {
                Some(m @ ("dark" | "light")) => json!(m),
                _ => return Err(bad("mode must be dark or light")),
            },
            // Follow the operating system's light or dark setting.
            "followSystem" => match value.as_bool() {
                Some(b) => json!(b),
                None => return Err(bad("followSystem must be true or false")),
            },
            "backgroundStyle" => match value.as_str() {
                Some(m @ ("solid" | "glow")) => json!(m),
                _ => return Err(bad("backgroundStyle must be solid or glow")),
            },
            "radius" | "blur" => {
                let max = if key == "radius" { 32 } else { 40 };
                let n = value
                    .as_i64()
                    .filter(|n| (0..=max).contains(n))
                    .ok_or_else(|| bad(&format!("{key} must be a whole number 0-{max}")))?;
                json!(n)
            }
            other => return Err(bad(&format!("unknown setting '{other}'"))),
        };
        out.insert(key.clone(), normalised);
    }
    Ok(serde_json::Value::Object(out))
}

/// PUT /api/me/theme -- keep a person's appearance settings so their theme
/// follows them to every browser and the desktop app. Saving the same
/// theme twice is harmless, so a retried request needs no special replay.
pub async fn save_theme(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Json(body): Json<SaveThemeRequest>,
) -> Result<Response, AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let theme = body.theme.as_ref().map(normalise_theme).transpose()?;
    sqlx::query("UPDATE users SET ui_theme = $1, updated_at = now() WHERE id = $2")
        .bind(&theme)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "ui_theme": theme })).into_response())
}

#[derive(Deserialize, Default)]
pub struct DevLoginRequest {
    pub email: Option<String>,
    pub idempotency_key: Option<String>,
}

/// Email domains the dev sign-in may create or enter. It never proves
/// control of an inbox, so it must not be able to enter a real account.
const DEV_LOGIN_DOMAINS: [&str; 2] = ["@airobot.local", "@example.com"];

/// One-click sign-in for local development. Off unless `AIR_DEV_LOGIN=1`
/// is set (config.rs refuses that in production) and limited to reserved
/// test domains, so a reachable dev server cannot be used to sign in as an
/// arbitrary user.
pub async fn dev_login(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Json(body): Json<DevLoginRequest>,
) -> Result<Response, AppError> {
    if !state.config.dev_login || state.config.production {
        return Err(AppError::NotFound);
    }
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let email = body
        .email
        .as_deref()
        .unwrap_or("founder@example.com")
        .trim()
        .to_lowercase();
    if !DEV_LOGIN_DOMAINS.iter().any(|d| email.ends_with(d)) || email.len() > 254 {
        return Err(AppError::BadRequest(
            "dev sign-in only accepts @airobot.local or @example.com addresses".into(),
        ));
    }
    let email = email.as_str();
    let mut tx = state.pool.begin().await?;
    let mut user = find_or_create_user_by_email(&mut tx, email).await?;
    if user.handle.is_none() {
        let base_handle: String = email
            .split('@')
            .next()
            .unwrap_or("user")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(24)
            .collect();
        let base_handle = if base_handle.len() < 3 {
            "dev_user".to_string()
        } else {
            base_handle
        };
        let taken: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE handle = $1)")
                .bind(&base_handle)
                .fetch_one(&mut *tx)
                .await?;

        let chosen_handle = if taken {
            format!(
                "{}_{}",
                base_handle,
                &uuid::Uuid::new_v4().simple().to_string()[..4]
            )
        } else {
            base_handle
        };

        sqlx::query("UPDATE users SET handle = $1, updated_at = now() WHERE id = $2")
            .bind(&chosen_handle)
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
        user.handle = Some(chosen_handle);
    }
    let user_agent = headers.get(USER_AGENT).and_then(|v| v.to_str().ok());
    let ip = client_ip(&headers);
    let raw_session = auth::create_session(&mut *tx, user.id, user_agent, ip.as_deref()).await?;
    tx.commit().await?;
    crate::office::ensure_starter_bots_logged(&state.pool, user.id).await;

    let secure = state.config.app_base_url.starts_with("https");
    let jar = jar.add(auth::build_session_cookie(raw_session, secure));

    Ok((
        jar,
        Json(json!({
            "authenticated": true,
            "id": user.id,
            "email": user.email,
            "handle": user.handle,
        })),
    )
        .into_response())
}

/// Detects the server's outbound LAN IPv4 address so mobile devices on the same Wi-Fi can connect.
pub fn detect_lan_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let local_addr = socket.local_addr().ok()?;
    match local_addr.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => {
            Some(ip.to_string())
        }
        _ => None,
    }
}

/// POST /api/auth/qr-link — Generates a single-use phone login token and reachable QR URL
pub async fn create_qr_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    caller: Caller,
) -> Result<Response, AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };
    crate::idempotency::key(&headers, None)?;

    let id = Uuid::new_v4();
    let raw_secret = secret::random_hex_token(32);
    let token_hash = secret::hash_secret(&raw_secret).map_err(|_| AppError::Internal)?;
    let expires_at = Utc::now() + chrono::Duration::seconds(300);

    sqlx::query(
        "INSERT INTO email_tokens (id, email, token_hash, purpose, expires_at)
         VALUES ($1, $2, $3, 'qr_login', $4)",
    )
    .bind(id)
    .bind(&user.email)
    .bind(&token_hash)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;

    let raw_token = secret::encode_keyed_secret("", id, &raw_secret);

    let base = if state.config.app_base_url.contains("localhost")
        || state.config.app_base_url.contains("127.0.0.1")
    {
        if let Some(lan_ip) = detect_lan_ip() {
            format!("http://{}:{}", lan_ip, state.config.port)
        } else {
            state.config.app_base_url.clone()
        }
    } else {
        state.config.app_base_url.clone()
    };

    let qr_url = format!("{}/auth/qr?token={}", base.trim_end_matches('/'), raw_token);

    Ok(Json(json!({
        "qr_url": qr_url,
        "expires_in_seconds": 300,
        "email": user.email,
        "handle": user.handle,
    }))
    .into_response())
}

/// GET /auth/qr?token=... — Consumes the QR token and logs the phone in with a 303 redirect
pub async fn qr_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, AppError> {
    let Some((token_id, raw_secret)) = secret::decode_keyed_secret("", &query.token) else {
        return Err(AppError::BadRequest(
            "invalid or expired qr code".to_string(),
        ));
    };

    let token = sqlx::query_as::<_, EmailToken>(
        "SELECT * FROM email_tokens WHERE id = $1 AND purpose = 'qr_login'",
    )
    .bind(token_id)
    .fetch_optional(&state.pool)
    .await?;

    let Some(token) = token else {
        return Err(AppError::BadRequest(
            "invalid or expired qr code".to_string(),
        ));
    };

    let valid = token.consumed_at.is_none()
        && token.expires_at > Utc::now()
        && secret::verify_secret(&raw_secret, &token.token_hash);

    if !valid {
        return Err(AppError::Unauthorized);
    }

    let mut tx = state.pool.begin().await?;
    let claimed = sqlx::query(
        "UPDATE email_tokens SET consumed_at = now(), updated_at = now()
         WHERE id = $1 AND consumed_at IS NULL AND expires_at > now() AND purpose = 'qr_login'",
    )
    .bind(token_id)
    .execute(&mut *tx)
    .await?;

    if claimed.rows_affected() != 1 {
        return Err(AppError::BadRequest("qr code already consumed".to_string()));
    }

    let user =
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE email = $1 AND deleted_at IS NULL")
            .bind(&token.email)
            .fetch_one(&mut *tx)
            .await?;

    let user_agent = headers.get(USER_AGENT).and_then(|v| v.to_str().ok());
    let ip = client_ip(&headers);
    let raw_session = auth::create_session(&mut *tx, user.id, user_agent, ip.as_deref()).await?;
    tx.commit().await?;
    crate::office::ensure_starter_bots_logged(&state.pool, user.id).await;

    let secure = state.config.app_base_url.starts_with("https");
    let jar = CookieJar::new().add(auth::build_session_cookie(raw_session, secure));

    // 303 See Other redirect to /?paired=phone
    Ok((jar, Redirect::to("/?paired=phone")).into_response())
}
