//! Recurring jobs set from chat and per-bot permission modes: an Ask first
//! bot's schedule waits for Start, a Trusted bot's starts at once, the
//! owner adds, starts, stops and deletes schedules, and a due schedule runs
//! once per slot as one of the bot's jobs. Real router and database; a
//! scripted local AI plays the bot.

mod common;

use std::sync::Arc;

use airobot::config::Config;
use airobot::local::{LocalAi, LocalAiRequest, LocalMode};
use airobot::office::recurring;
use airobot::state::AppState;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::{send_with_state, session_cookie};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const BASE: &str = "http://127.0.0.1:43135";
const HOST: &str = "127.0.0.1:43135";
const TOKEN: &str = "recurring-test-token";

fn local_state(pool: PgPool) -> AppState {
    let ai: LocalAi = Arc::new(move |req: LocalAiRequest| {
        Box::pin(async move {
            if req.system.contains("Reporting this job") {
                return Ok("Checked the inbox.\nOUTCOME: nothing to do — it was empty".to_string());
            }
            // The newest request is checked first: the history keeps both.
            if req.prompt.contains("every day at 7 check the inbox") {
                return Ok("Will do.\nEVERY day at 7: check the inbox".to_string());
            }
            if req
                .prompt
                .contains("every Monday at 9 list overdue invoices")
            {
                return Ok("Sure.\nEVERY Monday at 9: list overdue invoices".to_string());
            }
            Ok("Hello.".to_string())
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

async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    cookie: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let res = send_with_state(state.clone(), request(method, uri, Some(cookie), body)).await;
    (res.status, res.body)
}

async fn wait_for<T, F, Fut>(mut check: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    for _ in 0..100 {
        if let Some(found) = check().await {
            return Some(found);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    None
}

/// An office with one bot in its own room: (cookie, office, bot, room).
async fn setup(state: &AppState) -> (String, Uuid, Uuid, Uuid) {
    let res = send_with_state(
        state.clone(),
        request("GET", &format!("/local/enter?token={TOKEN}"), None, None),
    )
    .await;
    let cookie = session_cookie(&res.headers).expect("session");
    let (_, made) = call(
        state,
        "POST",
        "/api/offices",
        &cookie,
        Some(json!({ "name": "Shop" })),
    )
    .await;
    let office: Uuid = made["office"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let (status, made) = call(
        state,
        "POST",
        "/agents",
        &cookie,
        Some(json!({ "name": "Ledger", "handle": "ledger", "instructions": "Keep the books.", "workspace_id": office })),
    )
    .await;
    assert!(status.is_success(), "{made:?}");
    let bot: Uuid = made["agent"]["id"]
        .as_str()
        .expect("bot")
        .parse()
        .expect("uuid");
    let room: Uuid = sqlx::query_scalar(
        "SELECT t.id FROM threads t JOIN thread_agents ta ON ta.thread_id = t.id
         WHERE ta.agent_id = $1 AND t.workspace_id = $2 AND t.kind = 'room'",
    )
    .bind(bot)
    .bind(office)
    .fetch_one(&state.pool)
    .await
    .expect("the bot's room");
    (cookie, office, bot, room)
}

async fn say(state: &AppState, cookie: &str, room: Uuid, text: &str) {
    let (status, body) = call(
        state,
        "POST",
        &format!("/threads/{room}/notes"),
        cookie,
        Some(json!({ "body": text, "kind": "status" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
}

#[sqlx::test]
async fn schedules_set_from_chat_follow_the_bots_permission_mode(pool: PgPool) {
    let state = local_state(pool.clone());
    let (cookie, _, bot, room) = setup(&state).await;

    // Ask first (the default): proposed, with a Start button.
    say(
        &state,
        &cookie,
        room,
        "every Monday at 9 list overdue invoices",
    )
    .await;
    let (body, actions): (String, Option<Value>) = wait_for(|| async {
        sqlx::query_as(
            "SELECT body, actions FROM notes WHERE thread_id = $1 AND body LIKE '%Recurring job%'",
        )
        .bind(room)
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten()
    })
    .await
    .expect("the bot set a schedule");
    assert!(body.starts_with("Sure."), "{body}");
    assert!(!body.contains("EVERY"), "{body}");
    assert!(
        body.contains("every Monday at 09:00: list overdue invoices"),
        "{body}"
    );
    assert!(body.contains("starts when you press Start"), "{body}");
    let actions = actions.expect("buttons");
    assert_eq!(actions[0]["kind"], "start_job");
    let id = actions[0]["target"].as_str().expect("id").to_string();
    let (_, list) = call(
        &state,
        "GET",
        &format!("/agents/{bot}/schedules"),
        &cookie,
        None,
    )
    .await;
    assert_eq!(list["permission_mode"], "ask_first");
    assert_eq!(list["schedules"][0]["status"], "proposed");
    assert_eq!(list["schedules"][0]["source"], "bot");
    // The bot is told what it has, so it won't set it twice.
    let block = recurring::prompt_block(&pool, bot).await;
    assert!(
        block.contains("list overdue invoices (waiting for the owner's Start)"),
        "{block}"
    );

    // Start.
    let (status, started) = call(
        &state,
        "PATCH",
        &format!("/api/schedules/{id}"),
        &cookie,
        Some(json!({ "status": "active" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started:?}");
    assert_eq!(started["status"], "active");

    // Trusted: starts at once, with a Stop button.
    let (status, saved) = call(
        &state,
        "POST",
        &format!("/agents/{bot}/settings"),
        &cookie,
        Some(json!({ "permission_mode": "trusted" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved:?}");
    assert_eq!(saved["permission_mode"], "trusted");
    let (status, _) = call(
        &state,
        "POST",
        &format!("/agents/{bot}/settings"),
        &cookie,
        Some(json!({ "permission_mode": "yolo" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    say(&state, &cookie, room, "every day at 7 check the inbox").await;
    let actions: Option<Value> = wait_for(|| async {
        sqlx::query_scalar::<_, Option<Value>>(
            "SELECT actions FROM notes WHERE thread_id = $1 AND body LIKE '%Recurring job set, every day at 07:00%'",
        )
        .bind(room)
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten()
    })
    .await
    .expect("a trusted schedule");
    assert_eq!(actions.expect("buttons")[0]["kind"], "stop_job");
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM recurring_jobs WHERE agent_id = $1 AND status = 'active'",
    )
    .bind(bot)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(active, 2);
}

#[sqlx::test]
async fn the_owner_manages_schedules_and_due_ones_run_once(pool: PgPool) {
    let state = local_state(pool.clone());
    let (cookie, office, bot, room) = setup(&state).await;
    let uri = format!("/agents/{bot}/schedules?office={office}");
    for bad in [
        json!({ "job": "", "every": "day" }),
        json!({ "job": "x", "every": "hourly" }),
        json!({ "job": "x", "every": "week" }),
        json!({ "job": "x", "every": "month", "day_of_month": 31 }),
        json!({ "job": "x", "every": "day", "hour": 24 }),
    ] {
        let (status, _) = call(&state, "POST", &uri, &cookie, Some(bad.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}");
    }
    let (status, made) = call(
        &state,
        "POST",
        &uri,
        &cookie,
        Some(
            json!({ "job": "Check the inbox", "every": "day", "hour": 0, "idempotency_key": "s1" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made:?}");
    assert_eq!(made["status"], "active");
    assert_eq!(made["source"], "owner");
    assert_eq!(made["when"], "every day at 00:00");
    assert_eq!(made["thread_id"], room.to_string());
    let id: Uuid = made["id"].as_str().expect("id").parse().expect("uuid");
    let (_, again) = call(
        &state,
        "POST",
        &uri,
        &cookie,
        Some(json!({ "job": "Check the inbox", "every": "day", "idempotency_key": "s1" })),
    )
    .await;
    assert_eq!(again["id"], made["id"]);

    // Started after today's slot: it waits for tomorrow's.
    say(&state, &cookie, room, "hello").await;
    assert_eq!(recurring::run_due(&state).await.expect("tick"), 0);

    // Started yesterday: today's slot runs, once, as one of the bot's jobs.
    sqlx::query("UPDATE recurring_jobs SET started_at = now() - interval '1 day' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("age");
    assert_eq!(recurring::run_due(&state).await.expect("tick"), 1);
    assert_eq!(recurring::run_due(&state).await.expect("tick"), 0);
    let (title, outcome): (String, Option<String>) = wait_for(|| async {
        sqlx::query_as(
            "SELECT title, outcome FROM tasks WHERE assigned_agent_id = $1 AND executor = 'chat'",
        )
        .bind(bot)
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten()
    })
    .await
    .expect("the job ran");
    assert_eq!(title, "Check the inbox");
    assert_eq!(outcome.as_deref(), Some("nothing_to_do"));

    // A stopped office runs nothing.
    sqlx::query("UPDATE recurring_jobs SET last_run_at = NULL WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("reset");
    sqlx::query("UPDATE workspaces SET paused = true WHERE id = $1")
        .bind(office)
        .execute(&pool)
        .await
        .expect("stop");
    assert_eq!(recurring::run_due(&state).await.expect("tick"), 0);

    // Stop, start, delete.
    let (_, stopped) = call(
        &state,
        "PATCH",
        &format!("/api/schedules/{id}"),
        &cookie,
        Some(json!({ "status": "stopped" })),
    )
    .await;
    assert_eq!(stopped["status"], "stopped");
    let (status, _) = call(
        &state,
        "PATCH",
        &format!("/api/schedules/{id}"),
        &cookie,
        Some(json!({ "status": "proposed" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &state,
        "POST",
        &format!("/api/schedules/{id}/delete"),
        &cookie,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = call(
        &state,
        "GET",
        &format!("/agents/{bot}/schedules"),
        &cookie,
        None,
    )
    .await;
    assert!(list["schedules"].as_array().expect("list").is_empty());
    let stranger = Uuid::new_v4();
    let (status, _) = call(
        &state,
        "PATCH",
        &format!("/api/schedules/{stranger}"),
        &cookie,
        Some(json!({ "status": "active" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &state,
        "GET",
        &format!("/agents/{stranger}/schedules"),
        &cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
