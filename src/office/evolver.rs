//! Self-learning procedure optimizer (TextGrad pattern, docs/ui-spec.md
//! §3.5). Reads recent pass/fail runs, asks a model why runs failed, and
//! writes the improved instructions as a *new version*.
//!
//! Only the procedure owner's own runs count, so strangers recording
//! failing runs on a public procedure cannot steer a rewrite of it, and the
//! nightly sweep never touches published (non-private) procedures.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Serialize;
use serde_json::json;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::auth::Caller;
use crate::error::AppError;
use crate::office::budget_gate::{self, ActionProposal};
use crate::openrouter::call_openrouter_with_usage;
use crate::state::AppState;

const EVOLVER_MODEL: &str = "anthropic/claude-3.5-haiku";
const SWEEP_INTERVAL: Duration = Duration::from_secs(6 * 3600);
const MIN_RUNS_FOR_SWEEP: i64 = 5;
const SWEEP_PASS_RATE_THRESHOLD: f64 = 0.80;

#[derive(Debug, Serialize)]
pub struct EvolveResponse {
    pub procedure_id: Uuid,
    pub previous_version: i32,
    pub new_version: i32,
    pub pass_rate_before: f64,
    pub evaluation_gradient: String,
    pub mutated_body: String,
    pub status: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ProcedureRow {
    id: Uuid,
    user_id: Uuid,
    title: String,
    summary: String,
    body: String,
    check_description: String,
    current_version: i32,
    kind: String,
}

/// Background loop: every six hours, evolves the owner's struggling
/// private procedures.
pub async fn run_nightly_evolution_loop(state: AppState) {
    info!("self-learning evolver starting");
    loop {
        tokio::time::sleep(SWEEP_INTERVAL).await;
        if let Err(e) = run_evolution_sweep(&state).await {
            error!(error = ?e, "evolution sweep failed");
        }
    }
}

pub async fn run_evolution_sweep(state: &AppState) -> Result<usize, AppError> {
    // Pass rate from the owner's own runs in the last 7 days; skip anything
    // evolved in the last day so an unchanged pass rate doesn't churn out a
    // new version every sweep.
    let candidates: Vec<Uuid> = sqlx::query_scalar(
        r#"
        SELECT p.id
        FROM procedures p
        JOIN LATERAL (
            SELECT count(*) AS total,
                   count(*) FILTER (WHERE r.outcome = 'pass') AS passed
            FROM runs r
            WHERE r.procedure_id = p.id AND r.user_id = p.user_id
              AND r.deleted_at IS NULL AND r.outcome IS NOT NULL
              AND r.created_at >= now() - interval '7 days'
        ) stats ON true
        WHERE p.deleted_at IS NULL
          AND p.visibility = 'private'
          AND stats.total >= $1
          AND stats.passed::float8 / stats.total::float8 < $2
          AND NOT EXISTS (
              SELECT 1 FROM procedure_evolutions e
              WHERE e.procedure_id = p.id AND e.created_at > now() - interval '1 day'
          )
        LIMIT 10
        "#,
    )
    .bind(MIN_RUNS_FOR_SWEEP)
    .bind(SWEEP_PASS_RATE_THRESHOLD)
    .fetch_all(&state.pool)
    .await?;

    let mut evolved = 0;
    for procedure_id in candidates {
        match execute_procedure_evolution(state, procedure_id, None).await {
            Ok(_) => evolved += 1,
            Err(e) => warn!(%procedure_id, error = ?e, "sweep could not evolve procedure"),
        }
    }
    Ok(evolved)
}

/// Strips a ```json fence some models wrap around JSON answers.
fn strip_code_fence(text: &str) -> &str {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

/// The owner's own OpenRouter key if they connected one, else the server
/// default (metered through the owner's workspace budget).
async fn resolve_key(state: &AppState, owner: Uuid) -> Result<Option<(String, bool)>, AppError> {
    let own: Option<String> = sqlx::query_scalar(
        "SELECT provider_api_key FROM agents
         WHERE user_id = $1 AND provider_type = 'openrouter'
           AND provider_api_key IS NOT NULL AND provider_api_key <> ''
           AND revoked_at IS NULL AND deleted_at IS NULL
         ORDER BY created_at ASC LIMIT 1",
    )
    .bind(owner)
    .fetch_optional(&state.pool)
    .await?;
    if let Some(key) = own.and_then(|k| crate::crypto::decrypt(&k)) {
        return Ok(Some((key, false)));
    }
    Ok(std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .map(|k| (k, true)))
}

pub async fn execute_procedure_evolution(
    state: &AppState,
    procedure_id: Uuid,
    idempotency_key: Option<&str>,
) -> Result<EvolveResponse, AppError> {
    let pool = &state.pool;
    let proc: ProcedureRow = sqlx::query_as(
        "SELECT id, user_id, title, summary, body, check_description, current_version, kind::text AS kind
         FROM procedures WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(procedure_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let workspace =
        crate::handlers::airo::get_or_create_default_workspace(pool, proc.user_id).await?;

    let (total, passed): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE outcome = 'pass')
         FROM runs
         WHERE procedure_id = $1 AND user_id = $2 AND deleted_at IS NULL
           AND outcome IS NOT NULL AND created_at >= now() - interval '7 days'",
    )
    .bind(proc.id)
    .bind(proc.user_id)
    .fetch_one(pool)
    .await?;
    let pass_rate_before = if total > 0 {
        passed as f64 / total as f64
    } else {
        1.0
    };

    // Sample outputs: the latest step output of each recent run.
    let sample = |outcome: &'static str| {
        sqlx::query_scalar::<_, String>(
            r#"
            SELECT COALESCE(
                (SELECT rs.output FROM run_steps rs
                 WHERE rs.run_id = r.id AND rs.output IS NOT NULL AND rs.deleted_at IS NULL
                 ORDER BY rs.step_order DESC LIMIT 1),
                '(no output recorded)')
            FROM runs r
            WHERE r.procedure_id = $1 AND r.user_id = $2 AND r.deleted_at IS NULL
              AND r.outcome = $3::run_outcome AND r.created_at >= now() - interval '7 days'
            ORDER BY r.created_at DESC
            LIMIT 3
            "#,
        )
        .bind(proc.id)
        .bind(proc.user_id)
        .bind(outcome)
    };
    let failures = sample("fail").fetch_all(pool).await?;
    let successes = sample("pass").fetch_all(pool).await?;
    let clip = |s: &String| s.chars().take(2000).collect::<String>();
    let failures_summary = if failures.is_empty() {
        "No failed runs in the last 7 days.".to_string()
    } else {
        failures
            .iter()
            .map(clip)
            .collect::<Vec<_>>()
            .join("\n---\n")
    };
    let successes_summary = if successes.is_empty() {
        "No successful runs in the last 7 days.".to_string()
    } else {
        successes
            .iter()
            .map(clip)
            .collect::<Vec<_>>()
            .join("\n---\n")
    };

    let next_version = proc.current_version + 1;
    let heuristic = || {
        (
            format!(
                "Heuristic update (no AI provider): pass rate {:.1}%. Added an explicit check against '{}'.",
                pass_rate_before * 100.0,
                proc.check_description
            ),
            format!(
                "{}\n\n## Self-check (v{next_version})\n- Before finishing, confirm the output satisfies: {}",
                proc.body, proc.check_description
            ),
        )
    };

    // The model call happens outside any transaction: no row lock or pooled
    // connection is held while waiting on the provider.
    let (evaluation_gradient, mutated_body) = match resolve_key(state, proc.user_id).await? {
        None => heuristic(),
        Some((api_key, metered)) => {
            let proposal = ActionProposal {
                provider: "openrouter".into(),
                model_or_tool: EVOLVER_MODEL.into(),
                estimated_credits: 25,
                raw_cost_micros: 250,
                requires_manual_approval: false,
                action_type: "evolve_procedure".into(),
            };
            let mut ledger_id = None;
            if metered {
                let gate =
                    budget_gate::check_and_deduct(pool, workspace.id, &proposal, None, false)
                        .await?;
                if !gate.passed {
                    return Err(AppError::Conflict(format!(
                        "usage guard: {}",
                        gate.reasons.join(", ")
                    )));
                }
                ledger_id = gate.ledger_id;
            }
            let system_prompt = r#"You improve step-by-step instructions for autonomous agents.
Analyze the procedure, its verification check, and recent successful and failed outputs.
Reply with only a JSON object:
{"evaluation_gradient": "why runs failed and what pattern won", "mutated_instructions": "the improved instructions"}"#;
            let user_prompt = format!(
                "Procedure Title: {}\nSummary: {}\nCheck Description: {}\nCurrent Instructions:\n{}\n\nWinning Outputs:\n{}\n\nFailing Outputs:\n{}",
                proc.title, proc.summary, proc.check_description, proc.body, successes_summary, failures_summary
            );
            match call_openrouter_with_usage(&api_key, EVOLVER_MODEL, system_prompt, &user_prompt)
                .await
            {
                Ok(completion) => {
                    budget_gate::record_usage(pool, ledger_id, &completion.usage).await;
                    let resp = completion.text;
                    let parsed =
                        serde_json::from_str::<serde_json::Value>(strip_code_fence(&resp)).ok();
                    let gradient = parsed
                        .as_ref()
                        .and_then(|v| v["evaluation_gradient"].as_str())
                        .map(str::to_string);
                    let body = parsed
                        .as_ref()
                        .and_then(|v| v["mutated_instructions"].as_str())
                        .filter(|b| !b.trim().is_empty())
                        .map(str::to_string);
                    match (gradient, body) {
                        (Some(g), Some(b)) => (g, b),
                        _ => {
                            warn!(%procedure_id, "evolver reply was not the expected JSON");
                            heuristic()
                        }
                    }
                }
                Err(e) => {
                    warn!(%procedure_id, error = %e, "evolver provider call failed");
                    if metered {
                        let _ =
                            budget_gate::refund_credit_charge(pool, workspace.id, None, &proposal)
                                .await;
                    }
                    heuristic()
                }
            }
        }
    };

    let mut tx = pool.begin().await?;
    if let Some(key) = idempotency_key {
        if let Some(existing) = sqlx::query_as::<_, (i32, i32, f64, String, serde_json::Value)>(
            "SELECT previous_version, new_version, pass_rate_before, evaluation_gradient, mutated_steps_json
             FROM procedure_evolutions WHERE procedure_id = $1 AND idempotency_key = $2",
        )
        .bind(proc.id)
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?
        {
            return Ok(EvolveResponse {
                procedure_id,
                previous_version: existing.0,
                new_version: existing.1,
                pass_rate_before: existing.2,
                evaluation_gradient: existing.3,
                mutated_body: existing.4["body"].as_str().unwrap_or_default().to_string(),
                status: "evolved".to_string(),
            });
        }
    }

    // Optimistic check: if someone saved a new version while the model was
    // thinking, don't overwrite it.
    let current: i32 = sqlx::query_scalar(
        "SELECT current_version FROM procedures WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(proc.id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if current != proc.current_version {
        return Err(AppError::Conflict(
            "the procedure changed while it was being evolved; try again".to_string(),
        ));
    }

    sqlx::query(
        "INSERT INTO procedure_versions (procedure_id, version, body, changelog)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(proc.id)
    .bind(next_version)
    .bind(&mutated_body)
    .bind(format!("Self-learning evolution v{next_version}"))
    .execute(&mut *tx)
    .await?;

    // A workflow's steps are versioned with it; carry them forward or the
    // new version would have no steps to run.
    if proc.kind == "workflow" {
        sqlx::query(
            "INSERT INTO workflow_steps (procedure_id, version, step_order, title, instructions, agent_id)
             SELECT procedure_id, $2, step_order, title, instructions, agent_id
             FROM workflow_steps
             WHERE procedure_id = $1 AND version = $3 AND deleted_at IS NULL",
        )
        .bind(proc.id)
        .bind(next_version)
        .bind(proc.current_version)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query(
        "UPDATE procedures SET current_version = $1, body = $2, updated_at = now() WHERE id = $3",
    )
    .bind(next_version)
    .bind(&mutated_body)
    .bind(proc.id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO procedure_evolutions (
            procedure_id, workspace_id, previous_version, new_version,
            pass_rate_before, evaluation_gradient, mutated_steps_json, idempotency_key
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(proc.id)
    .bind(workspace.id)
    .bind(proc.current_version)
    .bind(next_version)
    .bind(pass_rate_before)
    .bind(&evaluation_gradient)
    .bind(json!({ "body": mutated_body, "changelog": format!("Self-learning evolution v{next_version}") }))
    .bind(idempotency_key)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(EvolveResponse {
        procedure_id,
        previous_version: proc.current_version,
        new_version: next_version,
        pass_rate_before,
        evaluation_gradient,
        mutated_body,
        status: "evolved".to_string(),
    })
}

/// POST /api/procedures/{id}/evolve -- owner only.
pub async fn evolve_procedure(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(procedure_id): Path<Uuid>,
) -> Result<Json<EvolveResponse>, AppError> {
    let key = crate::idempotency::key(&headers, None)?;
    let owner: Uuid =
        sqlx::query_scalar("SELECT user_id FROM procedures WHERE id = $1 AND deleted_at IS NULL")
            .bind(procedure_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(AppError::NotFound)?;
    if owner != caller.user_id() {
        return Err(AppError::Forbidden);
    }
    Ok(Json(
        execute_procedure_evolution(&state, procedure_id, key).await?,
    ))
}
