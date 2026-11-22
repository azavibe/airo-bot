//! Recurring jobs set from chat, and per-bot permission modes.
//!
//! The owner asks a bot for something regular ("every Monday at 9, list
//! overdue invoices"); the bot answers with one line, `EVERY Monday at 9:
//! list overdue invoices`, which leaves the message and becomes a schedule
//! of that bot in its room. In the bot's default mode, **Ask first**, the
//! schedule waits for the owner's Start; a **Trusted** bot's schedule
//! starts at once and can be stopped. The owner can also add, start, stop
//! and delete schedules in the bot's settings. A due schedule runs as one of
//! the bot's jobs (so it reports an honest outcome and is metered and capped
//! like any other), in the owner's time, at most once per slot.
//! Anything sent to an outside tool still needs an approval in every mode.
//! The modes also decide whether the bot's changes to existing app
//! records apply at once or wait for an OK (see `records.rs`).

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::Caller;
use crate::error::AppError;
use crate::state::AppState;

/// Schedules one bot keeps (proposed and active).
const MAX_PER_BOT: i64 = 10;
const WEEKDAYS: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct When {
    /// `day`, `week` or `month`.
    pub every: &'static str,
    /// ISO weekday, 1 = Monday (weekly).
    pub weekday: Option<i16>,
    /// 1-28 (monthly).
    pub day_of_month: Option<i16>,
    pub hour: i16,
}

impl When {
    pub fn describe(&self) -> String {
        let at = format!("at {:02}:00", self.hour);
        match self.every {
            "week" => {
                let day = WEEKDAYS[(self.weekday.unwrap_or(1) - 1) as usize];
                let mut name = day.to_string();
                name[..1].make_ascii_uppercase();
                format!("every {name} {at}")
            }
            "month" => {
                let d = self.day_of_month.unwrap_or(1);
                let suffix = match d {
                    1 | 21 => "st",
                    2 | 22 => "nd",
                    3 | 23 => "rd",
                    _ => "th",
                };
                format!("every month on the {d}{suffix} {at}")
            }
            _ => format!("every day {at}"),
        }
    }
}

fn weekday_of(word: &str) -> Option<i16> {
    let w = word.trim_end_matches(['s', ',']);
    WEEKDAYS
        .iter()
        .position(|d| *d == w || (w.len() >= 3 && d.starts_with(w)))
        .map(|i| i as i16 + 1)
}

/// The hour in "9", "09:30", "9am", "5pm", "17h".
fn hour_of(word: &str) -> Option<i16> {
    let w = word.trim_end_matches(['.', ',']).to_ascii_lowercase();
    let (num, pm) = if let Some(n) = w.strip_suffix("pm") {
        (n.to_string(), Some(true))
    } else if let Some(n) = w.strip_suffix("am") {
        (n.to_string(), Some(false))
    } else {
        (w.trim_end_matches('h').to_string(), None)
    };
    let hour: i16 = num.split(':').next()?.trim().parse().ok()?;
    let hour = match pm {
        Some(true) if hour < 12 => hour + 12,
        Some(false) if hour == 12 => 0,
        _ => hour,
    };
    (0..=23).contains(&hour).then_some(hour)
}

/// Reads the "when" of an `EVERY` line: "day at 9", "Monday at 8am",
/// "week on friday at 17", "month on the 1st at 9". The hour defaults to 9.
pub fn parse_when(text: &str) -> Option<When> {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| w.to_ascii_lowercase())
        .collect();
    let first = words.first()?;
    let mut when = When {
        every: "day",
        weekday: None,
        day_of_month: None,
        hour: 9,
    };
    if first == "day" || first == "morning" {
        when.every = "day";
    } else if let Some(d) = weekday_of(first) {
        when.every = "week";
        when.weekday = Some(d);
    } else if first == "week" {
        when.every = "week";
        when.weekday = Some(
            words
                .iter()
                .skip(1)
                .find_map(|w| weekday_of(w))
                .unwrap_or(1),
        );
    } else if first == "month" {
        when.every = "month";
        let day = words.iter().skip(1).find_map(|w| {
            let digits: String = w.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<i16>().ok().filter(|d| (1..=31).contains(d))
        });
        when.day_of_month = Some(day.unwrap_or(1).min(28));
    } else {
        return None;
    }
    if let Some(at) = words.iter().position(|w| w == "at") {
        when.hour = hour_of(words.get(at + 1)?)?;
    }
    Some(when)
}

/// An `EVERY <when>: <job>` line: its schedule and job.
fn parse_line(line: &str) -> Option<(When, String)> {
    let trimmed = line
        .trim()
        .trim_start_matches(['-', '*', ' '])
        .trim_matches('*')
        .trim();
    // Capitals only, as bots are told to write it: a sentence that starts
    // "Every Monday: ..." is not a schedule.
    if !trimmed.starts_with("EVERY ") {
        return None;
    }
    let rest = &trimmed["every ".len()..];
    let (when, job) = rest.split_once(": ")?;
    let when = parse_when(when.trim_matches('*'))?;
    let job: String = job
        .trim()
        .trim_matches('*')
        .trim()
        .chars()
        .take(500)
        .collect();
    (!job.is_empty()).then_some((when, job))
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Schedule {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub thread_id: Uuid,
    pub job: String,
    pub every: String,
    pub weekday: Option<i16>,
    pub day_of_month: Option<i16>,
    pub hour: i16,
    pub status: String,
    pub source: String,
    pub last_run_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, workspace_id, agent_id, thread_id, job, every, weekday, day_of_month, hour, status, source, last_run_at, created_at";

fn when_of(s: &Schedule) -> When {
    When {
        every: match s.every.as_str() {
            "week" => "week",
            "month" => "month",
            _ => "day",
        },
        weekday: s.weekday,
        day_of_month: s.day_of_month,
        hour: s.hour,
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert(
    pool: &PgPool,
    office: Uuid,
    agent: Uuid,
    thread: Uuid,
    when: When,
    job: &str,
    active: bool,
    source: &str,
    key: Option<&str>,
) -> Result<Schedule, sqlx::Error> {
    sqlx::query_as::<_, Schedule>(&format!(
        "INSERT INTO recurring_jobs
            (workspace_id, agent_id, thread_id, job, every, weekday, day_of_month, hour,
             status, source, started_at, idempotency_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, CASE WHEN $9 = 'active' THEN now() END, $11)
         RETURNING {COLUMNS}"
    ))
    .bind(office)
    .bind(agent)
    .bind(thread)
    .bind(job)
    .bind(when.every)
    .bind(when.weekday)
    .bind(when.day_of_month)
    .bind(when.hour)
    .bind(if active { "active" } else { "proposed" })
    .bind(source)
    .bind(key)
    .fetch_one(pool)
    .await
}

async fn open_count(pool: &PgPool, agent: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM recurring_jobs
         WHERE agent_id = $1 AND deleted_at IS NULL AND status <> 'stopped'",
    )
    .bind(agent)
    .fetch_one(pool)
    .await
    .unwrap_or(MAX_PER_BOT)
}

/// Whether a bot runs in Trusted mode.
async fn trusted(pool: &PgPool, agent: Uuid) -> bool {
    sqlx::query_scalar::<_, String>("SELECT permission_mode FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .is_some_and(|m| m == "trusted")
}

/// Takes `EVERY ...:` lines out of a bot's reply and keeps them as its
/// schedules in this room (proposed, or active for a Trusted bot).
/// Returns the reply without them and (id, description, active).
pub async fn apply_schedules(
    pool: &PgPool,
    thread_id: Uuid,
    agent_id: Uuid,
    text: &str,
) -> (String, Vec<(Uuid, String, bool)>) {
    if !text.contains("EVERY ") {
        return (text.to_string(), Vec::new());
    }
    let lines: Vec<(&str, Option<(When, String)>)> =
        text.lines().map(|l| (l, parse_line(l))).collect();
    if lines.iter().all(|(_, p)| p.is_none()) {
        return (text.to_string(), Vec::new());
    }
    let Some(office) = crate::office::context::business_of_thread(pool, thread_id)
        .await
        .map(|b| b.workspace_id)
    else {
        return (text.to_string(), Vec::new());
    };
    let active = trusted(pool, agent_id).await;
    let mut kept = Vec::new();
    let mut made = Vec::new();
    for (line, parsed) in lines {
        match parsed {
            Some((when, job)) if open_count(pool, agent_id).await < MAX_PER_BOT => {
                match insert(
                    pool, office, agent_id, thread_id, when, &job, active, "bot", None,
                )
                .await
                {
                    Ok(s) => made.push((s.id, format!("{}: {job}", when.describe()), active)),
                    Err(error) => {
                        tracing::warn!(?error, %thread_id, "could not keep a schedule");
                        kept.push(line);
                    }
                }
            }
            _ => kept.push(line),
        }
    }
    (kept.join("\n").trim().to_string(), made)
}

/// The line and buttons a reply gets for the schedules it set.
pub fn schedules_note(made: &[(Uuid, String, bool)]) -> (String, Vec<Value>) {
    let mut lines = Vec::new();
    let mut buttons = Vec::new();
    for (id, what, active) in made {
        if *active {
            lines.push(format!("*Recurring job set, {what}.*"));
            buttons.push(json!({ "label": "Stop it", "kind": "stop_job", "target": id }));
        } else {
            lines.push(format!(
                "*Recurring job proposed, {what}. It starts when you press Start.*"
            ));
            buttons.push(json!({ "label": "Start", "kind": "start_job", "target": id }));
        }
    }
    (lines.join("\n"), buttons)
}

/// What a bot is told about recurring jobs, with the ones it has.
pub async fn prompt_block(pool: &PgPool, agent_id: Uuid) -> String {
    let rows: Vec<Schedule> = sqlx::query_as::<_, Schedule>(&format!(
        "SELECT {COLUMNS} FROM recurring_jobs
         WHERE agent_id = $1 AND deleted_at IS NULL AND status <> 'stopped'
         ORDER BY created_at LIMIT 10"
    ))
    .bind(agent_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let mut out = String::from(
        "\n\n## Recurring jobs\nWhen the owner asks for something to happen regularly, set it with one line on its own: EVERY <day | Monday ... Sunday | month on <1-28>> at <hour>: <the job>. For example: EVERY Monday at 9: list overdue invoices and draft reminders.",
    );
    out.push_str(if trusted(pool, agent_id).await {
        " It starts at once; the owner can stop it.\n"
    } else {
        " The owner starts it.\n"
    });
    if !rows.is_empty() {
        out.push_str("Your recurring jobs (don't set them again):\n");
        for s in &rows {
            out.push_str(&format!(
                "- {}: {}{}\n",
                when_of(s).describe(),
                s.job,
                if s.status == "proposed" {
                    " (waiting for the owner's Start)"
                } else {
                    ""
                }
            ));
        }
    }
    out
}

/// SQL: the owner's local time in office `w`.
const LOCAL_NOW: &str =
    "((now() AT TIME ZONE 'UTC') + make_interval(mins => w.utc_offset_minutes))";

/// Claims the schedules whose slot has come and starts each as a job of
/// its bot. A slot runs once, only if it came after the schedule started;
/// stopped and capped offices, and owners away for two weeks, skip theirs.
pub async fn run_due(state: &AppState) -> Result<usize, sqlx::Error> {
    let due: Vec<(Uuid, Uuid, String, Uuid, String)> = sqlx::query_as(&format!(
        "UPDATE recurring_jobs j SET last_run_at = now(), updated_at = now()
         FROM workspaces w, agents a, threads t
         WHERE w.id = j.workspace_id AND a.id = j.agent_id AND t.id = j.thread_id
           AND a.deleted_at IS NULL AND a.revoked_at IS NULL AND t.deleted_at IS NULL
           AND j.id IN (
             SELECT j.id FROM recurring_jobs j JOIN workspaces w ON w.id = j.workspace_id
             WHERE j.deleted_at IS NULL AND j.status = 'active'
               AND w.deleted_at IS NULL AND NOT w.paused
               AND w.cap_notice_month IS DISTINCT FROM date_trunc('month', now())::date
               AND EXTRACT(HOUR FROM {LOCAL_NOW}) >= j.hour
               AND (j.every = 'day'
                    OR (j.every = 'week' AND EXTRACT(ISODOW FROM {LOCAL_NOW}) = j.weekday)
                    OR (j.every = 'month' AND EXTRACT(DAY FROM {LOCAL_NOW}) = j.day_of_month))
               AND (j.last_run_at IS NULL
                    OR ((j.last_run_at AT TIME ZONE 'UTC') + make_interval(mins => w.utc_offset_minutes))::date
                       < ({LOCAL_NOW})::date)
               AND ((({LOCAL_NOW})::date + make_interval(hours => j.hour))
                    - make_interval(mins => w.utc_offset_minutes))
                   >= (j.started_at AT TIME ZONE 'UTC')
               AND EXISTS (
                   SELECT 1 FROM notes n JOIN threads nt ON nt.id = n.thread_id
                   WHERE nt.user_id = w.owner_user_id AND n.author_user_id = w.owner_user_id
                     AND n.created_at > now() - interval '14 days')
             ORDER BY j.last_run_at NULLS FIRST
             LIMIT 20
             FOR UPDATE OF j SKIP LOCKED
           )
         RETURNING j.thread_id, j.agent_id, a.name, w.owner_user_id, j.job"
    ))
    .fetch_all(&state.pool)
    .await?;
    for (thread, agent, name, owner, job) in &due {
        crate::handlers::threads::spawn_agent_reply(
            state.clone(),
            *thread,
            *agent,
            *owner,
            crate::handlers::threads::ReplyMode::task(name, job),
        );
    }
    Ok(due.len())
}

// ---------------------------------------------------------------- handlers

async fn owned_bot(pool: &PgPool, caller: &Caller, agent: Uuid) -> Result<Uuid, AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };
    let mine: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE id = $1 AND user_id = $2 AND deleted_at IS NULL)",
    )
    .bind(agent)
    .bind(user.id)
    .fetch_one(pool)
    .await?;
    if !mine {
        return Err(AppError::NotFound);
    }
    Ok(user.id)
}

async fn owned_schedule(pool: &PgPool, caller: &Caller, id: Uuid) -> Result<Schedule, AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };
    sqlx::query_as::<_, Schedule>(&format!(
        "SELECT {} FROM recurring_jobs j JOIN workspaces w ON w.id = j.workspace_id
         WHERE j.id = $1 AND j.deleted_at IS NULL AND w.owner_user_id = $2 AND w.deleted_at IS NULL",
        COLUMNS
            .split(", ")
            .map(|c| format!("j.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
    .bind(id)
    .bind(user.id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)
}

fn view(s: &Schedule) -> Value {
    let mut v = serde_json::to_value(s).unwrap_or_else(|_| json!({}));
    v["when"] = json!(when_of(s).describe());
    v
}

/// GET /agents/:id/schedules -- the bot's schedules and permission mode.
pub async fn list_schedules(
    State(state): State<AppState>,
    caller: Caller,
    Path(agent): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    owned_bot(&state.pool, &caller, agent).await?;
    let mode: String = sqlx::query_scalar("SELECT permission_mode FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(&state.pool)
        .await?;
    let rows = sqlx::query_as::<_, Schedule>(&format!(
        "SELECT {COLUMNS} FROM recurring_jobs
         WHERE agent_id = $1 AND deleted_at IS NULL
         ORDER BY CASE status WHEN 'proposed' THEN 0 WHEN 'active' THEN 1 ELSE 2 END, created_at"
    ))
    .bind(agent)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!({
        "permission_mode": mode,
        "schedules": rows.iter().map(view).collect::<Vec<_>>(),
        "limit": MAX_PER_BOT,
    })))
}

#[derive(Debug, Deserialize)]
pub struct OfficeParam {
    pub office: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct CreateScheduleRequest {
    pub job: String,
    /// `day`, `week` or `month`.
    pub every: String,
    /// ISO weekday 1-7 (weekly).
    #[serde(default)]
    pub weekday: Option<i16>,
    /// 1-28 (monthly).
    #[serde(default)]
    pub day_of_month: Option<i16>,
    #[serde(default)]
    pub hour: Option<i16>,
    pub idempotency_key: Option<String>,
}

/// POST /agents/:id/schedules?office= -- the owner's own schedule for this
/// bot, run in its room in that office. Starts at once.
pub async fn create_schedule(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(agent): Path<Uuid>,
    Query(q): Query<OfficeParam>,
    Json(body): Json<CreateScheduleRequest>,
) -> Result<Json<Value>, AppError> {
    let key = crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let user_id = owned_bot(&state.pool, &caller, agent).await?;
    crate::handlers::offices::owned_office(&state.pool, user_id, q.office).await?;
    let job: String = body.job.split_whitespace().collect::<Vec<_>>().join(" ");
    if job.is_empty() || job.chars().count() > 500 {
        return Err(AppError::BadRequest("a job is 1-500 characters".into()));
    }
    let hour = body.hour.unwrap_or(9);
    if !(0..=23).contains(&hour) {
        return Err(AppError::BadRequest("the hour is 0-23".into()));
    }
    let when = match body.every.as_str() {
        "day" => When {
            every: "day",
            weekday: None,
            day_of_month: None,
            hour,
        },
        "week" => When {
            every: "week",
            weekday: Some(
                body.weekday
                    .filter(|d| (1..=7).contains(d))
                    .ok_or_else(|| {
                        AppError::BadRequest("a weekly job needs a weekday, 1 (Monday) to 7".into())
                    })?,
            ),
            day_of_month: None,
            hour,
        },
        "month" => When {
            every: "month",
            weekday: None,
            day_of_month: Some(
                body.day_of_month
                    .filter(|d| (1..=28).contains(d))
                    .ok_or_else(|| {
                        AppError::BadRequest("a monthly job needs a day of the month, 1-28".into())
                    })?,
            ),
            hour,
        },
        _ => return Err(AppError::BadRequest("every is day, week or month".into())),
    };
    if let Some(key) = key {
        if let Some(existing) = sqlx::query_as::<_, Schedule>(&format!(
            "SELECT {COLUMNS} FROM recurring_jobs
             WHERE agent_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL"
        ))
        .bind(agent)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?
        {
            return Ok(Json(view(&existing)));
        }
    }
    if open_count(&state.pool, agent).await >= MAX_PER_BOT {
        return Err(AppError::BadRequest(format!(
            "a bot has at most {MAX_PER_BOT} recurring jobs; stop or delete one first"
        )));
    }
    // Its own room in that office (the one it answers alone, if any).
    let room: Option<Uuid> = sqlx::query_scalar(
        "SELECT t.id FROM threads t JOIN thread_agents ta ON ta.thread_id = t.id
         WHERE ta.agent_id = $1 AND t.workspace_id = $2 AND t.deleted_at IS NULL
         ORDER BY (SELECT COUNT(*) FROM thread_agents x WHERE x.thread_id = t.id), t.created_at
         LIMIT 1",
    )
    .bind(agent)
    .bind(q.office)
    .fetch_optional(&state.pool)
    .await?;
    let Some(room) = room else {
        return Err(AppError::BadRequest(
            "this bot has no room in that office".into(),
        ));
    };
    let saved = insert(
        &state.pool,
        q.office,
        agent,
        room,
        when,
        &job,
        true,
        "owner",
        key,
    )
    .await?;
    Ok(Json(view(&saved)))
}

#[derive(Debug, Deserialize)]
pub struct UpdateScheduleRequest {
    /// `active` (start) or `stopped`.
    pub status: String,
    pub idempotency_key: Option<String>,
}

/// PATCH /api/schedules/:id -- start or stop.
pub async fn update_schedule(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateScheduleRequest>,
) -> Result<Json<Value>, AppError> {
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let current = owned_schedule(&state.pool, &caller, id).await?;
    if !matches!(body.status.as_str(), "active" | "stopped") {
        return Err(AppError::BadRequest("status is active or stopped".into()));
    }
    if body.status == "active"
        && current.status == "stopped"
        && open_count(&state.pool, current.agent_id).await >= MAX_PER_BOT
    {
        return Err(AppError::BadRequest(format!(
            "a bot has at most {MAX_PER_BOT} recurring jobs; stop or delete one first"
        )));
    }
    let saved = sqlx::query_as::<_, Schedule>(&format!(
        "UPDATE recurring_jobs SET status = $2,
            started_at = CASE WHEN $2 = 'active' AND status <> 'active' THEN now() ELSE started_at END,
            updated_at = now()
         WHERE id = $1
         RETURNING {COLUMNS}"
    ))
    .bind(id)
    .bind(&body.status)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(view(&saved)))
}

#[derive(Debug, Default, Deserialize)]
pub struct DeleteRequest {
    pub idempotency_key: Option<String>,
}

/// POST /api/schedules/:id/delete -- soft.
pub async fn delete_schedule(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    body: Option<Json<DeleteRequest>>,
) -> Result<Json<Value>, AppError> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    owned_schedule(&state.pool, &caller, id).await?;
    sqlx::query("UPDATE recurring_jobs SET deleted_at = now(), updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "deleted": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_is_read_from_plain_words() {
        let w = parse_when("Monday at 8am").unwrap();
        assert_eq!((w.every, w.weekday, w.hour), ("week", Some(1), 8));
        let w = parse_when("day at 17:30").unwrap();
        assert_eq!((w.every, w.hour), ("day", 17));
        let w = parse_when("week on friday at 5pm").unwrap();
        assert_eq!((w.every, w.weekday, w.hour), ("week", Some(5), 17));
        let w = parse_when("month on the 31st").unwrap();
        assert_eq!((w.every, w.day_of_month, w.hour), ("month", Some(28), 9));
        assert_eq!(parse_when("Thu").unwrap().weekday, Some(4));
        assert!(parse_when("now and then").is_none());
        assert!(parse_when("day at noonish").is_none());
    }

    #[test]
    fn lines_and_descriptions() {
        let (w, job) = parse_line("EVERY Monday at 9: list overdue invoices").unwrap();
        assert_eq!(job, "list overdue invoices");
        assert_eq!(w.describe(), "every Monday at 09:00");
        let (w, job) =
            parse_line("- **EVERY day at 7:30: check the inbox: reply drafts**").unwrap();
        assert_eq!(job, "check the inbox: reply drafts");
        assert_eq!(w.describe(), "every day at 07:00");
        let (w, _) = parse_line("EVERY month on 1 at 10: send the summary").unwrap();
        assert_eq!(w.describe(), "every month on the 1st at 10:00");
        assert!(parse_line("Every time you call, I answer.").is_none());
        assert!(parse_line("Every Monday: we bake bread.").is_none());
        assert!(parse_line("EVERY Monday:   ").is_none());
    }
}
