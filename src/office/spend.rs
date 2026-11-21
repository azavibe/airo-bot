//! Monthly spending caps and spend per office and per bot.
//!
//! Every metered AI call (one paid with the platform's credits) is charged
//! in `credit_ledger` with the office and the bot it was for. An office may
//! have a hard monthly cap, and so may each bot; [`cap_hit`] is checked in
//! the budget gate's transaction before a charge, so a call that would go
//! past a cap is never made. When an office reaches its cap, its autonomous
//! work waits (morning plans, queued tasks: see [`NOT_CAPPED`]) and its
//! group chat gets one card that month, with a button to raise the cap.
//! Calls with the owner's own key or AI tools cost no credits and are not
//! capped. Months are calendar months in UTC.
//! TODO(air): months follow UTC, not the office's own time zone; an office
//! far from UTC sees its month turn over a few hours early or late.

use axum::extract::{Path, State};
use axum::Json;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::Caller;
use crate::error::AppError;
use crate::office::budget_gate::LimitHit;
use crate::state::AppState;

/// Who a metered call is for: the office it works in and the bot making
/// it, when known. Both are recorded on the ledger row and both caps are
/// checked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Spender {
    pub office: Option<Uuid>,
    pub agent: Option<Uuid>,
}

impl Spender {
    pub fn new(office: Uuid, agent: Option<Uuid>) -> Self {
        Self {
            office: Some(office),
            agent,
        }
    }
}

/// SQL true for an office (`workspaces` row) whose autonomous work may run:
/// it has not reached its cap this month, or the cap was changed since.
pub const NOT_CAPPED: &str = "(cap_notice_month IS DISTINCT FROM date_trunc('month', now())::date)";

/// The largest cap accepted (a guard against typos).
pub const MAX_CAP_CREDITS: i64 = 100_000_000;

/// A cap from a request: `None` keeps it, `Some(None)` removes it (0 or
/// null), `Some(Some(n))` sets it.
pub fn cap_setting(raw: Option<i64>) -> Result<Option<Option<i64>>, AppError> {
    match raw {
        None => Ok(None),
        Some(0) => Ok(Some(None)),
        Some(n) if (1..=MAX_CAP_CREDITS).contains(&n) => Ok(Some(Some(n))),
        Some(_) => Err(AppError::BadRequest(format!(
            "a monthly cap is between 1 and {MAX_CAP_CREDITS} credits, or 0 for none"
        ))),
    }
}

/// Credits an office has spent this month.
pub async fn office_month_spent(pool: &PgPool, office: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COALESCE(SUM(credits_charged), 0)::bigint FROM credit_ledger
         WHERE office_id = $1 AND created_at >= date_trunc('month', now())",
    )
    .bind(office)
    .fetch_one(pool)
    .await
    .unwrap_or(0)
}

/// Credits a bot has spent this month, across offices.
pub async fn agent_month_spent(pool: &PgPool, agent: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COALESCE(SUM(credits_charged), 0)::bigint FROM credit_ledger
         WHERE agent_id = $1 AND created_at >= date_trunc('month', now())",
    )
    .bind(agent)
    .fetch_one(pool)
    .await
    .unwrap_or(0)
}

/// In the budget gate's transaction: the cap a charge of `credits` would go
/// past, if any, with a reason for the logs. The office and bot rows are
/// locked, so two charges at once can't both slip under a cap.
pub(crate) async fn cap_hit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    spender: Spender,
    credits: i64,
) -> Result<Option<(LimitHit, String)>, sqlx::Error> {
    if credits <= 0 {
        return Ok(None);
    }
    if let Some(office) = spender.office {
        let cap: Option<Option<i64>> = sqlx::query_scalar(
            "SELECT monthly_cap_credits FROM workspaces WHERE id = $1 FOR UPDATE",
        )
        .bind(office)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(Some(cap)) = cap {
            let spent: i64 = sqlx::query_scalar(
                "SELECT COALESCE(SUM(credits_charged), 0)::bigint FROM credit_ledger
                 WHERE office_id = $1 AND created_at >= date_trunc('month', now())",
            )
            .bind(office)
            .fetch_one(&mut **tx)
            .await?;
            if spent + credits > cap {
                return Ok(Some((
                    LimitHit::MonthlyOffice,
                    format!("office monthly cap reached: {spent} + {credits} > {cap}"),
                )));
            }
        }
    }
    if let Some(agent) = spender.agent {
        let cap: Option<Option<i64>> =
            sqlx::query_scalar("SELECT monthly_cap_credits FROM agents WHERE id = $1 FOR UPDATE")
                .bind(agent)
                .fetch_optional(&mut **tx)
                .await?;
        if let Some(Some(cap)) = cap {
            let spent: i64 = sqlx::query_scalar(
                "SELECT COALESCE(SUM(credits_charged), 0)::bigint FROM credit_ledger
                 WHERE agent_id = $1 AND created_at >= date_trunc('month', now())",
            )
            .bind(agent)
            .fetch_one(&mut **tx)
            .await?;
            if spent + credits > cap {
                return Ok(Some((
                    LimitHit::MonthlyBot,
                    format!("bot monthly cap reached: {spent} + {credits} > {cap}"),
                )));
            }
        }
    }
    Ok(None)
}

/// Records that an office reached its cap this month. True only the first
/// time in the month (or after the cap was changed), when the owner should
/// be told.
pub async fn mark_office_capped(pool: &PgPool, office: Uuid) -> bool {
    sqlx::query_scalar::<_, Uuid>(&format!(
        "UPDATE workspaces SET cap_notice_month = date_trunc('month', now())::date, updated_at = now()
         WHERE id = $1 AND {NOT_CAPPED}
         RETURNING id"
    ))
    .bind(office)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .is_some()
}

/// [`mark_office_capped`] for one bot.
pub async fn mark_bot_capped(pool: &PgPool, agent: Uuid) -> bool {
    sqlx::query_scalar::<_, Uuid>(&format!(
        "UPDATE agents SET cap_notice_month = date_trunc('month', now())::date, updated_at = now()
         WHERE id = $1 AND {NOT_CAPPED}
         RETURNING id"
    ))
    .bind(agent)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .is_some()
}

/// Whether an office's autonomous work is waiting on its cap.
pub async fn office_capped(pool: &PgPool, office: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(&format!(
        "SELECT NOT {NOT_CAPPED} FROM workspaces WHERE id = $1"
    ))
    .bind(office)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or(false)
}

/// After a charge was refused by a cap: marks it, and the first time this
/// month posts the card that says so (in the office's group chat for an
/// office cap; in `thread_id` for a bot's). Returns the thread the card
/// went to, if one was posted.
pub async fn notify_cap(
    state: &AppState,
    limit: LimitHit,
    spender: Spender,
    thread_id: Option<Uuid>,
) -> Option<Uuid> {
    let pool = &state.pool;
    match limit {
        LimitHit::MonthlyOffice => {
            let office = spender.office?;
            if !mark_office_capped(pool, office).await {
                return None;
            }
            let (owner, cap): (Uuid, Option<i64>) = sqlx::query_as(
                "SELECT owner_user_id, monthly_cap_credits FROM workspaces WHERE id = $1",
            )
            .bind(office)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()?;
            let chat = crate::office::group::ensure_office_chat(pool, owner, office)
                .await
                .ok()?;
            let architect = crate::office::group::architect_bot(pool, owner)
                .await
                .ok()?;
            let spent = office_month_spent(pool, office).await;
            let body = format!(
                "**This office reached its monthly spending cap** ({spent} of {} credits). Its bots won't start paid work, and the morning plan and queued tasks wait, until the 1st or until you raise the cap. Chats answered with your own OpenRouter key or AI tools aren't affected.",
                cap.unwrap_or(spent)
            );
            let actions = json!([{ "label": "Raise the cap", "kind": "open", "target": "office" }]);
            crate::handlers::threads::insert_bot_note(
                pool,
                &state.note_sender,
                chat,
                architect,
                &body,
                Some(actions),
            )
            .await;
            Some(chat)
        }
        LimitHit::MonthlyBot => {
            let agent = spender.agent?;
            let thread = thread_id?;
            if !mark_bot_capped(pool, agent).await {
                return None;
            }
            let (name, cap): (String, Option<i64>) =
                sqlx::query_as("SELECT name, monthly_cap_credits FROM agents WHERE id = $1")
                    .bind(agent)
                    .fetch_optional(pool)
                    .await
                    .ok()
                    .flatten()?;
            let spent = agent_month_spent(pool, agent).await;
            let body = format!(
                "**{name} reached its monthly spending cap** ({spent} of {} credits), so it won't make paid AI calls until the 1st or until you raise its cap in its settings.",
                cap.unwrap_or(spent)
            );
            let actions = json!([{ "label": "Bot settings", "kind": "open", "target": format!("bot:{agent}") }]);
            crate::handlers::threads::insert_bot_note(
                pool,
                &state.note_sender,
                thread,
                agent,
                &body,
                Some(actions),
            )
            .await;
            Some(thread)
        }
        _ => None,
    }
}

/// GET /agents/:id/stats -- what one bot has spent: this month (and its
/// cap), last month, and credits per day for the last 30 days; and its 10
/// most recent jobs with their outcomes.
pub async fn agent_stats(
    State(state): State<AppState>,
    caller: Caller,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let Caller::User(user) = &caller else {
        return Err(AppError::Forbidden);
    };
    let row: Option<(Option<i64>, bool)> = sqlx::query_as(&format!(
        "SELECT monthly_cap_credits, NOT {NOT_CAPPED} FROM agents
         WHERE id = $1 AND user_id = $2 AND deleted_at IS NULL"
    ))
    .bind(agent_id)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((cap, cap_reached)) = row else {
        return Err(AppError::NotFound);
    };
    let (month_spent, month_calls, last_month_spent): (i64, i64, i64) = sqlx::query_as(
        "SELECT
            COALESCE(SUM(credits_charged) FILTER (WHERE created_at >= date_trunc('month', now())), 0)::bigint,
            COUNT(*) FILTER (WHERE created_at >= date_trunc('month', now()) AND credits_charged > 0),
            COALESCE(SUM(credits_charged) FILTER (
                WHERE created_at < date_trunc('month', now())
                  AND created_at >= date_trunc('month', now()) - interval '1 month'), 0)::bigint
         FROM credit_ledger
         WHERE agent_id = $1 AND created_at >= date_trunc('month', now()) - interval '1 month'",
    )
    .bind(agent_id)
    .fetch_one(&state.pool)
    .await?;
    let daily: Vec<(chrono::NaiveDate, i64)> = sqlx::query_as(
        "SELECT d::date, COALESCE(SUM(l.credits_charged), 0)::bigint
         FROM generate_series(CURRENT_DATE - 29, CURRENT_DATE, interval '1 day') AS d
         LEFT JOIN credit_ledger l
           ON l.agent_id = $1 AND l.created_at >= d AND l.created_at < d + interval '1 day'
         GROUP BY d ORDER BY d",
    )
    .bind(agent_id)
    .fetch_all(&state.pool)
    .await?;
    // Its recent jobs with their honest outcomes and what each cost.
    type WorkRow = (
        Uuid,
        String,
        Option<String>,
        Option<String>,
        String,
        String,
        chrono::DateTime<chrono::Utc>,
        i64,
    );
    let work: Vec<WorkRow> = sqlx::query_as(
        "SELECT t.id, t.title, t.outcome, t.outcome_note, t.status, t.executor, t.created_at,
                (CASE WHEN t.executor = 'chat' AND jsonb_typeof(t.payload->'credits') = 'number'
                      THEN (t.payload->>'credits')::numeric::bigint ELSE 0 END
                 + COALESCE((SELECT SUM(l.credits_charged) FROM credit_ledger l WHERE l.task_id = t.id), 0)
                )::bigint
         FROM tasks t
         WHERE t.assigned_agent_id = $1 AND t.deleted_at IS NULL
         ORDER BY t.created_at DESC LIMIT 10",
    )
    .bind(agent_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!({
        "recent_work": work.into_iter().map(|(id, title, outcome, note, status, executor, at, credits)| json!({
            "id": id,
            "title": title,
            "outcome": outcome,
            "outcome_note": note,
            "status": status,
            "executor": executor,
            "created_at": at,
            "credits": credits,
        })).collect::<Vec<_>>(),
        "month_spent": month_spent,
        "month_calls": month_calls,
        "last_month_spent": last_month_spent,
        "monthly_cap": cap,
        // Refused a call this month and not raised since.
        "cap_reached": cap_reached && cap.is_some(),
        "daily": daily.into_iter().map(|(day, credits)| json!({ "day": day, "credits": credits })).collect::<Vec<_>>(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_from_requests() {
        assert_eq!(cap_setting(None).unwrap(), None);
        assert_eq!(cap_setting(Some(0)).unwrap(), Some(None));
        assert_eq!(cap_setting(Some(500)).unwrap(), Some(Some(500)));
        assert!(cap_setting(Some(-1)).is_err());
        assert!(cap_setting(Some(MAX_CAP_CREDITS + 1)).is_err());
    }
}
