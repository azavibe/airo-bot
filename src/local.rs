//! Local mode: Airobot running inside the desktop app for one person on one
//! computer, with no website, account or credits.
//!
//! The desktop app starts its own PostgreSQL, then this same server bound to
//! 127.0.0.1 on a random port, and opens it in its window. Three things
//! differ from the hosted service:
//!
//! - **Sign-in.** There is one owner: the person at this computer. The window
//!   enters through `/local/enter?token=...` with a secret made fresh on each
//!   launch; that sets an ordinary session cookie. Nothing else can sign in.
//! - **Host guard.** Every request must name the loopback address it was
//!   sent to, so a web page cannot reach this server through a DNS name that
//!   resolves to 127.0.0.1 (DNS rebinding).
//! - **AI.** Replies and office tasks use the AI tool the person chose on
//!   this computer (`AppState::local_ai`), never the platform's key, and
//!   nothing is metered.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::state::AppState;

/// The one account in local mode. `.local` is reserved for local use, so it
/// can never be someone's real address.
pub const LOCAL_OWNER_EMAIL: &str = "you@this-computer.local";
const LOCAL_OWNER_HANDLE: &str = "you";

#[derive(Clone)]
pub struct LocalMode {
    /// Made fresh on every launch; only the desktop window knows it.
    pub token: String,
}

/// One request for the person's own AI tool.
pub struct LocalAiRequest {
    /// Character, rules and context.
    pub system: String,
    /// What to answer.
    pub prompt: String,
    /// The bot's own choice of tool (`claude_code`, `codex`, ...); `None`
    /// uses the tool chosen for this computer.
    pub engine: Option<String>,
    /// The model inside that tool; `None` uses the tool's default.
    pub model: Option<String>,
    /// The tool may search the web and read pages (its web tools only;
    /// files and shell stay off). Counted by [`crate::web::take_run`].
    pub web: bool,
}

pub type LocalAiFuture = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;
/// Runs a prompt through the AI tool chosen on this computer.
pub type LocalAi = Arc<dyn Fn(LocalAiRequest) -> LocalAiFuture + Send + Sync>;

/// Constant-time comparison through fixed-length digests, so the token
/// can't be guessed byte by byte from timing.
fn same_secret(a: &str, b: &str) -> bool {
    let (x, y) = (Sha256::digest(a.as_bytes()), Sha256::digest(b.as_bytes()));
    x.iter()
        .zip(y.iter())
        .fold(0u8, |acc, (p, q)| acc | (p ^ q))
        == 0
}

/// Accepts only requests addressed to the loopback name and port this
/// server was started on (no-op outside local mode).
pub async fn host_guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if state.config.local.is_some() {
        let expected = state
            .config
            .app_base_url
            .trim_start_matches("http://")
            .to_string();
        let port = expected.rsplit(':').next().unwrap_or_default().to_string();
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default();
        let allowed = host == expected
            || host == format!("localhost:{port}")
            || host == format!("127.0.0.1:{port}");
        if !allowed {
            return (
                StatusCode::FORBIDDEN,
                "local Airobot answers only this computer",
            )
                .into_response();
        }
    }
    next.run(request).await
}

#[derive(Deserialize)]
pub struct EnterQuery {
    pub token: Option<String>,
}

/// GET /local/enter?token=... -- the desktop window's way in: signs in the
/// computer's owner (created on first launch) and opens the app.
pub async fn enter(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Query(query): Query<EnterQuery>,
) -> Result<Response, AppError> {
    let Some(local) = state.config.local.as_ref() else {
        return Err(AppError::NotFound);
    };
    let given = query.token.unwrap_or_default();
    if given.is_empty() || !same_secret(&given, &local.token) {
        return Err(AppError::Forbidden);
    }
    let mut tx = state.pool.begin().await?;
    let user_id = ensure_owner(&mut tx).await?;
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    let raw_session = crate::auth::create_session(&mut *tx, user_id, user_agent, None).await?;
    tx.commit().await?;
    // The general Assistant (and bots for office channels) on every launch,
    // so an install from before they existed gets them too.
    crate::office::ensure_starter_bots_logged(&state.pool, user_id).await;
    let jar = jar.add(crate::auth::build_session_cookie(raw_session, false));
    Ok((jar, Redirect::to("/")).into_response())
}

/// The computer's owner: created once, admin of their own office.
async fn ensure_owner(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<uuid::Uuid, AppError> {
    let existing: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE email = $1 AND deleted_at IS NULL")
            .bind(LOCAL_OWNER_EMAIL)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, handle, display_name, is_admin)
         VALUES ($1, $2, 'You', TRUE)
         RETURNING id",
    )
    .bind(LOCAL_OWNER_EMAIL)
    .bind(LOCAL_OWNER_HANDLE)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

/// Starts the background loops and serves until `shutdown` resolves. Used
/// by the hosted server (`main.rs`) and the desktop app alike.
pub async fn serve(
    state: AppState,
    listener: tokio::net::TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    tokio::spawn(crate::office::fsm_dispatcher::run_dispatcher_loop(
        state.clone(),
    ));
    tokio::spawn(crate::office::evolver::run_nightly_evolution_loop(
        state.clone(),
    ));
    tokio::spawn(crate::office::daily::run_daily_plan_loop(state.clone()));
    tokio::spawn(crate::telegram::run_loop(state.clone()));
    tokio::spawn(crate::slack::run_loop(state.clone()));
    axum::serve(
        listener,
        crate::app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_compare_exactly() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abc", "abd"));
        assert!(!same_secret("abc", ""));
    }
}
