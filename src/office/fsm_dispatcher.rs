//! In-process FSM dispatcher for tasks created with `executor = 'server'`.
//!
//! Agent-executed tasks (the default) are never touched here: agents claim
//! those through `/tasks/{id}/claim`. Earlier versions picked up every
//! queued task and marked it completed with fabricated output, racing the
//! agents that were meant to do the work.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::error::AppError;
use crate::office::budget_gate::{self, ActionProposal};
use crate::office::post_office_note;
use crate::state::AppState;

/// Fallback poll when nothing wakes the dispatcher explicitly.
const IDLE_POLL: Duration = Duration::from_secs(5);
/// Most tasks processed at once; bounds memory and provider fan-out.
const MAX_CONCURRENT_TASKS: usize = 16;
/// Run the reaper every this many loop iterations (~1 minute when idle).
const REAP_EVERY: u64 = 12;
/// A server task stuck in CLAIMED this long lost its coroutine (crash or
/// restart) and goes back to the queue.
const CLAIMED_STALE_MINUTES: i32 = 15;
/// How long an external job may stay unanswered before the task fails.
const EXTERNAL_JOB_DEADLINE_HOURS: i64 = 24;
const OFFICE_MODEL: &str = "anthropic/claude-3.5-haiku";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FsmTaskState {
    Queued,
    Claimed,
    WaitingIo,
    WaitingApproval,
    Completed,
    Failed,
}

impl FsmTaskState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "QUEUED",
            Self::Claimed => "CLAIMED",
            Self::WaitingIo => "WAITING_IO",
            Self::WaitingApproval => "WAITING_APPROVAL",
            Self::Completed => "COMPLETED",
            Self::Failed => "FAILED",
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ClaimedTaskRow {
    id: Uuid,
    workspace_id: Uuid,
    assigned_agent_id: Option<Uuid>,
    thread_id: Option<Uuid>,
    title: String,
    state_payload: serde_json::Value,
}

/// Background loop: wakes on `AppState::dispatcher_wake` (new server task,
/// approval, webhook) or every `IDLE_POLL`, and periodically reaps stuck
/// tasks and expired leases.
pub async fn run_dispatcher_loop(state: AppState) {
    info!("FSM dispatcher starting");
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_TASKS));
    let mut iteration: u64 = 0;
    loop {
        if iteration.is_multiple_of(REAP_EVERY) {
            if let Err(e) = reap_stale_work(&state.pool).await {
                error!(error = %e, "dispatcher reaper failed");
            }
        }
        iteration = iteration.wrapping_add(1);
        if let Err(e) = run_dispatcher_tick(&state, &permits).await {
            error!(error = %e, "FSM dispatcher tick failed");
        }
        tokio::select! {
            _ = state.dispatcher_wake.notified() => {}
            _ = tokio::time::sleep(IDLE_POLL) => {}
        }
    }
}

/// One dispatcher pass that waits for every task it started. Used by tests
/// and anything that needs the pass to be finished when it returns.
pub async fn run_dispatcher_once(state: &AppState) -> Result<usize, sqlx::Error> {
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_TASKS));
    let handles = run_dispatcher_tick(state, &permits).await?;
    let count = handles.len();
    for handle in handles {
        let _ = handle.await;
    }
    Ok(count)
}

/// Claims ready server tasks with `FOR UPDATE SKIP LOCKED`, marks them
/// CLAIMED, commits, then processes each in its own coroutine.
async fn run_dispatcher_tick(
    state: &AppState,
    permits: &Arc<Semaphore>,
) -> Result<Vec<JoinHandle<()>>, sqlx::Error> {
    let capacity = permits.available_permits();
    if capacity == 0 {
        return Ok(Vec::new());
    }

    let mut tx = state.pool.begin().await?;
    let tasks: Vec<ClaimedTaskRow> = sqlx::query_as(
        r#"
        SELECT id, workspace_id, assigned_agent_id, thread_id, title, state_payload
        FROM tasks
        WHERE executor = 'server' AND deleted_at IS NULL
          AND (fsm_state = 'QUEUED'
               OR (fsm_state = 'WAITING_IO' AND (wake_at IS NULL OR wake_at <= now())))
          -- An office at its monthly spending cap starts no task until the
          -- cap is raised or the month turns over.
          AND NOT EXISTS (
              SELECT 1 FROM workspaces w
              WHERE w.id = tasks.workspace_id
                AND w.cap_notice_month = date_trunc('month', now())::date)
        ORDER BY created_at ASC
        LIMIT $1
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(capacity as i64)
    .fetch_all(&mut *tx)
    .await?;

    if tasks.is_empty() {
        tx.commit().await?;
        return Ok(Vec::new());
    }

    let ids: Vec<Uuid> = tasks.iter().map(|t| t.id).collect();
    sqlx::query(
        "UPDATE tasks SET fsm_state = 'CLAIMED', status = 'running', updated_at = now()
         WHERE id = ANY($1)",
    )
    .bind(&ids)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let mut handles = Vec::with_capacity(tasks.len());
    for task in tasks {
        let Ok(permit) = Arc::clone(permits).acquire_owned().await else {
            break;
        };
        let state = state.clone();
        handles.push(tokio::spawn(async move {
            process_single_task(&state, task).await;
            drop(permit);
        }));
    }
    Ok(handles)
}

fn needs_approval(task: &ClaimedTaskRow) -> bool {
    let title = task.title.to_lowercase();
    task.state_payload
        .get("requires_approval")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || ["approval", "verification", "sign-off", "outbound"]
            .iter()
            .any(|word| title.contains(word))
}

fn is_approved(task: &ClaimedTaskRow) -> bool {
    task.state_payload.get("approval_id").is_some()
}

async fn set_state(pool: &PgPool, task_id: Uuid, fsm: &str, status: &str) {
    if let Err(e) = sqlx::query(
        "UPDATE tasks SET fsm_state = $2, status = $3, updated_at = now() WHERE id = $1",
    )
    .bind(task_id)
    .bind(fsm)
    .bind(status)
    .execute(pool)
    .await
    {
        error!(%task_id, error = %e, "failed to update task state");
    }
}

async fn finish(state: &AppState, task: &ClaimedTaskRow, ok: bool, result: serde_json::Value) {
    let payload = json!({
        "step": "done",
        "completed_at": Utc::now(),
        "result": result,
    });
    let (fsm, status) = if ok {
        ("COMPLETED", "completed")
    } else {
        ("FAILED", "failed")
    };
    if let Err(e) = sqlx::query(
        "UPDATE tasks
         SET fsm_state = $2, status = $3, state_payload = state_payload || $4,
             wake_at = NULL, updated_at = now()
         WHERE id = $1",
    )
    .bind(task.id)
    .bind(fsm)
    .bind(status)
    .bind(payload)
    .execute(&state.pool)
    .await
    {
        error!(task_id = %task.id, error = %e, "failed to finish task");
    }
}

async fn note(state: &AppState, task: &ClaimedTaskRow, body: &str) {
    if let Some(thread_id) = task.thread_id {
        post_office_note(state, thread_id, body).await;
    }
}

/// Holds the task for a human decision. At most one pending approval per
/// task, however often the task is re-queued.
async fn request_approval(state: &AppState, task: &ClaimedTaskRow, reason: &str) {
    let pool = &state.pool;
    let inserted = sqlx::query(
        r#"
        INSERT INTO approvals (workspace_id, task_id, requested_action, risk_level, status, requested_by)
        SELECT $1, $2, $3, 'medium', 'pending', 'office-bot'
        WHERE NOT EXISTS (
            SELECT 1 FROM approvals WHERE task_id = $2 AND status = 'pending' AND deleted_at IS NULL
        )
        "#,
    )
    .bind(task.workspace_id)
    .bind(task.id)
    .bind(&task.title)
    .execute(pool)
    .await;
    if let Err(e) = inserted {
        error!(task_id = %task.id, error = %e, "failed to create approval; re-queueing");
        retry_later(pool, task.id, 60).await;
        return;
    }
    set_state(pool, task.id, "WAITING_APPROVAL", "waiting_approval").await;
    note(
        state,
        task,
        &format!(
            "Needs your OK: '{}' ({reason}). Waiting in Approvals.",
            task.title
        ),
    )
    .await;
}

/// Parks a task to be retried after `seconds` (transient failures only).
async fn retry_later(pool: &PgPool, task_id: Uuid, seconds: i32) {
    if let Err(e) = sqlx::query(
        "UPDATE tasks
         SET fsm_state = 'WAITING_IO', status = 'queued',
             wake_at = now() + make_interval(secs => $2), updated_at = now()
         WHERE id = $1",
    )
    .bind(task_id)
    .bind(f64::from(seconds))
    .execute(pool)
    .await
    {
        error!(%task_id, error = %e, "failed to park task for retry");
    }
}

async fn process_single_task(state: &AppState, task: ClaimedTaskRow) {
    let step = task
        .state_payload
        .get("step")
        .and_then(|s| s.as_str())
        .unwrap_or("initial")
        .to_string();
    info!(task_id = %task.id, %step, "processing server task");

    match step.as_str() {
        "initial" => run_initial_step(state, &task).await,
        "waiting_external_completion" => resume_external_job(state, &task).await,
        // Already finished, or a step this version doesn't know: leave a
        // terminal state instead of inventing a result.
        "done" => set_state(&state.pool, task.id, "COMPLETED", "completed").await,
        other => {
            warn!(task_id = %task.id, step = other, "unknown task step");
            finish(state, &task, false, json!({ "error": "unknown task step" })).await;
        }
    }
}

/// Web access for one office task, from the account's daily allowance.
async fn web_for(state: &AppState, task: &ClaimedTaskRow) -> crate::web::WebAccess {
    crate::web::take_run(&state.pool, task.workspace_id, &state.config.web)
        .await
        .unwrap_or_else(|error| {
            warn!(task_id = %task.id, ?error, "web allowance unavailable; working without the web");
            crate::web::WebAccess::Off
        })
}

async fn run_initial_step(state: &AppState, task: &ClaimedTaskRow) {
    let pool = &state.pool;
    let approved = is_approved(task);
    if needs_approval(task) && !approved {
        request_approval(state, task, "marked as needing a person's OK").await;
        return;
    }

    let title = task.title.to_lowercase();
    if title.contains("video") || title.contains("wan") {
        park_for_external_job(state, task).await;
        return;
    }

    // Local mode (desktop app): the AI tool chosen on this computer does
    // the work; nothing is metered.
    if let Some(ai) = state.local_ai.clone() {
        let (mut system, prompt) = office_prompts(state, task).await;
        let web = web_for(state, task).await;
        system.push_str(web.prompt_note());
        // Office tasks belong to no bot: the computer's default tool runs them.
        let request = crate::local::LocalAiRequest {
            system,
            prompt,
            engine: None,
            model: None,
            web: web.is_on(),
        };
        match ai(request).await {
            Ok(output) => {
                finish(state, task, true, json!(output)).await;
                note(
                    state,
                    task,
                    &format!("Done: '{}'.\n\n{}", task.title, output),
                )
                .await;
            }
            Err(err) => {
                finish(state, task, false, json!({ "error": err })).await;
                note(
                    state,
                    task,
                    &format!("Couldn't finish '{}': {err}", task.title),
                )
                .await;
            }
        }
        return;
    }

    // Without a provider there is nothing that can do the work; say so
    // instead of reporting a success that never happened.
    let api_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
    if api_key.trim().is_empty() {
        finish(
            state,
            task,
            false,
            json!({ "error": "no AI provider is configured on this server" }),
        )
        .await;
        note(
            state,
            task,
            &format!(
                "Couldn't run '{}': no AI provider is configured on this server.",
                task.title
            ),
        )
        .await;
        return;
    }

    let web = web_for(state, task).await;
    let proposal = ActionProposal {
        provider: "openrouter".into(),
        model_or_tool: OFFICE_MODEL.into(),
        estimated_credits: 25
            + if web.is_on() {
                crate::web::WEB_RUN_CREDITS
            } else {
                0
            },
        raw_cost_micros: 250,
        requires_manual_approval: false,
        action_type: "execute_task_step".into(),
    };
    let spender = budget_gate::Spender::new(task.workspace_id, task.assigned_agent_id);
    let ledger_id = match budget_gate::check_and_deduct(
        pool,
        task.workspace_id,
        &proposal,
        Some(task.id),
        approved,
        spender,
    )
    .await
    {
        Err(e) => {
            // Fail closed: never run unmetered. Try again shortly.
            warn!(task_id = %task.id, error = ?e, "budget gate unavailable; re-queueing");
            retry_later(pool, task.id, 60).await;
            return;
        }
        Ok(gate)
            if matches!(
                gate.limit,
                Some(budget_gate::LimitHit::MonthlyOffice | budget_gate::LimitHit::MonthlyBot)
            ) =>
        {
            // A monthly cap: the task waits (it isn't failed) and the
            // owner is told once this month.
            if let Some(limit) = gate.limit {
                crate::office::spend::notify_cap(state, limit, spender, task.thread_id).await;
            }
            retry_later(pool, task.id, 3600).await;
            return;
        }
        Ok(gate) if !gate.passed => {
            let reason = gate.reasons.join(", ");
            finish(
                state,
                task,
                false,
                json!({ "error": "budget gate", "reasons": gate.reasons }),
            )
            .await;
            note(
                state,
                task,
                &format!("'{}' stopped: usage guard ({reason}).", task.title),
            )
            .await;
            return;
        }
        Ok(gate) if gate.requires_approval => {
            request_approval(state, task, "cost above the approval threshold").await;
            return;
        }
        Ok(gate) => gate.ledger_id,
    };

    let (mut sys, user_prompt) = office_prompts(state, task).await;
    sys.push_str(web.prompt_note());
    let limits = &state.config.web;

    match crate::openrouter::call_openrouter_web(
        &api_key,
        OFFICE_MODEL,
        &sys,
        &user_prompt,
        web.is_on().then_some(limits),
    )
    .await
    {
        Ok(completion) => {
            budget_gate::record_usage(pool, ledger_id, &completion.usage).await;
            let output = completion.text;
            finish(state, task, true, json!(output)).await;
            note(
                state,
                task,
                &format!("Done: '{}'.\n\n{}", task.title, output),
            )
            .await;
        }
        Err(e) => {
            warn!(task_id = %task.id, error = %e, "provider call failed");
            let _ = budget_gate::refund_credit_charge(
                pool,
                task.workspace_id,
                Some(task.id),
                &proposal,
                spender,
            )
            .await;
            finish(
                state,
                task,
                false,
                json!({ "error": "AI provider call failed" }),
            )
            .await;
            note(
                state,
                task,
                &format!(
                    "Couldn't finish '{}': the AI provider call failed.",
                    task.title
                ),
            )
            .await;
        }
    }
}

/// The system prompt (with the workspace's lessons) and task prompt for an
/// office task, whichever AI runs it.
async fn office_prompts(state: &AppState, task: &ClaimedTaskRow) -> (String, String) {
    let lessons: Vec<String> = sqlx::query_scalar(
        "SELECT rule FROM workspace_lessons
         WHERE workspace_id = $1 AND deleted_at IS NULL
         ORDER BY created_at DESC LIMIT 5",
    )
    .bind(task.workspace_id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    let lessons_prompt = if lessons.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n[Workspace Lessons & Rules]:\n{}",
            lessons
                .iter()
                .map(|l| format!("- {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let sys = format!(
        "You are an autonomous SMB virtual office agent executing an office task. Be concise, actionable, and professional.{lessons_prompt}"
    );
    let user_prompt = format!("Task: {}\nPayload: {}", task.title, task.state_payload);
    (sys, user_prompt)
}

/// Parks the task in WAITING_IO until the external provider calls the
/// webhook (or the deadline passes). Holds no memory while waiting.
async fn park_for_external_job(state: &AppState, task: &ClaimedTaskRow) {
    let external_job_id = format!("job_{}", Uuid::new_v4());
    let payload = json!({
        "step": "waiting_external_completion",
        "dispatched_at": Utc::now(),
    });
    let res = sqlx::query(
        r#"
        UPDATE tasks
        SET fsm_state = 'WAITING_IO', status = 'running',
            external_job_id = $1,
            state_payload = state_payload || $2,
            wake_at = now() + interval '5 minutes',
            updated_at = now()
        WHERE id = $3
        "#,
    )
    .bind(&external_job_id)
    .bind(payload)
    .bind(task.id)
    .execute(&state.pool)
    .await;
    match res {
        Ok(_) => {
            note(
                state,
                task,
                &format!(
                    "Started '{}'. Waiting for the external job to report back.",
                    task.title
                ),
            )
            .await
        }
        Err(e) => error!(task_id = %task.id, error = %e, "failed to park task"),
    }
}

/// Woken either by the webhook (result attached) or by the wake timer.
async fn resume_external_job(state: &AppState, task: &ClaimedTaskRow) {
    if let Some(data) = task.state_payload.get("webhook_data").cloned() {
        finish(state, task, true, data).await;
        note(
            state,
            task,
            &format!("'{}' finished: the external job reported back.", task.title),
        )
        .await;
        return;
    }

    let dispatched_at = task
        .state_payload
        .get("dispatched_at")
        .and_then(|v| serde_json::from_value::<chrono::DateTime<Utc>>(v.clone()).ok())
        .unwrap_or_else(Utc::now);
    if Utc::now() - dispatched_at > chrono::Duration::hours(EXTERNAL_JOB_DEADLINE_HOURS) {
        finish(
            state,
            task,
            false,
            json!({ "error": "external job timed out" }),
        )
        .await;
        note(
            state,
            task,
            &format!(
                "'{}' failed: the external job never reported back.",
                task.title
            ),
        )
        .await;
        return;
    }

    // Still waiting: park again without charging or re-dispatching.
    if let Err(e) = sqlx::query(
        "UPDATE tasks
         SET fsm_state = 'WAITING_IO', status = 'running',
             wake_at = now() + interval '5 minutes', updated_at = now()
         WHERE id = $1",
    )
    .bind(task.id)
    .execute(&state.pool)
    .await
    {
        error!(task_id = %task.id, error = %e, "failed to re-park task");
    }
}

/// Returns server tasks whose coroutine died back to the queue, and
/// expires agent leases past `expires_at` (returning still-assigned tasks
/// to the queue, per docs/leases.md).
pub async fn reap_stale_work(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE tasks SET fsm_state = 'QUEUED', status = 'queued', updated_at = now()
         WHERE executor = 'server' AND fsm_state = 'CLAIMED' AND deleted_at IS NULL
           AND updated_at < now() - make_interval(mins => $1)",
    )
    .bind(CLAIMED_STALE_MINUTES)
    .execute(pool)
    .await?;

    let mut tx = pool.begin().await?;
    let expired: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE leases SET status = 'expired', updated_at = now()
         WHERE status = 'active' AND expires_at < now() AND deleted_at IS NULL
         RETURNING task_id",
    )
    .fetch_all(&mut *tx)
    .await?;
    if !expired.is_empty() {
        sqlx::query(
            "UPDATE tasks
             SET status = 'queued', fsm_state = 'QUEUED',
                 assigned_agent_id = NULL, assigned_node_id = NULL, updated_at = now()
             WHERE id = ANY($1) AND executor = 'agent' AND status = 'assigned'
               AND NOT EXISTS (
                   SELECT 1 FROM leases l
                   WHERE l.task_id = tasks.id AND l.status = 'active' AND l.deleted_at IS NULL
               )",
        )
        .bind(&expired)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

#[derive(Deserialize)]
pub struct WebhookQuery {
    pub token: Option<String>,
    pub idempotency_key: Option<String>,
}

/// POST /api/webhooks/io/{task_id}?token=...
///
/// Called by an external provider when a parked job finishes. The token is
/// `crypto::webhook_token(task_id, external_job_id)` (shown to workspace
/// members on the task's JSON detail), so it only works for the task's
/// current job. Only a WAITING_IO server task can be resumed.
pub async fn handle_io_webhook(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
    Query(query): Query<WebhookQuery>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let token = query
        .token
        .or_else(|| {
            headers
                .get("x-airobot-webhook-token")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let idem = crate::idempotency::key(&headers, query.idempotency_key.as_deref())?;

    let row: Option<(Option<String>, String, serde_json::Value)> = sqlx::query_as(
        "SELECT external_job_id, fsm_state, state_payload FROM tasks
         WHERE id = $1 AND executor = 'server' AND deleted_at IS NULL",
    )
    .bind(task_id)
    .fetch_optional(&state.pool)
    .await?;
    // Unknown task and bad token look the same, so the endpoint can't be
    // used to probe for task ids.
    let Some((Some(job_id), fsm_state, state_payload)) = row else {
        return Err(AppError::NotFound);
    };
    if !crate::crypto::verify_webhook_token(task_id, &job_id, &token) {
        return Err(AppError::NotFound);
    }

    if fsm_state != "WAITING_IO" {
        let replay = idem.is_some()
            && state_payload
                .get("webhook_idempotency_key")
                .and_then(|v| v.as_str())
                == idem;
        if replay {
            return Ok(Json(json!({ "status": "resumed", "task_id": task_id })));
        }
        return Err(AppError::Conflict(
            "task is not waiting for this job".to_string(),
        ));
    }

    let resumed = sqlx::query(
        r#"
        UPDATE tasks
        SET fsm_state = 'QUEUED',
            state_payload = state_payload
                || jsonb_build_object('webhook_data', $1::jsonb)
                || CASE WHEN $3::text IS NULL THEN '{}'::jsonb
                        ELSE jsonb_build_object('webhook_idempotency_key', $3::text) END,
            wake_at = NULL,
            updated_at = now()
        WHERE id = $2 AND fsm_state = 'WAITING_IO' AND external_job_id = $4
        "#,
    )
    .bind(&payload)
    .bind(task_id)
    .bind(idem)
    .bind(&job_id)
    .execute(&state.pool)
    .await?
    .rows_affected();
    if resumed == 0 {
        return Err(AppError::Conflict(
            "task is not waiting for this job".to_string(),
        ));
    }

    state.dispatcher_wake.notify_one();
    Ok(Json(json!({
        "status": "resumed",
        "task_id": task_id,
        "fsm_state": "QUEUED"
    })))
}
