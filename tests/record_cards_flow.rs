//! Live app record cards: a bot's ADD and UPDATE lines become card
//! references in its message; an Ask first bot's change waits as an
//! approval tied to the record (approve, approve with edits, or reject); a
//! Trusted bot's applies at once; the records endpoint serves cards with
//! tombstones and never leaks another office's record; record changes
//! stream live. Real router and database; a scripted local AI plays the
//! Sales Lead.

mod common;

use std::sync::Arc;

use airobot::config::Config;
use airobot::local::{LocalAi, LocalAiRequest, LocalMode};
use airobot::state::AppState;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::{send_with_state, session_cookie};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const BASE: &str = "http://127.0.0.1:43136";
const HOST: &str = "127.0.0.1:43136";
const TOKEN: &str = "cards-test-token";

/// The 8-character id the bot was shown for Rosa's Cafe.
fn rosa_id(system: &str) -> Option<String> {
    system
        .lines()
        .find(|l| l.contains("Rosa's Cafe"))
        .and_then(|l| {
            let start = l.find('[')? + 1;
            Some(l[start..start + 8].to_string())
        })
}

fn local_state(pool: PgPool) -> AppState {
    let ai: LocalAi = Arc::new(move |req: LocalAiRequest| {
        Box::pin(async move {
            let asked = req
                .prompt
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty() && !l.starts_with("Write "))
                .unwrap_or("")
                .to_string();
            let text = if req.prompt.contains("add Rosa") && !req.system.contains("Rosa's Cafe") {
                "Added her.\nADD leads-pipeline {\"name\": \"Rosa's Cafe\", \"stage\": \"New Lead\", \"estimated_value\": 900, \"phone\": \"555-0100\"}".to_string()
            } else if let Some(id) = rosa_id(&req.system) {
                if req.prompt.contains("value 1500") && asked.contains("value 1500") {
                    format!("Done.\nUPDATE leads-pipeline {id} {{\"estimated_value\": 1500}}")
                } else if asked.contains("Won") {
                    format!("She signed!\nUPDATE leads-pipeline {id} {{\"stage\": \"Won\"}}")
                } else if asked.contains("Qualified") {
                    format!("Moving her.\nUPDATE leads-pipeline {id} {{\"stage\": \"Qualified\"}}")
                } else {
                    "Hello.".to_string()
                }
            } else {
                "Hello.".to_string()
            };
            Ok(text)
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

/// An office set up for sales: (cookie, office, Sales Lead, its room).
async fn setup(state: &AppState) -> (String, Uuid, Uuid, Uuid) {
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
        Some(json!({ "name": "Cafe Co" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made:?}");
    let office: Uuid = made["office"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let (status, body) = call(
        state,
        "POST",
        "/api/office/onboard",
        &cookie,
        Some(json!({ "workspace_id": office, "business_type": "Coffee roaster", "replaced_saas": [], "goals": "", "teams": ["sales"], "idempotency_key": "onb" })),
    )
    .await;
    assert!(status.is_success(), "{body:?}");
    let (bot, room): (Uuid, Uuid) = sqlx::query_as(
        "SELECT a.id, t.id FROM agents a JOIN thread_agents ta ON ta.agent_id = a.id
         JOIN threads t ON t.id = ta.thread_id
         WHERE a.name = 'Sales Lead' AND t.workspace_id = $1 AND t.kind = 'room'",
    )
    .bind(office)
    .fetch_one(&state.pool)
    .await
    .expect("the Sales Lead's room");
    (cookie, office, bot, room)
}

/// Sends a message as the owner; returns its place in the room, so a
/// reply can be told apart from earlier ones.
async fn say(state: &AppState, cookie: &str, room: Uuid, text: &str) -> i64 {
    let (status, body) = call(
        state,
        "POST",
        &format!("/threads/{room}/notes"),
        cookie,
        Some(json!({ "body": text, "kind": "status" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    let id: Uuid = body["id"].as_str().expect("note id").parse().expect("uuid");
    sqlx::query_scalar("SELECT seq FROM notes WHERE id = $1")
        .bind(id)
        .fetch_one(&state.pool)
        .await
        .expect("seq")
}

/// The bot's reply in `room` after the owner's message `after` (its seq)
/// whose body contains `marker`.
async fn bot_note(
    pool: &PgPool,
    room: Uuid,
    bot: Uuid,
    after: i64,
    marker: &str,
) -> (String, Option<Value>) {
    wait_for(|| async {
        sqlx::query_as::<_, (String, Option<Value>)>(
            "SELECT body, cards FROM notes WHERE thread_id = $1 AND author_agent_id = $2
               AND seq > $4 AND body LIKE '%' || $3 || '%'
             ORDER BY seq LIMIT 1",
        )
        .bind(room)
        .bind(bot)
        .bind(marker)
        .bind(after)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
    })
    .await
    .unwrap_or_else(|| panic!("no reply with {marker}"))
}

async fn record_view(state: &AppState, cookie: &str, office: Uuid, id: &str) -> Value {
    let (status, body) = call(
        state,
        "GET",
        &format!("/api/office/records?office={office}&ids={id}"),
        cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    body["records"][0].clone()
}

#[sqlx::test]
async fn bot_records_become_live_cards_and_changes_wait_for_an_ok(pool: PgPool) {
    let state = local_state(pool.clone());
    let (cookie, office, bot, room) = setup(&state).await;

    // ADD: the message carries a card reference, not a copy.
    let sent = say(&state, &cookie, room, "Please add Rosa as a lead").await;
    let (body, cards) = bot_note(&pool, room, bot, sent, "Added her").await;
    assert!(!body.contains("ADD "), "{body}");
    let cards = cards.expect("cards");
    assert_eq!(cards[0]["type"], "app_record_card");
    assert_eq!(cards[0]["app_slug"], "leads-pipeline");
    assert_eq!(cards[0]["action"], "created");
    assert_eq!(cards[0]["title"], "Rosa's Cafe");
    let id = cards[0]["record_id"].as_str().expect("id").to_string();
    let view = record_view(&state, &cookie, office, &id).await;
    assert_eq!(view["app_title"], "Lead Pipeline & CRM");
    assert_eq!(view["data"]["estimated_value"], 900);
    assert!(view["gone"].is_null());
    assert_eq!(
        view["contact"]["name"], "Rosa's Cafe",
        "linked to its contact"
    );
    assert_eq!(view["pending"].as_array().expect("pending").len(), 0);

    // UPDATE by an Ask first bot: nothing changes until the owner says so.
    let sent = say(&state, &cookie, room, "Rosa just said yes, mark her Won").await;
    let (body, cards) = bot_note(&pool, room, bot, sent, "waits for your OK").await;
    assert!(body.contains("stage: New Lead → Won"), "{body}");
    let card = &cards.expect("cards")[0];
    assert_eq!(card["action"], "proposed_update");
    assert_eq!(card["changed_fields"][0], "stage");
    let approval = card["approval_id"].as_str().expect("approval").to_string();
    let view = record_view(&state, &cookie, office, &id).await;
    assert_eq!(view["status"], "New Lead", "not applied yet");
    assert_eq!(view["pending"][0]["approval_id"], approval.as_str());
    assert_eq!(view["pending"][0]["changes"]["stage"], "Won");
    assert_eq!(view["pending"][0]["requested_by"], "Sales Lead");
    // It's in the approvals inbox like any other.
    let waiting: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM approvals WHERE id = $1::uuid AND status = 'pending'",
    )
    .bind(&approval)
    .fetch_one(&pool)
    .await
    .expect("approval");
    assert_eq!(waiting, 1);

    // Approve with the owner's edits: their values win.
    let (status, bad) = call(
        &state,
        "POST",
        &format!("/approvals/{approval}/decide"),
        &cookie,
        Some(json!({ "decision": "approve", "changes": [1] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}");
    let (status, decided) = call(
        &state,
        "POST",
        &format!("/approvals/{approval}/decide"),
        &cookie,
        Some(json!({ "decision": "approve", "changes": { "stage": "Qualified" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{decided:?}");
    let view = record_view(&state, &cookie, office, &id).await;
    assert_eq!(view["status"], "Qualified");
    assert_eq!(view["data"]["stage"], "Qualified");
    assert!(view["pending"].as_array().expect("pending").is_empty());
    let (outcome, note): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT t.outcome, t.outcome_note FROM tasks t JOIN approvals a ON a.task_id = t.id WHERE a.id = $1::uuid",
    )
    .bind(&approval)
    .fetch_one(&pool)
    .await
    .expect("job");
    assert_eq!(outcome.as_deref(), Some("done"));
    assert_eq!(note.as_deref(), Some("approved with your edits"));

    // Another proposal, rejected: the record stays as it was.
    let sent = say(&state, &cookie, room, "Actually mark her Won").await;
    let (_, cards) = bot_note(&pool, room, bot, sent, "She signed").await;
    let approval = cards.expect("cards")[0]["approval_id"]
        .as_str()
        .expect("approval")
        .to_string();
    let (status, _) = call(
        &state,
        "POST",
        &format!("/approvals/{approval}/decide"),
        &cookie,
        Some(json!({ "decision": "reject" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let view = record_view(&state, &cookie, office, &id).await;
    assert_eq!(view["status"], "Qualified");
    assert!(view["pending"].as_array().expect("pending").is_empty());

    // A Trusted bot's change applies at once.
    sqlx::query("UPDATE agents SET permission_mode = 'trusted' WHERE id = $1")
        .bind(bot)
        .execute(&pool)
        .await
        .expect("trust");
    let sent = say(&state, &cookie, room, "Set Rosa's value 1500").await;
    let (body, cards) = bot_note(&pool, room, bot, sent, "Updated Rosa's Cafe").await;
    assert!(body.contains("estimated_value: 900 → 1500"), "{body}");
    let card = &cards.expect("cards")[0];
    assert_eq!(card["action"], "updated");
    assert_eq!(card["changed_fields"][0], "estimated_value");
    assert!(card.get("approval_id").is_none());
    let view = record_view(&state, &cookie, office, &id).await;
    assert_eq!(view["data"]["estimated_value"], 1500);
}

#[sqlx::test]
async fn cards_show_tombstones_and_never_another_offices_records(pool: PgPool) {
    let state = local_state(pool.clone());
    let (cookie, office, _, _) = setup(&state).await;
    let (status, rec) = call(
        &state,
        "POST",
        &format!("/api/office/apps/leads-pipeline/records?office={office}"),
        &cookie,
        Some(json!({ "record_type": "lead", "data": { "name": "Ben" } })),
    )
    .await;
    assert!(status.is_success(), "{rec:?}");
    let id = rec["id"].as_str().expect("id").to_string();
    let unknown = Uuid::new_v4();
    let (_, both) = call(
        &state,
        "GET",
        &format!("/api/office/records?office={office}&ids={id},{unknown}"),
        &cookie,
        None,
    )
    .await;
    assert!(both["records"][0]["gone"].is_null());
    assert_eq!(both["records"][1]["gone"], "deleted");
    assert!(both["records"][1].get("data").is_none());

    // Another office of the same owner can't read it through its id.
    let (_, other) = call(
        &state,
        "POST",
        "/api/offices",
        &cookie,
        Some(json!({ "name": "Second" })),
    )
    .await;
    let other: Uuid = other["office"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let view = record_view(&state, &cookie, other, &id).await;
    assert_eq!(view["gone"], "deleted");
    assert!(view.get("data").is_none(), "{view:?}");

    // Archived app, then deleted record.
    let (status, _) = call(
        &state,
        "POST",
        &format!("/api/office/apps/leads-pipeline/archive?office={office}"),
        &cookie,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        record_view(&state, &cookie, office, &id).await["gone"],
        "archived"
    );
    call(
        &state,
        "POST",
        &format!("/api/office/apps/leads-pipeline/unarchive?office={office}"),
        &cookie,
        Some(json!({})),
    )
    .await;
    let (status, _) = call(
        &state,
        "POST",
        &format!("/api/office/records/{id}/delete?office={office}"),
        &cookie,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        record_view(&state, &cookie, office, &id).await["gone"],
        "deleted"
    );

    // Bad requests.
    let (status, _) = call(
        &state,
        "GET",
        &format!("/api/office/records?office={office}&ids=nope"),
        &cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let many = (0..51)
        .map(|_| Uuid::new_v4().to_string())
        .collect::<Vec<_>>()
        .join(",");
    let (status, _) = call(
        &state,
        "GET",
        &format!("/api/office/records?office={office}&ids={many}"),
        &cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let stranger = Uuid::new_v4();
    let (status, _) = call(
        &state,
        "GET",
        &format!("/api/office/records?office={stranger}&ids={id}"),
        &cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let res = send_with_state(
        state.clone(),
        request(
            "GET",
            &format!("/api/office/events?office={stranger}"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn record_changes_stream_live_to_the_office(pool: PgPool) {
    let state = local_state(pool.clone());
    let (cookie, office, _, _) = setup(&state).await;
    let (_, rec) = call(
        &state,
        "POST",
        &format!("/api/office/apps/leads-pipeline/records?office={office}"),
        &cookie,
        Some(json!({ "record_type": "lead", "data": { "name": "Cara" } })),
    )
    .await;
    let id = rec["id"].as_str().expect("id").to_string();

    let response = airobot::app(state.clone())
        .oneshot(request(
            "GET",
            &format!("/api/office/events?office={office}"),
            Some(&cookie),
            None,
        ))
        .await
        .expect("events");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap_or("")
        .starts_with("text/event-stream"));
    let mut stream = response.into_body().into_data_stream();

    let (status, _) = call(
        &state,
        "PATCH",
        &format!("/api/office/records/{id}?office={office}"),
        &cookie,
        Some(json!({ "status": "Qualified" })),
    )
    .await;
    assert!(status.is_success());
    let mut seen = String::new();
    for _ in 0..5 {
        let Ok(Some(Ok(chunk))) =
            tokio::time::timeout(std::time::Duration::from_secs(5), futures_next(&mut stream))
                .await
        else {
            break;
        };
        seen.push_str(&String::from_utf8_lossy(&chunk));
        if seen.contains(&id) {
            break;
        }
    }
    assert!(seen.contains("event: record"), "{seen}");
    assert!(seen.contains(&id), "{seen}");
    assert!(seen.contains("\"kind\":\"updated\""), "{seen}");
}

async fn futures_next(
    stream: &mut axum::body::BodyDataStream,
) -> Option<Result<axum::body::Bytes, axum::Error>> {
    use tokio_stream::StreamExt;
    stream.next().await
}
