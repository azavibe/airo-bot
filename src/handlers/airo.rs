use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use sha2::Digest;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::Caller;
use crate::error::AppError;
use crate::models::{
    Approval, AuditEvent, ContextPack, Lease, Node, Note, Task, Thread, Workspace,
};
use crate::state::AppState;

use super::{render, slug_handle, unique_handle, wants_html, JsonOrForm};

// ---------- View Helpers ----------

pub fn time_ago(dt: DateTime<Utc>) -> String {
    let secs = (Utc::now() - dt).num_seconds();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        let m = secs / 60;
        format!("{} minute{} ago", m, if m == 1 { "" } else { "s" })
    } else if secs < 86400 {
        let h = secs / 3600;
        format!("{} hour{} ago", h, if h == 1 { "" } else { "s" })
    } else if secs < 2_592_000 {
        let d = secs / 86400;
        format!("{} day{} ago", d, if d == 1 { "" } else { "s" })
    } else {
        dt.format("%b %d, %Y").to_string()
    }
}

// ---------- Audit Helper ----------

#[allow(clippy::too_many_arguments)]
pub async fn record_audit(
    pool: &PgPool,
    workspace_id: Option<Uuid>,
    group_id: Option<Uuid>,
    task_id: Option<Uuid>,
    run_id: Option<Uuid>,
    actor_type: &str,
    actor_id: Option<Uuid>,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<Uuid>,
    payload: serde_json::Value,
    result: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO audit_events (
            id, workspace_id, group_id, task_id, run_id,
            actor_type, actor_id, action, target_type, target_id,
            payload, result, created_at, updated_at
        )
        VALUES (
            gen_random_uuid(), $1, $2, $3, $4,
            $5, $6, $7, $8, $9,
            $10, $11, now(), now()
        )",
    )
    .bind(workspace_id)
    .bind(group_id)
    .bind(task_id)
    .bind(run_id)
    .bind(actor_type)
    .bind(actor_id)
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .bind(payload)
    .bind(result)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------- Context pack redaction ----------

const SECRET_KEY_HINTS: [&str; 9] = [
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "authorization",
    "private_key",
    "credential",
];
const SECRET_VALUE_PREFIXES: [&str; 7] = [
    "sk-",
    "air_ag_",
    "ghp_",
    "github_pat_",
    "xox",
    "AKIA",
    "-----BEGIN",
];

/// Best-effort scrub of credentials before a payload is sealed into a
/// context pack: values under secret-looking keys, and strings that look
/// like well-known key formats, become "[redacted]". This is a safety net,
/// not a guarantee -- task authors still must not put secrets in payloads.
/// TODO(air): pattern list is deliberately small; extend from real leaks
/// rather than guessing broadly and mangling ordinary text.
pub fn redact_secrets(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    let key = k.to_ascii_lowercase();
                    if SECRET_KEY_HINTS.iter().any(|h| key.contains(h)) && !v.is_null() {
                        (k, Value::String("[redacted]".to_string()))
                    } else {
                        (k, redact_secrets(v))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(redact_secrets).collect()),
        Value::String(s) => {
            let t = s.trim_start();
            if SECRET_VALUE_PREFIXES.iter().any(|p| t.starts_with(p)) && !t.contains(' ') {
                Value::String("[redacted]".to_string())
            } else {
                Value::String(s)
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod redaction_tests {
    use super::redact_secrets;
    use serde_json::json;

    #[test]
    fn scrubs_secret_keys_and_key_shaped_values() {
        let out = redact_secrets(json!({
            "repo": "acme/web",
            "api_key": "abc",
            "nested": { "DB_PASSWORD": "hunter2", "note": "sk-or-v1-123" },
            "list": ["ghp_abcdef", "plain text"],
            "summary": "sk- is how keys start"
        }));
        assert_eq!(out["repo"], "acme/web");
        assert_eq!(out["api_key"], "[redacted]");
        assert_eq!(out["nested"]["DB_PASSWORD"], "[redacted]");
        assert_eq!(out["nested"]["note"], "[redacted]");
        assert_eq!(out["list"][0], "[redacted]");
        assert_eq!(out["list"][1], "plain text");
        assert_eq!(out["summary"], "sk- is how keys start");
    }
}

// ---------- Workspace Resolution Helper ----------

pub async fn get_or_create_default_workspace(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Workspace, AppError> {
    // A person's own first office: never one they were invited to, so an
    // invited teammate gets an office of their own as well.
    let existing = sqlx::query_as::<_, Workspace>(
        "SELECT w.* FROM workspaces w
         WHERE w.owner_user_id = $1 AND w.deleted_at IS NULL
         ORDER BY w.created_at ASC
         LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    if let Some(ws) = existing {
        return Ok(ws);
    }

    let ws = sqlx::query_as::<_, Workspace>(
        "INSERT INTO workspaces (id, name, slug, owner_user_id, description, created_at, updated_at)
         VALUES (gen_random_uuid(), 'My office', 'default', $1, 'Your first office', now(), now())
         RETURNING *",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;

    let _ = sqlx::query(
        "INSERT INTO workspace_members (id, workspace_id, user_id, role, created_at, updated_at)
         VALUES (gen_random_uuid(), $1, $2, 'owner', now(), now())
         ON CONFLICT DO NOTHING",
    )
    .bind(ws.id)
    .bind(user_id)
    .execute(pool)
    .await?;

    let _ = record_audit(
        pool,
        Some(ws.id),
        None,
        None,
        None,
        "system",
        Some(user_id),
        "workspace.created",
        Some("workspace"),
        Some(ws.id),
        json!({ "name": ws.name, "slug": ws.slug }),
        "ok",
    )
    .await;

    Ok(ws)
}

/// The caller owns this office (or is one of the owner's bots). Setting an
/// office up (bots, tasks, connections, apps, devices) is the owner's;
/// invited members and viewers get only what `office::team` grants them.
pub async fn ensure_workspace_access(
    pool: &PgPool,
    caller: &Caller,
    workspace_id: Uuid,
) -> Result<(), AppError> {
    ensure_workspace_role(pool, caller, workspace_id, crate::office::team::Role::Owner).await
}

/// The caller has at least `needed` in this office; otherwise forbidden.
pub async fn ensure_workspace_role(
    pool: &PgPool,
    caller: &Caller,
    workspace_id: Uuid,
    needed: crate::office::team::Role,
) -> Result<(), AppError> {
    match crate::office::team::role_in(pool, caller.user_id(), workspace_id).await? {
        Some(role) if role >= needed => Ok(()),
        _ => Err(AppError::Forbidden),
    }
}

// ---------- View Models ----------

pub struct TaskView {
    pub id: Uuid,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub bot_label: String,
    pub created_ago: String,
    pub thread_id: Option<Uuid>,
}

pub struct ApprovalView {
    pub id: Uuid,
    /// The office it belongs to.
    pub workspace_id: Uuid,
    /// The caller may approve or reject it (not a viewer).
    pub can_decide: bool,
    pub task_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub task_title: String,
    pub requested_action: String,
    pub risk_level: String,
    pub status: String,
    pub requested_by: String,
    pub bot_label: String,
    pub message_body: String,
    pub created_ago: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub reason: Option<String>,
}

pub struct AgentChoice {
    pub id: Uuid,
    pub name: String,
    pub description: String,
}

pub struct AuditEventView {
    pub id: Uuid,
    pub actor_label: String,
    pub action: String,
    pub action_text: String,
    pub target_label: String,
    pub result: String,
    pub created_ago: String,
}

// ---------- Templates ----------

#[derive(Template)]
#[template(path = "tasks.html")]
pub struct TasksTemplate {
    pub active_tab: &'static str,
    pub workspace: Workspace,
    pub tasks: Vec<TaskView>,
    pub filter_status: String,
}

#[derive(Template)]
#[template(path = "task_detail.html")]
pub struct TaskDetailTemplate {
    pub active_tab: &'static str,
    pub task: Task,
    pub workspace: Workspace,
    pub assigned_bot: Option<String>,
    pub notes: Vec<Note>,
    pub approval: Option<Approval>,
}

#[derive(Template)]
#[template(path = "approvals.html")]
pub struct ApprovalsTemplate {
    pub active_tab: &'static str,
    pub approvals: Vec<ApprovalView>,
    pub bots: Vec<AgentChoice>,
    pub handled_without_you: i64,
    pub running_now: i64,
}

#[derive(Template)]
#[template(path = "activity.html")]
pub struct ActivityTemplate {
    pub active_tab: &'static str,
    pub events: Vec<AuditEventView>,
}

// ---------- Handlers ----------

// ---------- Tasks ----------

#[derive(Deserialize)]
pub struct TaskFilter {
    pub workspace_id: Option<Uuid>,
    pub status: Option<String>,
}

/// `id -> name` for `ids`, by `sql` (which takes the ids as `$1`); no
/// query when there are none.
async fn names_by_id(
    pool: &PgPool,
    sql: &'static str,
    ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, String>, AppError> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    Ok(sqlx::query_as::<_, (Uuid, String)>(sql)
        .bind(ids)
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect())
}

/// GET /tasks — List tasks
pub async fn list_tasks(
    State(state): State<AppState>,
    caller: Caller,
    Query(filter): Query<TaskFilter>,
    headers: axum::http::HeaderMap,
) -> Result<Response, AppError> {
    let ws = if let Some(wid) = filter.workspace_id {
        ensure_workspace_access(&state.pool, &caller, wid).await?;
        sqlx::query_as::<_, Workspace>("SELECT * FROM workspaces WHERE id = $1")
            .bind(wid)
            .fetch_one(&state.pool)
            .await?
    } else {
        // The caller's first office, the same one the rest of the app means
        // by "your office" (an unordered pick could land on any of them).
        get_or_create_default_workspace(&state.pool, caller.user_id()).await?
    };

    let status_filter = filter.status.unwrap_or_else(|| "all".to_string());
    let raw_tasks = if status_filter == "all" || status_filter.is_empty() {
        sqlx::query_as::<_, Task>(
            "SELECT * FROM tasks WHERE workspace_id = $1 AND deleted_at IS NULL ORDER BY created_at DESC LIMIT 100",
        )
        .bind(ws.id)
        .fetch_all(&state.pool)
        .await?
    } else {
        sqlx::query_as::<_, Task>(
            "SELECT * FROM tasks WHERE workspace_id = $1 AND status = $2 AND deleted_at IS NULL ORDER BY created_at DESC LIMIT 100",
        )
        .bind(ws.id)
        .bind(&status_filter)
        .fetch_all(&state.pool)
        .await?
    };

    // The assignees' names in one query per kind, not one per task.
    let agent_ids: Vec<Uuid> = raw_tasks
        .iter()
        .filter_map(|t| t.assigned_agent_id)
        .collect();
    let node_ids: Vec<Uuid> = raw_tasks
        .iter()
        .filter(|t| t.assigned_agent_id.is_none())
        .filter_map(|t| t.assigned_node_id)
        .collect();
    let agent_names = names_by_id(
        &state.pool,
        "SELECT id, name FROM agents WHERE id = ANY($1) AND deleted_at IS NULL",
        &agent_ids,
    )
    .await?;
    let node_names = names_by_id(
        &state.pool,
        "SELECT id, name FROM nodes WHERE id = ANY($1) AND deleted_at IS NULL",
        &node_ids,
    )
    .await?;

    let mut task_views = Vec::new();
    for t in raw_tasks {
        let bot_label = if let Some(agent_id) = t.assigned_agent_id {
            let name = agent_names.get(&agent_id).map_or("agent", String::as_str);
            format!("@{}", name)
        } else if let Some(node_id) = t.assigned_node_id {
            let name = node_names.get(&node_id).map_or("node", String::as_str);
            format!("node:{}", name)
        } else {
            "unassigned".to_string()
        };

        task_views.push(TaskView {
            id: t.id,
            title: t.title,
            status: t.status,
            priority: t.priority,
            bot_label,
            created_ago: time_ago(t.created_at),
            thread_id: t.thread_id,
        });
    }

    if wants_html(&headers) {
        let html = render(TasksTemplate {
            active_tab: "yours",
            workspace: ws,
            tasks: task_views,
            filter_status: status_filter,
        })?;
        Ok(html.into_response())
    } else {
        Ok(Json(json!({
            "workspace_id": ws.id,
            "tasks": task_views.iter().map(|tv| json!({
                "id": tv.id,
                "title": tv.title,
                "status": tv.status,
                "priority": tv.priority,
                "bot": tv.bot_label,
                "created_ago": tv.created_ago,
                "thread_id": tv.thread_id
            })).collect::<Vec<_>>()
        }))
        .into_response())
    }
}

#[derive(Deserialize)]
pub struct CreateTaskPayload {
    pub title: String,
    pub workspace_id: Option<Uuid>,
    pub group_id: Option<Uuid>,
    pub thread_id: Option<Uuid>,
    pub priority: Option<String>,
    pub payload: Option<serde_json::Value>,
    pub idempotency_key: Option<String>,
    /// `agent` (default): an agent claims it via /tasks/{id}/claim.
    /// `server`: the in-process FSM dispatcher runs it.
    pub executor: Option<String>,
}

/// POST /tasks — Create a new task (with optional auto-created dedicated thread)
pub async fn create_task(
    State(state): State<AppState>,
    caller: Caller,
    headers: axum::http::HeaderMap,
    JsonOrForm(body): JsonOrForm<CreateTaskPayload>,
) -> Result<Response, AppError> {
    let title = body.title.trim();
    if title.is_empty() {
        return Err(AppError::BadRequest("Title is required".to_string()));
    }

    let ws = if let Some(wid) = body.workspace_id {
        ensure_workspace_access(&state.pool, &caller, wid).await?;
        sqlx::query_as::<_, Workspace>("SELECT * FROM workspaces WHERE id = $1")
            .bind(wid)
            .fetch_one(&state.pool)
            .await?
    } else {
        get_or_create_default_workspace(&state.pool, caller.user_id()).await?
    };

    // Check idempotency key if provided
    if let Some(ref idem) = body.idempotency_key {
        let existing = sqlx::query_as::<_, Task>(
            "SELECT * FROM tasks WHERE workspace_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL",
        )
        .bind(ws.id)
        .bind(idem)
        .fetch_optional(&state.pool)
        .await?;

        if let Some(t) = existing {
            if wants_html(&headers) {
                return Ok(Redirect::to(&format!("/tasks/{}", t.id)).into_response());
            } else {
                return Ok(Json(json!({ "task": t })).into_response());
            }
        }
    }

    let executor = match body.executor.as_deref().map(str::trim) {
        None | Some("") | Some("agent") => "agent",
        Some("server") => "server",
        Some(_) => {
            return Err(AppError::BadRequest(
                "executor must be 'agent' or 'server'".to_string(),
            ))
        }
    };

    // A task may only point at a group of its own workspace.
    if let Some(gid) = body.group_id {
        let in_workspace: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM groups WHERE id = $1 AND workspace_id = $2 AND deleted_at IS NULL)",
        )
        .bind(gid)
        .bind(ws.id)
        .fetch_one(&state.pool)
        .await?;
        if !in_workspace {
            return Err(AppError::Forbidden);
        }
    }

    // Connect to or auto-create dedicated thread for the task. An existing
    // thread must belong to the caller (or to this workspace): results and
    // dispatcher notes are written into it, and that thread feeds the
    // owner's agents through /inbox.
    let thread_id = if let Some(tid) = body.thread_id {
        let allowed: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM threads
                WHERE id = $1 AND deleted_at IS NULL
                  AND (user_id = $2 OR workspace_id = $3)
            )",
        )
        .bind(tid)
        .bind(caller.user_id())
        .bind(ws.id)
        .fetch_one(&state.pool)
        .await?;
        if !allowed {
            return Err(AppError::Forbidden);
        }
        Some(tid)
    } else {
        let slug = format!("task-{}", &Uuid::new_v4().to_string()[..8]);
        let t = sqlx::query_as::<_, Thread>(
            "INSERT INTO threads (
                id, user_id, title, slug, status, visibility, note_count, workspace_id, created_at, updated_at
            )
            VALUES (gen_random_uuid(), $1, $2, $3, 'open', 'private', 0, $4, now(), now())
            RETURNING *",
        )
        .bind(caller.user_id())
        .bind(title)
        .bind(slug)
        .bind(ws.id)
        .fetch_one(&state.pool)
        .await?;
        Some(t.id)
    };

    let priority = body.priority.unwrap_or_else(|| "normal".to_string());
    let payload = body.payload.unwrap_or_else(|| json!({}));

    let (actor_type, actor_id) = match &caller {
        Caller::User(u) => ("user", Some(u.id)),
        Caller::Agent(a) => ("agent", Some(a.id)),
    };

    let task = sqlx::query_as::<_, Task>(
        "INSERT INTO tasks (
            id, workspace_id, group_id, thread_id, title, status, priority,
            payload, created_by_actor_type, created_by_actor_id, idempotency_key,
            executor, fsm_state, created_at, updated_at
        )
        VALUES (
            gen_random_uuid(), $1, $2, $3, $4, 'queued', $5,
            $6, $7, $8, $9, $10, 'QUEUED', now(), now()
        )
        RETURNING *",
    )
    .bind(ws.id)
    .bind(body.group_id)
    .bind(thread_id)
    .bind(title)
    .bind(priority)
    .bind(payload)
    .bind(actor_type)
    .bind(actor_id)
    .bind(&body.idempotency_key)
    .bind(executor)
    .fetch_one(&state.pool)
    .await?;

    if executor == "server" {
        state.dispatcher_wake.notify_one();
    }

    // Routing memory: a job like one you sent to an agent before goes to
    // that agent again, visibly and reversibly (the audit trail says why).
    let routed = if executor == "agent" {
        apply_routing(&state.pool, ws.id, &task).await
    } else {
        None
    };
    if let Some(agent_id) = routed {
        let _ = record_audit(
            &state.pool,
            Some(ws.id),
            None,
            Some(task.id),
            None,
            "user",
            Some(caller.user_id()),
            "task.assigned",
            Some("task"),
            Some(task.id),
            json!({ "agent_id": agent_id, "reason": "routing" }),
            "ok",
        )
        .await;
    }

    let _ = record_audit(
        &state.pool,
        Some(ws.id),
        body.group_id,
        Some(task.id),
        None,
        actor_type,
        actor_id,
        "task.created",
        Some("task"),
        Some(task.id),
        json!({ "title": task.title, "priority": task.priority }),
        "ok",
    )
    .await;

    if wants_html(&headers) {
        Ok(Redirect::to(&format!("/tasks/{}", task.id)).into_response())
    } else {
        Ok((StatusCode::CREATED, Json(json!({ "task": task }))).into_response())
    }
}

/// GET /tasks/:id — View task details and progress
pub async fn task_detail(
    State(state): State<AppState>,
    caller: Caller,
    Path(task_id): Path<Uuid>,
    headers: axum::http::HeaderMap,
) -> Result<Response, AppError> {
    let task =
        sqlx::query_as::<_, Task>("SELECT * FROM tasks WHERE id = $1 AND deleted_at IS NULL")
            .bind(task_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(AppError::NotFound)?;

    ensure_workspace_access(&state.pool, &caller, task.workspace_id).await?;

    let workspace = sqlx::query_as::<_, Workspace>("SELECT * FROM workspaces WHERE id = $1")
        .bind(task.workspace_id)
        .fetch_one(&state.pool)
        .await?;

    let bot_label = if let Some(agent_id) = task.assigned_agent_id {
        sqlx::query_scalar::<_, String>(
            "SELECT name FROM agents WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(agent_id)
        .fetch_optional(&state.pool)
        .await?
        .map(|n| format!("@{}", n))
    } else if let Some(node_id) = task.assigned_node_id {
        sqlx::query_scalar::<_, String>(
            "SELECT name FROM nodes WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(node_id)
        .fetch_optional(&state.pool)
        .await?
        .map(|n| format!("node:{}", n))
    } else {
        None
    };

    let notes = if let Some(tid) = task.thread_id {
        sqlx::query_as::<_, Note>(
            "SELECT * FROM (
                SELECT * FROM notes WHERE thread_id = $1 AND deleted_at IS NULL
                ORDER BY seq DESC LIMIT 500
             ) recent ORDER BY seq ASC",
        )
        .bind(tid)
        .fetch_all(&state.pool)
        .await?
    } else {
        Vec::new()
    };

    let approval = sqlx::query_as::<_, Approval>(
        "SELECT * FROM approvals WHERE task_id = $1 AND deleted_at IS NULL ORDER BY created_at DESC LIMIT 1",
    )
    .bind(task.id)
    .fetch_optional(&state.pool)
    .await?;

    if wants_html(&headers) {
        let html = render(TaskDetailTemplate {
            active_tab: "yours",
            task,
            workspace,
            assigned_bot: bot_label,
            notes,
            approval,
        })?;
        Ok(html.into_response())
    } else {
        // Members of the workspace get the callback URL for a parked
        // external job, to hand to the provider that will report back.
        let webhook_url = if task.executor == "server" && task.fsm_state == "WAITING_IO" {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT external_job_id FROM tasks WHERE id = $1",
            )
            .bind(task.id)
            .fetch_one(&state.pool)
            .await?
            .map(|job| {
                format!(
                    "{}/api/webhooks/io/{}?token={}",
                    state.config.app_base_url,
                    task.id,
                    crate::crypto::webhook_token(task.id, &job)
                )
            })
        } else {
            None
        };
        Ok(Json(json!({
            "task": task,
            "assigned_bot": bot_label,
            "notes": notes,
            "approval": approval,
            "webhook_url": webhook_url
        }))
        .into_response())
    }
}

#[derive(Deserialize)]
pub struct ClaimTaskPayload {
    pub node_id: Option<Uuid>,
    pub idempotency_key: Option<String>,
}

/// POST /tasks/:id/claim — Atomically claims a task for a bot using SELECT FOR UPDATE SKIP LOCKED
pub async fn claim_task(
    State(state): State<AppState>,
    caller: Caller,
    Path(task_id): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<ClaimTaskPayload>,
) -> Result<Response, AppError> {
    // Authorize before claiming: a claim returns the task's context pack
    // (its payload), so only members of the task's workspace may claim it.
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT workspace_id FROM tasks WHERE id = $1 AND deleted_at IS NULL")
            .bind(task_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(AppError::NotFound)?;
    ensure_workspace_access(&state.pool, &caller, workspace_id).await?;

    if let Some(node_id) = body.node_id {
        let node_in_workspace: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM nodes WHERE id = $1 AND workspace_id = $2 AND deleted_at IS NULL)",
        )
        .bind(node_id)
        .bind(workspace_id)
        .fetch_one(&state.pool)
        .await?;
        if !node_in_workspace {
            return Err(AppError::Forbidden);
        }
    }

    let mut tx = state.pool.begin().await?;

    // Atomic claim with SELECT FOR UPDATE SKIP LOCKED. Server-executed
    // tasks belong to the in-process dispatcher and are never claimable.
    let task = sqlx::query_as::<_, Task>(
        "SELECT * FROM tasks
         WHERE id = $1 AND status = 'queued' AND executor = 'agent' AND deleted_at IS NULL
         FOR UPDATE SKIP LOCKED",
    )
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;

    let task = match task {
        Some(t) => t,
        None => {
            return Err(AppError::Conflict(
                "Task is already claimed or not in queued state".to_string(),
            ));
        }
    };

    let (claiming_agent_id, actor_type, actor_id) = match &caller {
        Caller::Agent(a) => (Some(a.id), "agent", a.id),
        Caller::User(u) => (None, "user", u.id),
    };

    // A task meant for one agent can only be claimed by that agent.
    if let Some(assigned) = task.assigned_agent_id {
        if claiming_agent_id != Some(assigned) {
            return Err(AppError::Conflict(
                "this task is assigned to another agent".to_string(),
            ));
        }
    }

    // Check idempotency
    if let Some(ref idem) = body.idempotency_key {
        let existing_lease = sqlx::query_as::<_, Lease>(
            "SELECT * FROM leases WHERE task_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL",
        )
        .bind(task.id)
        .bind(idem)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some(lease) = existing_lease {
            tx.commit().await?;
            return Ok(Json(json!({
                "status": "claimed",
                "task": task,
                "lease_id": lease.id
            }))
            .into_response());
        }
    }

    // Generate ContextPack (secrets scrubbed, docs/context-pack.md)
    let pack_payload = json!({
        "task_id": task.id,
        "title": task.title,
        "priority": task.priority,
        "payload": redact_secrets(task.payload.clone())
    });
    let pack_hash = format!(
        "{:x}",
        sha2::Sha256::digest(pack_payload.to_string().as_bytes())
    );

    let context_pack = sqlx::query_as::<_, ContextPack>(
        "INSERT INTO context_packs (
            id, workspace_id, task_id, agent_id, hash, payload, tool_allowlist, created_at, updated_at
        )
        VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, now(), now())
        RETURNING *",
    )
    .bind(task.workspace_id)
    .bind(task.id)
    .bind(claiming_agent_id)
    .bind(&pack_hash)
    .bind(&pack_payload)
    .bind(json!(["read_context", "post_note", "submit_result"]))
    .fetch_one(&mut *tx)
    .await?;

    // Create active internal Lease (e.g. 5 minutes budget)
    let lease = sqlx::query_as::<_, Lease>(
        "INSERT INTO leases (
            id, workspace_id, task_id, holder_actor_type, holder_actor_id,
            node_id, starts_at, expires_at, budget, permissions, safety_class, status, idempotency_key, created_at, updated_at
        )
        VALUES (
            gen_random_uuid(), $1, $2, $3, $4,
            $5, now(), now() + interval '30 minutes', $6, $7, 'standard', 'active', $8, now(), now()
        )
        RETURNING *",
    )
    .bind(task.workspace_id)
    .bind(task.id)
    .bind(actor_type)
    .bind(actor_id)
    .bind(body.node_id)
    .bind(json!({ "max_seconds": 1800 }))
    .bind(json!({ "allowed_tools": ["read_context", "post_note", "submit_result"] }))
    .bind(&body.idempotency_key)
    .fetch_one(&mut *tx)
    .await?;

    // Update task status to 'assigned'
    let updated_task = sqlx::query_as::<_, Task>(
        "UPDATE tasks
         SET status = 'assigned', fsm_state = 'CLAIMED',
             assigned_agent_id = $2, assigned_node_id = $3, updated_at = now()
         WHERE id = $1
         RETURNING *",
    )
    .bind(task.id)
    .bind(claiming_agent_id)
    .bind(body.node_id)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    let _ = record_audit(
        &state.pool,
        Some(task.workspace_id),
        task.group_id,
        Some(task.id),
        None,
        actor_type,
        Some(actor_id),
        "task.claimed",
        Some("task"),
        Some(task.id),
        json!({ "lease_id": lease.id, "context_pack_hash": pack_hash }),
        "ok",
    )
    .await;

    Ok(Json(json!({
        "status": "claimed",
        "task": updated_task,
        "lease_id": lease.id,
        "context_pack": context_pack
    }))
    .into_response())
}

#[derive(Deserialize)]
pub struct TaskResultPayload {
    pub status: String, // "completed" or "failed"
    pub output: Option<serde_json::Value>,
    pub notes: Option<String>,
    pub idempotency_key: Option<String>,
}

/// POST /tasks/:id/result — Post task execution result (releases lease, appends thread notes)
pub async fn post_task_result(
    State(state): State<AppState>,
    caller: Caller,
    Path(task_id): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<TaskResultPayload>,
) -> Result<Response, AppError> {
    let task =
        sqlx::query_as::<_, Task>("SELECT * FROM tasks WHERE id = $1 AND deleted_at IS NULL")
            .bind(task_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(AppError::NotFound)?;

    ensure_workspace_access(&state.pool, &caller, task.workspace_id).await?;

    // Check idempotency if key provided
    if let Some(ref idem) = body.idempotency_key {
        let existing = sqlx::query_as::<_, AuditEvent>(
            "SELECT * FROM audit_events WHERE task_id = $1 AND payload->>'idempotency_key' = $2 AND deleted_at IS NULL",
        )
        .bind(task.id)
        .bind(idem)
        .fetch_optional(&state.pool)
        .await?;

        if existing.is_some() {
            return Ok(Json(json!({ "status": "ok", "task": task })).into_response());
        }
    }

    if matches!(task.status.as_str(), "completed" | "failed" | "cancelled") {
        return Err(AppError::Conflict(
            "task already has a final result".to_string(),
        ));
    }

    // Release any active lease for this task
    let _ = sqlx::query(
        "UPDATE leases SET status = 'completed', updated_at = now()
         WHERE task_id = $1 AND status = 'active'",
    )
    .bind(task.id)
    .execute(&state.pool)
    .await?;

    let new_status = if body.status == "failed" {
        "failed"
    } else {
        "completed"
    };

    let updated_task = sqlx::query_as::<_, Task>(
        "UPDATE tasks
         SET status = $2, fsm_state = $4, payload = payload || $3, updated_at = now()
         WHERE id = $1 AND status NOT IN ('completed', 'failed', 'cancelled')
         RETURNING *",
    )
    .bind(task.id)
    .bind(new_status)
    .bind(json!({ "output": body.output }))
    .bind(if new_status == "completed" {
        "COMPLETED"
    } else {
        "FAILED"
    })
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::Conflict("task already has a final result".to_string()))?;

    // A desktop chat reply: post the answer in the thread as the agent the
    // person was talking to, or say plainly why there is none.
    let is_chat_reply = task.payload.get("kind").and_then(|k| k.as_str()) == Some("chat_reply");
    if is_chat_reply {
        let output = body.output.clone().unwrap_or(serde_json::Value::Null);
        deliver_chat_reply(&state, &caller, &task, new_status, &output).await;
    }

    // Append status note to linked thread if present
    if let (false, Some(tid), Some(note_body)) = (is_chat_reply, task.thread_id, body.notes) {
        if !note_body.trim().is_empty() {
            let (author_agent, author_user) = match &caller {
                Caller::Agent(a) => (Some(a.id), None),
                Caller::User(u) => (None, Some(u.id)),
            };
            let _ = sqlx::query(
                "INSERT INTO notes (id, thread_id, author_agent_id, author_user_id, body, kind, created_at, updated_at)
                 VALUES (gen_random_uuid(), $1, $2, $3, $4, 'status', now(), now())",
            )
            .bind(tid)
            .bind(author_agent)
            .bind(author_user)
            .bind(note_body)
            .execute(&state.pool)
            .await;

            let _ = sqlx::query(
                "UPDATE threads SET note_count = note_count + 1, last_note_at = now(), updated_at = now() WHERE id = $1",
            )
            .bind(tid)
            .execute(&state.pool)
            .await;
        }
    }

    let (actor_type, actor_id) = match &caller {
        Caller::User(u) => ("user", u.id),
        Caller::Agent(a) => ("agent", a.id),
    };

    let _ = record_audit(
        &state.pool,
        Some(task.workspace_id),
        task.group_id,
        Some(task.id),
        None,
        actor_type,
        Some(actor_id),
        if new_status == "completed" {
            "task.completed"
        } else {
            "task.failed"
        },
        Some("task"),
        Some(task.id),
        json!({ "status": new_status, "output": body.output, "idempotency_key": body.idempotency_key }),
        "ok",
    )
    .await;

    Ok(Json(json!({ "status": "ok", "task": updated_task })).into_response())
}

// ---------- Bots & Nodes ----------

/// GET /bots — List connected bots, agents, and nodes
#[derive(Deserialize)]
pub struct AssignTaskPayload {
    pub agent_id: Uuid,
    pub idempotency_key: Option<String>,
}

/// POST /tasks/:id/assign — A human puts one of their own agents onto a
/// queued task. Agents claim work themselves via /tasks/:id/claim; this is
/// the human-side shortcut that makes the Bots page action-first. A retry
/// that assigns the same agent again is a no-op success, not a 409.
pub async fn assign_task(
    State(state): State<AppState>,
    caller: Caller,
    Path(task_id): Path<Uuid>,
    headers: axum::http::HeaderMap,
    JsonOrForm(body): JsonOrForm<AssignTaskPayload>,
) -> Result<Response, AppError> {
    let ws = get_or_create_default_workspace(&state.pool, caller.user_id()).await?;

    let agent = sqlx::query_as::<_, crate::models::Agent>(
        "SELECT * FROM agents
         WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL AND deleted_at IS NULL",
    )
    .bind(body.agent_id)
    .bind(caller.user_id())
    .fetch_optional(&state.pool)
    .await?;
    let agent = agent.ok_or_else(|| AppError::BadRequest("Unknown bot".to_string()))?;

    let assigned = sqlx::query_as::<_, Task>(
        "UPDATE tasks SET assigned_agent_id = $1, status = 'assigned', fsm_state = 'CLAIMED', updated_at = now()
         WHERE id = $2 AND workspace_id = $3 AND status = 'queued' AND executor = 'agent' AND deleted_at IS NULL
         RETURNING *",
    )
    .bind(agent.id)
    .bind(task_id)
    .bind(ws.id)
    .fetch_optional(&state.pool)
    .await?;

    let task = match assigned {
        Some(t) => t,
        None => {
            // Idempotent by outcome: re-assigning the same bot is a no-op.
            let already = sqlx::query_as::<_, Task>(
                "SELECT * FROM tasks
                 WHERE id = $1 AND workspace_id = $2 AND assigned_agent_id = $3 AND deleted_at IS NULL",
            )
            .bind(task_id)
            .bind(ws.id)
            .bind(agent.id)
            .fetch_optional(&state.pool)
            .await?;
            match already {
                Some(t) => t,
                None => {
                    return Err(AppError::Conflict(
                        "Task is already claimed or not in queued state".to_string(),
                    ));
                }
            }
        }
    };

    remember_routing(&state.pool, ws.id, &task.title, agent.id).await;
    let _ = record_audit(
        &state.pool,
        Some(ws.id),
        None,
        Some(task.id),
        None,
        "user",
        Some(caller.user_id()),
        "task.assigned",
        Some("task"),
        Some(task.id),
        json!({ "agent_id": agent.id, "agent_name": agent.name, "idempotency_key": body.idempotency_key }),
        "ok",
    )
    .await;

    if wants_html(&headers) {
        Ok(Redirect::to("/bots").into_response())
    } else {
        Ok(
            Json(json!({ "status": "assigned", "task": task, "agent": agent.name }))
                .into_response(),
        )
    }
}

#[derive(Deserialize)]
pub struct RegisterNodePayload {
    pub name: String,
    pub node_type: Option<String>,
    pub protocol: Option<String>,
    pub capabilities: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
    pub workspace_id: Option<Uuid>,
}

/// POST /nodes — Register or pair a bot / node
pub async fn register_node(
    State(state): State<AppState>,
    caller: Caller,
    headers: axum::http::HeaderMap,
    JsonOrForm(body): JsonOrForm<RegisterNodePayload>,
) -> Result<Response, AppError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("Node name is required".to_string()));
    }

    let ws = if let Some(wid) = body.workspace_id {
        ensure_workspace_access(&state.pool, &caller, wid).await?;
        sqlx::query_as::<_, Workspace>("SELECT * FROM workspaces WHERE id = $1")
            .bind(wid)
            .fetch_one(&state.pool)
            .await?
    } else {
        get_or_create_default_workspace(&state.pool, caller.user_id()).await?
    };

    let node_handle = unique_handle(&state.pool, name).await;

    let node = sqlx::query_as::<_, Node>(
        "INSERT INTO nodes (
            id, workspace_id, name, handle, node_type, protocol, status,
            owner_type, owner_id, capabilities, metadata, last_seen_at, created_at, updated_at
        )
        VALUES (
            gen_random_uuid(), $1, $2, $3, $4, $5, 'idle',
            'user', $6, $7, $8, now(), now(), now()
        )
        RETURNING *",
    )
    .bind(ws.id)
    .bind(name)
    .bind(&node_handle)
    .bind(body.node_type.unwrap_or_else(|| "agent".to_string()))
    .bind(body.protocol.unwrap_or_else(|| "http".to_string()))
    .bind(caller.user_id())
    .bind(body.capabilities.unwrap_or_else(|| json!([])))
    .bind(body.metadata.unwrap_or_else(|| json!({})))
    .fetch_one(&state.pool)
    .await?;

    let _ = record_audit(
        &state.pool,
        Some(ws.id),
        None,
        None,
        None,
        "user",
        Some(caller.user_id()),
        "node.registered",
        Some("node"),
        Some(node.id),
        json!({ "name": node.name, "type": node.node_type }),
        "ok",
    )
    .await;

    if wants_html(&headers) {
        Ok(Redirect::to("/bots").into_response())
    } else {
        Ok((StatusCode::CREATED, Json(json!({ "node": node }))).into_response())
    }
}

// ---------- Approvals ----------

/// An approval with its task's title and room, and that room's newest
/// message.
#[derive(sqlx::FromRow)]
struct ApprovalRow {
    #[sqlx(flatten)]
    approval: Approval,
    task_title: Option<String>,
    task_thread_id: Option<Uuid>,
    last_message: Option<String>,
}

/// GET /approvals — List pending approval items ("Needs your OK")
pub async fn list_approvals(
    State(state): State<AppState>,
    caller: Caller,
    headers: axum::http::HeaderMap,
) -> Result<Response, AppError> {
    let ws = get_or_create_default_workspace(&state.pool, caller.user_id()).await?;

    // Every office the caller can see: their own and the ones shared with
    // them. Viewers see approvals but can't decide them.
    // The task's title and the room's newest message come with each
    // approval in this one query (two queries per card before).
    let raw_approvals = sqlx::query_as::<_, ApprovalRow>(
        "SELECT a.*, t.title AS task_title, t.thread_id AS task_thread_id,
                last.body AS last_message
         FROM approvals a
         JOIN workspaces w ON w.id = a.workspace_id AND w.deleted_at IS NULL
         LEFT JOIN tasks t ON t.id = a.task_id AND t.deleted_at IS NULL
         LEFT JOIN LATERAL (
             SELECT n.body FROM notes n
             WHERE n.thread_id = t.thread_id AND n.deleted_at IS NULL
             ORDER BY n.created_at DESC
             LIMIT 1
         ) last ON TRUE
         WHERE a.status = 'pending' AND a.deleted_at IS NULL
           AND (w.owner_user_id = $1 OR EXISTS (
                SELECT 1 FROM workspace_members m
                WHERE m.workspace_id = w.id AND m.user_id = $1 AND m.deleted_at IS NULL))
         ORDER BY a.created_at DESC
         LIMIT 200",
    )
    .bind(caller.user_id())
    .fetch_all(&state.pool)
    .await?;
    let mut roles: std::collections::HashMap<Uuid, crate::office::team::Role> =
        std::collections::HashMap::new();
    for row in &raw_approvals {
        let office = row.approval.workspace_id;
        if let std::collections::hash_map::Entry::Vacant(slot) = roles.entry(office) {
            if let Some(role) =
                crate::office::team::role_in(&state.pool, caller.user_id(), office).await?
            {
                slot.insert(role);
            }
        }
    }

    let mut approval_views = Vec::new();
    for row in raw_approvals {
        let a = row.approval;
        let task_title = row.task_title.unwrap_or_else(|| "Task".to_string());
        let thread_id = row.task_thread_id;

        // The card must say who did the work and show the real message, so
        // the human decides on the thing itself, not a job title.
        let message_body = row.last_message.unwrap_or_else(|| task_title.clone());
        let bot_label = {
            let cleaned = a
                .requested_by
                .trim()
                .trim_start_matches("agent:")
                .replace('_', " ");
            if cleaned.is_empty() {
                "Your bot".to_string()
            } else {
                cleaned
            }
        };

        approval_views.push(ApprovalView {
            id: a.id,
            workspace_id: a.workspace_id,
            can_decide: roles.get(&a.workspace_id).is_some_and(|r| r.can_act()),
            task_id: a.task_id,
            thread_id,
            task_title,
            requested_action: a.requested_action,
            risk_level: a.risk_level,
            status: a.status,
            requested_by: a.requested_by,
            bot_label,
            message_body,
            created_ago: time_ago(a.created_at),
            created_at: a.created_at,
            reason: a.reason,
        });
    }

    // The two numbers the decision page is judged by: how much work never
    // reached the human, and how much is still moving without them.
    let handled_without_you = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM tasks t
         WHERE t.workspace_id = $1 AND t.status = 'completed' AND t.deleted_at IS NULL
           AND t.updated_at >= date_trunc('day', now())
           AND NOT EXISTS (SELECT 1 FROM approvals a WHERE a.task_id = t.id AND a.deleted_at IS NULL)",
    )
    .bind(ws.id)
    .fetch_one(&state.pool)
    .await?;
    let running_now = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM tasks
         WHERE workspace_id = $1 AND status IN ('assigned', 'running') AND deleted_at IS NULL",
    )
    .bind(ws.id)
    .fetch_one(&state.pool)
    .await?;

    // Choices for "give it to a different bot": the user's own agents.
    let agent_rows = sqlx::query_as::<_, crate::models::Agent>(
        "SELECT * FROM agents WHERE user_id = $1 AND revoked_at IS NULL AND deleted_at IS NULL
         ORDER BY created_at DESC",
    )
    .bind(caller.user_id())
    .fetch_all(&state.pool)
    .await?;
    let bot_choices = agent_rows
        .into_iter()
        .map(|a| AgentChoice {
            id: a.id,
            name: a.name,
            description: "Your own agent — takes jobs and reports back.".to_string(),
        })
        .collect();

    if wants_html(&headers) {
        let html = render(ApprovalsTemplate {
            active_tab: "approvals",
            approvals: approval_views,
            bots: bot_choices,
            handled_without_you,
            running_now,
        })?;
        Ok(html.into_response())
    } else {
        Ok(Json(json!({
            "workspace": ws,
            "approvals": approval_views.iter().map(|av| json!({
                "id": av.id,
                "workspace_id": av.workspace_id,
                "can_decide": av.can_decide,
                "task_id": av.task_id,
                "task_title": av.task_title,
                "requested_action": av.requested_action,
                "risk_level": av.risk_level,
                "status": av.status,
                "requested_by": av.requested_by,
                "created_ago": av.created_ago,
                // The names the app's approval cards read.
                "action_summary": av.requested_action,
                "agent_name": av.bot_label,
                "reason": av.reason,
                "created_at": av.created_at,
            })).collect::<Vec<_>>()
        }))
        .into_response())
    }
}

#[derive(Deserialize)]
pub struct DecideApprovalPayload {
    pub decision: String, // "approve" or "reject"
    pub reason: Option<String>,
    pub idempotency_key: Option<String>,
    /// For a bot's proposed change to an app record: the field values the
    /// owner edited, approved in place of the bot's.
    #[serde(default)]
    pub changes: Option<serde_json::Value>,
}

/// POST /approvals/:id/decide — Human approves or rejects a pending action
pub async fn decide_approval(
    State(state): State<AppState>,
    caller: Caller,
    Path(approval_id): Path<Uuid>,
    headers: axum::http::HeaderMap,
    JsonOrForm(body): JsonOrForm<DecideApprovalPayload>,
) -> Result<Response, AppError> {
    let approval = sqlx::query_as::<_, Approval>(
        "SELECT * FROM approvals WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(approval_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    // Members decide approvals too; viewers only see them.
    ensure_workspace_role(
        &state.pool,
        &caller,
        approval.workspace_id,
        crate::office::team::Role::Member,
    )
    .await?;

    // Idempotency: if already decided, return current state without re-deciding or duplicating audit
    if approval.status != "pending" {
        if wants_html(&headers) {
            return Ok(Redirect::to("/approvals").into_response());
        } else {
            return Ok(Json(json!({ "status": "ok", "approval": approval })).into_response());
        }
    }

    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let is_approve = match body.decision.trim().to_ascii_lowercase().as_str() {
        "approve" | "approved" => true,
        "reject" | "rejected" | "deny" | "denied" => false,
        _ => {
            return Err(AppError::BadRequest(
                "decision must be 'approve' or 'reject'".to_string(),
            ))
        }
    };
    let new_status = if is_approve { "approved" } else { "rejected" };
    let edited = match &body.changes {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Object(map)) if !map.is_empty() && map.len() <= 30 => {
            Some(map.clone())
        }
        Some(_) => {
            return Err(AppError::BadRequest(
                "changes must be an object of at most 30 fields".to_string(),
            ))
        }
    };

    // Atomic: only a still-pending approval can be decided, so two
    // simultaneous decisions cannot both apply.
    let Some(updated) = sqlx::query_as::<_, Approval>(
        "UPDATE approvals
         SET status = $2, decided_by = $3, decided_at = now(), reason = $4, updated_at = now()
         WHERE id = $1 AND status = 'pending'
         RETURNING *",
    )
    .bind(approval.id)
    .bind(new_status)
    .bind(caller.user_id())
    .bind(&body.reason)
    .fetch_optional(&state.pool)
    .await?
    else {
        let current = sqlx::query_as::<_, Approval>("SELECT * FROM approvals WHERE id = $1")
            .bind(approval.id)
            .fetch_one(&state.pool)
            .await?;
        if wants_html(&headers) {
            return Ok(Redirect::to("/approvals").into_response());
        }
        return Ok(Json(json!({ "status": "ok", "approval": current })).into_response());
    };

    // A bot's proposed change to an app record: applied (with the owner's
    // edits) or dropped, and its job ends here.
    let record_change: Option<Uuid> =
        sqlx::query_scalar("SELECT record_id FROM approvals WHERE id = $1")
            .bind(approval.id)
            .fetch_one(&state.pool)
            .await?;
    if record_change.is_some() {
        crate::office::records::finish_record_change(
            &state,
            approval.id,
            is_approve,
            edited.as_ref(),
        )
        .await?;
    } else if is_approve {
        // Approved: re-queue, and record the approval on the task so the
        // dispatcher does not ask for the same approval again.
        let _ = sqlx::query(
            "UPDATE tasks
             SET status = 'queued', fsm_state = 'QUEUED',
                 state_payload = state_payload || jsonb_build_object('approved_by', $2::text, 'approval_id', $3::text),
                 updated_at = now()
             WHERE id = $1 AND (status = 'waiting_approval' OR fsm_state = 'WAITING_APPROVAL')",
        )
        .bind(approval.task_id)
        .bind(caller.user_id().to_string())
        .bind(approval.id.to_string())
        .execute(&state.pool)
        .await;
        state.dispatcher_wake.notify_one();
    } else {
        let _ = sqlx::query(
            "UPDATE tasks SET status = 'failed', fsm_state = 'FAILED', updated_at = now() WHERE id = $1 AND (status = 'waiting_approval' OR fsm_state = 'WAITING_APPROVAL')",
        )
        .bind(approval.task_id)
        .execute(&state.pool)
        .await;
    }

    // Kiro Crew Pattern: Persistent Lessons Memory
    // When an action is rejected or corrected with a reason, persist the lesson into workspace_lessons
    if !is_approve
        || body
            .reason
            .as_ref()
            .map(|r| !r.trim().is_empty())
            .unwrap_or(false)
    {
        let rule_text = body
            .reason
            .clone()
            .filter(|r| !r.trim().is_empty())
            .unwrap_or_else(|| {
                format!(
                    "Do not execute action without review: {}",
                    approval.requested_action
                )
            });

        let _ = sqlx::query(
            r#"
            INSERT INTO workspace_lessons (workspace_id, rule, source, context)
            VALUES ($1, $2, 'approval_rejection', $3)
            "#,
        )
        .bind(approval.workspace_id)
        .bind(&rule_text)
        .bind(json!({
            "approval_id": approval.id,
            "task_id": approval.task_id,
            "requested_action": approval.requested_action,
            "decision": new_status,
        }))
        .execute(&state.pool)
        .await;
    }

    let _ = record_audit(
        &state.pool,
        Some(approval.workspace_id),
        None,
        Some(approval.task_id),
        approval.run_id,
        "user",
        Some(caller.user_id()),
        "approval.decided",
        Some("approval"),
        Some(approval.id),
        json!({ "decision": new_status, "reason": body.reason }),
        "ok",
    )
    .await;

    if wants_html(&headers) {
        Ok(Redirect::to("/approvals").into_response())
    } else {
        Ok(Json(json!({ "status": "ok", "approval": updated })).into_response())
    }
}

// ---------- Activity ----------

/// GET /activity — Plain-language activity trail from append-only audit_events
pub async fn list_activity(
    State(state): State<AppState>,
    caller: Caller,
    headers: axum::http::HeaderMap,
) -> Result<Response, AppError> {
    let ws = get_or_create_default_workspace(&state.pool, caller.user_id()).await?;

    let raw_events = sqlx::query_as::<_, AuditEvent>(
        "SELECT * FROM audit_events WHERE workspace_id = $1 ORDER BY created_at DESC LIMIT 50",
    )
    .bind(ws.id)
    .fetch_all(&state.pool)
    .await?;

    let mut event_views = Vec::new();
    for ev in raw_events {
        let action_text = match ev.action.as_str() {
            "task.created" => "created task",
            "task.claimed" => "bot claimed task",
            "task.completed" => "finished task successfully",
            "task.failed" => "task failed",
            "approval.decided" => "decided approval",
            "node.registered" => "registered bot",
            "workspace.created" => "created workspace",
            other => other,
        }
        .to_string();

        let target_label = if let Some(target) = ev.target_type {
            format!(
                "{}:{}",
                target,
                ev.target_id.map(|u| u.to_string()).unwrap_or_default()
            )
        } else {
            String::new()
        };

        event_views.push(AuditEventView {
            id: ev.id,
            actor_label: ev.actor_type,
            action: ev.action.clone(),
            action_text,
            target_label,
            result: ev.result,
            created_ago: time_ago(ev.created_at),
        });
    }

    if wants_html(&headers) {
        let html = render(ActivityTemplate {
            active_tab: "activity",
            events: event_views,
        })?;
        Ok(html.into_response())
    } else {
        Ok(Json(json!({
            "workspace": ws,
            "events": event_views.iter().map(|ev| json!({
                "id": ev.id,
                "actor": ev.actor_label,
                "action": ev.action,
                "action_text": ev.action_text,
                "target": ev.target_label,
                "result": ev.result,
                "created_ago": ev.created_ago
            })).collect::<Vec<_>>()
        }))
        .into_response())
    }
}

// ---------- Agent Polling ----------

/// GET /agent/tasks/poll — Endpoint for CLI/headless agents to poll for queued tasks
pub async fn agent_poll_tasks(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<serde_json::Value>, AppError> {
    let ws = get_or_create_default_workspace(&state.pool, caller.user_id()).await?;
    // A task meant for one agent (e.g. a chat reply for the person's
    // desktop bot) is offered only to that agent.
    let me: Option<Uuid> = match &caller {
        Caller::Agent(a) => Some(a.id),
        Caller::User(_) => None,
    };

    let tasks = sqlx::query_as::<_, Task>(
        "SELECT * FROM tasks
         WHERE workspace_id = $1 AND status = 'queued' AND executor = 'agent' AND deleted_at IS NULL
           AND ($2::uuid IS NULL OR assigned_agent_id IS NULL OR assigned_agent_id = $2)
         ORDER BY (priority = 'urgent') DESC, (priority = 'high') DESC, created_at ASC
         LIMIT 5",
    )
    .bind(ws.id)
    .bind(me)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(json!({
        "workspace_id": ws.id,
        "queued_tasks": tasks
    })))
}

/// Posts a desktop bot's chat answer as the agent it was written for. Only
/// the agent the task was assigned to may deliver it, and only into the
/// task's own thread, as an agent owned by the same person.
async fn deliver_chat_reply(
    state: &AppState,
    caller: &Caller,
    task: &Task,
    status: &str,
    output: &serde_json::Value,
) {
    let Caller::Agent(desktop) = caller else {
        return;
    };
    if task.assigned_agent_id != Some(desktop.id) {
        return;
    }
    let (Some(thread_id), Some(reply_as)) = (
        task.thread_id,
        task.payload
            .get("reply_as_agent_id")
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse::<Uuid>().ok()),
    ) else {
        return;
    };
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE id = $1 AND user_id = $2 AND deleted_at IS NULL)",
    )
    .bind(reply_as)
    .bind(desktop.user_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(false);
    if !owned {
        return;
    }
    let text = if status == "completed" {
        output
            .get("result")
            .and_then(|r| r.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| t.chars().take(20_000).collect::<String>())
    } else {
        None
    };
    // A failure for a request long since abandoned (the computer was off
    // for hours) would only confuse; say nothing then.
    if text.is_none() && task.created_at < chrono::Utc::now() - chrono::Duration::minutes(10) {
        return;
    }
    let body = match text {
        Some(t) => t,
        None => {
            let why = output
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("no answer came back")
                .chars()
                .take(300)
                .collect::<String>();
            format!(
                "I couldn't answer on your computer: {why}. Check that the AI tool is signed in, or turn off \"Answer my chats here\" in Devices."
            )
        }
    };
    crate::handlers::threads::insert_reply_note(
        &state.pool,
        &state.note_sender,
        thread_id,
        reply_as,
        &body,
    )
    .await;
}

#[derive(Deserialize)]
pub struct ChatRoutePayload {
    pub enabled: bool,
    pub idempotency_key: Option<String>,
}

/// POST /agent/chat-route -- a desktop bot turns "answer my chats on this
/// computer" on or off for its owner. Turning off only clears the route if
/// it points at this bot. Setting it again is harmless, so retries need no
/// replay record.
pub async fn set_chat_route(
    State(state): State<AppState>,
    caller: Caller,
    headers: axum::http::HeaderMap,
    Json(body): Json<ChatRoutePayload>,
) -> Result<Json<serde_json::Value>, AppError> {
    let Caller::Agent(agent) = caller else {
        return Err(AppError::Forbidden);
    };
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    if body.enabled {
        sqlx::query(
            "UPDATE users SET chat_desktop_agent_id = $1, updated_at = now() WHERE id = $2",
        )
        .bind(agent.id)
        .bind(agent.user_id)
        .execute(&state.pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE users SET chat_desktop_agent_id = NULL, updated_at = now()
             WHERE id = $1 AND chat_desktop_agent_id = $2",
        )
        .bind(agent.user_id)
        .bind(agent.id)
        .execute(&state.pool)
        .await?;
    }
    Ok(Json(
        json!({ "answer_chats": body.enabled, "agent_id": agent.id }),
    ))
}

// ---------- Agents: everything that works, by handle ----------

#[derive(Deserialize)]
pub struct AgentsQuery {
    pub q: Option<String>,
}

#[derive(Template)]
#[template(path = "agents_overview.html")]
pub struct AgentsTemplate {
    pub active_tab: &'static str,
    pub workspace: Workspace,
    pub cards: Vec<AgentCardView>,
    pub query: String,
}

pub struct AgentCardView {
    pub id: Uuid,
    pub handle: String,
    pub name: String,
    pub kind_label: String,
    pub description: String,
    pub status_line: String,
    pub status_dot_class: String,
    pub is_agent: bool,
}

/// GET /agents (alias /bots) -- one URL for people and machines: the
/// Agents page for browsers, the connected-agents JSON agents consume.
pub async fn agents_overview(
    State(state): State<AppState>,
    caller: Caller,
    headers: axum::http::HeaderMap,
    Query(q): Query<AgentsQuery>,
) -> Result<Response, AppError> {
    if !wants_html(&headers) {
        return crate::handlers::workflows::list_connected_agents(State(state), caller, headers)
            .await;
    }
    let ws = get_or_create_default_workspace(&state.pool, caller.user_id()).await?;
    let needle = q.q.unwrap_or_default().trim().to_lowercase();

    let mut cards: Vec<AgentCardView> = Vec::new();
    let agents = sqlx::query_as::<_, crate::models::Agent>(
        "SELECT * FROM agents WHERE user_id = $1 AND deleted_at IS NULL
         ORDER BY created_at DESC LIMIT 200",
    )
    .bind(caller.user_id())
    .fetch_all(&state.pool)
    .await?;
    for a in agents {
        let handle = a.handle.clone().unwrap_or_else(|| slug_handle(&a.name));
        if !needle.is_empty()
            && !handle.contains(&needle)
            && !a.name.to_lowercase().contains(&needle)
        {
            continue;
        }
        let status_line = match a.last_seen_at {
            Some(t) if (Utc::now() - t).num_seconds() < 3600 => "Ready for work.".to_string(),
            Some(_) => "Sleeping — wakes when it next checks in.".to_string(),
            None => "Ready for work.".to_string(),
        };
        cards.push(AgentCardView {
            id: a.id,
            handle,
            name: a.name,
            kind_label: "Agent".to_string(),
            description: if a.instructions.is_some() {
                "Has a character you wrote.".to_string()
            } else {
                "Your agent — give it a character to change how it talks.".to_string()
            },
            status_line: if a.revoked_at.is_some() {
                "Revoked — its key no longer works.".to_string()
            } else {
                status_line
            },
            status_dot_class: if a.revoked_at.is_some() {
                "dot-offline".to_string()
            } else {
                "dot-idle".to_string()
            },
            is_agent: true,
        });
    }
    let nodes = sqlx::query_as::<_, Node>(
        "SELECT * FROM nodes WHERE workspace_id = $1 AND deleted_at IS NULL
         ORDER BY created_at DESC LIMIT 200",
    )
    .bind(ws.id)
    .fetch_all(&state.pool)
    .await?;
    for n in nodes {
        let handle = n.handle.clone().unwrap_or_else(|| slug_handle(&n.name));
        if !needle.is_empty()
            && !handle.contains(&needle)
            && !n.name.to_lowercase().contains(&needle)
        {
            continue;
        }
        let (status_line, dot_class) = match n.status.as_str() {
            "idle" => ("Ready for work.", "dot-idle"),
            "busy" => ("Working now.", "dot-working"),
            "blocked" => ("Stuck — needs a look.", "dot-blocked"),
            _ => ("Sleeping — comes back when it's on.", "dot-offline"),
        };
        cards.push(AgentCardView {
            id: n.id,
            handle,
            name: n.name,
            kind_label: "Device".to_string(),
            description: format!("A paired {}.", n.node_type),
            status_line: status_line.to_string(),
            status_dot_class: dot_class.to_string(),
            is_agent: false,
        });
    }

    let html = render(AgentsTemplate {
        active_tab: "bots",
        workspace: ws,
        cards,
        query: needle,
    })?;
    Ok(html.into_response())
}

#[derive(Deserialize)]
pub struct CreateAgentPayload {
    pub name: String,
    pub handle: Option<String>,
    pub instructions: Option<String>,
    pub api_key: Option<String>,
    pub idempotency_key: Option<String>,
    /// Puts the new bot in this office with a room of its own (`#handle`).
    pub workspace_id: Option<Uuid>,
}

/// POST /agents -- create a software agent: a name, an @handle, and a
/// character. No device, no pairing; text it right away. The character is
/// the system prompt it answers with (the "cactus" pattern), and its AI
/// voice is the agent's own OpenRouter key or the server's default.
pub async fn create_agent(
    State(state): State<AppState>,
    caller: Caller,
    headers: axum::http::HeaderMap,
    JsonOrForm(body): JsonOrForm<CreateAgentPayload>,
) -> Result<Response, AppError> {
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(AppError::BadRequest(
            "agent name must be 1-80 characters".to_string(),
        ));
    }
    if let Some(ws) = body.workspace_id {
        ensure_workspace_access(&state.pool, &caller, ws).await?;
    }
    let base = match body.handle.as_deref() {
        Some(h) if !h.trim().is_empty() => slug_handle(h.trim()),
        _ => slug_handle(name),
    };
    let handle = unique_handle(&state.pool, &base).await;
    let instructions = body
        .instructions
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let api_key = match body
        .api_key
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        Some(plain) => Some(crate::crypto::encrypt(&plain).ok_or(AppError::Internal)?),
        None => None,
    };

    let agent_id = Uuid::new_v4();
    let (_raw, raw_secret, key_prefix) = crate::auth::generate_agent_key(agent_id);
    let key_hash = crate::auth::secret::hash_secret(&raw_secret).map_err(|_| AppError::Internal)?;
    sqlx::query(
        "INSERT INTO agents (
            id, user_id, name, handle, instructions, key_hash, key_prefix,
            provider_type, provider_model, provider_api_key, created_at, updated_at
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'openrouter', NULL, $8, now(), now())",
    )
    .bind(agent_id)
    .bind(caller.user_id())
    .bind(name)
    .bind(&handle)
    .bind(&instructions)
    .bind(&key_hash)
    .bind(&key_prefix)
    .bind(&api_key)
    .execute(&state.pool)
    .await?;

    let _ = record_audit(
        &state.pool,
        None,
        None,
        None,
        None,
        "user",
        Some(caller.user_id()),
        "agent.created",
        Some("agent"),
        Some(agent_id),
        json!({ "name": name, "handle": handle, "has_character": instructions.is_some() }),
        "ok",
    )
    .await;

    // In an office, the bot gets a room of its own to talk in.
    let mut room_id: Option<Uuid> = None;
    if let Some(ws) = body.workspace_id {
        let title = format!("#{handle}");
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO threads (title, slug, user_id, workspace_id, visibility, status)
             VALUES ($1, $2, $3, $4, 'private', 'open')
             RETURNING id",
        )
        .bind(&title)
        .bind(format!(
            "{handle}-{}",
            &Uuid::new_v4().simple().to_string()[..6]
        ))
        .bind(caller.user_id())
        .bind(ws)
        .fetch_one(&state.pool)
        .await?;
        sqlx::query("INSERT INTO thread_agents (thread_id, agent_id) VALUES ($1, $2)")
            .bind(id)
            .bind(agent_id)
            .execute(&state.pool)
            .await?;
        room_id = Some(id);
        // The new bot joins the office's group chat.
        crate::office::group::ensure_office_chat(&state.pool, caller.user_id(), ws).await?;
    }

    if wants_html(&headers) {
        Ok(Redirect::to("/agents").into_response())
    } else {
        Ok(Json(json!({
            "status": "created",
            "agent": { "id": agent_id, "name": name, "handle": handle },
            "room_id": room_id,
        }))
        .into_response())
    }
}

// ---------- Routing memory: Air learns where you send work ----------

fn routing_pattern(title: &str) -> String {
    let s: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect();
    // The first few words carry the kind of job ("welcome note to …");
    // that is what "a job like this" means in practice.
    s.split_whitespace().take(3).collect::<Vec<_>>().join(" ")
}

/// Record a manual assignment so a job like this one can follow it next
/// time. The rule stays visible and editable in the audit trail.
async fn remember_routing(pool: &sqlx::PgPool, ws_id: Uuid, title: &str, agent_id: Uuid) {
    let pattern = routing_pattern(title);
    if pattern.is_empty() {
        return;
    }
    let _ = sqlx::query(
        "INSERT INTO routing_preferences (id, workspace_id, task_pattern, agent_id, times_chosen, created_at, updated_at)
         VALUES (gen_random_uuid(), $1, $2, $3, 1, now(), now())
         ON CONFLICT (workspace_id, task_pattern) WHERE deleted_at IS NULL
         DO UPDATE SET times_chosen = routing_preferences.times_chosen + 1,
                       agent_id = $3, updated_at = now()",
    )
    .bind(ws_id)
    .bind(&pattern)
    .bind(agent_id)
    .execute(pool)
    .await;
}

/// Apply routing memory to a fresh job: a clear match goes to the agent
/// you chose before, and the reason is recorded on the task's audit trail.
async fn apply_routing(pool: &sqlx::PgPool, ws_id: Uuid, task: &Task) -> Option<Uuid> {
    let pattern = routing_pattern(&task.title);
    if pattern.is_empty() {
        return None;
    }
    let prefs = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT agent_id, task_pattern FROM routing_preferences
         WHERE workspace_id = $1 AND deleted_at IS NULL
         ORDER BY times_chosen DESC LIMIT 50",
    )
    .bind(ws_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    for (agent_id, pref) in prefs {
        if pattern == pref || (!pref.is_empty() && pattern.contains(&pref)) {
            let updated = sqlx::query(
                "UPDATE tasks SET assigned_agent_id = $1, status = 'assigned', updated_at = now()
                 WHERE id = $2 AND status = 'queued' AND deleted_at IS NULL",
            )
            .bind(agent_id)
            .bind(task.id)
            .execute(pool)
            .await
            .map(|r| r.rows_affected())
            .unwrap_or(0);
            if updated > 0 {
                return Some(agent_id);
            }
        }
    }
    None
}

// ---------- Local PC Bot Status & Fallback (Tauri & Web Mode) ----------

#[derive(Deserialize)]
pub struct LocalBotToggleRequest {
    pub enabled: bool,
    pub node_name: Option<String>,
    pub idempotency_key: Option<String>,
}

/// GET /api/bot/local-status — Auto-detects local Ollama and queries active PC node status
pub async fn get_local_bot_status(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<serde_json::Value>, AppError> {
    let ws = get_or_create_default_workspace(&state.pool, caller.user_id()).await?;

    let node = sqlx::query_as::<_, Node>(
        "SELECT * FROM nodes WHERE workspace_id = $1 AND deleted_at IS NULL ORDER BY last_seen_at DESC NULLS LAST LIMIT 1",
    )
    .bind(ws.id)
    .fetch_optional(&state.pool)
    .await?;

    let is_online = match &node {
        Some(n) => match n.last_seen_at {
            Some(t) => (chrono::Utc::now() - t).num_seconds() <= 25,
            None => false,
        },
        None => false,
    };

    // The probe hits the *server's* loopback, which only means "the user's
    // PC" when the server itself runs on that PC (local dev / desktop).
    let base = &state.config.app_base_url;
    let server_is_local = base.contains("://localhost") || base.contains("://127.0.0.1");
    let ollama_detected = if !server_is_local {
        false
    } else {
        match reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .build()
        {
            Ok(client) => client
                .get("http://127.0.0.1:11434/api/tags")
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false),
            Err(_) => false,
        }
    };

    Ok(Json(json!({
        "local_bot_enabled": is_online,
        "ollama_detected": ollama_detected,
        "node": node,
        "is_online": is_online
    })))
}

/// POST /api/bot/local-toggle — Toggles local bot execution mode
pub async fn set_local_bot_toggle(
    State(state): State<AppState>,
    caller: Caller,
    headers: axum::http::HeaderMap,
    Json(body): Json<LocalBotToggleRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Toggling is naturally idempotent; the key is accepted and validated.
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let ws = get_or_create_default_workspace(&state.pool, caller.user_id()).await?;
    let node_name = body.node_name.unwrap_or_else(|| "Local PC Bot".to_string());

    let node_name = node_name.trim().chars().take(80).collect::<String>();
    if node_name.is_empty() {
        return Err(AppError::BadRequest("node name is required".to_string()));
    }
    if body.enabled {
        // Update first; insert only when this workspace has no such node, so
        // repeated toggles never pile up duplicate nodes.
        let updated = sqlx::query(
            "UPDATE nodes SET last_seen_at = now(), status = 'online', updated_at = now()
             WHERE workspace_id = $1 AND name = $2 AND deleted_at IS NULL",
        )
        .bind(ws.id)
        .bind(&node_name)
        .execute(&state.pool)
        .await?
        .rows_affected();
        if updated == 0 {
            sqlx::query(
                "INSERT INTO nodes (id, workspace_id, name, node_type, protocol, status, owner_type, owner_id, last_seen_at)
                 VALUES (gen_random_uuid(), $1, $2, 'local_pc', 'ollama', 'online', 'user', $3, now())",
            )
            .bind(ws.id)
            .bind(&node_name)
            .bind(caller.user_id())
            .execute(&state.pool)
            .await?;
        }
    } else {
        sqlx::query(
            "UPDATE nodes SET status = 'offline', last_seen_at = now() - interval '1 hour', updated_at = now()
             WHERE workspace_id = $1 AND name = $2 AND deleted_at IS NULL",
        )
        .bind(ws.id)
        .bind(&node_name)
        .execute(&state.pool)
        .await?;
    }

    Ok(Json(json!({
        "status": "ok",
        "enabled": body.enabled
    })))
}
