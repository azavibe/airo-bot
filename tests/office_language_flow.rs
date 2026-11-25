// A language per office (issue #31): the owner picks one of the offered
// languages in Edit office, and the office's bots are told to write
// customer-facing text in it.
mod common;

use airobot::auth::secret;
use airobot::office::context;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::{authed_json_request, form_request, get_request, send, session_cookie};
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

async fn new_office(pool: &PgPool, cookie: &str, name: &str) -> Uuid {
    let (status, body) = call(
        pool,
        "POST",
        "/api/offices",
        json!({ "name": name }),
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    body["office"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid")
}

#[sqlx::test]
async fn an_office_works_in_the_language_its_owner_picks(pool: PgPool) {
    let cookie = sign_in(&pool, "idioma@example.com", "idioma").await;
    let office = new_office(&pool, &cookie, "Panadería Sol").await;
    let uri = format!("/api/offices/{office}");

    // The languages on offer, with their own names.
    let (status, list) = call(&pool, "GET", "/api/languages", json!({}), &cookie).await;
    assert_eq!(status, StatusCode::OK);
    let languages = list["languages"].as_array().expect("languages");
    assert!(languages
        .iter()
        .any(|l| l["code"] == "es" && l["native"] == "Español" && l["name"] == "Spanish"));

    // None set at first: bots aren't told a language.
    let chat: Uuid =
        sqlx::query_scalar("SELECT id FROM threads WHERE workspace_id = $1 AND kind = 'office'")
            .bind(office)
            .fetch_one(&pool)
            .await
            .expect("office chat");
    let told = context::prompt_for_thread(&pool, chat, Uuid::new_v4())
        .await
        .expect("office prompt");
    assert!(!told.contains("## Language"), "{told}");

    // Picked: saved, listed and told to the bots.
    let (status, saved) = call(
        &pool,
        "PATCH",
        &uri,
        json!({ "language": "es", "idempotency_key": "lang-1" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved:?}");
    assert_eq!(saved["language"], "es");
    assert_eq!(saved["name"], "Panadería Sol", "the name is kept");
    let (_, offices) = call(&pool, "GET", "/api/offices", json!({}), &cookie).await;
    let listed = offices["offices"]
        .as_array()
        .and_then(|o| o.iter().find(|o| o["id"] == office.to_string()))
        .cloned()
        .expect("listed");
    assert_eq!(listed["language"], "es");
    let told = context::prompt_for_thread(&pool, chat, Uuid::new_v4())
        .await
        .expect("office prompt");
    assert!(
        told.contains("## Language\nThis office works in Spanish (Español)."),
        "{told}"
    );

    // Other changes keep it.
    let (_, saved) = call(
        &pool,
        "PATCH",
        &uri,
        json!({ "focus": "More customers" }),
        &cookie,
    )
    .await;
    assert_eq!(saved["language"], "es");

    // Only offered languages.
    let (status, err) = call(
        &pool,
        "PATCH",
        &uri,
        json!({ "language": "klingon" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["error"]
            .as_str()
            .unwrap_or_default()
            .contains("language"),
        "{err:?}"
    );

    // Nobody else sets it.
    let other = sign_in(&pool, "otro@example.com", "otro").await;
    let (status, _) = call(&pool, "PATCH", &uri, json!({ "language": "fr" }), &other).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let kept: Option<String> = sqlx::query_scalar("SELECT language FROM workspaces WHERE id = $1")
        .bind(office)
        .fetch_one(&pool)
        .await
        .expect("office");
    assert_eq!(kept.as_deref(), Some("es"));

    // Empty clears it.
    let (status, saved) = call(&pool, "PATCH", &uri, json!({ "language": "" }), &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert!(saved["language"].is_null());
    let told = context::prompt_for_thread(&pool, chat, Uuid::new_v4())
        .await
        .expect("office prompt");
    assert!(!told.contains("## Language"), "{told}");
}
