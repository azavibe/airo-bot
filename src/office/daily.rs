//! The Architect's daily plan: every morning (8:00 in the owner's time),
//! in each office the owner has used in the last week, the Architect looks
//! at what happened and hands up to three tasks to its bots, who start at
//! once and post their work in the group chat. Nobody has to ask.
//!
//! It runs inside the server (a background loop over the database), not a
//! scheduler or queue. An office is claimed atomically before its plan
//! runs, so two server processes never plan the same office twice.
//! Each answer counts against the day's credits and web allowance like any
//! other; owners can turn it off per office (`workspaces.daily_plan`), and
//! a stopped office (`workspaces.paused`) gets no plan.

use std::time::Duration;

use uuid::Uuid;

use crate::handlers::threads::{run_agent_reply, ReplyMode};
use crate::state::AppState;

/// How often the loop looks for offices whose morning has come.
const TICK: Duration = Duration::from_secs(10 * 60);
/// Local hour the plan is due.
const PLAN_HOUR: i32 = 8;
/// Offices planned per tick, so a busy server spreads the work out.
const PER_TICK: i64 = 20;

/// The task the Architect is given each morning.
pub fn plan_task(waiting_approvals: i64) -> String {
    let mut task = String::from(
        "A new day has started. Look at what this office has done and talked about recently, then plan today: hand up to 3 useful tasks to your teammates as '@handle: task' lines, each one something a bot can do right now on its own (research, drafts, plans, checks), building on earlier work rather than repeating it. Put one short sentence before them saying what today is about.",
    );
    if waiting_approvals > 0 {
        task.push_str(&format!(
            " Also remind me that {waiting_approvals} approval(s) are waiting for my decision."
        ));
    }
    task
}

/// Claims and runs every office whose morning plan is due.
pub async fn run_due(state: &AppState) -> Result<usize, sqlx::Error> {
    // Due: plan on, morning has come in the owner's time, no plan in the
    // last 20 hours, the office is new or the owner wrote anywhere in the
    // last week (abandoned accounts cost nothing), and
    // the office has a group chat with at least one bot besides the
    // Architect.
    let due: Vec<(Uuid, Uuid, Uuid)> = sqlx::query_as(
        "UPDATE workspaces w SET last_plan_at = now(), updated_at = now()
         FROM threads chat
         WHERE chat.workspace_id = w.id AND chat.kind = 'office' AND chat.deleted_at IS NULL
           AND w.id IN (
             SELECT w2.id FROM workspaces w2
             WHERE w2.deleted_at IS NULL AND w2.daily_plan AND NOT w2.paused
               AND w2.cap_notice_month IS DISTINCT FROM date_trunc('month', now())::date
               AND (w2.last_plan_at IS NULL OR w2.last_plan_at < now() - interval '20 hours')
               AND EXTRACT(HOUR FROM (now() AT TIME ZONE 'UTC') + make_interval(mins => w2.utc_offset_minutes)) >= $1
               AND (
                 w2.created_at > now() - interval '7 days'
                 OR EXISTS (
                   SELECT 1 FROM notes n JOIN threads t ON t.id = n.thread_id
                   WHERE t.user_id = w2.owner_user_id AND n.author_user_id = w2.owner_user_id
                     AND n.created_at > now() - interval '7 days'
                 )
               )
             ORDER BY w2.last_plan_at NULLS FIRST
             LIMIT $2
             FOR UPDATE SKIP LOCKED
           )
           AND EXISTS (
             SELECT 1 FROM thread_agents ta JOIN agents a ON a.id = ta.agent_id
             WHERE ta.thread_id = chat.id AND a.name <> 'Architect'
               AND a.deleted_at IS NULL AND a.revoked_at IS NULL
           )
         RETURNING w.id, w.owner_user_id, chat.id",
    )
    .bind(f64::from(PLAN_HOUR))
    .bind(PER_TICK)
    .fetch_all(&state.pool)
    .await?;

    for (workspace_id, owner, chat) in &due {
        let architect = match super::group::architect_bot(&state.pool, *owner).await {
            Ok(id) => id,
            Err(error) => {
                tracing::warn!(?error, %workspace_id, "daily plan: no Architect");
                continue;
            }
        };
        let waiting: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM approvals a JOIN workspaces w ON w.owner_user_id = $1
             WHERE a.workspace_id = w.id AND a.status = 'pending' AND a.deleted_at IS NULL",
        )
        .bind(owner)
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0);
        let mode = ReplyMode::route().with_task_line(format!(
            "Your task now, {}: {}",
            super::group::ARCHITECT_NAME,
            plan_task(waiting)
        ));
        // One office at a time: the Architect's delegations start the bots.
        run_agent_reply(state.clone(), *chat, architect, *owner, mode).await;
    }
    Ok(due.len())
}

/// Background loop for [`run_due`].
pub async fn run_daily_plan_loop(state: AppState) {
    tracing::info!("daily plan loop starting");
    loop {
        tokio::time::sleep(TICK).await;
        match run_due(&state).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(offices = n, "daily plans started"),
            Err(error) => tracing::warn!(?error, "daily plan tick failed"),
        }
        // Opt-in check-ins and the Monday report share this loop.
        match super::goals::run_due_check_ins(&state).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(offices = n, "check-ins started"),
            Err(error) => tracing::warn!(?error, "check-in tick failed"),
        }
        match super::recurring::run_due(&state).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(jobs = n, "recurring jobs started"),
            Err(error) => tracing::warn!(?error, "recurring jobs tick failed"),
        }
        match super::goals::run_due_reports(&state).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(offices = n, "weekly reports posted"),
            Err(error) => tracing::warn!(?error, "weekly report tick failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_mentions_waiting_approvals_only_when_there_are_some() {
        assert!(plan_task(0).contains("@handle: task"));
        assert!(!plan_task(0).contains("approval"));
        assert!(plan_task(2).contains("2 approval(s) are waiting"));
    }
}
