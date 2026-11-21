//! Projects & Tasks, Quotes & Proposals and Reviews & Reputation: set up
//! with their areas, a quote converts into an invoice or a project by its
//! app's rules (once), and stages that need the owner's OK (a quote being
//! sent) wait for it even from a Trusted bot. Real router and database.

mod common;

use airobot::auth::secret;
use axum::http::StatusCode;
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

/// An office with every new area: returns (cookie, office).
async fn agency(pool: &PgPool, email: &str, handle: &str) -> (String, Uuid) {
    let cookie = sign_in(pool, email, handle).await;
    let (status, made) = call(
        pool,
        "POST",
        "/api/offices",
        json!({ "name": "Studio" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made:?}");
    let office: Uuid = made["office"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let (status, body) = call(
        pool,
        "POST",
        "/api/office/onboard",
        json!({
            "workspace_id": office, "business_type": "Design agency", "replaced_saas": [], "goals": "",
            "teams": ["sales", "quotes", "projects", "finance", "reviews"], "idempotency_key": "onb"
        }),
        &cookie,
    )
    .await;
    assert!(status.is_success(), "{body:?}");
    (cookie, office)
}

#[sqlx::test]
async fn the_new_areas_set_up_their_apps_rooms_and_bots(pool: PgPool) {
    let (cookie, office) = agency(&pool, "agency@example.com", "agencyowner").await;
    let (_, apps) = call(
        &pool,
        "GET",
        &format!("/api/office/apps?office={office}"),
        json!({}),
        &cookie,
    )
    .await;
    let slugs: Vec<&str> = apps
        .as_array()
        .expect("apps")
        .iter()
        .filter_map(|a| a["slug"].as_str())
        .collect();
    for slug in [
        "leads-pipeline",
        "quotes-proposals",
        "projects-tasks",
        "invoices-cashflow",
        "reviews-reputation",
    ] {
        assert!(slugs.contains(&slug), "{slug} in {slugs:?}");
    }
    let rooms: Vec<String> = sqlx::query_scalar(
        "SELECT title FROM threads WHERE workspace_id = $1 AND kind = 'room' ORDER BY title",
    )
    .bind(office)
    .fetch_all(&pool)
    .await
    .expect("rooms");
    assert!(rooms.contains(&"#projects".to_string()), "{rooms:?}");
    assert_eq!(
        rooms.iter().filter(|r| *r == "#leads-crm").count(),
        1,
        "quotes share the sales room"
    );
    let pm: Option<String> = sqlx::query_scalar(
        "SELECT a.name FROM agents a JOIN thread_agents ta ON ta.agent_id = a.id JOIN threads t ON t.id = ta.thread_id
         WHERE t.workspace_id = $1 AND t.title = '#projects'",
    )
    .bind(office)
    .fetch_optional(&pool)
    .await
    .expect("pm");
    assert_eq!(pm.as_deref(), Some("Project Manager"));
    // An unknown area is still refused.
    let (status, _) = call(
        &pool,
        "POST",
        "/api/office/onboard",
        json!({ "workspace_id": office, "business_type": "x", "replaced_saas": [], "goals": "", "teams": ["astrology"] }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // The apps install on their own too.
    let (status, body) = call(
        &pool,
        "POST",
        &format!("/api/office/apps/install?office={office}"),
        json!({ "kind": "reviews" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
}

#[sqlx::test]
async fn an_approved_quote_converts_once_into_an_invoice_and_a_project(pool: PgPool) {
    let (cookie, office) = agency(&pool, "quotes@example.com", "quotesowner").await;
    let (status, quote) = call(
        &pool,
        "POST",
        &format!("/api/office/apps/quotes-proposals/records?office={office}"),
        json!({
            "record_type": "quote", "status": "Draft",
            "data": {
                "title": "Brand refresh", "client_name": "Rosa's Cafe", "total": 4200, "stage": "Draft",
                "line_items": [{ "description": "Logo", "qty": 1, "unit_price": 1800 }, { "description": "Menus", "qty": 2, "unit_price": 1200 }]
            }
        }),
        &cookie,
    )
    .await;
    assert!(status.is_success(), "{quote:?}");
    let id = quote["id"].as_str().expect("id").to_string();
    let convert = |to: &str| json!({ "to_app": to, "idempotency_key": format!("c-{to}") });
    let uri = format!("/api/office/records/{id}/convert?office={office}");

    // Only an approved quote converts.
    let (status, err) = call(&pool, "POST", &uri, convert("invoices-cashflow"), &cookie).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err:?}");
    assert!(
        err["error"].as_str().unwrap_or("").contains("Approved"),
        "{err:?}"
    );
    let (status, _) = call(
        &pool,
        "PATCH",
        &format!("/api/office/records/{id}?office={office}"),
        json!({ "status": "Approved" }),
        &cookie,
    )
    .await;
    assert!(status.is_success());

    let (status, made) = call(&pool, "POST", &uri, convert("invoices-cashflow"), &cookie).await;
    assert_eq!(status, StatusCode::OK, "{made:?}");
    assert_eq!(made["already"], false);
    let invoice = &made["record"];
    assert_eq!(invoice["app_slug"], "invoices-cashflow");
    assert_eq!(invoice["data"]["client_name"], "Rosa's Cafe");
    assert_eq!(invoice["data"]["amount"], 4200);
    assert_eq!(invoice["data"]["description"], "Brand refresh");
    assert_eq!(invoice["status"], "Draft");
    assert!(invoice["data"]["invoice_number"]
        .as_str()
        .unwrap_or("")
        .starts_with("INV-"));
    let due = invoice["data"]["due_date"].as_str().expect("due date");
    assert_eq!(
        due,
        (Utc::now().date_naive() + Duration::days(14)).to_string()
    );
    assert_eq!(
        invoice["data"]["converted_from"],
        "Quotes & Proposals: Brand refresh"
    );
    let invoice_id = invoice["id"].as_str().expect("id").to_string();
    let contact_kept: bool = sqlx::query_scalar(
        "SELECT i.contact_id IS NOT DISTINCT FROM q.contact_id FROM office_records i, office_records q
         WHERE i.id = $1::uuid AND q.id = $2::uuid",
    )
    .bind(&invoice_id)
    .bind(&id)
    .fetch_one(&pool)
    .await
    .expect("contact");
    assert!(contact_kept);

    // Again: the same invoice.
    let (_, again) = call(&pool, "POST", &uri, convert("invoices-cashflow"), &cookie).await;
    assert_eq!(again["already"], true);
    assert_eq!(again["record"]["id"], invoice_id.as_str());
    let invoices: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM office_records r JOIN office_apps a ON a.id = r.app_id
         WHERE a.workspace_id = $1 AND a.slug = 'invoices-cashflow' AND r.deleted_at IS NULL",
    )
    .bind(office)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(invoices, 1);

    // And a project.
    let (status, project) = call(&pool, "POST", &uri, convert("projects-tasks"), &cookie).await;
    assert_eq!(status, StatusCode::OK, "{project:?}");
    assert_eq!(project["record"]["data"]["name"], "Brand refresh");
    assert_eq!(project["record"]["data"]["client"], "Rosa's Cafe");
    assert_eq!(project["record"]["data"]["value"], 4200);
    assert_eq!(project["record"]["status"], "Backlog");
    let (_, view) = call(
        &pool,
        "GET",
        &format!("/api/office/records?office={office}&ids={id}"),
        json!({}),
        &cookie,
    )
    .await;
    let links = &view["records"][0]["data"]["converted_to"];
    assert_eq!(links["invoices-cashflow"], invoice_id.as_str());
    assert!(links["projects-tasks"].is_string());
    assert_eq!(
        view["records"][0]["data"]["line_items"][1]["unit_price"],
        1200
    );

    // Not to an app without a rule, nor to one the office archived.
    let (status, _) = call(&pool, "POST", &uri, convert("leads-pipeline"), &cookie).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, second) = call(
        &pool,
        "POST",
        &format!("/api/office/apps/quotes-proposals/records?office={office}"),
        json!({ "record_type": "quote", "status": "Approved", "data": { "title": "Signage", "stage": "Approved" } }),
        &cookie,
    )
    .await;
    let second = second["id"].as_str().expect("id").to_string();
    call(
        &pool,
        "POST",
        &format!("/api/office/apps/projects-tasks/archive?office={office}"),
        json!({}),
        &cookie,
    )
    .await;
    let (status, err) = call(
        &pool,
        "POST",
        &format!("/api/office/records/{second}/convert?office={office}"),
        convert("projects-tasks"),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["error"].as_str().unwrap_or("").contains("first"),
        "{err:?}"
    );

    // Nobody else converts it.
    let other = sign_in(&pool, "nosy@example.com", "nosyquotes").await;
    let (status, _) = call(&pool, "POST", &uri, convert("invoices-cashflow"), &other).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn sending_a_quote_waits_for_the_owner_even_from_a_trusted_bot(pool: PgPool) {
    let state = common::state(pool.clone());
    let (cookie, office) = agency(&pool, "send@example.com", "sendowner").await;
    let (bot, room): (Uuid, Uuid) = sqlx::query_as(
        "SELECT a.id, t.id FROM agents a JOIN thread_agents ta ON ta.agent_id = a.id
         JOIN threads t ON t.id = ta.thread_id
         WHERE a.name = 'Sales Lead' AND t.workspace_id = $1 AND t.kind = 'room'",
    )
    .bind(office)
    .fetch_one(&pool)
    .await
    .expect("Sales Lead");
    sqlx::query("UPDATE agents SET permission_mode = 'trusted' WHERE id = $1")
        .bind(bot)
        .execute(&pool)
        .await
        .expect("trust");

    // A bot can't add a quote that's already "Sent": it starts as a Draft.
    let (_, _, cards) = airobot::office::context::apply_records(
        &pool,
        room,
        bot,
        "Drafted.\nADD quotes-proposals {\"title\": \"Window menus\", \"client_name\": \"Ben\", \"total\": 900, \"stage\": \"Sent\"}",
    )
    .await;
    let id = cards[0]["record_id"].as_str().expect("id").to_string();
    let status: String =
        sqlx::query_scalar("SELECT status FROM office_records WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .expect("status");
    assert_eq!(status, "Draft");
    let short = &id.replace('-', "")[..8];

    // Sending it waits for the owner, though the bot is Trusted...
    let (_, cards) = airobot::office::records::apply_updates(
        &state,
        room,
        bot,
        &format!("UPDATE quotes-proposals {short} {{\"stage\": \"Sent\"}}"),
    )
    .await;
    assert_eq!(cards[0]["action"], "proposed_update");
    assert!(cards[0]["approval_id"].is_string());
    // ...while other changes apply at once.
    let (_, cards) = airobot::office::records::apply_updates(
        &state,
        room,
        bot,
        &format!("UPDATE quotes-proposals {short} {{\"total\": 950}}"),
    )
    .await;
    assert_eq!(cards[0]["action"], "updated");
    let (_, view) = call(
        &pool,
        "GET",
        &format!("/api/office/records?office={office}&ids={id}"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(view["records"][0]["status"], "Draft");
    assert_eq!(view["records"][0]["data"]["total"], 950);
    assert_eq!(view["records"][0]["pending"][0]["changes"]["stage"], "Sent");
}
