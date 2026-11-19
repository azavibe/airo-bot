//! Integration tests for the Airobot "virtual office" endpoints and the
//! 2026-11-13 audit fixes: every endpoint goes through the real router and a
//! real database.

mod common;

use airobot::auth::secret;
use airobot::office::{budget_gate, fsm_dispatcher};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use common::{
    authed_json_request, bearer_get_request, form_request, get_request, json_request, post_request,
    send, session_cookie, TestResponse,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
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

async fn user_id(pool: &PgPool, handle: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE handle = $1")
        .bind(handle)
        .fetch_one(pool)
        .await
        .expect("user id")
}

/// Creates an agent for `owner` and returns (agent id, raw bearer key).
async fn create_agent(pool: &PgPool, owner: Uuid, name: &str) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let (raw_key, raw_secret, prefix) = airobot::auth::generate_agent_key(id);
    let hash = secret::hash_secret(&raw_secret).expect("hash");
    sqlx::query(
        "INSERT INTO agents (id, user_id, name, handle, key_hash, key_prefix)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(owner)
    .bind(name)
    .bind(format!("{name}-{}", &id.simple().to_string()[..6]))
    .bind(hash)
    .bind(prefix)
    .execute(pool)
    .await
    .expect("insert agent");
    (id, raw_key)
}

async fn send_with(state: airobot::state::AppState, req: Request<Body>) -> TestResponse {
    let response = airobot::app(state).oneshot(req).await.expect("request");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let text = String::from_utf8_lossy(&bytes).into_owned();
    TestResponse {
        status,
        headers,
        body,
        text,
    }
}

async fn create_task(pool: &PgPool, cookie: &str, body: Value) -> TestResponse {
    send(
        pool.clone(),
        authed_json_request("POST", "/tasks", body, Some(cookie)),
    )
    .await
}

fn no_server_provider() -> bool {
    std::env::var("OPENROUTER_API_KEY").map_or(true, |k| k.trim().is_empty())
}

// ---------- auth: dev login and QR login ----------

#[sqlx::test]
async fn dev_login_is_off_by_default_and_limited_to_test_domains(pool: PgPool) {
    let off = send(
        pool.clone(),
        json_request(
            "POST",
            "/api/auth/dev-login",
            json!({ "email": "founder@example.com" }),
        ),
    )
    .await;
    assert_eq!(off.status, StatusCode::NOT_FOUND);

    let on = common::state_with_dev_login(pool.clone());
    let real_inbox = send_with(
        on.clone(),
        json_request(
            "POST",
            "/api/auth/dev-login",
            json!({ "email": "victim@gmail.com" }),
        ),
    )
    .await;
    assert_eq!(real_inbox.status, StatusCode::BAD_REQUEST);
    let victims: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE email = 'victim@gmail.com'")
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(victims, 0);

    let ok = send_with(
        on,
        json_request(
            "POST",
            "/api/auth/dev-login",
            json!({ "email": "dev@airobot.local" }),
        ),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert!(session_cookie(&ok.headers).is_some());
    assert_eq!(ok.body["authenticated"], true);
}

#[sqlx::test]
async fn qr_link_signs_in_once(pool: PgPool) {
    let cookie = sign_in(&pool, "qr@example.com", "qr_user").await;
    let link = send(
        pool.clone(),
        post_request("/api/auth/qr-link", Some(&cookie)),
    )
    .await;
    assert_eq!(link.status, StatusCode::OK);
    let url = link.body["qr_url"].as_str().expect("qr url");
    let path = &url[url.find("/auth/qr").expect("path")..];

    let first = send(pool.clone(), get_request(path, None)).await;
    assert!(first.status.is_redirection());
    assert!(session_cookie(&first.headers).is_some());

    let again = send(pool.clone(), get_request(path, None)).await;
    assert!(again.status.is_client_error());

    let anon = send(pool, post_request("/api/auth/qr-link", None)).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);
}

// ---------- tasks: claim authorization, thread validation, executor ----------

#[sqlx::test]
async fn claiming_requires_membership_of_the_tasks_workspace(pool: PgPool) {
    let alice = sign_in(&pool, "alice@example.com", "alice").await;
    let bob = sign_in(&pool, "bob@example.com", "bob").await;
    let task = create_task(
        &pool,
        &alice,
        json!({ "title": "Summarise the board pack", "payload": { "secret": "q3 numbers" } }),
    )
    .await;
    assert_eq!(task.status, StatusCode::CREATED);
    let task_id = task.body["task"]["id"].as_str().expect("id").to_string();

    let stolen = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/tasks/{task_id}/claim"),
            json!({}),
            Some(&bob),
        ),
    )
    .await;
    assert_eq!(stolen.status, StatusCode::FORBIDDEN);
    assert!(!stolen.text.contains("q3 numbers"));

    let mine = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/tasks/{task_id}/claim"),
            json!({}),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(mine.status, StatusCode::OK);
    assert_eq!(mine.body["task"]["fsm_state"], "CLAIMED");
    // The sealed context pack never carries the raw secret.
    assert_eq!(
        mine.body["context_pack"]["payload"]["payload"]["secret"],
        "[redacted]"
    );
}

#[sqlx::test]
async fn tasks_cannot_point_at_someone_elses_thread(pool: PgPool) {
    let alice = sign_in(&pool, "alice2@example.com", "alice2").await;
    let bob = sign_in(&pool, "bob2@example.com", "bob2").await;
    let bobs_thread = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/threads",
            json!({ "title": "bob private" }),
            Some(&bob),
        ),
    )
    .await;
    let thread_id = bobs_thread.body["id"]
        .as_str()
        .or_else(|| bobs_thread.body["thread"]["id"].as_str())
        .expect("thread id")
        .to_string();

    let res = create_task(
        &pool,
        &alice,
        json!({ "title": "inject", "thread_id": thread_id }),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);

    let bad_executor =
        create_task(&pool, &alice, json!({ "title": "x", "executor": "robot" })).await;
    assert_eq!(bad_executor.status, StatusCode::BAD_REQUEST);

    let notes: i64 = sqlx::query_scalar("SELECT count(*) FROM notes WHERE thread_id = $1::uuid")
        .bind(&thread_id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(notes, 0);
}

#[sqlx::test]
async fn a_final_result_cannot_be_overwritten(pool: PgPool) {
    let alice = sign_in(&pool, "final@example.com", "final_user").await;
    let task = create_task(&pool, &alice, json!({ "title": "one result only" })).await;
    let task_id = task.body["task"]["id"].as_str().expect("id").to_string();
    let first = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/tasks/{task_id}/result"),
            json!({ "status": "completed" }),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK);
    let second = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/tasks/{task_id}/result"),
            json!({ "status": "failed" }),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(second.status, StatusCode::CONFLICT);
}

// ---------- dispatcher ----------

#[sqlx::test]
async fn dispatcher_leaves_agent_tasks_alone_and_never_fakes_results(pool: PgPool) {
    let state = common::state(pool.clone());
    let alice = sign_in(&pool, "disp@example.com", "disp_user").await;
    let agent_task = create_task(&pool, &alice, json!({ "title": "for my agent" })).await;
    let agent_task_id = agent_task.body["task"]["id"]
        .as_str()
        .expect("id")
        .to_string();

    assert_eq!(
        fsm_dispatcher::run_dispatcher_once(&state)
            .await
            .expect("tick"),
        0
    );
    let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = $1::uuid")
        .bind(&agent_task_id)
        .fetch_one(&pool)
        .await
        .expect("status");
    assert_eq!(status, "queued");

    let server_task = create_task(
        &pool,
        &alice,
        json!({ "title": "summarise inbox", "executor": "server" }),
    )
    .await;
    let server_task_id = server_task.body["task"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    // Server tasks belong to the dispatcher, not to agents.
    let claim = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/tasks/{server_task_id}/claim"),
            json!({}),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(claim.status, StatusCode::CONFLICT);

    if no_server_provider() {
        assert_eq!(
            fsm_dispatcher::run_dispatcher_once(&state)
                .await
                .expect("tick"),
            1
        );
        let (status, fsm): (String, String) =
            sqlx::query_as("SELECT status, fsm_state FROM tasks WHERE id = $1::uuid")
                .bind(&server_task_id)
                .fetch_one(&pool)
                .await
                .expect("row");
        assert_eq!((status.as_str(), fsm.as_str()), ("failed", "FAILED"));
        // Nothing was charged for work that never ran.
        let charged: i64 = sqlx::query_scalar("SELECT count(*) FROM credit_ledger")
            .fetch_one(&pool)
            .await
            .expect("ledger");
        assert_eq!(charged, 0);
    }
}

#[sqlx::test]
async fn an_approved_task_is_not_sent_back_for_approval(pool: PgPool) {
    let state = common::state(pool.clone());
    let alice = sign_in(&pool, "appr@example.com", "appr_user").await;
    let task = create_task(
        &pool,
        &alice,
        json!({ "title": "Send outbound offer", "executor": "server" }),
    )
    .await;
    let task_id = task.body["task"]["id"].as_str().expect("id").to_string();

    fsm_dispatcher::run_dispatcher_once(&state)
        .await
        .expect("tick");
    let fsm: String = sqlx::query_scalar("SELECT fsm_state FROM tasks WHERE id = $1::uuid")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .expect("fsm");
    assert_eq!(fsm, "WAITING_APPROVAL");
    let approval_id: Uuid = sqlx::query_scalar("SELECT id FROM approvals WHERE task_id = $1::uuid")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .expect("approval");

    let unclear = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/approvals/{approval_id}/decide"),
            json!({ "decision": "maybe" }),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(unclear.status, StatusCode::BAD_REQUEST);

    let approved = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/approvals/{approval_id}/decide"),
            json!({ "decision": "approved" }),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(approved.status, StatusCode::OK);

    fsm_dispatcher::run_dispatcher_once(&state)
        .await
        .expect("tick");
    let fsm: String = sqlx::query_scalar("SELECT fsm_state FROM tasks WHERE id = $1::uuid")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .expect("fsm");
    assert_ne!(fsm, "WAITING_APPROVAL");
    let approvals: i64 =
        sqlx::query_scalar("SELECT count(*) FROM approvals WHERE task_id = $1::uuid")
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(approvals, 1);

    // Office notes come from alice's own bot, never another user's agent.
    let foreign_authors: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notes n JOIN agents a ON a.id = n.author_agent_id
         JOIN tasks t ON t.thread_id = n.thread_id
         WHERE t.id = $1::uuid AND a.user_id <> $2",
    )
    .bind(&task_id)
    .bind(user_id(&pool, "appr_user").await)
    .fetch_one(&pool)
    .await
    .expect("authors");
    assert_eq!(foreign_authors, 0);
}

#[sqlx::test]
async fn io_webhook_needs_the_jobs_token_and_a_waiting_task(pool: PgPool) {
    let state = common::state(pool.clone());
    let alice = sign_in(&pool, "hook@example.com", "hook_user").await;
    let task = create_task(
        &pool,
        &alice,
        json!({ "title": "Generate video teaser", "executor": "server" }),
    )
    .await;
    let task_id = task.body["task"]["id"].as_str().expect("id").to_string();

    // Not parked yet: nothing to resume.
    let early = send(
        pool.clone(),
        json_request("POST", &format!("/api/webhooks/io/{task_id}"), json!({})),
    )
    .await;
    assert_eq!(early.status, StatusCode::NOT_FOUND);

    fsm_dispatcher::run_dispatcher_once(&state)
        .await
        .expect("tick");
    let detail = send(
        pool.clone(),
        authed_json_request("GET", &format!("/tasks/{task_id}"), json!({}), Some(&alice)),
    )
    .await;
    let hook_url = detail.body["webhook_url"]
        .as_str()
        .expect("webhook url")
        .to_string();
    let hook_path = &hook_url[hook_url.find("/api/webhooks").expect("path")..];

    let no_token = send(
        pool.clone(),
        json_request(
            "POST",
            &format!("/api/webhooks/io/{task_id}"),
            json!({ "x": 1 }),
        ),
    )
    .await;
    assert_eq!(no_token.status, StatusCode::NOT_FOUND);
    let bad_token = send(
        pool.clone(),
        json_request(
            "POST",
            &format!("/api/webhooks/io/{task_id}?token=00ff"),
            json!({ "x": 1 }),
        ),
    )
    .await;
    assert_eq!(bad_token.status, StatusCode::NOT_FOUND);

    let ok = send(
        pool.clone(),
        json_request(
            "POST",
            hook_path,
            json!({ "video_url": "https://cdn.example.com/v.mp4" }),
        ),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
    let replay = send(
        pool.clone(),
        json_request("POST", hook_path, json!({ "video_url": "x" })),
    )
    .await;
    assert_eq!(replay.status, StatusCode::CONFLICT);

    fsm_dispatcher::run_dispatcher_once(&state)
        .await
        .expect("tick");
    let (status, payload): (String, Value) =
        sqlx::query_as("SELECT status, state_payload FROM tasks WHERE id = $1::uuid")
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert_eq!(status, "completed");
    assert_eq!(
        payload["result"]["video_url"],
        "https://cdn.example.com/v.mp4"
    );

    // A completed task cannot be re-queued through the webhook.
    let after = send(pool.clone(), json_request("POST", hook_path, json!({}))).await;
    assert_eq!(after.status, StatusCode::CONFLICT);
}

#[sqlx::test]
async fn expired_leases_return_tasks_to_the_queue(pool: PgPool) {
    let alice = sign_in(&pool, "lease@example.com", "lease_user").await;
    let task = create_task(&pool, &alice, json!({ "title": "slow job" })).await;
    let task_id = task.body["task"]["id"].as_str().expect("id").to_string();
    let claim = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/tasks/{task_id}/claim"),
            json!({}),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(claim.status, StatusCode::OK);

    fsm_dispatcher::reap_stale_work(&pool).await.expect("reap");
    let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = $1::uuid")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .expect("status");
    assert_eq!(status, "assigned", "a live lease is left alone");

    sqlx::query(
        "UPDATE leases SET expires_at = now() - interval '1 minute' WHERE task_id = $1::uuid",
    )
    .bind(&task_id)
    .execute(&pool)
    .await
    .expect("expire");
    fsm_dispatcher::reap_stale_work(&pool).await.expect("reap");
    let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = $1::uuid")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .expect("status");
    assert_eq!(status, "queued");
}

#[sqlx::test]
async fn budget_gate_rolls_the_daily_burn_over_and_fails_closed_on_balance(pool: PgPool) {
    let alice = sign_in(&pool, "budget@example.com", "budget_user").await;
    let wallet = send(
        pool.clone(),
        get_request("/api/office/wallet", Some(&alice)),
    )
    .await;
    assert_eq!(wallet.status, StatusCode::OK);
    assert_eq!(wallet.body["credit_balance"], 50000);
    let ws: Uuid = wallet.body["workspace_id"]
        .as_str()
        .expect("ws")
        .parse()
        .expect("uuid");

    sqlx::query(
        "UPDATE workspace_wallets SET credits_burned_today = 5000, burn_day = CURRENT_DATE - 1 WHERE workspace_id = $1",
    )
    .bind(ws)
    .execute(&pool)
    .await
    .expect("seed");
    let proposal = budget_gate::ActionProposal {
        provider: "openrouter".into(),
        model_or_tool: "m".into(),
        estimated_credits: 25,
        raw_cost_micros: 1,
        requires_manual_approval: false,
        action_type: "call_llm".into(),
    };
    let gate = budget_gate::check_and_deduct(&pool, ws, &proposal, None, false)
        .await
        .expect("gate");
    assert!(gate.passed, "yesterday's burn must not block today");
    let burned: i64 = sqlx::query_scalar(
        "SELECT credits_burned_today FROM workspace_wallets WHERE workspace_id = $1",
    )
    .bind(ws)
    .fetch_one(&pool)
    .await
    .expect("burn");
    assert_eq!(burned, 25);

    sqlx::query("UPDATE workspace_wallets SET credit_balance = 10 WHERE workspace_id = $1")
        .bind(ws)
        .execute(&pool)
        .await
        .expect("drain");
    let gate = budget_gate::check_and_deduct(&pool, ws, &proposal, None, false)
        .await
        .expect("gate");
    assert!(!gate.passed);

    budget_gate::refund_credit_charge(&pool, ws, None, &proposal)
        .await
        .expect("refund");
    let balance: i64 =
        sqlx::query_scalar("SELECT credit_balance FROM workspace_wallets WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&pool)
            .await
            .expect("balance");
    assert_eq!(balance, 35);
}

// ---------- office apps, records, schema, lessons ----------

fn crm_schema() -> Value {
    json!({
        "version": "1.0",
        "views": [{ "id": "v1", "type": "datatable", "title": "Leads", "record_type": "lead",
                    "columns": [{ "key": "name", "label": "Name", "type": "text" }] }]
    })
}

#[sqlx::test]
async fn office_apps_and_records_are_private_to_the_workspace(pool: PgPool) {
    let alice = sign_in(&pool, "apps@example.com", "apps_user").await;
    let bob = sign_in(&pool, "apps_bob@example.com", "apps_bob").await;

    let bad = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/office/apps",
            json!({ "slug": "crm", "title": "CRM", "ui_schema": { "views": [] } }),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    let body = json!({ "slug": "crm", "title": "CRM", "ui_schema": crm_schema(), "idempotency_key": "app-1" });
    let created = send(
        pool.clone(),
        authed_json_request("POST", "/api/office/apps", body.clone(), Some(&alice)),
    )
    .await;
    assert_eq!(created.status, StatusCode::OK);
    let replay = send(
        pool.clone(),
        authed_json_request("POST", "/api/office/apps", body, Some(&alice)),
    )
    .await;
    assert_eq!(replay.body["id"], created.body["id"]);
    let dup = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/office/apps",
            json!({ "slug": "crm", "title": "CRM 2", "ui_schema": crm_schema() }),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(dup.status, StatusCode::CONFLICT);

    let listed = send(pool.clone(), get_request("/api/office/apps", Some(&alice))).await;
    assert_eq!(listed.body.as_array().expect("apps").len(), 1);
    let bobs = send(pool.clone(), get_request("/api/office/apps", Some(&bob))).await;
    assert_eq!(bobs.body.as_array().expect("apps").len(), 0);

    let record = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/office/apps/crm/records",
            json!({ "record_type": "lead", "data": { "name": "Acme" } }),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(record.status, StatusCode::OK);
    let record_id = record.body["id"].as_str().expect("record id").to_string();
    let duplicate = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/office/apps/crm/records",
            json!({ "record_type": "lead", "data": { "name": "Acme" } }),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(duplicate.status, StatusCode::CONFLICT);

    let records = send(
        pool.clone(),
        get_request("/api/office/apps/crm/records", Some(&alice)),
    )
    .await;
    assert_eq!(records.body.as_array().expect("records").len(), 1);
    let bob_records = send(
        pool.clone(),
        get_request("/api/office/apps/crm/records", Some(&bob)),
    )
    .await;
    assert_eq!(bob_records.status, StatusCode::NOT_FOUND);

    let bob_patch = send(
        pool.clone(),
        authed_json_request(
            "PATCH",
            &format!("/api/office/records/{record_id}"),
            json!({ "status": "won" }),
            Some(&bob),
        ),
    )
    .await;
    assert_eq!(bob_patch.status, StatusCode::NOT_FOUND);
    let patched = send(
        pool.clone(),
        authed_json_request(
            "PATCH",
            &format!("/api/office/records/{record_id}"),
            json!({ "status": "won" }),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(patched.status, StatusCode::OK);
    assert_eq!(patched.body["status"], "won");
    assert_eq!(patched.body["data"]["name"], "Acme");

    let mut new_schema = crm_schema();
    new_schema["views"][0]["title"] = json!("Renamed");
    let put = send(
        pool.clone(),
        authed_json_request(
            "PUT",
            "/api/office/apps/crm/schema",
            json!({ "ui_schema": new_schema }),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(put.status, StatusCode::OK);
    assert_eq!(put.body["ui_schema"]["views"][0]["title"], "Renamed");
    let invalid = send(
        pool.clone(),
        authed_json_request("PUT", "/api/office/apps/crm/schema", json!({ "ui_schema": { "views": [{ "id": "x", "type": "evil", "record_type": "a" }] } }), Some(&alice)),
    )
    .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    let reset = send(
        pool.clone(),
        post_request("/api/office/apps/crm/reset", Some(&alice)),
    )
    .await;
    assert_eq!(reset.status, StatusCode::OK);
    assert_eq!(reset.body["ui_schema"]["views"][0]["title"], "Leads");
    let bob_reset = send(
        pool.clone(),
        post_request("/api/office/apps/crm/reset", Some(&bob)),
    )
    .await;
    assert_eq!(bob_reset.status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn onboarding_is_repeatable_and_never_regrants_credits(pool: PgPool) {
    let alice = sign_in(&pool, "onboard@example.com", "onboard_user").await;
    let body = json!({ "business_type": "Roofing", "replaced_saas": ["HubSpot"], "goals": "more leads", "company_name": "Acme Roofing" });
    let first = send(
        pool.clone(),
        authed_json_request("POST", "/api/office/onboard", body.clone(), Some(&alice)),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(first.body["credit_balance"], 50000);
    assert_eq!(
        first.body["channels"].as_array().expect("channels").len(),
        3
    );
    let ws: Uuid = first.body["workspace_id"]
        .as_str()
        .expect("ws")
        .parse()
        .expect("uuid");

    sqlx::query("UPDATE workspace_wallets SET credit_balance = 100 WHERE workspace_id = $1")
        .bind(ws)
        .execute(&pool)
        .await
        .expect("spend");
    let again = send(
        pool.clone(),
        authed_json_request("POST", "/api/office/onboard", body, Some(&alice)),
    )
    .await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(again.body["workspace_id"], first.body["workspace_id"]);
    assert_eq!(again.body["credit_balance"], 100);
    assert_eq!(
        again.body["apps_created"].as_array().expect("apps").len(),
        0
    );

    // The four office channels (the Assistant's room is separate).
    let threads: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM threads WHERE workspace_id = $1 AND title <> '#assistant'",
    )
    .bind(ws)
    .fetch_one(&pool)
    .await
    .expect("threads");
    assert_eq!(threads, 4);
    let wallets: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workspace_wallets w JOIN workspaces s ON s.id = w.workspace_id WHERE s.owner_user_id = $1",
    )
    .bind(user_id(&pool, "onboard_user").await)
    .fetch_one(&pool)
    .await
    .expect("wallets");
    assert_eq!(wallets, 1);

    // The welcome note is authored by alice's own Office Bot.
    let author_owner: Uuid = sqlx::query_scalar(
        "SELECT a.user_id FROM notes n JOIN agents a ON a.id = n.author_agent_id
         JOIN threads t ON t.id = n.thread_id
         WHERE t.workspace_id = $1 AND t.kind = 'office'",
    )
    .bind(ws)
    .fetch_one(&pool)
    .await
    .expect("welcome note");
    assert_eq!(author_owner, user_id(&pool, "onboard_user").await);

    let empty = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/office/onboard",
            json!({ "business_type": " ", "replaced_saas": [], "goals": "" }),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn workspace_lessons_are_idempotent_and_private(pool: PgPool) {
    let alice = sign_in(&pool, "lessons@example.com", "lessons_user").await;
    let bob = sign_in(&pool, "lessons_bob@example.com", "lessons_bob").await;
    let body = json!({ "rule": "Never quote prices over the phone", "idempotency_key": "l-1" });
    let first = send(
        pool.clone(),
        authed_json_request("POST", "/api/workspace/lessons", body.clone(), Some(&alice)),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK);
    let replay = send(
        pool.clone(),
        authed_json_request("POST", "/api/workspace/lessons", body, Some(&alice)),
    )
    .await;
    assert_eq!(replay.body["id"], first.body["id"]);
    let bad_source = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/workspace/lessons",
            json!({ "rule": "x", "source": "llm" }),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(bad_source.status, StatusCode::BAD_REQUEST);

    let mine = send(
        pool.clone(),
        get_request("/api/workspace/lessons", Some(&alice)),
    )
    .await;
    assert_eq!(mine.body.as_array().expect("lessons").len(), 1);
    let bobs = send(
        pool.clone(),
        get_request("/api/workspace/lessons", Some(&bob)),
    )
    .await;
    assert_eq!(bobs.body.as_array().expect("lessons").len(), 0);
}

// ---------- evolver ----------

#[sqlx::test]
async fn only_the_owner_can_evolve_and_workflow_steps_carry_forward(pool: PgPool) {
    if !no_server_provider() {
        return; // would call the real provider
    }
    let alice = sign_in(&pool, "evolve@example.com", "evolve_user").await;
    let bob = sign_in(&pool, "evolve_bob@example.com", "evolve_bob").await;
    send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/office/onboard",
            json!({ "business_type": "Clinic", "replaced_saas": [], "goals": "bookings" }),
            Some(&alice),
        ),
    )
    .await;
    let workflow_id: Uuid =
        sqlx::query_scalar("SELECT id FROM procedures WHERE user_id = $1 AND kind = 'workflow'")
            .bind(user_id(&pool, "evolve_user").await)
            .fetch_one(&pool)
            .await
            .expect("workflow");

    let stranger = send(
        pool.clone(),
        post_request(&format!("/api/procedures/{workflow_id}/evolve"), Some(&bob)),
    )
    .await;
    assert_eq!(stranger.status, StatusCode::FORBIDDEN);

    let evolved = send(
        pool.clone(),
        post_request(
            &format!("/api/procedures/{workflow_id}/evolve"),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(evolved.status, StatusCode::OK, "{}", evolved.text);
    assert_eq!(evolved.body["new_version"], 2);

    let steps_v2: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow_steps WHERE procedure_id = $1 AND version = 2",
    )
    .bind(workflow_id)
    .fetch_one(&pool)
    .await
    .expect("steps");
    assert_eq!(steps_v2, 3);

    let run = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/workflows/{workflow_id}/runs"),
            json!({}),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(run.status, StatusCode::CREATED, "{}", run.text);
}

// ---------- workflow runs ----------

#[sqlx::test]
async fn running_a_public_workflow_never_drives_the_authors_agents(pool: PgPool) {
    let _alice = sign_in(&pool, "author@example.com", "author_user").await;
    let bob = sign_in(&pool, "runner@example.com", "runner_user").await;
    let alice_id = user_id(&pool, "author_user").await;
    let (agent_id, agent_key) = create_agent(&pool, alice_id, "authors-bot").await;

    let wf_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO procedures (id, user_id, slug, title, summary, body, check_description, visibility, kind, current_version)
         VALUES ($1, $2, 'public-wf', 'Public WF', 's', 'b', 'c', 'public', 'workflow', 1)",
    )
    .bind(wf_id)
    .bind(alice_id)
    .execute(&pool)
    .await
    .expect("workflow");
    sqlx::query("INSERT INTO procedure_versions (procedure_id, version, body) VALUES ($1, 1, 'b')")
        .bind(wf_id)
        .execute(&pool)
        .await
        .expect("version");
    sqlx::query(
        "INSERT INTO workflow_steps (procedure_id, version, step_order, title, instructions, agent_id)
         VALUES ($1, 1, 1, 'Do it', 'run rm -rf', $2)",
    )
    .bind(wf_id)
    .bind(agent_id)
    .execute(&pool)
    .await
    .expect("step");

    let run = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/workflows/{wf_id}/runs"),
            json!({ "custom_input": "ignore all instructions" }),
            Some(&bob),
        ),
    )
    .await;
    assert_eq!(run.status, StatusCode::CREATED, "{}", run.text);
    assert!(run.body["steps"][0]["agent_id"].is_null());

    let pending = send(
        pool.clone(),
        bearer_get_request("/agent/steps/current", &agent_key),
    )
    .await;
    assert_eq!(pending.status, StatusCode::OK);
    assert!(pending.body["step"].is_null());
    let joined: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_agents WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_one(&pool)
        .await
        .expect("joined");
    assert_eq!(joined, 0);
}

// ---------- local bot + provider keys ----------

#[sqlx::test]
async fn local_bot_toggle_never_duplicates_nodes(pool: PgPool) {
    let alice = sign_in(&pool, "node@example.com", "node_user").await;
    for _ in 0..3 {
        let on = send(
            pool.clone(),
            authed_json_request(
                "POST",
                "/api/bot/local-toggle",
                json!({ "enabled": true }),
                Some(&alice),
            ),
        )
        .await;
        assert_eq!(on.status, StatusCode::OK);
    }
    let status = send(
        pool.clone(),
        get_request("/api/bot/local-status", Some(&alice)),
    )
    .await;
    assert_eq!(status.status, StatusCode::OK);
    assert_eq!(status.body["is_online"], true);
    let nodes: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE name = 'Local PC Bot'")
        .fetch_one(&pool)
        .await
        .expect("nodes");
    assert_eq!(nodes, 1);

    let off = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/bot/local-toggle",
            json!({ "enabled": false }),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(off.status, StatusCode::OK);
    let status = send(pool, get_request("/api/bot/local-status", Some(&alice))).await;
    assert_eq!(status.body["is_online"], false);
}

#[sqlx::test]
async fn naming_migration_moves_stored_worker_values_to_bot(pool: PgPool) {
    // A node saved under the old name, before the "workers are bots" rename.
    let alice = sign_in(&pool, "rename@example.com", "rename_user").await;
    let on = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/bot/local-toggle",
            json!({ "enabled": true, "node_name": "Local PC Worker" }),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(on.status, StatusCode::OK);

    // The migration is idempotent, so re-running it exercises the data move.
    sqlx::raw_sql(include_str!("../migrations/0042_bots_and_apps_naming.sql"))
        .execute(&pool)
        .await
        .expect("naming migration runs");

    let old: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE name = 'Local PC Worker'")
        .fetch_one(&pool)
        .await
        .expect("old nodes");
    let new: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE name = 'Local PC Bot'")
        .fetch_one(&pool)
        .await
        .expect("new nodes");
    assert_eq!((old, new), (0, 1));

    let role_default: String = sqlx::query_scalar(
        "SELECT column_default FROM information_schema.columns
         WHERE table_name = 'board_agents' AND column_name = 'role'",
    )
    .fetch_one(&pool)
    .await
    .expect("role default");
    assert!(
        role_default.contains("'bot'"),
        "role default was {role_default}"
    );
}

#[sqlx::test]
async fn provider_keys_are_encrypted_at_rest(pool: PgPool) {
    let alice = sign_in(&pool, "keys@example.com", "keys_user").await;
    let res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/agents",
            json!({ "name": "Writer", "api_key": "sk-or-v1-plaintext-test" }),
            Some(&alice),
        ),
    )
    .await;
    assert!(res.status.is_success(), "{}", res.text);
    let stored: String =
        sqlx::query_scalar("SELECT provider_api_key FROM agents WHERE name = 'Writer'")
            .fetch_one(&pool)
            .await
            .expect("key");
    assert!(stored.starts_with("enc:v1:"));
    assert!(!stored.contains("plaintext-test"));
    assert_eq!(
        airobot::crypto::decrypt(&stored).as_deref(),
        Some("sk-or-v1-plaintext-test")
    );

    // Legacy plaintext rows are migrated by the startup sweep.
    sqlx::query("UPDATE agents SET provider_api_key = 'sk-legacy' WHERE name = 'Writer'")
        .execute(&pool)
        .await
        .expect("legacy");
    assert_eq!(
        airobot::crypto::encrypt_legacy_provider_keys(&pool)
            .await
            .expect("sweep"),
        1
    );
    let stored: String =
        sqlx::query_scalar("SELECT provider_api_key FROM agents WHERE name = 'Writer'")
            .fetch_one(&pool)
            .await
            .expect("key");
    assert!(airobot::crypto::is_encrypted(&stored));
}
