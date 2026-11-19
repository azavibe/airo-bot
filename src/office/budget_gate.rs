use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What one metered AI call costs today: a flat rate, until pricing
/// follows the real usage `record_usage` now records.
pub const FLAT_CALL_CREDITS: i64 = 25;

/// The limit the next flat-rate call would hit, if any.
pub fn next_call_limit(balance: i64, burned_today: i64, max_daily: i64) -> Option<LimitHit> {
    if balance < FLAT_CALL_CREDITS {
        Some(LimitHit::Balance)
    } else if burned_today + FLAT_CALL_CREDITS > max_daily {
        Some(LimitHit::Daily)
    } else {
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetPolicy {
    pub credit_balance: i64,
    pub credits_burned_today: i64,
    pub max_daily_credit_burn: i64,
    pub cost_approval_threshold: i64,
    pub is_paused: bool,
}

impl Default for BudgetPolicy {
    fn default() -> Self {
        Self {
            credit_balance: 50_000,
            credits_burned_today: 0,
            max_daily_credit_burn: 5_000,
            cost_approval_threshold: 200,
            is_paused: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionProposal {
    pub provider: String,
    pub model_or_tool: String,
    pub estimated_credits: i64,
    pub raw_cost_micros: i64,
    pub requires_manual_approval: bool,
    pub action_type: String, // e.g. "publish_post", "call_llm", "message_lead", "generate_video"
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BudgetGateResult {
    pub passed: bool,
    pub requires_approval: bool,
    pub reasons: Vec<String>,
    pub suggested_credit_charge: i64,
    /// The `credit_ledger` row this check charged, when it charged.
    /// `record_usage` attaches the real cost to it after the call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ledger_id: Option<Uuid>,
    /// Which limit stopped it, for the message people see.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<LimitHit>,
}

/// The wallet limit that refused a charge.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LimitHit {
    /// Today's burn cap; resets at the next calendar day.
    Daily,
    /// The plan's credit balance is used up.
    Balance,
}

/// Pure mathematical deterministic check on credit burn & workspace safety limits
pub fn evaluate_budget_gate(proposal: &ActionProposal, policy: &BudgetPolicy) -> BudgetGateResult {
    let mut reasons = Vec::new();
    let mut limit = None;
    let mut requires_approval = proposal.requires_manual_approval;

    if policy.is_paused {
        reasons.push("workspace operations are currently paused by safety kill switch".to_string());
    }

    if proposal.estimated_credits < 0 {
        reasons.push("estimated credits cannot be negative".to_string());
    }

    if policy.credit_balance < proposal.estimated_credits {
        limit = Some(LimitHit::Balance);
        reasons.push(format!(
            "insufficient credit balance: available {} < requested {}",
            policy.credit_balance, proposal.estimated_credits
        ));
    }

    let projected_daily_burn = policy.credits_burned_today + proposal.estimated_credits;
    if projected_daily_burn > policy.max_daily_credit_burn {
        limit = limit.or(Some(LimitHit::Daily));
        reasons.push(format!(
            "daily burn limit reached: {} + {} = {} exceeds max limit of {}",
            policy.credits_burned_today,
            proposal.estimated_credits,
            projected_daily_burn,
            policy.max_daily_credit_burn
        ));
    }

    // High cost or critical actions trigger human approval
    if proposal.estimated_credits >= policy.cost_approval_threshold {
        requires_approval = true;
    }

    if proposal.action_type == "publish_post"
        || proposal.action_type == "message_lead"
        || proposal.action_type == "execute_external_action"
    {
        requires_approval = true;
    }

    let passed = reasons.is_empty();

    BudgetGateResult {
        passed,
        requires_approval,
        reasons,
        suggested_credit_charge: proposal.estimated_credits,
        ledger_id: None,
        limit,
    }
}

/// Deducts credits atomically inside a Postgres transaction, logging to `credit_ledger`
#[allow(clippy::too_many_arguments)]
pub async fn apply_credit_charge(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    task_id: Option<Uuid>,
    provider: &str,
    model_or_tool: &str,
    credits: i64,
    raw_cost_micros: i64,
    idempotency_key: Option<&str>,
) -> Result<(i64, Uuid), sqlx::Error> {
    // 1. Update wallet balance and today's burn atomically
    let remaining_balance: i64 = sqlx::query_scalar(
        r#"
        UPDATE workspace_wallets
        SET credit_balance = credit_balance - $1,
            credits_burned_today = credits_burned_today + $1,
            updated_at = NOW()
        WHERE workspace_id = $2
        RETURNING credit_balance
        "#,
    )
    .bind(credits)
    .bind(workspace_id)
    .fetch_one(&mut **tx)
    .await?;

    // 2. Insert immutable audit entry in credit_ledger
    let ledger_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO credit_ledger (
            workspace_id, task_id, provider, model_or_tool,
            credits_charged, raw_cost_micros, idempotency_key
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(provider)
    .bind(model_or_tool)
    .bind(credits)
    .bind(raw_cost_micros)
    .bind(idempotency_key)
    .fetch_one(&mut **tx)
    .await?;

    Ok((remaining_balance, ledger_id))
}

/// Atomically inspects wallet policy, checks safety limits, and deducts
/// credits in one transaction.
///
/// Fails closed: any database error is returned to the caller, which must
/// not execute the action. A workspace without a wallet gets the plan's
/// default wallet provisioned once (it is never re-granted), and the daily
/// burn counter rolls over when the calendar day changes. `approved` means
/// a human already approved this exact action, so the cost threshold does
/// not ask again; nothing is charged while approval is still required.
pub async fn check_and_deduct(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    proposal: &ActionProposal,
    task_id: Option<Uuid>,
    approved: bool,
) -> Result<BudgetGateResult, crate::error::AppError> {
    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO workspace_wallets (workspace_id) VALUES ($1)
         ON CONFLICT (workspace_id) DO NOTHING",
    )
    .bind(workspace_id)
    .execute(&mut *tx)
    .await?;

    let (balance, burned_today, max_daily): (i64, i64, i64) = sqlx::query_as(
        r#"
        UPDATE workspace_wallets
        SET credits_burned_today = CASE WHEN burn_day < CURRENT_DATE THEN 0 ELSE credits_burned_today END,
            burn_day = CURRENT_DATE
        WHERE workspace_id = $1
        RETURNING credit_balance, credits_burned_today, max_daily_credit_burn
        "#,
    )
    .bind(workspace_id)
    .fetch_one(&mut *tx)
    .await?;

    let policy = BudgetPolicy {
        credit_balance: balance,
        credits_burned_today: burned_today,
        max_daily_credit_burn: max_daily,
        cost_approval_threshold: 200,
        is_paused: false,
    };

    let mut result = evaluate_budget_gate(proposal, &policy);
    if approved {
        result.requires_approval = false;
    }
    if result.passed && !result.requires_approval && result.suggested_credit_charge > 0 {
        let (_, ledger_id) = apply_credit_charge(
            &mut tx,
            workspace_id,
            task_id,
            &proposal.provider,
            &proposal.model_or_tool,
            result.suggested_credit_charge,
            proposal.raw_cost_micros,
            None,
        )
        .await?;
        result.ledger_id = Some(ledger_id);
    }

    tx.commit().await?;
    Ok(result)
}

/// Attaches what a metered call really used to the ledger row that paid
/// for it: tokens, and the provider-reported cost in place of the estimate.
/// Credits charged are not changed (pricing is still flat per call).
/// Best effort: a failure is logged, never surfaced, since the call itself
/// already happened.
pub async fn record_usage(
    pool: &sqlx::PgPool,
    ledger_id: Option<Uuid>,
    usage: &crate::openrouter::Usage,
) {
    let Some(ledger_id) = ledger_id else {
        return;
    };
    if let Err(error) = sqlx::query(
        "UPDATE credit_ledger
         SET prompt_tokens = $2,
             completion_tokens = $3,
             raw_cost_micros = COALESCE($4, raw_cost_micros),
             usage_recorded_at = now(),
             updated_at = now()
         WHERE id = $1",
    )
    .bind(ledger_id)
    .bind(usage.prompt_tokens)
    .bind(usage.completion_tokens)
    .bind(usage.cost_micros)
    .execute(pool)
    .await
    {
        tracing::warn!(%error, %ledger_id, "failed to record AI usage");
    }
}

/// Returns credits for an action that was charged but never performed
/// (e.g. the provider call failed). Recorded as a negative ledger row so
/// the ledger stays append-only.
pub async fn refund_credit_charge(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    task_id: Option<Uuid>,
    proposal: &ActionProposal,
) -> Result<(), sqlx::Error> {
    if proposal.estimated_credits <= 0 {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE workspace_wallets
         SET credit_balance = credit_balance + $1,
             credits_burned_today = GREATEST(credits_burned_today - $1, 0),
             updated_at = now()
         WHERE workspace_id = $2",
    )
    .bind(proposal.estimated_credits)
    .bind(workspace_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO credit_ledger (workspace_id, task_id, provider, model_or_tool, credits_charged, raw_cost_micros)
         VALUES ($1, $2, $3, $4, $5, 0)",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(&proposal.provider)
    .bind(format!("refund:{}", proposal.model_or_tool))
    .bind(-proposal.estimated_credits)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_call_limit_names_the_wall_that_is_hit() {
        assert_eq!(next_call_limit(50_000, 0, 5_000), None);
        assert_eq!(next_call_limit(50_000, 4_990, 5_000), Some(LimitHit::Daily));
        assert_eq!(next_call_limit(10, 0, 5_000), Some(LimitHit::Balance));
        // An empty balance wins over the daily cap: waiting a day won't help.
        assert_eq!(next_call_limit(0, 5_000, 5_000), Some(LimitHit::Balance));
    }

    #[test]
    fn test_valid_proposal_passes() {
        let policy = BudgetPolicy::default();
        let proposal = ActionProposal {
            provider: "openrouter".into(),
            model_or_tool: "anthropic/claude-3.5-haiku".into(),
            estimated_credits: 50,
            raw_cost_micros: 200,
            requires_manual_approval: false,
            action_type: "call_llm".into(),
        };

        let result = evaluate_budget_gate(&proposal, &policy);
        assert!(result.passed);
        assert!(!result.requires_approval);
        assert!(result.reasons.is_empty());
        assert_eq!(result.suggested_credit_charge, 50);
    }

    #[test]
    fn test_insufficient_credits_fails() {
        let policy = BudgetPolicy {
            credit_balance: 10,
            ..Default::default()
        };
        let proposal = ActionProposal {
            provider: "openrouter".into(),
            model_or_tool: "anthropic/claude-3.5-sonnet".into(),
            estimated_credits: 50,
            raw_cost_micros: 2000,
            requires_manual_approval: false,
            action_type: "call_llm".into(),
        };

        let result = evaluate_budget_gate(&proposal, &policy);
        assert!(!result.passed);
        assert!(result
            .reasons
            .iter()
            .any(|r| r.contains("insufficient credit balance")));
        assert_eq!(result.limit, Some(LimitHit::Balance));
    }

    #[test]
    fn test_daily_burn_cap_enforced() {
        let policy = BudgetPolicy {
            credits_burned_today: 4950,
            max_daily_credit_burn: 5000,
            ..Default::default()
        };
        let proposal = ActionProposal {
            provider: "wan3_video".into(),
            model_or_tool: "wan3-fast".into(),
            estimated_credits: 100,
            raw_cost_micros: 10000,
            requires_manual_approval: false,
            action_type: "generate_video".into(),
        };

        let result = evaluate_budget_gate(&proposal, &policy);
        assert!(!result.passed);
        assert!(result
            .reasons
            .iter()
            .any(|r| r.contains("daily burn limit reached")));
        assert_eq!(result.limit, Some(LimitHit::Daily));
    }

    #[test]
    fn test_high_value_action_triggers_approval() {
        let policy = BudgetPolicy::default();
        let proposal = ActionProposal {
            provider: "wan3_video".into(),
            model_or_tool: "wan3-ultra".into(),
            estimated_credits: 250,
            raw_cost_micros: 25000,
            requires_manual_approval: false,
            action_type: "generate_video".into(),
        };

        let result = evaluate_budget_gate(&proposal, &policy);
        assert!(result.passed);
        assert!(result.requires_approval);
    }

    #[test]
    fn test_external_actions_trigger_approval() {
        let policy = BudgetPolicy::default();
        let proposal = ActionProposal {
            provider: "zernio_social".into(),
            model_or_tool: "instagram_reels".into(),
            estimated_credits: 10,
            raw_cost_micros: 1000,
            requires_manual_approval: false,
            action_type: "publish_post".into(),
        };

        let result = evaluate_budget_gate(&proposal, &policy);
        assert!(result.passed);
        assert!(result.requires_approval);
    }
}
