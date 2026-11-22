//! Goals that move: milestones, check-ins and the weekly report.
//!
//! An office has a goal (`workspaces.goal`). Milestones are measurable
//! steps toward it, each with an optional date. The owner adds them, or the
//! Architect proposes them (a `MILESTONE: ... by YYYY-MM-DD` line, for
//! example right after the owner sets a goal); a proposed milestone waits
//! for the owner to keep it, so nothing becomes a target without an OK.
//! Every office bot is shown the open milestones.
//!
//! Check-ins are opt-in (`workspaces.check_in`: off, daily, weekly or
//! monthly): the Architect reports progress toward the goal in the office
//! chat in the afternoon. The weekly report is on by default and costs no
//! AI credits: every Monday morning the office chat gets a card built from
//! the database (records added, jobs and their outcomes, spend, milestones,
//! approvals waiting), so every number in it is real.
//! Both run in the daily plan's loop, and only for offices the owner has
//! used in the last week.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::Caller;
use crate::error::AppError;
use crate::handlers::threads::{run_agent_reply, ReplyMode};
use crate::state::AppState;

/// Open milestones one office keeps (proposed and active).
const MAX_OPEN: i64 = 20;
/// Milestones a bot may propose in one message.
const MAX_PROPOSED_PER_MESSAGE: usize = 5;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Milestone {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub title: String,
    pub due_on: Option<NaiveDate>,
    pub status: String,
    pub source: String,
    pub created_by_agent_id: Option<Uuid>,
    pub done_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLUMNS: &str =
    "id, workspace_id, title, due_on, status, source, created_by_agent_id, done_at, created_at, updated_at";

fn clean_title(raw: &str) -> Result<String, AppError> {
    let title = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() || title.chars().count() > 200 {
        return Err(AppError::BadRequest(
            "a milestone is 1-200 characters".into(),
        ));
    }
    Ok(title)
}

/// A milestone's date from a request: `None` keeps it, `Some(None)`
/// clears it (an empty string).
fn due_setting(raw: Option<&str>) -> Result<Option<Option<NaiveDate>>, AppError> {
    match raw.map(str::trim) {
        None => Ok(None),
        Some("") => Ok(Some(None)),
        Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d")
            .map(|d| Some(Some(d)))
            .map_err(|_| AppError::BadRequest("a date is YYYY-MM-DD".into())),
    }
}

/// A `MILESTONE:` line: its title and optional "by YYYY-MM-DD" date.
fn parse_line(line: &str) -> Option<(String, Option<NaiveDate>)> {
    let trimmed = line
        .trim()
        .trim_start_matches(['-', '*', ' '])
        .trim_matches('*');
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("milestone:") {
        return None;
    }
    let rest = trimmed["milestone:".len()..]
        .trim()
        .trim_matches('*')
        .trim();
    let (title, due) = match rest.rsplit_once(" by ") {
        Some((t, d)) => match NaiveDate::parse_from_str(d.trim().trim_end_matches('.'), "%Y-%m-%d")
        {
            Ok(date) => (t.trim(), Some(date)),
            Err(_) => (rest, None),
        },
        None => (rest, None),
    };
    let title: String = title.trim_end_matches('.').chars().take(200).collect();
    (!title.trim().is_empty()).then(|| (title.trim().to_string(), due))
}

/// Takes `MILESTONE:` lines out of a bot's reply and keeps them as
/// proposed milestones of the room's office. Returns the reply without
/// them and the proposals (id, title) for the Keep buttons.
pub async fn apply_milestones(
    pool: &PgPool,
    thread_id: Uuid,
    agent_id: Uuid,
    text: &str,
) -> (String, Vec<(Uuid, String)>) {
    if !text.to_ascii_lowercase().contains("milestone:") {
        return (text.to_string(), Vec::new());
    }
    let Some(office) = crate::office::context::business_of_thread(pool, thread_id)
        .await
        .map(|b| b.workspace_id)
    else {
        return (text.to_string(), Vec::new());
    };
    let mut kept = Vec::new();
    let mut proposed = Vec::new();
    for line in text.lines() {
        match parse_line(line) {
            Some((title, due)) if proposed.len() < MAX_PROPOSED_PER_MESSAGE => {
                let open: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM milestones
                     WHERE workspace_id = $1 AND deleted_at IS NULL AND status <> 'done'",
                )
                .bind(office)
                .fetch_one(pool)
                .await
                .unwrap_or(MAX_OPEN);
                if open >= MAX_OPEN {
                    kept.push(line);
                    continue;
                }
                let saved: Option<Uuid> = sqlx::query_scalar(
                    "INSERT INTO milestones
                        (workspace_id, title, due_on, status, source, created_by_agent_id, thread_id)
                     VALUES ($1, $2, $3, 'proposed', 'bot', $4, $5)
                     RETURNING id",
                )
                .bind(office)
                .bind(&title)
                .bind(due)
                .bind(agent_id)
                .bind(thread_id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten();
                match saved {
                    Some(id) => {
                        let label = match due {
                            Some(d) => format!("{title} (by {d})"),
                            None => title.clone(),
                        };
                        proposed.push((id, label));
                    }
                    None => kept.push(line),
                }
            }
            _ => kept.push(line),
        }
    }
    (kept.join("\n").trim().to_string(), proposed)
}

/// The line and buttons a reply gets for the milestones it proposed.
pub fn proposed_note(proposed: &[(Uuid, String)]) -> (String, Vec<Value>) {
    let mut line = String::from("*Proposed milestones (keep the ones you want):*");
    let mut buttons = Vec::new();
    for (id, label) in proposed {
        line.push_str(&format!("\n- {label}"));
        let short: String = label.chars().take(40).collect();
        let short = if label.chars().count() > 40 {
            format!("{short}…")
        } else {
            short
        };
        buttons.push(
            json!({ "label": format!("Keep: {short}"), "kind": "keep_milestone", "target": id }),
        );
    }
    buttons.push(json!({ "label": "Review milestones", "kind": "open", "target": "goal" }));
    (line, buttons)
}

/// The milestones section of a bot's prompt; the Architect is also told how
/// to propose one.
pub async fn prompt_block(pool: &PgPool, office: Uuid, is_architect: bool) -> String {
    let rows: Vec<(String, Option<NaiveDate>, String)> = sqlx::query_as(
        "SELECT title, due_on, status FROM milestones
         WHERE workspace_id = $1 AND deleted_at IS NULL
           AND (status = 'active' OR (status = 'done' AND done_at > now() - interval '30 days'))
         ORDER BY status = 'done', due_on NULLS LAST, created_at
         LIMIT 12",
    )
    .bind(office)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let mut out = String::new();
    if !rows.is_empty() {
        out.push_str("\n\n## Milestones toward the goal\n");
        for (title, due, status) in &rows {
            let mark = if status == "done" { "[x]" } else { "[ ]" };
            match due {
                Some(d) => out.push_str(&format!("- {mark} {title} (by {d})\n")),
                None => out.push_str(&format!("- {mark} {title}\n")),
            }
        }
        out.push_str("Work toward the open ones and say when one is reached.\n");
    }
    if is_architect {
        out.push_str(
            "\nTo propose a milestone toward the goal, write it on its own line as: MILESTONE: <something measurable> by YYYY-MM-DD. The owner decides whether to keep it.\n\
             When the owner tells you something lasting about the business (what it sells and at what prices, hours, area served, who the customers are, policies, tone), save each fact on its own line as: ABOUT: <the fact>. It goes into \"About the business\", which every bot reads.\n",
        );
    }
    out
}

/// After the owner sets a new goal: the Architect proposes milestones.
pub fn propose_for_goal(
    state: &AppState,
    office_chat: Uuid,
    architect: Uuid,
    owner: Uuid,
    goal: &str,
) {
    let task = format!(
        "The owner just set this office's goal: \"{goal}\". Propose 3 measurable milestones toward it, each on its own line as 'MILESTONE: <milestone> by YYYY-MM-DD' with realistic dates from today ({}), and put one short sentence before them.",
        Utc::now().date_naive()
    );
    crate::handlers::threads::spawn_agent_reply(
        state.clone(),
        office_chat,
        architect,
        owner,
        ReplyMode::task(super::group::ARCHITECT_NAME, &task),
    );
}

// ---------------------------------------------------------------- handlers

async fn owned(pool: &PgPool, caller: &Caller, office: Uuid) -> Result<Uuid, AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };
    crate::handlers::offices::owned_office(pool, user.id, office).await?;
    Ok(user.id)
}

async fn owned_milestone(pool: &PgPool, caller: &Caller, id: Uuid) -> Result<Milestone, AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };
    sqlx::query_as::<_, Milestone>(&format!(
        "SELECT {} FROM milestones m JOIN workspaces w ON w.id = m.workspace_id
         WHERE m.id = $1 AND m.deleted_at IS NULL AND w.owner_user_id = $2 AND w.deleted_at IS NULL",
        COLUMNS
            .split(", ")
            .map(|c| format!("m.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
    .bind(id)
    .bind(user.id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)
}

/// GET /api/offices/:id/goal -- the goal, its milestones, and the check-in
/// and weekly report settings.
pub async fn get_goal(
    State(state): State<AppState>,
    caller: Caller,
    Path(office): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    owned(&state.pool, &caller, office).await?;
    let (goal, check_in, weekly_report): (Option<String>, String, bool) =
        sqlx::query_as("SELECT goal, check_in, weekly_report FROM workspaces WHERE id = $1")
            .bind(office)
            .fetch_one(&state.pool)
            .await?;
    let milestones = sqlx::query_as::<_, Milestone>(&format!(
        "SELECT {COLUMNS} FROM milestones
         WHERE workspace_id = $1 AND deleted_at IS NULL
         ORDER BY CASE status WHEN 'proposed' THEN 0 WHEN 'active' THEN 1 ELSE 2 END,
                  due_on NULLS LAST, created_at
         LIMIT 100"
    ))
    .bind(office)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!({
        "goal": goal,
        "milestones": milestones,
        "check_in": check_in,
        "weekly_report": weekly_report,
        "limit": MAX_OPEN,
    })))
}

#[derive(Debug, Deserialize)]
pub struct CreateMilestoneRequest {
    pub title: String,
    #[serde(default)]
    pub due_on: Option<String>,
    pub idempotency_key: Option<String>,
}

/// POST /api/offices/:id/milestones
pub async fn create_milestone(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
    Json(body): Json<CreateMilestoneRequest>,
) -> Result<Json<Milestone>, AppError> {
    let key = crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    owned(&state.pool, &caller, office).await?;
    let title = clean_title(&body.title)?;
    let due = due_setting(body.due_on.as_deref())?.flatten();
    if let Some(key) = key {
        if let Some(existing) = sqlx::query_as::<_, Milestone>(&format!(
            "SELECT {COLUMNS} FROM milestones
             WHERE workspace_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL"
        ))
        .bind(office)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?
        {
            return Ok(Json(existing));
        }
    }
    let open: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM milestones
         WHERE workspace_id = $1 AND deleted_at IS NULL AND status <> 'done'",
    )
    .bind(office)
    .fetch_one(&state.pool)
    .await?;
    if open >= MAX_OPEN {
        return Err(AppError::BadRequest(format!(
            "an office has at most {MAX_OPEN} open milestones; finish or delete one first"
        )));
    }
    let saved = sqlx::query_as::<_, Milestone>(&format!(
        "INSERT INTO milestones (workspace_id, title, due_on, idempotency_key)
         VALUES ($1, $2, $3, $4)
         RETURNING {COLUMNS}"
    ))
    .bind(office)
    .bind(&title)
    .bind(due)
    .bind(key)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(saved))
}

#[derive(Debug, Deserialize)]
pub struct UpdateMilestoneRequest {
    #[serde(default)]
    pub title: Option<String>,
    /// YYYY-MM-DD, or empty to clear.
    #[serde(default)]
    pub due_on: Option<String>,
    /// `active` (keep a proposal, or reopen) or `done`.
    #[serde(default)]
    pub status: Option<String>,
    pub idempotency_key: Option<String>,
}

/// PATCH /api/milestones/:id
pub async fn update_milestone(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateMilestoneRequest>,
) -> Result<Json<Milestone>, AppError> {
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    owned_milestone(&state.pool, &caller, id).await?;
    let title = body.title.as_deref().map(clean_title).transpose()?;
    let due = due_setting(body.due_on.as_deref())?;
    if let Some(status) = body.status.as_deref() {
        if !matches!(status, "active" | "done") {
            return Err(AppError::BadRequest(
                "a milestone's status is active or done".into(),
            ));
        }
    }
    let saved = sqlx::query_as::<_, Milestone>(&format!(
        "UPDATE milestones SET
            title = COALESCE($2, title),
            due_on = CASE WHEN $3 THEN $4 ELSE due_on END,
            status = COALESCE($5, status),
            done_at = CASE WHEN $5 = 'done' THEN COALESCE(done_at, now())
                           WHEN $5 = 'active' THEN NULL ELSE done_at END,
            updated_at = now()
         WHERE id = $1
         RETURNING {COLUMNS}"
    ))
    .bind(id)
    .bind(title)
    .bind(due.is_some())
    .bind(due.flatten())
    .bind(body.status.as_deref())
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(saved))
}

#[derive(Debug, Default, Deserialize)]
pub struct DeleteRequest {
    pub idempotency_key: Option<String>,
}

/// POST /api/milestones/:id/delete -- soft; also how a proposal is dropped.
pub async fn delete_milestone(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    body: Option<Json<DeleteRequest>>,
) -> Result<Json<Value>, AppError> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    owned_milestone(&state.pool, &caller, id).await?;
    sqlx::query("UPDATE milestones SET deleted_at = now(), updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "deleted": true })))
}

// ---------------------------------------------------------------- check-ins

/// What the Architect is asked at a check-in.
pub fn check_in_task(period: &str) -> String {
    format!(
        "Time for the {period} check-in. In a few short lines: progress toward the goal and each open milestone since the last check-in (what was done, from this office's records and messages), what is at risk, and the one thing that would help most next. Ask me one question if a decision is mine. Don't hand out tasks."
    )
}

/// The SQL condition for an office the owner has used lately (or a new one).
const ACTIVE_OWNER: &str = "(w.created_at > now() - interval '7 days'
    OR EXISTS (
        SELECT 1 FROM notes n JOIN threads t ON t.id = n.thread_id
        WHERE t.user_id = w.owner_user_id AND n.author_user_id = w.owner_user_id
          AND n.created_at > now() - interval '7 days'))";

/// The owner's local time in an office, as SQL.
const LOCAL_NOW: &str =
    "((now() AT TIME ZONE 'UTC') + make_interval(mins => w.utc_offset_minutes))";

/// Claims and runs the check-ins that are due: after 16:00 local, daily;
/// on Fridays, weekly; after four weeks, monthly.
pub async fn run_due_check_ins(state: &AppState) -> Result<usize, sqlx::Error> {
    let due: Vec<(Uuid, Uuid, Uuid, String)> = sqlx::query_as(&format!(
        "UPDATE workspaces w SET last_check_in_at = now(), updated_at = now()
         FROM threads chat
         WHERE chat.workspace_id = w.id AND chat.kind = 'office' AND chat.deleted_at IS NULL
           AND w.id IN (
             SELECT w.id FROM workspaces w
             WHERE w.deleted_at IS NULL AND w.check_in <> 'off' AND NOT w.paused
               AND w.cap_notice_month IS DISTINCT FROM date_trunc('month', now())::date
               AND EXTRACT(HOUR FROM {LOCAL_NOW}) >= 16
               AND (
                 (w.check_in = 'daily' AND (w.last_check_in_at IS NULL OR w.last_check_in_at < now() - interval '20 hours'))
                 OR (w.check_in = 'weekly' AND EXTRACT(ISODOW FROM {LOCAL_NOW}) = 5
                     AND (w.last_check_in_at IS NULL OR w.last_check_in_at < now() - interval '6 days'))
                 OR (w.check_in = 'monthly'
                     AND (w.last_check_in_at IS NULL OR w.last_check_in_at < now() - interval '28 days'))
               )
               AND {ACTIVE_OWNER}
             ORDER BY w.last_check_in_at NULLS FIRST
             LIMIT 20
             FOR UPDATE SKIP LOCKED
           )
         RETURNING w.id, w.owner_user_id, chat.id, w.check_in"
    ))
    .fetch_all(&state.pool)
    .await?;
    for (office, owner, chat, period) in &due {
        let architect = match super::group::architect_bot(&state.pool, *owner).await {
            Ok(id) => id,
            Err(error) => {
                tracing::warn!(?error, %office, "check-in: no Architect");
                continue;
            }
        };
        run_agent_reply(
            state.clone(),
            *chat,
            architect,
            *owner,
            ReplyMode::task(super::group::ARCHITECT_NAME, &check_in_task(period)),
        )
        .await;
    }
    Ok(due.len())
}

// ---------------------------------------------------------------- report

/// The weekly report of one office, from the database: the last 7 days.
pub async fn weekly_report(pool: &PgPool, office: Uuid) -> Result<String, sqlx::Error> {
    let (offset,): (i32,) =
        sqlx::query_as("SELECT utc_offset_minutes FROM workspaces WHERE id = $1")
            .bind(office)
            .fetch_one(pool)
            .await?;
    let today = (Utc::now() + chrono::Duration::minutes(i64::from(offset))).date_naive();
    let from = today - chrono::Duration::days(7);
    let to = today - chrono::Duration::days(1);
    let mut out = format!(
        "**Weekly report · {} – {}**\n",
        from.format("%b %-d"),
        to.format("%b %-d")
    );

    let records: Vec<(String, i64)> = sqlx::query_as(
        "SELECT a.title, COUNT(*) FROM office_records r
         JOIN office_apps a ON a.id = r.app_id
         WHERE r.workspace_id = $1 AND r.deleted_at IS NULL AND a.deleted_at IS NULL
           AND r.created_at > now() - interval '7 days'
         GROUP BY a.title ORDER BY COUNT(*) DESC",
    )
    .bind(office)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let added: i64 = records.iter().map(|r| r.1).sum();
    if added == 0 {
        out.push_str("\n- No records added in your apps.");
    } else {
        let parts: Vec<String> = records.iter().map(|(t, n)| format!("{t} {n}")).collect();
        out.push_str(&format!(
            "\n- {added} {} added: {}",
            if added == 1 { "record" } else { "records" },
            parts.join(", ")
        ));
    }

    let jobs: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT outcome, COUNT(*) FROM tasks
         WHERE workspace_id = $1 AND deleted_at IS NULL AND executor IN ('chat', 'server')
           AND created_at > now() - interval '7 days'
         GROUP BY outcome",
    )
    .bind(office)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let count = |o: &str| {
        jobs.iter()
            .filter(|(k, _)| k.as_deref() == Some(o))
            .map(|(_, n)| *n)
            .sum::<i64>()
    };
    let total_jobs: i64 = jobs.iter().map(|(_, n)| n).sum();
    if total_jobs == 0 {
        out.push_str("\n- No jobs for the bots.");
    } else {
        let mut parts = Vec::new();
        for (key, label) in [
            ("done", "done"),
            ("nothing_to_do", "nothing to do"),
            ("needs_ok", "need your OK"),
            ("blocked", "blocked"),
            ("failed", "failed"),
        ] {
            let n = count(key);
            if n > 0 {
                parts.push(format!("{n} {label}"));
            }
        }
        let unknown = total_jobs - parts_total(&jobs);
        if unknown > 0 {
            parts.push(format!("{unknown} without an outcome"));
        }
        out.push_str(&format!("\n- Jobs: {}", parts.join(", ")));
    }

    let spent: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(credits_charged), 0)::bigint FROM credit_ledger
         WHERE office_id = $1 AND created_at > now() - interval '7 days'",
    )
    .bind(office)
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    if spent > 0 {
        out.push_str(&format!("\n- {spent} AI credits spent"));
    }

    let done: Vec<String> = sqlx::query_scalar(
        "SELECT title FROM milestones
         WHERE workspace_id = $1 AND deleted_at IS NULL AND status = 'done'
           AND done_at > now() - interval '7 days'
         ORDER BY done_at",
    )
    .bind(office)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let next: Option<(String, Option<NaiveDate>)> = sqlx::query_as(
        "SELECT title, due_on FROM milestones
         WHERE workspace_id = $1 AND deleted_at IS NULL AND status = 'active'
         ORDER BY due_on NULLS LAST, created_at LIMIT 1",
    )
    .bind(office)
    .fetch_optional(pool)
    .await
    .unwrap_or_default();
    if !done.is_empty() {
        out.push_str(&format!("\n- Milestones reached: {}", done.join("; ")));
    }
    if let Some((title, due)) = next {
        match due {
            Some(d) if d < today => {
                out.push_str(&format!("\n- Next milestone: {title} (was due {d})"))
            }
            Some(d) => out.push_str(&format!("\n- Next milestone: {title} (by {d})")),
            None => out.push_str(&format!("\n- Next milestone: {title}")),
        }
    }

    let waiting: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM approvals
         WHERE workspace_id = $1 AND status = 'pending' AND deleted_at IS NULL",
    )
    .bind(office)
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    if waiting > 0 {
        out.push_str(&format!(
            "\n- {waiting} {} waiting for you",
            if waiting == 1 {
                "approval"
            } else {
                "approvals"
            }
        ));
    }
    out.push_str(
        "\n\n*Counted from this office's records, jobs and spend; nothing here is estimated.*",
    );
    Ok(out)
}

fn parts_total(jobs: &[(Option<String>, i64)]) -> i64 {
    jobs.iter()
        .filter(|(k, _)| k.is_some())
        .map(|(_, n)| *n)
        .sum()
}

/// Claims and posts the weekly reports that are due: Monday from 08:00
/// local, once a week.
pub async fn run_due_reports(state: &AppState) -> Result<usize, sqlx::Error> {
    let due: Vec<(Uuid, Uuid, Uuid)> = sqlx::query_as(&format!(
        "UPDATE workspaces w SET last_report_at = now(), updated_at = now()
         FROM threads chat
         WHERE chat.workspace_id = w.id AND chat.kind = 'office' AND chat.deleted_at IS NULL
           AND w.id IN (
             SELECT w.id FROM workspaces w
             WHERE w.deleted_at IS NULL AND w.weekly_report AND NOT w.paused
               AND EXTRACT(ISODOW FROM {LOCAL_NOW}) = 1 AND EXTRACT(HOUR FROM {LOCAL_NOW}) >= 8
               AND (w.last_report_at IS NULL OR w.last_report_at < now() - interval '6 days')
               AND w.created_at < now() - interval '3 days'
               AND {ACTIVE_OWNER}
             ORDER BY w.last_report_at NULLS FIRST
             LIMIT 50
             FOR UPDATE SKIP LOCKED
           )
         RETURNING w.id, w.owner_user_id, chat.id"
    ))
    .fetch_all(&state.pool)
    .await?;
    for (office, owner, chat) in &due {
        post_report(state, *office, *owner, *chat).await;
    }
    Ok(due.len())
}

/// Posts an office's weekly report card in its group chat.
pub async fn post_report(state: &AppState, office: Uuid, owner: Uuid, chat: Uuid) {
    let body = match weekly_report(&state.pool, office).await {
        Ok(body) => body,
        Err(error) => {
            tracing::warn!(?error, %office, "weekly report failed");
            return;
        }
    };
    let Ok(architect) = super::group::architect_bot(&state.pool, owner).await else {
        return;
    };
    let actions = json!([
        { "label": "Open apps", "kind": "open", "target": "apps" },
        { "label": "Approvals", "kind": "open", "target": "approvals" },
        { "label": "Goal & milestones", "kind": "open", "target": "goal" }
    ]);
    crate::handlers::threads::insert_bot_note(
        &state.pool,
        &state.note_sender,
        chat,
        architect,
        &body,
        Some(actions),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milestone_lines() {
        assert_eq!(
            parse_line("MILESTONE: 20 paying customers by 2027-03-01"),
            Some((
                "20 paying customers".into(),
                NaiveDate::from_ymd_opt(2027, 3, 1)
            ))
        );
        assert_eq!(
            parse_line("- **Milestone:** Launch the new menu."),
            Some(("Launch the new menu".into(), None))
        );
        assert_eq!(
            parse_line("MILESTONE: Sell by the sea"),
            Some(("Sell by the sea".into(), None))
        );
        assert_eq!(parse_line("A milestone: nope"), None);
        assert_eq!(parse_line("MILESTONE:   "), None);
    }

    #[test]
    fn dates_from_requests() {
        assert_eq!(due_setting(None).unwrap(), None);
        assert_eq!(due_setting(Some("")).unwrap(), Some(None));
        assert!(due_setting(Some("2027-02-30")).is_err());
        assert!(due_setting(Some("next week")).is_err());
    }
}
