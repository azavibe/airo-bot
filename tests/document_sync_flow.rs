// Keeping office documents current (issue #34): website pages are read
// again (now, and weekly in the background), a failed read is told to the
// owner once, and a document marked outdated is no longer seen or cited by
// bots. A small local server plays the business's website.
mod common;

use std::sync::{Arc, Mutex};

use airobot::auth::secret;
use airobot::office::documents;
use axum::http::StatusCode;
use axum::routing::get;
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

/// The business's website: its text and status can change between reads.
type Site = Arc<Mutex<(u16, String)>>;

async fn website(html: &str) -> (String, Site) {
    let site: Site = Arc::new(Mutex::new((200, html.to_string())));
    let shared = site.clone();
    let app = axum::Router::new().route(
        "/prices",
        get(move || {
            let shared = shared.clone();
            async move {
                let (code, html) = shared.lock().expect("site").clone();
                (
                    StatusCode::from_u16(code).expect("status"),
                    [("content-type", "text/html")],
                    html,
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (format!("http://{addr}/prices"), site)
}

async fn text_of(pool: &PgPool, cookie: &str, id: &str) -> String {
    let (status, body) = call(
        pool,
        "GET",
        &format!("/api/documents/{id}"),
        json!({}),
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    body["body"].as_str().unwrap_or_default().to_string()
}

async fn office_chat_notes(pool: &PgPool, office: Uuid, like: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM notes n JOIN threads t ON t.id = n.thread_id
         WHERE t.workspace_id = $1 AND t.kind = 'office' AND n.body LIKE $2",
    )
    .bind(office)
    .bind(format!("%{like}%"))
    .fetch_one(pool)
    .await
    .expect("notes")
}

#[sqlx::test]
async fn a_website_page_is_read_again_and_failures_are_told_once(pool: PgPool) {
    let cookie = sign_in(&pool, "sync@example.com", "syncer").await;
    let office = new_office(&pool, &cookie, "Bright Smiles").await;
    let (url, site) = website(
        "<html><head><title>Prices</title></head><body><p>Cleaning costs $80.</p></body></html>",
    )
    .await;
    let (status, made) = call(
        &pool,
        "POST",
        &format!("/api/offices/{office}/documents"),
        json!({ "url": url }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made:?}");
    let id = made["document"]["id"].as_str().expect("id").to_string();
    let sync = format!("/api/documents/{id}/sync");

    // The site changed: read again now.
    site.lock().expect("site").1 =
        "<html><head><title>Prices</title></head><body><p>Cleaning costs $95.</p></body></html>"
            .into();
    let (status, res) = call(&pool, "POST", &sync, json!({}), &cookie).await;
    assert_eq!(status, StatusCode::OK, "{res:?}");
    assert_eq!(res["changed"], true);
    assert!(res["document"]["synced_at"].is_string());
    let text = text_of(&pool, &cookie, &id).await;
    assert!(text.contains("$95") && !text.contains("$80"), "{text}");
    // Search sees the new text only.
    let (_, found) = call(
        &pool,
        "GET",
        &format!("/api/offices/{office}/documents/search?q=cleaning"),
        json!({}),
        &cookie,
    )
    .await;
    // One passage: the old text's passages were replaced, not kept.
    assert_eq!(
        found["results"].as_array().map(Vec::len),
        Some(1),
        "{found}"
    );

    // Nothing new: unchanged.
    let (_, res) = call(&pool, "POST", &sync, json!({}), &cookie).await;
    assert_eq!(res["changed"], false);

    // The site is down: the last copy stays, the owner is told once.
    site.lock().expect("site").0 = 500;
    let (status, err) = call(&pool, "POST", &sync, json!({}), &cookie).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["error"].as_str().unwrap_or_default().contains("error"),
        "{err:?}"
    );
    let (_, listed) = call(
        &pool,
        "GET",
        &format!("/api/offices/{office}/documents"),
        json!({}),
        &cookie,
    )
    .await;
    assert!(
        listed
            .to_string()
            .contains("That page answered with an error (500)."),
        "{listed}"
    );
    assert!(text_of(&pool, &cookie, &id).await.contains("$95"));
    assert_eq!(
        office_chat_notes(&pool, office, "couldn't be read again").await,
        1
    );
    call(&pool, "POST", &sync, json!({}), &cookie).await;
    assert_eq!(
        office_chat_notes(&pool, office, "couldn't be read again").await,
        1,
        "told once per run of failures"
    );
    // A failed page is due again after a day, not a week.
    let state = common::state(pool.clone());
    assert_eq!(documents::sync_due(&state).await.expect("tick"), 0);
    sqlx::query(
        "UPDATE documents SET synced_at = synced_at - interval '25 hours' WHERE id = $1::uuid",
    )
    .bind(&id)
    .execute(&pool)
    .await
    .expect("clock");
    site.lock().expect("site").0 = 200;
    assert_eq!(documents::sync_due(&state).await.expect("tick"), 1);
    let error: Option<String> =
        sqlx::query_scalar("SELECT sync_error FROM documents WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .expect("doc");
    assert_eq!(error, None, "fixed by the background read");

    // The weekly read: due a week after the last one, not before.
    assert_eq!(documents::sync_due(&state).await.expect("tick"), 0);
    sqlx::query("UPDATE documents SET synced_at = now() - interval '8 days' WHERE id = $1::uuid")
        .bind(&id)
        .execute(&pool)
        .await
        .expect("clock");
    site.lock().expect("site").1 =
        "<html><body><p>Cleaning costs $99 from December.</p></body></html>".into();
    assert_eq!(documents::sync_due(&state).await.expect("tick"), 1);
    assert!(text_of(&pool, &cookie, &id).await.contains("$99"));

    // Only pages can be read again, and only by their owner.
    let (_, text_doc) = call(
        &pool,
        "POST",
        &format!("/api/offices/{office}/documents"),
        json!({ "text": "Opening hours: 9 to 5." }),
        &cookie,
    )
    .await;
    let (status, _) = call(
        &pool,
        "POST",
        &format!(
            "/api/documents/{}/sync",
            text_doc["document"]["id"].as_str().expect("id")
        ),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let other = sign_in(&pool, "nosy@example.com", "nosy").await;
    let (status, _) = call(&pool, "POST", &sync, json!({}), &other).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn an_outdated_document_is_not_seen_or_cited(pool: PgPool) {
    let cookie = sign_in(&pool, "outdated@example.com", "outdated").await;
    let office = new_office(&pool, &cookie, "Cut & Co").await;
    let (_, made) = call(
        &pool,
        "POST",
        &format!("/api/offices/{office}/documents"),
        json!({ "text": "A haircut costs $30.", "title": "Old price list" }),
        &cookie,
    )
    .await;
    let id: Uuid = made["document"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let chat: Uuid =
        sqlx::query_scalar("SELECT id FROM threads WHERE workspace_id = $1 AND kind = 'office'")
            .bind(office)
            .fetch_one(&pool)
            .await
            .expect("office chat");
    let tag = documents::tag(id);
    let bot = Uuid::new_v4();

    // Current: bots see it, and a citation gets a "is outdated" button.
    let told = documents::context_block(&pool, office, bot, "how much is a haircut").await;
    assert!(told.contains("Old price list"), "{told}");
    let (text, cited) = documents::cite_sources(&pool, chat, &format!("It's $30 {tag}.")).await;
    assert_eq!(text, "It's $30 (from “Old price list”).");
    assert_eq!(cited, vec![(id, "Old price list".to_string())]);
    let buttons = documents::outdated_buttons(&cited);
    assert_eq!(buttons[0]["kind"], "doc_outdated");
    assert_eq!(buttons[0]["target"], id.to_string());
    assert_eq!(buttons[0]["label"], "“Old price list” is outdated");

    // Marked outdated: gone from what bots see, and no longer cited.
    let uri = format!("/api/documents/{id}/outdated");
    let (status, saved) = call(
        &pool,
        "POST",
        &uri,
        json!({ "outdated": true, "idempotency_key": "out-1" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved:?}");
    assert!(saved["outdated_at"].is_string());
    let told = documents::context_block(&pool, office, bot, "how much is a haircut").await;
    assert!(!told.contains("Old price list"), "{told}");
    let (_, cited) = documents::cite_sources(&pool, chat, &format!("It's $30 {tag}.")).await;
    assert!(cited.is_empty());
    // The owner still sees it, marked.
    let (_, listed) = call(
        &pool,
        "GET",
        &format!("/api/offices/{office}/documents"),
        json!({}),
        &cookie,
    )
    .await;
    assert!(listed.to_string().contains("outdated_at"), "{listed}");

    // Nobody else marks it.
    let other = sign_in(&pool, "stranger@example.com", "stranger").await;
    let (status, _) = call(&pool, "POST", &uri, json!({ "outdated": false }), &other).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Current again.
    let (status, saved) = call(&pool, "POST", &uri, json!({ "outdated": false }), &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert!(saved["outdated_at"].is_null());
    let told = documents::context_block(&pool, office, bot, "how much is a haircut").await;
    assert!(told.contains("Old price list"), "{told}");
}
