// Talk to an office from Telegram (issue #33), against a stand-in for the
// Telegram Bot API: connecting a bot, linking the owner's chat with the
// one-time code, messages both ways, and approvals with buttons.
mod common;

use std::sync::{Arc, Mutex};

use airobot::auth::secret;
use airobot::state::{AppState, NoteEvent};
use axum::extract::Path as AxPath;
use axum::http::StatusCode;
use axum::routing::post;
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

const TOKEN: &str = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";

#[derive(Default)]
struct Fake {
    calls: Vec<(String, Value)>,
    updates: Vec<Value>,
}
type Shared = Arc<Mutex<Fake>>;

/// A stand-in for api.telegram.org: knows one bot, hands out queued
/// updates once, and records every call.
async fn fake_telegram() -> (String, Shared) {
    let shared: Shared = Arc::default();
    let fake = shared.clone();
    let app = axum::Router::new().route(
        "/api.telegram.org/{bot}/{method}",
        post(
            move |AxPath((bot, method)): AxPath<(String, String)>,
                  axum::Json(body): axum::Json<Value>| {
                let fake = fake.clone();
                async move {
                    let mut f = fake.lock().expect("fake");
                    f.calls.push((method.clone(), body));
                    if bot != format!("bot{TOKEN}") {
                        return axum::Json(json!({ "ok": false, "description": "Unauthorized" }));
                    }
                    let result = match method.as_str() {
                        "getMe" => {
                            json!({ "id": 777, "is_bot": true, "username": "bakery_office_bot" })
                        }
                        "getUpdates" => json!(std::mem::take(&mut f.updates)),
                        _ => json!({ "message_id": 1 }),
                    };
                    axum::Json(json!({ "ok": true, "result": result }))
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (format!("http://{addr}"), shared)
}

fn with_telegram(pool: PgPool, origin: &str) -> AppState {
    let mut s = common::state(pool);
    let mut config = (*s.config).clone();
    config.connectors.test_origin = Some(origin.to_string());
    s.config = Arc::new(config);
    s
}

async fn api(
    state: &AppState,
    method: &str,
    uri: &str,
    body: Value,
    cookie: &str,
) -> (StatusCode, Value) {
    let res = common::send_with_state(
        state.clone(),
        authed_json_request(method, uri, body, Some(cookie)),
    )
    .await;
    (res.status, res.body)
}

fn message(update_id: i64, chat: i64, text: &str) -> Value {
    json!({ "update_id": update_id, "message": {
        "message_id": update_id, "chat": { "id": chat, "type": "private" }, "text": text
    }})
}

/// The texts sent to `chat` since the last look.
fn sent_to(shared: &Shared, chat: i64) -> Vec<String> {
    let mut f = shared.lock().expect("fake");
    let texts = f
        .calls
        .iter()
        .filter(|(m, b)| m == "sendMessage" && b["chat_id"] == chat)
        .map(|(_, b)| b["text"].as_str().unwrap_or_default().to_string())
        .collect();
    f.calls.clear();
    texts
}

#[sqlx::test]
async fn an_office_talks_from_telegram(pool: PgPool) {
    let (origin, fake) = fake_telegram().await;
    let state = with_telegram(pool.clone(), &origin);
    let owner = sign_in(&pool, "baker@example.com", "baker").await;
    let office = new_office(&pool, &owner, "Corner Bakery").await;
    let uri = format!("/api/offices/{office}/telegram");

    let (status, view) = api(&state, "GET", &uri, json!({}), &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["connected"], false);

    // A token that isn't one, and one Telegram refuses.
    let (status, _) = api(&state, "POST", &uri, json!({ "token": "hello" }), &owner).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, err) = api(
        &state,
        "POST",
        &uri,
        json!({ "token": "123456789:AAwrongwrongwrongwrongwrong" }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["error"]
            .as_str()
            .unwrap_or_default()
            .contains("Unauthorized"),
        "{err}"
    );

    // Connected: the token is kept encrypted, and a link is handed out.
    let (status, made) = api(
        &state,
        "POST",
        &uri,
        json!({ "token": TOKEN, "idempotency_key": "tg-1" }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["bot_username"], "bakery_office_bot");
    assert_eq!(made["linked"], false);
    let link = made["link_url"].as_str().expect("link").to_string();
    assert!(
        link.starts_with("https://t.me/bakery_office_bot?start="),
        "{link}"
    );
    let code = link.split("start=").nth(1).expect("code").to_string();
    let stored: String = sqlx::query_scalar("SELECT token_encrypted FROM telegram_bots")
        .fetch_one(&pool)
        .await
        .expect("row");
    assert!(!stored.contains(TOKEN), "never kept in plain text");
    // Repeating the request doesn't make a second bot.
    let (status, again) = api(
        &state,
        "POST",
        &uri,
        json!({ "token": TOKEN, "idempotency_key": "tg-1" }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(again["link_url"].is_null());
    let bot: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM telegram_bots WHERE deleted_at IS NULL")
            .fetch_one(&pool)
            .await
            .expect("one bot");

    // Strangers learn nothing.
    let other = sign_in(&pool, "other@example.com", "other").await;
    let (status, _) = api(&state, "GET", &uri, json!({}), &other).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Linking: a wrong code doesn't, the right one does, once.
    fake.lock().expect("fake").updates = vec![message(1, 555, "/start nope")];
    airobot::telegram::poll_once(&state, bot, 0)
        .await
        .expect("poll");
    assert!(sent_to(&fake, 555)[0].contains("belongs to an Airobot office"));
    fake.lock().expect("fake").updates = vec![message(2, 555, &format!("/start {code}"))];
    airobot::telegram::poll_once(&state, bot, 0)
        .await
        .expect("poll");
    assert!(sent_to(&fake, 555)[0].starts_with("Linked to Corner Bakery."));
    let (_, view) = api(&state, "GET", &uri, json!({}), &owner).await;
    assert_eq!(view["linked"], true);
    fake.lock().expect("fake").updates = vec![message(3, 999, &format!("/start {code}"))];
    airobot::telegram::poll_once(&state, bot, 0)
        .await
        .expect("poll");
    assert!(
        sent_to(&fake, 999)[0].contains("belongs to an Airobot office"),
        "the code works once"
    );

    // Someone else writing to the bot is turned away; nothing is posted.
    fake.lock().expect("fake").updates = vec![message(4, 999, "Give me the invoices")];
    airobot::telegram::poll_once(&state, bot, 0)
        .await
        .expect("poll");
    assert!(sent_to(&fake, 999)[0].contains("only talks to its owner"));

    // The owner's message lands in the office chat, as them, once.
    let chat: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM threads WHERE workspace_id = $1 AND kind = 'office'")
            .bind(office)
            .fetch_one(&pool)
            .await
            .expect("office chat");
    fake.lock().expect("fake").updates = vec![message(5, 555, "What's on today?")];
    airobot::telegram::poll_once(&state, bot, 0)
        .await
        .expect("poll");
    // Telegram sending the same update again changes nothing.
    sqlx::query("UPDATE telegram_bots SET update_offset = 5 WHERE id = $1")
        .bind(bot)
        .execute(&pool)
        .await
        .expect("rewind");
    fake.lock().expect("fake").updates = vec![message(5, 555, "What's on today?")];
    airobot::telegram::poll_once(&state, bot, 0)
        .await
        .expect("poll");
    let posted: Vec<(Option<uuid::Uuid>, String)> = sqlx::query_as(
        "SELECT author_user_id, body FROM notes WHERE thread_id = $1 AND body = 'What''s on today?'",
    )
    .bind(chat)
    .fetch_all(&pool)
    .await
    .expect("notes");
    let owner_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE email = 'baker@example.com'")
            .fetch_one(&pool)
            .await
            .expect("owner");
    assert_eq!(
        posted,
        vec![(Some(owner_id), "What's on today?".to_string())]
    );

    // A bot's message in the office chat goes to Telegram; a person's doesn't.
    sent_to(&fake, 555);
    let bot_note = NoteEvent::new(
        chat,
        uuid::Uuid::new_v4(),
        None,
        Some("Architect"),
        "Today: 3 orders.",
        "status",
        chrono::Utc::now(),
    );
    airobot::telegram::relay_note(&state, &bot_note).await;
    let person_note = NoteEvent::new(
        chat,
        uuid::Uuid::new_v4(),
        Some(owner_id),
        Some("baker"),
        "Thanks",
        "status",
        chrono::Utc::now(),
    );
    airobot::telegram::relay_note(&state, &person_note).await;
    assert_eq!(
        sent_to(&fake, 555),
        vec!["Architect: Today: 3 orders.".to_string()]
    );

    // An approval arrives once, with buttons; Approve decides it.
    let task: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO tasks (workspace_id, title, status) VALUES ($1, 'Email the caterer', 'waiting_approval') RETURNING id",
    )
    .bind(office)
    .fetch_one(&pool)
    .await
    .expect("task");
    let approval: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO approvals (workspace_id, task_id, requested_action, risk_level, requested_by)
         VALUES ($1, $2, 'Email the caterer the order', 'low', 'agent:Sales') RETURNING id",
    )
    .bind(office)
    .bind(task)
    .fetch_one(&pool)
    .await
    .expect("approval");
    assert_eq!(airobot::telegram::send_approvals(&state).await, 1);
    let sent = fake.lock().expect("fake").calls.clone();
    let card = sent
        .iter()
        .find(|(m, _)| m == "sendMessage")
        .expect("sent")
        .1
        .clone();
    assert!(card["text"]
        .as_str()
        .unwrap_or_default()
        .contains("Email the caterer the order"));
    assert_eq!(
        card["reply_markup"]["inline_keyboard"][0][0]["callback_data"],
        format!("ap:{approval}:y")
    );
    assert_eq!(
        airobot::telegram::send_approvals(&state).await,
        0,
        "sent once"
    );
    fake.lock().expect("fake").calls.clear();

    let press = |id: i64, chat: i64| {
        json!({ "update_id": id, "callback_query": {
            "id": format!("cb{id}"), "data": format!("ap:{approval}:y"),
            "message": { "message_id": 42, "chat": { "id": chat, "type": "private" }, "text": "Approval needed" }
        }})
    };
    fake.lock().expect("fake").updates = vec![press(6, 999)];
    airobot::telegram::poll_once(&state, bot, 0)
        .await
        .expect("poll");
    let status: String = sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
        .bind(approval)
        .fetch_one(&pool)
        .await
        .expect("approval");
    assert_eq!(status, "pending", "a press from another chat does nothing");
    fake.lock().expect("fake").updates = vec![press(7, 555)];
    airobot::telegram::poll_once(&state, bot, 0)
        .await
        .expect("poll");
    let status: String = sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
        .bind(approval)
        .fetch_one(&pool)
        .await
        .expect("approval");
    assert_eq!(status, "approved");
    let calls = fake.lock().expect("fake").calls.clone();
    assert!(calls
        .iter()
        .any(|(m, b)| m == "answerCallbackQuery" && b["text"] == "Approved"));
    assert!(calls.iter().any(|(m, b)| m == "editMessageText"
        && b["text"]
            .as_str()
            .unwrap_or_default()
            .ends_with("✓ Approved")));

    // A new link replaces the old one; disconnecting stops the bot.
    let (status, fresh) = api(&state, "POST", &format!("{uri}/link"), json!({}), &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(fresh["link_url"].as_str(), Some(link.as_str()));
    let (status, gone) = api(&state, "POST", &format!("{uri}/delete"), json!({}), &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(gone["connected"], false);
    assert_eq!(
        airobot::telegram::poll_once(&state, bot, 0).await,
        Err("disconnected".to_string())
    );
}
