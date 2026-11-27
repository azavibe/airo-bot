// The lists the app loads on every visit take the same number of database
// queries however many items they show (no query per item).
mod common;

use airobot::auth::secret;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::{form_request, get_request, send, session_cookie};
use sqlx::PgPool;
use std::cell::Cell;
use tracing_subscriber::layer::SubscriberExt;
use uuid::Uuid;

thread_local! {
    /// Queries run on this thread; each test runs on its own runtime thread.
    static QUERIES: Cell<usize> = const { Cell::new(0) };
}

/// Counts sqlx's per-statement log events made while handling a request
/// (inside the router's `request` span). Background work a request started
/// with `tokio::spawn` runs outside it, whenever the runtime gets to it, so
/// it isn't the request's own cost.
struct CountQueries;

impl<S> tracing_subscriber::Layer<S> for CountQueries
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let in_request = ctx
            .event_scope(event)
            .is_some_and(|mut scope| scope.any(|span| span.name() == "request"));
        if event.metadata().target() == "sqlx::query" && in_request {
            QUERIES.with(|n| n.set(n.get() + 1));
        }
    }
}

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

/// Adds `count` tasks for `owner`'s first office, each on its own bot and in
/// its own room with a message, each waiting on an approval.
async fn add_work(pool: &PgPool, owner: Uuid, office: Uuid, count: usize) {
    for i in 0..count {
        let bot = Uuid::new_v4();
        let (_key, raw_secret, prefix) = airobot::auth::generate_agent_key(bot);
        sqlx::query(
            "INSERT INTO agents (id, user_id, name, handle, key_hash, key_prefix)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(bot)
        .bind(owner)
        .bind(format!("Bot {bot}"))
        .bind(format!("bot-{}", &bot.simple().to_string()[..8]))
        .bind(secret::hash_secret(&raw_secret).expect("hash"))
        .bind(prefix)
        .execute(pool)
        .await
        .expect("bot");
        let room: Uuid = sqlx::query_scalar(
            "INSERT INTO threads (user_id, title, slug, workspace_id)
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(owner)
        .bind(format!("Room {i}"))
        .bind(format!("room-{}", Uuid::new_v4().simple()))
        .bind(office)
        .fetch_one(pool)
        .await
        .expect("room");
        sqlx::query(
            "INSERT INTO notes (thread_id, author_user_id, body, kind)
             VALUES ($1, $2, 'Ready to send', 'comment')",
        )
        .bind(room)
        .bind(owner)
        .execute(pool)
        .await
        .expect("note");
        let task: Uuid = sqlx::query_scalar(
            "INSERT INTO tasks (workspace_id, thread_id, title, status, priority,
                                created_by_actor_type, assigned_agent_id)
             VALUES ($1, $2, $3, 'waiting_approval', 'normal', 'user', $4) RETURNING id",
        )
        .bind(office)
        .bind(room)
        .bind(format!("Task {i}"))
        .bind(bot)
        .fetch_one(pool)
        .await
        .expect("task");
        sqlx::query(
            "INSERT INTO approvals (workspace_id, task_id, requested_action, risk_level,
                                    status, requested_by)
             VALUES ($1, $2, 'send it', 'medium', 'pending', 'agent:bot')",
        )
        .bind(office)
        .bind(task)
        .execute(pool)
        .await
        .expect("approval");
    }
}

/// Queries one JSON GET of `uri` runs, and its body.
async fn queries_for(pool: &PgPool, cookie: &str, uri: &str) -> (usize, serde_json::Value) {
    let mut req = get_request(uri, Some(cookie));
    req.headers_mut()
        .insert("accept", "application/json".parse().expect("header value"));
    let before = QUERIES.with(Cell::get);
    let res = send(pool.clone(), req).await;
    assert_eq!(res.status, StatusCode::OK, "{uri}: {:?}", res.body);
    (QUERIES.with(Cell::get) - before, res.body)
}

#[sqlx::test]
async fn approvals_and_tasks_take_no_query_per_item(pool: PgPool) {
    let _ =
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(CountQueries));
    let cookie = sign_in(&pool, "busy@example.com", "busy").await;
    // The first list makes the office.
    queries_for(&pool, &cookie, "/tasks").await;
    let (owner, office): (Uuid, Uuid) = sqlx::query_as(
        "SELECT u.id, w.id FROM users u JOIN workspaces w ON w.owner_user_id = u.id
         WHERE u.handle = 'busy'",
    )
    .fetch_one(&pool)
    .await
    .expect("office");

    add_work(&pool, owner, office, 1).await;
    let (approvals_one, body) = queries_for(&pool, &cookie, "/approvals").await;
    assert_eq!(body["approvals"].as_array().map(Vec::len), Some(1));
    let (tasks_one, body) = queries_for(&pool, &cookie, "/tasks").await;
    assert_eq!(body["tasks"].as_array().map(Vec::len), Some(1));
    assert_ne!(approvals_one, 0, "the counter sees the queries");

    add_work(&pool, owner, office, 19).await;
    let (approvals_many, body) = queries_for(&pool, &cookie, "/approvals").await;
    assert_eq!(body["approvals"].as_array().map(Vec::len), Some(20));
    let (tasks_many, body) = queries_for(&pool, &cookie, "/tasks").await;
    assert_eq!(body["tasks"].as_array().map(Vec::len), Some(20));

    assert_eq!(approvals_many, approvals_one, "approvals");
    assert_eq!(tasks_many, tasks_one, "tasks");
}
