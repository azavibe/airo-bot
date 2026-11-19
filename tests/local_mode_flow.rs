// Local mode: Airobot inside the desktop app, for the person at this
// computer, answered by their own AI tool, with no account or credits.
mod common;

use std::sync::{Arc, Mutex};

use airobot::config::Config;
use airobot::local::{LocalAi, LocalAiRequest, LocalMode, LOCAL_OWNER_EMAIL};
use airobot::state::AppState;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::{send_with_state, session_cookie, TestResponse};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

const BASE: &str = "http://127.0.0.1:43123";
const HOST: &str = "127.0.0.1:43123";
const TOKEN: &str = "launch-secret-for-this-test";

/// What the fake local AI was asked (system, prompt, the bot's tool and
/// model), and a reply that shows it.
type Seen = Arc<Mutex<Vec<(String, String, Option<String>, Option<String>)>>>;

fn local_state(pool: PgPool) -> (AppState, Seen) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let ai: LocalAi = Arc::new(move |req: LocalAiRequest| {
        let log = Arc::clone(&log);
        Box::pin(async move {
            log.lock().unwrap().push((
                req.system.clone(),
                req.prompt.clone(),
                req.engine.clone(),
                req.model.clone(),
            ));
            Ok("Answered on this computer.".to_string())
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
    (s, seen)
}

fn request(
    method: &str,
    uri: &str,
    host: &str,
    cookie: Option<&str>,
    body: Option<serde_json::Value>,
) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, host)
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

async fn enter(state: &AppState, token: &str) -> TestResponse {
    send_with_state(
        state.clone(),
        request(
            "GET",
            &format!("/local/enter?token={token}"),
            HOST,
            None,
            None,
        ),
    )
    .await
}

#[sqlx::test]
async fn the_window_signs_in_the_owner_and_nothing_else_can(pool: PgPool) {
    let (state, _) = local_state(pool.clone());

    // Wrong or missing token: refused, nobody created.
    assert_eq!(enter(&state, "guess").await.status, StatusCode::FORBIDDEN);
    assert_eq!(enter(&state, "").await.status, StatusCode::FORBIDDEN);

    // The window's secret: signs in and opens the app.
    let ok = enter(&state, TOKEN).await;
    assert!(ok.status.is_redirection(), "{}", ok.status);
    let cookie = session_cookie(&ok.headers).expect("session");

    // Re-entering (next launch) reuses the same owner.
    enter(&state, TOKEN).await;
    let owners: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE email = $1")
        .bind(LOCAL_OWNER_EMAIL)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(owners, 1);

    let me = send_with_state(
        state.clone(),
        request("GET", "/api/me", HOST, Some(&cookie), None),
    )
    .await;
    assert_eq!(me.body["authenticated"], true);
    assert_eq!(me.body["email"], LOCAL_OWNER_EMAIL);
    assert_eq!(me.body["local_mode"], true);
    assert_eq!(me.body["is_admin"], true);

    // A page that reaches this port under another name (DNS rebinding) is
    // turned away before any handler runs, even with the session cookie.
    let rebound = send_with_state(
        state.clone(),
        request("GET", "/api/me", "evil.example:43123", Some(&cookie), None),
    )
    .await;
    assert_eq!(rebound.status, StatusCode::FORBIDDEN);
    let other_port = send_with_state(
        state.clone(),
        request("GET", "/api/me", "127.0.0.1:9999", None, None),
    )
    .await;
    assert_eq!(other_port.status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn chats_and_office_tasks_use_the_computers_ai(pool: PgPool) {
    let (state, seen) = local_state(pool.clone());
    let cookie = session_cookie(&enter(&state, TOKEN).await.headers).expect("session");

    // A character agent in a thread.
    let agent = send_with_state(
        state.clone(),
        request(
            "POST",
            "/agents",
            HOST,
            Some(&cookie),
            Some(json!({ "name": "Cactus", "handle": "cactus", "instructions": "You are Cactus, a calm plant." })),
        ),
    )
    .await;
    let agent_id: Uuid = agent.body["agent"]["id"]
        .as_str()
        .expect("agent")
        .parse()
        .expect("uuid");
    let thread = send_with_state(
        state.clone(),
        request(
            "POST",
            "/threads",
            HOST,
            Some(&cookie),
            Some(json!({ "title": "Garden" })),
        ),
    )
    .await;
    let thread_id: Uuid = thread.body["id"]
        .as_str()
        .expect("thread")
        .parse()
        .expect("uuid");
    sqlx::query("INSERT INTO thread_agents (thread_id, agent_id) VALUES ($1, $2)")
        .bind(thread_id)
        .bind(agent_id)
        .execute(&pool)
        .await
        .expect("link");

    let said = send_with_state(
        state.clone(),
        request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            HOST,
            Some(&cookie),
            Some(json!({ "body": "How is the soil?", "kind": "command" })),
        ),
    )
    .await;
    assert_eq!(said.status, StatusCode::CREATED, "{}", said.text);

    let mut reply = None;
    for _ in 0..60 {
        reply = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT author_agent_id, body FROM notes WHERE thread_id = $1 AND author_agent_id IS NOT NULL",
        )
        .bind(thread_id)
        .fetch_optional(&pool)
        .await
        .expect("notes");
        if reply.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let (author, body) = reply.expect("a reply from the local AI");
    assert_eq!(author, agent_id, "answered as the character");
    assert_eq!(body, "Answered on this computer.");
    {
        let calls = seen.lock().unwrap();
        let (system, prompt, _, _) = calls.first().expect("asked");
        assert!(system.contains("Cactus"), "character goes to the AI");
        assert!(
            prompt.contains("How is the soil?"),
            "conversation goes to the AI"
        );
    }

    // An office task run by the server's dispatcher also uses it.
    let wallet = send_with_state(
        state.clone(),
        request("GET", "/api/office/wallet", HOST, Some(&cookie), None),
    )
    .await;
    let workspace: Uuid = wallet.body["workspace_id"]
        .as_str()
        .expect("workspace")
        .parse()
        .expect("uuid");
    let task_id: Uuid = sqlx::query_scalar(
        "INSERT INTO tasks (workspace_id, title, executor) VALUES ($1, 'Draft a welcome note', 'server') RETURNING id",
    )
    .bind(workspace)
    .fetch_one(&pool)
    .await
    .expect("task");
    airobot::office::fsm_dispatcher::run_dispatcher_once(&state)
        .await
        .expect("tick");
    let (status, payload): (String, serde_json::Value) =
        sqlx::query_as("SELECT status, state_payload FROM tasks WHERE id = $1")
            .bind(task_id)
            .fetch_one(&pool)
            .await
            .expect("task row");
    assert_eq!(status, "completed");
    assert_eq!(payload["result"], "Answered on this computer.");

    // Nothing was charged: no platform key, no credits in local mode.
    let ledger: i64 = sqlx::query_scalar("SELECT count(*) FROM credit_ledger")
        .fetch_one(&pool)
        .await
        .expect("ledger");
    assert_eq!(ledger, 0);
}

#[sqlx::test]
async fn the_hosted_service_has_no_local_door(pool: PgPool) {
    let state = common::state(pool.clone());
    let res = send_with_state(
        state.clone(),
        request(
            "GET",
            &format!("/local/enter?token={TOKEN}"),
            "localhost:8080",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&pool)
        .await
        .expect("users");
    assert_eq!(users, 0);
}

/// Waits for the n-th note a bot posts in a room (0 = the first).
async fn bot_reply(pool: &PgPool, thread_id: Uuid, nth: usize) -> Option<(Uuid, String)> {
    for _ in 0..60 {
        let replies = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT author_agent_id, body FROM notes
             WHERE thread_id = $1 AND author_agent_id IS NOT NULL
             ORDER BY created_at",
        )
        .bind(thread_id)
        .fetch_all(pool)
        .await
        .expect("notes");
        if replies.len() > nth {
            return replies.into_iter().nth(nth);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    None
}

/// Sends a message the way the chat box does.
async fn say(state: &AppState, cookie: &str, thread_id: Uuid, text: &str) {
    let said = send_with_state(
        state.clone(),
        request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            HOST,
            Some(cookie),
            Some(json!({ "body": text, "kind": "status" })),
        ),
    )
    .await;
    assert_eq!(said.status, StatusCode::CREATED, "{}", said.text);
}

async fn room(pool: &PgPool, title: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM threads WHERE title = $1 AND deleted_at IS NULL")
        .bind(title)
        .fetch_one(pool)
        .await
        .expect("room")
}

/// A fresh install can chat straight away with a general Assistant, every
/// office channel has a bot that answers, and each bot can be set to its
/// own tool and model on this computer.
#[sqlx::test]
async fn a_fresh_install_can_chat_and_each_bot_picks_its_ai(pool: PgPool) {
    let (state, seen) = local_state(pool.clone());
    let cookie = session_cookie(&enter(&state, TOKEN).await.headers).expect("session");
    // The next launch must not make a second Assistant or room.
    enter(&state, TOKEN).await;
    let rooms: i64 = sqlx::query_scalar("SELECT count(*) FROM threads WHERE title = '#assistant'")
        .fetch_one(&pool)
        .await
        .expect("rooms");
    assert_eq!(rooms, 1);

    // The Assistant's room is there, staffed, and answers anything. It
    // starts empty: a stored greeting would land in agents' inboxes.
    let assistant_room = room(&pool, "#assistant").await;
    let assistant: Uuid =
        sqlx::query_scalar("SELECT agent_id FROM thread_agents WHERE thread_id = $1")
            .bind(assistant_room)
            .fetch_one(&pool)
            .await
            .expect("the Assistant is in its room");
    let notes: i64 = sqlx::query_scalar("SELECT count(*) FROM notes WHERE thread_id = $1")
        .bind(assistant_room)
        .fetch_one(&pool)
        .await
        .expect("notes");
    assert_eq!(notes, 0);
    say(&state, &cookie, assistant_room, "hi").await;
    let (author, body) = bot_reply(&pool, assistant_room, 0).await.expect("answer");
    assert_eq!(author, assistant);
    assert_eq!(body, "Answered on this computer.");
    {
        let calls = seen.lock().unwrap();
        let (system, _, engine, model) = calls.last().expect("asked");
        assert!(system.contains("Assistant"), "{system}");
        assert_eq!(
            (engine, model),
            (&None, &None),
            "no choice: computer default"
        );
    }

    // The bot list shows its character and AI settings.
    let bots = send_with_state(
        state.clone(),
        request("GET", "/agents", HOST, Some(&cookie), None),
    )
    .await;
    let listed = bots.body["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .find(|a| a["name"] == "Assistant")
        .cloned()
        .expect("assistant listed");
    assert!(listed["instructions"]
        .as_str()
        .unwrap_or("")
        .contains("general-purpose"));
    assert!(listed["ai_engine"].is_null());

    // Choose Codex with a model for the Assistant only.
    let set = send_with_state(
        state.clone(),
        request(
            "POST",
            &format!("/agents/{assistant}/settings"),
            HOST,
            Some(&cookie),
            Some(
                json!({ "ai_engine": "codex", "ai_model": "gpt-test", "idempotency_key": "set-1" }),
            ),
        ),
    )
    .await;
    assert_eq!(set.status, StatusCode::OK, "{}", set.text);
    assert_eq!(set.body["ai_engine"], "codex");
    assert_eq!(set.body["ai_model"], "gpt-test");
    say(&state, &cookie, assistant_room, "and now?").await;
    bot_reply(&pool, assistant_room, 1)
        .await
        .expect("second answer");
    {
        let calls = seen.lock().unwrap();
        let (_, _, engine, model) = calls.last().expect("asked");
        assert_eq!(engine.as_deref(), Some("codex"));
        assert_eq!(model.as_deref(), Some("gpt-test"));
    }

    // Setting up the office staffs its channels: the Architect answers.
    let onboard = send_with_state(
        state.clone(),
        request(
            "POST",
            "/api/office/onboard",
            HOST,
            Some(&cookie),
            Some(json!({ "business_type": "Roofing", "goals": "More leads", "replaced_saas": [] })),
        ),
    )
    .await;
    assert_eq!(onboard.status, StatusCode::OK, "{}", onboard.text);
    let architect_room = room(&pool, "#office").await;
    // The kickoff: intro, three plans and three first tasks.
    bot_reply(&pool, architect_room, 6)
        .await
        .expect("the kickoff finishes");
    let staff: Vec<String> = sqlx::query_scalar(
        "SELECT a.name FROM thread_agents ta JOIN agents a ON a.id = ta.agent_id
         JOIN threads t ON t.id = ta.thread_id
         WHERE t.title LIKE '#%' AND t.title <> '#assistant' AND t.kind = 'room'
         ORDER BY a.name",
    )
    .fetch_all(&pool)
    .await
    .expect("staff");
    assert_eq!(staff, ["Finance Manager", "Sales Lead", "Studio Producer"]);
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notes WHERE thread_id = $1 AND author_agent_id IS NOT NULL",
    )
    .bind(architect_room)
    .fetch_one(&pool)
    .await
    .expect("count");
    say(&state, &cookie, architect_room, "hi").await;
    let (_, body) = bot_reply(&pool, architect_room, before as usize)
        .await
        .expect("the Architect answers");
    assert_eq!(body, "Answered on this computer.");
    let calls = seen.lock().unwrap();
    let (system, _, engine, _) = calls.last().expect("asked");
    assert!(system.contains("Architect"), "{system}");
    assert_eq!(engine, &None, "the Architect still uses the default");
}

/// Bot settings: validated, owner only, and an empty value clears.
#[sqlx::test]
async fn bot_settings_are_checked_and_owner_only(pool: PgPool) {
    let (state, _) = local_state(pool.clone());
    let cookie = session_cookie(&enter(&state, TOKEN).await.headers).expect("session");
    let assistant: Uuid = sqlx::query_scalar("SELECT id FROM agents WHERE name = 'Assistant'")
        .fetch_one(&pool)
        .await
        .expect("assistant");
    let post = |id: Uuid, body: serde_json::Value| {
        let state = state.clone();
        let cookie = cookie.clone();
        async move {
            send_with_state(
                state,
                request(
                    "POST",
                    &format!("/agents/{id}/settings"),
                    HOST,
                    Some(&cookie),
                    Some(body),
                ),
            )
            .await
        }
    };

    for bad in [
        json!({ "ai_engine": "skynet" }),
        json!({ "ai_model": "two words" }),
        json!({ "ai_model": "--dangerously-skip-permissions" }),
        json!({ "name": "" }),
    ] {
        let res = post(assistant, bad.clone()).await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST, "{bad}");
    }

    let set = post(
        assistant,
        json!({ "ai_engine": "ollama", "ai_model": "llama3.1:8b", "instructions": "Be brief." }),
    )
    .await;
    assert_eq!(set.status, StatusCode::OK, "{}", set.text);
    assert_eq!(set.body["instructions"], "Be brief.");
    // Fields left out stay; an empty string clears back to the default.
    let cleared = post(assistant, json!({ "ai_model": "" })).await;
    assert_eq!(cleared.body["ai_engine"], "ollama");
    assert!(cleared.body["ai_model"].is_null());
    assert_eq!(cleared.body["name"], "Assistant");

    // Someone else's bot is not found, and is not changed.
    let other_user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, handle) VALUES ('other@example.com', 'other') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("user");
    let other_bot = airobot::office::owned_bot(&pool, other_user, "Theirs", "theirs", "x")
        .await
        .expect("bot");
    let res = post(other_bot, json!({ "ai_engine": "codex" })).await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    let engine: Option<String> = sqlx::query_scalar("SELECT ai_engine FROM agents WHERE id = $1")
        .bind(other_bot)
        .fetch_one(&pool)
        .await
        .expect("engine");
    assert_eq!(engine, None);
}
