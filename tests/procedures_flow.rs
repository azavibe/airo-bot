mod common;

use airobot::auth::secret;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::{
    authed_json_request, bearer_get_request, bearer_json_request, form_request, get_request,
    json_request, post_request, send, session_cookie,
};
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
    let callback = send(
        pool.clone(),
        get_request(&format!("/auth/callback?token={raw_token}"), None),
    )
    .await;
    let cookie = session_cookie(&callback.headers).expect("session cookie");
    let handle_response = send(
        pool.clone(),
        form_request(
            "POST",
            "/onboarding/handle",
            &[("handle", handle)],
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(handle_response.status, StatusCode::OK);
    cookie
}

async fn create_agent(pool: &PgPool, cookie: &str, name: &str) -> String {
    let code_response = send(pool.clone(), post_request("/device/code", None)).await;
    let code = code_response.body["code"].as_str().expect("code");
    let device_code = code_response.body["device_code"]
        .as_str()
        .expect("device code");
    let approve = send(
        pool.clone(),
        form_request(
            "POST",
            "/device/approve",
            &[("code", code), ("agent_name", name)],
            Some(cookie),
        ),
    )
    .await;
    assert_eq!(approve.status, StatusCode::OK);
    let poll = send(
        pool.clone(),
        json_request(
            "POST",
            "/device/token",
            json!({ "device_code": device_code }),
        ),
    )
    .await;
    poll.body["agent_key"]
        .as_str()
        .expect("agent key")
        .to_string()
}

async fn create_procedure(pool: &PgPool, agent_key: &str, title: &str) -> Value {
    let response = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            "/procedures",
            json!({
                "title": title,
                "summary": "A repeatable test procedure",
                "body": "1. Do the work.\n2. Verify it.",
                "check_description": "The verification passes."
            }),
            agent_key,
        ),
    )
    .await;
    assert_eq!(response.status, StatusCode::CREATED);
    response.body
}

async fn publish_procedure(pool: &PgPool, agent_key: &str, procedure_id: &str) -> Value {
    let response = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            &format!("/procedures/{procedure_id}/publish"),
            json!({ "visibility": "public" }),
            agent_key,
        ),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    response.body
}

async fn record_run(
    pool: &PgPool,
    agent_key: &str,
    procedure_id: &str,
    model: &str,
    outcome: &str,
    idempotency_key: &str,
) -> common::TestResponse {
    send(
        pool.clone(),
        bearer_json_request(
            "POST",
            "/runs",
            json!({
                "procedure_id": procedure_id,
                "model": model,
                "outcome": outcome,
                "attempts": 1,
                "duration_ms": 1250,
                "idempotency_key": idempotency_key
            }),
            agent_key,
        ),
    )
    .await
}

#[sqlx::test]
async fn create_procedure_requires_auth(pool: PgPool) {
    let response = send(
        pool,
        json_request(
            "POST",
            "/procedures",
            json!({
                "title": "Deploy safely",
                "summary": "Deploy with a health check",
                "body": "Deploy, then check health.",
                "check_description": "Health returns 200."
            }),
        ),
    )
    .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn create_is_private_and_writes_initial_version(pool: PgPool) {
    let cookie = sign_in(&pool, "owner@example.com", "owner").await;
    let agent_key = create_agent(&pool, &cookie, "builder").await;
    let procedure = create_procedure(&pool, &agent_key, "Deploy safely").await;
    assert_eq!(procedure["visibility"], "private");
    assert_eq!(procedure["current_version"], 1);

    let procedure_id: Uuid = procedure["id"].as_str().unwrap().parse().unwrap();
    let version_body: String = sqlx::query_scalar(
        "SELECT body FROM procedure_versions WHERE procedure_id = $1 AND version = 1",
    )
    .bind(procedure_id)
    .fetch_one(&pool)
    .await
    .expect("initial version");
    assert_eq!(version_body, "1. Do the work.\n2. Verify it.");
}

#[sqlx::test]
async fn create_idempotency_key_returns_one_procedure(pool: PgPool) {
    let cookie = sign_in(&pool, "owner@example.com", "owner").await;
    let body = json!({
        "title": "Deploy safely",
        "summary": "Deploy with a health check",
        "body": "Deploy, then check health.",
        "check_description": "Health returns 200.",
        "idempotency_key": "procedure-create-1"
    });
    let first = send(
        pool.clone(),
        authed_json_request("POST", "/procedures", body.clone(), Some(&cookie)),
    )
    .await;
    let replay = send(
        pool.clone(),
        authed_json_request("POST", "/procedures", body, Some(&cookie)),
    )
    .await;
    assert_eq!(first.status, StatusCode::CREATED);
    assert_eq!(replay.status, StatusCode::OK);
    assert_eq!(first.body["id"], replay.body["id"]);

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM procedures")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[sqlx::test]
async fn private_procedure_is_owner_only_and_supports_plain_text(pool: PgPool) {
    let cookie = sign_in(&pool, "owner@example.com", "owner").await;
    let owner_agent = create_agent(&pool, &cookie, "builder").await;
    let procedure = create_procedure(&pool, &owner_agent, "Deploy safely").await;
    let slug = procedure["slug"].as_str().unwrap();

    let anonymous = send(
        pool.clone(),
        get_request(&format!("/procedures/owner/{slug}"), None),
    )
    .await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);

    let mut request = bearer_get_request(&format!("/procedures/owner/{slug}"), &owner_agent);
    request
        .headers_mut()
        .insert("accept", "text/plain".parse().unwrap());
    let plain = send(pool, request).await;
    assert_eq!(plain.status, StatusCode::OK);
    assert_eq!(plain.text, "1. Do the work.\n2. Verify it.");
}

#[sqlx::test]
async fn patch_appends_a_version_and_preserves_the_published_body_history(pool: PgPool) {
    let cookie = sign_in(&pool, "owner@example.com", "owner").await;
    let agent_key = create_agent(&pool, &cookie, "builder").await;
    let procedure = create_procedure(&pool, &agent_key, "Deploy safely").await;
    let procedure_id = procedure["id"].as_str().unwrap();
    publish_procedure(&pool, &agent_key, procedure_id).await;

    let body = json!({
        "body": "1. Deploy.\n2. Check health.\n3. Check logs.",
        "changelog": "Add a log check",
        "idempotency_key": "version-2"
    });
    let update = send(
        pool.clone(),
        bearer_json_request(
            "PATCH",
            &format!("/procedures/{procedure_id}"),
            body.clone(),
            &agent_key,
        ),
    )
    .await;
    assert_eq!(update.status, StatusCode::CREATED);
    assert_eq!(update.body["procedure"]["current_version"], 2);
    assert_eq!(update.body["version"]["version"], 2);

    let replay = send(
        pool.clone(),
        bearer_json_request(
            "PATCH",
            &format!("/procedures/{procedure_id}"),
            body,
            &agent_key,
        ),
    )
    .await;
    assert_eq!(replay.status, StatusCode::OK);
    assert_eq!(replay.body["version"]["version"], 2);

    let id: Uuid = procedure_id.parse().unwrap();
    let versions: Vec<(i32, String)> = sqlx::query_as(
        "SELECT version, body FROM procedure_versions WHERE procedure_id = $1 ORDER BY version",
    )
    .bind(id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].1, "1. Do the work.\n2. Verify it.");
    assert!(versions[1].1.contains("Check logs"));
}

#[sqlx::test]
async fn publishing_makes_the_procedure_publicly_readable(pool: PgPool) {
    let cookie = sign_in(&pool, "owner@example.com", "owner").await;
    let agent_key = create_agent(&pool, &cookie, "builder").await;
    let procedure = create_procedure(&pool, &agent_key, "Deploy safely").await;
    let procedure_id = procedure["id"].as_str().unwrap();
    let published = publish_procedure(&pool, &agent_key, procedure_id).await;
    assert_eq!(published["visibility"], "public");
    assert!(!published["published_at"].is_null());

    let slug = published["slug"].as_str().unwrap();
    let public = send(
        pool,
        get_request(&format!("/procedures/owner/{slug}"), None),
    )
    .await;
    assert_eq!(public.status, StatusCode::OK);
    assert_eq!(public.body["procedure"]["id"], procedure_id);
}

#[sqlx::test]
async fn only_an_agent_can_record_a_run(pool: PgPool) {
    let cookie = sign_in(&pool, "owner@example.com", "owner").await;
    let agent_key = create_agent(&pool, &cookie, "builder").await;
    let procedure = create_procedure(&pool, &agent_key, "Deploy safely").await;
    let response = send(
        pool,
        authed_json_request(
            "POST",
            "/runs",
            json!({
                "procedure_id": procedure["id"],
                "model": "claude-opus-5",
                "outcome": "pass",
                "duration_ms": 1000
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
}

/// Phase 3's complete acceptance path: publish one procedure, run it from
/// two independently authenticated agents using different model labels,
/// and read both model success rates back from the stats endpoint.
#[sqlx::test]
async fn stats_break_success_rate_down_by_model(pool: PgPool) {
    let owner_cookie = sign_in(&pool, "owner@example.com", "owner").await;
    let owner_agent = create_agent(&pool, &owner_cookie, "builder").await;
    let procedure = create_procedure(&pool, &owner_agent, "Deploy safely").await;
    let procedure_id = procedure["id"].as_str().unwrap();
    publish_procedure(&pool, &owner_agent, procedure_id).await;

    let runner_cookie = sign_in(&pool, "runner@example.com", "runner").await;
    let runner_agent = create_agent(&pool, &runner_cookie, "runner-agent").await;
    let first = record_run(
        &pool,
        &owner_agent,
        procedure_id,
        "claude-opus-5",
        "pass",
        "run-claude-1",
    )
    .await;
    let second = record_run(
        &pool,
        &runner_agent,
        procedure_id,
        "gpt-5.6",
        "fail",
        "run-gpt-1",
    )
    .await;
    assert_eq!(first.status, StatusCode::CREATED);
    assert_eq!(second.status, StatusCode::CREATED);

    let stats = send(
        pool,
        get_request(&format!("/procedures/{procedure_id}/stats"), None),
    )
    .await;
    assert_eq!(stats.status, StatusCode::OK);
    assert_eq!(stats.body["overall"]["run_count"], 2);
    assert_eq!(stats.body["overall"]["success_count"], 1);
    assert_eq!(stats.body["overall"]["success_rate"], 0.5);

    let models = stats.body["by_model"].as_array().unwrap();
    let claude = models
        .iter()
        .find(|entry| entry["model"] == "claude-opus-5")
        .unwrap();
    let gpt = models
        .iter()
        .find(|entry| entry["model"] == "gpt-5.6")
        .unwrap();
    assert_eq!(claude["success_rate"], 1.0);
    assert_eq!(gpt["success_rate"], 0.0);
}

#[sqlx::test]
async fn run_idempotency_does_not_inflate_counters(pool: PgPool) {
    let cookie = sign_in(&pool, "owner@example.com", "owner").await;
    let agent_key = create_agent(&pool, &cookie, "builder").await;
    let procedure = create_procedure(&pool, &agent_key, "Deploy safely").await;
    let procedure_id = procedure["id"].as_str().unwrap();

    let first = record_run(
        &pool,
        &agent_key,
        procedure_id,
        "claude-opus-5",
        "pass",
        "same-run",
    )
    .await;
    let replay = record_run(
        &pool,
        &agent_key,
        procedure_id,
        "claude-opus-5",
        "pass",
        "same-run",
    )
    .await;
    assert_eq!(first.status, StatusCode::CREATED);
    assert_eq!(replay.status, StatusCode::OK);
    assert_eq!(first.body["id"], replay.body["id"]);

    let id: Uuid = procedure_id.parse().unwrap();
    let counters: (i64, i64) =
        sqlx::query_as("SELECT run_count, success_count FROM procedures WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(counters, (1, 1));
}

#[sqlx::test]
async fn owner_can_delete_procedure(pool: PgPool) {
    let cookie = sign_in(&pool, "owner@example.com", "owner").await;
    let stranger = sign_in(&pool, "stranger@example.com", "stranger").await;
    let agent_key = create_agent(&pool, &cookie, "builder").await;
    let procedure = create_procedure(&pool, &agent_key, "Procedure to delete").await;
    let procedure_id = procedure["id"].as_str().unwrap();

    // Stranger cannot delete
    let deny = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/procedures/{procedure_id}/delete"),
            json!({}),
            Some(&stranger),
        ),
    )
    .await;
    assert_eq!(deny.status, StatusCode::FORBIDDEN);

    // Owner can delete
    let delete = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/procedures/{procedure_id}/delete"),
            json!({}),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(delete.status, StatusCode::OK);
    assert_eq!(delete.body["deleted"], true);

    // Verify it is marked as deleted in database
    let id: Uuid = procedure_id.parse().unwrap();
    let deleted_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM procedures WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(deleted_at.is_some());
}
