use askama::Template;
use axum::extract::{Path, State};
use axum::http::{header::CONTENT_TYPE, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

use crate::auth::{Caller, OptionalCaller};
use crate::error::AppError;
use crate::markdown::render_markdown;
use crate::models::{Comment, Procedure, ProcedureVersion, Run, RunOutcome, Thread, Visibility};
use crate::state::AppState;

use super::{message_response, render, wants_html, wants_plain_text, JsonOrForm};

const MAX_TITLE_CHARS: usize = 200;
const MAX_SUMMARY_CHARS: usize = 1_000;
const MAX_BODY_CHARS: usize = 100_000;
const MAX_CHECK_CHARS: usize = 10_000;
const MAX_CHANGELOG_CHARS: usize = 2_000;
const MAX_MODEL_CHARS: usize = 200;
const MAX_IDEMPOTENCY_KEY_CHARS: usize = 200;

async fn find_procedure(pool: &PgPool, id: Uuid) -> Result<Option<Procedure>, AppError> {
    Ok(sqlx::query_as::<_, Procedure>(
        "SELECT * FROM procedures WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

fn owns_procedure(caller: &Caller, procedure: &Procedure) -> bool {
    caller.user_id() == procedure.user_id
}

fn can_view(caller: Option<&Caller>, procedure: &Procedure) -> bool {
    procedure.visibility != Visibility::Private
        || caller.is_some_and(|caller| owns_procedure(caller, procedure))
}

pub(crate) fn ensure_text(value: &str, label: &str, max_chars: usize) -> Result<(), AppError> {
    let count = value.chars().count();
    if count == 0 || count > max_chars {
        return Err(AppError::BadRequest(format!(
            "{label} must be 1-{max_chars} characters"
        )));
    }
    Ok(())
}

pub(crate) fn idempotency_key(value: Option<&str>) -> Result<Option<&str>, AppError> {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    if value.is_some_and(|value| value.chars().count() > MAX_IDEMPOTENCY_KEY_CHARS) {
        return Err(AppError::BadRequest(format!(
            "idempotency_key must be at most {MAX_IDEMPOTENCY_KEY_CHARS} characters"
        )));
    }
    Ok(value)
}

pub(crate) fn slugify(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut last_was_hyphen = true;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_hyphen = false;
        } else if !last_was_hyphen {
            slug.push('-');
            last_was_hyphen = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug.push_str("procedure");
    }
    slug.truncate(80);
    slug
}

fn write_response<T: Serialize>(
    headers: &HeaderMap,
    status: StatusCode,
    title: &str,
    message: &str,
    value: T,
) -> Result<Response, AppError> {
    if wants_html(headers) {
        return Ok((status, message_response(title, message)?).into_response());
    }
    Ok((status, Json(value)).into_response())
}

fn created_procedure_response(
    headers: &HeaderMap,
    status: StatusCode,
    caller: &Caller,
    procedure: Procedure,
) -> Result<Response, AppError> {
    if wants_html(headers) {
        if let Caller::User(user) = caller {
            let handle = user.handle.as_deref().ok_or(AppError::Forbidden)?;
            return Ok(Redirect::to(&format!("/u/{handle}/{}", procedure.slug)).into_response());
        }
    }
    Ok((status, Json(procedure)).into_response())
}

// ---------- POST /procedures ----------

#[derive(Deserialize)]
pub struct CreateProcedureRequest {
    pub title: String,
    pub summary: String,
    pub body: String,
    pub check_description: String,
    #[serde(default)]
    pub fork_of_id: Option<Uuid>,
    #[serde(default)]
    pub board: Option<String>,
    #[serde(default = "private_visibility")]
    pub visibility: Visibility,
    #[serde(default)]
    pub confirmed: bool,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

fn private_visibility() -> Visibility {
    Visibility::Private
}

pub async fn list_procedures(
    State(state): State<AppState>,
    OptionalCaller(caller): OptionalCaller,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let user_id = caller.as_ref().map(Caller::user_id);
    let procedures = if let Some(uid) = user_id {
        sqlx::query_as::<_, Procedure>(
            r#"
            SELECT * FROM procedures
            WHERE deleted_at IS NULL AND (visibility = 'public' OR user_id = $1)
            ORDER BY updated_at DESC
            LIMIT 50
            "#,
        )
        .bind(uid)
        .fetch_all(&state.pool)
        .await?
    } else {
        sqlx::query_as::<_, Procedure>(
            r#"
            SELECT * FROM procedures
            WHERE deleted_at IS NULL AND visibility = 'public'
            ORDER BY updated_at DESC
            LIMIT 50
            "#,
        )
        .fetch_all(&state.pool)
        .await?
    };

    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }

    Ok(Json(json!({ "procedures": procedures })).into_response())
}

pub async fn create_procedure(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    JsonOrForm(body): JsonOrForm<CreateProcedureRequest>,
) -> Result<Response, AppError> {
    let title = body.title.trim();
    let summary = body.summary.trim();
    let procedure_body = body.body.trim();
    let check_description = body.check_description.trim();
    ensure_text(title, "title", MAX_TITLE_CHARS)?;
    ensure_text(summary, "summary", MAX_SUMMARY_CHARS)?;
    ensure_text(procedure_body, "body", MAX_BODY_CHARS)?;
    ensure_text(check_description, "check_description", MAX_CHECK_CHARS)?;
    let key = idempotency_key(body.idempotency_key.as_deref())?;
    let (user_id, agent_id) = match &caller {
        Caller::User(u) => (u.id, None),
        Caller::Agent(a) => (a.user_id, Some(a.id)),
    };
    if wants_html(&headers) && body.visibility != Visibility::Private && !body.confirmed {
        return Err(AppError::BadRequest(
            "review the public preview before posting".to_string(),
        ));
    }
    let board_id = if let Some(board) = body.board.as_deref() {
        Some(
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM boards WHERE name = $1 AND deleted_at IS NULL",
            )
            .bind(board.trim().trim_start_matches('/'))
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| AppError::BadRequest("board does not exist".to_string()))?,
        )
    } else {
        None
    };

    if let Some(key) = key {
        let existing = sqlx::query_as::<_, Procedure>(
            "SELECT * FROM procedures
             WHERE user_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL",
        )
        .bind(user_id)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?;
        if let Some(procedure) = existing {
            return created_procedure_response(&headers, StatusCode::OK, &caller, procedure);
        }
    }

    if let Some(fork_id) = body.fork_of_id {
        let source = find_procedure(&state.pool, fork_id)
            .await?
            .ok_or(AppError::NotFound)?;
        if !can_view(Some(&caller), &source) {
            return Err(AppError::Forbidden);
        }
    }

    let base_slug = slugify(title);
    for attempt in 0..5 {
        let slug = if attempt == 0 {
            base_slug.clone()
        } else {
            format!("{base_slug}-{}", &Uuid::new_v4().simple().to_string()[..6])
        };
        let mut tx = state.pool.begin().await?;
        let inserted = sqlx::query_as::<_, Procedure>(
            "INSERT INTO procedures
                (user_id, agent_id, slug, title, summary, body, check_description, fork_of_id,
                 idempotency_key, board_id, visibility, published_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                     CASE WHEN $11 = 'private' THEN NULL ELSE now() END)
             RETURNING *",
        )
        .bind(user_id)
        .bind(agent_id)
        .bind(&slug)
        .bind(title)
        .bind(summary)
        .bind(procedure_body)
        .bind(check_description)
        .bind(body.fork_of_id)
        .bind(key)
        .bind(board_id)
        .bind(body.visibility)
        .fetch_one(&mut *tx)
        .await;

        let procedure = match inserted {
            Ok(procedure) => procedure,
            Err(sqlx::Error::Database(ref error)) if error.is_unique_violation() => {
                drop(tx);
                if let Some(key) = key {
                    if let Some(existing) = sqlx::query_as::<_, Procedure>(
                        "SELECT * FROM procedures
                         WHERE user_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL",
                    )
                    .bind(user_id)
                    .bind(key)
                    .fetch_optional(&state.pool)
                    .await?
                    {
                        return created_procedure_response(
                            &headers,
                            StatusCode::OK,
                            &caller,
                            existing,
                        );
                    }
                }
                continue;
            }
            Err(error) => return Err(error.into()),
        };

        sqlx::query(
            "INSERT INTO procedure_versions (procedure_id, version, body)
             VALUES ($1, 1, $2)",
        )
        .bind(procedure.id)
        .bind(procedure_body)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        return created_procedure_response(&headers, StatusCode::CREATED, &caller, procedure);
    }
    Err(AppError::Internal)
}

// ---------- GET /procedures/:handle/:slug ----------

#[derive(Template)]
#[template(path = "procedure_detail.html")]
struct ProcedureDetailTemplate {
    procedure_id: Uuid,
    slug: String,
    title: String,
    summary: String,
    is_post: bool,
    owner_handle: String,
    version: i32,
    visibility: &'static str,
    body: String,
    check_html: String,
    run_count: i64,
    success_rate: String,
    average_attempts: String,
    models: Vec<StatsEntry>,
    versions: Vec<VersionView>,
    comments: Vec<CommentView>,
    comment_count: i64,
    can_comment: bool,
    saved: bool,
    board_name: Option<String>,
    agent_name: Option<String>,
    is_owner: bool,
    agents: Vec<super::web::AgentOption>,
    active_tab: &'static str,
}

#[derive(Serialize)]
struct VersionView {
    version: i32,
    changelog: String,
    created_ago: String,
}

#[derive(sqlx::FromRow, Serialize)]
struct CommentRow {
    id: Uuid,
    user_id: Uuid,
    parent_id: Option<Uuid>,
    body: String,
    created_at: DateTime<Utc>,
    author_handle: String,
}

struct CommentView {
    id: Uuid,
    author_handle: String,
    body_html: String,
    created_ago: String,
    depth: usize,
}

fn visibility_label(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Private => "private",
        Visibility::Unlisted => "unlisted",
        Visibility::Public => "public",
    }
}

async fn fetch_comments(pool: &PgPool, procedure_id: Uuid) -> Result<Vec<CommentRow>, AppError> {
    Ok(sqlx::query_as::<_, CommentRow>(
        "SELECT c.id, c.user_id, c.parent_id, c.body, c.created_at,
                u.handle::text AS author_handle
         FROM comments c
         JOIN users u ON u.id = c.user_id AND u.deleted_at IS NULL
         WHERE c.procedure_id = $1 AND c.deleted_at IS NULL
         ORDER BY c.created_at, c.id
         LIMIT 500",
    )
    .bind(procedure_id)
    .fetch_all(pool)
    .await?)
}

fn comment_views(rows: &[CommentRow]) -> Vec<CommentView> {
    let mut depths = HashMap::new();
    rows.iter()
        .map(|row| {
            let depth = row
                .parent_id
                .and_then(|id| depths.get(&id).copied())
                .map_or(1, |parent_depth: usize| (parent_depth + 1).min(3));
            depths.insert(row.id, depth);
            CommentView {
                id: row.id,
                author_handle: row.author_handle.clone(),
                body_html: render_markdown(&row.body),
                created_ago: super::web::relative_time(row.created_at),
                depth,
            }
        })
        .collect()
}

pub async fn get_procedure(
    State(state): State<AppState>,
    OptionalCaller(caller): OptionalCaller,
    headers: HeaderMap,
    Path((handle, slug)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let procedure = sqlx::query_as::<_, Procedure>(
        "SELECT p.*
         FROM procedures p
         JOIN users u ON u.id = p.user_id
         WHERE u.handle = $1 AND u.deleted_at IS NULL
           AND p.slug = $2 AND p.deleted_at IS NULL",
    )
    .bind(&handle)
    .bind(&slug)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let owner_handle = sqlx::query_scalar::<_, String>(
        "SELECT handle::text FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(procedure.user_id)
    .fetch_one(&state.pool)
    .await?;

    if !can_view(caller.as_ref(), &procedure) {
        return Err(if caller.is_none() {
            AppError::Unauthorized
        } else {
            AppError::Forbidden
        });
    }

    if wants_plain_text(&headers) {
        return Ok((
            [(CONTENT_TYPE, "text/plain; charset=utf-8")],
            procedure.body.clone(),
        )
            .into_response());
    }
    if wants_html(&headers) {
        let models = stats_entries(fetch_model_stats(&state.pool, procedure.id).await?);
        let versions = sqlx::query_as::<_, ProcedureVersion>(
            "SELECT * FROM procedure_versions
             WHERE procedure_id = $1 AND deleted_at IS NULL
             ORDER BY version DESC LIMIT 50",
        )
        .bind(procedure.id)
        .fetch_all(&state.pool)
        .await?
        .into_iter()
        .map(|version| VersionView {
            version: version.version,
            changelog: version
                .changelog
                .unwrap_or_else(|| "Initial version".to_string()),
            created_ago: super::web::relative_time(version.created_at),
        })
        .collect::<Vec<_>>();
        let comments = comment_views(&fetch_comments(&state.pool, procedure.id).await?);
        let viewer_id = caller.as_ref().and_then(|caller| match caller {
            Caller::User(user) => Some(user.id),
            Caller::Agent(_) => None,
        });
        let saved = if let Some(viewer_id) = viewer_id {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM reactions
                 WHERE user_id = $1 AND target_type = 'procedure' AND target_id = $2
                   AND kind = 'save' AND deleted_at IS NULL)",
            )
            .bind(viewer_id)
            .bind(procedure.id)
            .fetch_one(&state.pool)
            .await?
        } else {
            false
        };
        let board_name = if let Some(board_id) = procedure.board_id {
            sqlx::query_scalar::<_, String>(
                "SELECT name::text FROM boards WHERE id = $1 AND deleted_at IS NULL",
            )
            .bind(board_id)
            .fetch_optional(&state.pool)
            .await?
        } else {
            None
        };
        let agent_name = if let Some(agent_id) = procedure.agent_id {
            sqlx::query_scalar::<_, String>(
                "SELECT name::text FROM agents WHERE id = $1 AND deleted_at IS NULL",
            )
            .bind(agent_id)
            .fetch_optional(&state.pool)
            .await?
        } else {
            None
        };
        let is_owner = caller
            .as_ref()
            .is_some_and(|c| owns_procedure(c, &procedure));
        let agents = super::web::connected_agents(&state.pool, viewer_id).await?;
        return Ok(render(ProcedureDetailTemplate {
            procedure_id: procedure.id,
            slug: procedure.slug.clone(),
            title: procedure.title.clone(),
            summary: procedure.summary.clone(),
            is_post: procedure.kind == "post",
            owner_handle,
            version: procedure.current_version,
            visibility: visibility_label(procedure.visibility),
            body: procedure.body.clone(),
            check_html: render_markdown(&procedure.check_description),
            run_count: procedure.run_count,
            success_rate: super::web::rate_label(
                procedure.success_count,
                procedure.scored_run_count,
            ),
            average_attempts: if procedure.run_count == 0 {
                "—".to_string()
            } else {
                format!(
                    "{:.1}",
                    procedure.attempt_count as f64 / procedure.run_count as f64
                )
            },
            models,
            versions,
            comments,
            comment_count: procedure.comment_count,
            can_comment: matches!(caller, Some(Caller::User(_))),
            saved,
            board_name,
            agent_name,
            is_owner,
            agents,
            active_tab: "feed",
        })?
        .into_response());
    }

    let versions = sqlx::query_as::<_, ProcedureVersion>(
        "SELECT * FROM procedure_versions
         WHERE procedure_id = $1 AND deleted_at IS NULL ORDER BY version DESC LIMIT 50",
    )
    .bind(procedure.id)
    .fetch_all(&state.pool)
    .await?;
    let comments = fetch_comments(&state.pool, procedure.id).await?;
    let by_model = stats_entries(fetch_model_stats(&state.pool, procedure.id).await?);

    Ok(Json(json!({
        "procedure": procedure,
        "owner_handle": owner_handle,
        "versions": versions,
        "by_model": by_model,
        "comments": comments,
    }))
    .into_response())
}

#[derive(Deserialize)]
pub struct SendProcedureRequest {
    agent_id: Uuid,
    #[serde(default)]
    idempotency_key: Option<String>,
}

pub async fn send_to_agent(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(procedure_id): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<SendProcedureRequest>,
) -> Result<Response, AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };
    let procedure = find_procedure(&state.pool, procedure_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !can_view(Some(&Caller::User(user.clone())), &procedure) {
        return Err(AppError::Forbidden);
    }
    let agent_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM agents
         WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL AND deleted_at IS NULL)",
    )
    .bind(body.agent_id)
    .bind(user.id)
    .fetch_one(&state.pool)
    .await?;
    if !agent_exists {
        return Err(AppError::NotFound);
    }
    let key = idempotency_key(body.idempotency_key.as_deref())?;
    let thread_key = key.map(|value| format!("procedure-send:{procedure_id}:{value}"));
    if let Some(existing) = sqlx::query_as::<_, Thread>(
        "SELECT * FROM threads WHERE user_id = $1 AND idempotency_key = $2
         AND deleted_at IS NULL",
    )
    .bind(user.id)
    .bind(thread_key.as_deref())
    .fetch_optional(&state.pool)
    .await?
    {
        if wants_html(&headers) {
            return Ok(Redirect::to(&format!("/t/{}", existing.id)).into_response());
        }
        return Ok((StatusCode::OK, Json(existing)).into_response());
    }
    let owner_handle = sqlx::query_scalar::<_, String>(
        "SELECT handle::text FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(procedure.user_id)
    .fetch_one(&state.pool)
    .await?;
    let mut tx = state.pool.begin().await?;
    let thread = sqlx::query_as::<_, Thread>(
        "INSERT INTO threads (user_id, title, slug, visibility, idempotency_key)
         VALUES ($1, $2, $3, 'private', $4) RETURNING *",
    )
    .bind(user.id)
    .bind(format!("Run {}", procedure.title))
    .bind(format!(
        "run-{}-{}",
        slugify(&procedure.title),
        &Uuid::new_v4().simple().to_string()[..6]
    ))
    .bind(thread_key.as_deref())
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO thread_agents (thread_id, agent_id) VALUES ($1, $2)")
        .bind(thread.id)
        .bind(body.agent_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO notes (thread_id, author_user_id, body, kind, idempotency_key)
         VALUES ($1, $2, $3, 'command', $4)",
    )
    .bind(thread.id)
    .bind(user.id)
    .bind(format!(
        "Run procedure: air get {owner_handle}/{}",
        procedure.slug
    ))
    .bind(key.map(|value| format!("send:{value}")))
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE threads SET note_count = 1, last_note_at = now(), updated_at = now()
         WHERE id = $1",
    )
    .bind(thread.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if wants_html(&headers) {
        return Ok(Redirect::to(&format!("/t/{}", thread.id)).into_response());
    }
    Ok((StatusCode::CREATED, Json(thread)).into_response())
}

#[derive(Deserialize, Default)]
pub struct ForkProcedureRequest {
    #[serde(default)]
    idempotency_key: Option<String>,
}

pub async fn fork_procedure(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(procedure_id): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<ForkProcedureRequest>,
) -> Result<Response, AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };
    let source = find_procedure(&state.pool, procedure_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !can_view(Some(&Caller::User(user.clone())), &source) {
        return Err(AppError::Forbidden);
    }
    let key = idempotency_key(body.idempotency_key.as_deref())?;
    if let Some(existing) = sqlx::query_as::<_, Procedure>(
        "SELECT * FROM procedures WHERE user_id = $1 AND idempotency_key = $2
         AND deleted_at IS NULL",
    )
    .bind(user.id)
    .bind(key)
    .fetch_optional(&state.pool)
    .await?
    {
        if wants_html(&headers) {
            let handle = user.handle.as_deref().ok_or(AppError::Forbidden)?;
            return Ok(Redirect::to(&format!("/u/{handle}/{}", existing.slug)).into_response());
        }
        return Ok((StatusCode::OK, Json(existing)).into_response());
    }
    let title = format!("Fork of {}", source.title);
    let slug = format!(
        "{}-{}",
        slugify(&title),
        &Uuid::new_v4().simple().to_string()[..6]
    );
    let mut tx = state.pool.begin().await?;
    let fork = sqlx::query_as::<_, Procedure>(
        "INSERT INTO procedures
            (user_id, slug, title, summary, body, check_description, visibility,
             board_id, fork_of_id, idempotency_key)
         VALUES ($1, $2, $3, $4, $5, $6, 'private', $7, $8, $9)
         RETURNING *",
    )
    .bind(user.id)
    .bind(&slug)
    .bind(title)
    .bind(&source.summary)
    .bind(&source.body)
    .bind(&source.check_description)
    .bind(source.board_id)
    .bind(source.id)
    .bind(key)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO procedure_versions (procedure_id, version, body, changelog)
         VALUES ($1, 1, $2, 'Forked copy')",
    )
    .bind(fork.id)
    .bind(&fork.body)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if wants_html(&headers) {
        let handle = user.handle.as_deref().ok_or(AppError::Forbidden)?;
        return Ok(Redirect::to(&format!("/u/{handle}/{}", fork.slug)).into_response());
    }
    Ok((StatusCode::CREATED, Json(fork)).into_response())
}

// ---------- PATCH /procedures/:id ----------

#[derive(Deserialize)]
pub struct UpdateProcedureRequest {
    pub body: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub check_description: Option<String>,
    #[serde(default)]
    pub changelog: Option<String>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Serialize)]
struct UpdateProcedureResponse {
    procedure: Procedure,
    version: ProcedureVersion,
}

pub async fn update_procedure(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(procedure_id): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<UpdateProcedureRequest>,
) -> Result<Response, AppError> {
    let procedure_body = body.body.trim();
    ensure_text(procedure_body, "body", MAX_BODY_CHARS)?;
    if let Some(title) = body.title.as_deref() {
        ensure_text(title.trim(), "title", MAX_TITLE_CHARS)?;
    }
    if let Some(summary) = body.summary.as_deref() {
        ensure_text(summary.trim(), "summary", MAX_SUMMARY_CHARS)?;
    }
    if let Some(check) = body.check_description.as_deref() {
        ensure_text(check.trim(), "check_description", MAX_CHECK_CHARS)?;
    }
    if let Some(changelog) = body.changelog.as_deref() {
        if changelog.chars().count() > MAX_CHANGELOG_CHARS {
            return Err(AppError::BadRequest(format!(
                "changelog must be at most {MAX_CHANGELOG_CHARS} characters"
            )));
        }
    }
    let key = idempotency_key(body.idempotency_key.as_deref())?;

    let mut tx = state.pool.begin().await?;
    let procedure = sqlx::query_as::<_, Procedure>(
        "SELECT * FROM procedures
         WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(procedure_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if !owns_procedure(&caller, &procedure) {
        return Err(AppError::Forbidden);
    }

    if let Some(key) = key {
        let version = sqlx::query_as::<_, ProcedureVersion>(
            "SELECT * FROM procedure_versions
             WHERE procedure_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL",
        )
        .bind(procedure.id)
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(version) = version {
            tx.commit().await?;
            return write_response(
                &headers,
                StatusCode::OK,
                "Procedure unchanged",
                "This version was already saved.",
                UpdateProcedureResponse { procedure, version },
            );
        }
    }

    let next_version = procedure.current_version + 1;
    let version = sqlx::query_as::<_, ProcedureVersion>(
        "INSERT INTO procedure_versions
            (procedure_id, version, body, changelog, idempotency_key)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING *",
    )
    .bind(procedure.id)
    .bind(next_version)
    .bind(procedure_body)
    .bind(body.changelog.as_deref().map(str::trim))
    .bind(key)
    .fetch_one(&mut *tx)
    .await?;

    let updated = sqlx::query_as::<_, Procedure>(
        "UPDATE procedures
         SET title = COALESCE($2, title),
             summary = COALESCE($3, summary),
             body = $4,
             check_description = COALESCE($5, check_description),
             current_version = $6,
             updated_at = now()
         WHERE id = $1
         RETURNING *",
    )
    .bind(procedure.id)
    .bind(body.title.as_deref().map(str::trim))
    .bind(body.summary.as_deref().map(str::trim))
    .bind(procedure_body)
    .bind(body.check_description.as_deref().map(str::trim))
    .bind(next_version)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    write_response(
        &headers,
        StatusCode::CREATED,
        "Procedure updated",
        "A new procedure version was created.",
        UpdateProcedureResponse {
            procedure: updated,
            version,
        },
    )
}

// ---------- POST /procedures/:id/publish ----------

#[derive(Deserialize)]
pub struct PublishProcedureRequest {
    pub visibility: Visibility,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

pub async fn publish_procedure(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(procedure_id): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<PublishProcedureRequest>,
) -> Result<Response, AppError> {
    if body.visibility == Visibility::Private {
        return Err(AppError::BadRequest(
            "published visibility must be unlisted or public".to_string(),
        ));
    }
    let key = idempotency_key(body.idempotency_key.as_deref())?;
    let procedure = find_procedure(&state.pool, procedure_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !owns_procedure(&caller, &procedure) {
        return Err(AppError::Forbidden);
    }

    let updated = sqlx::query_as::<_, Procedure>(
        "UPDATE procedures
         SET visibility = $2,
             published_at = COALESCE(published_at, now()),
             publish_idempotency_key = COALESCE($3, publish_idempotency_key),
             updated_at = now()
         WHERE id = $1
         RETURNING *",
    )
    .bind(procedure.id)
    .bind(body.visibility)
    .bind(key)
    .fetch_one(&state.pool)
    .await?;

    write_response(
        &headers,
        StatusCode::OK,
        "Procedure published",
        "The procedure body is now available at its published URL.",
        updated,
    )
}

// ---------- POST /runs ----------

#[derive(Deserialize)]
pub struct CreateRunRequest {
    pub procedure_id: Uuid,
    #[serde(default)]
    pub procedure_version: Option<i32>,
    pub model: String,
    pub outcome: RunOutcome,
    #[serde(default = "default_attempts")]
    pub attempts: i32,
    pub duration_ms: i64,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub thread_id: Option<Uuid>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

fn default_attempts() -> i32 {
    1
}

pub async fn create_run(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    JsonOrForm(body): JsonOrForm<CreateRunRequest>,
) -> Result<Response, AppError> {
    let Caller::Agent(agent) = caller else {
        return Err(AppError::Forbidden);
    };
    let model = body.model.trim();
    ensure_text(model, "model", MAX_MODEL_CHARS)?;
    if body.attempts <= 0 {
        return Err(AppError::BadRequest(
            "attempts must be greater than zero".to_string(),
        ));
    }
    if body.duration_ms < 0 {
        return Err(AppError::BadRequest(
            "duration_ms must not be negative".to_string(),
        ));
    }
    let key = idempotency_key(body.idempotency_key.as_deref())?;

    if let Some(key) = key {
        let existing = sqlx::query_as::<_, Run>(
            "SELECT * FROM runs
             WHERE agent_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL",
        )
        .bind(agent.id)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?;
        if let Some(run) = existing {
            return write_response(
                &headers,
                StatusCode::OK,
                "Run recorded",
                "The existing run was returned.",
                run,
            );
        }
    }

    let procedure = find_procedure(&state.pool, body.procedure_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if procedure.visibility == Visibility::Private && procedure.user_id != agent.user_id {
        return Err(AppError::Forbidden);
    }
    let version = body.procedure_version.unwrap_or(procedure.current_version);
    if version <= 0 {
        return Err(AppError::BadRequest(
            "procedure_version must be greater than zero".to_string(),
        ));
    }
    let version_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM procedure_versions
            WHERE procedure_id = $1 AND version = $2 AND deleted_at IS NULL
         )",
    )
    .bind(procedure.id)
    .bind(version)
    .fetch_one(&state.pool)
    .await?;
    if !version_exists {
        return Err(AppError::BadRequest(
            "procedure_version does not exist".to_string(),
        ));
    }

    if let Some(thread_id) = body.thread_id {
        let owns_thread: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM threads
                WHERE id = $1 AND user_id = $2 AND deleted_at IS NULL
             )",
        )
        .bind(thread_id)
        .bind(agent.user_id)
        .fetch_one(&state.pool)
        .await?;
        if !owns_thread {
            return Err(AppError::Forbidden);
        }
    }

    let finished_at = body.finished_at.unwrap_or_else(Utc::now);
    let started_at = match body.started_at {
        Some(started_at) => started_at,
        None => {
            let duration = ChronoDuration::try_milliseconds(body.duration_ms)
                .ok_or_else(|| AppError::BadRequest("duration_ms is too large".to_string()))?;
            finished_at
                .checked_sub_signed(duration)
                .ok_or_else(|| AppError::BadRequest("duration_ms is too large".to_string()))?
        }
    };
    if finished_at < started_at {
        return Err(AppError::BadRequest(
            "finished_at must not be before started_at".to_string(),
        ));
    }

    let mut tx = state.pool.begin().await?;
    let run = sqlx::query_as::<_, Run>(
        "INSERT INTO runs
            (procedure_id, procedure_version, agent_id, user_id, model, outcome,
             attempts, duration_ms, started_at, finished_at, thread_id, idempotency_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
         RETURNING *",
    )
    .bind(procedure.id)
    .bind(version)
    .bind(agent.id)
    .bind(agent.user_id)
    .bind(model)
    .bind(body.outcome)
    .bind(body.attempts)
    .bind(body.duration_ms)
    .bind(started_at)
    .bind(finished_at)
    .bind(body.thread_id)
    .bind(key)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "UPDATE procedures
         SET run_count = run_count + 1,
             success_count = success_count + CASE WHEN $2 = 'pass' THEN 1 ELSE 0 END,
             scored_run_count = scored_run_count + CASE WHEN $2 IN ('pass', 'fail') THEN 1 ELSE 0 END,
             attempt_count = attempt_count + $3,
             updated_at = now()
         WHERE id = $1",
    )
    .bind(procedure.id)
    .bind(body.outcome)
    .bind(i64::from(body.attempts))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    write_response(
        &headers,
        StatusCode::CREATED,
        "Run recorded",
        "The run and procedure counters were updated.",
        run,
    )
}

// ---------- GET /procedures/:id/stats ----------

#[derive(sqlx::FromRow)]
struct ModelStatsRow {
    model: String,
    run_count: i64,
    scored_run_count: i64,
    success_count: i64,
    attempt_count: i64,
}

#[derive(Serialize)]
struct StatsEntry {
    model: String,
    run_count: i64,
    success_count: i64,
    scored_run_count: i64,
    success_rate: Option<f64>,
    #[serde(skip)]
    rate_label: String,
    average_attempts: String,
}

#[derive(Serialize)]
struct OverallStats {
    run_count: i64,
    scored_run_count: i64,
    success_count: i64,
    success_rate: Option<f64>,
    average_attempts: Option<f64>,
}

#[derive(Serialize)]
struct StatsResponse {
    procedure_id: Uuid,
    overall: OverallStats,
    by_model: Vec<StatsEntry>,
}

#[derive(Template)]
#[template(path = "procedure_stats.html")]
struct ProcedureStatsTemplate {
    title: String,
    run_count: i64,
    success_count: i64,
    success_rate: String,
    models: Vec<StatsEntry>,
}

fn success_rate(success_count: i64, run_count: i64) -> Option<f64> {
    (run_count > 0).then(|| success_count as f64 / run_count as f64)
}

fn format_rate(rate: Option<f64>) -> String {
    rate.map_or_else(|| "—".to_string(), |rate| format!("{:.1}%", rate * 100.0))
}

async fn fetch_model_stats(
    pool: &PgPool,
    procedure_id: Uuid,
) -> Result<Vec<ModelStatsRow>, AppError> {
    Ok(sqlx::query_as::<_, ModelStatsRow>(
        "SELECT model,
                count(*)::bigint AS run_count,
                count(*) FILTER (WHERE outcome IN ('pass', 'fail'))::bigint AS scored_run_count,
                count(*) FILTER (WHERE outcome = 'pass')::bigint AS success_count,
                sum(attempts)::bigint AS attempt_count
         FROM runs
         WHERE procedure_id = $1 AND deleted_at IS NULL
         GROUP BY model
         ORDER BY run_count DESC, model ASC",
    )
    .bind(procedure_id)
    .fetch_all(pool)
    .await?)
}

fn stats_entries(rows: Vec<ModelStatsRow>) -> Vec<StatsEntry> {
    rows.into_iter()
        .map(|row| {
            let rate = success_rate(row.success_count, row.scored_run_count);
            StatsEntry {
                success_rate: rate,
                rate_label: format_rate(rate),
                average_attempts: format!("{:.1}", row.attempt_count as f64 / row.run_count as f64),
                model: row.model,
                run_count: row.run_count,
                scored_run_count: row.scored_run_count,
                success_count: row.success_count,
            }
        })
        .collect()
}

pub async fn get_stats(
    State(state): State<AppState>,
    OptionalCaller(caller): OptionalCaller,
    headers: HeaderMap,
    Path(procedure_id): Path<Uuid>,
) -> Result<Response, AppError> {
    let procedure = find_procedure(&state.pool, procedure_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !can_view(caller.as_ref(), &procedure) {
        return Err(if caller.is_none() {
            AppError::Unauthorized
        } else {
            AppError::Forbidden
        });
    }

    // The overall numbers come from the procedure counter cache. This
    // grouped query is only for the requested model breakdown, not a feed
    // count; the (procedure_id, model) index keeps it bounded to one item.
    let by_model = stats_entries(fetch_model_stats(&state.pool, procedure.id).await?);
    let overall = OverallStats {
        run_count: procedure.run_count,
        scored_run_count: procedure.scored_run_count,
        success_count: procedure.success_count,
        success_rate: success_rate(procedure.success_count, procedure.scored_run_count),
        average_attempts: (procedure.run_count > 0)
            .then(|| procedure.attempt_count as f64 / procedure.run_count as f64),
    };

    if wants_html(&headers) {
        return Ok(render(ProcedureStatsTemplate {
            title: procedure.title,
            run_count: overall.run_count,
            success_count: overall.success_count,
            success_rate: format_rate(overall.success_rate),
            models: by_model,
        })?
        .into_response());
    }

    Ok(Json(StatsResponse {
        procedure_id: procedure.id,
        overall,
        by_model,
    })
    .into_response())
}

// ---------- POST /procedures/:id/comments ----------

#[derive(Deserialize)]
pub struct CreateCommentRequest {
    pub body: String,
    #[serde(default)]
    pub parent_id: Option<Uuid>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ParentCommentRow {
    parent_id: Option<Uuid>,
    author_handle: String,
}

fn comment_response(
    headers: &HeaderMap,
    status: StatusCode,
    comment: Comment,
    owner_handle: &str,
    slug: &str,
) -> Response {
    if wants_html(headers) {
        return Redirect::to(&format!("/u/{owner_handle}/{slug}#comments")).into_response();
    }
    (status, Json(comment)).into_response()
}

pub async fn create_comment(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(procedure_id): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<CreateCommentRequest>,
) -> Result<Response, AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };
    let procedure = find_procedure(&state.pool, procedure_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let user_caller = Caller::User(user.clone());
    if !can_view(Some(&user_caller), &procedure) {
        return Err(AppError::Forbidden);
    }
    let owner_handle = sqlx::query_scalar::<_, String>(
        "SELECT handle::text FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(procedure.user_id)
    .fetch_one(&state.pool)
    .await?;
    let key = idempotency_key(body.idempotency_key.as_deref())?;
    if let Some(key) = key {
        if let Some(existing) = sqlx::query_as::<_, Comment>(
            "SELECT * FROM comments
             WHERE user_id = $1 AND procedure_id = $2
               AND idempotency_key = $3 AND deleted_at IS NULL",
        )
        .bind(user.id)
        .bind(procedure.id)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?
        {
            return Ok(comment_response(
                &headers,
                StatusCode::OK,
                existing,
                &owner_handle,
                &procedure.slug,
            ));
        }
    }

    let mut text = body.body.trim().to_string();
    ensure_text(&text, "comment body", 10_000)?;
    let mut parent_id = body.parent_id;
    if let Some(requested_parent_id) = body.parent_id {
        let parent = sqlx::query_as::<_, ParentCommentRow>(
            "SELECT c.parent_id, u.handle::text AS author_handle
             FROM comments c JOIN users u ON u.id = c.user_id
             WHERE c.id = $1 AND c.procedure_id = $2
               AND c.deleted_at IS NULL AND u.deleted_at IS NULL",
        )
        .bind(requested_parent_id)
        .bind(procedure.id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::BadRequest("parent comment does not exist".to_string()))?;
        let depth: i64 = sqlx::query_scalar(
            "WITH RECURSIVE chain AS (
                SELECT id, parent_id FROM comments WHERE id = $1 AND deleted_at IS NULL
                UNION ALL
                SELECT c.id, c.parent_id FROM comments c
                JOIN chain ON c.id = chain.parent_id
                WHERE c.deleted_at IS NULL
             )
             SELECT count(*) FROM chain",
        )
        .bind(requested_parent_id)
        .fetch_one(&state.pool)
        .await?;
        if depth >= 3 {
            parent_id = parent.parent_id;
            text = format!("Replying to u/{} — {text}", parent.author_handle);
            ensure_text(&text, "comment body", 10_000)?;
        }
    }

    let mut tx = state.pool.begin().await?;
    let comment = sqlx::query_as::<_, Comment>(
        "INSERT INTO comments (user_id, procedure_id, parent_id, body, idempotency_key)
         VALUES ($1, $2, $3, $4, $5) RETURNING *",
    )
    .bind(user.id)
    .bind(procedure.id)
    .bind(parent_id)
    .bind(&text)
    .bind(key)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE procedures
         SET comment_count = comment_count + 1, updated_at = now()
         WHERE id = $1",
    )
    .bind(procedure.id)
    .execute(&mut *tx)
    .await?;
    let notification_id = if procedure.user_id != user.id {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO notifications
                (user_id, kind, actor_user_id, target_type, target_id, idempotency_key)
             VALUES ($1, 'comment', $2, 'procedure', $3, $4)
             ON CONFLICT (user_id, idempotency_key)
               WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL
             DO NOTHING
             RETURNING id",
        )
        .bind(procedure.user_id)
        .bind(user.id)
        .bind(procedure.id)
        .bind(format!("comment:{}", comment.id))
        .fetch_optional(&mut *tx)
        .await?
    } else {
        None
    };
    tx.commit().await?;
    if let Some(notification_id) = notification_id {
        super::web::deliver_notification(&state, notification_id).await?;
    }

    Ok(comment_response(
        &headers,
        StatusCode::CREATED,
        comment,
        &owner_handle,
        &procedure.slug,
    ))
}

// ---------- POST /procedures/:id/delete ----------

pub async fn delete_procedure(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let procedure = find_procedure(&state.pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !owns_procedure(&caller, &procedure) {
        return Err(AppError::Forbidden);
    }
    sqlx::query("UPDATE procedures SET deleted_at = now(), updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;

    if wants_html(&headers) {
        Ok(axum::response::Redirect::to("/").into_response())
    } else {
        Ok(Json(json!({ "deleted": true, "id": id })).into_response())
    }
}
