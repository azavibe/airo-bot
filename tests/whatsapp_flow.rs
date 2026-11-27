// WhatsApp for an office, against a stand-in for Meta's Graph API and
// signed webhook calls from "Meta": connecting a number, the webhook check,
// a customer whose table is booked into the office's Appointments app, a
// message delivered twice, a person answering from the WhatsApp room (and
// the bot leaving that customer to them), the 24-hour window, asking for a
// person, and answering by hand only.
mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use airobot::auth::secret;
use airobot::state::AppState;
use axum::extract::Path as AxPath;
use axum::http::StatusCode;
use axum::routing::{get, post};
use chrono::{Duration, Utc};
use common::{authed_json_request, form_request, get_request, send, session_cookie};
use hmac::Mac;
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

const NUMBER_ID: &str = "1111111111";
const TOKEN: &str = "EAAGtestaccesstoken";
const APP_SECRET: &str = "0123456789abcdef0123456789abcdef";
const LEE: &str = "15550002222";

type Sent = Arc<Mutex<Vec<Value>>>;

/// A stand-in for graph.facebook.com: one business number; keeps every
/// message sent.
async fn fake_meta() -> (String, Sent) {
    let sent: Sent = Arc::default();
    let seen = sent.clone();
    let authorized = |headers: &axum::http::HeaderMap| {
        headers.get("authorization").and_then(|v| v.to_str().ok())
            == Some(format!("Bearer {TOKEN}").as_str())
    };
    let app = axum::Router::new()
        .route(
            "/graph.facebook.com/v26.0/{id}",
            get(
                move |headers: axum::http::HeaderMap, AxPath(id): AxPath<String>| async move {
                    if !authorized(&headers) {
                        return (
                            StatusCode::UNAUTHORIZED,
                            axum::Json(
                                json!({ "error": { "message": "Invalid OAuth access token." } }),
                            ),
                        );
                    }
                    if id != NUMBER_ID {
                        return (
                            StatusCode::BAD_REQUEST,
                            axum::Json(
                                json!({ "error": { "message": "Unsupported get request." } }),
                            ),
                        );
                    }
                    (
                        StatusCode::OK,
                        axum::Json(json!({
                            "id": NUMBER_ID,
                            "display_phone_number": "+1 555-000-1111",
                            "verified_name": "Corner Bakery",
                        })),
                    )
                },
            ),
        )
        .route(
            "/graph.facebook.com/v26.0/{id}/messages",
            post(
                move |headers: axum::http::HeaderMap, axum::Json(body): axum::Json<Value>| {
                    let seen = seen.clone();
                    async move {
                        if !authorized(&headers) {
                            return (StatusCode::UNAUTHORIZED, axum::Json(json!({})));
                        }
                        let mut all = seen.lock().expect("sent");
                        all.push(body);
                        let n = all.len();
                        (
                            StatusCode::OK,
                            axum::Json(json!({ "messages": [{ "id": format!("wamid.out.{n}") }] })),
                        )
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
    (format!("http://{addr}"), sent)
}

/// The receptionist's answers, scripted by the customer's last message.
fn scripted_ai() -> airobot::local::LocalAi {
    Arc::new(|req: airobot::local::LocalAiRequest| {
        Box::pin(async move {
            assert!(req
                .system
                .contains("receptionist answering WhatsApp messages for Corner Bakery"));
            // Lee's answers never see another customer's conversation.
            if req.system.contains("+15550002222") {
                assert!(!req.prompt.contains("someone else's secret"));
            }
            let last = req
                .prompt
                .rsplit("Customer: ")
                .next()
                .unwrap_or("")
                .to_string();
            Ok(if last.contains("table for two") {
                "Lovely! What time, and under what name?".to_string()
            } else if last.contains("Lee, at 7") {
                "ADD appointments {\"client\": \"Lee\", \"service\": \"Table for two\", \"starts_at\": \"2026-11-25T19:00\", \"status\": \"booked\"}\nBooked for Lee at seven. See you tonight!".to_string()
            } else if last.contains("talk to a person") {
                "Of course, someone from the team will reply here soon.\nTRANSFER".to_string()
            } else {
                "Thanks! Anything else?".to_string()
            })
        }) as airobot::local::LocalAiFuture
    })
}

fn with_meta(pool: PgPool, origin: &str) -> AppState {
    let mut s = common::state(pool);
    let mut config = (*s.config).clone();
    config.connectors.test_origin = Some(origin.to_string());
    config.app_base_url = "https://office.example".to_string();
    s.config = Arc::new(config);
    s.local_ai = Some(scripted_ai());
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

async fn new_office(state: &AppState, cookie: &str, name: &str) -> Uuid {
    let (status, body) = api(
        state,
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

/// A webhook call from "Meta", signed with `secret`.
async fn from_meta(
    state: &AppState,
    office: Uuid,
    payload: &Value,
    secret: Option<&str>,
) -> StatusCode {
    let body = serde_json::to_vec(payload).expect("json");
    let mut req = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/whatsapp/{office}/webhook"))
        .header("content-type", "application/json")
        .header("x-forwarded-for", "203.0.113.50");
    if let Some(secret) = secret {
        let mut mac =
            <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(secret.as_bytes()).expect("key");
        mac.update(&body);
        let hex: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        req = req.header("x-hub-signature-256", format!("sha256={hex}"));
    }
    let req = req.body(axum::body::Body::from(body)).expect("request");
    common::send_with_state(state.clone(), req).await.status
}

/// A customer's text message as Meta delivers it.
fn message(customer: &str, name: &str, wamid: &str, text: &str) -> Value {
    json!({
        "object": "whatsapp_business_account",
        "entry": [{ "id": "WABA", "changes": [{ "field": "messages", "value": {
            "messaging_product": "whatsapp",
            "metadata": { "display_phone_number": "15550001111", "phone_number_id": NUMBER_ID },
            "contacts": [{ "wa_id": customer, "profile": { "name": name } }],
            "messages": [{ "from": customer, "id": wamid, "timestamp": "1764000000",
                           "type": "text", "text": { "body": text } }],
        }}]}],
    })
}

/// The texts sent to `customer` so far (not read receipts).
fn texts_to(sent: &Sent, customer: &str) -> Vec<String> {
    sent.lock()
        .expect("sent")
        .iter()
        .filter(|m| m["type"] == "text" && m["to"] == customer)
        .filter_map(|m| m["text"]["body"].as_str().map(str::to_string))
        .collect()
}

/// Waits for the replies sent in the background.
async fn wait_for_texts(sent: &Sent, customer: &str, n: usize) -> Vec<String> {
    for _ in 0..100 {
        let texts = texts_to(sent, customer);
        if texts.len() >= n {
            return texts;
        }
        tokio::time::sleep(StdDuration::from_millis(50)).await;
    }
    texts_to(sent, customer)
}

/// Waits for a note containing `needle` in `thread`.
async fn wait_for_note(pool: &PgPool, thread: Uuid, needle: &str) -> bool {
    for _ in 0..100 {
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM notes WHERE thread_id = $1 AND strpos(body, $2) > 0)",
        )
        .bind(thread)
        .bind(needle)
        .fetch_one(pool)
        .await
        .expect("notes");
        if found {
            return true;
        }
        tokio::time::sleep(StdDuration::from_millis(50)).await;
    }
    false
}

async fn office_chat(pool: &PgPool, office: Uuid) -> Uuid {
    sqlx::query_scalar(
        "SELECT id FROM threads WHERE workspace_id = $1 AND kind = 'office' AND deleted_at IS NULL",
    )
    .bind(office)
    .fetch_one(pool)
    .await
    .expect("office chat")
}

async fn connect(state: &AppState, office: Uuid, cookie: &str) -> Value {
    let (status, body) = api(
        state,
        "POST",
        &format!("/api/offices/{office}/whatsapp"),
        json!({ "phone_number_id": NUMBER_ID, "access_token": TOKEN, "app_secret": APP_SECRET }),
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    body
}

#[sqlx::test]
async fn a_customer_on_whatsapp_is_answered_booked_and_handed_to_a_person(pool: PgPool) {
    let (origin, sent) = fake_meta().await;
    let state = with_meta(pool.clone(), &origin);
    let owner = sign_in(&pool, "waowner@example.com", "waowner").await;
    let office = new_office(&state, &owner, "Corner Bakery").await;
    let uri = format!("/api/offices/{office}/whatsapp");
    let (status, _) = api(
        &state,
        "POST",
        &format!("/api/office/apps/install?office={office}"),
        json!({ "kind": "scheduling" }),
        &owner,
    )
    .await;
    assert!(status.is_success());

    let (status, view) = api(&state, "GET", &uri, json!({}), &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["connected"], false);
    assert_eq!(view["available"], true);

    // Wrong details are refused, with Meta's reason.
    let (status, body) = api(
        &state,
        "POST",
        &uri,
        json!({ "phone_number_id": NUMBER_ID, "access_token": "EAAGwrong", "app_secret": APP_SECRET }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]
        .as_str()
        .is_some_and(|e| e.contains("Invalid OAuth access token")));
    let (status, _) = api(
        &state,
        "POST",
        &uri,
        json!({ "phone_number_id": NUMBER_ID, "access_token": TOKEN, "app_secret": "short" }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let made = connect(&state, office, &owner).await;
    assert_eq!(made["display_number"], "+1 555-000-1111");
    assert_eq!(
        made["webhook_url"],
        format!("https://office.example/whatsapp/{office}/webhook")
    );
    let verify_token = made["verify_token"].as_str().expect("token").to_string();
    let room: Uuid = made["room_id"]
        .as_str()
        .expect("room")
        .parse()
        .expect("uuid");
    // The credentials are kept only encrypted.
    let (stored_token, stored_secret): (String, String) = sqlx::query_as(
        "SELECT access_token_encrypted, app_secret_encrypted FROM whatsapp_numbers
         WHERE workspace_id = $1 AND deleted_at IS NULL",
    )
    .bind(office)
    .fetch_one(&pool)
    .await
    .expect("number");
    assert!(!stored_token.contains(TOKEN) && !stored_secret.contains(APP_SECRET));
    let (_, view) = api(&state, "GET", &uri, json!({}), &owner).await;
    assert_eq!(view["verify_token"], verify_token.as_str());

    // Meta checks the webhook with the verify token.
    let check = |token: &str| {
        format!("/whatsapp/{office}/webhook?hub.mode=subscribe&hub.verify_token={token}&hub.challenge=1158201444")
    };
    let res =
        common::send_with_state(state.clone(), get_request(&check(&verify_token), None)).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.text, "1158201444");
    let res = common::send_with_state(state.clone(), get_request(&check("guess"), None)).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);

    // Unsigned or wrongly signed calls are refused.
    let hello = message(
        LEE,
        "Lee",
        "wamid.in.1",
        "Hi! Do you have a table for two tonight?",
    );
    assert_eq!(
        from_meta(&state, office, &hello, None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        from_meta(
            &state,
            office,
            &hello,
            Some("fedcba9876543210fedcba9876543210")
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    assert!(texts_to(&sent, LEE).is_empty());

    // Another customer's conversation never reaches Lee's answers.
    let other = message("15550003333", "Sam", "wamid.in.0", "someone else's secret");
    assert_eq!(
        from_meta(&state, office, &other, Some(APP_SECRET)).await,
        StatusCode::OK
    );
    wait_for_texts(&sent, "15550003333", 1).await;

    // Lee asks; the Receptionist answers. Meta delivering it again changes nothing.
    assert_eq!(
        from_meta(&state, office, &hello, Some(APP_SECRET)).await,
        StatusCode::OK
    );
    let texts = wait_for_texts(&sent, LEE, 1).await;
    assert_eq!(texts, ["Lovely! What time, and under what name?"]);
    assert_eq!(
        from_meta(&state, office, &hello, Some(APP_SECRET)).await,
        StatusCode::OK
    );
    let booked = message(LEE, "Lee", "wamid.in.2", "Lee, at 7 please");
    assert_eq!(
        from_meta(&state, office, &booked, Some(APP_SECRET)).await,
        StatusCode::OK
    );
    let texts = wait_for_texts(&sent, LEE, 2).await;
    assert_eq!(texts.len(), 2, "{texts:?}");
    assert_eq!(texts[1], "Booked for Lee at seven. See you tonight!");

    // The booking is in the office's Appointments app, and Lee in Contacts.
    let appointments: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM office_records r JOIN office_apps a ON a.id = r.app_id
         WHERE a.workspace_id = $1 AND a.slug = 'appointments' AND r.data->>'client' = 'Lee'",
    )
    .bind(office)
    .fetch_one(&pool)
    .await
    .expect("records");
    assert_eq!(appointments, 1);
    let contact: Option<String> = sqlx::query_scalar(
        "SELECT name FROM contacts WHERE workspace_id = $1 AND phone = '+15550002222' AND deleted_at IS NULL",
    )
    .bind(office)
    .fetch_optional(&pool)
    .await
    .expect("contacts");
    assert_eq!(contact.as_deref(), Some("Lee"));

    // The conversation shows in the WhatsApp room; the office chat hears of it.
    assert!(
        wait_for_note(
            &pool,
            room,
            "💬 Lee (+15550002222): Hi! Do you have a table"
        )
        .await
    );
    let chat = office_chat(&pool, office).await;
    assert!(wait_for_note(&pool, chat, "New WhatsApp conversation with Lee").await);

    // A person answers Lee from the room; the bot then leaves Lee to them.
    let (status, _) = api(
        &state,
        "POST",
        &format!("/threads/{room}/notes"),
        json!({ "body": "+15550002222 See you at 7, Lee! Ask for Maria.", "kind": "comment" }),
        &owner,
    )
    .await;
    assert!(status.is_success());
    let texts = wait_for_texts(&sent, LEE, 3).await;
    assert_eq!(texts[2], "See you at 7, Lee! Ask for Maria.");
    assert!(wait_for_note(&pool, room, "Sent to Lee on WhatsApp").await);
    let later = message(LEE, "Lee", "wamid.in.3", "Can I bring my dog?");
    assert_eq!(
        from_meta(&state, office, &later, Some(APP_SECRET)).await,
        StatusCode::OK
    );
    assert!(wait_for_note(&pool, room, "Can I bring my dog?").await);
    tokio::time::sleep(StdDuration::from_millis(200)).await;
    assert_eq!(texts_to(&sent, LEE).len(), 3, "the bot stays quiet");

    // Nobody who hasn't written in 24 hours can be messaged.
    let (status, _) = api(
        &state,
        "POST",
        &format!("/threads/{room}/notes"),
        json!({ "body": "+15559999999 Special offer!", "kind": "comment" }),
        &owner,
    )
    .await;
    assert!(status.is_success());
    assert!(wait_for_note(&pool, room, "only lets a business reply within 24 hours").await);
    assert!(texts_to(&sent, "15559999999").is_empty());

    // Only the person can connect or see it: not their bots, not others.
    let owner_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE handle = 'waowner'")
        .fetch_one(&pool)
        .await
        .expect("owner");
    let helper = Uuid::new_v4();
    let (raw_key, raw_secret, prefix) = airobot::auth::generate_agent_key(helper);
    sqlx::query(
        "INSERT INTO agents (id, user_id, name, handle, key_hash, key_prefix)
         VALUES ($1, $2, 'Helper', 'wa-helper', $3, $4)",
    )
    .bind(helper)
    .bind(owner_id)
    .bind(secret::hash_secret(&raw_secret).expect("hash"))
    .bind(&prefix)
    .execute(&pool)
    .await
    .expect("helper");
    let res =
        common::send_with_state(state.clone(), common::bearer_get_request(&uri, &raw_key)).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    let stranger = sign_in(&pool, "stranger@example.com", "stranger").await;
    let (status, _) = api(&state, "GET", &uri, json!({}), &stranger).await;
    assert!(status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND);

    // Disconnected: Meta's calls are no longer taken.
    let (status, _) = api(&state, "POST", &format!("{uri}/delete"), json!({}), &owner).await;
    assert_eq!(status, StatusCode::OK);
    let (_, view) = api(&state, "GET", &uri, json!({}), &owner).await;
    assert_eq!(view["connected"], false);
    let bye = message(LEE, "Lee", "wamid.in.4", "Thanks!");
    assert_eq!(
        from_meta(&state, office, &bye, Some(APP_SECRET)).await,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test]
async fn asking_for_a_person_tells_the_office_and_answering_can_be_left_to_people(pool: PgPool) {
    let (origin, sent) = fake_meta().await;
    let state = with_meta(pool.clone(), &origin);
    let owner = sign_in(&pool, "waperson@example.com", "waperson").await;
    let office = new_office(&state, &owner, "Corner Bakery").await;
    let made = connect(&state, office, &owner).await;
    let room: Uuid = made["room_id"]
        .as_str()
        .expect("room")
        .parse()
        .expect("uuid");
    let chat = office_chat(&pool, office).await;

    let ask = message(
        LEE,
        "Lee",
        "wamid.p.1",
        "I'd like to talk to a person please",
    );
    assert_eq!(
        from_meta(&state, office, &ask, Some(APP_SECRET)).await,
        StatusCode::OK
    );
    let texts = wait_for_texts(&sent, LEE, 1).await;
    assert_eq!(
        texts,
        ["Of course, someone from the team will reply here soon."]
    );
    assert!(
        wait_for_note(
            &pool,
            chat,
            "Lee (+15550002222) on WhatsApp would like a person"
        )
        .await
    );
    assert!(wait_for_note(&pool, room, "waiting for a person").await);

    // Answering by hand only: messages still arrive, nobody is answered.
    let (status, body) = api(
        &state,
        "POST",
        &format!("/api/offices/{office}/whatsapp"),
        json!({ "auto_reply": false }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let sam = message("15550003333", "Sam", "wamid.p.2", "Are you open on Sunday?");
    assert_eq!(
        from_meta(&state, office, &sam, Some(APP_SECRET)).await,
        StatusCode::OK
    );
    assert!(
        wait_for_note(
            &pool,
            room,
            "💬 Sam (+15550003333): Are you open on Sunday?"
        )
        .await
    );
    assert!(wait_for_note(&pool, chat, "New WhatsApp message from Sam").await);
    tokio::time::sleep(StdDuration::from_millis(200)).await;
    assert!(texts_to(&sent, "15550003333").is_empty());
}
