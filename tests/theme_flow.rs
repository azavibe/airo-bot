// Appearance: a person's theme is validated, normalised, kept on their
// account and returned by /api/me so it follows them everywhere.
mod common;

use airobot::auth::secret;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::{authed_json_request, form_request, get_request, json_request, send, session_cookie};
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

#[sqlx::test]
async fn a_theme_is_validated_kept_and_reset(pool: PgPool) {
    let cookie = sign_in(&pool, "painter@example.com", "painter").await;
    assert!(
        me(&pool, &cookie).await["ui_theme"].is_null(),
        "default theme"
    );

    let saved = send(
        pool.clone(),
        authed_json_request(
            "PUT",
            "/api/me/theme",
            json!({
                "theme": {
                    "mode": "light", "accent": "#2563EB", "background": "#dfe7f5",
                    "backgroundStyle": "glow", "surface": "#ffffff", "border": "#1e3a8a",
                    "text": "#0f172a", "userBubble": "#dbe7ff", "radius": 20, "blur": 0
                },
                "idempotency_key": "theme-1"
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(saved.status, StatusCode::OK);
    // Colours are normalised to lower case.
    assert_eq!(saved.body["ui_theme"]["accent"], "#2563eb");

    let profile = me(&pool, &cookie).await;
    assert_eq!(profile["ui_theme"]["mode"], "light");
    assert_eq!(profile["ui_theme"]["radius"], 20);

    // Anything that is not a known, well-formed setting is refused whole,
    // so the stored theme can never carry arbitrary CSS.
    for bad in [
        json!({ "accent": "red" }),
        json!({ "accent": "#12345" }),
        json!({ "background": "url(javascript:alert(1))" }),
        json!({ "radius": 99 }),
        json!({ "blur": -1 }),
        json!({ "mode": "neon" }),
        json!({ "fontFamily": "Comic Sans" }),
        json!("dark"),
    ] {
        let res = send(
            pool.clone(),
            authed_json_request(
                "PUT",
                "/api/me/theme",
                json!({ "theme": bad }),
                Some(&cookie),
            ),
        )
        .await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(
        me(&pool, &cookie).await["ui_theme"]["accent"],
        "#2563eb",
        "unchanged"
    );

    // A person only ever changes their own theme.
    let other = sign_in(&pool, "other@example.com", "other").await;
    assert!(me(&pool, &other).await["ui_theme"].is_null());

    // null goes back to the default.
    let reset = send(
        pool.clone(),
        authed_json_request(
            "PUT",
            "/api/me/theme",
            json!({ "theme": null }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(reset.status, StatusCode::OK);
    assert!(me(&pool, &cookie).await["ui_theme"].is_null());
}

#[sqlx::test]
async fn saving_a_theme_needs_an_account(pool: PgPool) {
    let res = send(
        pool.clone(),
        json_request(
            "PUT",
            "/api/me/theme",
            json!({ "theme": { "accent": "#ffffff" } }),
        ),
    )
    .await;
    assert!(
        res.status == StatusCode::UNAUTHORIZED || res.status == StatusCode::FORBIDDEN,
        "{}",
        res.status
    );
}
