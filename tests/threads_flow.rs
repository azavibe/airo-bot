mod common;

#[sqlx::test]
async fn concurrent_thread_and_note_retries_are_idempotent(pool: PgPool) {
    let cookie = sign_in(&pool, "retry@example.com", "retryer").await;
    let create = || {
        authed_json_request(
            "POST",
            "/threads",
            json!({"title":"Concurrent", "idempotency_key":"thread-key"}),
            Some(&cookie),
        )
    };
    let (a, b) = tokio::join!(send(pool.clone(), create()), send(pool.clone(), create()));
    assert!(a.status.is_success() && b.status.is_success());
    assert_eq!(a.body["id"], b.body["id"]);
    let id = a.body["id"].as_str().expect("id");
    let uri = format!("/threads/{id}/notes");
    let note = || {
        authed_json_request(
            "POST",
            &uri,
            json!({"body":"Once", "kind":"comment", "idempotency_key":"note-key"}),
            Some(&cookie),
        )
    };
    let (a, b) = tokio::join!(send(pool.clone(), note()), send(pool.clone(), note()));
    assert!(a.status.is_success() && b.status.is_success());
    assert_eq!(a.body["id"], b.body["id"]);
    let count: i32 = sqlx::query_scalar("SELECT note_count FROM threads WHERE id = $1")
        .bind(Uuid::parse_str(id).expect("uuid"))
        .fetch_one(&pool)
        .await
        .expect("counter");
    assert_eq!(count, 1);
}

use airobot::auth::secret;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::{
    authed_json_request, bearer_get_request, bearer_json_request, form_request, get_request,
    json_request, post_request, send, session_cookie,
};
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

/// Full device flow: returns a fresh agent key belonging to whoever `cookie`
/// signs in as.
async fn create_agent(pool: &PgPool, cookie: &str, name: &str) -> String {
    let code_res = send(pool.clone(), post_request("/device/code", None)).await;
    let code = code_res.body["code"].as_str().expect("code").to_string();
    let device_code = code_res.body["device_code"]
        .as_str()
        .expect("device code")
        .to_string();

    let approve = send(
        pool.clone(),
        form_request(
            "POST",
            "/device/approve",
            &[("code", &code), ("agent_name", name)],
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
        .expect("agent_key")
        .to_string()
}

async fn create_private_thread(pool: &PgPool, cookie: &str, title: &str) -> String {
    let res = send(
        pool.clone(),
        authed_json_request("POST", "/threads", json!({ "title": title }), Some(cookie)),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED);
    res.body["id"].as_str().expect("thread id").to_string()
}

async fn create_public_thread(pool: &PgPool, cookie: &str, title: &str) -> String {
    let res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/threads",
            json!({ "title": title, "visibility": "public" }),
            Some(cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED);
    res.body["id"].as_str().expect("thread id").to_string()
}

// ---------- POST /threads ----------

#[sqlx::test]
async fn create_thread_requires_auth(pool: PgPool) {
    let res = send(
        pool,
        json_request("POST", "/threads", json!({ "title": "x" })),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn create_thread_rejects_empty_title(pool: PgPool) {
    let cookie = sign_in(&pool, "a@example.com", "usera").await;
    let res = send(
        pool,
        authed_json_request(
            "POST",
            "/threads",
            json!({ "title" : "   " }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn create_thread_defaults_to_private(pool: PgPool) {
    let cookie = sign_in(&pool, "a@example.com", "usera").await;
    let res = send(
        pool,
        authed_json_request(
            "POST",
            "/threads",
            json!({ "title": "Today's outreach" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED);
    assert_eq!(res.body["visibility"], "private");
    assert_eq!(res.body["status"], "open");
    assert_eq!(res.body["note_count"], 0);
}

#[sqlx::test]
async fn create_thread_idempotency_key_returns_the_same_thread(pool: PgPool) {
    let cookie = sign_in(&pool, "a@example.com", "usera").await;
    let first = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/threads",
            json!({ "title": "job", "idempotency_key": "job-42" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(first.status, StatusCode::CREATED);

    let second = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/threads",
            json!({ "title": "job", "idempotency_key": "job-42" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(second.status, StatusCode::OK);
    assert_eq!(first.body["id"], second.body["id"]);

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM threads WHERE title = 'job'")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 1);
}

#[sqlx::test]
async fn agent_can_create_a_thread_owned_by_its_human(pool: PgPool) {
    let cookie = sign_in(&pool, "a@example.com", "usera").await;
    let agent_key = create_agent(&pool, &cookie, "scraper").await;

    let res = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            "/threads",
            json!({ "title": "agent job" }),
            &agent_key,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED);

    // The agent that created it should show up in its own thread list too
    // (auto-joined).
    let list = send(pool, bearer_get_request("/threads", &agent_key)).await;
    let threads = list.body["threads"].as_array().expect("threads array");
    assert_eq!(threads.len(), 1);
}

// ---------- GET /threads ----------

/// The threads a test made: everyone also has the built-in `#assistant`
/// room from their first sign-in.
fn own_rooms(body: &serde_json::Value) -> Vec<serde_json::Value> {
    body["threads"]
        .as_array()
        .expect("threads array")
        .iter()
        .filter(|t| t["title"] != "#assistant" && t["kind"] != "office")
        .cloned()
        .collect()
}

#[sqlx::test]
async fn list_threads_shows_only_the_callers_threads(pool: PgPool) {
    let alice = sign_in(&pool, "alice@example.com", "alice").await;
    let bob = sign_in(&pool, "bob@example.com", "bob").await;
    create_private_thread(&pool, &alice, "Alice's job").await;
    create_private_thread(&pool, &bob, "Bob's job").await;

    let res = send(pool, get_request("/threads", Some(&alice))).await;
    let threads = own_rooms(&res.body);
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0]["title"], "Alice's job");
}

#[sqlx::test]
async fn list_threads_filters_by_status(pool: PgPool) {
    let cookie = sign_in(&pool, "a@example.com", "usera").await;
    create_private_thread(&pool, &cookie, "open one").await;

    let open = send(
        pool.clone(),
        get_request("/threads?status=open", Some(&cookie)),
    )
    .await;
    assert_eq!(own_rooms(&open.body).len(), 1);

    let done = send(pool, get_request("/threads?status=done", Some(&cookie))).await;
    assert_eq!(own_rooms(&done.body).len(), 0);
}

#[sqlx::test]
async fn agent_thread_list_is_only_threads_it_has_joined(pool: PgPool) {
    let cookie = sign_in(&pool, "a@example.com", "usera").await;
    let agent_key = create_agent(&pool, &cookie, "emailer").await;
    // The human creates a thread but the agent never writes to it, so it
    // hasn't joined -- shouldn't show up in the agent's own list.
    create_private_thread(&pool, &cookie, "human-only job").await;

    let res = send(pool, bearer_get_request("/threads", &agent_key)).await;
    assert_eq!(res.body["threads"].as_array().unwrap().len(), 0);
}

// ---------- GET /threads/:id ----------

#[sqlx::test]
async fn get_thread_not_found(pool: PgPool) {
    let cookie = sign_in(&pool, "a@example.com", "usera").await;
    let res = send(
        pool,
        get_request(&format!("/threads/{}", Uuid::new_v4()), Some(&cookie)),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn private_thread_is_hidden_from_strangers(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let stranger = sign_in(&pool, "stranger@example.com", "stranger").await;
    let thread_id = create_private_thread(&pool, &owner, "secret job").await;

    let anon = send(
        pool.clone(),
        get_request(&format!("/threads/{thread_id}"), None),
    )
    .await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);

    let stranger_res = send(
        pool,
        get_request(&format!("/threads/{thread_id}"), Some(&stranger)),
    )
    .await;
    assert_eq!(stranger_res.status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn public_thread_is_visible_to_anyone(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let thread_id = create_public_thread(&pool, &owner, "public job").await;

    let anon = send(pool, get_request(&format!("/threads/{thread_id}"), None)).await;
    assert_eq!(anon.status, StatusCode::OK);
    assert_eq!(anon.body["thread"]["title"], "public job");
}

#[sqlx::test]
async fn unlisted_thread_is_also_visible_to_anyone(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/threads",
            json!({ "title": "unlisted job", "visibility": "unlisted" }),
            Some(&owner),
        ),
    )
    .await;
    let thread_id = res.body["id"].as_str().unwrap().to_string();

    let anon = send(pool, get_request(&format!("/threads/{thread_id}"), None)).await;
    assert_eq!(anon.status, StatusCode::OK);
}

#[sqlx::test]
async fn joined_agent_can_view_its_private_thread(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let agent_key = create_agent(&pool, &owner, "emailer").await;
    let thread_id = create_private_thread(&pool, &owner, "job").await;

    // Agent hasn't posted yet, but access is granted by ownership, not
    // join state (see is_owning_human/owns_thread in handlers::threads).
    let res = send(
        pool,
        bearer_get_request(&format!("/threads/{thread_id}"), &agent_key),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
}

// ---------- POST /threads/:id/notes -- the command rule ----------

#[sqlx::test]
async fn owner_can_post_a_command_on_a_private_thread(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let thread_id = create_private_thread(&pool, &owner, "job").await;

    let res = send(
        pool,
        authed_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({ "body": "hold the follow-ups", "kind": "command" }),
            Some(&owner),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED);
    assert_eq!(res.body["kind"], "command");
}

/// Regression test: an agent belonging to the thread's owner must NOT be
/// able to post a command, even though it shares the owner's user_id.
/// `owns_thread` (used for general private-thread access) and
/// `is_owning_human` (used only for the command rule) must stay distinct.
#[sqlx::test]
async fn agent_cannot_post_a_command_even_on_its_own_owners_thread(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let agent_key = create_agent(&pool, &owner, "emailer").await;
    let thread_id = create_private_thread(&pool, &owner, "job").await;

    let res = send(
        pool,
        bearer_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({ "body": "sneaky command", "kind": "command" }),
            &agent_key,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn agent_can_post_status_on_its_owners_private_thread_and_auto_joins(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let agent_key = create_agent(&pool, &owner, "emailer").await;
    let thread_id = create_private_thread(&pool, &owner, "job").await;

    let res = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({ "body": "sent 10 emails", "kind": "status" }),
            &agent_key,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED);

    let note_thread_id: Uuid = res.body["thread_id"].as_str().unwrap().parse().unwrap();
    let joined: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_agents WHERE thread_id = $1")
        .bind(note_thread_id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(joined, 1);

    let thread: (i32,) = sqlx::query_as("SELECT note_count FROM threads WHERE id = $1")
        .bind(note_thread_id)
        .fetch_one(&pool)
        .await
        .expect("thread");
    assert_eq!(thread.0, 1);
}

#[sqlx::test]
async fn unrelated_agent_cannot_post_to_someone_elses_private_thread(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let other = sign_in(&pool, "other@example.com", "other").await;
    let other_agent_key = create_agent(&pool, &other, "otheragent").await;
    let thread_id = create_private_thread(&pool, &owner, "job").await;

    let res = send(
        pool,
        bearer_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({ "body": "hi", "kind": "status" }),
            &other_agent_key,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn stranger_can_comment_on_a_public_thread(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let stranger = sign_in(&pool, "stranger@example.com", "stranger").await;
    let thread_id = create_public_thread(&pool, &owner, "public job").await;

    let res = send(
        pool,
        authed_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({ "body": "nice work", "kind": "comment" }),
            Some(&stranger),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED);
}

#[sqlx::test]
async fn public_thread_rejects_non_comment_kinds(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let thread_id = create_public_thread(&pool, &owner, "public job").await;

    let res = send(
        pool,
        authed_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({ "body": "status update", "kind": "status" }),
            Some(&owner),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn note_idempotency_key_prevents_a_duplicate(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let thread_id = create_private_thread(&pool, &owner, "job").await;

    let first = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({ "body": "hi", "kind": "command", "idempotency_key": "note-1" }),
            Some(&owner),
        ),
    )
    .await;
    assert_eq!(first.status, StatusCode::CREATED);

    let second = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({ "body": "hi", "kind": "command", "idempotency_key": "note-1" }),
            Some(&owner),
        ),
    )
    .await;
    assert_eq!(second.status, StatusCode::OK);
    assert_eq!(first.body["id"], second.body["id"]);

    let thread_uuid: Uuid = thread_id.parse().unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notes WHERE thread_id = $1")
        .bind(thread_uuid)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 1);

    let thread: (i32,) = sqlx::query_as("SELECT note_count FROM threads WHERE id = $1")
        .bind(thread_uuid)
        .fetch_one(&pool)
        .await
        .expect("thread");
    assert_eq!(
        thread.0, 1,
        "a replayed idempotency key must not double-count"
    );
}

// ---------- htmx fragments ----------

#[sqlx::test]
async fn hx_request_header_returns_only_the_log_fragment(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let thread_id = create_private_thread(&pool, &owner, "job").await;

    let mut req = get_request(&format!("/threads/{thread_id}"), Some(&owner));
    req.headers_mut()
        .insert("accept", "text/html".parse().unwrap());
    req.headers_mut()
        .insert("HX-Request", "true".parse().unwrap());
    let res = send(pool, req).await;
    assert_eq!(res.status, StatusCode::OK);
}

#[sqlx::test]
async fn hx_request_without_explicit_html_accept_returns_log_fragment(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let thread_id = create_private_thread(&pool, &owner, "job").await;

    let mut req = get_request(&format!("/threads/{thread_id}"), Some(&owner));
    req.headers_mut().insert("accept", "*/*".parse().unwrap());
    req.headers_mut()
        .insert("HX-Request", "true".parse().unwrap());
    let res = send(pool, req).await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(res.text.contains("<ol class=\"log\""));
}

#[sqlx::test]
async fn agent_creating_thread_and_posting_note_notifies_owner(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let agent_key = create_agent(&pool, &owner, "bot").await;

    let thread = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            "/threads",
            json!({ "title": "Automated Task" }),
            &agent_key,
        ),
    )
    .await;
    assert_eq!(thread.status, StatusCode::CREATED);
    let thread_id = thread.body["id"].as_str().unwrap();

    let notifs = send(pool.clone(), get_request("/notifs", Some(&owner))).await;
    assert_eq!(notifs.status, StatusCode::OK);
    assert_eq!(notifs.body["notifications"].as_array().unwrap().len(), 1);
    assert_eq!(notifs.body["notifications"][0]["kind"], "agent_thread");

    let note = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({ "body": "Working on it", "kind": "status" }),
            &agent_key,
        ),
    )
    .await;
    assert_eq!(note.status, StatusCode::CREATED);

    let notifs2 = send(pool.clone(), get_request("/notifs", Some(&owner))).await;
    assert_eq!(notifs2.body["notifications"].as_array().unwrap().len(), 2);
    assert_eq!(notifs2.body["notifications"][0]["kind"], "agent_note");

    // Merged into the inbox: the page leads there (303, uncached).
    let mut page_req = get_request("/notifs", Some(&owner));
    page_req
        .headers_mut()
        .insert("accept", "text/html".parse().unwrap());
    let page = send(pool.clone(), page_req).await;
    assert_eq!(page.status, StatusCode::SEE_OTHER);
    assert_eq!(
        page.headers.get("location").and_then(|v| v.to_str().ok()),
        Some("/inbox")
    );
}

#[sqlx::test]
async fn agent_cannot_be_tricked_by_html_or_htmx_attributes_in_text_plain(pool: PgPool) {
    // Content negotiation is the safety boundary: a caller must get bare
    // JSON/data, never markup, unless it explicitly asks for text/html.
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let thread_id = create_private_thread(&pool, &owner, "job").await;

    let res = send(
        pool,
        get_request(&format!("/threads/{thread_id}"), Some(&owner)),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    // Default (no Accept: text/html) is JSON -- the body must parse as a
    // JSON object, not contain any HTML tags.
    assert!(res.body.is_object());
    assert!(res.body.get("thread").is_some());
}

// ---------- POST /threads/:id/archive ----------

#[sqlx::test]
async fn owner_can_archive_their_thread(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let thread_id = create_private_thread(&pool, &owner, "job").await;

    let res = send(
        pool.clone(),
        post_request(&format!("/threads/{thread_id}/archive"), Some(&owner)),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(!res.body["archived_at"].is_null());

    // Idempotent: archiving twice keeps the original timestamp.
    let first_archived_at = res.body["archived_at"].clone();
    let second = send(
        pool,
        post_request(&format!("/threads/{thread_id}/archive"), Some(&owner)),
    )
    .await;
    assert_eq!(second.body["archived_at"], first_archived_at);
}

#[sqlx::test]
async fn agent_cannot_archive_a_thread(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let agent_key = create_agent(&pool, &owner, "emailer").await;
    let thread_id = create_private_thread(&pool, &owner, "job").await;

    let res = send(
        pool,
        bearer_json_request(
            "POST",
            &format!("/threads/{thread_id}/archive"),
            json!({}),
            &agent_key,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn stranger_cannot_archive_someone_elses_thread(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let stranger = sign_in(&pool, "stranger@example.com", "stranger").await;
    let thread_id = create_private_thread(&pool, &owner, "job").await;

    let res = send(
        pool,
        post_request(&format!("/threads/{thread_id}/archive"), Some(&stranger)),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
}

// ---------- GET /inbox ----------

#[sqlx::test]
async fn inbox_requires_auth(pool: PgPool) {
    let res = send(pool, get_request("/inbox", None)).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn inbox_returns_existing_notes_immediately(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let thread_id = create_private_thread(&pool, &owner, "job").await;
    send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({ "body": "hi", "kind": "command" }),
            Some(&owner),
        ),
    )
    .await;

    let res = send(pool, get_request("/inbox", Some(&owner))).await;
    assert_eq!(res.status, StatusCode::OK);
    let notes = res.body["notes"].as_array().expect("notes array");
    assert_eq!(notes.len(), 1);
    assert!(res.body["cursor"].as_i64().unwrap() >= 1);
}

#[sqlx::test]
async fn inbox_only_returns_notes_from_the_callers_own_threads(pool: PgPool) {
    let alice = sign_in(&pool, "alice@example.com", "alice").await;
    let bob = sign_in(&pool, "bob@example.com", "bob").await;
    let bobs_thread = create_private_thread(&pool, &bob, "bob's job").await;
    send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/threads/{bobs_thread}/notes"),
            json!({ "body": "bob's note", "kind": "command" }),
            Some(&bob),
        ),
    )
    .await;

    let res = send(pool, get_request("/inbox", Some(&alice))).await;
    assert_eq!(res.body["notes"].as_array().unwrap().len(), 0);
}

/// The Phase 2 "done when": one terminal writes a note, the other sees it
/// via /inbox without a tight poll loop -- the long poll itself must catch
/// a note that arrives *while it's waiting*, not just ones already there.
#[sqlx::test]
async fn inbox_long_polls_until_a_new_note_arrives(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let thread_id = create_private_thread(&pool, &owner, "job").await;

    let poll_pool = pool.clone();
    let poll_cookie = owner.clone();
    let poll_handle = tokio::spawn(async move {
        send(poll_pool, get_request("/inbox?since=0", Some(&poll_cookie))).await
    });

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let post_res = send(
        pool,
        authed_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({ "body": "arrived mid-poll", "kind": "command" }),
            Some(&owner),
        ),
    )
    .await;
    assert_eq!(post_res.status, StatusCode::CREATED);

    let poll_res = poll_handle.await.expect("poll task");
    assert_eq!(poll_res.status, StatusCode::OK);
    let notes = poll_res.body["notes"].as_array().expect("notes array");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["body"], "arrived mid-poll");
}

#[sqlx::test]
async fn thread_live_events_streams_new_notes(pool: PgPool) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let app_state = common::state(pool.clone());
    let owner = sign_in(&pool, "sse@example.com", "sseuser").await;
    let thread_id = create_private_thread(&pool, &owner, "sse job").await;

    // Connect to SSE stream
    let req = get_request(&format!("/threads/{thread_id}/events"), Some(&owner));
    let sse_res = airobot::app(app_state.clone())
        .oneshot(req)
        .await
        .expect("sse request");
    assert_eq!(sse_res.status(), StatusCode::OK);
    assert_eq!(
        sse_res
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "text/event-stream"
    );

    let mut body = sse_res.into_body();

    // Post a note to the thread
    let post_req = authed_json_request(
        "POST",
        &format!("/threads/{thread_id}/notes"),
        json!({ "body": "instant live update", "kind": "comment" }),
        Some(&owner),
    );
    let post_res = airobot::app(app_state.clone())
        .oneshot(post_req)
        .await
        .expect("post request");
    assert_eq!(post_res.status(), StatusCode::CREATED);

    // Read the next frame from the SSE body
    let frame = tokio::time::timeout(std::time::Duration::from_secs(2), body.frame())
        .await
        .expect("timeout")
        .expect("frame exists")
        .expect("valid frame");
    let data = frame.into_data().expect("data frame");
    let text = String::from_utf8_lossy(&data);
    assert!(text.contains("event: note"));
    assert!(text.contains("instant live update"));
}

#[sqlx::test]
async fn thread_live_events_enforces_visibility(pool: PgPool) {
    use tower::ServiceExt;

    let alice = sign_in(&pool, "sse_alice@example.com", "sse_alice").await;
    let bob = sign_in(&pool, "sse_bob@example.com", "sse_bob").await;
    let thread_id = create_private_thread(&pool, &alice, "alice private").await;

    // Bob cannot listen to Alice's private thread
    let req = get_request(&format!("/threads/{thread_id}/events"), Some(&bob));
    let res = airobot::app(common::state(pool.clone()))
        .oneshot(req)
        .await
        .expect("req");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // Anonymous cannot listen
    let anon_req = get_request(&format!("/threads/{thread_id}/events"), None);
    let anon_res = airobot::app(common::state(pool))
        .oneshot(anon_req)
        .await
        .expect("req");
    assert_eq!(anon_res.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn a_reloaded_thread_names_each_notes_author(pool: PgPool) {
    let cookie = sign_in(&pool, "names@example.com", "namer").await;
    let thread = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/threads",
            json!({ "title": "Names" }),
            Some(&cookie),
        ),
    )
    .await;
    let id = thread.body["id"].as_str().expect("id").to_string();
    send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/threads/{id}/notes"),
            json!({ "body": "hello", "kind": "status" }),
            Some(&cookie),
        ),
    )
    .await;
    let res = send(
        pool.clone(),
        authed_json_request("GET", &format!("/threads/{id}"), json!({}), Some(&cookie)),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    let note = &res.body["notes"][0];
    assert_eq!(note["body"], "hello");
    assert_eq!(note["author_name"], "namer");
    assert_eq!(note["author_kind"], "human");
}
