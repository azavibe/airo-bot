// Security checks (the review of November 2026): a bot's key never acts
// as an admin or shares an office, and email-sending endpoints can't be
// used to flood an inbox.
mod common;

use airobot::auth::secret;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use common::{
    authed_json_request, bearer_get_request, bearer_json_request, form_request, get_request, send,
    session_cookie,
};
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

/// A bot for `owner` and its raw key.
async fn bot_key(pool: &PgPool, email: &str) -> String {
    let owner: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(pool)
        .await
        .expect("owner");
    let id = Uuid::new_v4();
    let (raw_key, raw_secret, prefix) = airobot::auth::generate_agent_key(id);
    let hash = secret::hash_secret(&raw_secret).expect("hash");
    sqlx::query(
        "INSERT INTO agents (id, user_id, name, handle, key_hash, key_prefix)
         VALUES ($1, $2, 'Helper', $3, $4, $5)",
    )
    .bind(id)
    .bind(owner)
    .bind(format!("helper-{}", &id.simple().to_string()[..6]))
    .bind(hash)
    .bind(prefix)
    .execute(pool)
    .await
    .expect("bot");
    raw_key
}

#[sqlx::test]
async fn a_bots_key_never_acts_as_an_admin_or_shares_the_office(pool: PgPool) {
    // The first account is the platform's admin.
    let cookie = sign_in(&pool, "admin@example.com", "admin").await;
    let key = bot_key(&pool, "admin@example.com").await;
    let mut as_person = get_request("/admin", Some(&cookie));
    as_person
        .headers_mut()
        .insert("accept", "application/json".parse().expect("accept"));
    assert_eq!(send(pool.clone(), as_person).await.status, StatusCode::OK);
    assert_eq!(
        send(pool.clone(), bearer_get_request("/admin", &key))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    sign_in(&pool, "member@example.com", "member").await;
    let member: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE email = 'member@example.com'")
            .fetch_one(&pool)
            .await
            .expect("member");
    let res = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            &format!("/admin/users/{member}/admin"),
            json!({}),
            &key,
        ),
    )
    .await;
    assert_ne!(
        res.status,
        StatusCode::OK,
        "a bot can't make someone an admin"
    );
    let promoted: bool = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
        .bind(member)
        .fetch_one(&pool)
        .await
        .expect("member");
    assert!(!promoted);

    // Sharing the office and connecting outside channels need the person.
    let (status, made) = {
        let res = send(
            pool.clone(),
            authed_json_request(
                "POST",
                "/api/offices",
                json!({ "name": "Shop" }),
                Some(&cookie),
            ),
        )
        .await;
        (res.status, res.body)
    };
    assert_eq!(status, StatusCode::CREATED);
    let office = made["office"]["id"].as_str().expect("office").to_string();
    for (uri, body) in [
        (
            format!("/api/offices/{office}/invites"),
            json!({ "email": "friend@example.com" }),
        ),
        (
            format!("/api/offices/{office}/telegram"),
            json!({ "token": "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw" }),
        ),
        (
            format!("/api/offices/{office}/slack"),
            json!({ "bot_token": "xoxb-1", "signing_secret": "abcdef0123456789" }),
        ),
        (
            format!("/api/offices/{office}/phone"),
            json!({ "forward_to": "+15551234567" }),
        ),
    ] {
        let res = send(pool.clone(), bearer_json_request("POST", &uri, body, &key)).await;
        assert_eq!(res.status, StatusCode::FORBIDDEN, "{uri}: {:?}", res.body);
    }
}

fn signin_from(ip: &str, email: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/signin")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .header("x-forwarded-for", ip)
        .body(Body::from(format!("email={}", email.replace('@', "%40"))))
        .expect("request")
}

#[sqlx::test]
async fn one_inbox_gets_a_few_sign_in_links_an_hour(pool: PgPool) {
    // From many addresses, as someone trying to flood an inbox would.
    let mut statuses = Vec::new();
    for i in 0..8 {
        let res = send(
            pool.clone(),
            signin_from(&format!("203.0.113.{i}"), "victim@example.com"),
        )
        .await;
        statuses.push(res.status);
    }
    // Every answer looks the same: nothing says the cap was hit.
    assert!(statuses.iter().all(|s| *s == statuses[0]), "{statuses:?}");
    assert!(
        statuses[0].is_success() || statuses[0].is_redirection(),
        "{statuses:?}"
    );
    let sent: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM email_tokens WHERE email = 'victim@example.com'")
            .fetch_one(&pool)
            .await
            .expect("tokens");
    assert_eq!(sent, 5);
    // Another inbox is unaffected.
    send(
        pool.clone(),
        signin_from("203.0.113.99", "someone@example.com"),
    )
    .await;
    let other: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM email_tokens WHERE email = 'someone@example.com'")
            .fetch_one(&pool)
            .await
            .expect("tokens");
    assert_eq!(other, 1);
}

#[sqlx::test]
async fn an_office_sends_a_limited_number_of_invites_an_hour(pool: PgPool) {
    let cookie = sign_in(&pool, "inviter@example.com", "inviter").await;
    let res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/offices",
            json!({ "name": "Studio" }),
            Some(&cookie),
        ),
    )
    .await;
    let office = res.body["office"]["id"]
        .as_str()
        .expect("office")
        .to_string();
    let uri = format!("/api/offices/{office}/invites");
    // Re-inviting the same address replaces the open invite, but each one is
    // an email, so they all count.
    for i in 0..20 {
        let res = send(
            pool.clone(),
            authed_json_request(
                "POST",
                &uri,
                json!({ "email": format!("guest{}@example.com", i % 3) }),
                Some(&cookie),
            ),
        )
        .await;
        assert_eq!(
            res.status,
            StatusCode::CREATED,
            "invite {i}: {:?}",
            res.body
        );
    }
    let res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &uri,
            json!({ "email": "one.more@example.com" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    let error: &Value = &res.body["error"];
    assert!(
        error
            .as_str()
            .unwrap_or_default()
            .contains("try again in an hour"),
        "{error}"
    );
}
