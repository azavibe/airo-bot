// A phone receptionist per office (issue #32), against a stand-in for the
// Twilio API and signed calls from "Twilio": connecting a number, a call
// that books a table into the office's Appointments app, handing over to a
// person, silence, and the line in the office chat when the call ends.
mod common;

use std::sync::{Arc, Mutex};

use airobot::auth::secret;
use airobot::state::AppState;
use axum::body::Body;
use axum::extract::Path as AxPath;
use axum::http::StatusCode;
use axum::routing::{get, post};
use base64::Engine;
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

/// A made-up Account SID, built here so it never reads as a real one.
fn sid() -> &'static str {
    static FAKE_SID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FAKE_SID.get_or_init(|| format!("AC{}", "0123456789abcdef".repeat(2)))
}
const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const NUMBER: &str = "+15550001111";

type Calls = Arc<Mutex<Vec<(String, String)>>>;

/// A stand-in for api.twilio.com: one account with one number; records
/// every change to the number.
async fn fake_twilio() -> (String, Calls) {
    let calls: Calls = Arc::default();
    let seen = calls.clone();
    let authorized = |headers: &axum::http::HeaderMap| {
        let expected = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{}:{TOKEN}", sid()))
        );
        headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(expected.as_str())
    };
    let app = axum::Router::new()
        .route(
            "/api.twilio.com/2010-04-01/Accounts/{sid}/IncomingPhoneNumbers.json",
            get(
                move |headers: axum::http::HeaderMap,
                      axum::extract::RawQuery(q): axum::extract::RawQuery| async move {
                    if !authorized(&headers) {
                        return (
                            axum::http::StatusCode::UNAUTHORIZED,
                            axum::Json(json!({ "message": "Authenticate" })),
                        );
                    }
                    let mine = q.unwrap_or_default().contains("%2B15550001111");
                    let numbers = if mine {
                        json!([{ "sid": "PN1", "phone_number": NUMBER }])
                    } else {
                        json!([])
                    };
                    (
                        axum::http::StatusCode::OK,
                        axum::Json(json!({ "incoming_phone_numbers": numbers })),
                    )
                },
            ),
        )
        .route(
            "/api.twilio.com/2010-04-01/Accounts/{sid}/IncomingPhoneNumbers/{file}",
            post(
                move |AxPath((_sid, file)): AxPath<(String, String)>, body: String| {
                    let seen = seen.clone();
                    async move {
                        seen.lock().expect("calls").push((file, body));
                        axum::Json(json!({ "sid": "PN1" }))
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
    (format!("http://{addr}"), calls)
}

/// The receptionist's answers, scripted by what the caller said.
fn scripted_ai() -> airobot::local::LocalAi {
    Arc::new(|req: airobot::local::LocalAiRequest| {
        Box::pin(async move {
            assert!(req
                .system
                .contains("receptionist answering the phone for Corner Bakery"));
            let last = req
                .prompt
                .rsplit("Caller: ")
                .next()
                .unwrap_or("")
                .to_string();
            Ok(if last.contains("table for two") {
                "**Lovely!** What name is it under, and what time?".to_string()
            } else if last.contains("Lee") {
                "ADD appointments {\"client\": \"Lee\", \"service\": \"Table for two\", \"starts_at\": \"2026-11-25T19:00\", \"status\": \"booked\"}\nBooked for Lee at seven. Anything else?".to_string()
            } else if last.contains("That's all") {
                "Thanks for calling, see you tonight!\nEND_CALL".to_string()
            } else {
                "Sorry, could you say that again?".to_string()
            })
        }) as airobot::local::LocalAiFuture
    })
}

fn with_phone(pool: PgPool, origin: &str) -> AppState {
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

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// A call from Twilio to `/phone/<office>/<what>`, signed with `token`.
async fn from_twilio(
    state: &AppState,
    office: uuid::Uuid,
    what: &str,
    fields: &[(&str, &str)],
    token: &str,
) -> common::TestResponse {
    let url = format!("https://office.example/phone/{office}/{what}");
    let mut sorted: Vec<&(&str, &str)> = fields.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut mac = <hmac::Hmac<sha1::Sha1> as Mac>::new_from_slice(token.as_bytes()).expect("key");
    mac.update(url.as_bytes());
    for (k, v) in sorted {
        mac.update(k.as_bytes());
        mac.update(v.as_bytes());
    }
    let signature = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    let body = fields
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    common::send_with_state(
        state.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri(format!("/phone/{office}/{what}"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("x-forwarded-for", "127.0.0.1")
            .header("x-twilio-signature", signature)
            .body(Body::from(body))
            .expect("request"),
    )
    .await
}

fn said(state_of_call: &str) -> Vec<(String, String)> {
    vec![
        ("CallSid".to_string(), state_of_call.to_string()),
        ("AccountSid".to_string(), sid().to_string()),
    ]
}

#[sqlx::test]
async fn the_office_answers_the_phone(pool: PgPool) {
    let (origin, changes) = fake_twilio().await;
    let state = with_phone(pool.clone(), &origin);
    let owner = sign_in(&pool, "phoneowner@example.com", "phoneowner").await;
    let office = new_office(&pool, &owner, "Corner Bakery").await;
    let uri = format!("/api/offices/{office}/phone");
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

    // Wrong details are refused; a number not in the account too.
    let (status, _) = api(
        &state,
        "POST",
        &uri,
        json!({ "account_sid": "nope", "auth_token": TOKEN, "phone_number": NUMBER }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, err) = api(
        &state,
        "POST",
        &uri,
        json!({ "account_sid": sid(), "auth_token": TOKEN, "phone_number": "+15559999999" }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["error"]
            .as_str()
            .unwrap_or_default()
            .contains("isn't in this Twilio account"),
        "{err}"
    );

    // Connected: the number's calls now come here.
    let (status, made) = api(
        &state,
        "POST",
        &uri,
        json!({ "account_sid": sid(), "auth_token": TOKEN, "phone_number": "+1 555 000 1111",
                "forward_to": "+1 (555) 222-3333", "idempotency_key": "ph-1" }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["phone_number"], NUMBER);
    let set = changes.lock().expect("calls").clone();
    assert_eq!(set[0].0, "PN1.json");
    assert!(
        set[0].1.contains(&format!(
            "VoiceUrl={}",
            encode_form(&format!("https://office.example/phone/{office}/voice"))
        )),
        "{:?}",
        set[0]
    );
    let stored: String = sqlx::query_scalar("SELECT auth_token_encrypted FROM phone_lines")
        .fetch_one(&pool)
        .await
        .expect("line");
    assert!(!stored.contains(TOKEN), "kept encrypted");
    let room: uuid::Uuid = made["room_id"]
        .as_str()
        .expect("room")
        .parse()
        .expect("uuid");
    let other = sign_in(&pool, "nosy@example.com", "nosy").await;
    let (status, _) = api(&state, "GET", &uri, json!({}), &other).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A call starts: only signed calls are taken; the greeting asks.
    let start = [
        ("CallSid", "CA1"),
        ("AccountSid", sid()),
        ("From", "+15557778888"),
        ("To", NUMBER),
    ];
    assert_eq!(
        from_twilio(
            &state,
            office,
            "voice",
            &start,
            "ffffffffffffffffffffffffffffffff"
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    let greeting = from_twilio(&state, office, "voice", &start, TOKEN).await;
    assert_eq!(greeting.status, StatusCode::OK);
    assert!(greeting.text.contains("<Gather"), "{}", greeting.text);
    assert!(
        greeting
            .text
            .contains("Hello, you&apos;ve reached Corner Bakery."),
        "{}",
        greeting.text
    );
    assert!(greeting.text.contains("Polly.Joanna-Neural"));

    // The caller books a table; it lands in Appointments.
    let mut turn = said("CA1");
    turn.push((
        "SpeechResult".into(),
        "Hi, I'd like a table for two tonight".into(),
    ));
    let pairs: Vec<(&str, &str)> = turn.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let res = from_twilio(&state, office, "turn", &pairs, TOKEN).await;
    assert!(
        res.text
            .contains("Lovely! What name is it under, and what time?"),
        "markdown is spoken plain: {}",
        res.text
    );
    assert!(res.text.contains("<Gather"));
    let mut turn = said("CA1");
    turn.push(("SpeechResult".into(), "It's under Lee, at seven".into()));
    let pairs: Vec<(&str, &str)> = turn.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let res = from_twilio(&state, office, "turn", &pairs, TOKEN).await;
    assert!(
        res.text.contains("Booked for Lee at seven."),
        "{}",
        res.text
    );
    assert!(
        !res.text.contains("ADD"),
        "the record line isn't spoken: {}",
        res.text
    );
    let booked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM office_records r JOIN office_apps a ON a.id = r.app_id
         WHERE a.workspace_id = $1 AND a.slug = 'appointments' AND r.data->>'client' = 'Lee'",
    )
    .bind(office)
    .fetch_one(&pool)
    .await
    .expect("records");
    assert_eq!(booked, 1);
    // The conversation shows in the Phone calls room.
    let notes: Vec<String> =
        sqlx::query_scalar("SELECT body FROM notes WHERE thread_id = $1 ORDER BY created_at")
            .bind(room)
            .fetch_all(&pool)
            .await
            .expect("notes");
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("📞 +15557778888: Hi, I'd like a table")),
        "{notes:?}"
    );

    // Asking for a person puts them through to the forward number.
    let mut turn = said("CA1");
    turn.push((
        "SpeechResult".into(),
        "Actually can I speak to a real person".into(),
    ));
    let pairs: Vec<(&str, &str)> = turn.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let res = from_twilio(&state, office, "turn", &pairs, TOKEN).await;
    assert!(
        res.text
            .contains("<Dial callerId=\"+15550001111\">+15552223333</Dial>"),
        "{}",
        res.text
    );

    // The call ends: one line in the office chat, however often Twilio says so.
    let done = [
        ("CallSid", "CA1"),
        ("AccountSid", sid()),
        ("CallStatus", "completed"),
    ];
    assert_eq!(
        from_twilio(&state, office, "status", &done, TOKEN)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    from_twilio(&state, office, "status", &done, TOKEN).await;
    let chat_lines: Vec<String> = sqlx::query_scalar(
        "SELECT n.body FROM notes n JOIN threads t ON t.id = n.thread_id
         WHERE t.workspace_id = $1 AND t.kind = 'office' AND n.body LIKE '📞 Call from%'",
    )
    .bind(office)
    .fetch_all(&pool)
    .await
    .expect("chat");
    assert_eq!(
        chat_lines,
        vec!["📞 Call from +15557778888 ended: 1 record added, handed over to a person. The conversation is in Phone calls.".to_string()]
    );

    // A second call: silence is asked about once, then the call ends.
    let start = [
        ("CallSid", "CA2"),
        ("AccountSid", sid()),
        ("From", "+15550000000"),
        ("To", NUMBER),
    ];
    from_twilio(&state, office, "voice", &start, TOKEN).await;
    let quiet = [("CallSid", "CA2"), ("AccountSid", sid())];
    let res = from_twilio(&state, office, "turn", &quiet, TOKEN).await;
    assert!(
        res.text.contains("didn&apos;t catch that") && res.text.contains("<Gather"),
        "{}",
        res.text
    );
    let res = from_twilio(&state, office, "turn", &quiet, TOKEN).await;
    assert!(res.text.contains("<Hangup/>"), "{}", res.text);
    // "That's all": the receptionist says goodbye and hangs up.
    let start = [
        ("CallSid", "CA3"),
        ("AccountSid", sid()),
        ("From", "+15550000000"),
        ("To", NUMBER),
    ];
    from_twilio(&state, office, "voice", &start, TOKEN).await;
    let bye = [
        ("CallSid", "CA3"),
        ("AccountSid", sid()),
        ("SpeechResult", "That's all, thanks"),
    ];
    let res = from_twilio(&state, office, "turn", &bye, TOKEN).await;
    assert!(
        res.text.contains("see you tonight!") && res.text.ends_with("<Hangup/></Response>"),
        "{}",
        res.text
    );

    // Settings change without reconnecting; disconnecting frees the number.
    let (status, changed) = api(
        &state,
        "POST",
        &uri,
        json!({ "forward_to": "", "greeting": "Corner Bakery, how can I help?" }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert!(changed["forward_to"].is_null());
    let (status, _) = api(&state, "POST", &format!("{uri}/delete"), json!({}), &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        changes
            .lock()
            .expect("calls")
            .iter()
            .any(|(_, b)| b.contains("VoiceUrl=&")),
        "the voice URL is cleared"
    );
    let start = [
        ("CallSid", "CA4"),
        ("AccountSid", sid()),
        ("From", "+15550000000"),
        ("To", NUMBER),
    ];
    assert_eq!(
        from_twilio(&state, office, "voice", &start, TOKEN)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

/// How reqwest's form encoding writes a value.
fn encode_form(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'*' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
