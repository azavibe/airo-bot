use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Json;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::auth::{Caller, OptionalCaller};
use crate::error::AppError;
use crate::markdown::render_markdown;
use crate::models::{Note, NoteKind, Thread, ThreadStatus, Visibility};
use crate::state::AppState;

use super::{render, wants_fragment, wants_html, JsonOrForm};

const MAX_NOTES_PER_THREAD: i64 = 500;
const INBOX_LONG_POLL: ChronoDuration = ChronoDuration::seconds(30);
/// Fallback re-check for notes written without a broadcast. New notes
/// normally wake the long poll through the broadcast channel immediately.
const INBOX_POLL_INTERVAL_MS: u64 = 2_000;
/// Re-reading the caller's thread list on unrelated notes is rate-limited
/// so one busy thread elsewhere can't fan out into a query per waiting poll.
const INBOX_THREAD_REFRESH_MS: i64 = 2_000;

// ---------- shared lookups ----------

async fn find_thread(pool: &PgPool, thread_id: Uuid) -> Result<Option<Thread>, AppError> {
    let thread =
        sqlx::query_as::<_, Thread>("SELECT * FROM threads WHERE id = $1 AND deleted_at IS NULL")
            .bind(thread_id)
            .fetch_optional(pool)
            .await?;
    Ok(thread)
}

/// True for the owning human OR one of their own agents -- general private-
/// thread access (view, and any note kind except `command`).
fn owns_thread(caller: &Caller, thread: &Thread) -> bool {
    caller.user_id() == thread.user_id
}

/// True only for the owning human, never their agents. The command rule
/// ("the author is the thread owner (a signed-in human, not an agent)")
/// needs exactly this, not `owns_thread` -- an agent's `user_id()` returns
/// its *owner's* id, so `owns_thread` alone would wrongly let an owner's
/// own agent post commands.
fn is_owning_human(caller: &Caller, thread: &Thread) -> bool {
    matches!(caller, Caller::User(u) if u.id == thread.user_id)
}

/// Whether `caller` may read `thread`: anyone for public/unlisted, only the
/// owning human or one of their own agents for private.
fn can_view(caller: Option<&Caller>, thread: &Thread) -> bool {
    if thread.visibility != Visibility::Private {
        return true;
    }
    caller.is_some_and(|c| owns_thread(c, thread))
}

async fn ensure_agent_joined(
    pool: &PgPool,
    thread_id: Uuid,
    agent_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO thread_agents (thread_id, agent_id) VALUES ($1, $2)
         ON CONFLICT (thread_id, agent_id) DO NOTHING",
    )
    .bind(thread_id)
    .bind(agent_id)
    .execute(pool)
    .await?;
    Ok(())
}

fn slugify(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut last_was_hyphen = true; // swallow a leading hyphen
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
        slug.push_str("thread");
    }
    slug.truncate(80);
    slug
}

// ---------- POST /threads ----------

#[derive(Deserialize)]
pub struct CreateThreadRequest {
    pub title: String,
    #[serde(default)]
    pub visibility: Option<Visibility>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub board_id: Option<Uuid>,
    #[serde(default)]
    pub board_name: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    /// The office (workspace) this room belongs to; checked against the
    /// caller's access. Omitted: no office (the primary one in the app).
    #[serde(default)]
    pub workspace_id: Option<Uuid>,
    /// The caller's own bots to put in the room; they answer its messages.
    #[serde(default)]
    pub agent_ids: Vec<Uuid>,
}

pub async fn create_thread_inner(
    state: &AppState,
    caller: &Caller,
    body: CreateThreadRequest,
) -> Result<(Thread, bool), AppError> {
    let title = body.title.trim();
    if title.is_empty() || title.chars().count() > 200 {
        return Err(AppError::BadRequest(
            "title must be 1-200 characters".to_string(),
        ));
    }
    let owner_id = caller.user_id();
    let visibility = body.visibility.unwrap_or(Visibility::Private);

    if let Some(ws) = body.workspace_id {
        crate::handlers::airo::ensure_workspace_access(&state.pool, caller, ws).await?;
    }
    if body.agent_ids.len() > 20 {
        return Err(AppError::BadRequest(
            "a room can start with at most 20 bots".to_string(),
        ));
    }
    if !body.agent_ids.is_empty() {
        let owned: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT id) FROM agents
             WHERE id = ANY($1) AND user_id = $2 AND deleted_at IS NULL AND revoked_at IS NULL",
        )
        .bind(&body.agent_ids)
        .bind(owner_id)
        .fetch_one(&state.pool)
        .await?;
        let mut wanted = body.agent_ids.clone();
        wanted.sort();
        wanted.dedup();
        if owned != wanted.len() as i64 {
            return Err(AppError::Forbidden);
        }
    }

    let board_id = if let Some(bid) = body.board_id {
        Some(bid)
    } else if let Some(ref bname) = body.board_name {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM boards WHERE name = $1 AND deleted_at IS NULL",
        )
        .bind(bname.trim().trim_start_matches('/'))
        .fetch_optional(&state.pool)
        .await?
    } else {
        None
    };

    if let Some(key) = body.idempotency_key.as_deref().filter(|k| !k.is_empty()) {
        let existing = sqlx::query_as::<_, Thread>(
            "SELECT * FROM threads WHERE user_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL",
        )
        .bind(owner_id)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?;
        if let Some(thread) = existing {
            return Ok((thread, false));
        }
    }

    let base_slug = slugify(title);
    let mut thread = None;
    for attempt in 0..5 {
        let slug = if attempt == 0 {
            base_slug.clone()
        } else {
            format!("{base_slug}-{}", &Uuid::new_v4().simple().to_string()[..6])
        };
        let result = sqlx::query_as::<_, Thread>(
            "INSERT INTO threads (user_id, title, slug, visibility, idempotency_key, board_id, workspace_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING *",
        )
        .bind(owner_id)
        .bind(title)
        .bind(&slug)
        .bind(visibility)
        .bind(body.idempotency_key.as_deref())
        .bind(board_id)
        .bind(body.workspace_id)
        .fetch_one(&state.pool)
        .await;

        match result {
            Ok(t) => {
                thread = Some(t);
                break;
            }
            Err(sqlx::Error::Database(ref e)) if e.is_unique_violation() => {
                if let Some(key) = body.idempotency_key.as_deref().filter(|k| !k.is_empty()) {
                    if let Some(existing) = sqlx::query_as::<_, Thread>(
                        "SELECT * FROM threads WHERE user_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL"
                    ).bind(owner_id).bind(key).fetch_optional(&state.pool).await? {
                        if let Caller::Agent(agent) = caller { ensure_agent_joined(&state.pool, existing.id, agent.id).await?; }
                        return Ok((existing, false));
                    }
                }
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
    let thread = thread.ok_or(AppError::Internal)?;

    if let Some(bid) = board_id {
        sqlx::query(
            "INSERT INTO thread_agents (thread_id, agent_id)
             SELECT $1, agent_id FROM board_agents
             WHERE board_id = $2 AND deleted_at IS NULL
             ON CONFLICT DO NOTHING",
        )
        .bind(thread.id)
        .bind(bid)
        .execute(&state.pool)
        .await?;
    }

    for agent_id in &body.agent_ids {
        ensure_agent_joined(&state.pool, thread.id, *agent_id).await?;
    }
    // Bots put in a room of an office join that office's group chat too.
    if let (Some(ws), false) = (body.workspace_id, body.agent_ids.is_empty()) {
        crate::office::group::ensure_office_chat(&state.pool, owner_id, ws).await?;
    }

    if let Caller::Agent(agent) = caller {
        ensure_agent_joined(&state.pool, thread.id, agent.id).await?;
        let notification_id = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO notifications
                (user_id, kind, actor_user_id, target_type, target_id, idempotency_key)
             VALUES ($1, 'agent_thread', NULL, 'thread', $2, $3)
             ON CONFLICT (user_id, idempotency_key)
               WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL
             DO NOTHING
             RETURNING id",
        )
        .bind(owner_id)
        .bind(thread.id)
        .bind(format!("agent-thread:{}", thread.id))
        .fetch_optional(&state.pool)
        .await?;

        if let Some(nid) = notification_id {
            super::web::deliver_notification(state, nid).await?;
        }
    }

    if let Some(ref cmd) = body.command {
        if !cmd.trim().is_empty() {
            let kind =
                if thread.visibility == Visibility::Private && is_owning_human(caller, &thread) {
                    NoteKind::Command
                } else if thread.visibility == Visibility::Private {
                    NoteKind::Status
                } else {
                    NoteKind::Comment
                };
            let _ = insert_note(state, &thread, caller, cmd.trim(), kind, None).await?;
        }
    }

    let thread = find_thread(&state.pool, thread.id).await?.unwrap_or(thread);

    // Titles should read like topics, not sentences: let the thread's
    // agent rewrite a sentence-shaped title, or trim it without a voice.
    if let Caller::User(u) = caller {
        let aid = sqlx::query_scalar::<_, Uuid>(
            "SELECT ta.agent_id FROM thread_agents ta
             JOIN agents a ON a.id = ta.agent_id
             WHERE ta.thread_id = $1 AND a.deleted_at IS NULL
             ORDER BY (a.instructions IS NULL) ASC LIMIT 1",
        )
        .bind(thread.id)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten();
        spawn_auto_title(state.clone(), thread.id, thread.title.clone(), aid, u.id);
    }
    Ok((thread, true))
}

pub async fn create_thread(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<CreateThreadRequest>,
) -> Result<Response, AppError> {
    let (thread, is_new) = create_thread_inner(&state, &caller, body).await?;
    let status = if is_new {
        axum::http::StatusCode::CREATED
    } else {
        axum::http::StatusCode::OK
    };
    Ok((status, Json(thread)).into_response())
}

// ---------- GET /threads ----------

#[derive(Deserialize)]
pub struct ListThreadsQuery {
    #[serde(default)]
    pub status: Option<ThreadStatus>,
}

pub async fn list_threads(
    State(state): State<AppState>,
    caller: Caller,
    Query(query): Query<ListThreadsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let threads = match &caller {
        Caller::User(user) => {
            sqlx::query_as::<_, Thread>(
                "SELECT * FROM threads
                 WHERE user_id = $1 AND deleted_at IS NULL
                   AND ($2::thread_status IS NULL OR status = $2)
                 ORDER BY COALESCE(last_note_at, created_at) DESC
                 LIMIT 100",
            )
            .bind(user.id)
            .bind(query.status)
            .fetch_all(&state.pool)
            .await?
        }
        Caller::Agent(agent) => {
            sqlx::query_as::<_, Thread>(
                "SELECT t.* FROM threads t
                 JOIN thread_agents ta ON ta.thread_id = t.id
                 WHERE ta.agent_id = $1 AND t.deleted_at IS NULL
                   AND ($2::thread_status IS NULL OR t.status = $2)
                 ORDER BY COALESCE(t.last_note_at, t.created_at) DESC
                 LIMIT 100",
            )
            .bind(agent.id)
            .bind(query.status)
            .fetch_all(&state.pool)
            .await?
        }
    };

    // Which bots answer in each room, so the app can name them and their AI.
    let ids: Vec<Uuid> = threads.iter().map(|t| t.id).collect();
    let pairs = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT ta.thread_id, ta.agent_id FROM thread_agents ta
         JOIN agents a ON a.id = ta.agent_id
         WHERE ta.thread_id = ANY($1) AND a.deleted_at IS NULL AND a.revoked_at IS NULL
         ORDER BY (a.instructions IS NULL) ASC, a.created_at ASC",
    )
    .bind(&ids)
    .fetch_all(&state.pool)
    .await?;
    let threads: Vec<serde_json::Value> = threads
        .into_iter()
        .map(|t| {
            let agent_ids: Vec<Uuid> = pairs
                .iter()
                .filter(|(tid, _)| *tid == t.id)
                .map(|(_, aid)| *aid)
                .collect();
            let mut value = serde_json::to_value(&t).unwrap_or_default();
            if let Some(obj) = value.as_object_mut() {
                obj.insert("agent_ids".to_string(), json!(agent_ids));
            }
            value
        })
        .collect();

    Ok(Json(json!({ "threads": threads })))
}

// ---------- POST /threads/:id/delete ----------

/// Soft-deletes a room (the owning human only). Its notes stay stored.
pub async fn delete_thread(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(thread_id): Path<Uuid>,
) -> Result<Response, AppError> {
    crate::idempotency::key(&headers, None)?;
    let thread = find_thread(&state.pool, thread_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !is_owning_human(&caller, &thread) {
        return Err(AppError::Forbidden);
    }
    if thread.kind == "office" {
        return Err(AppError::BadRequest(
            "an office's group chat stays as long as the office does".to_string(),
        ));
    }
    sqlx::query("UPDATE threads SET deleted_at = now(), updated_at = now() WHERE id = $1")
        .bind(thread.id)
        .execute(&state.pool)
        .await?;
    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    Ok(Json(json!({ "status": "deleted", "id": thread.id })).into_response())
}

// ---------- POST /notes/:id/delete ----------

/// Soft-deletes one message. Only the room's owner may, for any message in
/// it (their own or a bot's); the message is hidden, not erased.
pub async fn delete_note(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(note_id): Path<Uuid>,
) -> Result<Response, AppError> {
    crate::idempotency::key(&headers, None)?;
    let thread_id: Option<Uuid> =
        sqlx::query_scalar("SELECT thread_id FROM notes WHERE id = $1 AND deleted_at IS NULL")
            .bind(note_id)
            .fetch_optional(&state.pool)
            .await?;
    let thread_id = thread_id.ok_or(AppError::NotFound)?;
    let thread = find_thread(&state.pool, thread_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !is_owning_human(&caller, &thread) {
        // Someone else's room: as if the message did not exist.
        return Err(AppError::NotFound);
    }
    let mut tx = state.pool.begin().await?;
    let deleted =
        sqlx::query("UPDATE notes SET deleted_at = now() WHERE id = $1 AND deleted_at IS NULL")
            .bind(note_id)
            .execute(&mut *tx)
            .await?;
    if deleted.rows_affected() > 0 {
        sqlx::query(
            "UPDATE threads SET note_count = GREATEST(note_count - 1, 0), updated_at = now()
             WHERE id = $1",
        )
        .bind(thread.id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    if wants_html(&headers) {
        return Ok(Redirect::to(&format!("/t/{}", thread.id)).into_response());
    }
    Ok(Json(json!({ "status": "deleted", "id": note_id, "thread_id": thread.id })).into_response())
}

// ---------- GET /threads/:id ----------

#[derive(sqlx::FromRow)]
struct NoteRow {
    id: Uuid,
    author_user_id: Option<Uuid>,
    body: String,
    kind: NoteKind,
    created_at: DateTime<Utc>,
    author_handle: Option<String>,
    author_agent_name: Option<String>,
}

async fn fetch_note_rows(pool: &PgPool, thread_id: Uuid) -> Result<Vec<NoteRow>, AppError> {
    let rows = sqlx::query_as::<_, NoteRow>(
        "SELECT n.id, n.author_user_id, n.body, n.kind, n.created_at,
                u.handle AS author_handle, a.name AS author_agent_name
         FROM notes n
         LEFT JOIN users u ON u.id = n.author_user_id
         LEFT JOIN agents a ON a.id = n.author_agent_id
         WHERE n.thread_id = $1 AND n.deleted_at IS NULL
         ORDER BY n.seq DESC
         LIMIT $2",
    )
    .bind(thread_id)
    .bind(MAX_NOTES_PER_THREAD)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[derive(Template)]
#[template(path = "note_row.html")]
struct NoteRowTemplate<'a> {
    note_id: Uuid,
    time: String,
    author_label: &'a str,
    is_human: bool,
    kind_tag: Option<&'static str>,
    body_html: String,
}

fn note_event(thread_id: Uuid, row: &NoteRow) -> crate::state::NoteEvent {
    let kind = serde_json::to_value(row.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    crate::state::NoteEvent::new(
        thread_id,
        row.id,
        row.author_user_id,
        row.author_handle
            .as_deref()
            .or(row.author_agent_name.as_deref()),
        &row.body,
        &kind,
        row.created_at,
    )
}

fn render_note_row(row: &NoteRow, viewer_user_id: Option<Uuid>) -> Result<String, AppError> {
    let is_human = row.author_user_id.is_some();
    let author_label = if is_human && row.author_user_id == viewer_user_id {
        "you".to_string()
    } else if let Some(handle) = &row.author_handle {
        handle.clone()
    } else if let Some(name) = &row.author_agent_name {
        name.clone()
    } else {
        "unknown".to_string()
    };
    let kind_tag = matches!(row.kind, NoteKind::Command).then_some("command");
    render(NoteRowTemplate {
        note_id: row.id,
        time: row.created_at.format("%H:%M").to_string(),
        author_label: &author_label,
        is_human,
        kind_tag,
        body_html: render_markdown(&row.body),
    })
    .map(|Html(s)| s)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Composer {
    Command,
    Comment,
    None,
}

#[derive(Template)]
#[template(path = "thread_log.html")]
struct ThreadLogTemplate {
    thread_id: Uuid,
    note_rows: Vec<String>,
}

#[derive(Template)]
#[template(path = "thread_detail.html")]
struct ThreadDetailTemplate {
    thread_id: Uuid,
    title: String,
    visibility_label: &'static str,
    status_label: &'static str,
    note_rows: Vec<String>,
    composer: Composer,
    agent_names: String,
    opened_ago: String,
    active_tab: &'static str,
}

fn composer_for(caller: Option<&Caller>, thread: &Thread) -> Composer {
    match (thread.visibility, caller) {
        (Visibility::Private, Some(c)) if is_owning_human(c, thread) => Composer::Command,
        (Visibility::Private, _) => Composer::None,
        (_, Some(_)) => Composer::Comment,
        (_, None) => Composer::None,
    }
}

fn visibility_label(v: Visibility) -> &'static str {
    match v {
        Visibility::Private => "private",
        Visibility::Unlisted => "unlisted",
        Visibility::Public => "public",
    }
}

fn status_label(s: ThreadStatus) -> &'static str {
    match s {
        ThreadStatus::Open => "open",
        ThreadStatus::Waiting => "waiting",
        ThreadStatus::Done => "done",
        ThreadStatus::Failed => "failed",
    }
}

pub async fn get_thread(
    State(state): State<AppState>,
    OptionalCaller(caller): OptionalCaller,
    headers: HeaderMap,
    Path(thread_id): Path<Uuid>,
) -> Result<Response, AppError> {
    let thread = find_thread(&state.pool, thread_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if !can_view(caller.as_ref(), &thread) {
        return Err(if caller.is_none() {
            AppError::Unauthorized
        } else {
            AppError::Forbidden
        });
    }

    if wants_fragment(&headers) || wants_html(&headers) {
        let rows = fetch_note_rows(&state.pool, thread.id).await?;
        let viewer_user_id = caller.as_ref().map(|c| c.user_id());
        let note_rows = rows
            .iter()
            .map(|r| render_note_row(r, viewer_user_id))
            .collect::<Result<Vec<_>, _>>()?;
        let composer = composer_for(caller.as_ref(), &thread);

        if wants_fragment(&headers) {
            return Ok(render(ThreadLogTemplate {
                thread_id: thread.id,
                note_rows,
            })?
            .into_response());
        }
        let agent_names = sqlx::query_scalar::<_, String>(
            "SELECT COALESCE(string_agg(a.name, ', ' ORDER BY a.name), 'none')
             FROM thread_agents ta JOIN agents a ON a.id = ta.agent_id
             WHERE ta.thread_id = $1 AND a.deleted_at IS NULL",
        )
        .bind(thread.id)
        .fetch_one(&state.pool)
        .await?;
        return Ok(render(ThreadDetailTemplate {
            thread_id: thread.id,
            title: thread.title.clone(),
            visibility_label: visibility_label(thread.visibility),
            status_label: status_label(thread.status),
            note_rows,
            composer,
            agent_names,
            opened_ago: super::web::relative_time(thread.created_at),
            active_tab: "yours",
        })?
        .into_response());
    }

    // The newest notes, in reading order (oldest first), so a room past the
    // cap reloads with its latest messages rather than its first ones.
    // TODO(air): past MAX_NOTES_PER_THREAD a reloaded room has no way to page
    // back to its oldest notes. A `seq` cursor here plus a "load older"
    // control in the web app would reach them, at the cost of a second load
    // path in the chat; the spec does not ask for one yet.
    let notes = sqlx::query_as::<_, Note>(
        "SELECT * FROM (
             SELECT * FROM notes
             WHERE thread_id = $1 AND deleted_at IS NULL
             ORDER BY seq DESC
             LIMIT $2
         ) newest
         ORDER BY seq ASC",
    )
    .bind(thread.id)
    .bind(MAX_NOTES_PER_THREAD)
    .fetch_all(&state.pool)
    .await?;

    // Who wrote each note, the same way the live stream names them, so a
    // reloaded room shows the same names as a live one.
    let names = sqlx::query_as::<_, (Uuid, Option<String>)>(
        "SELECT n.id, COALESCE(u.handle, a.name)
         FROM notes n
         LEFT JOIN users u ON u.id = n.author_user_id
         LEFT JOIN agents a ON a.id = n.author_agent_id
         WHERE n.thread_id = $1 AND n.deleted_at IS NULL",
    )
    .bind(thread.id)
    .fetch_all(&state.pool)
    .await?;
    let names: std::collections::HashMap<Uuid, Option<String>> = names.into_iter().collect();
    let notes: Vec<serde_json::Value> = notes
        .into_iter()
        .map(|n| {
            let name = names.get(&n.id).cloned().flatten();
            let kind = if n.author_user_id.is_some() {
                "human"
            } else {
                "agent"
            };
            let mut value = serde_json::to_value(&n).unwrap_or_default();
            if let Some(obj) = value.as_object_mut() {
                obj.insert("author_name".to_string(), json!(name));
                obj.insert("author_kind".to_string(), json!(kind));
            }
            value
        })
        .collect();

    Ok(Json(json!({ "thread": thread, "notes": notes })).into_response())
}

// ---------- POST /threads/:id/notes ----------

#[derive(Deserialize)]
pub struct CreateNoteRequest {
    pub body: String,
    pub kind: NoteKind,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// The command rule: a `command` note requires
/// a private thread and the owning human as author. Every other kind is
/// open to the owner or their own agents on a private thread, but only
/// `comment` on a public or unlisted one -- posting a `status`/`system`
/// note to a thread the world can see isn't something any caller does.
fn authorize_note(caller: &Caller, thread: &Thread, kind: NoteKind) -> Result<(), AppError> {
    if kind == NoteKind::Command {
        if thread.visibility == Visibility::Private && is_owning_human(caller, thread) {
            return Ok(());
        }
        return Err(AppError::Forbidden);
    }

    match thread.visibility {
        Visibility::Private => {
            if owns_thread(caller, thread) {
                Ok(())
            } else {
                Err(AppError::Forbidden)
            }
        }
        Visibility::Unlisted | Visibility::Public => {
            if kind == NoteKind::Comment {
                Ok(())
            } else {
                Err(AppError::BadRequest(
                    "public and unlisted threads only accept comment notes".to_string(),
                ))
            }
        }
    }
}

async fn insert_note(
    state: &AppState,
    thread: &Thread,
    caller: &Caller,
    body: &str,
    kind: NoteKind,
    idempotency_key: Option<&str>,
) -> Result<(Note, bool), AppError> {
    let mut tx = state.pool.begin().await?;
    // Serialize writes to a thread before checking the retry key and updating counters.
    sqlx::query("SELECT id FROM threads WHERE id = $1 FOR UPDATE")
        .bind(thread.id)
        .execute(&mut *tx)
        .await?;
    if let Some(key) = idempotency_key.filter(|k| !k.is_empty()) {
        let existing = sqlx::query_as::<_, Note>(
            "SELECT * FROM notes WHERE thread_id = $1 AND idempotency_key = $2",
        )
        .bind(thread.id)
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(note) = existing {
            return Ok((note, false));
        }
    }

    let (author_agent_id, author_user_id) = match caller {
        Caller::Agent(a) => (Some(a.id), None),
        Caller::User(u) => (None, Some(u.id)),
    };

    let note = sqlx::query_as::<_, Note>(
        "INSERT INTO notes (thread_id, author_agent_id, author_user_id, body, kind, idempotency_key)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING *",
    )
    .bind(thread.id)
    .bind(author_agent_id)
    .bind(author_user_id)
    .bind(body)
    .bind(kind)
    .bind(idempotency_key)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "UPDATE threads SET note_count = note_count + 1, last_note_at = now(), updated_at = now()
         WHERE id = $1",
    )
    .bind(thread.id)
    .execute(&mut *tx)
    .await?;

    let mut notification_id = None;
    if let Caller::Agent(agent) = caller {
        sqlx::query(
            "INSERT INTO thread_agents (thread_id, agent_id) VALUES ($1, $2)
             ON CONFLICT (thread_id, agent_id) DO NOTHING",
        )
        .bind(thread.id)
        .bind(agent.id)
        .execute(&mut *tx)
        .await?;

        if thread.status == ThreadStatus::Waiting {
            notification_id = sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO notifications
                    (user_id, kind, actor_user_id, target_type, target_id, idempotency_key)
                 VALUES ($1, 'job_needs_you', NULL, 'thread', $2, $3)
                 ON CONFLICT (user_id, idempotency_key)
                   WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL
                 DO NOTHING
                 RETURNING id",
            )
            .bind(thread.user_id)
            .bind(thread.id)
            .bind(format!("job-note:{}", note.id))
            .fetch_optional(&mut *tx)
            .await?;
        } else {
            notification_id = sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO notifications
                    (user_id, kind, actor_user_id, target_type, target_id, idempotency_key)
                 VALUES ($1, 'agent_note', NULL, 'thread', $2, $3)
                 ON CONFLICT (user_id, idempotency_key)
                   WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL
                 DO NOTHING
                 RETURNING id",
            )
            .bind(thread.user_id)
            .bind(thread.id)
            .bind(format!("agent-note:{}", note.id))
            .fetch_optional(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    if let Some(notification_id) = notification_id {
        super::web::deliver_notification(state, notification_id).await?;
    }
    Ok((note, true))
}

pub async fn create_note(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(thread_id): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<CreateNoteRequest>,
) -> Result<Response, AppError> {
    let text = body.body.trim();
    if text.is_empty() || text.chars().count() > 10_000 {
        return Err(AppError::BadRequest(
            "note body must be 1-10000 characters".to_string(),
        ));
    }
    let thread = find_thread(&state.pool, thread_id)
        .await?
        .ok_or(AppError::NotFound)?;
    authorize_note(&caller, &thread, body.kind)?;

    let (note, created) = insert_note(
        &state,
        &thread,
        &caller,
        text,
        body.kind,
        body.idempotency_key.as_deref(),
    )
    .await?;

    if created {
        let broadcast_row = sqlx::query_as::<_, NoteRow>(
            "SELECT n.id, n.author_user_id, n.body, n.kind, n.created_at,
                    u.handle AS author_handle, a.name AS author_agent_name
             FROM notes n
             LEFT JOIN users u ON u.id = n.author_user_id
             LEFT JOIN agents a ON a.id = n.author_agent_id
             WHERE n.id = $1",
        )
        .bind(note.id)
        .fetch_optional(&state.pool)
        .await?;

        if let Some(row) = broadcast_row {
            let _ = state.note_sender.send(note_event(thread.id, &row));
        }

        // Free chat: any human message reaches the thread's agent, which
        // answers in its own character. Commands travel the same path.
        if let Caller::User(u) = &caller {
            match (thread.kind.as_str(), thread.workspace_id) {
                // The office group chat: the Architect (or an @mentioned
                // bot) answers, and hands work to the right bot.
                ("office", Some(workspace_id)) if is_owning_human(&caller, &thread) => {
                    crate::office::group::on_person_message(
                        &state,
                        thread.id,
                        workspace_id,
                        u.id,
                        text,
                    )
                    .await;
                }
                _ => {
                    let aid = sqlx::query_scalar::<_, Uuid>(
                        "SELECT ta.agent_id FROM thread_agents ta
                         JOIN agents a ON a.id = ta.agent_id
                         WHERE ta.thread_id = $1 AND a.deleted_at IS NULL AND a.revoked_at IS NULL
                         ORDER BY (a.instructions IS NULL) ASC LIMIT 1",
                    )
                    .bind(thread.id)
                    .fetch_optional(&state.pool)
                    .await
                    .ok()
                    .flatten();
                    if let Some(aid) = aid {
                        spawn_agent_reply(
                            state.clone(),
                            thread.id,
                            aid,
                            u.id,
                            ReplyMode::default(),
                        );
                    }
                }
            }
        }
    }

    if wants_fragment(&headers) {
        let row = sqlx::query_as::<_, NoteRow>(
            "SELECT n.id, n.author_user_id, n.body, n.kind, n.created_at,
                    u.handle AS author_handle, a.name AS author_agent_name
             FROM notes n
             LEFT JOIN users u ON u.id = n.author_user_id
             LEFT JOIN agents a ON a.id = n.author_agent_id
             WHERE n.id = $1",
        )
        .bind(note.id)
        .fetch_one(&state.pool)
        .await?;
        let html = render_note_row(&row, Some(caller.user_id()))?;
        return Ok(Html(html).into_response());
    }

    let status = if created {
        axum::http::StatusCode::CREATED
    } else {
        axum::http::StatusCode::OK
    };
    Ok((status, Json(note)).into_response())
}

// ---------- GET /threads/:id/events ----------

pub async fn thread_live_events(
    State(state): State<AppState>,
    OptionalCaller(caller): OptionalCaller,
    Path(thread_id): Path<Uuid>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>>, AppError>
{
    let thread = find_thread(&state.pool, thread_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if !can_view(caller.as_ref(), &thread) {
        return Err(if caller.is_none() {
            AppError::Unauthorized
        } else {
            AppError::Forbidden
        });
    }

    let rx = state.note_sender.subscribe();
    let stream =
        tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(move |item| match item {
            Ok(event) if event.thread_id == thread_id => Some(Ok(Event::default()
                .event("note")
                .data(event.note.to_string()))),
            _ => None,
        });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

// ---------- POST /threads/:id/archive ----------

pub async fn archive_thread(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(thread_id): Path<Uuid>,
) -> Result<Response, AppError> {
    let thread = find_thread(&state.pool, thread_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let Caller::User(_) = &caller else {
        return Err(AppError::Forbidden);
    };
    if !owns_thread(&caller, &thread) {
        return Err(AppError::Forbidden);
    }

    let updated = sqlx::query_as::<_, Thread>(
        "UPDATE threads SET archived_at = COALESCE(archived_at, now()), updated_at = now()
         WHERE id = $1 RETURNING *",
    )
    .bind(thread.id)
    .fetch_one(&state.pool)
    .await?;

    if wants_html(&headers) {
        return Ok(Redirect::to("/yours").into_response());
    }
    Ok((StatusCode::OK, Json(updated)).into_response())
}

// ---------- POST /threads/:id/unarchive ----------

pub async fn unarchive_thread(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(thread_id): Path<Uuid>,
) -> Result<Response, AppError> {
    let thread = find_thread(&state.pool, thread_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let Caller::User(_) = &caller else {
        return Err(AppError::Forbidden);
    };
    if !owns_thread(&caller, &thread) {
        return Err(AppError::Forbidden);
    }

    let updated = sqlx::query_as::<_, Thread>(
        "UPDATE threads SET archived_at = NULL, updated_at = now()
         WHERE id = $1 RETURNING *",
    )
    .bind(thread.id)
    .fetch_one(&state.pool)
    .await?;

    if wants_html(&headers) {
        return Ok(Redirect::to(&format!("/t/{}", thread.id)).into_response());
    }
    Ok((StatusCode::OK, Json(updated)).into_response())
}

// ---------- GET /inbox ----------

#[derive(Deserialize)]
pub struct InboxQuery {
    #[serde(default)]
    pub since: Option<i64>,
    #[serde(default)]
    pub view: Option<String>,
}

#[derive(Serialize)]
pub struct InboxResponse {
    notes: Vec<Note>,
    cursor: i64,
}

async fn callers_thread_ids(pool: &PgPool, caller: &Caller) -> Result<Vec<Uuid>, AppError> {
    let ids = match caller {
        Caller::User(user) => {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM threads WHERE user_id = $1 AND deleted_at IS NULL",
            )
            .bind(user.id)
            .fetch_all(pool)
            .await?
        }
        Caller::Agent(agent) => {
            sqlx::query_scalar::<_, Uuid>("SELECT thread_id FROM thread_agents WHERE agent_id = $1")
                .bind(agent.id)
                .fetch_all(pool)
                .await?
        }
    };
    Ok(ids)
}

async fn notes_since(
    pool: &PgPool,
    thread_ids: &[Uuid],
    cursor: i64,
) -> Result<Vec<Note>, AppError> {
    if thread_ids.is_empty() {
        return Ok(Vec::new());
    }
    let notes = sqlx::query_as::<_, Note>(
        "SELECT * FROM notes
         WHERE thread_id = ANY($1) AND seq > $2 AND deleted_at IS NULL
         ORDER BY seq ASC
         LIMIT 200",
    )
    .bind(thread_ids)
    .bind(cursor)
    .fetch_all(pool)
    .await?;
    Ok(notes)
}

pub async fn inbox(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Query(query): Query<InboxQuery>,
) -> Result<Response, AppError> {
    if wants_html(&headers) {
        return crate::handlers::web::inbox(
            State(state),
            caller,
            headers,
            Query(crate::handlers::web::YoursQuery {
                view: query.view.unwrap_or_else(|| "active".to_string()),
            }),
        )
        .await;
    }

    let cursor_start = query.since.unwrap_or(0);
    let mut thread_ids = callers_thread_ids(&state.pool, &caller).await?;

    let deadline = Utc::now() + INBOX_LONG_POLL;
    let mut rx = state.note_sender.subscribe();
    let mut last_refresh = Utc::now();
    loop {
        let notes = notes_since(&state.pool, &thread_ids, cursor_start).await?;
        if !notes.is_empty() || Utc::now() >= deadline {
            let cursor = notes.last().map(|n| n.seq).unwrap_or(cursor_start);
            return Ok(Json(InboxResponse { notes, cursor }).into_response());
        }

        tokio::select! {
            event = rx.recv() => {
                if let Ok(note_event) = event {
                    if !thread_ids.contains(&note_event.thread_id)
                        && (Utc::now() - last_refresh).num_milliseconds() >= INBOX_THREAD_REFRESH_MS
                    {
                        last_refresh = Utc::now();
                        if let Ok(current_thread_ids) = callers_thread_ids(&state.pool, &caller).await {
                            thread_ids = current_thread_ids;
                        }
                    }
                    if thread_ids.contains(&note_event.thread_id) {
                        let notes = notes_since(&state.pool, &thread_ids, cursor_start).await?;
                        if !notes.is_empty() || Utc::now() >= deadline {
                            let cursor = notes.last().map(|n| n.seq).unwrap_or(cursor_start);
                            return Ok(Json(InboxResponse { notes, cursor }).into_response());
                        }
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(INBOX_POLL_INTERVAL_MS)) => {}
        }
    }
}

// ---------- Agent voices: characters that talk ----------

/// An AI voice for an agent's replies. `metered` is true only for the
/// platform's own key: those calls are paid by us, so each one is charged
/// to the workspace wallet first. A user's own key is never metered.
struct Voice {
    key: String,
    model: String,
    metered: bool,
}

/// The voice chain for replies: the agent's own key first, then any
/// OpenRouter key the owner already connected, then the server default.
async fn resolve_voice(
    pool: &sqlx::PgPool,
    server_key: Option<&str>,
    user_id: Uuid,
    agent_id: Uuid,
) -> Option<Voice> {
    let own = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT provider_api_key, provider_model FROM agents
         WHERE id = $1 AND user_id = $2 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    // The model chosen in the bot's own settings wins on any of the
    // owner's keys; on the platform's key the included model is used.
    let bot_model = own
        .as_ref()
        .and_then(|(_, m)| m.clone())
        .filter(|m| !m.trim().is_empty());
    if let Some((Some(key), model)) = own {
        if let Some(key) = crate::crypto::decrypt(&key).filter(|k| !k.trim().is_empty()) {
            return Some(Voice {
                key,
                model: model.unwrap_or_default(),
                metered: false,
            });
        }
    }
    let shared = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT provider_api_key, provider_model FROM agents
         WHERE user_id = $1 AND provider_type = 'openrouter'
           AND provider_api_key IS NOT NULL AND provider_api_key <> ''
           AND deleted_at IS NULL AND revoked_at IS NULL
         ORDER BY created_at ASC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    if let Some((key, model)) = shared {
        if let Some(key) = crate::crypto::decrypt(&key) {
            return Some(Voice {
                key,
                model: bot_model.or(model).unwrap_or_default(),
                metered: false,
            });
        }
    }
    server_key.map(|k| Voice {
        key: k.to_string(),
        model: String::new(),
        metered: true,
    })
}

/// The workspace that pays for replies in a thread: always the owner's
/// primary office, so a second office never brings a second wallet of
/// free credits (`check_and_deduct` provisions a wallet on first charge).
async fn paying_workspace(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<Uuid, AppError> {
    let owner: Uuid = sqlx::query_scalar("SELECT user_id FROM threads WHERE id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await?;
    Ok(
        crate::handlers::airo::get_or_create_default_workspace(pool, owner)
            .await?
            .id,
    )
}

/// Marker the limit notes carry, so a thread gets at most one per hour
/// however many messages are sent while the wallet is empty.
const LIMIT_NOTE_MARKER: &str = "AI credit limit";

/// Tells the thread why the agent went quiet and what to do about it.
async fn post_limit_note(
    state: &AppState,
    thread_id: Uuid,
    agent_id: Uuid,
    agent_name: &str,
    limit: Option<crate::office::budget_gate::LimitHit>,
) {
    use crate::office::budget_gate::LimitHit;
    let recent: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notes
         WHERE thread_id = $1 AND author_agent_id = $2 AND deleted_at IS NULL
           AND body LIKE '%' || $3 || '%' AND created_at > now() - interval '1 hour'",
    )
    .bind(thread_id)
    .bind(agent_id)
    .bind(LIMIT_NOTE_MARKER)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);
    if recent > 0 {
        return;
    }
    let why = match limit {
        Some(LimitHit::Daily) => format!(
            "You've reached today's {LIMIT_NOTE_MARKER}, so {agent_name} can't answer until it resets tomorrow."
        ),
        Some(LimitHit::Balance) => format!(
            "Your plan has used its {LIMIT_NOTE_MARKER}, so {agent_name} can't answer right now."
        ),
        Some(LimitHit::MonthlyOffice) => format!(
            "This office has reached its monthly spending cap, the {LIMIT_NOTE_MARKER} you set, so {agent_name} can't answer with included credits until the 1st. Raise the cap in Edit office, or add your own OpenRouter key in Settings."
        ),
        Some(LimitHit::MonthlyBot) => format!(
            "{agent_name} has reached its own monthly spending cap, the {LIMIT_NOTE_MARKER} you set, so it can't answer with included credits until the 1st. Raise it in {agent_name}'s settings."
        ),
        None => format!("{agent_name} is paused by the workspace's {LIMIT_NOTE_MARKER}."),
    };
    if matches!(limit, Some(LimitHit::MonthlyOffice | LimitHit::MonthlyBot)) {
        insert_reply_note(&state.pool, &state.note_sender, thread_id, agent_id, &why).await;
        return;
    }
    let next = match state.config.upgrade_url.as_deref() {
        Some(url) => format!(
            "Upgrade your plan for more ({url}), or add your own OpenRouter key in Settings to keep chatting now."
        ),
        None => "Upgrade your plan for more, or add your own OpenRouter key in Settings to keep chatting now.".to_string(),
    };
    insert_reply_note(
        &state.pool,
        &state.note_sender,
        thread_id,
        agent_id,
        &format!("{why} {next}"),
    )
    .await;
}

/// The last few messages of a thread, as conversation history.
async fn conversation_history(pool: &sqlx::PgPool, thread_id: Uuid, limit: i64) -> String {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT COALESCE(a.name, u.handle, 'someone'), n.body
         FROM notes n
         LEFT JOIN agents a ON a.id = n.author_agent_id
         LEFT JOIN users u ON u.id = n.author_user_id
         WHERE n.thread_id = $1 AND n.deleted_at IS NULL
         ORDER BY n.created_at DESC LIMIT $2",
    )
    .bind(thread_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .rev()
        .map(|(who, body)| format!("{who}: {body}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Post a reply note as the agent and push it to the thread's live view.
pub(crate) async fn insert_reply_note(
    pool: &sqlx::PgPool,
    note_sender: &tokio::sync::broadcast::Sender<crate::state::NoteEvent>,
    thread_id: Uuid,
    agent_id: Uuid,
    body: &str,
) {
    insert_bot_note(pool, note_sender, thread_id, agent_id, body, None).await;
}

/// [`insert_reply_note`] with one-tap actions for the person (buttons the
/// app shows under the note).
pub(crate) async fn insert_bot_note(
    pool: &sqlx::PgPool,
    note_sender: &tokio::sync::broadcast::Sender<crate::state::NoteEvent>,
    thread_id: Uuid,
    agent_id: Uuid,
    body: &str,
    actions: Option<serde_json::Value>,
) {
    let note = sqlx::query_as::<_, Note>(
        "INSERT INTO notes (thread_id, author_agent_id, body, kind, actions)
         VALUES ($1, $2, $3, 'status', $4)
         RETURNING *",
    )
    .bind(thread_id)
    .bind(agent_id)
    .bind(body)
    .bind(&actions)
    .fetch_one(pool)
    .await;
    let Ok(reply_note) = note else {
        return;
    };
    let _ = sqlx::query(
        "UPDATE threads SET note_count = note_count + 1, last_note_at = now(), updated_at = now()
         WHERE id = $1",
    )
    .bind(thread_id)
    .execute(pool)
    .await;
    let row = sqlx::query_as::<_, NoteRow>(
        "SELECT n.id, n.author_user_id, n.body, n.kind, n.created_at,
                u.handle AS author_handle, a.name AS author_agent_name
         FROM notes n
         LEFT JOIN users u ON u.id = n.author_user_id
         LEFT JOIN agents a ON a.id = n.author_agent_id
         WHERE n.id = $1",
    )
    .bind(reply_note.id)
    .fetch_optional(pool)
    .await;
    if let Ok(Some(row)) = row {
        let mut event = note_event(thread_id, &row);
        if let Some(obj) = event.note.as_object_mut() {
            obj.insert(
                "actions".to_string(),
                reply_note.actions.unwrap_or_default(),
            );
        }
        let _ = note_sender.send(event);
    }
}

/// How recently a desktop bot must have been seen to take a chat reply.
/// It polls every few seconds while it answers chats.
const DESKTOP_ONLINE_SECS: i64 = 90;

/// Queues this reply for the person's desktop bot when they turned on
/// "answer my chats on this computer" and it is online. Only the person's
/// own message, in their own thread, to their own agent ever goes there, so
/// nobody else's request reaches their subscription.
async fn queue_desktop_reply(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    agent: &crate::models::Agent,
    user_id: Uuid,
    system: &str,
    history: &str,
) -> Result<bool, AppError> {
    let route: Option<Uuid> = sqlx::query_scalar(
        "SELECT u.chat_desktop_agent_id
         FROM users u
         JOIN agents d ON d.id = u.chat_desktop_agent_id
         JOIN threads t ON t.id = $2
         WHERE u.id = $1 AND d.user_id = u.id
           AND d.revoked_at IS NULL AND d.deleted_at IS NULL
           AND d.last_seen_at > now() - make_interval(secs => $3)
           AND t.user_id = u.id",
    )
    .bind(user_id)
    .bind(thread_id)
    .bind(DESKTOP_ONLINE_SECS as f64)
    .fetch_optional(pool)
    .await?;
    let Some(desktop_id) = route else {
        return Ok(false);
    };
    if agent.user_id != user_id {
        return Ok(false);
    }
    let workspace = crate::handlers::airo::get_or_create_default_workspace(pool, user_id).await?;
    sqlx::query(
        "INSERT INTO tasks (
            workspace_id, thread_id, title, status, priority, payload,
            created_by_actor_type, created_by_actor_id, assigned_agent_id,
            executor, fsm_state
         )
         VALUES ($1, $2, $3, 'queued', 'high', $4, 'user', $5, $6, 'agent', 'QUEUED')",
    )
    .bind(workspace.id)
    .bind(thread_id)
    .bind(format!("Reply in chat as {}", agent.name))
    .bind(serde_json::json!({
        "kind": "chat_reply",
        "reply_as_agent_id": agent.id,
        "agent_name": agent.name,
        "system": system,
        "history": history,
        "requested_at": chrono::Utc::now(),
    }))
    .bind(user_id)
    .bind(desktop_id)
    .execute(pool)
    .await?;
    Ok(true)
}

/// Credits one chat reply on the platform's key costs (same flat rate as
/// every other metered call until pricing follows real usage).
const CHAT_REPLY_CREDITS: i64 = crate::office::budget_gate::FLAT_CALL_CREDITS;

/// Free chat: any human message in a thread reaches its agent, and the
/// agent answers in its own character using the conversation so far. With
/// no AI voice anywhere, it says so once instead of going quiet.
/// How an agent answers in a thread.
#[derive(Default, Clone)]
pub(crate) struct ReplyMode {
    /// The Architect of an office group chat: its answer may hand work to
    /// teammates with `@handle: task` lines instead of answering itself.
    route: bool,
    /// The work this bot was asked to do, as a line shown after the
    /// conversation ("Architect (to Sales Lead): ..."), so it knows why.
    task_line: Option<String>,
}

impl ReplyMode {
    pub(crate) fn route() -> Self {
        Self {
            route: true,
            task_line: None,
        }
    }

    /// Adds the line telling the bot what it was asked to do.
    pub(crate) fn with_task_line(mut self, line: String) -> Self {
        self.task_line = Some(line);
        self
    }

    /// A task the office gives a bot on its own (kickoff, daily plan).
    pub(crate) fn task(bot_name: &str, task: &str) -> Self {
        Self {
            route: false,
            task_line: Some(format!("Your task now, {bot_name}: {task}")),
        }
    }
}

/// Posts an agent's answer. In routing mode, `@handle: task` lines in the
/// Architect's answer start those bots instead of being shown. `credits` is
/// what the answer cost, kept with a job's outcome.
#[allow(clippy::too_many_arguments)]
async fn deliver_reply(
    state: &AppState,
    thread_id: Uuid,
    architect: &crate::models::Agent,
    user_id: Uuid,
    mode: &ReplyMode,
    team: &[crate::office::group::Teammate],
    body: &str,
    credits: i64,
) {
    let pool = &state.pool;
    // A job (work the bot was given, not a chat message) ends with its
    // outcome: the OUTCOME line becomes an italic line, and the job is kept.
    let job_body;
    let body = match mode.task_line.as_deref().filter(|_| !mode.route) {
        Some(task_line) => {
            let (text, outcome) = crate::office::outcome::take(body);
            let (outcome, note) = match outcome {
                Some((o, note)) => (Some(o), note),
                None => (None, String::new()),
            };
            crate::office::outcome::record_job(
                pool,
                thread_id,
                architect.id,
                task_line,
                outcome,
                &note,
                credits,
            )
            .await;
            job_body = match outcome {
                Some(o) => format!("{text}\n\n{}", crate::office::outcome::line(o, &note))
                    .trim()
                    .to_string(),
                None => text,
            };
            job_body.as_str()
        }
        None => body,
    };
    // Records the bot wrote (`ADD <app> {...}` lines) go into the office's
    // apps; the chat shows a short "Added ..." line instead.
    let (body, actions) =
        crate::office::context::apply_records(pool, thread_id, architect.id, body).await;
    // Facts it saved (`REMEMBER: ...` lines) go into the office's memory and
    // the message says so, with an Undo button each.
    let (body, saved) =
        crate::office::memory::apply_remember(pool, thread_id, architect.id, &body).await;
    // `[doc 1a2b3c4d]` citations become "(from “Price list”)".
    let body = crate::office::documents::cite(pool, thread_id, &body).await;
    // Milestones it proposed (`MILESTONE:` lines) wait for the owner's
    // Keep.
    let (body, proposed) =
        crate::office::goals::apply_milestones(pool, thread_id, architect.id, &body).await;
    let (body, actions) = if proposed.is_empty() {
        (body, actions)
    } else {
        let (line, buttons) = crate::office::goals::proposed_note(&proposed);
        let mut all = actions
            .and_then(|a| a.as_array().cloned())
            .unwrap_or_default();
        all.extend(buttons);
        let body = if body.is_empty() {
            line
        } else {
            format!("{body}\n\n{line}")
        };
        (body, Some(serde_json::Value::Array(all)))
    };
    let (body, actions) = if saved.is_empty() {
        (body, actions)
    } else {
        let (line, buttons) = crate::office::memory::remembered_note(&saved);
        let mut all = actions
            .and_then(|a| a.as_array().cloned())
            .unwrap_or_default();
        all.extend(buttons);
        let body = if body.is_empty() {
            line
        } else {
            format!("{body}\n\n{line}")
        };
        (body, Some(serde_json::Value::Array(all)))
    };
    let body = body.as_str();
    if !mode.route || team.is_empty() {
        insert_bot_note(
            pool,
            &state.note_sender,
            thread_id,
            architect.id,
            body,
            actions,
        )
        .await;
        return;
    }
    let (calls, own) = crate::office::group::parse_delegations(body, team);
    if calls.is_empty() {
        insert_bot_note(
            pool,
            &state.note_sender,
            thread_id,
            architect.id,
            body,
            actions,
        )
        .await;
        return;
    }
    let said = if own.is_empty() {
        let names: Vec<String> = calls.iter().map(|(_, name, _)| name.clone()).collect();
        format!("Passing this to {}.", names.join(" and "))
    } else {
        own
    };
    insert_reply_note(pool, &state.note_sender, thread_id, architect.id, &said).await;
    for (bot_id, name, task) in calls {
        spawn_agent_reply(
            state.clone(),
            thread_id,
            bot_id,
            user_id,
            ReplyMode {
                route: false,
                task_line: Some(format!("Architect (to {name}): {task}")),
            },
        );
    }
}

/// A job that ended without an answer is kept too, with why.
async fn record_unanswered_job(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    agent_id: Uuid,
    mode: &ReplyMode,
    outcome: crate::office::outcome::Outcome,
    note: &str,
) {
    if let Some(task_line) = mode.task_line.as_deref().filter(|_| !mode.route) {
        crate::office::outcome::record_job(
            pool,
            thread_id,
            agent_id,
            task_line,
            Some(outcome),
            note,
            0,
        )
        .await;
    }
}

pub(crate) fn spawn_agent_reply(
    state: AppState,
    thread_id: Uuid,
    agent_id: Uuid,
    user_id: Uuid,
    mode: ReplyMode,
) {
    tokio::spawn(run_agent_reply(state, thread_id, agent_id, user_id, mode));
}

/// Whether this answer may use the web, from the account's daily allowance.
async fn web_access(state: &AppState, thread_id: Uuid) -> crate::web::WebAccess {
    let taken = match paying_workspace(&state.pool, thread_id).await {
        Ok(ws) => crate::web::take_run(&state.pool, ws, &state.config.web)
            .await
            .map_err(AppError::from),
        Err(error) => Err(error),
    };
    taken.unwrap_or_else(|error| {
        tracing::warn!(?error, %thread_id, "web allowance unavailable; answering without the web");
        crate::web::WebAccess::Off
    })
}

/// One agent's answer in a thread, start to finish (posted when it is
/// ready). [`spawn_agent_reply`] runs it in the background; office kickoff
/// awaits it so first tasks run one after another.
pub(crate) async fn run_agent_reply(
    state: AppState,
    thread_id: Uuid,
    agent_id: Uuid,
    user_id: Uuid,
    mode: ReplyMode,
) {
    {
        let pool = state.pool.clone();
        // A stopped office starts no new work until it is run again.
        let paused: bool = sqlx::query_scalar(
            "SELECT COALESCE(w.paused, false) FROM threads t
             LEFT JOIN workspaces w ON w.id = t.workspace_id
             WHERE t.id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(false);
        if paused {
            return;
        }
        let agent = sqlx::query_as::<_, crate::models::Agent>(
            "SELECT * FROM agents WHERE id = $1 AND deleted_at IS NULL AND revoked_at IS NULL",
        )
        .bind(agent_id)
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten();
        let Some(agent) = agent else {
            return;
        };
        let mut sys = match agent.instructions.clone() {
            Some(ins) if !ins.trim().is_empty() => ins.trim().to_string(),
            _ => format!(
                "You are '{}', an agent connected to the Air noticeboard. Answer briefly and helpfully.",
                agent.name
            ),
        };
        let mut history = conversation_history(&pool, thread_id, 20).await;
        // The Architect of a group chat is told who is on the team.
        let team = if mode.route {
            crate::office::group::roster(&pool, thread_id, agent_id)
                .await
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if mode.route && !team.is_empty() {
            sys.push_str(&crate::office::group::roster_prompt(&team));
        }
        // Office bots know the business and its apps; the general
        // Assistant stays general.
        if agent.name != crate::office::ASSISTANT_NAME {
            if let Some(office) =
                crate::office::context::prompt_for_thread(&pool, thread_id, agent_id).await
            {
                sys.push_str(&office);
            }
            // Fresh data from the connected tools this bot may use.
            if let Some(business) =
                crate::office::context::business_of_thread(&pool, thread_id).await
            {
                sys.push_str(
                    &crate::office::access::tools_block(&state, business.workspace_id, agent_id)
                        .await,
                );
                // The contacts this conversation names, with their facts.
                sys.push_str(
                    &crate::office::contacts::context_block(&pool, business.workspace_id, &history)
                        .await,
                );
                // The passages of the office's documents that fit this
                // conversation, and whether to answer only from them.
                sys.push_str(
                    &crate::office::documents::context_block(
                        &pool,
                        business.workspace_id,
                        agent_id,
                        &history,
                    )
                    .await,
                );
            }
        }
        if let Some(line) = &mode.task_line {
            history.push('\n');
            history.push_str(line);
            if !mode.route {
                sys.push_str(crate::office::outcome::INSTRUCTION);
            }
        }

        // Local mode (desktop app): the AI tool chosen on this computer
        // answers, in the agent's character, with nothing metered.
        if let Some(ai) = state.local_ai.clone() {
            let prompt = format!(
                "The conversation so far, oldest first:\n\n{history}\n\nWrite {}'s next message only, in character. No preamble.",
                agent.name
            );
            let web = web_access(&state, thread_id).await;
            let request = crate::local::LocalAiRequest {
                system: format!("{sys}{}", web.prompt_note()),
                prompt,
                engine: agent.ai_engine.clone(),
                model: agent.ai_model.clone(),
                web: web.is_on(),
            };
            match ai(request).await {
                Ok(text) if !text.trim().is_empty() => {
                    deliver_reply(
                        &state,
                        thread_id,
                        &agent,
                        user_id,
                        &mode,
                        &team,
                        text.trim(),
                        0,
                    )
                    .await;
                }
                other => {
                    record_unanswered_job(
                        &pool,
                        thread_id,
                        agent_id,
                        &mode,
                        crate::office::outcome::Outcome::Failed,
                        "the AI on this computer didn't answer",
                    )
                    .await;
                    let body = match other {
                        Ok(_) => format!("{} had nothing to say. Try asking again.", agent.name),
                        Err(err) => format!(
                            "{} couldn't answer: {err}. Choose or sign in to your AI in Settings → Connections → Phone and this computer.",
                            agent.name
                        ),
                    };
                    insert_reply_note(&pool, &state.note_sender, thread_id, agent_id, &body).await;
                }
            }
            return;
        }

        // The person asked for their own chats to be answered by their
        // desktop bot (their own AI subscription on their computer): hand
        // the reply to it while it is online. Nothing is charged here.
        match queue_desktop_reply(&pool, thread_id, &agent, user_id, &sys, &history).await {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(?error, %thread_id, "desktop reply routing failed; answering here")
            }
        }

        let server_key = state.config.openrouter_api_key.clone();
        match resolve_voice(&pool, server_key.as_deref(), user_id, agent_id).await {
            Some(voice) => {
                use crate::office::budget_gate::{self, ActionProposal};
                let web = web_access(&state, thread_id).await;
                let sys = format!("{sys}{}", web.prompt_note());
                let model = if voice.model.trim().is_empty() {
                    crate::openrouter::DEFAULT_MODEL.to_string()
                } else {
                    voice.model.clone()
                };
                // The platform's key costs us money: charge the workspace
                // before calling, refuse when a limit is reached, and refund
                // if the provider fails. Fails closed on database errors.
                let mut charge: Option<(Uuid, ActionProposal, Option<Uuid>, budget_gate::Spender)> =
                    None;
                if voice.metered {
                    let workspace_id = match paying_workspace(&pool, thread_id).await {
                        Ok(id) => id,
                        Err(error) => {
                            tracing::warn!(?error, %thread_id, "no paying workspace for chat reply");
                            return;
                        }
                    };
                    // The office the room is in (rooms without one belong to
                    // the first office) and this bot: both caps apply.
                    let office: Option<Uuid> =
                        sqlx::query_scalar("SELECT workspace_id FROM threads WHERE id = $1")
                            .bind(thread_id)
                            .fetch_optional(&pool)
                            .await
                            .ok()
                            .flatten()
                            .flatten();
                    let spender =
                        budget_gate::Spender::new(office.unwrap_or(workspace_id), Some(agent_id));
                    let proposal = ActionProposal {
                        provider: "openrouter".into(),
                        model_or_tool: model.clone(),
                        estimated_credits: CHAT_REPLY_CREDITS
                            + if web.is_on() {
                                crate::web::WEB_RUN_CREDITS
                            } else {
                                0
                            },
                        raw_cost_micros: 250,
                        requires_manual_approval: false,
                        action_type: "chat_reply".into(),
                    };
                    match budget_gate::check_and_deduct(
                        &pool,
                        workspace_id,
                        &proposal,
                        None,
                        false,
                        spender,
                    )
                    .await
                    {
                        Err(error) => {
                            tracing::warn!(?error, %thread_id, "budget gate unavailable; reply skipped");
                            return;
                        }
                        Ok(gate) if !gate.passed || gate.requires_approval => {
                            record_unanswered_job(
                                &pool,
                                thread_id,
                                agent_id,
                                &mode,
                                crate::office::outcome::Outcome::Blocked,
                                "a spending limit was reached",
                            )
                            .await;
                            // A monthly cap posts its card once; the room
                            // that got it needs no second note.
                            let carded = match gate.limit {
                                Some(limit) => {
                                    crate::office::spend::notify_cap(
                                        &state,
                                        limit,
                                        spender,
                                        Some(thread_id),
                                    )
                                    .await
                                }
                                None => None,
                            };
                            if carded != Some(thread_id) {
                                post_limit_note(
                                    &state,
                                    thread_id,
                                    agent_id,
                                    &agent.name,
                                    gate.limit,
                                )
                                .await;
                            }
                            return;
                        }
                        Ok(gate) => {
                            charge = Some((workspace_id, proposal, gate.ledger_id, spender))
                        }
                    }
                }
                match crate::openrouter::call_openrouter_web(
                    &voice.key,
                    &model,
                    &sys,
                    &history,
                    web.is_on().then_some(&state.config.web),
                )
                .await
                {
                    Ok(completion) => {
                        if let Some((_, _, ledger_id, _)) = &charge {
                            budget_gate::record_usage(&pool, *ledger_id, &completion.usage).await;
                        }
                        let trimmed = completion.text.trim();
                        if !trimmed.is_empty() {
                            let credits = charge
                                .as_ref()
                                .map(|(_, p, _, _)| p.estimated_credits)
                                .unwrap_or(0);
                            deliver_reply(
                                &state, thread_id, &agent, user_id, &mode, &team, trimmed, credits,
                            )
                            .await;
                        }
                    }
                    Err(err) => {
                        tracing::warn!(%err, agent = %agent.name, "agent reply failed");
                        record_unanswered_job(
                            &pool,
                            thread_id,
                            agent_id,
                            &mode,
                            crate::office::outcome::Outcome::Failed,
                            "the AI provider didn't answer",
                        )
                        .await;
                        if let Some((workspace_id, proposal, _, spender)) = &charge {
                            let _ = budget_gate::refund_credit_charge(
                                &pool,
                                *workspace_id,
                                None,
                                proposal,
                                *spender,
                            )
                            .await;
                        }
                    }
                }
            }
            None => {
                record_unanswered_job(
                    &pool,
                    thread_id,
                    agent_id,
                    &mode,
                    crate::office::outcome::Outcome::Blocked,
                    "no AI is connected",
                )
                .await;
                post_no_ai_note(&state, thread_id, agent_id, &agent.name).await
            }
        }
    }
}

/// What the Architect says in an office group chat when no bot has an AI.
const TEAM_NEEDS_AI: &str = "Connect an AI to wake up your team: add an OpenRouter key in Settings → Account, and every bot here will answer in its own character.";

/// Says, once per thread, that no AI can answer yet, with a button to
/// Settings → Account. In an office group chat the Architect says it for the
/// whole team, so a new office does not open on one warning per bot (each
/// bot keeps its "Needs AI" badge); in a bot's own room the bot says it.
async fn post_no_ai_note(state: &AppState, thread_id: Uuid, agent_id: Uuid, agent_name: &str) {
    let pool = &state.pool;
    let connect = json!([{ "label": "Connect an AI", "kind": "open", "target": "account" }]);
    let architect: Option<Uuid> = sqlx::query_scalar(
        "SELECT a.id FROM threads t
         JOIN thread_agents ta ON ta.thread_id = t.id
         JOIN agents a ON a.id = ta.agent_id AND a.deleted_at IS NULL
         WHERE t.id = $1 AND t.kind = 'office' AND a.name = $2
         LIMIT 1",
    )
    .bind(thread_id)
    .bind(crate::office::group::ARCHITECT_NAME)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    // TODO(air): two bots answering at the same moment can both see "not
    // said yet" and post twice. The kickoff runs bots one at a time, so this
    // is rare; a unique marker on notes would close it at the cost of a
    // migration.
    let (author, body, said) = match architect {
        Some(architect) => {
            let said: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM notes WHERE thread_id = $1 AND body = $2)",
            )
            .bind(thread_id)
            .bind(TEAM_NEEDS_AI)
            .fetch_one(pool)
            .await
            .unwrap_or(true);
            (architect, TEAM_NEEDS_AI.to_string(), said)
        }
        None => {
            let said: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM notes
                                WHERE thread_id = $1 AND author_agent_id = $2
                                  AND body LIKE '%needs an AI voice%')",
            )
            .bind(thread_id)
            .bind(agent_id)
            .fetch_one(pool)
            .await
            .unwrap_or(true);
            let body = format!(
                "{agent_name} needs an AI voice to answer: add an OpenRouter key in Settings → Account, and it will talk in its own character."
            );
            (agent_id, body, said)
        }
    };
    if !said {
        insert_bot_note(
            pool,
            &state.note_sender,
            thread_id,
            author,
            &body,
            Some(connect),
        )
        .await;
    }
}

/// Thread titles should read like topics ("Cactus check-in"), not
/// sentences. A sentence-shaped title gets rewritten by the thread's
/// agent; without a voice it is trimmed to its first words.
fn spawn_auto_title(
    state: AppState,
    thread_id: Uuid,
    title: String,
    agent_id: Option<Uuid>,
    user_id: Uuid,
) {
    let words = title.split_whitespace().count();
    if title.chars().count() <= 48 && words < 5 {
        return;
    }
    tokio::spawn(async move {
        let pool = state.pool.clone();
        let mut short: Option<String> = None;
        if let Some(aid) = agent_id {
            // Titles are a nicety: only a user's own key may spend on them.
            // On the platform's key the free trim below is used instead.
            let server_key = state.config.openrouter_api_key.clone();
            let voice = resolve_voice(&pool, server_key.as_deref(), user_id, aid)
                .await
                .filter(|v| !v.metered);
            if let Some(voice) = voice {
                let prompt = format!("Conversation starts with: {title}");
                if let Ok(t) = crate::openrouter::call_openrouter(
                    &voice.key,
                    &voice.model,
                    "Give this conversation a short topic title of at most five words. Plain words, no quotes, no trailing punctuation.",
                    &prompt,
                )
                .await
                {
                    let t = t.trim().trim_matches(|c| c == '"' || c == '.' || c == '!').to_string();
                    if !t.is_empty() && t.chars().count() <= 60 {
                        short = Some(t);
                    }
                }
            }
        }
        let new_title = short.unwrap_or_else(|| {
            title
                .split_whitespace()
                .take(5)
                .collect::<Vec<_>>()
                .join(" ")
        });
        let _ = sqlx::query("UPDATE threads SET title = $1, updated_at = now() WHERE id = $2")
            .bind(&new_title)
            .bind(thread_id)
            .execute(&pool)
            .await;
    });
}

#[derive(Deserialize)]
pub struct RenameThreadRequest {
    pub title: String,
    pub idempotency_key: Option<String>,
}

/// POST /threads/:id/rename -- give a thread a topic title by hand. The
/// auto-titler does this automatically; this is the human override.
pub async fn rename_thread(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(thread_id): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<RenameThreadRequest>,
) -> Result<Response, AppError> {
    let thread = find_thread(&state.pool, thread_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if thread.user_id != caller.user_id() {
        return Err(AppError::Forbidden);
    }
    if thread.kind == "office" {
        return Err(AppError::BadRequest(
            "an office's group chat is named after the office; rename the office".to_string(),
        ));
    }
    let title = body.title.trim();
    if title.is_empty() || title.chars().count() > 120 {
        return Err(AppError::BadRequest(
            "title must be 1-120 characters".to_string(),
        ));
    }
    sqlx::query("UPDATE threads SET title = $1, updated_at = now() WHERE id = $2")
        .bind(title)
        .bind(thread.id)
        .execute(&state.pool)
        .await?;
    if wants_html(&headers) {
        Ok(Redirect::to(&format!("/t/{}", thread.id)).into_response())
    } else {
        Ok(Json(serde_json::json!({ "status": "renamed", "title": title })).into_response())
    }
}
