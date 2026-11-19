// Billing: replies on the platform's AI key are metered, a reached limit
// produces one upgrade message instead of silence, and metered calls
// record what they really cost.
mod common;

use std::sync::Arc;

use airobot::auth::secret;
use airobot::config::Config;
use airobot::office::budget_gate::{self, ActionProposal};
use airobot::openrouter::Usage;
use airobot::state::AppState;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::{
    authed_json_request, form_request, get_request, send, send_with_state, session_cookie,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

const UPGRADE: &str = "https://example.com/upgrade";

/// A state whose server has a platform AI key and an upgrade page. The key
/// is never sent anywhere here: every request in these tests is refused by
/// the budget gate before a provider call.
fn metered_state(pool: PgPool) -> AppState {
    let mut s = common::state(pool);
    let mut config: Config = (*s.config).clone();
    config.openrouter_api_key = Some("platform-key-not-used".to_string());
    config.upgrade_url = Some(UPGRADE.to_string());
    s.config = Arc::new(config);
    s
}

async fn sign_in(pool: &PgPool, email: &str, handle: &str) -> String {
    let id = Uuid::new_v4();
    let raw_secret = secret::random_hex_token(32);
    let token_hash = secret::hash_secret(&raw_secret).expect("hash");
    sqlx::query(
        "INSERT INTO email_tokens (id, email, token_hash, purpose, expires_at)
         VALUES ($1, $2, $3, 'login', $4)",
    )
    .bind(id)
    .bind(email)
    .bind(&token_hash)
    .bind(Utc::now() + Duration::minutes(15))
    .execute(pool)
    .await
    .expect("insert email token");
    let raw_token = secret::encode_keyed_secret("", id, &raw_secret);
    let res = send(
        pool.clone(),
        get_request(&format!("/auth/callback?token={raw_token}"), None),
    )
    .await;
    let cookie = session_cookie(&res.headers).expect("session cookie");
    let set = send(
        pool.clone(),
        form_request(
            "POST",
            "/onboarding/handle",
            &[("handle", handle)],
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(set.status, StatusCode::OK);
    cookie
}

/// A thread with one agent (no key of its own) listening in it.
async fn thread_with_agent(state: &AppState, cookie: &str) -> Uuid {
    let pool = state.pool.clone();
    let agent = send_with_state(
        state.clone(),
        authed_json_request(
            "POST",
            "/agents",
            json!({ "name": "Cactus", "handle": "cactus", "instructions": "You are Cactus." }),
            Some(cookie),
        ),
    )
    .await;
    let agent_id: Uuid = agent.body["agent"]["id"]
        .as_str()
        .expect("agent id")
        .parse()
        .expect("uuid");
    let thread = send_with_state(
        state.clone(),
        authed_json_request("POST", "/threads", json!({ "title": "Chat" }), Some(cookie)),
    )
    .await;
    let thread_id: Uuid = thread.body["id"].as_str().expect("id").parse().expect("u");
    sqlx::query("INSERT INTO thread_agents (thread_id, agent_id) VALUES ($1, $2)")
        .bind(thread_id)
        .bind(agent_id)
        .execute(&pool)
        .await
        .expect("link agent");
    thread_id
}

async fn say(state: &AppState, cookie: &str, thread_id: Uuid, body: &str) {
    let res = send_with_state(
        state.clone(),
        authed_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({ "body": body, "kind": "command" }),
            Some(cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED);
}

async fn limit_notes(pool: &PgPool, thread_id: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT body FROM notes
         WHERE thread_id = $1 AND author_agent_id IS NOT NULL AND body LIKE '%AI credit limit%'
         ORDER BY created_at",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .expect("notes")
}

async fn wait_for_limit_note(pool: &PgPool, thread_id: Uuid) -> Vec<String> {
    for _ in 0..60 {
        let notes = limit_notes(pool, thread_id).await;
        if !notes.is_empty() {
            return notes;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Vec::new()
}

#[sqlx::test]
async fn platform_key_reply_at_the_limit_says_upgrade_once_and_charges_nothing(pool: PgPool) {
    let state = metered_state(pool.clone());
    let cookie = sign_in(&pool, "capped@example.com", "capped").await;
    let thread_id = thread_with_agent(&state, &cookie).await;

    // Provision the wallet, then spend it down to less than one reply.
    let wallet = send_with_state(
        state.clone(),
        authed_json_request("GET", "/api/office/wallet", json!({}), Some(&cookie)),
    )
    .await;
    assert_eq!(wallet.status, StatusCode::OK);
    assert!(wallet.body["limit_reached"].is_null());
    sqlx::query("UPDATE workspace_wallets SET credit_balance = 10")
        .execute(&pool)
        .await
        .expect("drain");

    say(&state, &cookie, thread_id, "How are you?").await;
    let notes = wait_for_limit_note(&pool, thread_id).await;
    assert_eq!(notes.len(), 1, "the agent explains instead of going quiet");
    assert!(
        notes[0].contains("used its AI credit limit"),
        "{}",
        notes[0]
    );
    assert!(notes[0].contains(UPGRADE), "links the upgrade page");
    assert!(notes[0].contains("own OpenRouter key"));

    // A second message within the hour does not repeat the message.
    say(&state, &cookie, thread_id, "Hello?").await;
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    assert_eq!(limit_notes(&pool, thread_id).await.len(), 1);

    // Nothing was charged and no provider call was paid for.
    let ledger: i64 = sqlx::query_scalar("SELECT count(*) FROM credit_ledger")
        .fetch_one(&pool)
        .await
        .expect("ledger");
    assert_eq!(ledger, 0);

    // The app can show the banner before anyone is refused.
    let wallet = send_with_state(
        state.clone(),
        authed_json_request("GET", "/api/office/wallet", json!({}), Some(&cookie)),
    )
    .await;
    assert_eq!(wallet.body["limit_reached"], "balance");
    assert_eq!(wallet.body["upgrade_url"], UPGRADE);
}

#[sqlx::test]
async fn the_daily_cap_names_tomorrow(pool: PgPool) {
    let state = metered_state(pool.clone());
    let cookie = sign_in(&pool, "daily@example.com", "daily").await;
    let thread_id = thread_with_agent(&state, &cookie).await;
    send_with_state(
        state.clone(),
        authed_json_request("GET", "/api/office/wallet", json!({}), Some(&cookie)),
    )
    .await;
    sqlx::query(
        "UPDATE workspace_wallets
         SET credits_burned_today = max_daily_credit_burn, burn_day = CURRENT_DATE",
    )
    .execute(&pool)
    .await
    .expect("burn");

    say(&state, &cookie, thread_id, "Still there?").await;
    let notes = wait_for_limit_note(&pool, thread_id).await;
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("today's AI credit limit"), "{}", notes[0]);
    assert!(notes[0].contains("resets tomorrow"));

    let wallet = send_with_state(
        state.clone(),
        authed_json_request("GET", "/api/office/wallet", json!({}), Some(&cookie)),
    )
    .await;
    assert_eq!(wallet.body["limit_reached"], "daily");
}

#[sqlx::test]
async fn without_a_platform_key_nothing_is_metered(pool: PgPool) {
    // The default test state has no platform key: an agent without its own
    // key keeps the old "needs an AI voice" note and the wallet is untouched.
    let state = common::state(pool.clone());
    let cookie = sign_in(&pool, "free@example.com", "free").await;
    let thread_id = thread_with_agent(&state, &cookie).await;
    say(&state, &cookie, thread_id, "Hi").await;
    let mut voice = 0;
    for _ in 0..60 {
        voice = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notes WHERE thread_id = $1 AND body LIKE '%needs an AI voice%'",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .expect("count");
        if voice > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(voice, 1);
    assert!(limit_notes(&pool, thread_id).await.is_empty());
    let ledger: i64 = sqlx::query_scalar("SELECT count(*) FROM credit_ledger")
        .fetch_one(&pool)
        .await
        .expect("ledger");
    assert_eq!(ledger, 0);
}

#[sqlx::test]
async fn a_metered_call_records_its_real_cost_on_the_charge(pool: PgPool) {
    let cookie = sign_in(&pool, "usage@example.com", "usage").await;
    let wallet = send(
        pool.clone(),
        authed_json_request("GET", "/api/office/wallet", json!({}), Some(&cookie)),
    )
    .await;
    let workspace_id: Uuid = wallet.body["workspace_id"]
        .as_str()
        .expect("workspace")
        .parse()
        .expect("uuid");

    let proposal = ActionProposal {
        provider: "openrouter".into(),
        model_or_tool: "test/model".into(),
        estimated_credits: budget_gate::FLAT_CALL_CREDITS,
        raw_cost_micros: 250,
        requires_manual_approval: false,
        action_type: "chat_reply".into(),
    };
    let gate = budget_gate::check_and_deduct(&pool, workspace_id, &proposal, None, false)
        .await
        .expect("gate");
    assert!(gate.passed);
    let ledger_id = gate.ledger_id.expect("the charge names its ledger row");

    budget_gate::record_usage(
        &pool,
        Some(ledger_id),
        &Usage {
            prompt_tokens: Some(1200),
            completion_tokens: Some(340),
            cost_micros: Some(4_100),
        },
    )
    .await;

    let (credits, cost, prompt, completion, recorded): (i64, i64, Option<i64>, Option<i64>, bool) =
        sqlx::query_as(
            "SELECT credits_charged, raw_cost_micros, prompt_tokens, completion_tokens,
                    usage_recorded_at IS NOT NULL
             FROM credit_ledger WHERE id = $1",
        )
        .bind(ledger_id)
        .fetch_one(&pool)
        .await
        .expect("row");
    assert_eq!(
        credits,
        budget_gate::FLAT_CALL_CREDITS,
        "pricing is unchanged"
    );
    assert_eq!(cost, 4_100, "the estimate is replaced by the real cost");
    assert_eq!((prompt, completion), (Some(1200), Some(340)));
    assert!(recorded);

    // A provider that reports no cost keeps the estimate rather than zero.
    let gate = budget_gate::check_and_deduct(&pool, workspace_id, &proposal, None, false)
        .await
        .expect("gate");
    budget_gate::record_usage(&pool, gate.ledger_id, &Usage::default()).await;
    let cost: i64 = sqlx::query_scalar("SELECT raw_cost_micros FROM credit_ledger WHERE id = $1")
        .bind(gate.ledger_id.expect("id"))
        .fetch_one(&pool)
        .await
        .expect("row");
    assert_eq!(cost, 250);
}
