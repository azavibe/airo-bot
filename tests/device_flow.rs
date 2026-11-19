mod common;

#[sqlx::test]
async fn device_creation_retry_does_not_duplicate_or_replay_secret(pool: PgPool) {
    let request = || {
        json_request(
            "POST",
            "/device/code",
            json!({"idempotency_key":"one-device"}),
        )
    };
    let a = send(pool.clone(), request()).await;
    let b = send(pool.clone(), request()).await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(b.status, StatusCode::CONFLICT);
    assert!(b.body.get("device_code").is_none());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM login_codes")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 1);
}

use airobot::auth::secret;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use common::{form_request, get_request, json_request, post_request, send, session_cookie};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn sign_in(pool: &PgPool, email: &str, handle: &str) -> String {
    let id = Uuid::new_v4();
    let raw_secret = secret::random_hex_token(32);
    let token_hash = secret::hash_secret(&raw_secret).expect("hash");
    let expires_at = Utc::now() + Duration::minutes(15);
    sqlx::query(
        "INSERT INTO email_tokens (id, email, token_hash, purpose, expires_at)
         VALUES ($1, $2, $3, 'login', $4)",
    )
    .bind(id)
    .bind(email)
    .bind(&token_hash)
    .bind(expires_at)
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

struct IssuedDeviceCode {
    code: String,
    device_code: String,
}

async fn request_device_code(pool: &PgPool) -> IssuedDeviceCode {
    let res = send(pool.clone(), post_request("/device/code", None)).await;
    assert_eq!(res.status, StatusCode::OK);
    IssuedDeviceCode {
        code: res.body["code"].as_str().expect("code").to_string(),
        device_code: res.body["device_code"]
            .as_str()
            .expect("device code")
            .to_string(),
    }
}

#[sqlx::test]
async fn device_code_creates_a_row(pool: PgPool) {
    let issued = request_device_code(&pool).await;
    let stored: String = sqlx::query_scalar("SELECT code FROM login_codes WHERE code = $1")
        .bind(&issued.code)
        .fetch_one(&pool)
        .await
        .expect("fetch");
    assert_eq!(stored, issued.code);
    assert!(!issued.device_code.contains(&stored));
    let hash: String =
        sqlx::query_scalar("SELECT device_secret_hash FROM login_codes WHERE code = $1")
            .bind(&stored)
            .fetch_one(&pool)
            .await
            .expect("device secret hash");
    assert!(hash.starts_with("$argon2"));
}

#[sqlx::test]
async fn verify_form_requires_auth(pool: PgPool) {
    let issued = request_device_code(&pool).await;
    let res = send(
        pool,
        get_request(&format!("/device?code={}", issued.code), None),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn poll_token_is_pending_before_approval(pool: PgPool) {
    let issued = request_device_code(&pool).await;
    let short_code_poll = send(
        pool.clone(),
        json_request("POST", "/device/token", json!({ "code": &issued.code })),
    )
    .await;
    assert_eq!(short_code_poll.body["status"], "expired");
    let res = send(
        pool,
        json_request(
            "POST",
            "/device/token",
            json!({ "device_code": &issued.device_code }),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.body["status"], "pending");
}

#[sqlx::test]
async fn poll_token_is_expired_for_an_unknown_code(pool: PgPool) {
    let res = send(
        pool,
        json_request("POST", "/device/token", json!({ "code": "NOPENOPE" })),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.body["status"], "expired");
}

#[sqlx::test]
async fn approve_then_poll_hands_back_the_agent_key_exactly_once(pool: PgPool) {
    let cookie = sign_in(&pool, "device@example.com", "deviceuser").await;
    let issued = request_device_code(&pool).await;

    let approve_res = send(
        pool.clone(),
        form_request(
            "POST",
            "/device/approve",
            &[("code", &issued.code), ("agent_name", "my-agent")],
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(approve_res.status, StatusCode::OK);

    let poll1 = send(
        pool.clone(),
        json_request(
            "POST",
            "/device/token",
            json!({ "device_code": &issued.device_code }),
        ),
    )
    .await;
    assert_eq!(poll1.status, StatusCode::OK);
    assert_eq!(poll1.body["status"], "approved");
    assert_eq!(poll1.body["agent_name"], "my-agent");
    let agent_key = poll1.body["agent_key"]
        .as_str()
        .expect("agent_key")
        .to_string();
    assert!(agent_key.starts_with("air_ag_"));

    // "exactly once": a second poll for the same device credential must not repeat it.
    let poll2 = send(
        pool.clone(),
        json_request(
            "POST",
            "/device/token",
            json!({ "device_code": &issued.device_code }),
        ),
    )
    .await;
    assert_eq!(poll2.body["status"], "expired");

    // The minted key actually authenticates as an agent. Agents can't use
    // this browser-only page, so a 403 (not 401) proves the Bearer key
    // resolved to a real Caller::Agent rather than being rejected outright.
    let auth_res = send(
        pool.clone(),
        Request::builder()
            .method("GET")
            .uri("/onboarding/handle")
            .header("x-forwarded-for", "127.0.0.1")
            .header("authorization", format!("Bearer {agent_key}"))
            .body(Body::empty())
            .expect("build request"),
    )
    .await;
    assert_eq!(auth_res.status, StatusCode::FORBIDDEN);

    let hb_res = send(
        pool,
        Request::builder()
            .method("POST")
            .uri("/agent/heartbeat")
            .header("x-forwarded-for", "127.0.0.1")
            .header("authorization", format!("Bearer {agent_key}"))
            .body(Body::empty())
            .expect("heartbeat request"),
    )
    .await;
    assert_eq!(hb_res.status, StatusCode::OK);
    assert_eq!(hb_res.body["status"], "online");
    assert_eq!(hb_res.body["agent_name"], "my-agent");
}

#[sqlx::test]
async fn deny_blocks_the_poll(pool: PgPool) {
    let cookie = sign_in(&pool, "deny@example.com", "denyuser").await;
    let issued = request_device_code(&pool).await;

    let deny_res = send(
        pool.clone(),
        form_request(
            "POST",
            "/device/deny",
            &[("code", &issued.code)],
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(deny_res.status, StatusCode::OK);

    let poll = send(
        pool,
        json_request(
            "POST",
            "/device/token",
            json!({ "device_code": &issued.device_code }),
        ),
    )
    .await;
    assert_eq!(poll.body["status"], "denied");
}

#[sqlx::test]
async fn approve_rejects_a_duplicate_agent_name(pool: PgPool) {
    let cookie = sign_in(&pool, "dup@example.com", "dupuser").await;

    let issued1 = request_device_code(&pool).await;
    let approve1 = send(
        pool.clone(),
        form_request(
            "POST",
            "/device/approve",
            &[("code", &issued1.code), ("agent_name", "same-name")],
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(approve1.status, StatusCode::OK);
    let poll1 = send(
        pool.clone(),
        json_request(
            "POST",
            "/device/token",
            json!({ "device_code": &issued1.device_code }),
        ),
    )
    .await;
    assert_eq!(poll1.body["status"], "approved");

    let issued2 = request_device_code(&pool).await;
    let approve2 = send(
        pool.clone(),
        form_request(
            "POST",
            "/device/approve",
            &[("code", &issued2.code), ("agent_name", "same-name")],
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(approve2.status, StatusCode::CONFLICT);
}

#[sqlx::test]
async fn approve_requires_auth(pool: PgPool) {
    let issued = request_device_code(&pool).await;
    let res = send(
        pool,
        form_request(
            "POST",
            "/device/approve",
            &[("code", &issued.code), ("agent_name", "x")],
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}
