//! Monthly spending caps: an office's and a bot's hard caps refuse a
//! metered call before it is made, a reached cap posts one card that month
//! (and the office's autonomous work waits), changing the cap lets work
//! run again, and spend shows per office and per bot. Real router and
//! database; the platform key is never sent anywhere because every call
//! here is refused or charged directly through the budget gate.

mod common;

use std::sync::Arc;

use airobot::auth::secret;
use airobot::config::Config;
use airobot::office::budget_gate::{self, ActionProposal, LimitHit, Spender};
use airobot::office::spend;
use airobot::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use common::{
    authed_json_request, form_request, get_request, send, send_with_state, session_cookie,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

fn metered_state(pool: PgPool) -> AppState {
    let mut s = common::state(pool);
    let mut config: Config = (*s.config).clone();
    config.openrouter_api_key = Some("platform-key-not-used".to_string());
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

async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    body: Value,
    cookie: &str,
) -> (StatusCode, Value) {
    let res = send_with_state(
        state.clone(),
        authed_json_request(method, uri, body, Some(cookie)),
    )
    .await;
    (res.status, res.body)
}

/// POST /agents/:id/settings as JSON.
async fn bot_settings(
    state: &AppState,
    cookie: &str,
    bot: Uuid,
    body: Value,
) -> (StatusCode, Value) {
    let res = send_with_state(
        state.clone(),
        Request::builder()
            .method("POST")
            .uri(format!("/agents/{bot}/settings"))
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .header("x-forwarded-for", "203.0.113.9")
            .header("cookie", cookie)
            .body(Body::from(body.to_string()))
            .expect("request"),
    )
    .await;
    (res.status, res.body)
}

fn proposal() -> ActionProposal {
    ActionProposal {
        provider: "openrouter".into(),
        model_or_tool: "test/model".into(),
        estimated_credits: budget_gate::FLAT_CALL_CREDITS,
        raw_cost_micros: 250,
        requires_manual_approval: false,
        action_type: "chat_reply".into(),
    }
}

async fn primary_office(state: &AppState, cookie: &str) -> (Uuid, Value) {
    let (status, list) = call(state, "GET", "/api/offices", json!({}), cookie).await;
    assert_eq!(status, StatusCode::OK);
    let office = list["offices"][0].clone();
    (
        office["id"].as_str().expect("id").parse().expect("uuid"),
        office,
    )
}

async fn new_bot(state: &AppState, cookie: &str, name: &str) -> Uuid {
    let (_, made) = call(
        state,
        "POST",
        "/agents",
        json!({ "name": name, "instructions": "Help." }),
        cookie,
    )
    .await;
    made["agent"]["id"]
        .as_str()
        .expect("bot")
        .parse()
        .expect("uuid")
}

#[sqlx::test]
async fn caps_refuse_calls_past_them_and_spend_is_counted(pool: PgPool) {
    let state = metered_state(pool.clone());
    let cookie = sign_in(&pool, "caps@example.com", "capsowner").await;
    let (office, item) = primary_office(&state, &cookie).await;
    assert!(item["monthly_cap_credits"].is_null());
    assert_eq!(item["month_spent_credits"], 0);
    assert_eq!(item["cap_reached"], false);
    let bot = new_bot(&state, &cookie, "Penny").await;

    // The owner sets a cap of two calls.
    let (status, saved) = call(
        &state,
        "PATCH",
        &format!("/api/offices/{office}"),
        json!({ "monthly_cap_credits": 50, "idempotency_key": "cap-1" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved:?}");
    assert_eq!(saved["monthly_cap_credits"], 50);
    for bad in [json!(-5), json!(1_000_000_000_i64)] {
        let (status, _) = call(
            &state,
            "PATCH",
            &format!("/api/offices/{office}"),
            json!({ "monthly_cap_credits": bad }),
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let nosy = sign_in(&pool, "nosy@example.com", "nosycap").await;
    let (status, _) = call(
        &state,
        "PATCH",
        &format!("/api/offices/{office}"),
        json!({ "monthly_cap_credits": 0 }),
        &nosy,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let spender = Spender::new(office, Some(bot));
    for _ in 0..2 {
        let gate = budget_gate::check_and_deduct(&pool, office, &proposal(), None, false, spender)
            .await
            .expect("gate");
        assert!(gate.passed, "{:?}", gate.reasons);
    }
    let gate = budget_gate::check_and_deduct(&pool, office, &proposal(), None, false, spender)
        .await
        .expect("gate");
    assert!(!gate.passed);
    assert_eq!(gate.limit, Some(LimitHit::MonthlyOffice));
    assert!(gate.ledger_id.is_none(), "nothing is charged");
    assert_eq!(spend::office_month_spent(&pool, office).await, 50);
    assert_eq!(spend::agent_month_spent(&pool, bot).await, 50);

    // A refund nets out for the office and the bot.
    budget_gate::refund_credit_charge(&pool, office, None, &proposal(), spender)
        .await
        .expect("refund");
    assert_eq!(spend::office_month_spent(&pool, office).await, 25);
    assert_eq!(spend::agent_month_spent(&pool, bot).await, 25);

    // The office list shows it.
    let (_, item) = primary_office(&state, &cookie).await;
    assert_eq!(item["monthly_cap_credits"], 50);
    assert_eq!(item["month_spent_credits"], 25);

    // A bot's own cap, below what it has spent.
    let (status, saved) = bot_settings(
        &state,
        &cookie,
        bot,
        json!({ "monthly_cap_credits": 30, "idempotency_key": "bot-cap" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved:?}");
    assert_eq!(saved["monthly_cap_credits"], 30);
    let gate = budget_gate::check_and_deduct(&pool, office, &proposal(), None, false, spender)
        .await
        .expect("gate");
    assert_eq!(gate.limit, Some(LimitHit::MonthlyBot));
    // Another bot in the same office still may (the office has room for one).
    let other = new_bot(&state, &cookie, "Otto").await;
    let gate = budget_gate::check_and_deduct(
        &pool,
        office,
        &proposal(),
        None,
        false,
        Spender::new(office, Some(other)),
    )
    .await
    .expect("gate");
    assert!(gate.passed, "{:?}", gate.reasons);

    // The bot's stats.
    let (status, stats) = call(
        &state,
        "GET",
        &format!("/agents/{bot}/stats"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stats:?}");
    assert_eq!(stats["month_spent"], 25);
    assert_eq!(stats["month_calls"], 2, "two charges, one refunded");
    assert_eq!(stats["monthly_cap"], 30);
    let daily = stats["daily"].as_array().expect("daily");
    assert_eq!(daily.len(), 30);
    assert_eq!(daily[29]["credits"], 25, "today is last");
    let (status, _) = call(
        &state,
        "GET",
        &format!("/agents/{bot}/stats"),
        json!({}),
        &nosy,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 0 removes the bot's cap.
    let (_, saved) = bot_settings(&state, &cookie, bot, json!({ "monthly_cap_credits": 0 })).await;
    assert!(saved["monthly_cap_credits"].is_null());
    let (status, _) =
        bot_settings(&state, &cookie, bot, json!({ "monthly_cap_credits": -1 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn a_reached_cap_posts_one_card_and_work_waits_until_it_changes(pool: PgPool) {
    let state = metered_state(pool.clone());
    let cookie = sign_in(&pool, "card@example.com", "cardowner").await;
    let (office, _) = primary_office(&state, &cookie).await;
    let bot = new_bot(&state, &cookie, "Cactus").await;
    // A room with the bot in it.
    let (_, thread) = call(
        &state,
        "POST",
        "/threads",
        json!({ "title": "Chat" }),
        &cookie,
    )
    .await;
    let thread_id: Uuid = thread["id"].as_str().expect("id").parse().expect("uuid");
    sqlx::query("INSERT INTO thread_agents (thread_id, agent_id) VALUES ($1, $2)")
        .bind(thread_id)
        .bind(bot)
        .execute(&pool)
        .await
        .expect("link agent");

    // A cap below one reply: the owner's message is refused before any AI call.
    call(
        &state,
        "PATCH",
        &format!("/api/offices/{office}"),
        json!({ "monthly_cap_credits": 10 }),
        &cookie,
    )
    .await;
    let res = send_with_state(
        state.clone(),
        authed_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({ "body": "How are you?", "kind": "command" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED);

    // One card in the office's group chat, with a button to raise the cap...
    let mut card: Option<(String, Option<Value>)> = None;
    for _ in 0..60 {
        card = sqlx::query_as(
            "SELECT n.body, n.actions FROM notes n JOIN threads t ON t.id = n.thread_id
             WHERE t.workspace_id = $1 AND t.kind = 'office' AND n.body LIKE '%monthly spending cap%'",
        )
        .bind(office)
        .fetch_optional(&pool)
        .await
        .expect("notes");
        if card.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let (body, actions) = card.expect("a cap card in the office chat");
    assert!(body.contains("of 10 credits"), "{body}");
    let actions = actions.expect("actions");
    assert_eq!(actions[0]["target"], "office");
    // ...and a short note in the room where the reply was refused.
    let mut note: Option<String> = None;
    for _ in 0..60 {
        note = sqlx::query_scalar(
            "SELECT body FROM notes WHERE thread_id = $1 AND author_agent_id IS NOT NULL",
        )
        .bind(thread_id)
        .fetch_optional(&pool)
        .await
        .expect("notes");
        if note.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let note = note.expect("a note in the room");
    assert!(note.contains("monthly spending cap"), "{note}");
    let charged: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM credit_ledger WHERE office_id = $1 AND credits_charged > 0",
    )
    .bind(office)
    .fetch_one(&pool)
    .await
    .expect("ledger");
    assert_eq!(charged, 0, "nothing was charged");

    // The office's autonomous work now waits, and the card isn't repeated.
    assert!(spend::office_capped(&pool, office).await);
    let (_, item) = primary_office(&state, &cookie).await;
    assert_eq!(item["cap_reached"], true);
    let again = spend::notify_cap(
        &state,
        LimitHit::MonthlyOffice,
        Spender::new(office, Some(bot)),
        Some(thread_id),
    )
    .await;
    assert!(again.is_none(), "one card a month");

    // Raising the cap lets work run again.
    call(
        &state,
        "PATCH",
        &format!("/api/offices/{office}"),
        json!({ "monthly_cap_credits": 1000 }),
        &cookie,
    )
    .await;
    assert!(!spend::office_capped(&pool, office).await);
    let (_, item) = primary_office(&state, &cookie).await;
    assert_eq!(item["cap_reached"], false);

    // A bot's own cap: its card goes to the room it was refused in.
    bot_settings(&state, &cookie, bot, json!({ "monthly_cap_credits": 5 })).await;
    let posted = spend::notify_cap(
        &state,
        LimitHit::MonthlyBot,
        Spender::new(office, Some(bot)),
        Some(thread_id),
    )
    .await;
    assert_eq!(posted, Some(thread_id));
    let actions: Option<Value> = sqlx::query_scalar(
        "SELECT actions FROM notes WHERE thread_id = $1 AND body LIKE '%Cactus reached its monthly spending cap%'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("bot card");
    assert_eq!(actions.expect("actions")[0]["target"], format!("bot:{bot}"));
}
