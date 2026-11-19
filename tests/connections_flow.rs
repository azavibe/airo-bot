//! Connections: the catalog, OAuth sign-in for Google, Stripe and Shopify
//! (against a fake provider on a local port), actions bots run through a
//! connection, webhooks in and out, and who may touch what. Real router,
//! real database.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use airobot::auth::secret;
use airobot::connectors::{ConnectorApps, OAuthApp};
use airobot::state::AppState;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use common::{authed_json_request, form_request, get_request, send_with_state, session_cookie};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

// ---------- A fake provider ----------

#[derive(Default)]
struct Fake {
    drafts: AtomicUsize,
    reject_refresh: AtomicBool,
    refreshes: AtomicUsize,
    outbound: Mutex<Vec<Value>>,
    revoked: AtomicUsize,
}

fn bearer(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

async fn google_token(
    State(fake): State<Arc<Fake>>,
    Form(form): Form<std::collections::HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    match form.get("grant_type").map(String::as_str) {
        Some("authorization_code")
            if form.get("code").map(String::as_str) == Some("good-code")
                && form.get("code_verifier").is_some_and(|v| v.len() == 64) =>
        {
            (
                StatusCode::OK,
                Json(json!({
                    "access_token": "g-access",
                    "expires_in": 3600,
                    "refresh_token": "g-refresh",
                    "scope": "openid email https://www.googleapis.com/auth/gmail.readonly https://www.googleapis.com/auth/gmail.compose https://www.googleapis.com/auth/calendar.events",
                    "token_type": "Bearer"
                })),
            )
        }
        Some("refresh_token") if !fake.reject_refresh.load(Ordering::SeqCst) => {
            fake.refreshes.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                Json(json!({ "access_token": "g-access-2", "expires_in": 3600 })),
            )
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        ),
    }
}

fn google_ok(headers: &HeaderMap) -> bool {
    matches!(
        bearer(headers).as_str(),
        "Bearer g-access" | "Bearer g-access-2"
    )
}

async fn google_userinfo(headers: HeaderMap) -> (StatusCode, Json<Value>) {
    if !google_ok(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({})));
    }
    (
        StatusCode::OK,
        Json(json!({ "sub": "g-123", "email": "owner@example.com" })),
    )
}

async fn gmail_list(headers: HeaderMap) -> (StatusCode, Json<Value>) {
    if !google_ok(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({})));
    }
    (
        StatusCode::OK,
        Json(json!({ "messages": [{ "id": "m1", "threadId": "t1" }] })),
    )
}

async fn gmail_get(headers: HeaderMap, Path(id): Path<String>) -> (StatusCode, Json<Value>) {
    if !google_ok(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({})));
    }
    (
        StatusCode::OK,
        Json(json!({
            "id": id,
            "threadId": "t1",
            "snippet": "Can you quote the roof?",
            "labelIds": ["INBOX", "UNREAD"],
            "payload": { "headers": [
                { "name": "From", "value": "Client <client@example.com>" },
                { "name": "Subject", "value": "Quote" },
                { "name": "Date", "value": "Mon, 16 Nov 2026 09:00:00 +0000" }
            ]}
        })),
    )
}

async fn gmail_draft(
    State(fake): State<Arc<Fake>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !google_ok(&headers) || body["message"]["raw"].as_str().is_none() {
        return (StatusCode::BAD_REQUEST, Json(json!({})));
    }
    let n = fake.drafts.fetch_add(1, Ordering::SeqCst) + 1;
    (
        StatusCode::OK,
        Json(json!({ "id": format!("d{n}"), "message": { "id": format!("m{n}") } })),
    )
}

async fn revoke(State(fake): State<Arc<Fake>>) -> Json<Value> {
    fake.revoked.fetch_add(1, Ordering::SeqCst);
    Json(json!({}))
}

async fn stripe_token(
    headers: HeaderMap,
    Form(form): Form<std::collections::HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    // Basic auth with the platform secret key.
    if !bearer(&headers).starts_with("Basic ")
        || form.get("code").map(String::as_str) != Some("good-code")
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        );
    }
    (
        StatusCode::OK,
        Json(
            json!({ "stripe_user_id": "acct_1", "scope": "read_write", "livemode": false, "token_type": "bearer" }),
        ),
    )
}

async fn stripe_account(headers: HeaderMap) -> Json<Value> {
    assert_eq!(bearer(&headers), "Bearer sk_test_platform");
    Json(json!({ "id": "acct_1", "settings": { "dashboard": { "display_name": "Acme Roofing" } } }))
}

async fn stripe_balance(headers: HeaderMap) -> (StatusCode, Json<Value>) {
    let account = headers.get("stripe-account").and_then(|v| v.to_str().ok());
    // The platform key on behalf of acct_1, or a key the owner pasted.
    let platform = bearer(&headers) == "Bearer sk_test_platform" && account == Some("acct_1");
    let pasted = bearer(&headers) == "Bearer rk_test_good" && account.is_none();
    if !platform && !pasted {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": { "type": "invalid_request_error" } })),
        );
    }
    (
        StatusCode::OK,
        Json(json!({ "available": [{ "amount": 125000, "currency": "usd" }], "pending": [] })),
    )
}

async fn shopify_token(
    Form(form): Form<std::collections::HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    if form.get("code").map(String::as_str) != Some("good-code")
        || form.get("expiring").map(String::as_str) != Some("1")
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_request" })),
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "access_token": "s-access",
            "scope": "read_orders,read_products",
            "expires_in": 3600,
            "refresh_token": "s-refresh",
            "refresh_token_expires_in": 7776000
        })),
    )
}

async fn shopify_graphql(headers: HeaderMap, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    if headers
        .get("x-shopify-access-token")
        .and_then(|v| v.to_str().ok())
        .filter(|t| *t == "s-access" || *t == "shpat_good")
        .is_none()
    {
        return (StatusCode::UNAUTHORIZED, Json(json!({})));
    }
    let query = body["query"].as_str().unwrap_or_default();
    let data = if query.contains("orders") {
        json!({ "orders": { "nodes": [{
            "id": "gid://shopify/Order/1", "name": "#1001", "createdAt": "2026-11-16T10:00:00Z",
            "displayFinancialStatus": "PAID", "displayFulfillmentStatus": "UNFULFILLED",
            "totalPriceSet": { "shopMoney": { "amount": "89.00", "currencyCode": "USD" } }
        }]}})
    } else {
        json!({ "shop": { "name": "Acme Store" } })
    };
    (StatusCode::OK, Json(json!({ "data": data })))
}

async fn outbound(State(fake): State<Arc<Fake>>, Json(body): Json<Value>) -> StatusCode {
    fake.outbound.lock().expect("lock").push(body);
    StatusCode::OK
}

/// Starts the fake provider and returns its origin.
async fn fake_provider(fake: Arc<Fake>) -> String {
    let app = Router::new()
        .route("/oauth2.googleapis.com/token", post(google_token))
        .route("/oauth2.googleapis.com/revoke", post(revoke))
        .route(
            "/openidconnect.googleapis.com/v1/userinfo",
            get(google_userinfo),
        )
        .route(
            "/gmail.googleapis.com/gmail/v1/users/me/messages",
            get(gmail_list),
        )
        .route(
            "/gmail.googleapis.com/gmail/v1/users/me/messages/{id}",
            get(gmail_get),
        )
        .route(
            "/gmail.googleapis.com/gmail/v1/users/me/drafts",
            post(gmail_draft),
        )
        .route("/connect.stripe.com/oauth/token", post(stripe_token))
        .route("/connect.stripe.com/oauth/deauthorize", post(revoke))
        .route("/api.stripe.com/v1/accounts/{id}", get(stripe_account))
        .route("/api.stripe.com/v1/balance", get(stripe_balance))
        // A restricted key may not read the account.
        .route(
            "/api.stripe.com/v1/account",
            get(|| async { (StatusCode::FORBIDDEN, Json(json!({}))) }),
        )
        .route(
            "/acme.myshopify.com/admin/oauth/access_token",
            post(shopify_token),
        )
        .route(
            "/acme.myshopify.com/admin/api/2026-10/graphql.json",
            post(shopify_graphql),
        )
        .route("/outbound", post(outbound))
        .with_state(fake);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    format!("http://{addr}")
}

fn app(id: &str, secret: &str) -> Option<OAuthApp> {
    Some(OAuthApp {
        client_id: id.to_string(),
        client_secret: secret.to_string(),
    })
}

fn state_with_providers(pool: PgPool, origin: &str) -> AppState {
    let mut s = common::state(pool);
    let mut config = (*s.config).clone();
    config.connectors = ConnectorApps {
        google: app("google-client", "google-secret"),
        stripe: app("ca_test", "sk_test_platform"),
        shopify: app("shopify-client", "shopify-secret"),
        test_origin: Some(origin.to_string()),
    };
    s.config = Arc::new(config);
    s
}

// ---------- Helpers ----------

async fn sign_in(state: &AppState, email: &str, handle: &str) -> String {
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
    .execute(&state.pool)
    .await
    .expect("insert email token");
    let raw_token = secret::encode_keyed_secret("", id, &raw_secret);
    let res = send_with_state(
        state.clone(),
        get_request(&format!("/auth/callback?token={raw_token}"), None),
    )
    .await;
    let cookie = session_cookie(&res.headers).expect("session cookie");
    let set = send_with_state(
        state.clone(),
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
    state: &AppState,
    method: &str,
    uri: &str,
    body: Value,
    cookie: &str,
) -> common::TestResponse {
    send_with_state(
        state.clone(),
        authed_json_request(method, uri, body, Some(cookie)),
    )
    .await
}

/// Starts a sign-in and returns the `state` the provider would hand back.
async fn start(state: &AppState, cookie: &str, provider: &str, body: Value) -> (String, String) {
    let res = call(
        state,
        "POST",
        &format!("/api/connections/oauth/{provider}/start"),
        body,
        cookie,
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    let url = res.body["authorize_url"]
        .as_str()
        .expect("authorize_url")
        .to_string();
    let parsed = reqwest::Url::parse(&url).expect("url");
    let oauth_state = parsed
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .expect("state");
    (url, oauth_state)
}

async fn callback(state: &AppState, cookie: &str, provider: &str, query: &str) -> String {
    let res = send_with_state(
        state.clone(),
        get_request(
            &format!("/api/connections/oauth/{provider}/callback?{query}"),
            Some(cookie),
        ),
    )
    .await;
    assert!(res.status.is_redirection(), "{:?} {}", res.status, res.text);
    res.headers
        .get("location")
        .and_then(|v| v.to_str().ok())
        .expect("location")
        .to_string()
}

async fn connections(state: &AppState, cookie: &str) -> Value {
    let res = call(state, "GET", "/api/connections", json!({}), cookie).await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    res.body
}

fn enc(value: &str) -> String {
    reqwest::Url::parse_with_params("http://x/", &[("v", value)])
        .expect("url")
        .query()
        .expect("query")
        .trim_start_matches("v=")
        .to_string()
}

// ---------- Tests ----------

#[sqlx::test]
async fn catalog_lists_every_provider_and_unset_ones_cannot_start(pool: PgPool) {
    let state = common::state(pool);
    let cookie = sign_in(&state, "cat@example.com", "cat").await;

    let body = connections(&state, &cookie).await;
    let catalog = body["catalog"].as_array().expect("catalog");
    let ids: Vec<&str> = catalog.iter().filter_map(|c| c["id"].as_str()).collect();
    assert_eq!(ids, vec!["google", "stripe", "shopify", "webhook"]);
    let available =
        |id: &str| catalog.iter().find(|c| c["id"] == id).expect("entry")["available"].clone();
    assert_eq!(available("google"), json!(false));
    assert_eq!(available("webhook"), json!(true));
    assert!(catalog[0]["actions"]
        .as_array()
        .is_some_and(|a| a.iter().any(|x| x["id"] == "gmail.draft")));
    assert_eq!(body["connections"], json!([]));

    let res = call(
        &state,
        "POST",
        "/api/connections/oauth/google/start",
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(res.status, StatusCode::SERVICE_UNAVAILABLE);
    let res = call(
        &state,
        "POST",
        "/api/connections/oauth/nope/start",
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);

    // Signed-out callers get nothing.
    let res = send_with_state(state.clone(), get_request("/api/connections", None)).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn google_connects_runs_actions_refreshes_and_disconnects(pool: PgPool) {
    let fake = Arc::new(Fake::default());
    let origin = fake_provider(fake.clone()).await;
    let state = state_with_providers(pool, &origin);
    let cookie = sign_in(&state, "owner@example.com", "owner").await;

    let bad = call(
        &state,
        "POST",
        "/api/connections/oauth/google/start",
        json!({ "products": ["fax"] }),
        &cookie,
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    let (url, oauth_state) = start(
        &state,
        &cookie,
        "google",
        json!({ "products": ["gmail", "calendar"] }),
    )
    .await;
    assert!(
        url.starts_with(&format!("{origin}/accounts.google.com/o/oauth2/v2/auth?")),
        "{url}"
    );
    assert!(url.contains("code_challenge_method=S256"));
    assert!(url.contains("access_type=offline"));
    assert!(url.contains("gmail.compose"));
    assert!(!url.contains("spreadsheets"));

    // Someone else cannot finish this person's sign-in.
    let intruder = sign_in(&state, "intruder@example.com", "intruder").await;
    let location = callback(
        &state,
        &intruder,
        "google",
        &format!("code=good-code&state={}", enc(&oauth_state)),
    )
    .await;
    assert!(location.contains("error=expired"), "{location}");

    // That attempt used the sign-in up, so start again.
    let (_, oauth_state) = start(
        &state,
        &cookie,
        "google",
        json!({ "products": ["gmail", "calendar"] }),
    )
    .await;
    let location = callback(
        &state,
        &cookie,
        "google",
        &format!("code=good-code&state={}", enc(&oauth_state)),
    )
    .await;
    assert_eq!(location, "/settings/connections?connected=google");

    // A state works once.
    let again = callback(
        &state,
        &cookie,
        "google",
        &format!("code=good-code&state={}", enc(&oauth_state)),
    )
    .await;
    assert!(again.contains("error=expired"));

    let body = connections(&state, &cookie).await;
    let list = body["connections"].as_array().expect("connections");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["label"], "owner@example.com");
    assert_eq!(list[0]["products"], json!(["gmail", "calendar"]));
    assert!(list[0].get("access_token_enc").is_none());
    let id = list[0]["id"].as_str().expect("id").to_string();

    // Tokens are stored encrypted, never in plaintext.
    let (access, refresh): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT access_token_enc, refresh_token_enc FROM connections WHERE id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&state.pool)
    .await
    .expect("row");
    assert!(access
        .as_deref()
        .is_some_and(|a| a.starts_with("enc:v1:") && !a.contains("g-access")));
    assert!(refresh.as_deref().is_some_and(|r| r.starts_with("enc:v1:")));

    let search = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/actions/gmail.search"),
        json!({ "input": { "query": "is:unread" } }),
        &cookie,
    )
    .await;
    assert_eq!(search.status, StatusCode::OK, "{:?}", search.body);
    assert_eq!(search.body["result"]["messages"][0]["subject"], "Quote");
    assert_eq!(search.body["result"]["messages"][0]["unread"], true);

    // A retried draft with the same key is made once.
    let draft = json!({ "input": { "to": "client@example.com", "subject": "Your quote", "body": "Hi" }, "idempotency_key": "draft-1" });
    let first = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/actions/gmail.draft"),
        draft.clone(),
        &cookie,
    )
    .await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.body);
    let second = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/actions/gmail.draft"),
        draft,
        &cookie,
    )
    .await;
    assert_eq!(second.status, StatusCode::OK);
    assert_eq!(second.body["replayed"], true);
    assert_eq!(second.body["result"], first.body["result"]);
    assert_eq!(fake.drafts.load(Ordering::SeqCst), 1);

    // Header injection is refused before anything is sent.
    let injected = call(&state, "POST", &format!("/api/connections/{id}/actions/gmail.draft"), json!({ "input": { "to": "a@example.com\r\nBcc: x@example.com", "subject": "s", "body": "b" } }), &cookie).await;
    assert_eq!(injected.status, StatusCode::BAD_REQUEST);

    // Sheets was not granted.
    let sheets = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/actions/sheets.read"),
        json!({ "input": { "spreadsheet_id": "x", "range": "A1" } }),
        &cookie,
    )
    .await;
    assert_eq!(sheets.status, StatusCode::BAD_REQUEST);
    let unknown = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/actions/stripe.balance"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);

    // An expired token is refreshed before use.
    sqlx::query(
        "UPDATE connections SET token_expires_at = now() - interval '1 minute' WHERE id = $1::uuid",
    )
    .bind(&id)
    .execute(&state.pool)
    .await
    .expect("expire");
    let refreshed = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/actions/gmail.search"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(refreshed.status, StatusCode::OK, "{:?}", refreshed.body);
    assert_eq!(fake.refreshes.load(Ordering::SeqCst), 1);

    // A refused refresh asks for reconnecting.
    fake.reject_refresh.store(true, Ordering::SeqCst);
    sqlx::query(
        "UPDATE connections SET token_expires_at = now() - interval '1 minute' WHERE id = $1::uuid",
    )
    .bind(&id)
    .execute(&state.pool)
    .await
    .expect("expire");
    let gone = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/actions/gmail.search"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(gone.status, StatusCode::CONFLICT, "{:?}", gone.body);
    let body = connections(&state, &cookie).await;
    assert_eq!(body["connections"][0]["status"], "needs_reconnect");

    // Connecting the same account again repairs it in place.
    fake.reject_refresh.store(false, Ordering::SeqCst);
    let (_, oauth_state) = start(&state, &cookie, "google", json!({})).await;
    callback(
        &state,
        &cookie,
        "google",
        &format!("code=good-code&state={}", enc(&oauth_state)),
    )
    .await;
    let body = connections(&state, &cookie).await;
    assert_eq!(body["connections"].as_array().map(Vec::len), Some(1));
    assert_eq!(body["connections"][0]["status"], "active");

    // Others see nothing and cannot use or remove it.
    let theirs = connections(&state, &intruder).await;
    assert_eq!(theirs["connections"], json!([]));
    let use_it = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/actions/gmail.search"),
        json!({}),
        &intruder,
    )
    .await;
    assert_eq!(use_it.status, StatusCode::NOT_FOUND);
    let remove = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/delete"),
        json!({}),
        &intruder,
    )
    .await;
    assert_eq!(remove.status, StatusCode::NOT_FOUND);

    let removed = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/delete"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(removed.status, StatusCode::OK, "{:?}", removed.body);
    assert_eq!(connections(&state, &cookie).await["connections"], json!([]));
    let (deleted, token): (bool, Option<String>) = sqlx::query_as(
        "SELECT deleted_at IS NOT NULL, access_token_enc FROM connections WHERE id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&state.pool)
    .await
    .expect("row");
    assert!(deleted, "soft-deleted, not removed");
    assert!(token.is_none(), "tokens are wiped");
    let after = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/actions/gmail.search"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(after.status, StatusCode::NOT_FOUND);

    // Every action was logged.
    let logged: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM connection_actions WHERE connection_id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&state.pool)
    .await
    .expect("count");
    assert!(logged >= 4, "{logged}");
}

#[sqlx::test]
async fn cancelled_google_sign_in_returns_to_connections(pool: PgPool) {
    let fake = Arc::new(Fake::default());
    let origin = fake_provider(fake).await;
    let state = state_with_providers(pool, &origin);
    let cookie = sign_in(&state, "c@example.com", "canceller").await;
    let (_, oauth_state) = start(&state, &cookie, "google", json!({})).await;
    let location = callback(
        &state,
        &cookie,
        "google",
        &format!("error=access_denied&state={}", enc(&oauth_state)),
    )
    .await;
    assert!(location.contains("error=cancelled"), "{location}");
    let garbage = callback(
        &state,
        &cookie,
        "google",
        "code=good-code&state=not-a-state",
    )
    .await;
    assert!(garbage.contains("error=expired"));
    assert_eq!(connections(&state, &cookie).await["connections"], json!([]));
}

#[sqlx::test]
async fn stripe_connects_and_reads_through_the_connected_account(pool: PgPool) {
    let fake = Arc::new(Fake::default());
    let origin = fake_provider(fake).await;
    let state = state_with_providers(pool, &origin);
    let cookie = sign_in(&state, "s@example.com", "stripey").await;

    let (url, oauth_state) = start(
        &state,
        &cookie,
        "stripe",
        json!({ "idempotency_key": "start-1" }),
    )
    .await;
    assert!(url.starts_with(&format!("{origin}/connect.stripe.com/oauth/authorize?")));
    assert!(url.contains("scope=read_write"));
    let location = callback(
        &state,
        &cookie,
        "stripe",
        &format!(
            "code=good-code&scope=read_write&state={}",
            enc(&oauth_state)
        ),
    )
    .await;
    assert_eq!(location, "/settings/connections?connected=stripe");

    let body = connections(&state, &cookie).await;
    assert_eq!(body["connections"][0]["label"], "Acme Roofing (test mode)");
    let id = body["connections"][0]["id"]
        .as_str()
        .expect("id")
        .to_string();
    // Stripe keeps no per-account secret.
    let access: Option<String> =
        sqlx::query_scalar("SELECT access_token_enc FROM connections WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&state.pool)
            .await
            .expect("row");
    assert!(access.is_none());

    let balance = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/actions/stripe.balance"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(balance.status, StatusCode::OK, "{:?}", balance.body);
    assert_eq!(balance.body["result"]["available"][0]["amount"], 125000);
}

#[sqlx::test]
async fn shopify_checks_the_shop_and_signature(pool: PgPool) {
    use hmac::Mac;
    let fake = Arc::new(Fake::default());
    let origin = fake_provider(fake).await;
    let state = state_with_providers(pool, &origin);
    let cookie = sign_in(&state, "shop@example.com", "shopkeeper").await;

    let bad = call(
        &state,
        "POST",
        "/api/connections/oauth/shopify/start",
        json!({ "shop": "evil.com" }),
        &cookie,
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    let signed = |oauth_state: &str, shop: &str, secret: &str| {
        let params = [
            ("code", "good-code"),
            ("shop", shop),
            ("state", oauth_state),
            ("timestamp", "1763300000"),
        ];
        let message = params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let mut mac =
            <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(secret.as_bytes()).expect("key");
        mac.update(message.as_bytes());
        let hmac: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        params
            .iter()
            .map(|(k, v)| format!("{k}={}", enc(v)))
            .chain(std::iter::once(format!("hmac={hmac}")))
            .collect::<Vec<_>>()
            .join("&")
    };

    let (url, oauth_state) = start(&state, &cookie, "shopify", json!({ "shop": "acme" })).await;
    assert!(
        url.starts_with(&format!(
            "{origin}/acme.myshopify.com/admin/oauth/authorize?"
        )),
        "{url}"
    );
    // A forged signature is refused.
    let location = callback(
        &state,
        &cookie,
        "shopify",
        &signed(&oauth_state, "acme.myshopify.com", "wrong-secret"),
    )
    .await;
    assert!(location.contains("error=expired"), "{location}");

    // A different shop than the one asked for is refused.
    let (_, oauth_state) = start(&state, &cookie, "shopify", json!({ "shop": "acme" })).await;
    let location = callback(
        &state,
        &cookie,
        "shopify",
        &signed(&oauth_state, "other.myshopify.com", "shopify-secret"),
    )
    .await;
    assert!(location.contains("error=expired"), "{location}");

    let (_, oauth_state) = start(
        &state,
        &cookie,
        "shopify",
        json!({ "shop": "acme.myshopify.com" }),
    )
    .await;
    let location = callback(
        &state,
        &cookie,
        "shopify",
        &signed(&oauth_state, "acme.myshopify.com", "shopify-secret"),
    )
    .await;
    assert_eq!(location, "/settings/connections?connected=shopify");

    let body = connections(&state, &cookie).await;
    assert_eq!(body["connections"][0]["label"], "Acme Store");
    let id = body["connections"][0]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let orders = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/actions/shopify.recent_orders"),
        json!({ "input": { "max": 5 } }),
        &cookie,
    )
    .await;
    assert_eq!(orders.status, StatusCode::OK, "{:?}", orders.body);
    assert_eq!(orders.body["result"]["orders"][0]["name"], "#1001");
}

#[sqlx::test]
async fn webhooks_post_into_a_room_and_send_out(pool: PgPool) {
    let fake = Arc::new(Fake::default());
    let origin = fake_provider(fake.clone()).await;
    let state = state_with_providers(pool, &origin);
    let cookie = sign_in(&state, "hooks@example.com", "hooker").await;
    let other = sign_in(&state, "other@example.com", "other").await;

    let room = call(
        &state,
        "POST",
        "/threads",
        json!({ "title": "#orders" }),
        &cookie,
    )
    .await;
    assert_eq!(room.status, StatusCode::CREATED, "{:?}", room.body);
    let room_id = room.body["id"].as_str().expect("room id").to_string();
    let their_room = call(
        &state,
        "POST",
        "/threads",
        json!({ "title": "#theirs" }),
        &other,
    )
    .await;
    let their_room_id = their_room.body["id"].as_str().expect("room id").to_string();

    let none = call(
        &state,
        "POST",
        "/api/connections/webhook",
        json!({ "label": "Zap" }),
        &cookie,
    )
    .await;
    assert_eq!(none.status, StatusCode::BAD_REQUEST);
    let foreign = call(
        &state,
        "POST",
        "/api/connections/webhook",
        json!({ "label": "Zap", "room_id": their_room_id }),
        &cookie,
    )
    .await;
    assert_eq!(foreign.status, StatusCode::BAD_REQUEST);
    let ftp = call(
        &state,
        "POST",
        "/api/connections/webhook",
        json!({ "label": "Zap", "outbound_url": "ftp://x" }),
        &cookie,
    )
    .await;
    assert_eq!(ftp.status, StatusCode::BAD_REQUEST);

    let body = json!({
        "label": "Zapier",
        "room_id": room_id,
        "outbound_url": format!("{origin}/outbound"),
        "idempotency_key": "hook-1"
    });
    let created = call(
        &state,
        "POST",
        "/api/connections/webhook",
        body.clone(),
        &cookie,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let again = call(&state, "POST", "/api/connections/webhook", body, &cookie).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(
        again.body["connection"]["id"],
        created.body["connection"]["id"]
    );

    let id = created.body["connection"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let inbound = created.body["connection"]["inbound_url"]
        .as_str()
        .expect("inbound url")
        .to_string();
    let path = inbound
        .trim_start_matches(&state.config.app_base_url)
        .to_string();
    assert!(path.starts_with(&format!("/api/hooks/{id}/")));

    let event = |path: &str, key: Option<&str>| {
        let mut builder = axum::http::Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .header("x-forwarded-for", "127.0.0.1");
        if let Some(key) = key {
            builder = builder.header("idempotency-key", key);
        }
        builder
            .body(axum::body::Body::from(
                json!({ "order": "#1001", "total": 89 }).to_string(),
            ))
            .expect("request")
    };
    let res = send_with_state(state.clone(), event(&path, Some("evt-1"))).await;
    assert_eq!(res.status, StatusCode::ACCEPTED, "{:?}", res.body);
    let res = send_with_state(state.clone(), event(&path, Some("evt-1"))).await;
    assert_eq!(res.status, StatusCode::ACCEPTED);
    assert_eq!(res.body["duplicate"], true);
    let wrong = send_with_state(
        state.clone(),
        event(&format!("/api/hooks/{id}/{}", "0".repeat(64)), None),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::NOT_FOUND);

    let notes: Vec<String> = sqlx::query_scalar(
        "SELECT body FROM notes WHERE thread_id = $1::uuid AND deleted_at IS NULL ORDER BY created_at",
    )
    .bind(&room_id)
    .fetch_all(&state.pool)
    .await
    .expect("notes");
    let hook_notes: Vec<&String> = notes.iter().filter(|n| n.contains("#1001")).collect();
    assert_eq!(hook_notes.len(), 1, "{notes:?}");
    assert!(hook_notes[0].contains("**Zapier**"));

    let sent = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/actions/http.send"),
        json!({ "input": { "payload": { "event": "lead.won" } } }),
        &cookie,
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK, "{:?}", sent.body);
    let outbound = fake.outbound.lock().expect("lock").clone();
    assert_eq!(outbound.len(), 1);
    assert_eq!(outbound[0]["data"]["event"], "lead.won");

    // Removing it turns the inbound URL off.
    let removed = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/delete"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(removed.status, StatusCode::OK);
    let res = send_with_state(state.clone(), event(&path, None)).await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn bots_use_connections_but_only_people_manage_them(pool: PgPool) {
    let fake = Arc::new(Fake::default());
    let origin = fake_provider(fake.clone()).await;
    let state = state_with_providers(pool, &origin);
    let cookie = sign_in(&state, "boss@example.com", "boss").await;

    let created = call(
        &state,
        "POST",
        "/api/connections/webhook",
        json!({ "label": "Out", "outbound_url": format!("{origin}/outbound") }),
        &cookie,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let id = created.body["connection"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    assert!(
        created.body["connection"]["inbound_url"].is_null(),
        "no room, no inbound URL"
    );

    // A bot key for this person, and one for someone else.
    let key = bot_key(&state, &cookie, "Helper").await;
    let stranger = sign_in(&state, "stranger@example.com", "stranger").await;
    let stranger_key = bot_key(&state, &stranger, "Snoop").await;

    let bot_call = |key: String, method: &'static str, uri: String, body: Value| {
        let state = state.clone();
        async move {
            send_with_state(state, common::bearer_json_request(method, &uri, body, &key)).await
        }
    };
    let listed = send_with_state(
        state.clone(),
        common::bearer_get_request("/api/connections", &key),
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(listed.body["connections"].as_array().map(Vec::len), Some(1));

    let sent = bot_call(
        key.clone(),
        "POST",
        format!("/api/connections/{id}/actions/http.send"),
        json!({ "input": { "payload": { "from": "bot" } } }),
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK, "{:?}", sent.body);
    let actor: Option<Uuid> = sqlx::query_scalar(
        "SELECT actor_agent_id FROM connection_actions WHERE connection_id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&state.pool)
    .await
    .expect("row");
    assert!(actor.is_some());

    let delete = bot_call(
        key.clone(),
        "POST",
        format!("/api/connections/{id}/delete"),
        json!({}),
    )
    .await;
    assert_eq!(delete.status, StatusCode::FORBIDDEN);
    let make = bot_call(
        key.clone(),
        "POST",
        "/api/connections/webhook".to_string(),
        json!({ "label": "x", "outbound_url": format!("{origin}/outbound") }),
    )
    .await;
    assert_eq!(make.status, StatusCode::FORBIDDEN);
    let start = bot_call(
        key,
        "POST",
        "/api/connections/oauth/google/start".to_string(),
        json!({}),
    )
    .await;
    assert_eq!(start.status, StatusCode::FORBIDDEN);

    let snoop = bot_call(
        stranger_key,
        "POST",
        format!("/api/connections/{id}/actions/http.send"),
        json!({}),
    )
    .await;
    assert_eq!(snoop.status, StatusCode::NOT_FOUND);
    assert_eq!(fake.outbound.lock().expect("lock").len(), 1);
}

/// A bot key through the device flow, approved by whoever `cookie` is.
async fn bot_key(state: &AppState, cookie: &str, name: &str) -> String {
    let code = send_with_state(state.clone(), common::post_request("/device/code", None)).await;
    let user_code = code.body["code"].as_str().expect("code").to_string();
    let device_code = code.body["device_code"]
        .as_str()
        .expect("device code")
        .to_string();
    let approve = send_with_state(
        state.clone(),
        form_request(
            "POST",
            "/device/approve",
            &[("code", &user_code), ("agent_name", name)],
            Some(cookie),
        ),
    )
    .await;
    assert_eq!(approve.status, StatusCode::OK);
    let poll = send_with_state(
        state.clone(),
        common::json_request(
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

#[sqlx::test]
async fn stripe_and_shopify_connect_with_a_pasted_key(pool: PgPool) {
    let fake = Arc::new(Fake::default());
    let origin = fake_provider(fake).await;
    // No sign-in apps at all: keys still work.
    let mut state = common::state(pool);
    let mut config = (*state.config).clone();
    config.connectors = ConnectorApps {
        test_origin: Some(origin.clone()),
        ..ConnectorApps::default()
    };
    state.config = Arc::new(config);
    let cookie = sign_in(&state, "keys@example.com", "keyholder").await;

    let catalog = connections(&state, &cookie).await["catalog"].clone();
    let stripe = catalog
        .as_array()
        .and_then(|c| c.iter().find(|e| e["id"] == "stripe"))
        .expect("stripe")
        .clone();
    assert_eq!(stripe["available"], true);
    assert_eq!(stripe["oauth"], false);
    assert_eq!(stripe["accepts_key"], true);

    let wrong_kind = call(
        &state,
        "POST",
        "/api/connections/key/stripe",
        json!({ "api_key": "pk_live_x" }),
        &cookie,
    )
    .await;
    assert_eq!(wrong_kind.status, StatusCode::BAD_REQUEST);
    let refused = call(
        &state,
        "POST",
        "/api/connections/key/stripe",
        json!({ "api_key": "rk_test_bad" }),
        &cookie,
    )
    .await;
    assert_eq!(
        refused.status,
        StatusCode::BAD_REQUEST,
        "{:?}",
        refused.body
    );
    let google = call(
        &state,
        "POST",
        "/api/connections/key/google",
        json!({ "api_key": "x" }),
        &cookie,
    )
    .await;
    assert_eq!(google.status, StatusCode::NOT_FOUND);

    let made = call(
        &state,
        "POST",
        "/api/connections/key/stripe",
        json!({ "api_key": "rk_test_good", "idempotency_key": "k1" }),
        &cookie,
    )
    .await;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    assert_eq!(
        made.body["connection"]["label"],
        "Stripe account (test mode)"
    );
    let id = made.body["connection"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let stored: Option<String> =
        sqlx::query_scalar("SELECT access_token_enc FROM connections WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&state.pool)
            .await
            .expect("row");
    assert!(stored
        .as_deref()
        .is_some_and(|k| k.starts_with("enc:v1:") && !k.contains("rk_test_good")));

    let balance = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/actions/stripe.balance"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(balance.status, StatusCode::OK, "{:?}", balance.body);
    assert_eq!(balance.body["result"]["available"][0]["amount"], 125000);

    // Shopify with a custom app's token.
    let no_shop = call(
        &state,
        "POST",
        "/api/connections/key/shopify",
        json!({ "api_key": "shpat_good" }),
        &cookie,
    )
    .await;
    assert_eq!(no_shop.status, StatusCode::BAD_REQUEST);
    let bad = call(
        &state,
        "POST",
        "/api/connections/key/shopify",
        json!({ "api_key": "shpat_wrong", "shop": "acme" }),
        &cookie,
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST, "{:?}", bad.body);
    let shop = call(
        &state,
        "POST",
        "/api/connections/key/shopify",
        json!({ "api_key": "shpat_good", "shop": "acme" }),
        &cookie,
    )
    .await;
    assert_eq!(shop.status, StatusCode::CREATED, "{:?}", shop.body);
    assert_eq!(shop.body["connection"]["label"], "Acme Store");
    let shop_id = shop.body["connection"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let orders = call(
        &state,
        "POST",
        &format!("/api/connections/{shop_id}/actions/shopify.recent_orders"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(orders.status, StatusCode::OK, "{:?}", orders.body);

    // Pasting a key for the same store replaces it rather than adding one.
    let again = call(
        &state,
        "POST",
        "/api/connections/key/shopify",
        json!({ "api_key": "shpat_good", "shop": "acme.myshopify.com" }),
        &cookie,
    )
    .await;
    assert_eq!(
        again.body["connection"]["id"],
        shop.body["connection"]["id"]
    );
    assert_eq!(
        connections(&state, &cookie).await["connections"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );

    // Nobody else can use them.
    let other = sign_in(&state, "notkeys@example.com", "notkeys").await;
    let snoop = call(
        &state,
        "POST",
        &format!("/api/connections/{id}/actions/stripe.balance"),
        json!({}),
        &other,
    )
    .await;
    assert_eq!(snoop.status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn an_admin_sets_up_google_sign_in_in_settings(pool: PgPool) {
    let fake = Arc::new(Fake::default());
    let origin = fake_provider(fake).await;
    let mut state = common::state(pool);
    let mut config = (*state.config).clone();
    config.connectors = ConnectorApps {
        test_origin: Some(origin.clone()),
        ..ConnectorApps::default()
    };
    state.config = Arc::new(config);
    // The first account owns the platform (bootstrap admin); the second does not.
    let admin = sign_in(&state, "owner@example.com", "owner").await;
    let member = sign_in(&state, "member@example.com", "member").await;

    let before = call(
        &state,
        "POST",
        "/api/connections/oauth/google/start",
        json!({}),
        &admin,
    )
    .await;
    assert_eq!(before.status, StatusCode::SERVICE_UNAVAILABLE);

    let forbidden = call(&state, "GET", "/api/connections/apps", json!({}), &member).await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    let denied = call(
        &state,
        "POST",
        "/api/connections/apps/google",
        json!({ "client_id": "x.apps.googleusercontent.com", "client_secret": "s" }),
        &member,
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);

    let listed = call(&state, "GET", "/api/connections/apps", json!({}), &admin).await;
    assert_eq!(listed.status, StatusCode::OK, "{:?}", listed.body);
    assert_eq!(listed.body["apps"][0]["provider"], "google");
    assert!(listed.body["apps"][0]["source"].is_null());
    assert!(listed.body["apps"][0]["redirect_uri"]
        .as_str()
        .is_some_and(|u| u.ends_with("/api/connections/oauth/google/callback")));

    let not_google = call(
        &state,
        "POST",
        "/api/connections/apps/google",
        json!({ "client_id": "abc", "client_secret": "s" }),
        &admin,
    )
    .await;
    assert_eq!(not_google.status, StatusCode::BAD_REQUEST);
    let saved = call(
        &state,
        "POST",
        "/api/connections/apps/google",
        json!({ "client_id": "123-abc.apps.googleusercontent.com", "client_secret": "google-secret", "idempotency_key": "g1" }),
        &admin,
    )
    .await;
    assert_eq!(saved.status, StatusCode::OK, "{:?}", saved.body);
    let secret: String =
        sqlx::query_scalar("SELECT client_secret_enc FROM connector_apps WHERE deleted_at IS NULL")
            .fetch_one(&state.pool)
            .await
            .expect("row");
    assert!(secret.starts_with("enc:v1:") && !secret.contains("google-secret"));

    // Everyone can now sign in with Google, using the saved app.
    let catalog = connections(&state, &member).await["catalog"].clone();
    assert_eq!(catalog[0]["oauth"], true);
    let (url, oauth_state) =
        start(&state, &member, "google", json!({ "products": ["gmail"] })).await;
    assert!(
        url.contains("client_id=123-abc.apps.googleusercontent.com"),
        "{url}"
    );
    let location = callback(
        &state,
        &member,
        "google",
        &format!("code=good-code&state={}", enc(&oauth_state)),
    )
    .await;
    assert_eq!(location, "/settings/connections?connected=google");

    let listed = call(&state, "GET", "/api/connections/apps", json!({}), &admin).await;
    assert_eq!(listed.body["apps"][0]["source"], "settings");
    assert_eq!(
        listed.body["apps"][0]["client_id"],
        "123-abc.apps.googleusercontent.com"
    );

    let removed = call(
        &state,
        "POST",
        "/api/connections/apps/google/delete",
        json!({}),
        &admin,
    )
    .await;
    assert_eq!(removed.status, StatusCode::OK);
    let after = call(
        &state,
        "POST",
        "/api/connections/oauth/google/start",
        json!({}),
        &admin,
    )
    .await;
    assert_eq!(after.status, StatusCode::SERVICE_UNAVAILABLE);
}

/// A request to the desktop app's local server (it answers only its own
/// loopback address).
fn local_request(
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> axum::http::Request<axum::body::Body> {
    let mut b = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:43210")
        .header("x-forwarded-for", "127.0.0.1");
    if let Some(c) = cookie {
        b = b.header("cookie", c);
    }
    match body {
        Some(v) => b
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .body(axum::body::Body::from(v.to_string()))
            .expect("request"),
        None => b.body(axum::body::Body::empty()).expect("request"),
    }
}

#[sqlx::test]
async fn the_desktop_app_signs_in_to_google_in_the_browser(pool: PgPool) {
    let fake = Arc::new(Fake::default());
    let origin = fake_provider(fake).await;
    let mut state = state_with_providers(pool, &origin);
    let mut config = (*state.config).clone();
    config.app_base_url = "http://127.0.0.1:43210".to_string();
    config.local = Some(airobot::local::LocalMode {
        token: "desktop-token".to_string(),
    });
    state.config = Arc::new(config);

    let entered = send_with_state(
        state.clone(),
        local_request("GET", "/local/enter?token=desktop-token", None, None),
    )
    .await;
    let cookie = session_cookie(&entered.headers).expect("session");

    let started = send_with_state(
        state.clone(),
        local_request(
            "POST",
            "/api/connections/oauth/google/start",
            Some(&cookie),
            Some(json!({ "products": ["calendar"] })),
        ),
    )
    .await;
    assert_eq!(started.status, StatusCode::OK, "{:?}", started.body);
    let url = started.body["authorize_url"]
        .as_str()
        .expect("url")
        .to_string();
    // Google sends the browser back to this computer's own address.
    assert!(
        url.contains(&enc(
            "http://127.0.0.1:43210/api/connections/oauth/google/callback"
        )),
        "{url}"
    );
    let oauth_state = reqwest::Url::parse(&url)
        .expect("url")
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .expect("state");

    // The computer's browser has no Airobot session: the state is enough,
    // and the tab is told to go back to the app.
    let done = send_with_state(
        state.clone(),
        local_request(
            "GET",
            &format!(
                "/api/connections/oauth/google/callback?code=good-code&state={}",
                enc(&oauth_state)
            ),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.text);
    assert!(done.text.contains("Google is connected"), "{}", done.text);
    let listed = send_with_state(
        state.clone(),
        local_request("GET", "/api/connections", Some(&cookie), None),
    )
    .await;
    assert_eq!(listed.body["connections"][0]["label"], "owner@example.com");

    // A made-up state is still refused.
    let forged = send_with_state(
        state.clone(),
        local_request(
            "GET",
            "/api/connections/oauth/google/callback?code=good-code&state=nope",
            None,
            None,
        ),
    )
    .await;
    assert!(forged.text.contains("was not connected"), "{}", forged.text);
}

#[sqlx::test]
async fn on_the_website_the_callback_needs_the_signed_in_person(pool: PgPool) {
    let fake = Arc::new(Fake::default());
    let origin = fake_provider(fake).await;
    let state = state_with_providers(pool, &origin);
    let cookie = sign_in(&state, "web@example.com", "webperson").await;
    let (_, oauth_state) = start(&state, &cookie, "google", json!({})).await;
    let anonymous = send_with_state(
        state.clone(),
        get_request(
            &format!(
                "/api/connections/oauth/google/callback?code=good-code&state={}",
                enc(&oauth_state)
            ),
            None,
        ),
    )
    .await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    assert_eq!(connections(&state, &cookie).await["connections"], json!([]));
}
