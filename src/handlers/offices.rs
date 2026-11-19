//! Offices: a person's separate places for rooms and bots.
//!
//! An office is a `workspaces` row. Everyone has a first (primary) office,
//! made on first use; more can be added, renamed and deleted. Rooms
//! (threads) belong to one office through `threads.workspace_id`; a room
//! made before offices existed (no `workspace_id`) belongs to the primary.
//!
//! Credits, apps and workflows stay with the account's primary office, so a
//! new office never brings a second wallet of free credits.
//! TODO(air): per-office apps and dashboards; today they are account-wide,
//! which is simpler but shows the same Dashboard in every office.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::auth::Caller;
use crate::error::AppError;
use crate::handlers::airo::{get_or_create_default_workspace, record_audit};
use crate::state::AppState;

use super::{wants_html, JsonOrForm};

const MAX_OFFICES: i64 = 50;

#[derive(Serialize, sqlx::FromRow)]
pub struct OfficeItem {
    pub id: Uuid,
    pub name: String,
    pub primary: bool,
    pub room_count: i64,
    /// The Architect plans each morning and the bots start on it.
    pub daily_plan: bool,
    /// Stopped: the office's bots start no new work.
    pub paused: bool,
    pub website: Option<String>,
    pub focus: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

fn clean_name(raw: &str) -> Result<String, AppError> {
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > 60 {
        return Err(AppError::BadRequest(
            "office name must be 1-60 characters".to_string(),
        ));
    }
    Ok(name)
}

fn slug_of(name: &str) -> String {
    let mut slug = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        "office".to_string()
    } else {
        slug.chars().take(40).collect()
    }
}

/// The offices this person owns or belongs to, primary first.
pub async fn list_for(pool: &sqlx::PgPool, user_id: Uuid) -> Result<Vec<OfficeItem>, AppError> {
    let primary = get_or_create_default_workspace(pool, user_id).await?;
    let rows = sqlx::query_as::<_, OfficeItem>(
        "SELECT w.id, w.name, (w.id = $2) AS primary, w.daily_plan, w.paused, w.website, w.focus,
                (SELECT COUNT(*) FROM threads t
                  WHERE t.user_id = $1 AND t.deleted_at IS NULL AND t.kind = 'room'
                    AND (t.workspace_id = w.id OR (t.workspace_id IS NULL AND w.id = $2))
                ) AS room_count,
                w.created_at
         FROM workspaces w
         WHERE w.deleted_at IS NULL
           AND (w.owner_user_id = $1 OR EXISTS (
                SELECT 1 FROM workspace_members m
                WHERE m.workspace_id = w.id AND m.user_id = $1 AND m.deleted_at IS NULL))
         ORDER BY (w.id = $2) DESC, w.created_at ASC
         LIMIT 100",
    )
    .bind(user_id)
    .bind(primary.id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// GET /api/offices
pub async fn list_offices(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<serde_json::Value>, AppError> {
    let offices = list_for(&state.pool, caller.user_id()).await?;
    Ok(Json(json!({ "offices": offices })))
}

#[derive(Deserialize)]
pub struct CreateOfficeRequest {
    pub name: String,
    pub idempotency_key: Option<String>,
    /// The business's website and this month's focus (the team works from
    /// them), and the person's offset from UTC in minutes.
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub focus: Option<String>,
    #[serde(default)]
    pub utc_offset_minutes: Option<i32>,
    /// Post the Architect's welcome in the new (blank) office's group chat.
    /// An office set up for areas right after gets its kickoff there instead.
    #[serde(default)]
    pub intro: Option<bool>,
}

/// POST /api/offices -- a new, empty office. Rooms and bots are added to it
/// afterwards (or by office setup with its `workspace_id`).
pub async fn create_office(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    JsonOrForm(body): JsonOrForm<CreateOfficeRequest>,
) -> Result<Response, AppError> {
    let Caller::User(user) = &caller else {
        return Err(AppError::Forbidden);
    };
    let name = clean_name(&body.name)?;
    let key = crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let website = crate::office::kickoff::clean_website(body.website.as_deref().unwrap_or(""))
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let focus = body
        .focus
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(str::to_string);
    if focus.as_ref().is_some_and(|f| f.chars().count() > 200) {
        return Err(AppError::BadRequest(
            "focus must be at most 200 characters".to_string(),
        ));
    }
    let utc_offset = body
        .utc_offset_minutes
        .filter(|m| (-840..=840).contains(m))
        .unwrap_or(0);
    // The primary office exists before any other, so it stays first.
    get_or_create_default_workspace(&state.pool, user.id).await?;

    if let Some(key) = key {
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM workspaces
             WHERE owner_user_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL",
        )
        .bind(user.id)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?;
        if let Some(id) = existing {
            return Ok((
                StatusCode::OK,
                Json(json!({ "office": { "id": id, "name": name, "primary": false, "room_count": 0 } })),
            )
                .into_response());
        }
    }

    let owned: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workspaces WHERE owner_user_id = $1 AND deleted_at IS NULL",
    )
    .bind(user.id)
    .fetch_one(&state.pool)
    .await?;
    if owned >= MAX_OFFICES {
        return Err(AppError::BadRequest(format!(
            "you can have up to {MAX_OFFICES} offices"
        )));
    }

    let base = slug_of(&name);
    let mut created: Option<Uuid> = None;
    for attempt in 0..5 {
        let slug = if attempt == 0 {
            base.clone()
        } else {
            format!("{base}-{}", &Uuid::new_v4().simple().to_string()[..6])
        };
        let result = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO workspaces (name, slug, owner_user_id, idempotency_key, website, focus, utc_offset_minutes)
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
        )
        .bind(&name)
        .bind(&slug)
        .bind(user.id)
        .bind(key)
        .bind(&website)
        .bind(&focus)
        .bind(utc_offset)
        .fetch_one(&state.pool)
        .await;
        match result {
            Ok(id) => {
                created = Some(id);
                break;
            }
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let id = created.ok_or(AppError::Internal)?;
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(user.id)
    .execute(&state.pool)
    .await?;
    // Every office starts with its group chat (the Architect is in it).
    let chat = crate::office::group::ensure_office_chat(&state.pool, user.id, id).await?;
    if body.intro == Some(true) {
        crate::office::kickoff::start(
            &state,
            user.id,
            chat,
            crate::office::kickoff::Profile {
                name: name.clone(),
                business_type: None,
                website,
                focus,
            },
        )
        .await;
    }
    let _ = record_audit(
        &state.pool,
        Some(id),
        None,
        None,
        None,
        "user",
        Some(user.id),
        "workspace.created",
        Some("workspace"),
        Some(id),
        json!({ "name": name }),
        "ok",
    )
    .await;

    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({ "office": { "id": id, "name": name, "primary": false, "room_count": 0 } })),
    )
        .into_response())
}

/// The office, if `user_id` owns it (renaming and deleting are the owner's).
async fn owned_office(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    id: Uuid,
) -> Result<crate::models::Workspace, AppError> {
    let ws = sqlx::query_as::<_, crate::models::Workspace>(
        "SELECT * FROM workspaces WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;
    if ws.owner_user_id != user_id {
        return Err(AppError::Forbidden);
    }
    Ok(ws)
}

#[derive(Deserialize)]
pub struct RenameOfficeRequest {
    /// A new name (optional when only `daily_plan` changes).
    #[serde(default)]
    pub name: Option<String>,
    /// Turns the Architect's morning plan on or off for this office.
    #[serde(default)]
    pub daily_plan: Option<bool>,
    /// Stop (true) or run (false) the office's bots.
    #[serde(default)]
    pub paused: Option<bool>,
    pub idempotency_key: Option<String>,
}

/// PATCH /api/offices/:id (and POST /api/offices/:id/rename for forms)
pub async fn rename_office(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<RenameOfficeRequest>,
) -> Result<Response, AppError> {
    let Caller::User(user) = &caller else {
        return Err(AppError::Forbidden);
    };
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let ws = owned_office(&state.pool, user.id, id).await?;
    let name = match body.name.as_deref() {
        Some(raw) => clean_name(raw)?,
        None if body.daily_plan.is_some() || body.paused.is_some() => ws.name.clone(),
        None => {
            return Err(AppError::BadRequest(
                "send a name, daily_plan or paused".to_string(),
            ))
        }
    };
    let (daily_plan, paused): (bool, bool) = sqlx::query_as(
        "UPDATE workspaces SET name = $1, daily_plan = COALESCE($3, daily_plan),
                paused = COALESCE($4, paused), updated_at = now()
         WHERE id = $2 RETURNING daily_plan, paused",
    )
    .bind(&name)
    .bind(ws.id)
    .bind(body.daily_plan)
    .bind(body.paused)
    .fetch_one(&state.pool)
    .await?;
    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    Ok(
        Json(json!({ "status": "updated", "id": ws.id, "name": name, "daily_plan": daily_plan, "paused": paused }))
            .into_response(),
    )
}

/// POST /api/offices/:id/delete -- soft-deletes the office and its rooms.
/// Bots stay (they may work in other offices); the primary office stays.
/// Deleting is naturally idempotent; an `Idempotency-Key` header is
/// accepted and validated like every other write.
pub async fn delete_office(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let Caller::User(user) = &caller else {
        return Err(AppError::Forbidden);
    };
    crate::idempotency::key(&headers, None)?;
    let primary = get_or_create_default_workspace(&state.pool, user.id).await?;
    if primary.id == id {
        return Err(AppError::BadRequest(
            "your first office can't be deleted; rename it instead".to_string(),
        ));
    }
    let ws = owned_office(&state.pool, user.id, id).await?;
    let mut tx = state.pool.begin().await?;
    // Its rooms and its group chat go with it.
    let rooms: i64 = sqlx::query_scalar(
        "WITH gone AS (
             UPDATE threads SET deleted_at = now(), updated_at = now()
             WHERE workspace_id = $1 AND deleted_at IS NULL
             RETURNING kind
         )
         SELECT COUNT(*) FILTER (WHERE kind = 'room') FROM gone",
    )
    .bind(ws.id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE workspaces SET deleted_at = now(), updated_at = now() WHERE id = $1")
        .bind(ws.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let _ = record_audit(
        &state.pool,
        Some(ws.id),
        None,
        None,
        None,
        "user",
        Some(user.id),
        "workspace.deleted",
        Some("workspace"),
        Some(ws.id),
        json!({ "name": ws.name, "rooms": rooms }),
        "ok",
    )
    .await;
    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    Ok(Json(json!({ "status": "deleted", "id": ws.id, "rooms_deleted": rooms })).into_response())
}
