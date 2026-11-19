// Answering a person's own chats with their desktop bot (the AI tool they
// have signed in on their computer): routing, isolation, delivery as the
// chosen agent, failures, and falling back when the computer is offline.
mod common;

use airobot::auth::secret;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::{
    authed_json_request, bearer_get_request, bearer_json_request, form_request, get_request,
    json_request, send, session_cookie,
};
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

/// Pairs a desktop bot through the real device flow and returns its key.
async fn pair_desktop(pool: &PgPool, cookie: &str, name: &str) -> String {
    let issued = send(
        pool.clone(),
        json_request("POST", "/device/code", json!({})),
    )
    .await;
    let code = issued.body["code"].as_str().expect("code").to_string();
    let device_code = issued.body["device_code"]
        .as_str()
        .expect("device code")
        .to_string();
    let approve = send(
        pool.clone(),
        form_request(
            "POST",
            "/device/approve",
            &[("code", code.as_str()), ("agent_name", name)],
            Some(cookie),
        ),
    )
    .await;
    assert!(
        approve.status.is_success() || approve.status.is_redirection(),
        "{}",
        approve.status
    );
    let token = send(
        pool.clone(),
        json_request(
            "POST",
            "/device/token",
            json!({ "device_code": device_code }),
        ),
    )
    .await;
    assert_eq!(token.body["status"], "approved", "{}", token.text);
    token.body["agent_key"].as_str().expect("key").to_string()
}

/// A thread of the person's with their character agent "Cactus" in it.
async fn thread_with_cactus(pool: &PgPool, cookie: &str) -> (Uuid, Uuid) {
    let agent = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/agents",
            json!({ "name": "Cactus", "handle": "cactus", "instructions": "You are Cactus, a calm plant." }),
            Some(cookie),
        ),
    )
    .await;
    let agent_id: Uuid = agent.body["agent"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("u");
    let thread = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/threads",
            json!({ "title": "Garden" }),
            Some(cookie),
        ),
    )
    .await;
    let thread_id: Uuid = thread.body["id"].as_str().expect("id").parse().expect("u");
    sqlx::query("INSERT INTO thread_agents (thread_id, agent_id) VALUES ($1, $2)")
        .bind(thread_id)
        .bind(agent_id)
        .execute(pool)
        .await
        .expect("link");
    (thread_id, agent_id)
}

async fn say(pool: &PgPool, cookie: &str, thread_id: Uuid, body: &str) {
    let res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({ "body": body, "kind": "command" }),
            Some(cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED);
}

async fn chat_tasks(pool: &PgPool) -> Vec<(Uuid, Option<Uuid>, serde_json::Value)> {
    sqlx::query_as(
        "SELECT id, assigned_agent_id, payload FROM tasks
         WHERE payload->>'kind' = 'chat_reply' ORDER BY created_at",
    )
    .fetch_all(pool)
    .await
    .expect("tasks")
}

async fn wait_for<T, F, Fut>(mut check: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    for _ in 0..60 {
        if let Some(v) = check().await {
            return Some(v);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    None
}

#[sqlx::test]
async fn own_chats_go_to_the_desktop_and_come_back_as_the_agent(pool: PgPool) {
    let cookie = sign_in(&pool, "gardener@example.com", "gardener").await;
    let desktop = pair_desktop(&pool, &cookie, "Desktop Bot").await;
    let other_device = pair_desktop(&pool, &cookie, "Old Laptop").await;
    let (thread_id, cactus) = thread_with_cactus(&pool, &cookie).await;

    // The desktop turns on "answer my chats here" with its own key.
    let on = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            "/agent/chat-route",
            json!({ "enabled": true }),
            &desktop,
        ),
    )
    .await;
    assert_eq!(on.status, StatusCode::OK, "{}", on.text);
    // A person (not a bot) cannot set the route.
    let as_user = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/agent/chat-route",
            json!({ "enabled": true }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(as_user.status, StatusCode::FORBIDDEN);

    say(&pool, &cookie, thread_id, "How is the soil today?").await;
    let tasks = wait_for(|| async {
        let t = chat_tasks(&pool).await;
        (!t.is_empty()).then_some(t)
    })
    .await
    .expect("a chat job for the desktop");
    assert_eq!(tasks.len(), 1);
    let (task_id, assigned, payload) = &tasks[0];
    let desktop_id: Uuid = sqlx::query_scalar("SELECT id FROM agents WHERE name = 'Desktop Bot'")
        .fetch_one(&pool)
        .await
        .expect("desktop id");
    assert_eq!(*assigned, Some(desktop_id));
    assert_eq!(payload["reply_as_agent_id"], cactus.to_string());
    assert!(payload["system"]
        .as_str()
        .unwrap_or_default()
        .contains("Cactus"));
    assert!(payload["history"]
        .as_str()
        .unwrap_or_default()
        .contains("How is the soil today?"));

    // Nothing was answered or charged on the server meanwhile.
    let voice: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notes WHERE thread_id = $1 AND author_agent_id IS NOT NULL",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("notes");
    assert_eq!(voice, 0);
    let ledger: i64 = sqlx::query_scalar("SELECT count(*) FROM credit_ledger")
        .fetch_one(&pool)
        .await
        .expect("ledger");
    assert_eq!(ledger, 0);

    // Another device of the same person never sees or claims it.
    let polled = send(
        pool.clone(),
        bearer_get_request("/agent/tasks/poll", &other_device),
    )
    .await;
    assert!(!polled.text.contains(&task_id.to_string()));
    let stolen = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            &format!("/tasks/{task_id}/claim"),
            json!({}),
            &other_device,
        ),
    )
    .await;
    assert_eq!(stolen.status, StatusCode::CONFLICT);

    // The desktop sees it, claims it and answers.
    let polled = send(
        pool.clone(),
        bearer_get_request("/agent/tasks/poll", &desktop),
    )
    .await;
    assert!(polled.text.contains(&task_id.to_string()));
    let claim = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            &format!("/tasks/{task_id}/claim"),
            json!({}),
            &desktop,
        ),
    )
    .await;
    assert_eq!(claim.status, StatusCode::OK, "{}", claim.text);
    let done = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            &format!("/tasks/{task_id}/result"),
            json!({ "status": "completed", "output": { "result": "Moist and happy, thank you." } }),
            &desktop,
        ),
    )
    .await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.text);

    // The answer appears in the thread as Cactus, not as the desktop bot.
    let (author, body): (Uuid, String) = sqlx::query_as(
        "SELECT author_agent_id, body FROM notes WHERE thread_id = $1 AND author_agent_id IS NOT NULL",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("reply");
    assert_eq!(author, cactus);
    assert_eq!(body, "Moist and happy, thank you.");
}

#[sqlx::test]
async fn a_failed_desktop_answer_says_why(pool: PgPool) {
    let cookie = sign_in(&pool, "fail@example.com", "failer").await;
    let desktop = pair_desktop(&pool, &cookie, "Desktop Bot").await;
    let (thread_id, cactus) = thread_with_cactus(&pool, &cookie).await;
    send(
        pool.clone(),
        bearer_json_request(
            "POST",
            "/agent/chat-route",
            json!({ "enabled": true }),
            &desktop,
        ),
    )
    .await;
    say(&pool, &cookie, thread_id, "Hello?").await;
    let task_id = wait_for(|| async { chat_tasks(&pool).await.first().map(|t| t.0) })
        .await
        .expect("job");
    send(
        pool.clone(),
        bearer_json_request(
            "POST",
            &format!("/tasks/{task_id}/claim"),
            json!({}),
            &desktop,
        ),
    )
    .await;
    send(
        pool.clone(),
        bearer_json_request(
            "POST",
            &format!("/tasks/{task_id}/result"),
            json!({ "status": "failed", "output": { "error": "Claude Code is not signed in" } }),
            &desktop,
        ),
    )
    .await;
    let (author, body): (Uuid, String) = sqlx::query_as(
        "SELECT author_agent_id, body FROM notes WHERE thread_id = $1 AND author_agent_id IS NOT NULL",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("note");
    assert_eq!(author, cactus);
    assert!(body.contains("Claude Code is not signed in"), "{body}");
    assert!(body.contains("Answer my chats here"));
}

#[sqlx::test]
async fn offline_or_switched_off_desktops_leave_chat_as_before(pool: PgPool) {
    let cookie = sign_in(&pool, "offline@example.com", "offliner").await;
    let desktop = pair_desktop(&pool, &cookie, "Desktop Bot").await;
    let (thread_id, _) = thread_with_cactus(&pool, &cookie).await;
    send(
        pool.clone(),
        bearer_json_request(
            "POST",
            "/agent/chat-route",
            json!({ "enabled": true }),
            &desktop,
        ),
    )
    .await;

    // The computer has not been seen for a while: chat is answered here.
    sqlx::query(
        "UPDATE agents SET last_seen_at = now() - interval '10 minutes' WHERE name = 'Desktop Bot'",
    )
    .execute(&pool)
    .await
    .expect("age");
    say(&pool, &cookie, thread_id, "Anyone there?").await;
    let voiced = wait_for(|| async {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM notes WHERE thread_id = $1 AND body LIKE '%needs an AI voice%'",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .ok()?;
        (n > 0).then_some(n)
    })
    .await;
    assert_eq!(voiced, Some(1), "falls back to the normal path");
    assert!(chat_tasks(&pool).await.is_empty());

    // Switched off by the desktop: no jobs even while it is online.
    let off = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            "/agent/chat-route",
            json!({ "enabled": false }),
            &desktop,
        ),
    )
    .await;
    assert_eq!(off.status, StatusCode::OK);
    let route: Option<Uuid> =
        sqlx::query_scalar("SELECT chat_desktop_agent_id FROM users WHERE email = $1")
            .bind("offline@example.com")
            .fetch_one(&pool)
            .await
            .expect("route");
    assert!(route.is_none());
    say(&pool, &cookie, thread_id, "Still nobody?").await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(chat_tasks(&pool).await.is_empty());
}
