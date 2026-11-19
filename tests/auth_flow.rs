mod common;

fn browser_request(
    mut request: axum::http::Request<axum::body::Body>,
) -> axum::http::Request<axum::body::Body> {
    request
        .headers_mut()
        .insert("accept", "text/html".parse().expect("header"));
    request
}

#[sqlx::test]
async fn browser_signup_and_returning_login_open_account(pool: PgPool) {
    let token = insert_email_token(&pool, "journey@example.com").await;
    let response = send(
        pool.clone(),
        browser_request(get_request(&format!("/auth/callback?token={token}"), None)),
    )
    .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.headers["location"], "/onboarding/handle");
    let cookie = session_cookie(&response.headers).expect("signed-in cookie");
    let setup = send(
        pool.clone(),
        browser_request(get_request("/onboarding/handle", Some(&cookie))),
    )
    .await;
    assert_eq!(setup.status, StatusCode::OK);
    assert!(setup.text.contains("Your email is verified"));
    let finish = send(
        pool.clone(),
        browser_request(form_request(
            "POST",
            "/onboarding/handle",
            &[("handle", "journey")],
            Some(&cookie),
        )),
    )
    .await;
    assert_eq!(finish.status, StatusCode::SEE_OTHER);
    assert_eq!(finish.headers["location"], "/inbox");
    let account = send(
        pool.clone(),
        browser_request(get_request("/inbox", Some(&cookie))),
    )
    .await;
    assert_eq!(account.status, StatusCode::OK);
    // A new account opens on its rooms: the built-in Assistant's is there.
    assert!(account.text.contains("#assistant"), "{}", account.text);
    for path in ["/signin", "/onboarding/handle"] {
        let response = send(
            pool.clone(),
            browser_request(get_request(path, Some(&cookie))),
        )
        .await;
        assert_eq!(response.headers["location"], "/inbox");
    }
    let token = insert_email_token(&pool, "journey@example.com").await;
    let returning = send(
        pool.clone(),
        browser_request(get_request(&format!("/auth/callback?token={token}"), None)),
    )
    .await;
    assert_eq!(returning.headers["location"], "/inbox");
    let new_cookie = session_cookie(&returning.headers).expect("new session");
    let logout = send(
        pool,
        browser_request(common::post_request("/auth/logout", Some(&new_cookie))),
    )
    .await;
    assert_eq!(logout.headers["location"], "/signin");
}

#[sqlx::test]
async fn browser_invalid_link_offers_a_new_link(pool: PgPool) {
    let response = send(
        pool,
        browser_request(get_request("/auth/callback?token=invalid", None)),
    )
    .await;
    assert!(response.text.contains("action=\"/signin\""));
    assert!(response.text.contains("already used"));
    assert!(!response.headers.contains_key("set-cookie"));
}

#[sqlx::test]
async fn browser_check_email_has_next_actions(pool: PgPool) {
    let response = send(
        pool,
        browser_request(form_request(
            "POST",
            "/signin",
            &[("email", "next@example.com")],
            None,
        )),
    )
    .await;
    assert!(response.text.contains("expires in 3 hours"));
    assert!(response.text.contains("href=\"/signin\""));
    assert!(response.text.contains("href=\"/inbox\""));
}

#[sqlx::test]
async fn concurrent_callbacks_create_only_one_session(pool: PgPool) {
    let token = insert_email_token(&pool, "race@example.com").await;
    let url = format!("/auth/callback?token={token}");
    let (a, b) = tokio::join!(
        send(pool.clone(), get_request(&url, None)),
        send(pool.clone(), get_request(&url, None))
    );
    assert_eq!(
        usize::from(session_cookie(&a.headers).is_some())
            + usize::from(session_cookie(&b.headers).is_some()),
        1
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&pool)
        .await
        .expect("sessions");
    assert_eq!(count, 1);
}

#[sqlx::test]
async fn concurrent_signin_retries_create_one_token(pool: PgPool) {
    let request = || {
        form_request(
            "POST",
            "/signin",
            &[
                ("email", "retry@example.com"),
                ("idempotency_key", "retry-1"),
            ],
            None,
        )
    };
    let (a, b) = tokio::join!(send(pool.clone(), request()), send(pool.clone(), request()));
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(b.status, StatusCode::OK);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM email_tokens")
        .fetch_one(&pool)
        .await
        .expect("tokens");
    assert_eq!(count, 1);
}

#[sqlx::test]
async fn invented_bearer_tokens_do_not_bypass_signin_limit(pool: PgPool) {
    use tower::ServiceExt;
    let app = airobot::app(common::state(pool));
    for i in 0..4 {
        let mut req = form_request("POST", "/signin", &[("email", "invalid")], None);
        req.headers_mut().insert(
            "authorization",
            format!("Bearer invented-{i}").parse().expect("header"),
        );
        let response = app.clone().oneshot(req).await.expect("response");
        assert_eq!(
            response.status(),
            if i == 3 {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::BAD_REQUEST
            }
        );
    }
}

#[sqlx::test]
async fn smtp_failure_returns_retryable_error_for_browser_and_json(pool: PgPool) {
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tower::ServiceExt;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    // Keep the port reserved and reject connections without sending mail.
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (stream, _) = listener.accept().expect("connection");
            drop(stream);
        }
    });
    for html in [false, true] {
        let mut state = common::state(pool.clone());
        state.mailer = Arc::new(airobot::email::Mailer::new(Some(
            &airobot::config::SmtpConfig {
                host: "localhost".into(),
                port,
                username: "test".into(),
                password: "test".into(),
                from: "Air <test@example.com>".into(),
            },
        )));
        let mut request = form_request("POST", "/signin", &[("email", "test@example.com")], None);
        if html {
            request
                .headers_mut()
                .insert("accept", "text/html".parse().expect("header"));
        }
        let response = airobot::app(state)
            .oneshot(request)
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(body.contains("Please try again shortly"));
        assert!(!body.contains("Check your email"));
        assert!(!body.contains("Connection error"));
    }
    server.join().expect("server");
}

use airobot::auth::secret;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::{form_request, get_request, send, session_cookie};
use sqlx::PgPool;
use uuid::Uuid;

/// Simulates "a magic link was requested": inserts the email_tokens row the
/// same way the handler does, and returns the raw token that would have
/// been emailed. There's no way to recover this from the DB afterward (only
/// its argon2 hash is stored), so tests that need a valid link build their
/// own instead of going through the mailer.
async fn insert_email_token(pool: &PgPool, email: &str) -> String {
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
    secret::encode_keyed_secret("", id, &raw_secret)
}

async fn sign_in(pool: &PgPool, email: &str, handle: &str) -> String {
    let raw_token = insert_email_token(pool, email).await;
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

#[sqlx::test]
async fn login_rejects_invalid_email(pool: PgPool) {
    let res = send(
        pool,
        form_request("POST", "/signin", &[("email", "not-an-email")], None),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn login_accepts_valid_email_and_creates_a_token(pool: PgPool) {
    let res = send(
        pool.clone(),
        form_request("POST", "/signin", &[("email", "new@example.com")], None),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(res.text.contains("expires in 3 hours"));

    let lifetime_is_three_hours: bool = sqlx::query_scalar(
        "SELECT expires_at BETWEEN created_at + interval '2 hours 59 minutes'
             AND created_at + interval '3 hours 1 minute'
         FROM email_tokens WHERE email = $1",
    )
    .bind("new@example.com")
    .fetch_one(&pool)
    .await
    .expect("token lifetime");
    assert!(lifetime_is_three_hours);
}

#[sqlx::test]
async fn login_response_is_the_same_for_unknown_emails(pool: PgPool) {
    // No enumeration signal: same response whether or not the email exists.
    let res = send(
        pool,
        form_request("POST", "/signin", &[("email", "nobody@example.com")], None),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
}

#[sqlx::test]
async fn callback_rejects_garbage_token(pool: PgPool) {
    let res = send(
        pool,
        get_request("/auth/callback?token=not-a-real-token", None),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn callback_rejects_expired_token(pool: PgPool) {
    let id = Uuid::new_v4();
    let raw_secret = secret::random_hex_token(32);
    let token_hash = secret::hash_secret(&raw_secret).expect("hash");
    sqlx::query(
        "INSERT INTO email_tokens (id, email, token_hash, purpose, expires_at)
         VALUES ($1, 'expired@example.com', $2, 'login', now() - interval '1 minute')",
    )
    .bind(id)
    .bind(&token_hash)
    .execute(&pool)
    .await
    .expect("insert");
    let raw_token = secret::encode_keyed_secret("", id, &raw_secret);

    let res = send(
        pool,
        get_request(&format!("/auth/callback?token={raw_token}"), None),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn callback_creates_user_and_asks_for_a_handle(pool: PgPool) {
    let raw_token = insert_email_token(&pool, "person@example.com").await;
    let res = send(
        pool.clone(),
        get_request(&format!("/auth/callback?token={raw_token}"), None),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.body["next"], "/onboarding/handle");
    assert!(session_cookie(&res.headers).is_some());

    let handle: Option<String> = sqlx::query_scalar("SELECT handle FROM users WHERE email = $1")
        .bind("person@example.com")
        .fetch_one(&pool)
        .await
        .expect("fetch user");
    assert!(handle.is_none());
}

#[sqlx::test]
async fn callback_token_is_single_use(pool: PgPool) {
    let raw_token = insert_email_token(&pool, "once@example.com").await;
    let uri = format!("/auth/callback?token={raw_token}");
    let first = send(pool.clone(), get_request(&uri, None)).await;
    assert_eq!(first.status, StatusCode::OK);

    let second = send(pool, get_request(&uri, None)).await;
    assert_eq!(second.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn callback_signing_in_again_does_not_duplicate_the_user(pool: PgPool) {
    let first_token = insert_email_token(&pool, "again@example.com").await;
    send(
        pool.clone(),
        get_request(&format!("/auth/callback?token={first_token}"), None),
    )
    .await;

    let second_token = insert_email_token(&pool, "again@example.com").await;
    let res = send(
        pool.clone(),
        get_request(&format!("/auth/callback?token={second_token}"), None),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE email = $1")
        .bind("again@example.com")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 1);
}

#[sqlx::test]
async fn onboarding_handle_requires_auth(pool: PgPool) {
    let res = send(pool, get_request("/onboarding/handle", None)).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn browser_authentication_failures_redirect_to_signin(pool: PgPool) {
    for path in [
        "/onboarding/handle",
        "/post",
        "/inbox",
        "/settings/agents",
        "/notifs",
    ] {
        let mut request = get_request(path, None);
        request
            .headers_mut()
            .insert("accept", "text/html".parse().expect("accept header"));
        let response = send(pool.clone(), request).await;
        assert_eq!(response.status, StatusCode::SEE_OTHER, "path: {path}");
        assert_eq!(
            response
                .headers
                .get("location")
                .and_then(|value| value.to_str().ok()),
            Some("/signin"),
            "path: {path}"
        );
    }
}

#[sqlx::test]
async fn full_sign_up_sets_a_handle(pool: PgPool) {
    let cookie = sign_in(&pool, "signup@example.com", "newuser").await;
    assert!(!cookie.is_empty());

    let handle: Option<String> = sqlx::query_scalar("SELECT handle FROM users WHERE email = $1")
        .bind("signup@example.com")
        .fetch_one(&pool)
        .await
        .expect("fetch user");
    assert_eq!(handle.as_deref(), Some("newuser"));
}

#[sqlx::test]
async fn onboarding_handle_rejects_invalid_handles(pool: PgPool) {
    let raw_token = insert_email_token(&pool, "badhandle@example.com").await;
    let callback_res = send(
        pool.clone(),
        get_request(&format!("/auth/callback?token={raw_token}"), None),
    )
    .await;
    let cookie = session_cookie(&callback_res.headers).expect("cookie");

    let res = send(
        pool,
        form_request(
            "POST",
            "/onboarding/handle",
            &[("handle", "a")],
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn onboarding_handle_rejects_a_taken_handle(pool: PgPool) {
    sign_in(&pool, "first@example.com", "taken").await;

    let raw_token = insert_email_token(&pool, "second@example.com").await;
    let callback_res = send(
        pool.clone(),
        get_request(&format!("/auth/callback?token={raw_token}"), None),
    )
    .await;
    let cookie = session_cookie(&callback_res.headers).expect("cookie");

    let res = send(
        pool,
        form_request(
            "POST",
            "/onboarding/handle",
            &[("handle", "taken")],
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CONFLICT);
}

#[sqlx::test]
async fn logout_revokes_the_session(pool: PgPool) {
    let cookie = sign_in(&pool, "logout@example.com", "logoutuser").await;

    let logout_res = send(
        pool.clone(),
        common::post_request("/auth/logout", Some(&cookie)),
    )
    .await;
    assert_eq!(logout_res.status, StatusCode::OK);

    let res = send(pool, get_request("/onboarding/handle", Some(&cookie))).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}
