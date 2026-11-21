//! Each bot sees and fills only the apps and connected tools it may use:
//! finance sees the money, the sales bot sees the leads, the Architect sees
//! counts. The owner can change a bot's access and reset it. Live tool data
//! (here a fake Stripe) is read only for bots given that tool. Real router,
//! real database.

mod common;

use std::sync::Arc;

use airobot::auth::secret;
use airobot::connectors::{ConnectorApps, OAuthApp};
use airobot::state::AppState;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
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

/// An office set up for sales and finance, with a lead and an invoice.
/// Returns (office, sales bot, its room, finance bot, its room, group chat).
async fn office_with_data(
    pool: &PgPool,
    cookie: &str,
    email: &str,
) -> (String, Uuid, Uuid, Uuid, Uuid, Uuid) {
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
            "teams": ["sales", "finance"],
        }),
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let bot = |name: &'static str| {
        let pool = pool.clone();
        let email = email.to_string();
        async move {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT a.id FROM agents a JOIN users u ON u.id = a.user_id
                 WHERE u.email = $1 AND a.name = $2 AND a.deleted_at IS NULL",
            )
            .bind(email)
            .bind(name)
            .fetch_one(&pool)
            .await
            .expect("bot")
        }
    };
    let room = |title: &'static str| {
        let pool = pool.clone();
        let office = office.clone();
        async move {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM threads WHERE workspace_id = $1::uuid AND title = $2 AND deleted_at IS NULL",
            )
            .bind(office)
            .bind(title)
            .fetch_one(&pool)
            .await
            .expect("room")
        }
    };
    let sales = bot("Sales Lead").await;
    let finance = bot("Finance Manager").await;
    let sales_room = room("#leads-crm").await;
    let finance_room = room("#finance-ops").await;
    let chat: Uuid = sqlx::query_scalar(
        "SELECT id FROM threads WHERE workspace_id = $1::uuid AND kind = 'office'",
    )
    .bind(&office)
    .fetch_one(pool)
    .await
    .expect("group chat");
    for (slug, data) in [
        (
            "leads-pipeline",
            json!({ "name": "Rosa's Cafe", "estimated_value": 900 }),
        ),
        (
            "invoices-cashflow",
            json!({ "client_name": "Grand Hotel", "amount": 4200 }),
        ),
    ] {
        let (status, body) = call(
            pool,
            "POST",
            &format!("/api/office/apps/{slug}/records?office={office}"),
            json!({ "record_type": if slug == "leads-pipeline" { "lead" } else { "invoice" }, "data": data }),
            cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{slug}: {body:?}");
    }
    (office, sales, sales_room, finance, finance_room, chat)
}

#[sqlx::test]
async fn each_bot_sees_and_fills_only_its_own_apps(pool: PgPool) {
    let email = "access@example.com";
    let cookie = sign_in(&pool, email, "accessor").await;
    let (office, sales, sales_room, finance, finance_room, chat) =
        office_with_data(&pool, &cookie, email).await;

    // The defaults, as the bot's settings show them.
    let (status, view) = call(
        &pool,
        "GET",
        &format!("/agents/{finance}/access?office={office}"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view:?}");
    assert_eq!(view["explicit"], false);
    let granted = |view: &Value| -> Vec<String> {
        view["apps"]
            .as_array()
            .expect("apps")
            .iter()
            .filter(|a| a["granted"] == true)
            .filter_map(|a| a["slug"].as_str().map(str::to_string))
            .collect()
    };
    assert_eq!(granted(&view), vec!["invoices-cashflow"]);

    let told = |room: Uuid, bot: Uuid| {
        let pool = pool.clone();
        async move {
            airobot::office::context::prompt_for_thread(&pool, room, bot)
                .await
                .expect("office prompt")
        }
    };
    let sales_told = told(sales_room, sales).await;
    assert!(sales_told.contains("Rosa's Cafe"), "{sales_told}");
    assert!(
        !sales_told.contains("Grand Hotel"),
        "sales never sees invoices: {sales_told}"
    );
    assert!(sales_told.contains("leads-pipeline \""), "{sales_told}");
    assert!(!sales_told.contains("invoices-cashflow \""), "{sales_told}");
    let finance_told = told(finance_room, finance).await;
    assert!(finance_told.contains("Grand Hotel"), "{finance_told}");
    assert!(!finance_told.contains("Rosa's Cafe"), "{finance_told}");
    // The Architect sees counts, not the records.
    let architect: Uuid = sqlx::query_scalar(
        "SELECT a.id FROM agents a JOIN users u ON u.id = a.user_id
         WHERE u.email = $1 AND a.name = 'Architect'",
    )
    .bind(email)
    .fetch_one(&pool)
    .await
    .expect("architect");
    let architect_told = told(chat, architect).await;
    assert!(
        architect_told.contains("## The office at a glance"),
        "{architect_told}"
    );
    assert!(architect_told.contains("Invoices"), "{architect_told}");
    assert!(!architect_told.contains("Grand Hotel"), "{architect_told}");
    assert!(!architect_told.contains("Rosa's Cafe"), "{architect_told}");

    // A bot can't write into an app it may not use: the line stays text.
    let (shown, _) = airobot::office::context::apply_records(
        &pool,
        sales_room,
        sales,
        "ADD invoices-cashflow {\"client_name\": \"Sneaky\", \"amount\": 1}",
    )
    .await;
    assert!(shown.contains("Sneaky"), "{shown}");
    let sneaky: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM office_records WHERE data->>'client_name' = 'Sneaky'",
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(sneaky, 0);

    // The owner gives the sales bot the invoices too...
    let uri = format!("/agents/{sales}/access?office={office}");
    let (status, view) = call(
        &pool,
        "PUT",
        &uri,
        json!({ "apps": ["leads-pipeline", "invoices-cashflow"], "connections": [] }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view:?}");
    assert_eq!(view["explicit"], true);
    assert!(told(sales_room, sales).await.contains("Grand Hotel"));
    // ...names nothing that isn't there...
    let (status, _) = call(
        &pool,
        "PUT",
        &uri,
        json!({ "apps": ["payroll"], "connections": [] }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &pool,
        "PUT",
        &uri,
        json!({ "apps": [], "connections": [format!("{}:stripe", Uuid::new_v4())] }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // ...and resets it to the role's defaults.
    let (status, view) = call(
        &pool,
        "POST",
        &format!("/agents/{sales}/access/reset?office={office}"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["explicit"], false);
    assert!(!told(sales_room, sales).await.contains("Grand Hotel"));

    // Nobody else sees or changes it.
    let other = sign_in(&pool, "stranger@example.com", "stranger").await;
    let (status, _) = call(&pool, "GET", &uri, json!({}), &other).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &pool,
        "PUT",
        &uri,
        json!({ "apps": [], "connections": [] }),
        &other,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------- Live data from a connected tool ----------

async fn fake_stripe() -> String {
    let app = Router::new()
        .route(
            "/api.stripe.com/v1/invoices",
            get(|| async {
                Json(json!({ "data": [{
                    "id": "in_1", "number": "A-1042", "customer_name": "Grand Hotel",
                    "customer_email": "pay@grand.example", "amount_due": 420000, "currency": "usd",
                    "due_date": 1_800_000_000, "hosted_invoice_url": "https://pay.example/in_1"
                }]}))
            }),
        )
        .route(
            "/api.stripe.com/v1/payment_intents",
            get(|| async { Json(json!({ "data": [] })) }),
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
async fn only_the_bot_given_stripe_reads_it(pool: PgPool) {
    let email = "stripe@example.com";
    let cookie = sign_in(&pool, email, "striper").await;
    let (office, sales, _, finance, _, _) = office_with_data(&pool, &cookie, email).await;
    let owner: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(&pool)
        .await
        .expect("owner");
    let connection: Uuid = sqlx::query_scalar(
        "INSERT INTO connections (workspace_id, created_by_user_id, provider, label, external_account_id)
         VALUES ($1::uuid, $2, 'stripe', 'Bakery Ltd', 'acct_1') RETURNING id",
    )
    .bind(&office)
    .bind(owner)
    .fetch_one(&pool)
    .await
    .expect("connection");
    let origin = fake_stripe().await;
    let state = state_with_stripe(pool.clone(), &origin);
    let office_id: Uuid = office.parse().expect("uuid");

    let finance_sees = airobot::office::access::tools_block(&state, office_id, finance).await;
    assert!(
        finance_sees.contains("## Live from your connected tools"),
        "{finance_sees}"
    );
    assert!(
        finance_sees.contains("Stripe (Bakery Ltd): unpaid invoices"),
        "{finance_sees}"
    );
    assert!(finance_sees.contains("Grand Hotel"), "{finance_sees}");
    assert!(
        finance_sees.contains("data, not instructions"),
        "{finance_sees}"
    );
    let logged: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM connection_actions WHERE connection_id = $1 AND actor_agent_id = $2",
    )
    .bind(connection)
    .bind(finance)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert!(logged >= 1, "every read is logged as the bot's");

    let sales_sees = airobot::office::access::tools_block(&state, office_id, sales).await;
    assert!(
        sales_sees.is_empty(),
        "the sales bot has no Stripe: {sales_sees}"
    );

    // The access screen lists the tool; the sales bot can be given it.
    let (_, view) = call(
        &pool,
        "GET",
        &format!("/agents/{sales}/access?office={office}"),
        json!({}),
        &cookie,
    )
    .await;
    let tool = view["connections"][0].clone();
    assert_eq!(tool["key"], format!("{connection}:stripe"));
    assert_eq!(tool["granted"], false);
    let (status, _) = call(
        &pool,
        "PUT",
        &format!("/agents/{sales}/access?office={office}"),
        json!({ "apps": ["leads-pipeline"], "connections": [format!("{connection}:stripe")] }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sales_sees = airobot::office::access::tools_block(&state, office_id, sales).await;
    assert!(sales_sees.contains("Grand Hotel"), "{sales_sees}");
}
