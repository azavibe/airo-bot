//! Offices: several workspaces per person, each with its own rooms; rooms
//! and bots made inside an office; deleting rooms and offices; and the AI
//! each bot answers with. Real router, real database.

mod common;

use airobot::auth::secret;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::{authed_json_request, form_request, get_request, post_request, send, session_cookie};
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

async fn offices(pool: &PgPool, cookie: &str) -> Vec<Value> {
    let res = send(pool.clone(), get_request("/api/offices", Some(cookie))).await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    res.body["offices"].as_array().expect("offices").clone()
}

async fn new_office(pool: &PgPool, cookie: &str, name: &str) -> String {
    let res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/offices",
            json!({ "name": name }),
            Some(cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{:?}", res.body);
    res.body["office"]["id"].as_str().expect("id").to_string()
}

async fn threads(pool: &PgPool, cookie: &str) -> Vec<Value> {
    let res = send(
        pool.clone(),
        authed_json_request("GET", "/threads", json!({}), Some(cookie)),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    res.body["threads"].as_array().expect("threads").clone()
}

#[sqlx::test]
async fn offices_are_listed_created_renamed_and_deleted(pool: PgPool) {
    let cookie = sign_in(&pool, "offices@example.com", "officer").await;

    // Everyone starts with a first office with a plain name.
    let list = offices(&pool, &cookie).await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["primary"], true);
    assert_eq!(list[0]["name"], "My office");
    // The Assistant room is in it.
    assert!(list[0]["room_count"].as_i64().expect("count") >= 1);

    let second = new_office(&pool, &cookie, "  Side   project ").await;
    let list = offices(&pool, &cookie).await;
    assert_eq!(list.len(), 2);
    assert_eq!(list[1]["name"], "Side project");
    assert_eq!(list[1]["primary"], false);
    assert_eq!(list[1]["room_count"], 0);

    // Same idempotency key, same office.
    let again = |key: &str| {
        authed_json_request(
            "POST",
            "/api/offices",
            json!({ "name": "Studio", "idempotency_key": key }),
            Some(&cookie),
        )
    };
    let a = send(pool.clone(), again("office-key")).await;
    let b = send(pool.clone(), again("office-key")).await;
    assert!(a.status.is_success() && b.status.is_success());
    assert_eq!(a.body["office"]["id"], b.body["office"]["id"]);

    let blank = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/offices",
            json!({ "name": "  " }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(blank.status, StatusCode::BAD_REQUEST);

    let renamed = send(
        pool.clone(),
        authed_json_request(
            "PATCH",
            &format!("/api/offices/{second}"),
            json!({ "name": "Agency" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(renamed.status, StatusCode::OK, "{:?}", renamed.body);
    assert_eq!(renamed.body["name"], "Agency");

    // A room in the second office, then deleting the office takes it along.
    let room = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/threads",
            json!({ "title": "#planning", "workspace_id": second }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(room.status, StatusCode::CREATED, "{:?}", room.body);
    assert_eq!(room.body["workspace_id"], second.as_str());

    let deleted = send(
        pool.clone(),
        post_request(&format!("/api/offices/{second}/delete"), Some(&cookie)),
    )
    .await;
    assert_eq!(deleted.status, StatusCode::OK, "{:?}", deleted.body);
    assert_eq!(deleted.body["rooms_deleted"], 1);
    assert!(threads(&pool, &cookie)
        .await
        .iter()
        .all(|t| t["title"] != "#planning"));
    assert!(offices(&pool, &cookie)
        .await
        .iter()
        .all(|o| o["id"] != second.as_str()));

    // The first office can't be deleted.
    let primary = offices(&pool, &cookie).await[0]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let refused = send(
        pool.clone(),
        post_request(&format!("/api/offices/{primary}/delete"), Some(&cookie)),
    )
    .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn offices_are_private_to_their_owner(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let other = sign_in(&pool, "other@example.com", "other").await;
    let office = new_office(&pool, &owner, "Private office").await;

    assert!(offices(&pool, &other)
        .await
        .iter()
        .all(|o| o["id"] != office.as_str()));
    let rename = send(
        pool.clone(),
        authed_json_request(
            "PATCH",
            &format!("/api/offices/{office}"),
            json!({ "name": "Mine now" }),
            Some(&other),
        ),
    )
    .await;
    assert_eq!(rename.status, StatusCode::FORBIDDEN);
    let delete = send(
        pool.clone(),
        post_request(&format!("/api/offices/{office}/delete"), Some(&other)),
    )
    .await;
    assert_eq!(delete.status, StatusCode::FORBIDDEN);

    // Nobody can put a room in someone else's office...
    let room = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/threads",
            json!({ "title": "#sneaky", "workspace_id": office }),
            Some(&other),
        ),
    )
    .await;
    assert_eq!(room.status, StatusCode::FORBIDDEN);
    // ...or a bot there.
    let bot = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/agents",
            json!({ "name": "Spy", "workspace_id": office }),
            Some(&other),
        ),
    )
    .await;
    assert_eq!(bot.status, StatusCode::FORBIDDEN);

    // Nor anyone else's bot in their own room.
    let owners_bot = send(
        pool.clone(),
        authed_json_request("POST", "/agents", json!({ "name": "Helper" }), Some(&owner)),
    )
    .await;
    let bot_id = owners_bot.body["agent"]["id"].as_str().expect("id");
    let stolen = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/threads",
            json!({ "title": "#borrowed", "agent_ids": [bot_id] }),
            Some(&other),
        ),
    )
    .await;
    assert_eq!(stolen.status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn a_new_bot_in_an_office_gets_its_own_room(pool: PgPool) {
    let cookie = sign_in(&pool, "bots@example.com", "botmaker").await;
    let office = new_office(&pool, &cookie, "Shop").await;

    let res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/agents",
            json!({ "name": "Support", "instructions": "Answer customers.", "workspace_id": office }),
            Some(&cookie),
        ),
    )
    .await;
    assert!(res.status.is_success(), "{:?}", res.body);
    let bot_id = res.body["agent"]["id"].as_str().expect("id").to_string();
    let room_id = res.body["room_id"].as_str().expect("room").to_string();

    let room = threads(&pool, &cookie)
        .await
        .into_iter()
        .find(|t| t["id"] == room_id.as_str())
        .expect("room listed");
    assert_eq!(room["title"], "#support");
    assert_eq!(room["workspace_id"], office.as_str());
    assert_eq!(room["agent_ids"], json!([bot_id]));

    // A second room in the office with the same bot.
    let second = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/threads",
            json!({ "title": "#returns", "workspace_id": office, "agent_ids": [bot_id] }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(second.status, StatusCode::CREATED, "{:?}", second.body);
    let second_id = second.body["id"].as_str().expect("id").to_string();
    let listed = threads(&pool, &cookie).await;
    let returns = listed
        .iter()
        .find(|t| t["id"] == second_id.as_str())
        .expect("listed");
    assert_eq!(returns["agent_ids"], json!([bot_id]));

    let list = offices(&pool, &cookie).await;
    assert_eq!(list[1]["room_count"], 2);
}

#[sqlx::test]
async fn rooms_are_deleted_only_by_their_owner(pool: PgPool) {
    let owner = sign_in(&pool, "rooms@example.com", "roomy").await;
    let other = sign_in(&pool, "nosy@example.com", "nosy").await;
    let room = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/threads",
            json!({ "title": "#scratch" }),
            Some(&owner),
        ),
    )
    .await;
    let id = room.body["id"].as_str().expect("id").to_string();

    let refused = send(
        pool.clone(),
        post_request(&format!("/threads/{id}/delete"), Some(&other)),
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);

    let deleted = send(
        pool.clone(),
        post_request(&format!("/threads/{id}/delete"), Some(&owner)),
    )
    .await;
    assert_eq!(deleted.status, StatusCode::OK, "{:?}", deleted.body);
    assert!(threads(&pool, &owner)
        .await
        .iter()
        .all(|t| t["id"] != id.as_str()));
    // Soft-deleted: the row and its notes stay.
    let kept: bool = sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM threads WHERE id = $1")
        .bind(Uuid::parse_str(&id).expect("uuid"))
        .fetch_one(&pool)
        .await
        .expect("row");
    assert!(kept);
}

#[sqlx::test]
async fn bots_say_which_ai_answers_for_them(pool: PgPool) {
    let cookie = sign_in(&pool, "models@example.com", "modeler").await;
    let res = send(
        pool.clone(),
        authed_json_request("GET", "/agents", json!({}), Some(&cookie)),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    let agents = res.body["agents"].as_array().expect("agents");
    let assistant = agents
        .iter()
        .find(|a| a["name"] == "Assistant")
        .expect("assistant");
    // The test server has no platform key and no OpenRouter key is
    // connected, so nothing can answer yet, and the bot says so.
    assert_eq!(assistant["ai_source"], "none");
    assert!(assistant["effective_model"].is_null());

    let bot_id = assistant["id"].as_str().expect("id");
    let saved = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/agents/{bot_id}/settings"),
            json!({ "name": "Assistant", "provider_model": "openai/gpt-5" }),
            Some(&cookie),
        ),
    )
    .await;
    assert!(saved.status.is_success(), "{:?}", saved.body);
    let res = send(
        pool.clone(),
        authed_json_request("GET", "/agents", json!({}), Some(&cookie)),
    )
    .await;
    let assistant = res.body["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .find(|a| a["name"] == "Assistant")
        .expect("assistant")
        .clone();
    assert_eq!(assistant["provider_model"], "openai/gpt-5");
}

#[sqlx::test]
async fn office_setup_can_target_a_second_office(pool: PgPool) {
    let cookie = sign_in(&pool, "setup@example.com", "setupper").await;
    let office = new_office(&pool, &cookie, "Second").await;
    let res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/office/onboard",
            json!({
                "business_type": "Bakery",
                "replaced_saas": [],
                "goals": "Take orders",
                "company_name": "Crumbs",
                "workspace_id": office,
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    assert_eq!(res.body["workspace_id"], office.as_str());

    let list = offices(&pool, &cookie).await;
    assert_eq!(
        list[0]["name"], "My office",
        "the first office keeps its name"
    );
    assert_eq!(list[1]["name"], "Crumbs");
    assert_eq!(list[1]["room_count"], 3);
    // Every room there has a bot that answers; the office's group chat has
    // the Architect and all of them.
    let all = threads(&pool, &cookie).await;
    let chat = all
        .iter()
        .find(|t| t["workspace_id"] == office.as_str() && t["kind"] == "office")
        .expect("the office's group chat");
    assert_eq!(chat["agent_ids"].as_array().map(Vec::len), Some(4));
    for room in all
        .iter()
        .filter(|t| t["workspace_id"] == office.as_str() && t["kind"] == "room")
    {
        assert_eq!(
            room["agent_ids"].as_array().map(Vec::len),
            Some(1),
            "{room}"
        );
    }

    // Someone else's office can't be set up.
    let other = sign_in(&pool, "intruder@example.com", "intruder").await;
    let refused = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/office/onboard",
            json!({ "business_type": "X", "replaced_saas": [], "goals": "", "workspace_id": office }),
            Some(&other),
        ),
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn an_office_is_set_up_for_the_areas_chosen(pool: PgPool) {
    let cookie = sign_in(&pool, "areas@example.com", "areas").await;
    let office = new_office(&pool, &cookie, "Clinic").await;
    let setup = |teams: Value| {
        let pool = pool.clone();
        let cookie = cookie.clone();
        let office = office.clone();
        async move {
            send(
                pool,
                authed_json_request(
                    "POST",
                    "/api/office/onboard",
                    json!({
                        "business_type": "",
                        "replaced_saas": [],
                        "goals": "",
                        "company_name": "Bright Smiles",
                        "workspace_id": office,
                        "teams": teams,
                    }),
                    Some(&cookie),
                ),
            )
            .await
        }
    };

    assert_eq!(setup(json!([])).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        setup(json!(["support", "marketingz"])).await.status,
        StatusCode::BAD_REQUEST
    );

    // No business type is needed when areas are chosen.
    let res = setup(json!(["support", "bookings", "store"])).await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    assert_eq!(
        res.body["channels"],
        json!(["#support-desk", "#bookings", "#store-ops"])
    );
    let apps: Vec<&str> = res.body["apps_created"]
        .as_array()
        .expect("apps")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(apps, vec!["Support Inbox", "Appointments"]);

    // Each room has its own bot, named for its work.
    let rooms: Vec<Value> = threads(&pool, &cookie)
        .await
        .into_iter()
        .filter(|t| t["workspace_id"] == office.as_str() && t["kind"] == "room")
        .collect();
    assert_eq!(rooms.len(), 3);
    for room in &rooms {
        assert_eq!(
            room["agent_ids"].as_array().map(Vec::len),
            Some(1),
            "{room:?}"
        );
    }
    let bots: Vec<String> = sqlx::query_scalar(
        "SELECT a.name FROM agents a JOIN users u ON u.id = a.user_id
         WHERE u.email = 'areas@example.com' AND a.deleted_at IS NULL ORDER BY a.name",
    )
    .fetch_all(&pool)
    .await
    .expect("bots");
    for name in ["Architect", "Scheduler", "Store Manager", "Support Agent"] {
        assert!(
            bots.iter().any(|b| b == name),
            "{name} missing from {bots:?}"
        );
    }
    assert!(
        !bots.iter().any(|b| b == "Sales Lead"),
        "only the chosen areas get bots"
    );
}

#[sqlx::test]
async fn each_office_has_its_own_apps(pool: PgPool) {
    let cookie = sign_in(&pool, "apps@example.com", "appsy").await;
    let shop = new_office(&pool, &cookie, "Shop").await;
    let res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/office/onboard",
            json!({ "business_type": "", "replaced_saas": [], "goals": "", "teams": ["sales"], "workspace_id": shop }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);

    let apps = |uri: String, who: String| {
        let pool = pool.clone();
        async move {
            send(
                pool,
                authed_json_request("GET", &uri, json!({}), Some(&who)),
            )
            .await
        }
    };
    let in_shop = apps(format!("/api/office/apps?office={shop}"), cookie.clone()).await;
    assert_eq!(in_shop.status, StatusCode::OK);
    let slugs: Vec<&str> = in_shop
        .body
        .as_array()
        .expect("apps")
        .iter()
        .filter_map(|a| a["slug"].as_str())
        .collect();
    assert_eq!(slugs, vec!["leads-pipeline"]);
    let first = apps("/api/office/apps".to_string(), cookie.clone()).await;
    assert!(
        first.body.as_array().expect("apps").is_empty(),
        "the first office has none"
    );

    let records = apps(
        format!("/api/office/apps/leads-pipeline/records?office={shop}"),
        cookie.clone(),
    )
    .await;
    assert_eq!(records.status, StatusCode::OK);
    // Apps start empty; the office's bots fill them for this business.
    assert_eq!(records.body, json!([]));
    // In the first office there is no such app.
    let missing = apps(
        "/api/office/apps/leads-pipeline/records".to_string(),
        cookie.clone(),
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);

    let other = sign_in(&pool, "peek@example.com", "peeker").await;
    let peek = apps(format!("/api/office/apps?office={shop}"), other).await;
    assert_eq!(peek.status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn one_app_is_added_without_setting_the_office_up_again(pool: PgPool) {
    let cookie = sign_in(&pool, "install@example.com", "installer").await;
    let office = new_office(&pool, &cookie, "Bamboo Hotel").await;
    let install = |kind: &str, who: &str| {
        let pool = pool.clone();
        let who = who.to_string();
        let uri = format!("/api/office/apps/install?office={office}");
        let body = json!({ "kind": kind, "idempotency_key": format!("i-{kind}") });
        async move { send(pool, authed_json_request("POST", &uri, body, Some(&who))).await }
    };

    let res = install("crm", &cookie).await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    assert_eq!(res.body["slug"], "leads-pipeline");
    assert_eq!(res.body["apps_created"], json!(["Lead Pipeline & CRM"]));
    // Installing it again adds nothing.
    let again = install("crm", &cookie).await;
    assert_eq!(again.body["apps_created"], json!([]));
    assert_eq!(
        install("payroll", &cookie).await.status,
        StatusCode::BAD_REQUEST
    );

    // The office keeps its name and gets no rooms, bots or kickoff.
    let list = offices(&pool, &cookie).await;
    let mine = list
        .iter()
        .find(|o| o["id"] == office.as_str())
        .expect("office");
    assert_eq!(mine["name"], "Bamboo Hotel");
    let business: Option<String> =
        sqlx::query_scalar("SELECT business_type FROM workspaces WHERE id = $1::uuid")
            .bind(&office)
            .fetch_one(&pool)
            .await
            .expect("office row");
    assert!(business.is_none(), "an app is not a business type");
    let apps = send(
        pool.clone(),
        get_request(&format!("/api/office/apps?office={office}"), Some(&cookie)),
    )
    .await;
    assert_eq!(apps.body.as_array().map(Vec::len), Some(1));
    let records = send(
        pool.clone(),
        get_request(
            &format!("/api/office/apps/leads-pipeline/records?office={office}"),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(
        records.body,
        json!([]),
        "apps start empty, no sample businesses"
    );

    // Someone else cannot add apps to this office.
    let other = sign_in(&pool, "noinstall@example.com", "noinstall").await;
    assert_eq!(
        install("support", &other).await.status,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test]
async fn bots_fill_the_office_apps_from_their_answers(pool: PgPool) {
    let cookie = sign_in(&pool, "records@example.com", "recorder").await;
    let office = new_office(&pool, &cookie, "Bamboo Hotel").await;
    let res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/office/onboard",
            json!({
                "business_type": "",
                "replaced_saas": [],
                "goals": "",
                "company_name": "Bamboo Hotel",
                "workspace_id": office,
                "teams": ["sales"],
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    let room: Uuid = sqlx::query_scalar(
        "SELECT id FROM threads WHERE workspace_id = $1::uuid AND title = '#leads-crm'",
    )
    .bind(&office)
    .fetch_one(&pool)
    .await
    .expect("room");
    let bot: Uuid = sqlx::query_scalar(
        "SELECT a.id FROM agents a JOIN users u ON u.id = a.user_id
         WHERE u.email = 'records@example.com' AND a.name = 'Sales Lead'",
    )
    .fetch_one(&pool)
    .await
    .expect("bot");

    // What a bot is told: this business, its apps, and how to write.
    let prompt = airobot::office::context::prompt_for_thread(&pool, room, bot)
        .await
        .expect("office prompt");
    assert!(prompt.contains("You work for Bamboo Hotel."), "{prompt}");
    assert!(prompt.contains("leads-pipeline"), "{prompt}");
    assert!(!prompt.contains("Small business"), "{prompt}");

    let answer = "Three groups to win this month.\nADD leads-pipeline {\"name\": \"Tour operator\", \"stage\": \"Qualified\", \"estimated_value\": 2400}\nADD leads-pipeline {\"name\": \"Wedding party\", \"estimated_value\": 5000}\nQuestion: Do you take group bookings over 20 rooms?";
    let (shown, actions, cards) =
        airobot::office::context::apply_records(&pool, room, bot, answer).await;
    assert!(shown.contains("Added 2 to Lead Pipeline & CRM"), "{shown}");
    assert!(!shown.contains("ADD "), "{shown}");
    assert!(
        shown.contains("Question: Do you take group bookings"),
        "{shown}"
    );
    assert_eq!(actions.expect("button")[0]["target"], "apps");
    // Each added record is also a live card in the message.
    assert_eq!(cards.len(), 2);
    assert!(cards
        .iter()
        .all(|c| c["type"] == "app_record_card" && c["action"] == "created"));

    let records = send(
        pool.clone(),
        get_request(
            &format!("/api/office/apps/leads-pipeline/records?office={office}"),
            Some(&cookie),
        ),
    )
    .await;
    let list = records.body.as_array().expect("records").clone();
    assert_eq!(list.len(), 2, "{list:?}");
    let tour = list
        .iter()
        .find(|r| r["data"]["name"] == "Tour operator")
        .expect("tour");
    assert_eq!(tour["status"], "Qualified");
    let wedding = list
        .iter()
        .find(|r| r["data"]["name"] == "Wedding party")
        .expect("wedding");
    assert_eq!(
        wedding["status"], "New Lead",
        "a lead without a stage starts at the first one"
    );

    // An answer without ADD lines is left alone.
    let (plain, none, _) =
        airobot::office::context::apply_records(&pool, room, bot, "Just advice.").await;
    assert_eq!(plain, "Just advice.");
    assert!(none.is_none());
}

/// Notes in a thread: (bot name, body, actions), oldest first.
async fn bot_notes(pool: &PgPool, thread: Uuid) -> Vec<(String, String, Option<Value>)> {
    sqlx::query_as(
        "SELECT a.name, n.body, n.actions FROM notes n JOIN agents a ON a.id = n.author_agent_id
         WHERE n.thread_id = $1 ORDER BY n.created_at, n.seq",
    )
    .bind(thread)
    .fetch_all(pool)
    .await
    .expect("notes")
}

#[sqlx::test]
async fn without_an_ai_the_architect_asks_once_for_the_whole_team(pool: PgPool) {
    // A fresh self-hosted server: no OpenRouter key and no AI on this
    // computer. The kickoff starts every bot's first task; none can answer.
    let cookie = sign_in(&pool, "noai@example.com", "noai").await;
    let office = new_office(&pool, &cookie, "Bakery").await;
    let res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/api/office/onboard",
            json!({
                "business_type": "Bakery",
                "replaced_saas": [],
                "goals": "More orders",
                "workspace_id": office,
                "teams": ["sales", "finance", "support"],
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    let office_id: Uuid = office.parse().expect("office id");
    let chat: Uuid =
        sqlx::query_scalar("SELECT id FROM threads WHERE kind = 'office' AND workspace_id = $1")
            .bind(office_id)
            .fetch_one(&pool)
            .await
            .expect("the office's group chat");

    let notice = |notes: &[(String, String, Option<Value>)]| {
        notes
            .iter()
            .filter(|(_, body, _)| body.starts_with("Connect an AI to wake up your team"))
            .cloned()
            .collect::<Vec<_>>()
    };
    let per_bot = |notes: &[(String, String, Option<Value>)]| {
        notes
            .iter()
            .filter(|(_, body, _)| body.contains("needs an AI voice"))
            .count()
    };
    // The bots work one after another; wait until all three have tried.
    for _ in 0..80 {
        let notes = bot_notes(&pool, chat).await;
        if !notice(&notes).is_empty() || per_bot(&notes) >= 3 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    let notes = bot_notes(&pool, chat).await;
    assert_eq!(per_bot(&notes), 0, "no warning per bot: {notes:?}");
    let said = notice(&notes);
    assert_eq!(said.len(), 1, "the Architect says it once: {notes:?}");
    assert_eq!(said[0].0, "Architect");
    assert_eq!(
        said[0].2,
        Some(json!([{ "label": "Connect an AI", "kind": "open", "target": "account" }])),
        "its button opens Settings → Account"
    );

    // Writing again does not repeat it.
    let say = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/threads/{chat}/notes"),
            json!({ "body": "Hello team", "kind": "status" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(say.status, StatusCode::CREATED, "{:?}", say.body);
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let notes = bot_notes(&pool, chat).await;
    assert_eq!(notice(&notes).len(), 1, "{notes:?}");
    assert_eq!(per_bot(&notes), 0, "{notes:?}");
}
