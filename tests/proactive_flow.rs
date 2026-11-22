// A proactive office: the kickoff (plans, one-tap actions, first tasks
// started at once), web access within a daily allowance, and the
// Architect's morning plan. A scripted local AI stands in for the person's
// AI tool and records what it was asked and whether the web was on.
mod common;

use std::sync::{Arc, Mutex};

use airobot::config::Config;
use airobot::local::{LocalAi, LocalAiRequest, LocalMode};
use airobot::state::AppState;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::{send_with_state, session_cookie, TestResponse};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const BASE: &str = "http://127.0.0.1:43125";
const HOST: &str = "127.0.0.1:43125";
const TOKEN: &str = "proactive-test-token";

#[derive(Clone, Debug)]
struct Call {
    system: String,
    prompt: String,
    web: bool,
}

type Seen = Arc<Mutex<Vec<Call>>>;

fn local_state(pool: PgPool, web_runs_per_day: i64) -> (AppState, Seen) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let ai: LocalAi = Arc::new(move |req: LocalAiRequest| {
        let log = Arc::clone(&log);
        Box::pin(async move {
            log.lock().unwrap().push(Call {
                system: req.system.clone(),
                prompt: req.prompt.clone(),
                web: req.web,
            });
            if req.system.contains("coordinate this office") && req.prompt.contains("A new day") {
                return Ok(
                    "Today is about follow-ups.\n@sales-lead: follow up with Acme".to_string(),
                );
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
    config.web.runs_per_day = web_runs_per_day;
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

async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    cookie: &str,
    body: Option<Value>,
) -> TestResponse {
    send_with_state(state.clone(), request(method, uri, Some(cookie), body)).await
}

async fn sign_in(state: &AppState) -> String {
    let res = send_with_state(
        state.clone(),
        request("GET", &format!("/local/enter?token={TOKEN}"), None, None),
    )
    .await;
    session_cookie(&res.headers).expect("session")
}

/// (bot name, body, actions) of the bot notes in a thread, oldest first.
async fn bot_notes(pool: &PgPool, thread: Uuid) -> Vec<(String, String, Option<Value>)> {
    sqlx::query_as(
        "SELECT a.name, n.body, n.actions FROM notes n JOIN agents a ON a.id = n.author_agent_id
         WHERE n.thread_id = $1 ORDER BY n.seq",
    )
    .bind(thread)
    .fetch_all(pool)
    .await
    .expect("notes")
}

async fn wait_for(
    pool: &PgPool,
    thread: Uuid,
    count: usize,
) -> Vec<(String, String, Option<Value>)> {
    for _ in 0..100 {
        let notes = bot_notes(pool, thread).await;
        if notes.len() >= count {
            return notes;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    bot_notes(pool, thread).await
}

async fn office_chat(pool: &PgPool, workspace: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM threads WHERE kind = 'office' AND workspace_id = $1")
        .bind(workspace.parse::<Uuid>().expect("uuid"))
        .fetch_one(pool)
        .await
        .expect("group chat")
}

async fn set_up(state: &AppState, cookie: &str, website: &str) -> TestResponse {
    call(
        state,
        "POST",
        "/api/office/onboard",
        cookie,
        Some(json!({
            "business_type": "Dental clinic",
            "replaced_saas": [],
            "goals": "",
            "company_name": "Bright Smiles",
            "teams": ["sales", "finance"],
            "website": website,
            "focus": "More customers",
            "utc_offset_minutes": 420,
        })),
    )
    .await
}

#[sqlx::test]
async fn a_new_office_starts_working_without_being_asked(pool: PgPool) {
    let (state, seen) = local_state(pool.clone(), 60);
    let cookie = sign_in(&state).await;
    let res = set_up(&state, &cookie, "brightsmiles.example").await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.text);
    let workspace = res.body["workspace_id"].as_str().expect("id").to_string();
    let chat = office_chat(&pool, &workspace).await;

    // Intro + two plans at once, then the website summary and both first
    // tasks, one after another.
    let notes = wait_for(&pool, chat, 6).await;
    assert_eq!(notes.len(), 6, "{notes:#?}");
    assert_eq!(notes[0].0, "Architect");
    assert!(notes[0].1.contains("read your website"), "{}", notes[0].1);
    let sales_plan = notes
        .iter()
        .find(|n| n.0 == "Sales Lead")
        .expect("sales plan");
    assert!(
        sales_plan.1.starts_with("My plan for this week:"),
        "{}",
        sales_plan.1
    );
    let actions = sales_plan.2.as_ref().expect("buttons");
    assert_eq!(actions[0]["kind"], "send");
    assert!(
        actions[0]["message"]
            .as_str()
            .unwrap_or_default()
            .starts_with("@sales-lead: Write a short, warm reply"),
        "{actions}"
    );
    assert!(notes
        .iter()
        .any(|n| n.0 == "Finance Manager" && n.1.starts_with("My plan")));

    // The work used the web and knew the business.
    let calls = seen.lock().unwrap().clone();
    assert_eq!(calls.len(), 3, "{calls:#?}");
    assert!(
        calls.iter().all(|c| c.web),
        "web was on for every first task"
    );
    assert!(
        calls[0]
            .prompt
            .contains("Read https://brightsmiles.example"),
        "{}",
        calls[0].prompt
    );
    assert!(
        calls[1].prompt.contains("Bright Smiles (Dental clinic)"),
        "{}",
        calls[1].prompt
    );
    assert!(
        calls[1].prompt.contains("More customers"),
        "{}",
        calls[1].prompt
    );
    assert!(
        calls[1].system.contains("search the web"),
        "{}",
        calls[1].system
    );
    // Office bots know the business and its apps, and how to write.
    assert!(
        calls[1]
            .system
            .contains("You work for Bright Smiles, a Dental clinic."),
        "{}",
        calls[1].system
    );
    assert!(calls[1].system.contains("ADD <app>"), "{}", calls[1].system);
    assert!(
        calls[1].system.contains("Question: "),
        "{}",
        calls[1].system
    );

    // The app gets the buttons with the notes.
    let thread = call(&state, "GET", &format!("/threads/{chat}"), &cookie, None).await;
    let with_actions = thread.body["notes"]
        .as_array()
        .expect("notes")
        .iter()
        .filter(|n| n["actions"].is_array())
        .count();
    assert_eq!(with_actions, 2);

    // The profile is kept for the team.
    let offices = call(&state, "GET", "/api/offices", &cookie, None).await;
    let office = offices.body["offices"]
        .as_array()
        .expect("offices")
        .iter()
        .find(|o| o["id"] == workspace.as_str())
        .expect("office")
        .clone();
    assert_eq!(office["website"], "https://brightsmiles.example");
    assert_eq!(office["focus"], "More customers");
    assert_eq!(office["daily_plan"], true);
}

#[sqlx::test]
async fn web_access_stops_at_the_daily_allowance(pool: PgPool) {
    let (state, seen) = local_state(pool.clone(), 2);
    let cookie = sign_in(&state).await;
    let res = set_up(&state, &cookie, "brightsmiles.example").await;
    let chat = office_chat(&pool, res.body["workspace_id"].as_str().expect("id")).await;
    wait_for(&pool, chat, 6).await;

    let calls = seen.lock().unwrap().clone();
    assert_eq!(calls.iter().filter(|c| c.web).count(), 2, "{calls:#?}");
    let third = &calls[2];
    assert!(!third.web);
    assert!(third.system.contains("used up"), "{}", third.system);

    let wallet = call(&state, "GET", "/api/office/wallet", &cookie, None).await;
    assert_eq!(wallet.body["web_runs_today"], 2);
    assert_eq!(wallet.body["web_runs_limit"], 2);
}

#[sqlx::test]
async fn a_blank_office_says_how_to_start_and_websites_are_checked(pool: PgPool) {
    let (state, seen) = local_state(pool.clone(), 60);
    let cookie = sign_in(&state).await;
    let made = call(
        &state,
        "POST",
        "/api/offices",
        &cookie,
        Some(json!({ "name": "Side project", "intro": true, "website": "side.example" })),
    )
    .await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.text);
    let chat = office_chat(&pool, made.body["office"]["id"].as_str().expect("id")).await;
    let notes = bot_notes(&pool, chat).await;
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].0, "Architect");
    assert_eq!(notes[0].2.as_ref().expect("button")[0]["kind"], "setup");
    assert!(
        seen.lock().unwrap().is_empty(),
        "no AI work in a blank office"
    );

    for bad in ["localhost", "http://192.168.1.4", "ftp://x.example"] {
        let res = call(
            &state,
            "POST",
            "/api/offices",
            &cookie,
            Some(json!({ "name": "Bad", "website": bad })),
        )
        .await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(
            set_up(&state, &cookie, bad).await.status,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
}

#[sqlx::test]
async fn the_architect_plans_each_morning_and_the_team_starts(pool: PgPool) {
    let (state, seen) = local_state(pool.clone(), 60);
    let cookie = sign_in(&state).await;
    let res = set_up(&state, &cookie, "").await;
    let workspace = res.body["workspace_id"].as_str().expect("id").to_string();
    let chat = office_chat(&pool, &workspace).await;
    let after_kickoff = wait_for(&pool, chat, 6).await.len();

    // Just after kickoff nothing is due: today's work was the kickoff.
    assert_eq!(
        airobot::office::daily::run_due(&state).await.expect("tick"),
        0
    );

    // The next morning (noon where the owner is, 21 hours later).
    sqlx::query(
        "UPDATE workspaces SET last_plan_at = now() - interval '21 hours',
                utc_offset_minutes = ((12 - EXTRACT(HOUR FROM now() AT TIME ZONE 'UTC')::int + 36) % 24 - 12) * 60
         WHERE id = $1",
    )
    .bind(workspace.parse::<Uuid>().expect("uuid"))
    .execute(&pool)
    .await
    .expect("move the clock");
    seen.lock().unwrap().clear();
    assert_eq!(
        airobot::office::daily::run_due(&state).await.expect("tick"),
        1
    );
    // Claimed: a second tick does nothing.
    assert_eq!(
        airobot::office::daily::run_due(&state).await.expect("tick"),
        0
    );

    let notes = wait_for(&pool, chat, after_kickoff + 2).await;
    assert_eq!(notes[after_kickoff].0, "Architect");
    assert_eq!(notes[after_kickoff].1, "Today is about follow-ups.");
    assert_eq!(notes[after_kickoff + 1].0, "Sales Lead");
    let calls = seen.lock().unwrap().clone();
    assert!(
        calls[0].prompt.contains("A new day has started"),
        "{}",
        calls[0].prompt
    );
    assert!(calls[1]
        .prompt
        .contains("Architect (to Sales Lead): follow up with Acme"));

    // Turned off, no plan comes.
    let off = call(
        &state,
        "PATCH",
        &format!("/api/offices/{workspace}"),
        &cookie,
        Some(json!({ "daily_plan": false })),
    )
    .await;
    assert_eq!(off.status, StatusCode::OK, "{}", off.text);
    assert_eq!(off.body["daily_plan"], false);
    sqlx::query("UPDATE workspaces SET last_plan_at = now() - interval '2 days' WHERE id = $1")
        .bind(workspace.parse::<Uuid>().expect("uuid"))
        .execute(&pool)
        .await
        .expect("clock");
    assert_eq!(
        airobot::office::daily::run_due(&state).await.expect("tick"),
        0
    );
}

#[sqlx::test]
async fn a_stopped_office_starts_no_work_until_it_runs_again(pool: PgPool) {
    let (state, seen) = local_state(pool.clone(), 60);
    let cookie = sign_in(&state).await;
    let res = set_up(&state, &cookie, "").await;
    let workspace = res.body["workspace_id"].as_str().expect("id").to_string();
    let chat = office_chat(&pool, &workspace).await;
    let before = wait_for(&pool, chat, 6).await.len();

    let stop = call(
        &state,
        "PATCH",
        &format!("/api/offices/{workspace}"),
        &cookie,
        Some(json!({ "paused": true })),
    )
    .await;
    assert_eq!(stop.status, StatusCode::OK, "{}", stop.text);
    assert_eq!(stop.body["paused"], true);
    seen.lock().unwrap().clear();

    let said = call(
        &state,
        "POST",
        &format!("/threads/{chat}/notes"),
        &cookie,
        Some(json!({ "body": "@sales-lead anything new?", "kind": "status" })),
    )
    .await;
    assert_eq!(
        said.status,
        StatusCode::CREATED,
        "the message itself is kept"
    );
    sqlx::query("UPDATE workspaces SET last_plan_at = now() - interval '2 days' WHERE id = $1")
        .bind(workspace.parse::<Uuid>().expect("uuid"))
        .execute(&pool)
        .await
        .expect("clock");
    assert_eq!(
        airobot::office::daily::run_due(&state).await.expect("tick"),
        0
    );
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(
        bot_notes(&pool, chat).await.len(),
        before,
        "nobody answered"
    );
    assert!(seen.lock().unwrap().is_empty(), "no AI work while stopped");

    let offices = call(&state, "GET", "/api/offices", &cookie, None).await;
    assert!(offices.body["offices"]
        .as_array()
        .expect("offices")
        .iter()
        .any(|o| o["id"] == workspace.as_str() && o["paused"] == true));

    call(
        &state,
        "PATCH",
        &format!("/api/offices/{workspace}"),
        &cookie,
        Some(json!({ "paused": false })),
    )
    .await;
    call(
        &state,
        "POST",
        &format!("/threads/{chat}/notes"),
        &cookie,
        Some(json!({ "body": "@sales-lead and now?", "kind": "status" })),
    )
    .await;
    let notes = wait_for(&pool, chat, before + 1).await;
    assert_eq!(notes[before].0, "Sales Lead");
}
