// Talk to an office from Slack (issue #33), against a stand-in for the
// Slack Web API: connecting the app, checking every call Slack makes is
// signed, linking the owner's account with the code, messages both ways,
// and approvals with buttons.
mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use airobot::auth::secret;
use airobot::state::{AppState, NoteEvent};
use axum::body::Body;
use axum::extract::Path as AxPath;
use axum::http::StatusCode;
use axum::routing::post;
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

const SECRET: &str = "8f742231b10e8888abcd99eeeeee85a5";
const BOT_TOKEN: &str = "xoxb-1111-2222-abcdefghijklmnop";

type Calls = Arc<Mutex<Vec<(String, Value)>>>;

/// A stand-in for slack.com/api: knows one app and records every call.
async fn fake_slack() -> (String, Calls) {
    let calls: Calls = Arc::default();
    let seen = calls.clone();
    let app = axum::Router::new().route(
        "/slack.com/api/{method}",
        post(
            move |AxPath(method): AxPath<String>, headers: axum::http::HeaderMap, body: String| {
                let seen = seen.clone();
                async move {
                    let body: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    seen.lock().expect("calls").push((method.clone(), body));
                    let authorized = headers.get("authorization").and_then(|v| v.to_str().ok())
                        == Some(&format!("Bearer {BOT_TOKEN}"));
                    if !authorized {
                        return axum::Json(json!({ "ok": false, "error": "invalid_auth" }));
                    }
                    match method.as_str() {
                        "auth.test" => axum::Json(json!({
                            "ok": true, "team_id": "T1", "team": "Bakery HQ", "user_id": "UBOT"
                        })),
                        _ => axum::Json(json!({ "ok": true, "ts": "1.2" })),
                    }
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

fn with_slack(pool: PgPool, origin: &str) -> AppState {
    let mut s = common::state(pool);
    let mut config = (*s.config).clone();
    config.connectors.test_origin = Some(origin.to_string());
    config.app_base_url = "https://office.example".to_string();
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

fn signature(secret: &str, ts: i64, body: &str) -> String {
    let mut mac =
        <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(secret.as_bytes()).expect("key");
    mac.update(format!("v0:{ts}:{body}").as_bytes());
    let bytes = mac.finalize().into_bytes();
    format!(
        "v0={}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}

/// What Slack sends: the body, signed with `secret` at `age` seconds ago.
async fn from_slack(
    state: &AppState,
    uri: &str,
    body: String,
    content_type: &str,
    secret: &str,
    age: i64,
    retry: bool,
) -> StatusCode {
    let ts = chrono::Utc::now().timestamp() - age;
    let mut req = axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", content_type)
        .header("x-forwarded-for", "127.0.0.1")
        .header("x-slack-request-timestamp", ts.to_string())
        .header("x-slack-signature", signature(secret, ts, &body));
    if retry {
        req = req.header("x-slack-retry-num", "1");
    }
    common::send_with_state(state.clone(), req.body(Body::from(body)).expect("request"))
        .await
        .status
}

fn dm(event_id: &str, user: &str, text: &str) -> String {
    json!({
        "type": "event_callback", "event_id": event_id,
        "event": { "type": "message", "channel_type": "im", "channel": "D1", "user": user, "text": text }
    })
    .to_string()
}

fn form(payload: &Value) -> String {
    let raw = payload.to_string();
    let encoded: String = raw
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("payload={encoded}")
}

/// Waits (up to two seconds) for a call matching `pred`, for work Slack
/// events start after the answer.
async fn wait_for(calls: &Calls, pred: impl Fn(&str, &Value) -> bool) -> Option<Value> {
    for _ in 0..40 {
        if let Some((_, b)) = calls
            .lock()
            .expect("calls")
            .iter()
            .find(|(m, b)| pred(m, b))
            .cloned()
        {
            return Some(b);
        }
        tokio::time::sleep(StdDuration::from_millis(50)).await;
    }
    None
}

async fn owner_notes(pool: &PgPool, chat: uuid::Uuid, body: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM notes WHERE thread_id = $1 AND body = $2 AND author_user_id IS NOT NULL")
        .bind(chat)
        .bind(body)
        .fetch_one(pool)
        .await
        .expect("notes")
}

#[sqlx::test]
async fn an_office_talks_from_slack(pool: PgPool) {
    let (origin, calls) = fake_slack().await;
    let state = with_slack(pool.clone(), &origin);
    let owner = sign_in(&pool, "slackowner@example.com", "slackowner").await;
    let office = new_office(&pool, &owner, "Corner Bakery").await;
    let uri = format!("/api/offices/{office}/slack");
    let events = format!("/slack/events/{office}");
    let actions = format!("/slack/actions/{office}");

    // Before connecting: the manifest points Slack at this server.
    let (status, view) = api(&state, "GET", &uri, json!({}), &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["connected"], false);
    assert_eq!(view["available"], true);
    let manifest = view["manifest"].as_str().unwrap_or_default();
    assert!(
        manifest.contains(&format!("https://office.example/slack/events/{office}")),
        "{manifest}"
    );
    // Slack checks the URL while the app is being made.
    let res = common::send_with_state(
        state.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri(&events)
            .header("content-type", "application/json")
            .header("x-forwarded-for", "127.0.0.1")
            .body(Body::from(
                r#"{"type":"url_verification","challenge":"abc123"}"#,
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(res.body["challenge"], "abc123");

    // Wrong credentials are refused; the right ones connect.
    let (status, _) = api(
        &state,
        "POST",
        &uri,
        json!({ "bot_token": "nope", "signing_secret": SECRET }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, err) = api(
        &state,
        "POST",
        &uri,
        json!({ "bot_token": "xoxb-wrong-wrong", "signing_secret": SECRET }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["error"]
            .as_str()
            .unwrap_or_default()
            .contains("invalid_auth"),
        "{err}"
    );
    let (status, made) = api(
        &state,
        "POST",
        &uri,
        json!({ "bot_token": BOT_TOKEN, "signing_secret": SECRET, "idempotency_key": "sl-1" }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["team_name"], "Bakery HQ");
    let code = made["link_code"].as_str().expect("code").to_string();
    let (token_enc, secret_enc): (String, String) =
        sqlx::query_as("SELECT token_encrypted, signing_secret_encrypted FROM slack_apps")
            .fetch_one(&pool)
            .await
            .expect("row");
    assert!(
        !token_enc.contains(BOT_TOKEN) && !secret_enc.contains(SECRET),
        "kept encrypted"
    );
    let other = sign_in(&pool, "nosy@example.com", "nosy").await;
    let (status, _) = api(&state, "GET", &uri, json!({}), &other).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Unsigned, wrongly signed and stale calls are refused.
    let body = dm("Ev0", "UOWNER", &format!("link {code}"));
    let unsigned = common::send_with_state(
        state.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri(&events)
            .header("content-type", "application/json")
            .header("x-forwarded-for", "127.0.0.1")
            .body(Body::from(body.clone()))
            .expect("request"),
    )
    .await;
    assert_eq!(unsigned.status, StatusCode::FORBIDDEN);
    assert_eq!(
        from_slack(
            &state,
            &events,
            body.clone(),
            "application/json",
            "wrong-secret-000000",
            0,
            false
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        from_slack(
            &state,
            &events,
            body,
            "application/json",
            SECRET,
            600,
            false
        )
        .await,
        StatusCode::FORBIDDEN
    );

    // A wrong code doesn't link; the right one does.
    let res = from_slack(
        &state,
        &events,
        dm("Ev1", "UOWNER", "link 00000000"),
        "application/json",
        SECRET,
        0,
        false,
    )
    .await;
    assert_eq!(res, StatusCode::OK);
    assert!(wait_for(&calls, |m, b| m == "chat.postMessage"
        && b["text"]
            .as_str()
            .unwrap_or_default()
            .contains("only talks to its owner"))
    .await
    .is_some());
    calls.lock().expect("calls").clear();
    from_slack(
        &state,
        &events,
        dm("Ev2", "UOWNER", &format!("link {code}")),
        "application/json",
        SECRET,
        0,
        false,
    )
    .await;
    let linked = wait_for(&calls, |m, b| {
        m == "chat.postMessage"
            && b["text"]
                .as_str()
                .unwrap_or_default()
                .starts_with("Linked to Corner Bakery")
    })
    .await;
    assert_eq!(linked.expect("linked reply")["channel"], "D1");
    let (_, view) = api(&state, "GET", &uri, json!({}), &owner).await;
    assert_eq!(view["linked"], true);

    // The owner's message lands in the office chat, once, even when Slack
    // retries or sends the event again.
    let chat: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM threads WHERE workspace_id = $1 AND kind = 'office'")
            .bind(office)
            .fetch_one(&pool)
            .await
            .expect("office chat");
    from_slack(
        &state,
        &events,
        dm("Ev3", "UOWNER", "What's on today?"),
        "application/json",
        SECRET,
        0,
        false,
    )
    .await;
    from_slack(
        &state,
        &events,
        dm("Ev3", "UOWNER", "What's on today?"),
        "application/json",
        SECRET,
        0,
        true,
    )
    .await;
    from_slack(
        &state,
        &events,
        dm("Ev3", "UOWNER", "What's on today?"),
        "application/json",
        SECRET,
        0,
        false,
    )
    .await;
    for _ in 0..40 {
        if owner_notes(&pool, chat, "What's on today?").await > 0 {
            break;
        }
        tokio::time::sleep(StdDuration::from_millis(50)).await;
    }
    tokio::time::sleep(StdDuration::from_millis(300)).await;
    assert_eq!(owner_notes(&pool, chat, "What's on today?").await, 1);

    // Someone else in the Slack workspace is turned away.
    calls.lock().expect("calls").clear();
    from_slack(
        &state,
        &events,
        dm("Ev4", "USOMEONE", "Show me the invoices"),
        "application/json",
        SECRET,
        0,
        false,
    )
    .await;
    assert!(wait_for(&calls, |m, b| m == "chat.postMessage"
        && b["text"]
            .as_str()
            .unwrap_or_default()
            .contains("only talks to its owner"))
    .await
    .is_some());
    assert_eq!(owner_notes(&pool, chat, "Show me the invoices").await, 0);

    // A bot's message in the office chat goes to Slack; a person's doesn't.
    calls.lock().expect("calls").clear();
    let owner_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE email = 'slackowner@example.com'")
            .fetch_one(&pool)
            .await
            .expect("owner");
    airobot::slack::relay_note(
        &state,
        &NoteEvent::new(
            chat,
            uuid::Uuid::new_v4(),
            None,
            Some("Architect"),
            "Today: 3 orders.",
            "status",
            chrono::Utc::now(),
        ),
    )
    .await;
    airobot::slack::relay_note(
        &state,
        &NoteEvent::new(
            chat,
            uuid::Uuid::new_v4(),
            Some(owner_id),
            Some("slackowner"),
            "Thanks",
            "status",
            chrono::Utc::now(),
        ),
    )
    .await;
    let posted: Vec<String> = calls
        .lock()
        .expect("calls")
        .iter()
        .filter(|(m, _)| m == "chat.postMessage")
        .map(|(_, b)| b["text"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(posted, vec!["*Architect:* Today: 3 orders.".to_string()]);

    // An approval arrives once with buttons; only the owner's press decides.
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
    calls.lock().expect("calls").clear();
    assert_eq!(airobot::slack::send_approvals(&state).await, 1);
    assert_eq!(airobot::slack::send_approvals(&state).await, 0, "sent once");
    let card = calls.lock().expect("calls")[0].1.clone();
    assert_eq!(
        card["blocks"][1]["elements"][0]["value"],
        format!("y:{approval}")
    );

    let press = |user: &str| {
        form(&json!({
            "type": "block_actions",
            "user": { "id": user },
            "actions": [{ "action_id": "approve", "value": format!("y:{approval}") }],
            "container": { "channel_id": "D1", "message_ts": "1.2" },
            "message": { "text": "Approval needed" }
        }))
    };
    calls.lock().expect("calls").clear();
    assert_eq!(
        from_slack(
            &state,
            &actions,
            press("USOMEONE"),
            "application/x-www-form-urlencoded",
            SECRET,
            0,
            false
        )
        .await,
        StatusCode::OK
    );
    let status: String = sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
        .bind(approval)
        .fetch_one(&pool)
        .await
        .expect("approval");
    assert_eq!(status, "pending");
    assert_eq!(
        from_slack(
            &state,
            &actions,
            press("UOWNER"),
            "application/x-www-form-urlencoded",
            "wrong-secret-000000",
            0,
            false
        )
        .await,
        StatusCode::FORBIDDEN
    );
    from_slack(
        &state,
        &actions,
        press("UOWNER"),
        "application/x-www-form-urlencoded",
        SECRET,
        0,
        false,
    )
    .await;
    let status: String = sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
        .bind(approval)
        .fetch_one(&pool)
        .await
        .expect("approval");
    assert_eq!(status, "approved");
    let updates: Vec<String> = calls
        .lock()
        .expect("calls")
        .iter()
        .filter(|(m, _)| m == "chat.update")
        .map(|(_, b)| b["text"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        updates[0].ends_with("Only this office's owner can decide its approvals."),
        "{updates:?}"
    );
    assert!(updates[1].ends_with("✓ Approved"), "{updates:?}");

    // Disconnected: Slack's calls are no longer accepted.
    let (status, _) = api(&state, "POST", &format!("{uri}/delete"), json!({}), &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        from_slack(
            &state,
            &events,
            dm("Ev9", "UOWNER", "hello"),
            "application/json",
            SECRET,
            0,
            false
        )
        .await,
        StatusCode::NOT_FOUND
    );
}
