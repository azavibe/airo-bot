// The language a person reads the app in (issue #31): one of the
// translated languages or automatic, kept on the account and returned by
// /api/me so it follows them to every browser and the desktop app.
mod common;

use airobot::auth::secret;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::{authed_json_request, form_request, get_request, send, session_cookie};
use serde_json::json;
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

async fn me(pool: &PgPool, cookie: &str) -> serde_json::Value {
    send(
        pool.clone(),
        authed_json_request("GET", "/api/me", json!({}), Some(cookie)),
    )
    .await
    .body
}

async fn save(
    pool: &PgPool,
    cookie: Option<&str>,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let res = send(
        pool.clone(),
        authed_json_request("PUT", "/api/me/language", body, cookie),
    )
    .await;
    (res.status, res.body)
}

#[sqlx::test]
async fn the_app_language_is_kept_and_reset(pool: PgPool) {
    let cookie = sign_in(&pool, "lector@example.com", "lector").await;
    // Automatic at first.
    let profile = me(&pool, &cookie).await;
    assert!(profile["ui_language"].is_null(), "{profile}");

    // The translated languages are listed with the office languages.
    let listed = send(
        pool.clone(),
        authed_json_request("GET", "/api/languages", json!({}), Some(&cookie)),
    )
    .await;
    let ui = listed.body["ui"].as_array().expect("ui languages");
    assert!(ui.iter().any(|c| c == "es") && ui.iter().any(|c| c == "en"));

    let (status, saved) = save(
        &pool,
        Some(&cookie),
        json!({ "language": "es", "idempotency_key": "ui-lang-1" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["ui_language"], "es");
    assert_eq!(me(&pool, &cookie).await["ui_language"], "es");

    // Only translated languages: Swahili is an office language, not (yet)
    // an app language.
    let (status, err) = save(&pool, Some(&cookie), json!({ "language": "sw" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["error"]
            .as_str()
            .unwrap_or_default()
            .contains("language"),
        "{err}"
    );
    assert_eq!(me(&pool, &cookie).await["ui_language"], "es");

    // Someone else's choice is their own.
    let other = sign_in(&pool, "otra@example.com", "otra").await;
    assert!(me(&pool, &other).await["ui_language"].is_null());

    // Signed-in only.
    let (status, _) = save(&pool, None, json!({ "language": "fr" })).await;
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
        "{status}"
    );

    // Null goes back to automatic.
    let (status, saved) = save(&pool, Some(&cookie), json!({ "language": null })).await;
    assert_eq!(status, StatusCode::OK);
    assert!(saved["ui_language"].is_null());
    assert!(me(&pool, &cookie).await["ui_language"].is_null());
}
