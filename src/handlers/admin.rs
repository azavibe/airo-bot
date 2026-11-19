//! Platform admin: the panel that manages the managed product.
//!
//! Gated by `users.is_admin` (migration 0036 bootstraps the first account).
//! It owns the onboarding-tour mode ("every" / "once" / "never"), the member
//! and bot lists, and the small operational counts an operator needs.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{render, wants_html, JsonOrForm};
use crate::auth::Caller;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;

#[derive(Template)]
#[template(path = "admin.html")]
pub struct AdminTemplate {
    pub active_tab: &'static str,
    pub onboarding_mode: String,
    pub users_count: i64,
    pub agents_count: i64,
    pub tasks_count: i64,
    pub decisions_count: i64,
    pub workspaces_count: i64,
    pub new_users_7d: i64,
    pub active_agents_24h: i64,
    pub users: Vec<AdminUserView>,
    pub agents: Vec<AdminAgentView>,
}

#[derive(Serialize)]
pub struct AdminUserView {
    pub id: Uuid,
    pub label: String,
    pub email: String,
    pub handle: Option<String>,
    pub is_admin: bool,
    /// The caller's own row: the panel hides "Revoke admin" here so an
    /// admin cannot lock themselves out.
    pub is_self: bool,
    pub bot_count: i64,
    pub created_at: DateTime<Utc>,
    pub joined_ago: String,
}

#[derive(Serialize)]
pub struct AdminAgentView {
    pub id: Uuid,
    pub name: String,
    pub owner_email: String,
    pub revoked: bool,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub last_seen_ago: Option<String>,
    pub created_at: DateTime<Utc>,
}

// TODO(air): the panel lists the newest 200 people and bots. Past that,
// add server-side search and paging instead of raising the limit.
const ADMIN_LIST_LIMIT: i64 = 200;

async fn require_admin(pool: &sqlx::PgPool, user_id: Uuid) -> Result<User, AppError> {
    let user =
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1 AND deleted_at IS NULL")
            .bind(user_id)
            .fetch_optional(pool)
            .await?
            .ok_or(AppError::Forbidden)?;
    if is_effective_admin(pool, &user).await? {
        Ok(user)
    } else {
        Err(AppError::Forbidden)
    }
}

/// Admin, including the bootstrap rule: while no admin exists, the earliest
/// account owns the platform and is promoted on first use. This is what
/// makes /admin reachable on installs that predate migration 0036.
pub(crate) async fn is_effective_admin(pool: &sqlx::PgPool, user: &User) -> Result<bool, AppError> {
    if user.is_admin {
        return Ok(true);
    }
    let admin_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_admin AND deleted_at IS NULL")
            .fetch_one(pool)
            .await?;
    if admin_count > 0 {
        return Ok(false);
    }
    let earliest: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM users WHERE deleted_at IS NULL ORDER BY created_at ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?
    .flatten();
    if earliest == Some(user.id) {
        sqlx::query("UPDATE users SET is_admin = TRUE, updated_at = now() WHERE id = $1")
            .bind(user.id)
            .execute(pool)
            .await?;
        return Ok(true);
    }
    Ok(false)
}

fn time_ago_short(from: DateTime<Utc>) -> String {
    crate::handlers::airo::time_ago(from)
}

/// GET /admin — the platform panel.
pub async fn admin_home(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let me = require_admin(&state.pool, caller.user_id()).await?;

    let onboarding_mode = onboarding_mode(&state.pool).await;

    let (
        users_count,
        agents_count,
        tasks_count,
        decisions_count,
        workspaces_count,
        new_users_7d,
        active_agents_24h,
    ) = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64, i64)>(
        "SELECT
            (SELECT COUNT(*) FROM users WHERE deleted_at IS NULL),
            (SELECT COUNT(*) FROM agents WHERE deleted_at IS NULL),
            (SELECT COUNT(*) FROM tasks WHERE deleted_at IS NULL),
            (SELECT COUNT(*) FROM approvals WHERE status = 'pending' AND deleted_at IS NULL),
            (SELECT COUNT(*) FROM workspaces WHERE deleted_at IS NULL),
            (SELECT COUNT(*) FROM users
              WHERE deleted_at IS NULL AND created_at > now() - interval '7 days'),
            (SELECT COUNT(*) FROM agents
              WHERE deleted_at IS NULL AND revoked_at IS NULL
                AND last_seen_at > now() - interval '24 hours')",
    )
    .fetch_one(&state.pool)
    .await?;

    let user_rows = sqlx::query_as::<_, (Uuid, String, Option<String>, bool, DateTime<Utc>, i64)>(
        "SELECT u.id, u.email, u.handle, u.is_admin, u.created_at,
                (SELECT COUNT(*) FROM agents a
                  WHERE a.user_id = u.id AND a.deleted_at IS NULL AND a.revoked_at IS NULL)
             FROM users u
             WHERE u.deleted_at IS NULL ORDER BY u.created_at ASC LIMIT $1",
    )
    .bind(ADMIN_LIST_LIMIT)
    .fetch_all(&state.pool)
    .await?;
    let users: Vec<AdminUserView> = user_rows
        .into_iter()
        .map(
            |(id, email, handle, is_admin, created_at, bot_count)| AdminUserView {
                id,
                label: handle.clone().unwrap_or_else(|| email.clone()),
                email,
                handle,
                is_admin,
                is_self: id == me.id,
                bot_count,
                created_at,
                joined_ago: time_ago_short(created_at),
            },
        )
        .collect();

    let agent_rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
            DateTime<Utc>,
        ),
    >(
        "SELECT a.id, a.name, u.email, a.revoked_at, a.last_seen_at, a.created_at FROM agents a
         JOIN users u ON u.id = a.user_id
         WHERE a.deleted_at IS NULL ORDER BY a.created_at DESC LIMIT $1",
    )
    .bind(ADMIN_LIST_LIMIT)
    .fetch_all(&state.pool)
    .await?;
    let agents: Vec<AdminAgentView> = agent_rows
        .into_iter()
        .map(
            |(id, name, owner_email, revoked_at, last_seen_at, created_at)| AdminAgentView {
                id,
                name,
                owner_email,
                revoked: revoked_at.is_some(),
                last_seen_ago: last_seen_at.map(time_ago_short),
                last_seen_at,
                created_at,
            },
        )
        .collect();

    if wants_html(&headers) {
        let html = render(AdminTemplate {
            active_tab: "admin",
            onboarding_mode,
            users_count,
            agents_count,
            tasks_count,
            decisions_count,
            workspaces_count,
            new_users_7d,
            active_agents_24h,
            users,
            agents,
        })?;
        Ok(html.into_response())
    } else {
        Ok(Json(json!({
            "onboarding_mode": onboarding_mode,
            "me": me.id,
            "counts": {
                "users": users_count,
                "agents": agents_count,
                "tasks": tasks_count,
                "decisions": decisions_count,
                "workspaces": workspaces_count,
                "new_users_7d": new_users_7d,
                "active_agents_24h": active_agents_24h
            },
            "users": users,
            "agents": agents
        }))
        .into_response())
    }
}

async fn onboarding_mode(pool: &sqlx::PgPool) -> String {
    sqlx::query_scalar::<_, String>("SELECT value FROM app_settings WHERE key = 'onboarding_mode'")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "every".to_string())
}

#[derive(Deserialize)]
pub struct AdminSettingsPayload {
    pub onboarding_mode: String,
    pub idempotency_key: Option<String>,
}

/// POST /admin/settings — currently the onboarding-tour mode.
pub async fn admin_settings(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    JsonOrForm(body): JsonOrForm<AdminSettingsPayload>,
) -> Result<Response, AppError> {
    require_admin(&state.pool, caller.user_id()).await?;
    let mode = match body.onboarding_mode.as_str() {
        m @ ("every" | "once" | "never") => m.to_string(),
        _ => {
            return Err(AppError::BadRequest(
                "onboarding_mode must be every, once, or never".to_string(),
            ))
        }
    };
    sqlx::query(
        "INSERT INTO app_settings (id, key, value, created_at, updated_at)
         VALUES (gen_random_uuid(), 'onboarding_mode', $1, now(), now())
         ON CONFLICT (key) DO UPDATE SET value = $1, updated_at = now()",
    )
    .bind(&mode)
    .execute(&state.pool)
    .await?;
    if wants_html(&headers) {
        Ok(Redirect::to("/admin").into_response())
    } else {
        Ok(Json(json!({ "status": "saved", "onboarding_mode": mode })).into_response())
    }
}

#[derive(Deserialize)]
pub struct AdminTogglePayload {
    pub is_admin: Option<bool>,
    pub idempotency_key: Option<String>,
}

/// POST /admin/users/:id/admin — grant or revoke platform admin. The form
/// sends the explicit target state so a retried submit cannot flip twice.
pub async fn toggle_admin(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<AdminTogglePayload>,
) -> Result<Response, AppError> {
    let me = require_admin(&state.pool, caller.user_id()).await?;
    let target =
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1 AND deleted_at IS NULL")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| AppError::BadRequest("Unknown user".to_string()))?;
    let next = match body.is_admin {
        Some(v) => v,
        None => !target.is_admin,
    };
    // The caller is always an admin, so refusing self-demotion is enough to
    // guarantee the platform never ends up with nobody who can manage it.
    if target.id == me.id && !next {
        return Err(AppError::BadRequest(
            "You cannot remove your own admin access. Ask another admin.".to_string(),
        ));
    }
    sqlx::query("UPDATE users SET is_admin = $1, updated_at = now() WHERE id = $2")
        .bind(next)
        .bind(target.id)
        .execute(&state.pool)
        .await?;
    if wants_html(&headers) {
        Ok(Redirect::to("/admin").into_response())
    } else {
        Ok(
            Json(json!({ "status": "updated", "user_id": target.id, "is_admin": next }))
                .into_response(),
        )
    }
}

/// POST /admin/agents/:id/revoke — kill a bot's key immediately.
pub async fn revoke_agent(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(agent_id): Path<Uuid>,
) -> Result<Response, AppError> {
    require_admin(&state.pool, caller.user_id()).await?;
    let revoked = sqlx::query(
        "UPDATE agents SET revoked_at = COALESCE(revoked_at, now()), updated_at = now()
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .execute(&state.pool)
    .await?
    .rows_affected();
    if revoked == 0 {
        return Err(AppError::BadRequest("Unknown bot".to_string()));
    }
    if wants_html(&headers) {
        Ok(Redirect::to("/admin").into_response())
    } else {
        Ok(Json(json!({ "status": "revoked", "agent_id": agent_id })).into_response())
    }
}

/// GET /onboarding/state — drives the guided tour from the server so the
/// admin panel, not the browser, decides whether it appears.
pub async fn onboarding_state(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Response, AppError> {
    let user =
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1 AND deleted_at IS NULL")
            .bind(caller.user_id())
            .fetch_optional(&state.pool)
            .await?
            .ok_or(AppError::Forbidden)?;
    let mode = onboarding_mode(&state.pool).await;
    let is_admin = is_effective_admin(&state.pool, &user).await?;
    Ok(Json(json!({
        "mode": mode,
        "done": user.onboarding_done_at.is_some(),
        "is_admin": is_admin
    }))
    .into_response())
}

/// POST /onboarding/done — the tour was dismissed for good (matters in
/// onboarding_mode = "once").
pub async fn onboarding_done(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Response, AppError> {
    sqlx::query(
        "UPDATE users SET onboarding_done_at = COALESCE(onboarding_done_at, now()), updated_at = now()
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(caller.user_id())
    .execute(&state.pool)
    .await?;
    Ok(Json(json!({ "status": "done" })).into_response())
}
