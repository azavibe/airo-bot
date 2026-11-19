// Shared helpers used across the tests/*.rs integration-test binaries; each
// one is compiled separately and only some use every helper here.
#![allow(dead_code)]

use std::sync::Arc;

use airobot::config::Config;
use airobot::email::Mailer;
use airobot::push::PushSender;
use airobot::state::AppState;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;

pub fn state(pool: PgPool) -> AppState {
    let (note_sender, _) = tokio::sync::broadcast::channel(256);
    AppState {
        pool,
        config: Arc::new(Config {
            database_url: String::new(),
            port: 8080,
            app_base_url: "http://localhost:8080".to_string(),
            smtp: None,
            vapid: None,
            production: false,
            dev_login: false,
            openrouter_api_key: None,
            upgrade_url: None,
            local: None,
            web_dist: "web/dist".into(),
            connectors: Default::default(),
            web: Default::default(),
        }),
        mailer: Arc::new(Mailer::new(None)),
        push: Arc::new(PushSender::new(None).expect("disabled push sender")),
        note_sender,
        dispatcher_wake: Arc::new(tokio::sync::Notify::new()),
        local_ai: None,
    }
}

/// Same as `state`, with the email-free dev sign-in switched on.
pub fn state_with_dev_login(pool: PgPool) -> AppState {
    let mut s = state(pool);
    s.config = Arc::new(Config {
        database_url: String::new(),
        port: 8080,
        app_base_url: "http://localhost:8080".to_string(),
        smtp: None,
        vapid: None,
        production: false,
        dev_login: true,
        openrouter_api_key: None,
        upgrade_url: None,
        local: None,
        web_dist: "web/dist".into(),
        connectors: Default::default(),
        web: Default::default(),
    });
    s
}

/// `send`, through a router built from a caller-supplied state (for tests
/// that need a server AI key or other config).
pub async fn send_with_state(state: AppState, req: Request<Body>) -> TestResponse {
    let response: Response<Body> = airobot::app(state).oneshot(req).await.expect("request");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("read body")
        .to_bytes();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    TestResponse {
        status,
        headers,
        body,
        text,
    }
}

pub struct TestResponse {
    pub status: StatusCode,
    pub headers: axum::http::HeaderMap,
    pub body: serde_json::Value,
    pub text: String,
}

/// Sends one request through the real router (in-process, no socket) and
/// parses the body as JSON -- every handler in this phase returns JSON by
/// default when `Accept` isn't `text/html`.
pub async fn send(pool: PgPool, req: Request<Body>) -> TestResponse {
    let response: Response<Body> = airobot::app(state(pool))
        .oneshot(req)
        .await
        .expect("request");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("read body")
        .to_bytes();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    TestResponse {
        status,
        headers,
        body,
        text,
    }
}

// The rate-limit layer always keys on the client IP, never an unverified
// Bearer token -- see rate_limit.rs.
// In production one of those is always present (Caddy sets the header, and
// main.rs wires ConnectInfo), but this in-process test harness goes through
// neither, so every request needs a stand-in IP or the governor layer
// itself 500s before a handler ever runs.
const TEST_CLIENT_IP: &str = "127.0.0.1";

pub fn json_request(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-forwarded-for", TEST_CLIENT_IP)
        .body(Body::from(body.to_string()))
        .expect("build request")
}

pub fn authed_json_request(
    method: &str,
    uri: &str,
    body: serde_json::Value,
    cookie: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-forwarded-for", TEST_CLIENT_IP);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    builder
        .body(Body::from(body.to_string()))
        .expect("build request")
}

pub fn bearer_json_request(
    method: &str,
    uri: &str,
    body: serde_json::Value,
    agent_key: &str,
) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-forwarded-for", TEST_CLIENT_IP)
        .header("authorization", format!("Bearer {agent_key}"))
        .body(Body::from(body.to_string()))
        .expect("build request")
}

pub fn bearer_get_request(uri: &str, agent_key: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("x-forwarded-for", TEST_CLIENT_IP)
        .header("authorization", format!("Bearer {agent_key}"))
        .body(Body::empty())
        .expect("build request")
}

pub fn form_request(
    method: &str,
    uri: &str,
    form: &[(&str, &str)],
    cookie: Option<&str>,
) -> Request<Body> {
    let body = encode_form(form);
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("x-forwarded-for", TEST_CLIENT_IP);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    builder.body(Body::from(body)).expect("build request")
}

pub fn post_request(uri: &str, cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("x-forwarded-for", TEST_CLIENT_IP);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    builder.body(Body::empty()).expect("build request")
}

pub fn get_request(uri: &str, cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("GET")
        .uri(uri)
        .header("x-forwarded-for", TEST_CLIENT_IP);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    builder.body(Body::empty()).expect("build request")
}

fn encode_form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Pulls the `air_session=...` pair out of a `Set-Cookie` header so it can
/// be replayed as a `Cookie` header on the next request.
pub fn session_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.to_string())
}
