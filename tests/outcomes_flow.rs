//! Honest outcomes: a job a bot is handed ends with its outcome (the
//! OUTCOME line becomes an italic line in the chat and the job is kept with
//! it), server tasks keep theirs, a job that fails says so, and the bot's
//! stats list its recent work. Real router and database; a scripted local
//! AI plays the Architect and the bot.

mod common;

use std::sync::Arc;

use airobot::config::Config;
use airobot::local::{LocalAi, LocalAiRequest, LocalMode};
use airobot::office::fsm_dispatcher;
use airobot::state::AppState;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::{send_with_state, session_cookie};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const BASE: &str = "http://127.0.0.1:43133";
const HOST: &str = "127.0.0.1:43133";
const TOKEN: &str = "outcomes-test-token";

/// The Architect hands the owner's message to @closer; @closer reports
/// that its drafts need the owner's OK. A server task finds nothing to do,
/// unless its title asks it to fail.
fn local_state(pool: PgPool) -> AppState {
    let ai: LocalAi = Arc::new(move |req: LocalAiRequest| {
        Box::pin(async move {
            if req.system.contains("Reporting this job") && req.prompt.contains("Break") {
                return Err("the tool crashed".to_string());
            }
            if req.system.contains("autonomous SMB virtual office agent") {
                return Ok("Checked the inbox.\nOUTCOME: nothing to do — no new mail".to_string());
            }
            if req.system.contains("Reporting this job") {
                return Ok(
                    "Drafted 3 follow-ups below.\n\n**OUTCOME: needs your OK — send the 3 follow-ups**"
                        .to_string(),
                );
            }
            if req.system.contains("Your teammates") {
                if req.prompt.contains("Break") {
                    return Ok("@closer: Break something".to_string());
                }
                return Ok("@closer: Follow up the quiet leads".to_string());
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

async fn enter(state: &AppState) -> String {
    let res = send_with_state(
        state.clone(),
        request("GET", &format!("/local/enter?token={TOKEN}"), None, None),
    )
    .await;
    session_cookie(&res.headers).expect("session")
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

#[sqlx::test]
async fn a_handed_job_reports_its_outcome_and_is_kept(pool: PgPool) {
    let state = local_state(pool.clone());
    let cookie = enter(&state).await;
    let res = send_with_state(
        state.clone(),
        request(
            "POST",
            "/api/offices",
            Some(&cookie),
            Some(json!({ "name": "Shop" })),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text);
    let office: Uuid = res.body["office"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let res = send_with_state(
        state.clone(),
        request(
            "POST",
            "/agents",
            Some(&cookie),
            Some(json!({ "name": "Closer", "handle": "closer", "instructions": "Close deals.", "workspace_id": office })),
        ),
    )
    .await;
    assert!(res.status.is_success(), "{}", res.text);
    let bot: Uuid = res.body["agent"]["id"]
        .as_str()
        .expect("bot")
        .parse()
        .expect("uuid");
    let chat: Uuid =
        sqlx::query_scalar("SELECT id FROM threads WHERE workspace_id = $1 AND kind = 'office'")
            .bind(office)
            .fetch_one(&pool)
            .await
            .expect("office chat");

    let res = send_with_state(
        state.clone(),
        request(
            "POST",
            &format!("/threads/{chat}/notes"),
            Some(&cookie),
            Some(json!({ "body": "Can someone chase the quiet leads?", "kind": "status" })),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text);

    let body: String = wait_for(|| async {
        sqlx::query_scalar::<_, String>(
            "SELECT body FROM notes WHERE thread_id = $1 AND author_agent_id = $2",
        )
        .bind(chat)
        .bind(bot)
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten()
    })
    .await
    .expect("the bot did the job");
    assert!(body.starts_with("Drafted 3 follow-ups below."), "{body}");
    assert!(
        body.ends_with("*Needs your OK: send the 3 follow-ups*"),
        "{body}"
    );
    assert!(!body.contains("OUTCOME"), "{body}");

    let (title, outcome, note, executor): (String, Option<String>, Option<String>, String) =
        wait_for(|| async {
            sqlx::query_as(
                "SELECT title, outcome, outcome_note, executor FROM tasks WHERE assigned_agent_id = $1",
            )
            .bind(bot)
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten()
        })
        .await
        .expect("the job is kept");
    assert_eq!(title, "Follow up the quiet leads");
    assert_eq!(outcome.as_deref(), Some("needs_ok"));
    assert_eq!(note.as_deref(), Some("send the 3 follow-ups"));
    assert_eq!(executor, "chat");
    // A chat job is a record only: no agent or the dispatcher picks it up.
    assert_eq!(
        fsm_dispatcher::run_dispatcher_once(&state)
            .await
            .expect("tick"),
        0
    );

    // The bot's settings list it.
    let res = send_with_state(
        state.clone(),
        request("GET", &format!("/agents/{bot}/stats"), Some(&cookie), None),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.text);
    let work = res.body["recent_work"].as_array().expect("work");
    assert_eq!(work.len(), 1);
    assert_eq!(work[0]["outcome"], "needs_ok");
    assert_eq!(work[0]["credits"], 0, "the computer's own AI costs nothing");

    // A job whose AI fails is kept as failed, not lost.
    let res = send_with_state(
        state.clone(),
        request(
            "POST",
            &format!("/threads/{chat}/notes"),
            Some(&cookie),
            Some(json!({ "body": "Break the build, please", "kind": "status" })),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED);
    let (outcome, note): (Option<String>, Option<String>) = wait_for(|| async {
        sqlx::query_as(
            "SELECT outcome, outcome_note FROM tasks
             WHERE assigned_agent_id = $1 AND title = 'Break something'",
        )
        .bind(bot)
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten()
    })
    .await
    .expect("the failed job is kept");
    assert_eq!(outcome.as_deref(), Some("failed"));
    assert!(note.unwrap_or_default().contains("didn't answer"));
}

#[sqlx::test]
async fn server_tasks_keep_their_outcome(pool: PgPool) {
    let state = local_state(pool.clone());
    let cookie = enter(&state).await;
    let res = send_with_state(
        state.clone(),
        request(
            "POST",
            "/tasks",
            Some(&cookie),
            Some(json!({ "title": "Check the inbox", "executor": "server" })),
        ),
    )
    .await;
    assert!(res.status.is_success(), "{}", res.text);
    let task: Uuid = res.body["task"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    assert_eq!(
        fsm_dispatcher::run_dispatcher_once(&state)
            .await
            .expect("tick"),
        1
    );
    let (status, outcome, note): (String, Option<String>, Option<String>) = wait_for(|| async {
        let row: Option<(String, Option<String>, Option<String>)> =
            sqlx::query_as("SELECT status, outcome, outcome_note FROM tasks WHERE id = $1")
                .bind(task)
                .fetch_optional(&pool)
                .await
                .ok()
                .flatten();
        row.filter(|r| r.0 != "running" && r.0 != "queued")
    })
    .await
    .expect("finished");
    assert_eq!(status, "completed");
    assert_eq!(outcome.as_deref(), Some("nothing_to_do"));
    assert_eq!(note.as_deref(), Some("no new mail"));
    let result: Value =
        sqlx::query_scalar("SELECT state_payload->'result' FROM tasks WHERE id = $1")
            .bind(task)
            .fetch_one(&pool)
            .await
            .expect("result");
    assert_eq!(
        result,
        json!("Checked the inbox."),
        "the OUTCOME line is not part of the result"
    );
}
