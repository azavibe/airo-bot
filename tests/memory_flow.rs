//! An office's memory: facts the owner keeps (office-wide or one bot's),
//! lessons from approvals, the goal and "about" text, and what a bot is
//! told: all of that plus the latest messages from the office's other
//! rooms. A bot saves a fact by writing `REMEMBER:`, and the owner can undo
//! it from the chat. Real router, real database; a scripted local AI
//! stands in for the person's AI tool.

mod common;

use std::sync::Arc;

use airobot::auth::secret;
use airobot::config::Config;
use airobot::local::{LocalAi, LocalAiRequest, LocalMode};
use airobot::state::AppState;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use chrono::{Duration, Utc};
use common::{
    authed_json_request, form_request, get_request, send, send_with_state, session_cookie,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

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
    pool: &PgPool,
    method: &str,
    uri: &str,
    body: Value,
    cookie: &str,
) -> (StatusCode, Value) {
    let res = send(
        pool.clone(),
        authed_json_request(method, uri, body, Some(cookie)),
    )
    .await;
    (res.status, res.body)
}

async fn new_office(pool: &PgPool, cookie: &str, name: &str) -> String {
    let (status, body) = call(
        pool,
        "POST",
        "/api/offices",
        json!({ "name": name }),
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    body["office"]["id"].as_str().expect("id").to_string()
}

#[sqlx::test]
async fn the_owner_keeps_edits_and_forgets_office_memory(pool: PgPool) {
    let cookie = sign_in(&pool, "memory@example.com", "rememberer").await;
    let office = new_office(&pool, &cookie, "Bakery").await;
    let uri = format!("/api/offices/{office}/memory");

    let (status, fact) = call(
        &pool,
        "POST",
        &uri,
        json!({ "body": "We close at 6pm on Saturdays.", "idempotency_key": "m-1" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{fact:?}");
    assert_eq!(fact["source"], "owner");
    assert!(fact["agent_id"].is_null(), "the whole office knows it");
    let fact_id = fact["id"].as_str().expect("id").to_string();
    // The same key returns the same fact.
    let (_, again) = call(
        &pool,
        "POST",
        &uri,
        json!({ "body": "We close at 6pm on Saturdays.", "idempotency_key": "m-1" }),
        &cookie,
    )
    .await;
    assert_eq!(again["id"], fact["id"]);

    // One bot's own note.
    let (_, made) = call(
        &pool,
        "POST",
        "/agents",
        json!({ "name": "Baker", "instructions": "Bake.", "workspace_id": office }),
        &cookie,
    )
    .await;
    let bot = made["agent"]["id"].as_str().expect("bot").to_string();
    let (status, own) = call(
        &pool,
        "POST",
        &uri,
        json!({ "body": "Sourdough needs 24 hours.", "agent_id": bot }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{own:?}");
    assert_eq!(own["agent_id"], bot.as_str());

    // Pin and edit.
    let (status, pinned) = call(
        &pool,
        "PATCH",
        &format!("/api/memories/{fact_id}"),
        json!({ "pinned": true, "body": "We close at 6pm on Saturdays and Sundays." }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pinned:?}");
    assert_eq!(pinned["pinned"], true);
    assert_eq!(pinned["body"], "We close at 6pm on Saturdays and Sundays.");
    let (_, list) = call(&pool, "GET", &uri, json!({}), &cookie).await;
    let memories = list["memories"].as_array().expect("memories");
    assert_eq!(memories.len(), 2);
    assert_eq!(memories[0]["id"], fact_id.as_str(), "pinned first");

    // Nobody else reads or changes it.
    let other = sign_in(&pool, "other@example.com", "otherone").await;
    let (status, _) = call(&pool, "GET", &uri, json!({}), &other).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&pool, "POST", &uri, json!({ "body": "Spam" }), &other).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/memories/{fact_id}/delete"),
        json!({}),
        &other,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Someone else's bot can't be named.
    let (_, theirs) = call(&pool, "POST", "/agents", json!({ "name": "Spy" }), &other).await;
    let spy = theirs["agent"]["id"].as_str().expect("spy").to_string();
    let (status, _) = call(
        &pool,
        "POST",
        &uri,
        json!({ "body": "x", "agent_id": spy }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Empty or too long facts are refused.
    let (status, _) = call(&pool, "POST", &uri, json!({ "body": "  " }), &cookie).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &pool,
        "POST",
        &uri,
        json!({ "body": "x".repeat(501) }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Forget (soft delete).
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/memories/{fact_id}/delete"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = call(&pool, "GET", &uri, json!({}), &cookie).await;
    assert_eq!(list["memories"].as_array().map(Vec::len), Some(1));
    let kept: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM memories WHERE id = $1::uuid")
            .bind(&fact_id)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert!(kept.is_some(), "soft-deleted, not erased");

    // A full office says so instead of losing facts silently.
    sqlx::query(
        "INSERT INTO memories (workspace_id, body)
         SELECT $1::uuid, 'fact ' || n FROM generate_series(1, 199) AS n",
    )
    .bind(&office)
    .execute(&pool)
    .await
    .expect("fill");
    let (status, full) = call(&pool, "POST", &uri, json!({ "body": "One more" }), &cookie).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{full:?}");
}

#[sqlx::test]
async fn lessons_show_in_memory_and_can_be_dropped(pool: PgPool) {
    let cookie = sign_in(&pool, "lessons@example.com", "learner").await;
    let office = new_office(&pool, &cookie, "Clinic").await;
    let lesson: Uuid = sqlx::query_scalar(
        "INSERT INTO workspace_lessons (workspace_id, rule, source)
         VALUES ($1::uuid, 'Never offer discounts over 10%', 'approval_rejection') RETURNING id",
    )
    .bind(&office)
    .fetch_one(&pool)
    .await
    .expect("lesson");
    let (_, list) = call(
        &pool,
        "GET",
        &format!("/api/offices/{office}/memory"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(list["lessons"][0]["rule"], "Never offer discounts over 10%");

    let other = sign_in(&pool, "nope@example.com", "nope").await;
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/lessons/{lesson}/delete"),
        json!({}),
        &other,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/lessons/{lesson}/delete"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = call(
        &pool,
        "GET",
        &format!("/api/offices/{office}/memory"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(list["lessons"].as_array().map(Vec::len), Some(0));
}

#[sqlx::test]
async fn a_bot_is_told_the_goal_memory_lessons_and_other_rooms(pool: PgPool) {
    let cookie = sign_in(&pool, "told@example.com", "teller").await;
    let office = new_office(&pool, &cookie, "Bakery").await;
    let (status, saved) = call(
        &pool,
        "PATCH",
        &format!("/api/offices/{office}"),
        json!({
            "goal": "Open a second shop by summer",
            "about": "Open 7am-6pm.\nCakes need two days' notice.",
        }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved:?}");
    assert_eq!(saved["goal"], "Open a second shop by summer");
    assert_eq!(
        saved["about"],
        "Open 7am-6pm.\nCakes need two days' notice."
    );
    let (_, list) = call(&pool, "GET", "/api/offices", json!({}), &cookie).await;
    let mine = list["offices"]
        .as_array()
        .expect("offices")
        .iter()
        .find(|o| o["id"] == office.as_str())
        .cloned()
        .expect("office");
    assert_eq!(mine["goal"], "Open a second shop by summer");
    let (status, _) = call(
        &pool,
        "PATCH",
        &format!("/api/offices/{office}"),
        json!({ "goal": "x".repeat(301) }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let bot = |name: &'static str| {
        let pool = pool.clone();
        let cookie = cookie.clone();
        let office = office.clone();
        async move {
            let (_, made) = call(
                &pool,
                "POST",
                "/agents",
                json!({ "name": name, "instructions": "Work.", "workspace_id": office }),
                &cookie,
            )
            .await;
            (
                made["agent"]["id"]
                    .as_str()
                    .expect("id")
                    .parse::<Uuid>()
                    .expect("uuid"),
                made["room_id"]
                    .as_str()
                    .expect("room")
                    .parse::<Uuid>()
                    .expect("uuid"),
            )
        }
    };
    let (sales, sales_room) = bot("Sales").await;
    let (finance, finance_room) = bot("Money").await;
    let memory = format!("/api/offices/{office}/memory");
    call(
        &pool,
        "POST",
        &memory,
        json!({ "body": "Our best seller is rye bread." }),
        &cookie,
    )
    .await;
    call(
        &pool,
        "POST",
        &memory,
        json!({ "body": "Invoices go out on Fridays.", "agent_id": finance.to_string() }),
        &cookie,
    )
    .await;
    sqlx::query(
        "INSERT INTO workspace_lessons (workspace_id, rule) VALUES ($1::uuid, 'Always quote prices with VAT')",
    )
    .bind(&office)
    .execute(&pool)
    .await
    .expect("lesson");
    // Something happened in the finance room; something in the sales room.
    for (room, text) in [
        (finance_room, "Invoice 1042 for Rosa's Cafe is overdue"),
        (sales_room, "Note in this very room"),
    ] {
        let (status, _) = call(
            &pool,
            "POST",
            &format!("/threads/{room}/notes"),
            json!({ "body": text, "kind": "status" }),
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let told = airobot::office::context::prompt_for_thread(&pool, sales_room, sales)
        .await
        .expect("office prompt");
    assert!(
        told.contains("Where the business is going: Open a second shop by summer."),
        "{told}"
    );
    assert!(told.contains("Cakes need two days' notice."), "{told}");
    assert!(told.contains("- Our best seller is rye bread."), "{told}");
    assert!(
        !told.contains("Invoices go out on Fridays."),
        "another bot's own note: {told}"
    );
    assert!(told.contains("Always quote prices with VAT"), "{told}");
    assert!(
        told.contains("[Money] Owner: Invoice 1042 for Rosa's Cafe is overdue"),
        "{told}"
    );
    assert!(
        !told.contains("Note in this very room"),
        "its own room is in its history already"
    );
    assert!(told.contains("REMEMBER: "), "{told}");

    let finance_told = airobot::office::context::prompt_for_thread(&pool, finance_room, finance)
        .await
        .expect("office prompt");
    assert!(
        finance_told.contains("Invoices go out on Fridays."),
        "{finance_told}"
    );
}

// ---------- A bot saves a fact in a real reply ----------

const BASE: &str = "http://127.0.0.1:43131";
const HOST: &str = "127.0.0.1:43131";
const TOKEN: &str = "memory-test-token";

fn local_state(pool: PgPool) -> AppState {
    let ai: LocalAi = Arc::new(move |_req: LocalAiRequest| {
        Box::pin(async move {
            Ok(
                "Noted, I'll keep that in mind.\nREMEMBER: We close at 6pm on Saturdays."
                    .to_string(),
            )
        })
    });
    let mut s = common::state(pool);
    let mut config: Config = (*s.config).clone();
    config.app_base_url = BASE.to_string();
    config.local = Some(LocalMode {
        token: TOKEN.to_string(),
    });
    s.config = Arc::new(config);
    s.local_ai = Some(ai);
    s
}

fn request(method: &str, uri: &str, cookie: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, HOST)
        .header("x-forwarded-for", "127.0.0.1")
        .header(header::ACCEPT, "application/json");
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .expect("request"),
        None => b.body(Body::empty()).expect("request"),
    }
}

#[sqlx::test]
async fn a_fact_a_bot_saves_is_shown_and_can_be_undone(pool: PgPool) {
    let state = local_state(pool.clone());
    let res = send_with_state(
        state.clone(),
        request("GET", &format!("/local/enter?token={TOKEN}"), None, None),
    )
    .await;
    let cookie = session_cookie(&res.headers).expect("session");
    let room: Uuid = sqlx::query_scalar("SELECT id FROM threads WHERE title = '#assistant'")
        .fetch_one(&pool)
        .await
        .expect("assistant room");
    let res = send_with_state(
        state.clone(),
        request(
            "POST",
            &format!("/threads/{room}/notes"),
            Some(&cookie),
            Some(json!({ "body": "We close at 6pm on Saturdays.", "kind": "status" })),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text);

    let mut reply: Option<(String, Option<Value>)> = None;
    for _ in 0..80 {
        reply = sqlx::query_as(
            "SELECT body, actions FROM notes WHERE thread_id = $1 AND author_agent_id IS NOT NULL
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(room)
        .fetch_optional(&pool)
        .await
        .expect("notes");
        if reply.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let (body, actions) = reply.expect("the bot answered");
    assert!(body.starts_with("Noted, I'll keep that in mind."), "{body}");
    assert!(
        body.contains("*Remembered: We close at 6pm on Saturdays.*"),
        "{body}"
    );
    assert!(!body.contains("REMEMBER:"), "{body}");
    let actions = actions.expect("an Undo button");
    assert_eq!(actions[0]["label"], "Undo");
    assert_eq!(actions[0]["kind"], "forget");
    let memory_id = actions[0]["target"]
        .as_str()
        .expect("memory id")
        .to_string();
    let (source, deleted): (String, Option<chrono::DateTime<Utc>>) =
        sqlx::query_as("SELECT source, deleted_at FROM memories WHERE id = $1::uuid")
            .bind(&memory_id)
            .fetch_one(&pool)
            .await
            .expect("memory row");
    assert_eq!(source, "bot");
    assert!(deleted.is_none());

    // Undo from the chat forgets it.
    let res = send_with_state(
        state.clone(),
        request(
            "POST",
            &format!("/api/memories/{memory_id}/delete"),
            Some(&cookie),
            Some(json!({})),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.text);
    let deleted: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM memories WHERE id = $1::uuid")
            .bind(&memory_id)
            .fetch_one(&pool)
            .await
            .expect("memory row");
    assert!(deleted.is_some());
}
