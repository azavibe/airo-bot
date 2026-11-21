//! Goals that move: milestones the owner keeps (their own, or the
//! Architect's proposals after a new goal), opt-in check-ins, and the
//! weekly report built from the database. Real router and database; a
//! scripted local AI plays the Architect.

mod common;

use std::sync::Arc;

use airobot::config::Config;
use airobot::local::{LocalAi, LocalAiRequest, LocalMode};
use airobot::office::goals;
use airobot::state::AppState;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use chrono::Timelike;
use common::{send_with_state, session_cookie};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const BASE: &str = "http://127.0.0.1:43134";
const HOST: &str = "127.0.0.1:43134";
const TOKEN: &str = "goals-test-token";

fn local_state(pool: PgPool) -> AppState {
    let ai: LocalAi = Arc::new(move |req: LocalAiRequest| {
        Box::pin(async move {
            if req.prompt.contains("Propose 3 measurable milestones") {
                return Ok("Here is a plan.\nMILESTONE: 10 regular customers by 2027-01-31\nMILESTONE: Launch the website\nOUTCOME: needs your OK — keep the milestones you want".to_string());
            }
            if req.prompt.contains("check-in") {
                return Ok("Progress: on track.\nOUTCOME: done".to_string());
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

async fn setup(state: &AppState) -> (String, Uuid, Uuid) {
    let res = send_with_state(
        state.clone(),
        request("GET", &format!("/local/enter?token={TOKEN}"), None, None),
    )
    .await;
    let cookie = session_cookie(&res.headers).expect("session");
    let (status, made) = call(
        state,
        "POST",
        "/api/offices",
        &cookie,
        Some(json!({ "name": "Bakery" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made:?}");
    let office: Uuid = made["office"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let chat: Uuid =
        sqlx::query_scalar("SELECT id FROM threads WHERE workspace_id = $1 AND kind = 'office'")
            .bind(office)
            .fetch_one(&state.pool)
            .await
            .expect("office chat");
    (cookie, office, chat)
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
async fn the_owner_keeps_milestones_and_settings(pool: PgPool) {
    let state = local_state(pool.clone());
    let (cookie, office, _) = setup(&state).await;
    let uri = format!("/api/offices/{office}/milestones");

    let (status, m) = call(
        &state,
        "POST",
        &uri,
        &cookie,
        Some(json!({ "title": "  20 paying   customers ", "due_on": "2027-03-01", "idempotency_key": "m1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{m:?}");
    assert_eq!(m["title"], "20 paying customers");
    assert_eq!(m["status"], "active");
    assert_eq!(m["source"], "owner");
    assert_eq!(m["due_on"], "2027-03-01");
    let id = m["id"].as_str().expect("id").to_string();
    let (_, again) = call(
        &state,
        "POST",
        &uri,
        &cookie,
        Some(json!({ "title": "20 paying customers", "idempotency_key": "m1" })),
    )
    .await;
    assert_eq!(again["id"], id.as_str());

    for bad in [
        json!({ "title": "" }),
        json!({ "title": "x", "due_on": "soon" }),
        json!({ "title": "x".repeat(201) }),
    ] {
        let (status, _) = call(&state, "POST", &uri, &cookie, Some(bad.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}");
    }

    // Reached, then reopened, then its date cleared.
    let (status, done) = call(
        &state,
        "PATCH",
        &format!("/api/milestones/{id}"),
        &cookie,
        Some(json!({ "status": "done" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done:?}");
    assert_eq!(done["status"], "done");
    assert!(done["done_at"].is_string());
    let (_, open) = call(
        &state,
        "PATCH",
        &format!("/api/milestones/{id}"),
        &cookie,
        Some(json!({ "status": "active", "due_on": "" })),
    )
    .await;
    assert_eq!(open["status"], "active");
    assert!(open["done_at"].is_null());
    assert!(open["due_on"].is_null());
    let (status, _) = call(
        &state,
        "PATCH",
        &format!("/api/milestones/{id}"),
        &cookie,
        Some(json!({ "status": "proposed" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Check-ins and the report are set on the office.
    let (status, saved) = call(
        &state,
        "PATCH",
        &format!("/api/offices/{office}"),
        &cookie,
        Some(json!({ "check_in": "weekly", "weekly_report": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved:?}");
    assert_eq!(saved["check_in"], "weekly");
    assert_eq!(saved["weekly_report"], false);
    let (status, _) = call(
        &state,
        "PATCH",
        &format!("/api/offices/{office}"),
        &cookie,
        Some(json!({ "check_in": "hourly" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, view) = call(
        &state,
        "GET",
        &format!("/api/offices/{office}/goal"),
        &cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["check_in"], "weekly");
    assert_eq!(view["milestones"].as_array().expect("list").len(), 1);

    // Deleted (soft).
    let (status, _) = call(
        &state,
        "POST",
        &format!("/api/milestones/{id}/delete"),
        &cookie,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, view) = call(
        &state,
        "GET",
        &format!("/api/offices/{office}/goal"),
        &cookie,
        None,
    )
    .await;
    assert!(view["milestones"].as_array().expect("list").is_empty());
    let (status, _) = call(
        &state,
        "PATCH",
        &format!("/api/milestones/{id}"),
        &cookie,
        Some(json!({ "status": "done" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Another office's owner can't see or change them (a second local
    // session is the same owner, so check by id instead).
    let stranger = Uuid::new_v4();
    let (status, _) = call(
        &state,
        "GET",
        &format!("/api/offices/{stranger}/goal"),
        &cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn a_new_goal_gets_proposed_milestones_the_owner_keeps(pool: PgPool) {
    let state = local_state(pool.clone());
    let (cookie, office, chat) = setup(&state).await;
    let (status, _) = call(
        &state,
        "PATCH",
        &format!("/api/offices/{office}"),
        &cookie,
        Some(json!({ "goal": "Become the favourite bakery in town" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (body, actions): (String, Option<Value>) = wait_for(|| async {
        sqlx::query_as(
            "SELECT body, actions FROM notes WHERE thread_id = $1 AND body LIKE '%Proposed milestones%'",
        )
        .bind(chat)
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten()
    })
    .await
    .expect("the Architect proposed milestones");
    assert!(
        body.contains("10 regular customers (by 2027-01-31)"),
        "{body}"
    );
    assert!(!body.contains("MILESTONE:"), "{body}");
    assert!(
        body.contains("*Needs your OK: keep the milestones you want*"),
        "{body}"
    );
    let actions = actions.expect("buttons");
    assert_eq!(actions[0]["kind"], "keep_milestone");
    let keep = actions[0]["target"].as_str().expect("id").to_string();

    let (_, view) = call(
        &state,
        "GET",
        &format!("/api/offices/{office}/goal"),
        &cookie,
        None,
    )
    .await;
    let list = view["milestones"].as_array().expect("list");
    assert_eq!(list.len(), 2);
    assert!(list
        .iter()
        .all(|m| m["status"] == "proposed" && m["source"] == "bot"));
    // Proposals aren't targets yet: bots aren't told about them.
    let block = goals::prompt_block(&pool, office, false).await;
    assert!(block.is_empty(), "{block}");

    // The owner keeps one; bots now work toward it.
    let (status, kept) = call(
        &state,
        "PATCH",
        &format!("/api/milestones/{keep}"),
        &cookie,
        Some(json!({ "status": "active" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(kept["status"], "active");
    let block = goals::prompt_block(&pool, office, false).await;
    assert!(
        block.contains("- [ ] 10 regular customers (by 2027-01-31)"),
        "{block}"
    );
    assert!(!block.contains("Launch the website"), "{block}");
    assert!(goals::prompt_block(&pool, office, true)
        .await
        .contains("MILESTONE:"));

    // The same goal again proposes nothing new.
    call(
        &state,
        "PATCH",
        &format!("/api/offices/{office}"),
        &cookie,
        Some(json!({ "goal": "Become the favourite bakery in town" })),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let proposals: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notes WHERE thread_id = $1 AND body LIKE '%Proposed milestones%'",
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(proposals, 1);
}

#[sqlx::test]
async fn check_ins_are_opt_in_and_run_in_the_afternoon(pool: PgPool) {
    let state = local_state(pool.clone());
    let (cookie, office, chat) = setup(&state).await;
    // 20:00 local, whatever the hour is now.
    let hour = chrono::Utc::now().hour() as i32;
    let mut offset_hours = (20 - hour).rem_euclid(24);
    if offset_hours > 12 {
        offset_hours -= 24;
    }
    sqlx::query("UPDATE workspaces SET utc_offset_minutes = $2 WHERE id = $1")
        .bind(office)
        .bind(offset_hours * 60)
        .execute(&pool)
        .await
        .expect("offset");

    // Off by default.
    assert_eq!(goals::run_due_check_ins(&state).await.expect("tick"), 0);

    call(
        &state,
        "PATCH",
        &format!("/api/offices/{office}"),
        &cookie,
        Some(json!({ "check_in": "daily" })),
    )
    .await;
    assert_eq!(goals::run_due_check_ins(&state).await.expect("tick"), 1);
    let body: String = wait_for(|| async {
        sqlx::query_scalar("SELECT body FROM notes WHERE thread_id = $1 AND body LIKE 'Progress:%'")
            .bind(chat)
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten()
    })
    .await
    .expect("a check-in");
    assert!(body.ends_with("*Done.*"), "{body}");
    // Once a day.
    assert_eq!(goals::run_due_check_ins(&state).await.expect("tick"), 0);
}

#[sqlx::test]
async fn the_weekly_report_counts_what_happened(pool: PgPool) {
    let state = local_state(pool.clone());
    let (cookie, office, chat) = setup(&state).await;
    // An app with two records this week and one older.
    let res = send_with_state(
        state.clone(),
        request(
            "POST",
            "/api/office/onboard",
            Some(&cookie),
            Some(json!({ "workspace_id": office, "business_type": "Bakery", "replaced_saas": [], "goals": "", "teams": ["sales"], "idempotency_key": "onb" })),
        ),
    )
    .await;
    assert!(res.status.is_success(), "{}", res.text);
    for name in ["Rosa", "Ben", "Old"] {
        let res = send_with_state(
            state.clone(),
            request(
                "POST",
                &format!("/api/office/apps/leads-pipeline/records?office={office}"),
                Some(&cookie),
                Some(json!({ "record_type": "lead", "data": { "name": name } })),
            ),
        )
        .await;
        assert!(res.status.is_success(), "{}", res.text);
    }
    sqlx::query("UPDATE office_records SET created_at = now() - interval '10 days' WHERE data->>'name' = 'Old'")
        .execute(&pool)
        .await
        .expect("age");
    // Jobs with outcomes, spend, a reached milestone, an open one.
    for outcome in ["done", "done", "blocked", "needs_ok"] {
        sqlx::query(
            "INSERT INTO tasks (workspace_id, title, status, payload, executor, fsm_state, outcome)
             VALUES ($1, 'job', 'completed', '{}', 'chat', 'COMPLETED', $2)",
        )
        .bind(office)
        .bind(outcome)
        .execute(&pool)
        .await
        .expect("job");
    }
    sqlx::query(
        "INSERT INTO credit_ledger (workspace_id, provider, model_or_tool, credits_charged, raw_cost_micros, office_id)
         VALUES ($1, 'openrouter', 'm', 75, 1, $1)",
    )
    .bind(office)
    .execute(&pool)
    .await
    .expect("spend");
    sqlx::query(
        "INSERT INTO milestones (workspace_id, title, status, done_at) VALUES ($1, 'Menu printed', 'done', now());
         ",
    )
    .bind(office)
    .execute(&pool)
    .await
    .expect("done milestone");
    sqlx::query("INSERT INTO milestones (workspace_id, title, due_on) VALUES ($1, '10 regulars', DATE '2030-01-01')")
        .bind(office)
        .execute(&pool)
        .await
        .expect("open milestone");

    let report = goals::weekly_report(&pool, office).await.expect("report");
    assert!(report.starts_with("**Weekly report · "), "{report}");
    assert!(
        report.contains("- 2 records added: Lead Pipeline & CRM 2"),
        "{report}"
    );
    assert!(
        report.contains("- Jobs: 2 done, 1 need your OK, 1 blocked"),
        "{report}"
    );
    assert!(report.contains("- 75 AI credits spent"), "{report}");
    assert!(
        report.contains("- Milestones reached: Menu printed"),
        "{report}"
    );
    assert!(
        report.contains("- Next milestone: 10 regulars (by 2030-01-01)"),
        "{report}"
    );

    // Posted in the office chat with buttons to the rows behind it.
    let owner: Uuid = sqlx::query_scalar("SELECT owner_user_id FROM workspaces WHERE id = $1")
        .bind(office)
        .fetch_one(&pool)
        .await
        .expect("owner");
    goals::post_report(&state, office, owner, chat).await;
    let actions: Option<Value> = sqlx::query_scalar(
        "SELECT actions FROM notes WHERE thread_id = $1 AND body LIKE '**Weekly report%'",
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .expect("posted");
    let actions = actions.expect("buttons");
    assert!(actions
        .as_array()
        .expect("list")
        .iter()
        .any(|a| a["target"] == "approvals"));
}
