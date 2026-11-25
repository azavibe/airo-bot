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
    pub business_type: Option<String>,
    pub website: Option<String>,
    pub focus: Option<String>,
    pub goal: Option<String>,
    pub about: Option<String>,
    /// The language its bots write customer-facing text in (a code from
    /// `office::language`); none set when null.
    pub language: Option<String>,
    /// The office's hard monthly spending cap in credits, if any.
    pub monthly_cap_credits: Option<i64>,
    /// Credits its metered AI calls have cost this month.
    pub month_spent_credits: i64,
    /// The cap refused a call this month and hasn't been changed since:
    /// its autonomous work waits.
    pub cap_reached: bool,
    /// The person's role here: owner, member or viewer (issue #29).
    pub role: String,
    /// Whose office it is, for one shared with this person.
    pub owner_handle: Option<String>,
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
        "SELECT w.id, w.name, (w.id = $2) AS primary, w.daily_plan, w.paused,
                w.business_type, w.website, w.focus, w.goal, w.about, w.language,
                w.monthly_cap_credits,
                (SELECT COALESCE(SUM(l.credits_charged), 0)::bigint FROM credit_ledger l
                  WHERE l.office_id = w.id AND l.created_at >= date_trunc('month', now())
                ) AS month_spent_credits,
                (w.monthly_cap_credits IS NOT NULL
                  AND w.cap_notice_month IS NOT DISTINCT FROM date_trunc('month', now())::date
                ) AS cap_reached,
                (SELECT COUNT(*) FROM threads t
                  WHERE t.user_id = w.owner_user_id AND t.deleted_at IS NULL AND t.kind = 'room'
                    AND (t.workspace_id = w.id OR (t.workspace_id IS NULL AND w.id = $2))
                ) AS room_count,
                CASE WHEN w.owner_user_id = $1 THEN 'owner' ELSE COALESCE((
                    SELECT m.role FROM workspace_members m
                    WHERE m.workspace_id = w.id AND m.user_id = $1 AND m.deleted_at IS NULL
                    LIMIT 1), 'member') END AS role,
                CASE WHEN w.owner_user_id = $1 THEN NULL ELSE (
                    SELECT u.handle FROM users u WHERE u.id = w.owner_user_id) END AS owner_handle,
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
pub(crate) async fn owned_office(
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
    /// The office's profile, as set up: what the business is, its website
    /// and what matters most this month. Absent: unchanged; empty: cleared.
    #[serde(default)]
    pub business_type: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub focus: Option<String>,
    /// Where the business is going, and what it is like (hours, prices,
    /// tone, policies). Every bot in the office reads both.
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub about: Option<String>,
    /// The language its bots write to customers in, a code from
    /// `GET /api/languages`; empty: none.
    #[serde(default)]
    pub language: Option<String>,
    /// The hard monthly spending cap in credits; 0 removes it. Changing it
    /// lets work that waited on the old cap run again.
    #[serde(default)]
    pub monthly_cap_credits: Option<i64>,
    /// The Architect's check-ins: off, daily, weekly or monthly.
    #[serde(default)]
    pub check_in: Option<String>,
    /// The Monday report card in the office chat.
    #[serde(default)]
    pub weekly_report: Option<bool>,
    pub idempotency_key: Option<String>,
}

/// A profile field from a PATCH: `None` keeps it, `Some(None)` clears it.
fn profile_field(
    raw: Option<&str>,
    field: &str,
    max: usize,
) -> Result<Option<Option<String>>, AppError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() > max {
        return Err(AppError::BadRequest(format!(
            "{field} must be at most {max} characters"
        )));
    }
    Ok(Some((!value.is_empty()).then_some(value)))
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
    let business_type = profile_field(body.business_type.as_deref(), "business_type", 100)?;
    let focus = profile_field(body.focus.as_deref(), "focus", 200)?;
    let goal = profile_field(body.goal.as_deref(), "goal", 300)?;
    let about = match body.about.as_deref() {
        // Keeps its line breaks: a list of opening hours stays a list.
        Some(raw) => {
            let text = raw.trim().to_string();
            if text.chars().count() > 4000 {
                return Err(AppError::BadRequest(
                    "about must be at most 4000 characters".to_string(),
                ));
            }
            Some((!text.is_empty()).then_some(text))
        }
        None => None,
    };
    let website = match body.website.as_deref() {
        Some(raw) => Some(
            crate::office::kickoff::clean_website(raw)
                .map_err(|e| AppError::BadRequest(e.to_string()))?,
        ),
        None => None,
    };
    let cap = crate::office::spend::cap_setting(body.monthly_cap_credits)?;
    let language = match body.language.as_deref().map(str::trim) {
        Some("") => Some(None),
        Some(code) => match crate::office::language::find(code) {
            Some(lang) => Some(Some(lang.code.to_string())),
            None => {
                return Err(AppError::BadRequest(
                    "language must be one of the codes from /api/languages".to_string(),
                ))
            }
        },
        None => None,
    };
    if let Some(check_in) = body.check_in.as_deref() {
        if !matches!(check_in, "off" | "daily" | "weekly" | "monthly") {
            return Err(AppError::BadRequest(
                "check_in is off, daily, weekly or monthly".to_string(),
            ));
        }
    }
    let profile_changes = business_type.is_some()
        || focus.is_some()
        || website.is_some()
        || goal.is_some()
        || about.is_some()
        || language.is_some()
        || cap.is_some()
        || body.check_in.is_some()
        || body.weekly_report.is_some();
    let old_goal: Option<String> = sqlx::query_scalar("SELECT goal FROM workspaces WHERE id = $1")
        .bind(ws.id)
        .fetch_one(&state.pool)
        .await?;
    let name = match body.name.as_deref() {
        Some(raw) => clean_name(raw)?,
        None if body.daily_plan.is_some() || body.paused.is_some() || profile_changes => {
            ws.name.clone()
        }
        None => {
            return Err(AppError::BadRequest(
                "send a name, daily_plan, paused, a profile field or monthly_cap_credits"
                    .to_string(),
            ))
        }
    };
    type Saved = (
        bool,
        bool,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        String,
        bool,
    );
    let (
        daily_plan,
        paused,
        business_type,
        website,
        focus,
        goal,
        about,
        language,
        monthly_cap,
        check_in,
        weekly_report,
    ): Saved = sqlx::query_as(
        "UPDATE workspaces SET name = $1, daily_plan = COALESCE($3, daily_plan),
                paused = COALESCE($4, paused),
                business_type = CASE WHEN $5 THEN $6 ELSE business_type END,
                website = CASE WHEN $7 THEN $8 ELSE website END,
                focus = CASE WHEN $9 THEN $10 ELSE focus END,
                goal = CASE WHEN $11 THEN $12 ELSE goal END,
                about = CASE WHEN $13 THEN $14 ELSE about END,
                monthly_cap_credits = CASE WHEN $15 THEN $16 ELSE monthly_cap_credits END,
                cap_notice_month = CASE WHEN $15 THEN NULL ELSE cap_notice_month END,
                check_in = COALESCE($17, check_in),
                weekly_report = COALESCE($18, weekly_report),
                language = CASE WHEN $19 THEN $20 ELSE language END,
                updated_at = now()
         WHERE id = $2
         RETURNING daily_plan, paused, business_type, website, focus, goal, about, language,
                   monthly_cap_credits, check_in, weekly_report",
    )
    .bind(&name)
    .bind(ws.id)
    .bind(body.daily_plan)
    .bind(body.paused)
    .bind(business_type.is_some())
    .bind(business_type.flatten())
    .bind(website.is_some())
    .bind(website.flatten())
    .bind(focus.is_some())
    .bind(focus.flatten())
    .bind(goal.is_some())
    .bind(goal.flatten())
    .bind(about.is_some())
    .bind(about.flatten())
    .bind(cap.is_some())
    .bind(cap.flatten())
    .bind(body.check_in.as_deref())
    .bind(body.weekly_report)
    .bind(language.is_some())
    .bind(language.flatten())
    .fetch_one(&state.pool)
    .await?;
    // A new goal: the Architect proposes milestones toward it.
    if let Some(new_goal) = goal.as_deref().filter(|g| Some(*g) != old_goal.as_deref()) {
        match (
            crate::office::group::ensure_office_chat(&state.pool, user.id, ws.id).await,
            crate::office::group::architect_bot(&state.pool, user.id).await,
        ) {
            (Ok(chat), Ok(architect)) => {
                crate::office::goals::propose_for_goal(&state, chat, architect, user.id, new_goal)
            }
            (chat, architect) => {
                tracing::warn!(?chat, ?architect, office = %ws.id, "no Architect to propose milestones")
            }
        }
    }
    if cap.is_some() {
        // Work that waited on the old cap may start again.
        state.dispatcher_wake.notify_one();
    }
    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    Ok(Json(json!({
        "status": "updated",
        "id": ws.id,
        "name": name,
        "daily_plan": daily_plan,
        "paused": paused,
        "business_type": business_type,
        "website": website,
        "focus": focus,
        "goal": goal,
        "about": about,
        "language": language,
        "monthly_cap_credits": monthly_cap,
        "check_in": check_in,
        "weekly_report": weekly_report,
    }))
    .into_response())
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
