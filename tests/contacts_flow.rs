//! Contacts: one per person, unique by email in an office; records that
//! name a person link to it (owner's and bots'); facts about a contact
//! reach a bot only when the conversation names them; Stripe customers
//! import without duplicates. Real router, real database.

mod common;

use std::sync::Arc;

use airobot::auth::secret;
use airobot::connectors::{ConnectorApps, OAuthApp};
use airobot::state::AppState;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{Duration, Utc};
use common::{
    authed_json_request, form_request, get_request, send, send_with_state, session_cookie,
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

/// An office set up for sales, finance and marketing.
async fn office(pool: &PgPool, cookie: &str) -> String {
    let (status, made) = call(
        pool,
        "POST",
        "/api/offices",
        json!({ "name": "Bakery" }),
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made:?}");
    let office = made["office"]["id"].as_str().expect("id").to_string();
    let (status, body) = call(
        pool,
        "POST",
        "/api/office/onboard",
        json!({
            "business_type": "Bakery",
            "replaced_saas": [],
            "goals": "",
            "workspace_id": office,
            "teams": ["sales", "finance", "marketing"],
        }),
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    office
}

#[sqlx::test]
async fn contacts_are_kept_one_per_person(pool: PgPool) {
    let cookie = sign_in(&pool, "contacts@example.com", "contacter").await;
    let office = office(&pool, &cookie).await;
    let uri = format!("/api/offices/{office}/contacts");

    let (status, ada) = call(
        &pool,
        "POST",
        &uri,
        json!({ "name": "Ada Lovelace", "email": " Ada@Example.com ", "phone": "+1 (555) 014-4000",
                "company": "Analytical Ltd", "tags": ["vip", "vip"], "idempotency_key": "c-1" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ada:?}");
    assert_eq!(ada["email"], "ada@example.com");
    assert_eq!(ada["phone"], "+15550144000");
    assert_eq!(ada["tags"], json!(["vip"]));
    assert_eq!(ada["source"], "owner");
    let ada_id = ada["id"].as_str().expect("id").to_string();
    let (_, again) = call(
        &pool,
        "POST",
        &uri,
        json!({ "name": "Ada Lovelace", "email": "ada@example.com", "idempotency_key": "c-1" }),
        &cookie,
    )
    .await;
    assert_eq!(again["id"], ada["id"], "same key, same contact");
    // The same email again is refused, not duplicated.
    let (status, _) = call(
        &pool,
        "POST",
        &uri,
        json!({ "name": "A. L.", "email": "ADA@example.com" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(
        &pool,
        "POST",
        &uri,
        json!({ "name": "X", "email": "not-an-email" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Search and edit.
    call(
        &pool,
        "POST",
        &uri,
        json!({ "name": "Grace Hopper" }),
        &cookie,
    )
    .await;
    let (_, found) = call(
        &pool,
        "GET",
        &format!("{uri}?q=analytical"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(found["contacts"].as_array().map(Vec::len), Some(1));
    assert_eq!(found["total"], 2);
    let (status, edited) = call(
        &pool,
        "PATCH",
        &format!("/api/contacts/{ada_id}"),
        json!({ "marketing_consent": "unsubscribed", "company": "" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited:?}");
    assert_eq!(edited["marketing_consent"], "unsubscribed");
    assert!(edited["consent_updated_at"].is_string());
    assert!(edited["company"].is_null());

    // Nobody else sees or touches them.
    let other = sign_in(&pool, "nosy@example.com", "nosy").await;
    let (status, _) = call(&pool, "GET", &uri, json!({}), &other).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &pool,
        "GET",
        &format!("/api/contacts/{ada_id}"),
        json!({}),
        &other,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/contacts/{ada_id}/delete"),
        json!({}),
        &other,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Delete (soft).
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/contacts/{ada_id}/delete"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, left) = call(&pool, "GET", &uri, json!({}), &cookie).await;
    assert_eq!(left["total"], 1);
}

#[sqlx::test]
async fn records_that_name_a_person_link_to_one_contact(pool: PgPool) {
    let email = "linker@example.com";
    let cookie = sign_in(&pool, email, "linker").await;
    let office = office(&pool, &cookie).await;
    // The owner adds a lead with an email...
    let (status, lead) = call(
        &pool,
        "POST",
        &format!("/api/office/apps/leads-pipeline/records?office={office}"),
        json!({ "record_type": "lead", "data": { "name": "Rosa's Cafe", "email": "rosa@cafe.example" } }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{lead:?}");
    let contact = lead["contact_id"].as_str().expect("linked").to_string();
    // ...and an invoice for the same email joins the same contact.
    let (_, invoice) = call(
        &pool,
        "POST",
        &format!("/api/office/apps/invoices-cashflow/records?office={office}"),
        json!({ "record_type": "invoice", "data": { "client_name": "Rosa Cafe Ltd", "client_email": "ROSA@cafe.example", "amount": 90 } }),
        &cookie,
    )
    .await;
    assert_eq!(invoice["contact_id"], contact.as_str());
    // A social post names nobody.
    let (_, post) = call(
        &pool,
        "POST",
        &format!("/api/office/apps/social-video-studio/records?office={office}"),
        json!({ "record_type": "video_post", "data": { "title": "Spring sale" } }),
        &cookie,
    )
    .await;
    assert!(post["contact_id"].is_null(), "{post:?}");

    // A bot's lead for the same person links there too, and never edits it.
    let sales: Uuid = sqlx::query_scalar(
        "SELECT a.id FROM agents a JOIN users u ON u.id = a.user_id WHERE u.email = $1 AND a.name = 'Sales Lead'",
    )
    .bind(email)
    .fetch_one(&pool)
    .await
    .expect("sales");
    let room: Uuid = sqlx::query_scalar(
        "SELECT id FROM threads WHERE workspace_id = $1::uuid AND title = '#leads-crm'",
    )
    .bind(&office)
    .fetch_one(&pool)
    .await
    .expect("room");
    airobot::office::context::apply_records(
        &pool,
        room,
        sales,
        "ADD leads-pipeline {\"name\": \"Rosa (new name)\", \"email\": \"rosa@cafe.example\"}\nADD leads-pipeline {\"name\": \"Tom Baker\", \"phone\": \"555 123 4567\"}",
    )
    .await;
    let (_, detail) = call(
        &pool,
        "GET",
        &format!("/api/contacts/{contact}"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(
        detail["contact"]["name"], "Rosa's Cafe",
        "bots don't rename contacts"
    );
    let apps: Vec<&str> = detail["timeline"]
        .as_array()
        .expect("timeline")
        .iter()
        .filter_map(|t| t["app_slug"].as_str())
        .collect();
    assert_eq!(apps.len(), 3, "{detail:?}");
    assert!(apps.contains(&"invoices-cashflow"));
    let (_, list) = call(
        &pool,
        "GET",
        &format!("/api/offices/{office}/contacts"),
        json!({}),
        &cookie,
    )
    .await;
    let tom = list["contacts"]
        .as_array()
        .expect("contacts")
        .iter()
        .find(|c| c["name"] == "Tom Baker")
        .cloned()
        .expect("a new contact from the bot's lead");
    assert_eq!(tom["source"], "bot");
    assert_eq!(tom["phone"], "5551234567");
    assert_eq!(list["total"], 2);
}

#[sqlx::test]
async fn a_contacts_facts_reach_a_bot_only_when_named(pool: PgPool) {
    let cookie = sign_in(&pool, "facts@example.com", "facter").await;
    let office = office(&pool, &cookie).await;
    let (_, rosa) = call(
        &pool,
        "POST",
        &format!("/api/offices/{office}/contacts"),
        json!({ "name": "Rosa's Cafe", "email": "rosa@cafe.example" }),
        &cookie,
    )
    .await;
    let rosa_id = rosa["id"].as_str().expect("id").to_string();
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/contacts/{rosa_id}/memory"),
        json!({ "body": "Pays on the 1st of the month." }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Not in the office's own memory list.
    let (_, memory) = call(
        &pool,
        "GET",
        &format!("/api/offices/{office}/memory"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(memory["memories"].as_array().map(Vec::len), Some(0));

    let office_id: Uuid = office.parse().expect("uuid");
    let named = airobot::office::contacts::context_block(
        &pool,
        office_id,
        "Owner: Can you chase Rosa's Cafe about the order?",
    )
    .await;
    assert!(
        named.contains("## Contacts in this conversation"),
        "{named}"
    );
    assert!(named.contains("rosa@cafe.example"), "{named}");
    assert!(named.contains("Pays on the 1st of the month."), "{named}");
    let other =
        airobot::office::contacts::context_block(&pool, office_id, "Owner: How is Tom doing?")
            .await;
    assert!(
        other.is_empty(),
        "nothing about Rosa when she isn't named: {other}"
    );

    // A bot keeps a fact about one customer on that customer.
    let room: Uuid = sqlx::query_scalar(
        "SELECT id FROM threads WHERE workspace_id = $1::uuid AND title = '#leads-crm'",
    )
    .bind(&office)
    .fetch_one(&pool)
    .await
    .expect("room");
    let bot: Uuid = sqlx::query_scalar(
        "SELECT ta.agent_id FROM thread_agents ta WHERE ta.thread_id = $1 LIMIT 1",
    )
    .bind(room)
    .fetch_one(&pool)
    .await
    .expect("bot");
    let (shown, saved) = airobot::office::memory::apply_remember(
        &pool,
        room,
        bot,
        "Done.\nREMEMBER: about Rosa's Cafe: prefers invoices by email",
    )
    .await;
    assert_eq!(shown, "Done.");
    assert_eq!(saved.len(), 1);
    let (_, detail) = call(
        &pool,
        "GET",
        &format!("/api/contacts/{rosa_id}"),
        json!({}),
        &cookie,
    )
    .await;
    let facts: Vec<&str> = detail["memories"]
        .as_array()
        .expect("memories")
        .iter()
        .filter_map(|m| m["body"].as_str())
        .collect();
    assert!(facts.contains(&"prefers invoices by email"), "{facts:?}");
    let (_, memory) = call(
        &pool,
        "GET",
        &format!("/api/offices/{office}/memory"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(
        memory["memories"].as_array().map(Vec::len),
        Some(0),
        "still not office-wide"
    );
}

async fn fake_stripe_customers() -> String {
    let app = Router::new().route(
        "/api.stripe.com/v1/customers",
        get(|| async {
            Json(json!({ "data": [
                { "id": "cus_1", "name": "Rosa's Cafe", "email": "ROSA@cafe.example", "created": 1 },
                { "id": "cus_2", "name": "Grand Hotel", "email": "pay@grand.example", "created": 2 },
                { "id": "cus_3", "name": null, "email": null, "created": 3 }
            ]}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    format!("http://{addr}")
}

fn state_with_stripe(pool: PgPool, origin: &str) -> AppState {
    let mut s = common::state(pool);
    let mut config = (*s.config).clone();
    config.connectors = ConnectorApps {
        google: None,
        stripe: Some(OAuthApp {
            client_id: "ca_test".to_string(),
            client_secret: "sk_test_platform".to_string(),
        }),
        shopify: None,
        test_origin: Some(origin.to_string()),
    };
    s.config = Arc::new(config);
    s
}

#[sqlx::test]
async fn stripe_customers_import_once(pool: PgPool) {
    let email = "import@example.com";
    let cookie = sign_in(&pool, email, "importer").await;
    let office = office(&pool, &cookie).await;
    let import = format!("/api/offices/{office}/contacts/import");
    let (status, _) = call(&pool, "POST", &import, json!({}), &cookie).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no Stripe connected yet");

    // Rosa is already a contact; Stripe brings her and the Grand Hotel.
    call(
        &pool,
        "POST",
        &format!("/api/offices/{office}/contacts"),
        json!({ "name": "Rosa", "email": "rosa@cafe.example" }),
        &cookie,
    )
    .await;
    let owner: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(&pool)
        .await
        .expect("owner");
    sqlx::query(
        "INSERT INTO connections (workspace_id, created_by_user_id, provider, label, external_account_id)
         VALUES ($1::uuid, $2, 'stripe', 'Bakery Ltd', 'acct_1')",
    )
    .bind(&office)
    .bind(owner)
    .execute(&pool)
    .await
    .expect("connection");
    let state = state_with_stripe(pool.clone(), &fake_stripe_customers().await);
    let run = |state: AppState| {
        let import = import.clone();
        let cookie = cookie.clone();
        async move {
            send_with_state(
                state,
                authed_json_request("POST", &import, json!({}), Some(&cookie)),
            )
            .await
        }
    };
    let first = run(state.clone()).await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.body);
    assert_eq!(first.body["added"], 1, "{:?}", first.body);
    assert_eq!(first.body["matched"], 1, "{:?}", first.body);
    let second = run(state.clone()).await;
    assert_eq!(second.body["added"], 0, "{:?}", second.body);
    assert_eq!(second.body["matched"], 2, "{:?}", second.body);
    let (_, list) = call(
        &pool,
        "GET",
        &format!("/api/offices/{office}/contacts"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(list["total"], 2);
}
