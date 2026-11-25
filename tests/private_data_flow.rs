// One person's data in a shared chat (issue #29): once an office has a
// team, its bots don't read the owner's own Gmail and Calendar in chats the
// team can read; they ask, and only the owner can allow it for an answer.
mod common;

use airobot::auth::secret;
use airobot::office::access;
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
async fn the_owners_inbox_stays_out_of_chats_the_team_reads(pool: PgPool) {
    let owner = sign_in(&pool, "lee@example.com", "lee").await;
    let office = new_office(&pool, &owner, "Lee's Bakery").await;
    let owner_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = 'lee@example.com'")
        .fetch_one(&pool)
        .await
        .expect("owner");
    let chat: Uuid =
        sqlx::query_scalar("SELECT id FROM threads WHERE workspace_id = $1 AND kind = 'office'")
            .bind(office)
            .fetch_one(&pool)
            .await
            .expect("office chat");
    let bot: Uuid = sqlx::query_scalar(
        "SELECT ta.agent_id FROM thread_agents ta WHERE ta.thread_id = $1 LIMIT 1",
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .expect("a bot in the office chat");
    // The owner's Gmail and Calendar, and Stripe, connected; the bot may use all.
    let google: Uuid = sqlx::query_scalar(
        "INSERT INTO connections (workspace_id, created_by_user_id, provider, label, scopes)
         VALUES ($1, $2, 'google', 'lee@gmail.example',
                 ARRAY['https://www.googleapis.com/auth/gmail.readonly',
                       'https://www.googleapis.com/auth/calendar.events'])
         RETURNING id",
    )
    .bind(office)
    .bind(owner_id)
    .fetch_one(&pool)
    .await
    .expect("google");
    sqlx::query("INSERT INTO bot_access (workspace_id, agent_id, connections) VALUES ($1, $2, $3)")
        .bind(office)
        .bind(bot)
        .bind(vec![
            format!("{google}:gmail"),
            format!("{google}:calendar"),
        ])
        .execute(&pool)
        .await
        .expect("access");
    let state = common::state(pool.clone());

    // Alone in the office: nothing changes, the inbox is read.
    assert!(!access::shared_with_team(&pool, chat).await);
    let seen = access::tools_block(&state, office, bot, true).await;
    assert!(
        seen.contains("Gmail (lee@gmail.example): recent email"),
        "{seen}"
    );
    assert!(!seen.contains("Private to the owner"), "{seen}");

    // With a teammate, the office's chats are shared: the bot is told the
    // inbox and calendar are private, and how to ask.
    let (status, invited) = call(
        &pool,
        "POST",
        &format!("/api/offices/{office}/invites"),
        json!({ "email": "sam@example.com" }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{invited}");
    let token = invited["link"]
        .as_str()
        .and_then(|l| l.split("invite=").nth(1))
        .expect("token")
        .to_string();
    let sam = sign_in(&pool, "sam@example.com", "sam").await;
    let (status, _) = call(
        &pool,
        "POST",
        "/api/invites/accept",
        json!({ "token": token }),
        &sam,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(access::shared_with_team(&pool, chat).await);
    let seen = access::tools_block(&state, office, bot, false).await;
    assert!(seen.contains("## Private to the owner"), "{seen}");
    assert!(
        seen.contains("Gmail (lee@gmail.example), Google Calendar (lee@gmail.example)"),
        "{seen}"
    );
    assert!(seen.contains(access::NEEDS_PRIVATE), "{seen}");
    assert!(
        !seen.contains("recent email") && !seen.contains("upcoming events"),
        "not read: {seen}"
    );
    // The owner's own Assistant room is still theirs alone.
    let assistant: Uuid =
        sqlx::query_scalar("SELECT id FROM threads WHERE user_id = $1 AND title = '#assistant'")
            .bind(owner_id)
            .fetch_one(&pool)
            .await
            .expect("assistant room");
    assert!(!access::shared_with_team(&pool, assistant).await);

    // Only the owner allows it for an answer.
    let uri = format!("/threads/{chat}/allow-private");
    let (status, err) = call(&pool, "POST", &uri, json!({ "agent_id": bot }), &sam).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["error"].as_str().unwrap_or_default().contains("owner"),
        "{err}"
    );
    let stranger = sign_in(&pool, "nosy@example.com", "nosy").await;
    let (status, _) = call(&pool, "POST", &uri, json!({ "agent_id": bot }), &stranger).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &pool,
        "POST",
        &uri,
        json!({ "agent_id": Uuid::new_v4() }),
        &owner,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a bot that isn't in this chat"
    );
    let (status, ok) = call(
        &pool,
        "POST",
        &uri,
        json!({ "agent_id": bot, "idempotency_key": "allow-1" }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{ok}");
    assert_eq!(ok["status"], "answering");

    // Sam leaves: the chat is the owner's alone again.
    let sam_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = 'sam@example.com'")
        .fetch_one(&pool)
        .await
        .expect("sam");
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/offices/{office}/members/{sam_id}/remove"),
        json!({}),
        &sam,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!access::shared_with_team(&pool, chat).await);
}
