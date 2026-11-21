//! Archive and delete: apps (archive, restore, delete and install again),
//! app records, single messages and bots, plus editing an office's profile.
//! Everything is a soft delete; other people get nothing. Real router, real
//! database.

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

async fn threads(pool: &PgPool, cookie: &str) -> Vec<Value> {
    let (status, body) = call(pool, "GET", "/threads", json!({}), cookie).await;
    assert_eq!(status, StatusCode::OK);
    body["threads"].as_array().expect("threads").clone()
}

async fn agents(pool: &PgPool, cookie: &str) -> Vec<Value> {
    let (status, body) = call(pool, "GET", "/agents", json!({}), cookie).await;
    assert_eq!(status, StatusCode::OK);
    body["agents"].as_array().expect("agents").clone()
}

#[sqlx::test]
async fn apps_are_archived_restored_deleted_and_installed_again(pool: PgPool) {
    let cookie = sign_in(&pool, "apps@example.com", "appowner").await;
    let office = new_office(&pool, &cookie, "Bakery").await;
    let install = format!("/api/office/apps/install?office={office}");
    let (status, body) = call(&pool, "POST", &install, json!({ "kind": "crm" }), &cookie).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let slug = body["slug"].as_str().expect("slug").to_string();

    // A record in it, then that record deleted on its own.
    let records = format!("/api/office/apps/{slug}/records?office={office}");
    let (status, record) = call(
        &pool,
        "POST",
        &records,
        json!({ "record_type": "lead", "data": { "name": "Ada" } }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{record:?}");
    let record_id = record["id"].as_str().expect("record id").to_string();
    let (_, keep) = call(
        &pool,
        "POST",
        &records,
        json!({ "record_type": "lead", "data": { "name": "Grace" } }),
        &cookie,
    )
    .await;
    let delete_record = format!("/api/office/records/{record_id}/delete?office={office}");
    let (status, _) = call(&pool, "POST", &delete_record, json!({}), &cookie).await;
    assert_eq!(status, StatusCode::OK);
    let (_, listed) = call(&pool, "GET", &records, json!({}), &cookie).await;
    let ids: Vec<&str> = listed
        .as_array()
        .expect("records")
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec![keep["id"].as_str().expect("id")],
        "the deleted record is gone"
    );
    // Deleting it again finds nothing.
    let (status, _) = call(&pool, "POST", &delete_record, json!({}), &cookie).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Archive hides it from the office's bots but keeps it listed (with
    // archived_at) so it can be restored.
    let list = format!("/api/office/apps?office={office}");
    let archive = format!("/api/office/apps/{slug}/archive?office={office}");
    let (status, archived) = call(&pool, "POST", &archive, json!({}), &cookie).await;
    assert_eq!(status, StatusCode::OK, "{archived:?}");
    assert!(archived["archived_at"].is_string());
    let bots_see: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM office_apps
         WHERE workspace_id = $1::uuid AND deleted_at IS NULL AND archived_at IS NULL",
    )
    .bind(&office)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(bots_see, 0);
    let (_, apps) = call(&pool, "GET", &list, json!({}), &cookie).await;
    assert_eq!(apps.as_array().map(Vec::len), Some(1));
    let unarchive = format!("/api/office/apps/{slug}/unarchive?office={office}");
    let (status, restored) = call(&pool, "POST", &unarchive, json!({}), &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert!(restored["archived_at"].is_null());

    // Someone else can't touch it.
    let other = sign_in(&pool, "other@example.com", "otherone").await;
    let (status, _) = call(&pool, "POST", &archive, json!({}), &other).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/office/apps/{slug}/delete?office={office}"),
        json!({}),
        &other,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Delete takes the app and its records; nothing is erased.
    let (status, gone) = call(
        &pool,
        "POST",
        &format!("/api/office/apps/{slug}/delete?office={office}"),
        json!({ "idempotency_key": "del-1" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{gone:?}");
    let (_, apps) = call(&pool, "GET", &list, json!({}), &cookie).await;
    assert_eq!(apps.as_array().map(Vec::len), Some(0));
    let (status, _) = call(&pool, "GET", &records, json!({}), &cookie).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let kept: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM office_records WHERE workspace_id = $1::uuid AND deleted_at IS NOT NULL",
    )
    .bind(&office)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(kept, 2, "records are soft-deleted, not erased");

    // The same app can be installed again, fresh.
    let (status, again) = call(&pool, "POST", &install, json!({ "kind": "crm" }), &cookie).await;
    assert_eq!(status, StatusCode::OK, "{again:?}");
    assert_eq!(again["apps_created"], json!(["Lead Pipeline & CRM"]));
    let (_, fresh) = call(&pool, "GET", &records, json!({}), &cookie).await;
    assert_eq!(fresh.as_array().map(Vec::len), Some(0));
}

#[sqlx::test]
async fn a_message_is_deleted_by_the_rooms_owner_only(pool: PgPool) {
    let cookie = sign_in(&pool, "notes@example.com", "noter").await;
    let (status, room) = call(
        &pool,
        "POST",
        "/threads",
        json!({ "title": "#plans" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{room:?}");
    let room_id = room["id"].as_str().expect("id").to_string();
    let notes = format!("/threads/{room_id}/notes");
    let (status, first) = call(
        &pool,
        "POST",
        &notes,
        json!({ "body": "Keep this", "kind": "status" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first:?}");
    let (_, second) = call(
        &pool,
        "POST",
        &notes,
        json!({ "body": "Delete this", "kind": "status" }),
        &cookie,
    )
    .await;
    let second_id = second["id"].as_str().expect("id").to_string();

    let delete = format!("/notes/{second_id}/delete");
    let other = sign_in(&pool, "nosy@example.com", "nosy").await;
    let (status, _) = call(&pool, "POST", &delete, json!({}), &other).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "someone else's room");

    let (status, body) = call(&pool, "POST", &delete, json!({}), &cookie).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["status"], "deleted");
    let (_, thread) = call(
        &pool,
        "GET",
        &format!("/threads/{room_id}"),
        json!({}),
        &cookie,
    )
    .await;
    let bodies: Vec<&str> = thread["notes"]
        .as_array()
        .expect("notes")
        .iter()
        .filter_map(|n| n["body"].as_str())
        .collect();
    assert_eq!(bodies, vec!["Keep this"]);
    let (status, _) = call(&pool, "POST", &delete, json!({}), &cookie).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "already gone");
    let kept: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM notes WHERE id = $1::uuid")
            .bind(&second_id)
            .fetch_one(&pool)
            .await
            .expect("note row");
    assert!(kept.is_some(), "soft-deleted, not erased");
}

#[sqlx::test]
async fn a_bot_is_deleted_with_its_own_room_only(pool: PgPool) {
    let cookie = sign_in(&pool, "bots@example.com", "botowner").await;
    let office = new_office(&pool, &cookie, "Shop").await;
    let (status, made) = call(
        &pool,
        "POST",
        "/agents",
        json!({ "name": "Support", "instructions": "Answer customers.", "workspace_id": office }),
        &cookie,
    )
    .await;
    assert!(status.is_success(), "{made:?}");
    let bot_id = made["agent"]["id"].as_str().expect("id").to_string();
    let own_room = made["room_id"].as_str().expect("room").to_string();
    // A room it shares with the Assistant stays when it goes.
    let assistant = agents(&pool, &cookie)
        .await
        .into_iter()
        .find(|a| a["name"] == "Assistant")
        .expect("assistant");
    let assistant_id = assistant["id"].as_str().expect("id").to_string();
    let (status, shared) = call(
        &pool,
        "POST",
        "/threads",
        json!({ "title": "#team", "workspace_id": office, "agent_ids": [bot_id, assistant_id] }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{shared:?}");
    let shared_id = shared["id"].as_str().expect("id").to_string();

    let delete = format!("/agents/{bot_id}/delete");
    let other = sign_in(&pool, "thief@example.com", "thief").await;
    let (status, _) = call(&pool, "POST", &delete, json!({}), &other).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = call(
        &pool,
        "POST",
        &delete,
        json!({ "idempotency_key": "bye" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["rooms_deleted"], json!([own_room]));
    assert!(!agents(&pool, &cookie)
        .await
        .iter()
        .any(|a| a["id"] == bot_id.as_str()));
    let rooms = threads(&pool, &cookie).await;
    assert!(!rooms.iter().any(|t| t["id"] == own_room.as_str()));
    let team = rooms
        .iter()
        .find(|t| t["id"] == shared_id.as_str())
        .expect("the shared room stays");
    assert_eq!(team["agent_ids"], json!([assistant_id]));

    // The Architect runs the group chats and stays.
    let architect: Uuid =
        sqlx::query_scalar("SELECT id FROM agents WHERE name = 'Architect' AND deleted_at IS NULL")
            .fetch_one(&pool)
            .await
            .expect("architect");
    let (status, body) = call(
        &pool,
        "POST",
        &format!("/agents/{architect}/delete"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");

    // A deleted Assistant is not brought back at the next sign-in.
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/agents/{assistant_id}/delete"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let again = sign_in(&pool, "bots@example.com", "botowner").await;
    assert!(!agents(&pool, &again)
        .await
        .iter()
        .any(|a| a["name"] == "Assistant"));
}

#[sqlx::test]
async fn an_offices_profile_is_edited_and_cleared(pool: PgPool) {
    let cookie = sign_in(&pool, "profile@example.com", "profiler").await;
    let office = new_office(&pool, &cookie, "Clinic").await;
    let uri = format!("/api/offices/{office}");
    let (status, body) = call(
        &pool,
        "PATCH",
        &uri,
        json!({
            "business_type": "Dental clinic",
            "website": "brightsmiles.example",
            "focus": "More bookings",
            "idempotency_key": "profile-1",
        }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["name"], "Clinic", "the name is unchanged");
    assert_eq!(body["business_type"], "Dental clinic");
    assert_eq!(body["website"], "https://brightsmiles.example");
    assert_eq!(body["focus"], "More bookings");

    let (_, list) = call(&pool, "GET", "/api/offices", json!({}), &cookie).await;
    let mine = list["offices"]
        .as_array()
        .expect("offices")
        .iter()
        .find(|o| o["id"] == office.as_str())
        .cloned()
        .expect("office");
    assert_eq!(mine["business_type"], "Dental clinic");

    // An empty value clears a field; the others stay.
    let (status, body) = call(&pool, "PATCH", &uri, json!({ "focus": "" }), &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["focus"].is_null());
    assert_eq!(body["website"], "https://brightsmiles.example");

    let (status, _) = call(
        &pool,
        "PATCH",
        &uri,
        json!({ "website": "http://localhost" }),
        &cookie,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "private addresses are refused"
    );
    let (status, _) = call(
        &pool,
        "PATCH",
        &uri,
        json!({ "focus": "x".repeat(201) }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let other = sign_in(&pool, "stranger@example.com", "stranger").await;
    let (status, _) = call(
        &pool,
        "PATCH",
        &uri,
        json!({ "business_type": "Spam" }),
        &other,
    )
    .await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN,
        "{status}"
    );
}
