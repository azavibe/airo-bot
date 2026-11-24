// "Check for updates" (issue #54): the server compares its own version with
// the newest published release and points at the installer. A small local
// server stands in for GitHub's release feed.
mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use airobot::auth::secret;
use airobot::config::Config;
use airobot::handlers::updates::CURRENT_VERSION;
use axum::http::{header, StatusCode};
use axum::routing::get;
use chrono::{Duration, Utc};
use common::{form_request, get_request, send, send_with_state, session_cookie};
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

/// A stand-in for GitHub's "latest release", counting how often it is asked.
async fn fake_feed(release: Value) -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = axum::Router::new().route(
        "/latest",
        get(move || {
            let counter = counter.clone();
            let release = release.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                axum::Json(release)
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
    (format!("http://{addr}/latest"), hits)
}

fn with_feed(pool: PgPool, feed: Option<String>) -> airobot::state::AppState {
    let mut s = common::state(pool);
    let mut config: Config = (*s.config).clone();
    config.update_feed = feed;
    s.config = Arc::new(config);
    s
}

fn check(uri: &str, cookie: Option<&str>) -> axum::http::Request<axum::body::Body> {
    let mut b = axum::http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("x-forwarded-for", "127.0.0.1")
        .header(header::ACCEPT, "application/json");
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    b.body(axum::body::Body::empty()).expect("request")
}

#[sqlx::test]
async fn a_newer_release_is_offered_with_its_installer(pool: PgPool) {
    let cookie = sign_in(&pool, "updates@example.com", "updater").await;
    let (feed, hits) = fake_feed(json!({
        "tag_name": "v99.1.0",
        "html_url": "https://github.com/azavibe/airo-bot/releases/tag/v99.1.0",
        "published_at": "2026-11-24T10:00:00Z",
        "draft": false,
        "prerelease": false,
        "assets": [
            { "name": "Airobot_99.1.0_aarch64.dmg", "browser_download_url": "https://example.com/a.dmg" },
            { "name": "Airobot_99.1.0_x64.dmg", "browser_download_url": "https://example.com/x.dmg" },
            { "name": "Airobot_99.1.0_x64-setup.exe", "browser_download_url": "https://example.com/s.exe" }
        ]
    }))
    .await;
    let state = with_feed(pool.clone(), Some(feed));

    // Signed-in only.
    let anon = send_with_state(state.clone(), check("/api/app/update", None)).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);

    let res = send_with_state(state.clone(), check("/api/app/update", Some(&cookie))).await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    assert_eq!(res.body["current"], CURRENT_VERSION);
    assert_eq!(res.body["latest"], "99.1.0");
    assert_eq!(res.body["available"], true);
    assert_eq!(
        res.body["release_url"],
        "https://github.com/azavibe/airo-bot/releases/tag/v99.1.0"
    );
    // The installer for the computer the server runs on (none on Linux).
    let expected = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => json!("https://example.com/a.dmg"),
        ("macos", _) => json!("https://example.com/x.dmg"),
        ("windows", _) => json!("https://example.com/s.exe"),
        _ => Value::Null,
    };
    assert_eq!(res.body["download_url"], expected);

    // Asked again soon: answered from memory, GitHub isn't called again.
    let again = send_with_state(state.clone(), check("/api/app/update", Some(&cookie))).await;
    assert_eq!(again.body, res.body);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[sqlx::test]
async fn the_same_version_is_up_to_date_and_checks_can_be_off(pool: PgPool) {
    let cookie = sign_in(&pool, "current@example.com", "current").await;
    let (feed, _) = fake_feed(json!({
        "tag_name": format!("v{CURRENT_VERSION}"),
        "html_url": "https://github.com/azavibe/airo-bot/releases",
        "assets": []
    }))
    .await;
    let res = send_with_state(
        with_feed(pool.clone(), Some(feed)),
        check("/api/app/update", Some(&cookie)),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.body["latest"], CURRENT_VERSION);
    assert_eq!(res.body["available"], false);

    // UPDATE_CHECK=off: nothing is fetched.
    let off = send_with_state(
        with_feed(pool.clone(), None),
        check("/api/app/update", Some(&cookie)),
    )
    .await;
    assert_eq!(off.status, StatusCode::OK);
    assert_eq!(off.body["disabled"], true);
    assert_eq!(off.body["available"], false);
    assert_eq!(off.body["current"], CURRENT_VERSION);
}

#[sqlx::test]
async fn an_unreachable_feed_says_so(pool: PgPool) {
    let cookie = sign_in(&pool, "offline@example.com", "offline").await;
    // Nothing listens on port 9 of the loopback.
    let res = send_with_state(
        with_feed(pool.clone(), Some("http://127.0.0.1:9/latest".into())),
        check("/api/app/update", Some(&cookie)),
    )
    .await;
    assert_eq!(res.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        res.body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("check for updates"),
        "{:?}",
        res.body
    );
}
