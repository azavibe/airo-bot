//! Connections: an office's links to Google, Stripe, Shopify and webhooks,
//! and the actions bots run through them.
//!
//! A connection belongs to one office (workspace). People and their bots
//! use the connections of offices they belong to; only people connect,
//! remove, or create webhooks.

use axum::body::Bytes;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{secret, Caller};
use crate::connectors::{self, ActionContext, ConnectorApps, OAuthApp, Provider, ProviderError};
use crate::crypto;
use crate::error::AppError;
use crate::handlers::airo::{
    ensure_workspace_access, get_or_create_default_workspace, record_audit,
};
use crate::state::AppState;

use super::JsonOrForm;

const MAX_CONNECTIONS_PER_OFFICE: i64 = 50;
const OAUTH_STATE_TTL_MINUTES: i64 = 15;
/// Room notes from a webhook are cut here so one payload cannot flood a room.
const MAX_HOOK_NOTE_CHARS: usize = 8_000;

#[derive(sqlx::FromRow)]
struct ConnectionRow {
    id: Uuid,
    workspace_id: Uuid,
    provider: String,
    label: String,
    external_account_id: Option<String>,
    scopes: Vec<String>,
    access_token_enc: Option<String>,
    refresh_token_enc: Option<String>,
    token_expires_at: Option<DateTime<Utc>>,
    config: Value,
    status: String,
    last_used_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    created_at: DateTime<Utc>,
}

impl ConnectionRow {
    fn provider(&self) -> Result<Provider, AppError> {
        Provider::parse(&self.provider).ok_or(AppError::Internal)
    }
}

const ROW_COLUMNS: &str = "id, workspace_id, provider, label, external_account_id, scopes,
    access_token_enc, refresh_token_enc, token_expires_at, config, status,
    last_used_at, last_error, created_at";

fn inbound_url(state: &AppState, row: &ConnectionRow) -> Option<String> {
    let salt = row.config["hook_salt"].as_str()?;
    row.config["thread_id"].as_str()?;
    Some(format!(
        "{}/api/hooks/{}/{}",
        state.config.app_base_url,
        row.id,
        crypto::connection_hook_token(row.id, salt)
    ))
}

/// What people and bots see: never a token.
fn view(state: &AppState, row: &ConnectionRow) -> Value {
    let provider = Provider::parse(&row.provider);
    json!({
        "id": row.id,
        "office_id": row.workspace_id,
        "provider": row.provider,
        "label": row.label,
        "status": row.status,
        "products": if provider == Some(Provider::Google) { connectors::google_products(&row.scopes) } else { Vec::new() },
        "room_id": row.config["thread_id"],
        "outbound_url": row.config["outbound_url"],
        "inbound_url": inbound_url(state, row),
        "last_used_at": row.last_used_at,
        "last_error": row.last_error,
        "created_at": row.created_at,
    })
}

/// The office a request is about: the one named (if the caller belongs to
/// it) or the caller's first office.
async fn resolve_office(
    state: &AppState,
    caller: &Caller,
    workspace_id: Option<Uuid>,
) -> Result<Uuid, AppError> {
    match workspace_id {
        Some(id) => {
            ensure_workspace_access(&state.pool, caller, id).await?;
            Ok(id)
        }
        None => Ok(
            get_or_create_default_workspace(&state.pool, caller.user_id())
                .await?
                .id,
        ),
    }
}

/// A live connection the caller may use, or 404 (never 403, so ids of other
/// people's connections reveal nothing).
async fn load_connection(
    state: &AppState,
    caller: &Caller,
    id: Uuid,
) -> Result<ConnectionRow, AppError> {
    let row = sqlx::query_as::<_, ConnectionRow>(&format!(
        "SELECT {ROW_COLUMNS} FROM connections WHERE id = $1 AND deleted_at IS NULL"
    ))
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    ensure_workspace_access(&state.pool, caller, row.workspace_id)
        .await
        .map_err(|_| AppError::NotFound)?;
    Ok(row)
}

fn people_only(caller: &Caller) -> Result<Uuid, AppError> {
    match caller {
        Caller::User(user) => Ok(user.id),
        Caller::Agent(_) => Err(AppError::Forbidden),
    }
}

fn provider_error(error: ProviderError) -> AppError {
    if error.bad_input {
        AppError::BadRequest(error.public)
    } else if error.needs_reconnect {
        AppError::Conflict(error.public)
    } else {
        AppError::ServiceUnavailable(error.public)
    }
}

// ---------- List ----------

#[derive(Deserialize, Default)]
pub struct OfficeQuery {
    pub workspace_id: Option<Uuid>,
}

/// GET /api/connections -- the catalog and this office's connections.
pub async fn list_connections(
    State(state): State<AppState>,
    caller: Caller,
    Query(query): Query<OfficeQuery>,
) -> Result<Json<Value>, AppError> {
    let office = resolve_office(&state, &caller, query.workspace_id).await?;
    let apps = connector_apps(&state).await?;
    let rows = sqlx::query_as::<_, ConnectionRow>(&format!(
        "SELECT {ROW_COLUMNS} FROM connections
         WHERE workspace_id = $1 AND deleted_at IS NULL
         ORDER BY created_at ASC
         LIMIT 100"
    ))
    .bind(office)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!({
        "office_id": office,
        "catalog": connectors::catalog(&apps),
        "connections": rows.iter().map(|r| view(&state, r)).collect::<Vec<_>>(),
        // Inbound webhooks need an address the internet can reach.
        "inbound_reachable": state.config.local.is_none(),
    })))
}

async fn office_is_full(state: &AppState, office: Uuid) -> Result<bool, AppError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM connections WHERE workspace_id = $1 AND deleted_at IS NULL",
    )
    .bind(office)
    .fetch_one(&state.pool)
    .await?;
    Ok(count >= MAX_CONNECTIONS_PER_OFFICE)
}

// ---------- OAuth ----------

#[derive(Deserialize)]
pub struct StartRequest {
    pub workspace_id: Option<Uuid>,
    /// Shopify: the store, `name` or `name.myshopify.com`.
    pub shop: Option<String>,
    /// Google: which of gmail, calendar, sheets. Default: all three.
    pub products: Option<Vec<String>>,
    /// Accepted for every write. Starting twice only makes a second
    /// short-lived, single-use sign-in link, which is harmless.
    pub idempotency_key: Option<String>,
}

fn redirect_uri(state: &AppState, provider: Provider) -> String {
    format!(
        "{}/api/connections/oauth/{}/callback",
        state.config.app_base_url,
        provider.as_str()
    )
}

/// POST /api/connections/oauth/{provider}/start -- a sign-in link for the
/// provider. The browser goes there; the provider sends it back to the
/// callback below.
pub async fn start_oauth(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(provider): Path<String>,
    JsonOrForm(body): JsonOrForm<StartRequest>,
) -> Result<Json<Value>, AppError> {
    let user_id = people_only(&caller)?;
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let provider = Provider::parse(&provider)
        .filter(|p| *p != Provider::Webhook)
        .ok_or(AppError::NotFound)?;
    let apps = connector_apps(&state).await?;
    let app = apps.app(provider).ok_or_else(|| {
        AppError::ServiceUnavailable(format!(
            "{} sign-in is not set up yet. Add its app in Settings, Connections.",
            provider.name()
        ))
    })?;
    let office = resolve_office(&state, &caller, body.workspace_id).await?;
    if office_is_full(&state, office).await? {
        return Err(AppError::BadRequest(format!(
            "An office can have at most {MAX_CONNECTIONS_PER_OFFICE} connections"
        )));
    }

    let products: Vec<String> = match provider {
        Provider::Google => {
            let asked = body.products.unwrap_or_else(|| {
                connectors::GOOGLE_PRODUCTS
                    .iter()
                    .map(|(id, _, _)| id.to_string())
                    .collect()
            });
            if asked.is_empty()
                || asked
                    .iter()
                    .any(|p| !connectors::GOOGLE_PRODUCTS.iter().any(|(id, _, _)| id == p))
            {
                return Err(AppError::BadRequest(
                    "products must be one or more of gmail, calendar, sheets".into(),
                ));
            }
            asked
        }
        _ => Vec::new(),
    };
    let shop = match provider {
        Provider::Shopify => Some(
            body.shop
                .as_deref()
                .and_then(connectors::normalize_shop)
                .ok_or_else(|| {
                    AppError::BadRequest("Enter your store as name.myshopify.com".into())
                })?,
        ),
        _ => None,
    };

    // Old, unused sign-ins are only clutter; they cannot be finished.
    sqlx::query("DELETE FROM oauth_states WHERE expires_at < now() - interval '1 day'")
        .execute(&state.pool)
        .await?;

    let raw_secret = secret::random_hex_token(32);
    let secret_hash = secret::hash_secret(&raw_secret).map_err(|_| AppError::Internal)?;
    let state_id: Uuid = sqlx::query_scalar(
        "INSERT INTO oauth_states (secret_hash, user_id, workspace_id, provider, shop, products, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id",
    )
    .bind(&secret_hash)
    .bind(user_id)
    .bind(office)
    .bind(provider.as_str())
    .bind(&shop)
    .bind(&products)
    .bind(Utc::now() + Duration::minutes(OAUTH_STATE_TTL_MINUTES))
    .fetch_one(&state.pool)
    .await?;

    let oauth_state = secret::encode_keyed_secret("", state_id, &raw_secret);
    let challenge = connectors::pkce_challenge(&crypto::pkce_verifier(&raw_secret));
    let redirect = redirect_uri(&state, provider);
    let url = connectors::authorize_url(
        &apps,
        provider,
        app,
        &connectors::SignIn {
            redirect_uri: &redirect,
            state: &oauth_state,
            pkce_challenge: &challenge,
            google_products: &products,
            shop: shop.as_deref(),
        },
    )
    .ok_or(AppError::Internal)?;
    Ok(Json(json!({ "authorize_url": url })))
}

/// Where a sign-in ends: back on the Connections screen in the app, or (the
/// desktop app's sign-in, in the computer's browser) a page saying to
/// return to the app, which notices the new connection when it is in front
/// again.
fn finish(in_app: bool, provider: Provider, outcome: &str) -> Response {
    if in_app {
        return back_to_connections(outcome);
    }
    let ok = outcome.starts_with("connected=");
    let title = if ok {
        format!("{} is connected", provider.name())
    } else {
        format!("{} was not connected", provider.name())
    };
    let detail = if ok {
        "You can close this tab and go back to Airobot."
    } else {
        "Close this tab, go back to Airobot and try Connect again."
    };
    axum::response::Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{title}</title>\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"></head>\
         <body style=\"font-family:system-ui,sans-serif;display:grid;place-items:center;min-height:90vh;margin:0\">\
         <main style=\"text-align:center;max-width:28rem;padding:1rem\"><h1 style=\"font-size:1.4rem\">{title}</h1>\
         <p style=\"color:#555\">{detail}</p></main></body></html>"
    ))
    .into_response()
}

fn back_to_connections(outcome: &str) -> Response {
    Redirect::to(&format!("/settings/connections?{outcome}")).into_response()
}

#[derive(sqlx::FromRow)]
struct OAuthStateRow {
    secret_hash: String,
    user_id: Uuid,
    workspace_id: Uuid,
    provider: String,
    shop: Option<String>,
}

/// GET /api/connections/oauth/{provider}/callback -- where the provider
/// sends the browser back. Always ends on the Connections screen.
pub async fn oauth_callback(
    State(state): State<AppState>,
    caller: Result<Caller, AppError>,
    Path(provider): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Result<Response, AppError> {
    // On the website the person comes back signed in. The desktop app signs
    // in in the computer's own browser (Google refuses app windows), which
    // has no Airobot session: there the single-use state alone says who
    // started, and the tab is told to go back to the app.
    let session_user = match caller {
        Ok(caller) => Some(people_only(&caller)?),
        Err(_) if state.config.local.is_some() => None,
        Err(error) => return Err(error),
    };
    let in_app = session_user.is_some();
    let Some(provider) = Provider::parse(&provider).filter(|p| *p != Provider::Webhook) else {
        return Err(AppError::NotFound);
    };
    let fail = |code: &str| {
        finish(
            in_app,
            provider,
            &format!("error={code}&provider={}", provider.as_str()),
        )
    };
    let apps = connector_apps(&state).await?;
    let Some(app) = apps.app(provider) else {
        return Ok(fail("not_set_up"));
    };

    let pairs: Vec<(String, String)> = reqwest::Url::parse(&format!(
        "http://callback/?{}",
        raw_query.unwrap_or_default()
    ))
    .map(|u| u.query_pairs().into_owned().collect())
    .unwrap_or_default();
    let param = |name: &str| {
        pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };

    let Some((state_id, raw_secret)) =
        param("state").and_then(|s| secret::decode_keyed_secret("", s))
    else {
        return Ok(fail("expired"));
    };
    // Single use: the first callback claims the sign-in.
    let claimed = sqlx::query_as::<_, OAuthStateRow>(
        "UPDATE oauth_states SET used_at = now(), updated_at = now()
         WHERE id = $1 AND used_at IS NULL AND expires_at > now()
         RETURNING secret_hash, user_id, workspace_id, provider, shop",
    )
    .bind(state_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(pending) = claimed else {
        return Ok(fail("expired"));
    };
    if pending.provider != provider.as_str()
        || session_user.is_some_and(|u| u != pending.user_id)
        || !secret::verify_secret(&raw_secret, &pending.secret_hash)
    {
        return Ok(fail("expired"));
    }
    let user_id = pending.user_id;
    if param("error").is_some() {
        return Ok(fail("cancelled"));
    }
    let Some(code) = param("code") else {
        return Ok(fail("cancelled"));
    };
    // The person may have left the office since starting.
    let still_member: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM workspaces w
            LEFT JOIN workspace_members m ON m.workspace_id = w.id AND m.deleted_at IS NULL
            WHERE w.id = $1 AND w.deleted_at IS NULL
              AND (w.owner_user_id = $2 OR m.user_id = $2))",
    )
    .bind(pending.workspace_id)
    .bind(user_id)
    .fetch_one(&state.pool)
    .await?;
    if !still_member {
        return Ok(fail("expired"));
    }

    if provider == Provider::Shopify {
        let returned_shop = param("shop").and_then(connectors::normalize_shop);
        if !connectors::verify_shopify_hmac(&app.client_secret, &pairs)
            || returned_shop.is_none()
            || returned_shop != pending.shop
        {
            tracing::warn!("shopify callback failed verification");
            return Ok(fail("expired"));
        }
    }

    let granted = match connectors::exchange_code(
        &apps,
        provider,
        app,
        code,
        &redirect_uri(&state, provider),
        &crypto::pkce_verifier(&raw_secret),
        pending.shop.as_deref(),
    )
    .await
    {
        Ok(granted) => granted,
        Err(error) => {
            tracing::warn!(
                provider = provider.as_str(),
                ?error,
                "oauth code exchange failed"
            );
            return Ok(fail("provider"));
        }
    };

    let seal = |token: Option<&String>| -> Result<Option<String>, AppError> {
        token
            .map(|t| crypto::encrypt(t).ok_or(AppError::Internal))
            .transpose()
    };
    let access = seal(granted.access_token.as_ref())?;
    let refresh = seal(granted.refresh_token.as_ref())?;

    // Connecting an account the office already has refreshes it in place.
    let connection_id: Uuid = sqlx::query_scalar(
        "INSERT INTO connections
            (workspace_id, created_by_user_id, provider, label, external_account_id, scopes,
             access_token_enc, refresh_token_enc, token_expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (workspace_id, provider, external_account_id)
            WHERE deleted_at IS NULL AND external_account_id IS NOT NULL
         DO UPDATE SET
            label = EXCLUDED.label,
            scopes = EXCLUDED.scopes,
            access_token_enc = EXCLUDED.access_token_enc,
            refresh_token_enc = COALESCE(EXCLUDED.refresh_token_enc, connections.refresh_token_enc),
            token_expires_at = EXCLUDED.token_expires_at,
            status = 'active',
            last_error = NULL,
            updated_at = now()
         RETURNING id",
    )
    .bind(pending.workspace_id)
    .bind(user_id)
    .bind(provider.as_str())
    .bind(&granted.label)
    .bind(&granted.external_account_id)
    .bind(&granted.scopes)
    .bind(access)
    .bind(refresh)
    .bind(granted.expires_at)
    .fetch_one(&state.pool)
    .await?;

    record_audit(
        &state.pool,
        Some(pending.workspace_id),
        None,
        None,
        None,
        "user",
        Some(user_id),
        "connection.connected",
        Some("connection"),
        Some(connection_id),
        json!({ "provider": provider.as_str(), "label": granted.label }),
        "ok",
    )
    .await?;
    Ok(finish(
        in_app,
        provider,
        &format!("connected={}", provider.as_str()),
    ))
}

// ---------- Webhooks ----------

#[derive(Deserialize)]
pub struct CreateWebhookRequest {
    pub workspace_id: Option<Uuid>,
    pub label: String,
    /// The room inbound events are posted into.
    pub room_id: Option<Uuid>,
    /// Where `http.send` POSTs.
    pub outbound_url: Option<String>,
    pub idempotency_key: Option<String>,
}

/// POST /api/connections/webhook -- a webhook connection: events in, into
/// a room, and/or events out, to a URL.
pub async fn create_webhook(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    JsonOrForm(body): JsonOrForm<CreateWebhookRequest>,
) -> Result<Response, AppError> {
    let user_id = people_only(&caller)?;
    let key = crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let office = resolve_office(&state, &caller, body.workspace_id).await?;

    if let Some(key) = key {
        let existing = sqlx::query_as::<_, ConnectionRow>(&format!(
            "SELECT {ROW_COLUMNS} FROM connections
             WHERE workspace_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL"
        ))
        .bind(office)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?;
        if let Some(row) = existing {
            return Ok((
                StatusCode::OK,
                Json(json!({ "connection": view(&state, &row) })),
            )
                .into_response());
        }
    }

    let label = body.label.split_whitespace().collect::<Vec<_>>().join(" ");
    if label.is_empty() || label.chars().count() > 60 {
        return Err(AppError::BadRequest("name must be 1-60 characters".into()));
    }
    let outbound_url = body
        .outbound_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(|u| connectors::check_outbound_url(u, state.config.production).map(|u| u.to_string()))
        .transpose()
        .map_err(provider_error)?;
    if body.room_id.is_none() && outbound_url.is_none() {
        return Err(AppError::BadRequest(
            "choose a room to receive events, an outbound URL, or both".into(),
        ));
    }
    if let Some(room) = body.room_id {
        // The room must be the caller's own, in this office.
        let in_office: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM threads t
                WHERE t.id = $1 AND t.user_id = $2 AND t.deleted_at IS NULL
                  AND (t.workspace_id = $3
                       OR (t.workspace_id IS NULL AND $3 = (
                           SELECT w.id FROM workspaces w
                           LEFT JOIN workspace_members m ON m.workspace_id = w.id
                           WHERE (w.owner_user_id = $2 OR m.user_id = $2) AND w.deleted_at IS NULL
                           ORDER BY (w.owner_user_id = $2) DESC, w.created_at ASC
                           LIMIT 1))))",
        )
        .bind(room)
        .bind(user_id)
        .bind(office)
        .fetch_one(&state.pool)
        .await?;
        if !in_office {
            return Err(AppError::BadRequest(
                "that room is not in this office".into(),
            ));
        }
    }
    if office_is_full(&state, office).await? {
        return Err(AppError::BadRequest(format!(
            "An office can have at most {MAX_CONNECTIONS_PER_OFFICE} connections"
        )));
    }

    let mut config = json!({ "hook_salt": secret::random_hex_token(16) });
    if let Some(room) = body.room_id {
        config["thread_id"] = json!(room);
    }
    if let Some(url) = &outbound_url {
        config["outbound_url"] = json!(url);
    }
    let row = sqlx::query_as::<_, ConnectionRow>(&format!(
        "INSERT INTO connections (workspace_id, created_by_user_id, provider, label, config, idempotency_key)
         VALUES ($1, $2, 'webhook', $3, $4, $5)
         ON CONFLICT (workspace_id, idempotency_key)
            WHERE deleted_at IS NULL AND idempotency_key IS NOT NULL
         DO UPDATE SET updated_at = connections.updated_at
         RETURNING {ROW_COLUMNS}"
    ))
    .bind(office)
    .bind(user_id)
    .bind(&label)
    .bind(&config)
    .bind(key)
    .fetch_one(&state.pool)
    .await?;

    record_audit(
        &state.pool,
        Some(office),
        None,
        None,
        None,
        "user",
        Some(user_id),
        "connection.connected",
        Some("connection"),
        Some(row.id),
        json!({ "provider": "webhook", "label": label }),
        "ok",
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "connection": view(&state, &row) })),
    )
        .into_response())
}

/// POST /api/hooks/{id}/{token} -- an outside tool sends an event; it is
/// posted into the webhook's room by the Office Bot. The token in the path
/// is the only credential.
pub async fn receive_hook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, token)): Path<(Uuid, String)>,
    body: Bytes,
) -> Result<Response, AppError> {
    let row = sqlx::query_as::<_, ConnectionRow>(&format!(
        "SELECT {ROW_COLUMNS} FROM connections
         WHERE id = $1 AND provider = 'webhook' AND deleted_at IS NULL"
    ))
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    // Unknown id and wrong token look the same from outside.
    let Some(row) = row.filter(|r| {
        r.config["hook_salt"]
            .as_str()
            .is_some_and(|salt| crypto::verify_connection_hook_token(r.id, salt, &token))
    }) else {
        return Err(AppError::NotFound);
    };
    let Some(room) = row.config["thread_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
    else {
        return Err(AppError::Conflict(
            "this webhook does not receive events".into(),
        ));
    };
    let key = crate::idempotency::key(&headers, None)?;

    let text = String::from_utf8_lossy(&body);
    let shown = match serde_json::from_slice::<Value>(&body) {
        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| text.to_string()),
        Err(_) => text.trim().to_string(),
    };
    let mut shown: String = shown.chars().take(MAX_HOOK_NOTE_CHARS).collect();
    if shown.len() < text.len() && shown.chars().count() == MAX_HOOK_NOTE_CHARS {
        shown.push_str("\n…");
    }
    let fence = if shown.contains("```") { "~~~~" } else { "```" };
    let note = format!(
        "**{}** received an event:\n\n{fence}\n{shown}\n{fence}",
        row.label
    );
    let note_key = key.map(|k| format!("hook:{}:{k}", row.id));
    let posted =
        crate::office::try_post_office_note(&state, room, &note, note_key.as_deref()).await?;

    sqlx::query("UPDATE connections SET last_used_at = now(), updated_at = now() WHERE id = $1")
        .bind(row.id)
        .execute(&state.pool)
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "received": true, "duplicate": !posted && key.is_some() })),
    )
        .into_response())
}

// ---------- Remove ----------

/// POST /api/connections/{id}/delete -- forget the connection's tokens and
/// tell the provider to drop Air's access. Takes an `Idempotency-Key`
/// header like the other deletes; repeating it after success is a 404.
pub async fn delete_connection(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let user_id = people_only(&caller)?;
    crate::idempotency::key(&headers, None)?;
    let row = load_connection(&state, &caller, id).await?;
    let provider = row.provider()?;

    sqlx::query(
        "UPDATE connections
         SET deleted_at = now(), updated_at = now(),
             access_token_enc = NULL, refresh_token_enc = NULL
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(row.id)
    .execute(&state.pool)
    .await?;
    record_audit(
        &state.pool,
        Some(row.workspace_id),
        None,
        None,
        None,
        "user",
        Some(user_id),
        "connection.removed",
        Some("connection"),
        Some(row.id),
        json!({ "provider": row.provider, "label": row.label }),
        "ok",
    )
    .await?;

    let apps = connector_apps(&state).await?;
    // A pasted key is the owner's to delete in Stripe or Shopify; only a
    // sign-in is revoked here.
    let pasted_key = row.scopes.iter().any(|s| s == "api_key");
    if let Some(app) = apps.app(provider).cloned().filter(|_| !pasted_key) {
        let apps = apps.clone();
        // Google revokes the whole grant from either token.
        let token = row
            .refresh_token_enc
            .as_deref()
            .or(row.access_token_enc.as_deref())
            .and_then(crypto::decrypt);
        let account = row.external_account_id.clone();
        tokio::spawn(async move {
            connectors::revoke(&apps, provider, &app, account.as_deref(), token.as_deref()).await;
        });
    }
    Ok(Json(json!({ "deleted": true })))
}

// ---------- Actions ----------

#[derive(Deserialize, Default)]
pub struct ActionRequest {
    #[serde(default)]
    pub input: Value,
    pub idempotency_key: Option<String>,
}

/// A current access token, refreshed first if it is about to expire.
async fn access_token(
    state: &AppState,
    apps: &ConnectorApps,
    row: &ConnectionRow,
    provider: Provider,
) -> Result<Option<String>, AppError> {
    // Stripe: a pasted key is used as is; an OAuth connection has none and
    // acts through the platform key.
    if provider == Provider::Stripe {
        return Ok(row.access_token_enc.as_deref().and_then(crypto::decrypt));
    }
    if !matches!(provider, Provider::Google | Provider::Shopify) {
        return Ok(None);
    }
    let current = row.access_token_enc.as_deref().and_then(crypto::decrypt);
    let fresh = row
        .token_expires_at
        .is_none_or(|at| at > Utc::now() + Duration::seconds(60));
    if fresh && current.is_some() {
        return Ok(current);
    }
    let (Some(app), Some(refresh_token)) = (
        apps.app(provider),
        row.refresh_token_enc.as_deref().and_then(crypto::decrypt),
    ) else {
        mark_needs_reconnect(state, row.id, "Access expired").await?;
        return Err(AppError::Conflict(format!(
            "{} access expired. Connect it again.",
            provider.name()
        )));
    };
    // TODO(air): two actions refreshing at once each get a token; both
    // work (Shopify keeps the presented refresh token valid until its
    // replacement is used), so no lock is taken.
    let refreshed = match connectors::refresh(
        apps,
        provider,
        app,
        &refresh_token,
        row.external_account_id.as_deref(),
    )
    .await
    {
        Ok(refreshed) => refreshed,
        Err(error) => {
            if error.needs_reconnect {
                mark_needs_reconnect(state, row.id, &error.public).await?;
            }
            return Err(provider_error(error));
        }
    };
    let access = crypto::encrypt(&refreshed.access_token).ok_or(AppError::Internal)?;
    let new_refresh = refreshed
        .refresh_token
        .as_deref()
        .map(|t| crypto::encrypt(t).ok_or(AppError::Internal))
        .transpose()?;
    sqlx::query(
        "UPDATE connections
         SET access_token_enc = $2,
             refresh_token_enc = COALESCE($3, refresh_token_enc),
             token_expires_at = $4,
             updated_at = now()
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(row.id)
    .bind(access)
    .bind(new_refresh)
    .bind(refreshed.expires_at)
    .execute(&state.pool)
    .await?;
    Ok(Some(refreshed.access_token))
}

async fn mark_needs_reconnect(state: &AppState, id: Uuid, reason: &str) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE connections SET status = 'needs_reconnect', last_error = $2, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(reason)
    .execute(&state.pool)
    .await?;
    Ok(())
}

/// POST /api/connections/{id}/actions/{action} -- run one action. People
/// and their bots may call it.
pub async fn run_action(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path((id, action)): Path<(Uuid, String)>,
    raw_body: Bytes,
) -> Result<Json<Value>, AppError> {
    // JSON only: inputs are nested. An empty body means no input.
    let body: ActionRequest = if raw_body.iter().all(u8::is_ascii_whitespace) {
        ActionRequest::default()
    } else {
        serde_json::from_slice(&raw_body)
            .map_err(|_| AppError::BadRequest("invalid JSON body".into()))?
    };
    let key = crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let row = load_connection(&state, &caller, id).await?;
    let provider = row.provider()?;
    let spec = connectors::action(provider, &action).ok_or(AppError::NotFound)?;

    if let Some(key) = key {
        let earlier: Option<(String, Option<Value>)> = sqlx::query_as(
            "SELECT status, result FROM connection_actions
             WHERE connection_id = $1 AND action = $2 AND idempotency_key = $3",
        )
        .bind(row.id)
        .bind(spec.id)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?;
        if let Some((status, result)) = earlier {
            if status == "ok" {
                return Ok(Json(
                    json!({ "action": spec.id, "result": result, "replayed": true }),
                ));
            }
            // A failed attempt may be retried with the same key.
            sqlx::query(
                "DELETE FROM connection_actions
                 WHERE connection_id = $1 AND action = $2 AND idempotency_key = $3 AND status = 'error'",
            )
            .bind(row.id)
            .bind(spec.id)
            .bind(key)
            .execute(&state.pool)
            .await?;
        }
    }

    if row.status == "needs_reconnect" {
        return Err(AppError::Conflict(format!(
            "{} needs to be connected again before bots can use it.",
            provider.name()
        )));
    }
    if let Some(product) = spec.product {
        if !connectors::google_products(&row.scopes).contains(&product) {
            return Err(AppError::BadRequest(format!(
                "This Google connection has no {product} permission. Connect Google again and include it."
            )));
        }
    }
    let input = if body.input.is_null() {
        json!({})
    } else {
        body.input
    };
    if !input.is_object() {
        return Err(AppError::BadRequest("input must be an object".into()));
    }

    let apps = connector_apps(&state).await?;
    let token = access_token(&state, &apps, &row, provider).await?;
    let ctx = ActionContext {
        apps: &apps,
        provider,
        access_token: token.as_deref(),
        external_account_id: row.external_account_id.as_deref(),
        config: &row.config,
        production: state.config.production,
    };
    let outcome = connectors::run_action(&ctx, spec.id, &input).await;

    let (actor_user, actor_agent) = match &caller {
        Caller::User(u) => (Some(u.id), None),
        Caller::Agent(a) => (None, Some(a.id)),
    };
    let stored_result = match (&outcome, key) {
        (Ok(result), Some(_)) => Some(result.clone()),
        _ => None,
    };
    sqlx::query(
        "INSERT INTO connection_actions
            (connection_id, workspace_id, actor_user_id, actor_agent_id, action, status, result, idempotency_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (connection_id, action, idempotency_key) WHERE idempotency_key IS NOT NULL
         DO NOTHING",
    )
    .bind(row.id)
    .bind(row.workspace_id)
    .bind(actor_user)
    .bind(actor_agent)
    .bind(spec.id)
    .bind(if outcome.is_ok() { "ok" } else { "error" })
    .bind(stored_result)
    .bind(key)
    .execute(&state.pool)
    .await?;

    match outcome {
        Ok(result) => {
            sqlx::query(
                "UPDATE connections SET last_used_at = now(), last_error = NULL, updated_at = now()
                 WHERE id = $1",
            )
            .bind(row.id)
            .execute(&state.pool)
            .await?;
            Ok(Json(json!({ "action": spec.id, "result": result })))
        }
        Err(error) => {
            if error.needs_reconnect {
                mark_needs_reconnect(&state, row.id, &error.public).await?;
            } else if !error.bad_input {
                sqlx::query(
                    "UPDATE connections SET last_error = $2, updated_at = now() WHERE id = $1",
                )
                .bind(row.id)
                .bind(&error.public)
                .execute(&state.pool)
                .await?;
            }
            Err(provider_error(error))
        }
    }
}

// ---------- Provider apps (Google sign-in and friends) ----------

/// The OAuth apps this server can use: the environment's first, then the
/// ones an admin (or the desktop app's owner) saved in Settings.
pub async fn connector_apps(state: &AppState) -> Result<ConnectorApps, AppError> {
    let mut apps = state.config.connectors.clone();
    if apps.google.is_some() && apps.stripe.is_some() && apps.shopify.is_some() {
        return Ok(apps);
    }
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT provider, client_id, client_secret_enc FROM connector_apps WHERE deleted_at IS NULL",
    )
    .fetch_all(&state.pool)
    .await?;
    for (provider, client_id, secret_enc) in rows {
        let Some(client_secret) = crypto::decrypt(&secret_enc) else {
            tracing::warn!(%provider, "saved connector app secret could not be read");
            continue;
        };
        let app = Some(OAuthApp {
            client_id,
            client_secret,
        });
        match provider.as_str() {
            "google" if apps.google.is_none() => apps.google = app,
            "stripe" if apps.stripe.is_none() => apps.stripe = app,
            "shopify" if apps.shopify.is_none() => apps.shopify = app,
            _ => {}
        }
    }
    Ok(apps)
}

/// Who may set this server's provider apps: the owner of the desktop app,
/// or an admin of the hosted site.
async fn may_manage_apps(state: &AppState, caller: &Caller) -> Result<(), AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };
    if state.config.local.is_some()
        || crate::handlers::admin::is_effective_admin(&state.pool, user).await?
    {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

fn oauth_provider(raw: &str) -> Result<Provider, AppError> {
    Provider::parse(raw)
        .filter(|p| *p != Provider::Webhook)
        .ok_or(AppError::NotFound)
}

/// GET /api/connections/apps -- each provider's sign-in app: where it comes
/// from and the redirect address to register with the provider.
pub async fn list_connector_apps(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Value>, AppError> {
    may_manage_apps(&state, &caller).await?;
    let saved: Vec<(String, String)> =
        sqlx::query_as("SELECT provider, client_id FROM connector_apps WHERE deleted_at IS NULL")
            .fetch_all(&state.pool)
            .await?;
    let apps: Vec<Value> = [Provider::Google, Provider::Stripe, Provider::Shopify]
        .into_iter()
        .map(|p| {
            let from_env = state.config.connectors.app(p).is_some();
            let saved_id = saved
                .iter()
                .find(|(provider, _)| provider == p.as_str())
                .map(|(_, id)| id.clone());
            let source = if from_env {
                Some("server")
            } else if saved_id.is_some() {
                Some("settings")
            } else {
                None
            };
            json!({
                "provider": p.as_str(),
                "name": p.name(),
                "source": source,
                "client_id": if from_env { None } else { saved_id },
                "redirect_uri": redirect_uri(&state, p),
            })
        })
        .collect();
    Ok(Json(json!({
        "apps": apps,
        // The desktop app answers on a new loopback port each launch, so
        // Google needs a "Desktop app" client (any loopback port is allowed).
        "local": state.config.local.is_some(),
    })))
}

#[derive(Deserialize)]
pub struct SaveAppRequest {
    pub client_id: String,
    pub client_secret: String,
    pub idempotency_key: Option<String>,
}

/// POST /api/connections/apps/{provider} -- saves a provider's sign-in app
/// (client id and secret; the secret only encrypted). Saving again
/// replaces it.
pub async fn save_connector_app(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(provider): Path<String>,
    JsonOrForm(body): JsonOrForm<SaveAppRequest>,
) -> Result<Json<Value>, AppError> {
    may_manage_apps(&state, &caller).await?;
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let provider = oauth_provider(&provider)?;
    let client_id = body.client_id.trim();
    let client_secret = body.client_secret.trim();
    if client_id.is_empty()
        || client_id.len() > 300
        || client_secret.is_empty()
        || client_secret.len() > 500
        || client_id
            .chars()
            .chain(client_secret.chars())
            .any(char::is_whitespace)
    {
        return Err(AppError::BadRequest(
            "paste the client ID and client secret exactly as the provider shows them".into(),
        ));
    }
    if provider == Provider::Google && !client_id.ends_with(".apps.googleusercontent.com") {
        return Err(AppError::BadRequest(
            "a Google client ID ends with .apps.googleusercontent.com".into(),
        ));
    }
    let sealed = crypto::encrypt(client_secret).ok_or(AppError::Internal)?;
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "UPDATE connector_apps SET deleted_at = now(), updated_at = now()
         WHERE provider = $1 AND deleted_at IS NULL",
    )
    .bind(provider.as_str())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO connector_apps (provider, client_id, client_secret_enc, set_by_user_id, idempotency_key)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(provider.as_str())
    .bind(client_id)
    .bind(&sealed)
    .bind(caller.user_id())
    .bind(body.idempotency_key.as_deref())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    record_audit(
        &state.pool,
        None,
        None,
        None,
        None,
        "user",
        Some(caller.user_id()),
        "connector_app.saved",
        None,
        None,
        json!({ "provider": provider.as_str() }),
        "ok",
    )
    .await?;
    Ok(Json(
        json!({ "saved": true, "provider": provider.as_str() }),
    ))
}

/// POST /api/connections/apps/{provider}/delete -- forgets a saved
/// sign-in app. Existing connections keep working until their tokens
/// need refreshing.
pub async fn delete_connector_app(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(provider): Path<String>,
) -> Result<Json<Value>, AppError> {
    may_manage_apps(&state, &caller).await?;
    crate::idempotency::key(&headers, None)?;
    let provider = oauth_provider(&provider)?;
    sqlx::query(
        "UPDATE connector_apps SET deleted_at = now(), updated_at = now()
         WHERE provider = $1 AND deleted_at IS NULL",
    )
    .bind(provider.as_str())
    .execute(&state.pool)
    .await?;
    Ok(Json(json!({ "deleted": true })))
}

// ---------- Connecting with a key ----------

#[derive(Deserialize)]
pub struct KeyRequest {
    pub workspace_id: Option<Uuid>,
    /// Stripe: a restricted (rk_...) or secret (sk_...) key. Shopify: a
    /// custom app's Admin API access token (shpat_...).
    pub api_key: String,
    /// Shopify: the store, `name` or `name.myshopify.com`.
    pub shop: Option<String>,
    pub idempotency_key: Option<String>,
}

/// POST /api/connections/key/{provider} -- connects Stripe or Shopify with
/// a key the owner pastes. The key is checked with the provider first and
/// stored only encrypted. Pasting a key for an account the office has
/// replaces the old key.
pub async fn connect_with_key(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(provider): Path<String>,
    JsonOrForm(body): JsonOrForm<KeyRequest>,
) -> Result<Response, AppError> {
    let user_id = people_only(&caller)?;
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let provider = Provider::parse(&provider)
        .filter(|p| matches!(p, Provider::Stripe | Provider::Shopify))
        .ok_or(AppError::NotFound)?;
    let office = resolve_office(&state, &caller, body.workspace_id).await?;
    let shop = match provider {
        Provider::Shopify => Some(
            body.shop
                .as_deref()
                .and_then(connectors::normalize_shop)
                .ok_or_else(|| {
                    AppError::BadRequest("Enter your store as name.myshopify.com".into())
                })?,
        ),
        _ => None,
    };
    if office_is_full(&state, office).await? {
        return Err(AppError::BadRequest(format!(
            "An office can have at most {MAX_CONNECTIONS_PER_OFFICE} connections"
        )));
    }
    let apps = connector_apps(&state).await?;
    let granted = connectors::verify_key(&apps, provider, body.api_key.trim(), shop.as_deref())
        .await
        .map_err(provider_error)?;
    let access = granted
        .access_token
        .as_deref()
        .map(|t| crypto::encrypt(t).ok_or(AppError::Internal))
        .transpose()?;

    let row = sqlx::query_as::<_, ConnectionRow>(&format!(
        "INSERT INTO connections
            (workspace_id, created_by_user_id, provider, label, external_account_id, scopes,
             access_token_enc, refresh_token_enc, token_expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, NULL, NULL)
         ON CONFLICT (workspace_id, provider, external_account_id)
            WHERE deleted_at IS NULL AND external_account_id IS NOT NULL
         DO UPDATE SET
            label = EXCLUDED.label,
            scopes = EXCLUDED.scopes,
            access_token_enc = EXCLUDED.access_token_enc,
            refresh_token_enc = NULL,
            token_expires_at = NULL,
            status = 'active',
            last_error = NULL,
            updated_at = now()
         RETURNING {ROW_COLUMNS}"
    ))
    .bind(office)
    .bind(user_id)
    .bind(provider.as_str())
    .bind(&granted.label)
    .bind(&granted.external_account_id)
    .bind(&granted.scopes)
    .bind(access)
    .fetch_one(&state.pool)
    .await?;
    record_audit(
        &state.pool,
        Some(office),
        None,
        None,
        None,
        "user",
        Some(user_id),
        "connection.connected",
        Some("connection"),
        Some(row.id),
        json!({ "provider": provider.as_str(), "label": granted.label, "with": "key" }),
        "ok",
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "connection": view(&state, &row) })),
    )
        .into_response())
}
