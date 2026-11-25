// Team invites and roles for an office (issue #29): the owner invites by
// email, the invited account joins with that same email, members chat with
// the bots and approve, viewers only read, and removed people lose access.
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

async fn new_office(pool: &PgPool, cookie: &str, name: &str) -> Uuid {
    let (status, body) = call(
        pool,
        "POST",
        "/api/offices",
        json!({ "name": name }),
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    body["office"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid")
}

async fn invite(pool: &PgPool, cookie: &str, office: Uuid, email: &str, role: &str) -> String {
    let (status, body) = call(
        pool,
        "POST",
        &format!("/api/offices/{office}/invites"),
        json!({ "email": email, "role": role }),
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    let link = body["link"].as_str().expect("link");
    link.split("invite=").nth(1).expect("token").to_string()
}

async fn accept(pool: &PgPool, cookie: &str, token: &str) -> (StatusCode, Value) {
    call(
        pool,
        "POST",
        "/api/invites/accept",
        json!({ "token": token }),
        cookie,
    )
    .await
}

async fn office_chat(pool: &PgPool, office: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT id FROM threads WHERE workspace_id = $1 AND kind = 'office'")
        .bind(office)
        .fetch_one(pool)
        .await
        .expect("office chat")
}

async fn say(pool: &PgPool, cookie: &str, chat: Uuid, text: &str) -> StatusCode {
    call(
        pool,
        "POST",
        &format!("/threads/{chat}/notes"),
        json!({ "body": text, "kind": "status" }),
        cookie,
    )
    .await
    .0
}

async fn offices(pool: &PgPool, cookie: &str) -> Vec<Value> {
    let (_, body) = call(pool, "GET", "/api/offices", json!({}), cookie).await;
    body["offices"].as_array().cloned().unwrap_or_default()
}

#[sqlx::test]
async fn an_invited_member_works_in_the_office_and_a_viewer_only_reads(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let office = new_office(&pool, &owner, "Bright Smiles").await;
    let chat = office_chat(&pool, office).await;
    let team_uri = format!("/api/offices/{office}/team");

    // Strangers learn nothing about the office.
    let ana = sign_in(&pool, "ana@example.com", "ana").await;
    let (status, _) = call(&pool, "GET", &team_uri, json!({}), &ana).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&pool, "GET", &format!("/threads/{chat}"), json!({}), &ana).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Invited as a member; the link works only for that address.
    let ana_token = invite(&pool, &owner, office, "Ana@Example.com", "member").await;
    let bob = sign_in(&pool, "bob@example.com", "bob").await;
    let (status, err) = accept(&pool, &bob, &ana_token).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["error"]
            .as_str()
            .unwrap_or_default()
            .contains("ana@example.com"),
        "{err}"
    );
    let (status, joined) = accept(&pool, &ana, &ana_token).await;
    assert_eq!(status, StatusCode::OK, "{joined}");
    assert_eq!(joined["role"], "member");
    assert_eq!(joined["office"]["name"], "Bright Smiles");
    // Accepting again is harmless.
    assert_eq!(accept(&pool, &ana, &ana_token).await.0, StatusCode::OK);

    // Ana sees the office next to her own, and its chat.
    let listed = offices(&pool, &ana).await;
    let shared = listed
        .iter()
        .find(|o| o["id"] == office.to_string())
        .expect("shared office listed");
    assert_eq!(shared["role"], "member");
    assert_eq!(shared["primary"], false);
    assert!(
        listed.iter().any(|o| o["role"] == "owner"),
        "her own office too"
    );
    assert_eq!(shared["owner_handle"], "owner");
    let (_, threads) = call(&pool, "GET", "/threads", json!({}), &ana).await;
    assert!(threads.to_string().contains(&chat.to_string()), "{threads}");
    // The owner's own Assistant room stays theirs.
    let assistant: Uuid = sqlx::query_scalar(
        "SELECT t.id FROM threads t JOIN users u ON u.id = t.user_id
         WHERE u.email = 'owner@example.com' AND t.title = '#assistant'",
    )
    .fetch_one(&pool)
    .await
    .expect("owner's assistant room");
    sqlx::query("UPDATE threads SET workspace_id = $1 WHERE id = $2")
        .bind(office)
        .bind(assistant)
        .execute(&pool)
        .await
        .expect("in the shared office");
    let (_, threads) = call(&pool, "GET", "/threads", json!({}), &ana).await;
    assert!(
        !threads.to_string().contains(&assistant.to_string()),
        "{threads}"
    );
    let (status, _) = call(
        &pool,
        "GET",
        &format!("/threads/{assistant}"),
        json!({}),
        &ana,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // She writes in the office chat; her message is hers.
    assert_eq!(
        say(&pool, &ana, chat, "Can someone call the new lead?").await,
        StatusCode::CREATED
    );
    let author: Option<Uuid> = sqlx::query_scalar(
        "SELECT author_user_id FROM notes WHERE thread_id = $1 AND body LIKE 'Can someone%'",
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .expect("note");
    let ana_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = 'ana@example.com'")
        .fetch_one(&pool)
        .await
        .expect("ana");
    assert_eq!(author, Some(ana_id));

    // Setting the office up stays the owner's.
    let (status, _) = call(
        &pool,
        "PATCH",
        &format!("/api/offices/{office}"),
        json!({ "name": "Ana's office" }),
        &ana,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/offices/{office}/invites"),
        json!({ "email": "friend@example.com" }),
        &ana,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A viewer reads but doesn't write.
    let carol = sign_in(&pool, "carol@example.com", "carol").await;
    let token = invite(&pool, &owner, office, "carol@example.com", "viewer").await;
    assert_eq!(accept(&pool, &carol, &token).await.1["role"], "viewer");
    let (status, _) = call(&pool, "GET", &format!("/threads/{chat}"), json!({}), &carol).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        say(&pool, &carol, chat, "Hello?").await,
        StatusCode::FORBIDDEN
    );

    // The team: everyone sees who's in it; only the owner sees addresses
    // and open invites.
    let (_, team) = call(&pool, "GET", &team_uri, json!({}), &owner).await;
    assert_eq!(team["role"], "owner");
    assert_eq!(team["members"].as_array().map(Vec::len), Some(3), "{team}");
    assert!(team.to_string().contains("ana@example.com"));
    let (_, seen_by_carol) = call(&pool, "GET", &team_uri, json!({}), &carol).await;
    assert_eq!(seen_by_carol["role"], "viewer");
    assert!(
        !seen_by_carol.to_string().contains("ana@example.com"),
        "{seen_by_carol}"
    );
    assert_eq!(seen_by_carol["invites"], json!([]));

    // Approvals: members decide, viewers only see.
    let task: Uuid = sqlx::query_scalar(
        "INSERT INTO tasks (workspace_id, title, status) VALUES ($1, 'Send the quote', 'waiting_approval')
         RETURNING id",
    )
    .bind(office)
    .fetch_one(&pool)
    .await
    .expect("task");
    let approval: Uuid = sqlx::query_scalar(
        "INSERT INTO approvals (workspace_id, task_id, requested_action, risk_level, requested_by)
         VALUES ($1, $2, 'Send the quote to Lee', 'low', 'agent:Sales') RETURNING id",
    )
    .bind(office)
    .bind(task)
    .fetch_one(&pool)
    .await
    .expect("approval");
    let (_, listed) = call(&pool, "GET", "/approvals", json!({}), &carol).await;
    let card = listed["approvals"]
        .as_array()
        .and_then(|a| a.iter().find(|a| a["id"] == approval.to_string()).cloned())
        .expect("viewer sees the approval");
    assert_eq!(card["can_decide"], false);
    assert_eq!(card["action_summary"], "Send the quote to Lee");
    let decide = format!("/approvals/{approval}/decide");
    let (status, _) = call(
        &pool,
        "POST",
        &decide,
        json!({ "decision": "approve" }),
        &carol,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, listed) = call(&pool, "GET", "/approvals", json!({}), &ana).await;
    assert!(
        listed.to_string().contains("\"can_decide\":true"),
        "{listed}"
    );
    let (status, decided) = call(
        &pool,
        "POST",
        &decide,
        json!({ "decision": "approve" }),
        &ana,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{decided}");

    // The owner makes Carol a member: now she writes.
    let carol_id: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE email = 'carol@example.com'")
            .fetch_one(&pool)
            .await
            .expect("carol");
    let (status, _) = call(
        &pool,
        "PATCH",
        &format!("/api/offices/{office}/members/{carol_id}"),
        json!({ "role": "member", "idempotency_key": "role-1" }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        say(&pool, &carol, chat, "On it.").await,
        StatusCode::CREATED
    );
    // Members don't change roles.
    let (status, _) = call(
        &pool,
        "PATCH",
        &format!("/api/offices/{office}/members/{ana_id}"),
        json!({ "role": "viewer" }),
        &carol,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Removed: Ana loses the office and its chat at once.
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/offices/{office}/members/{ana_id}/remove"),
        json!({}),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!offices(&pool, &ana)
        .await
        .iter()
        .any(|o| o["id"] == office.to_string()));
    let (status, _) = call(&pool, "GET", &format!("/threads/{chat}"), json!({}), &ana).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        say(&pool, &ana, chat, "Still here?").await,
        StatusCode::FORBIDDEN
    );
    let (status, _) = call(
        &pool,
        "GET",
        &format!("/api/office/apps?office={office}"),
        json!({}),
        &ana,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Her old invite doesn't bring her back.
    let (status, err) = accept(&pool, &ana, &ana_token).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["error"]
            .as_str()
            .unwrap_or_default()
            .contains("removed"),
        "{err}"
    );

    // Carol leaves on her own; the owner can't leave.
    let (status, left) = call(
        &pool,
        "POST",
        &format!("/api/offices/{office}/members/{carol_id}/remove"),
        json!({}),
        &carol,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(left["left"], true);
    let owner_id: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE email = 'owner@example.com'")
            .fetch_one(&pool)
            .await
            .expect("owner");
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/offices/{office}/members/{owner_id}/remove"),
        json!({}),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn invites_can_be_revoked_expire_and_are_checked(pool: PgPool) {
    let owner = sign_in(&pool, "boss@example.com", "boss").await;
    let office = new_office(&pool, &owner, "Cut & Co").await;
    let dave = sign_in(&pool, "dave@example.com", "dave").await;

    // Bad addresses and roles, and your own address.
    for (email, role) in [
        ("not-an-email", "member"),
        ("x@example.com", "owner"),
        ("boss@example.com", "member"),
    ] {
        let (status, _) = call(
            &pool,
            "POST",
            &format!("/api/offices/{office}/invites"),
            json!({ "email": email, "role": role }),
            &owner,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{email} {role}");
    }

    // The same key returns the same invite (without the link again).
    let body = json!({ "email": "dave@example.com", "idempotency_key": "inv-1" });
    let uri = format!("/api/offices/{office}/invites");
    let (status, first) = call(&pool, "POST", &uri, body.clone(), &owner).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, again) = call(&pool, "POST", &uri, body, &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["invite"]["id"], first["invite"]["id"]);
    assert!(again["link"].is_null());

    // Revoked: the link stops working, and the owner no longer lists it.
    let token = first["link"]
        .as_str()
        .and_then(|l| l.split("invite=").nth(1))
        .expect("token");
    let id = first["invite"]["id"].as_str().expect("id");
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/invites/{id}/revoke"),
        json!({}),
        &dave,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "only the owner revokes");
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/invites/{id}/revoke"),
        json!({}),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accept(&pool, &dave, token).await.0, StatusCode::BAD_REQUEST);
    let (_, team) = call(
        &pool,
        "GET",
        &format!("/api/offices/{office}/team"),
        json!({}),
        &owner,
    )
    .await;
    assert_eq!(team["invites"], json!([]));

    // Expired.
    let token = invite(&pool, &owner, office, "dave@example.com", "viewer").await;
    sqlx::query("UPDATE office_invites SET expires_at = now() - interval '1 minute' WHERE deleted_at IS NULL")
        .execute(&pool)
        .await
        .expect("expire");
    let (status, err) = accept(&pool, &dave, &token).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["error"]
            .as_str()
            .unwrap_or_default()
            .contains("expired"),
        "{err}"
    );

    // A forged secret for a real invite.
    let token = invite(&pool, &owner, office, "dave@example.com", "viewer").await;
    let (id, _) = token.split_once('.').expect("id.secret");
    let (status, _) = accept(&pool, &dave, &format!("{id}.{}", "0".repeat(64))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(accept(&pool, &dave, &token).await.0, StatusCode::OK);

    // Already in: no second invite.
    let (status, _) = call(
        &pool,
        "POST",
        &uri,
        json!({ "email": "dave@example.com" }),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}
