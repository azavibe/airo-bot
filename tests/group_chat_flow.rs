// The office group chat: the Architect reads a message and hands it to the
// right bot, an @mention goes straight to that bot, and every bot answers in
// the same thread so they share its context. A scripted local AI stands in
// for the person's AI tool; no network is used.
mod common;

use std::sync::{Arc, Mutex};

use airobot::config::Config;
use airobot::local::{LocalAi, LocalAiRequest, LocalMode};
use airobot::state::AppState;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::{send_with_state, session_cookie};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const BASE: &str = "http://127.0.0.1:43124";
const HOST: &str = "127.0.0.1:43124";
const TOKEN: &str = "group-chat-test-token";

/// (system, prompt) of every call the AI got.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

fn local_state(pool: PgPool) -> (AppState, Seen) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let ai: LocalAi = Arc::new(move |req: LocalAiRequest| {
        let log = Arc::clone(&log);
        Box::pin(async move {
            log.lock()
                .unwrap()
                .push((req.system.clone(), req.prompt.clone()));
            // The Architect is told it coordinates a team; it hands
            // follow-ups to sales and answers everything else itself.
            if req
                .system
                .contains("You coordinate this office's group chat")
            {
                if req.prompt.contains("follow up") {
                    return Ok("@sales-lead: draft a follow-up for Acme".to_string());
                }
                return Ok("Here is my plan.".to_string());
            }
            Ok("Done.".to_string())
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

async fn sign_in(state: &AppState) -> String {
    let res = send_with_state(
        state.clone(),
        request("GET", &format!("/local/enter?token={TOKEN}"), None, None),
    )
    .await;
    session_cookie(&res.headers).expect("session")
}

async fn say(state: &AppState, cookie: &str, thread: Uuid, text: &str) {
    let res = send_with_state(
        state.clone(),
        request(
            "POST",
            &format!("/threads/{thread}/notes"),
            Some(cookie),
            Some(json!({ "body": text, "kind": "status" })),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text);
}

/// Bot notes in a thread so far: (bot name, body), oldest first.
async fn bot_notes(pool: &PgPool, thread: Uuid) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT a.name, n.body FROM notes n JOIN agents a ON a.id = n.author_agent_id
         WHERE n.thread_id = $1 ORDER BY n.created_at, n.seq",
    )
    .bind(thread)
    .fetch_all(pool)
    .await
    .expect("notes")
}

/// Waits until the thread has at least `count` bot notes.
async fn wait_for(pool: &PgPool, thread: Uuid, count: usize) -> Vec<(String, String)> {
    for _ in 0..80 {
        let notes = bot_notes(pool, thread).await;
        if notes.len() >= count {
            return notes;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    bot_notes(pool, thread).await
}

async fn set_up_office(state: &AppState, cookie: &str, pool: &PgPool) -> Uuid {
    let res = send_with_state(
        state.clone(),
        request(
            "POST",
            "/api/office/onboard",
            Some(cookie),
            Some(json!({
                "business_type": "Roofing",
                "replaced_saas": [],
                "goals": "More leads",
                "teams": ["sales", "finance"],
            })),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.text);
    let chat: Uuid =
        sqlx::query_scalar("SELECT id FROM threads WHERE kind = 'office' AND workspace_id = $1")
            .bind(
                res.body["workspace_id"]
                    .as_str()
                    .and_then(|s| s.parse::<Uuid>().ok())
                    .expect("workspace"),
            )
            .fetch_one(pool)
            .await
            .expect("the office's group chat");
    // Let the kickoff finish (intro, two plans, two first tasks) so the
    // tests below see only their own messages.
    let kicked_off = wait_for(pool, chat, 5).await;
    assert_eq!(kicked_off.len(), 5, "{kicked_off:?}");
    chat
}

#[sqlx::test]
async fn the_architect_hands_a_message_to_the_right_bot(pool: PgPool) {
    let (state, seen) = local_state(pool.clone());
    let cookie = sign_in(&state).await;
    let office = set_up_office(&state, &cookie, &pool).await;
    seen.lock().unwrap().clear();

    // Everyone in the office is in the group chat; the roster is not hidden.
    let members: Vec<String> = sqlx::query_scalar(
        "SELECT a.name FROM thread_agents ta JOIN agents a ON a.id = ta.agent_id
         WHERE ta.thread_id = $1 ORDER BY a.name",
    )
    .bind(office)
    .fetch_all(&pool)
    .await
    .expect("members");
    assert_eq!(
        members,
        ["Architect", "Assistant", "Finance Manager", "Sales Lead"],
        "the Architect and the bots of the office's rooms (the first office has the Assistant's)"
    );
    // The welcome note is already there (Office Bot); count only replies.
    let before = bot_notes(&pool, office).await.len();

    say(&state, &cookie, office, "Please follow up with Acme").await;
    let notes = wait_for(&pool, office, before + 2).await;
    let replies = &notes[before..];
    assert_eq!(
        replies[0],
        ("Architect".into(), "Passing this to Sales Lead.".into())
    );
    assert_eq!(replies[1], ("Sales Lead".into(), "Done.".into()));

    // The Architect saw the team; the bot saw the conversation and its task.
    let calls = seen.lock().unwrap().clone();
    let architect_call = calls
        .iter()
        .find(|(sys, _)| sys.contains("coordinate this office"))
        .expect("the Architect was asked");
    assert!(
        architect_call.0.contains("@sales-lead"),
        "{}",
        architect_call.0
    );
    assert!(
        architect_call.0.contains("@finance"),
        "{}",
        architect_call.0
    );
    let sales_call = calls
        .iter()
        .find(|(sys, _)| sys.starts_with("You run sales"))
        .expect("Sales Lead was asked");
    assert!(
        sales_call.1.contains("Please follow up with Acme"),
        "{}",
        sales_call.1
    );
    assert!(
        sales_call
            .1
            .contains("Architect (to Sales Lead): draft a follow-up for Acme"),
        "{}",
        sales_call.1
    );
}

#[sqlx::test]
async fn the_architect_answers_itself_when_nobody_else_fits(pool: PgPool) {
    let (state, _) = local_state(pool.clone());
    let cookie = sign_in(&state).await;
    let office = set_up_office(&state, &cookie, &pool).await;
    let before = bot_notes(&pool, office).await.len();
    say(&state, &cookie, office, "What should we automate first?").await;
    let notes = wait_for(&pool, office, before + 1).await;
    assert_eq!(
        notes[before],
        ("Architect".into(), "Here is my plan.".into())
    );
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        bot_notes(&pool, office).await.len(),
        before + 1,
        "nobody else spoke"
    );
}

#[sqlx::test]
async fn an_at_mention_goes_straight_to_that_bot(pool: PgPool) {
    let (state, seen) = local_state(pool.clone());
    let cookie = sign_in(&state).await;
    let office = set_up_office(&state, &cookie, &pool).await;
    let before = bot_notes(&pool, office).await.len();
    seen.lock().unwrap().clear();
    say(&state, &cookie, office, "@finance when are invoices due?").await;
    let notes = wait_for(&pool, office, before + 1).await;
    assert_eq!(notes[before], ("Finance Manager".into(), "Done.".into()));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let calls = seen.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "only the mentioned bot was asked");
    assert!(!calls[0].0.contains("coordinate this office"));
}

#[sqlx::test]
async fn the_group_chat_cannot_be_deleted_or_renamed(pool: PgPool) {
    let (state, _) = local_state(pool.clone());
    let cookie = sign_in(&state).await;
    let office = set_up_office(&state, &cookie, &pool).await;
    for (uri, body) in [
        (format!("/threads/{office}/delete"), json!({})),
        (
            format!("/threads/{office}/rename"),
            json!({ "title": "#other" }),
        ),
    ] {
        let res = send_with_state(
            state.clone(),
            request("POST", &uri, Some(&cookie), Some(body)),
        )
        .await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST, "{uri}: {}", res.text);
    }
    // It is listed as the office's group chat, apart from the rooms.
    let list = send_with_state(
        state.clone(),
        request("GET", "/threads", Some(&cookie), None),
    )
    .await;
    let kinds: Vec<&str> = list.body["threads"]
        .as_array()
        .expect("threads")
        .iter()
        .filter_map(|t| t["kind"].as_str())
        .collect();
    assert!(
        kinds.contains(&"office") && kinds.contains(&"room"),
        "{kinds:?}"
    );
}
