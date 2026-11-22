//! A kickoff that never invents: with no website, the Architect asks the
//! questions the team needs, and the owner's answers go into "About the
//! business" (once each); the finance area brings Quotes with Invoices.
//! Real router and database; a scripted local AI plays the Architect.

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
use uuid::Uuid;

const BASE: &str = "http://127.0.0.1:43137";
const HOST: &str = "127.0.0.1:43137";
const TOKEN: &str = "kickoff-test-token";

fn local_state(pool: PgPool) -> AppState {
    let ai: LocalAi = Arc::new(move |req: LocalAiRequest| {
        Box::pin(async move {
            if req.prompt.contains("Ask me the 3 questions") {
                return Ok("A few quick questions.\nQuestion: What do you sell, and at what prices?\nQuestion: Who are your customers?\nQuestion: What are your opening hours?".to_string());
            }
            if req.system.contains("ABOUT: <the fact>") && req.prompt.contains("sourdough") {
                return Ok("Thanks, saved.\nABOUT: Sourdough loaves cost $8\nABOUT: Open 7am to 3pm, closed Mondays".to_string());
            }
            Ok("On it.".to_string())
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

#[sqlx::test]
async fn the_architect_asks_and_the_answers_fill_about(pool: PgPool) {
    let state = local_state(pool.clone());
    let res = send_with_state(
        state.clone(),
        request("GET", &format!("/local/enter?token={TOKEN}"), None, None),
    )
    .await;
    let cookie = session_cookie(&res.headers).expect("session");
    let (_, made) = call(
        &state,
        "POST",
        "/api/offices",
        &cookie,
        Some(json!({ "name": "Crumb" })),
    )
    .await;
    let office: Uuid = made["office"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let (status, body) = call(
        &state,
        "POST",
        "/api/office/onboard",
        &cookie,
        Some(json!({ "workspace_id": office, "business_type": "Bakery", "replaced_saas": [], "goals": "", "teams": ["finance"], "idempotency_key": "onb" })),
    )
    .await;
    assert!(status.is_success(), "{body:?}");

    // The finance area brings Quotes with Invoices.
    let (_, apps) = call(
        &state,
        "GET",
        &format!("/api/office/apps?office={office}"),
        &cookie,
        None,
    )
    .await;
    let slugs: Vec<&str> = apps
        .as_array()
        .expect("apps")
        .iter()
        .filter_map(|a| a["slug"].as_str())
        .collect();
    assert!(
        slugs.contains(&"invoices-cashflow") && slugs.contains(&"quotes-proposals"),
        "{slugs:?}"
    );

    // No website: the Architect asks its questions in the office chat.
    let chat: Uuid =
        sqlx::query_scalar("SELECT id FROM threads WHERE workspace_id = $1 AND kind = 'office'")
            .bind(office)
            .fetch_one(&pool)
            .await
            .expect("office chat");
    let questions: String = wait_for(|| async {
        sqlx::query_scalar("SELECT body FROM notes WHERE thread_id = $1 AND body LIKE '%Question: What do you sell%'")
            .bind(chat)
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten()
    })
    .await
    .expect("the Architect asked");
    assert!(
        questions.contains("Question: What are your opening hours?"),
        "{questions}"
    );

    // The owner answers; the facts go into About the business.
    let (status, _) = call(
        &state,
        "POST",
        &format!("/threads/{chat}/notes"),
        &cookie,
        Some(json!({ "body": "We bake sourdough, $8 a loaf, open 7 to 3, closed Mondays", "kind": "status" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (body, actions): (String, Option<Value>) = wait_for(|| async {
        sqlx::query_as("SELECT body, actions FROM notes WHERE thread_id = $1 AND body LIKE '%Added to About the business%'")
            .bind(chat)
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten()
    })
    .await
    .expect("facts saved");
    assert!(body.starts_with("Thanks, saved."), "{body}");
    assert!(!body.contains("ABOUT:"), "{body}");
    assert!(
        body.contains("*Added to About the business: Sourdough loaves cost $8*"),
        "{body}"
    );
    assert_eq!(actions.expect("button")[0]["target"], "office");
    let about: Option<String> = sqlx::query_scalar("SELECT about FROM workspaces WHERE id = $1")
        .bind(office)
        .fetch_one(&pool)
        .await
        .expect("about");
    assert_eq!(
        about.as_deref(),
        Some("- Sourdough loaves cost $8.\n- Open 7am to 3pm, closed Mondays.")
    );

    // The same facts again add nothing.
    let (rest, added) =
        airobot::office::memory::apply_about(&pool, chat, "ok\nABOUT: sourdough loaves cost $8")
            .await;
    assert_eq!(rest, "ok");
    assert!(added.is_empty());
    // Text without the line is untouched.
    let (rest, added) =
        airobot::office::memory::apply_about(&pool, chat, "About: what we do").await;
    assert_eq!(rest, "About: what we do");
    assert!(added.is_empty());
}
