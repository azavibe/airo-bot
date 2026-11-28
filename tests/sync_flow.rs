// Desktop and cloud sync (issue #28), end to end: a cloud server on a real
// port with this test's database, and a desktop (local mode) with a second
// database of its own. The desktop links with a code the person allows in
// the cloud, then both keep the account's offices the same: newest edit
// wins (the cloud's on a tie), messages are added, deletions travel, the
// person's Architect on both sides becomes one, and nothing private leaves
// the computer. A sync key only ever reaches its own person's offices.
mod common;

use std::net::SocketAddr;
use std::sync::Arc;

use airobot::auth::secret;
use airobot::config::Config;
use airobot::local::{LocalAi, LocalAiRequest, LocalMode};
use airobot::state::AppState;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use chrono::{Duration, Utc};
use common::{
    authed_json_request, bearer_get_request, bearer_json_request, form_request, get_request,
    send_with_state, session_cookie, TestResponse,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const BASE: &str = "http://127.0.0.1:43124";
const HOST: &str = "127.0.0.1:43124";
const TOKEN: &str = "launch-secret-for-the-sync-test";

async fn sign_in(state: &AppState, email: &str, handle: &str) -> String {
    let pool = &state.pool;
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

/// A database of the desktop's own, migrated like a fresh install.
async fn desktop_database() -> (PgPool, PgPool, String) {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let admin = PgPool::connect(&url).await.expect("admin");
    let name = format!("airobot_sync_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin)
        .await
        .expect("create database");
    let (base, _) = url.rsplit_once('/').expect("database url");
    let desk = PgPool::connect(&format!("{base}/{name}"))
        .await
        .expect("desktop database");
    airobot::db::run_migrations(&desk).await.expect("migrate");
    (desk, admin, name)
}

fn desktop_state(pool: PgPool) -> AppState {
    let ai: LocalAi = Arc::new(|_req: LocalAiRequest| {
        Box::pin(async move { Ok("Answered on this computer.".to_string()) })
    });
    let mut s = common::state(pool);
    let mut config: Config = (*s.config).clone();
    config.app_base_url = BASE.to_string();
    config.local = Some(LocalMode {
        token: TOKEN.to_string(),
    });
    s.config = Arc::new(config);
    s.local_ai = Some(ai);
    s
}

async fn desk(
    state: &AppState,
    method: &str,
    uri: &str,
    cookie: &str,
    body: Option<Value>,
) -> TestResponse {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, HOST)
        .header("x-forwarded-for", "127.0.0.1")
        .header(header::ACCEPT, "application/json")
        .header(header::COOKIE, cookie);
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .expect("request"),
        None => {
            b = b.header(header::CONTENT_LENGTH, "0");
            b.body(Body::empty()).expect("request")
        }
    };
    send_with_state(state.clone(), req).await
}

async fn cloud(
    state: &AppState,
    method: &str,
    uri: &str,
    body: Value,
    cookie: &str,
) -> TestResponse {
    send_with_state(
        state.clone(),
        authed_json_request(method, uri, body, Some(cookie)),
    )
    .await
}

async fn sync_now(desktop: &AppState, cookie: &str) {
    let res = desk(desktop, "POST", "/api/sync/now", cookie, None).await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
}

async fn uid(pool: &PgPool, sql: &str, bind: Uuid) -> Option<Uuid> {
    sqlx::query_scalar(sql)
        .bind(bind)
        .fetch_optional(pool)
        .await
        .expect("query")
}

#[sqlx::test]
async fn a_desktop_and_the_cloud_keep_the_same_offices(pool: PgPool) {
    // The cloud, serving on a real port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let mut cloud_state = common::state(pool.clone());
    let mut config: Config = (*cloud_state.config).clone();
    config.app_base_url = format!("http://{addr}");
    cloud_state.config = Arc::new(config);
    let app = airobot::app(cloud_state.clone());
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .ok();
    });
    let cloud_url = format!("http://{addr}");

    // The person in the cloud, with an office and a contact there.
    let me = sign_in(&cloud_state, "sam@example.com", "sam").await;
    let res = cloud(
        &cloud_state,
        "POST",
        "/api/offices",
        json!({ "name": "Shop" }),
        &me,
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{:?}", res.body);
    let shop: Uuid = res.body["office"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let res = cloud(
        &cloud_state,
        "POST",
        &format!("/api/offices/{shop}/contacts"),
        json!({ "name": "Ana Lee", "email": "ana@example.com" }),
        &me,
    )
    .await;
    assert!(res.status.is_success(), "{:?}", res.body);
    let ana: Uuid = res.body["id"].as_str().expect("id").parse().expect("uuid");
    let sam: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE handle = 'sam'")
        .fetch_one(&pool)
        .await
        .expect("sam");

    // The desktop, with an office, a room with messages, and a record.
    let (desk_pool, admin, db_name) = desktop_database().await;
    let desktop = desktop_state(desk_pool.clone());
    let entered = send_with_state(
        desktop.clone(),
        Request::builder()
            .uri(format!("/local/enter?token={TOKEN}"))
            .header(header::HOST, HOST)
            .header("x-forwarded-for", "127.0.0.1")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let dc = session_cookie(&entered.headers).expect("desktop session");
    let res = desk(
        &desktop,
        "POST",
        "/api/offices",
        &dc,
        Some(json!({ "name": "Bakery" })),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{:?}", res.body);
    let bakery: Uuid = res.body["office"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let res = desk(
        &desktop,
        "POST",
        "/threads",
        &dc,
        Some(json!({ "title": "Orders", "workspace_id": bakery })),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{:?}", res.body);
    let orders: Uuid = res.body["id"].as_str().expect("id").parse().expect("uuid");
    let res = desk(
        &desktop,
        "POST",
        &format!("/threads/{orders}/notes"),
        &dc,
        Some(json!({ "body": "Twelve croissants for Friday", "kind": "comment" })),
    )
    .await;
    assert!(res.status.is_success(), "{:?}", res.body);
    let res = desk(
        &desktop,
        "POST",
        &format!("/api/office/apps/install?office={bakery}"),
        &dc,
        Some(json!({ "kind": "crm" })),
    )
    .await;
    assert!(res.status.is_success(), "{:?}", res.body);
    let (app, kind): (Uuid, String) = sqlx::query_as(
        "SELECT id, slug FROM office_apps WHERE workspace_id = $1 AND deleted_at IS NULL ORDER BY created_at LIMIT 1",
    )
    .bind(bakery)
    .fetch_one(&desk_pool)
    .await
    .expect("app");
    let record: Uuid = sqlx::query_scalar(
        "INSERT INTO office_records (workspace_id, app_id, record_type, status, data)
         VALUES ($1, $2, 'lead', 'new', '{\"name\": \"Bea\"}') RETURNING id",
    )
    .bind(bakery)
    .bind(app)
    .fetch_one(&desk_pool)
    .await
    .expect("record");
    let _ = kind;
    let local_owner: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(airobot::local::LOCAL_OWNER_EMAIL)
        .fetch_one(&desk_pool)
        .await
        .expect("owner");
    // A document and a memory of the office.
    let res = desk(
        &desktop,
        "POST",
        &format!("/api/offices/{bakery}/documents"),
        &dc,
        Some(json!({ "text": "Croissants are 3 euros. Sourdough is 6 euros." })),
    )
    .await;
    assert!(res.status.is_success(), "{:?}", res.body);
    let prices: Uuid = sqlx::query_scalar("SELECT id FROM documents WHERE workspace_id = $1")
        .bind(bakery)
        .fetch_one(&desk_pool)
        .await
        .expect("document");
    sqlx::query("INSERT INTO memories (workspace_id, body, source) VALUES ($1, 'Closed on Mondays', 'owner')")
        .bind(bakery)
        .execute(&desk_pool)
        .await
        .expect("memory");
    // A bot only the desktop has, answering in Orders.
    let (_, baker_secret, baker_prefix) = airobot::auth::generate_agent_key(Uuid::new_v4());
    let baker: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (user_id, name, handle, key_hash, key_prefix, workspace_id)
         VALUES ($1, 'Baker', 'baker', $2, $3, $4) RETURNING id",
    )
    .bind(local_owner)
    .bind(secret::hash_secret(&baker_secret).expect("hash"))
    .bind(baker_prefix)
    .bind(bakery)
    .fetch_one(&desk_pool)
    .await
    .expect("baker");
    sqlx::query("INSERT INTO thread_agents (thread_id, agent_id) VALUES ($1, $2)")
        .bind(orders)
        .bind(baker)
        .execute(&desk_pool)
        .await
        .expect("baker answers in Orders");
    // A bot's AI tool here is this computer's business.
    sqlx::query("UPDATE agents SET ai_engine = 'codex', ai_model = 'gpt-local' WHERE user_id = $1")
        .bind(local_owner)
        .execute(&desk_pool)
        .await
        .expect("engine");

    // Linking: a code on the desktop, allowed by the person in the cloud.
    let res = desk(&desktop, "GET", "/api/sync/status", &dc, None).await;
    assert_eq!(res.body["available"], true);
    assert_eq!(res.body["linked"], false);
    let res = desk(
        &desktop,
        "POST",
        "/api/sync/connect",
        &dc,
        Some(json!({ "cloud_url": cloud_url, "name": "Sam's laptop" })),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    let code = res.body["code"].as_str().expect("code").to_string();
    assert!(res.body["link_url"]
        .as_str()
        .is_some_and(|u| u.contains("/settings?sync_code=")));
    let res = desk(&desktop, "GET", "/api/sync/status", &dc, None).await;
    assert_eq!(res.body["linked"], false);
    assert_eq!(res.body["pending"]["code"], code.as_str());

    // The bot sign-in flow can't redeem a sync code, and a bot can't allow one.
    let raw_code = code.replace('-', "");
    let device_code: String = {
        let row: (Uuid,) = sqlx::query_as("SELECT id FROM login_codes WHERE code = $1")
            .bind(&raw_code)
            .fetch_one(&pool)
            .await
            .expect("code row");
        row.0.to_string()
    };
    let _ = device_code;
    let shown = cloud(
        &cloud_state,
        "GET",
        &format!("/api/sync/link?code={code}"),
        json!({}),
        &me,
    )
    .await;
    assert_eq!(shown.status, StatusCode::OK, "{:?}", shown.body);
    assert_eq!(shown.body["name"], "Sam's laptop");
    let (raw_bot_key, raw_bot_secret, bot_prefix) =
        airobot::auth::generate_agent_key(Uuid::new_v4());
    let bot_id: Uuid = raw_bot_key
        .trim_start_matches("air_ag_")
        .split('.')
        .next()
        .expect("id")
        .parse()
        .expect("uuid");
    sqlx::query(
        "INSERT INTO agents (id, user_id, name, handle, key_hash, key_prefix)
         VALUES ($1, $2, 'Helper', 'sam-helper', $3, $4)",
    )
    .bind(bot_id)
    .bind(sam)
    .bind(secret::hash_secret(&raw_bot_secret).expect("hash"))
    .bind(bot_prefix)
    .execute(&pool)
    .await
    .expect("helper");
    let res = send_with_state(
        cloud_state.clone(),
        bearer_json_request(
            "POST",
            "/api/sync/link/approve",
            json!({ "code": code }),
            &raw_bot_key,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    let res = cloud(
        &cloud_state,
        "POST",
        "/api/sync/link/approve",
        json!({ "code": code }),
        &me,
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    let res = desk(&desktop, "GET", "/api/sync/status", &dc, None).await;
    assert_eq!(res.body["linked"], true, "{:?}", res.body);
    assert_eq!(res.body["cloud_handle"], "sam");
    assert!(airobot::sync::leaves_jobs_to_cloud(&desktop).await);

    // The first sync: each side gets the other's offices.
    sync_now(&desktop, &dc).await;
    let bakery_owner = uid(
        &pool,
        "SELECT owner_user_id FROM workspaces WHERE id = $1",
        bakery,
    )
    .await;
    assert_eq!(
        bakery_owner,
        Some(sam),
        "the desktop's office is in the cloud, as Sam's"
    );
    let croissants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notes WHERE thread_id = $1 AND body = 'Twelve croissants for Friday'",
    )
    .bind(orders)
    .fetch_one(&pool)
    .await
    .expect("notes");
    assert_eq!(croissants, 1);
    let author = uid(
        &pool,
        "SELECT author_user_id FROM notes WHERE thread_id = $1 AND body = 'Twelve croissants for Friday'",
        orders,
    )
    .await;
    assert_eq!(author, Some(sam));
    let bea: Option<String> =
        sqlx::query_scalar("SELECT data->>'name' FROM office_records WHERE id = $1")
            .bind(record)
            .fetch_optional(&pool)
            .await
            .expect("record");
    assert_eq!(bea.as_deref(), Some("Bea"));
    let passages: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_chunks WHERE document_id = $1 AND search @@ plainto_tsquery('english', 'sourdough')",
    )
    .bind(prices)
    .fetch_one(&pool)
    .await
    .expect("chunks");
    assert!(passages >= 1, "the cloud can search the synced document");
    let memory: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memories WHERE workspace_id = $1 AND body = 'Closed on Mondays'",
    )
    .bind(bakery)
    .fetch_one(&pool)
    .await
    .expect("memories");
    assert_eq!(memory, 1);
    let shop_owner = uid(
        &desk_pool,
        "SELECT owner_user_id FROM workspaces WHERE id = $1",
        shop,
    )
    .await;
    assert_eq!(
        shop_owner,
        Some(local_owner),
        "the cloud's office is on the desktop"
    );
    let ana_here: Option<String> = sqlx::query_scalar("SELECT name FROM contacts WHERE id = $1")
        .bind(ana)
        .fetch_optional(&desk_pool)
        .await
        .expect("contact");
    assert_eq!(ana_here.as_deref(), Some("Ana Lee"));
    // The person's Architect is one bot, mapped; private choices stayed home.
    let architects: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM agents WHERE user_id = $1 AND name = 'Architect' AND deleted_at IS NULL",
    )
    .bind(sam)
    .fetch_one(&pool)
    .await
    .expect("architects");
    assert_eq!(architects, 1);
    let mapped: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sync_id_map WHERE table_name = 'agents'")
            .fetch_one(&desk_pool)
            .await
            .expect("map");
    assert!(mapped >= 1);
    let leaked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM agents WHERE ai_engine = 'codex' OR ai_model = 'gpt-local'",
    )
    .fetch_one(&pool)
    .await
    .expect("agents");
    assert_eq!(leaked, 0);
    let baker_key: Option<String> =
        sqlx::query_scalar("SELECT key_hash FROM agents WHERE id = $1 AND user_id = $2")
            .bind(baker)
            .bind(sam)
            .fetch_optional(&pool)
            .await
            .expect("baker");
    assert_eq!(
        baker_key.as_deref(),
        Some("sync:no-key"),
        "a bot made by sync has no key"
    );
    let answers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_agents WHERE thread_id = $1 AND agent_id = $2",
    )
    .bind(orders)
    .bind(baker)
    .fetch_one(&pool)
    .await
    .expect("thread agents");
    assert_eq!(answers, 1);

    // The newest edit wins: the cloud's later edit, then the desktop's.
    sqlx::query(
        "UPDATE office_records SET data = '{\"name\": \"Bea (cloud)\"}', updated_at = now() + interval '1 second' WHERE id = $1",
    )
    .bind(record)
    .execute(&pool)
    .await
    .expect("cloud edit");
    sqlx::query("UPDATE office_records SET data = '{\"name\": \"Bea (desk)\"}', updated_at = now() WHERE id = $1")
        .bind(record)
        .execute(&desk_pool)
        .await
        .expect("desk edit");
    sync_now(&desktop, &dc).await;
    for p in [&pool, &desk_pool] {
        let name: String =
            sqlx::query_scalar("SELECT data->>'name' FROM office_records WHERE id = $1")
                .bind(record)
                .fetch_one(p)
                .await
                .expect("record");
        assert_eq!(name, "Bea (cloud)");
    }
    sqlx::query(
        "UPDATE office_records SET data = '{\"name\": \"Bea Smith\"}', updated_at = now() + interval '5 seconds' WHERE id = $1",
    )
    .bind(record)
    .execute(&desk_pool)
    .await
    .expect("desk edit");
    sync_now(&desktop, &dc).await;
    let name: String = sqlx::query_scalar("SELECT data->>'name' FROM office_records WHERE id = $1")
        .bind(record)
        .fetch_one(&pool)
        .await
        .expect("record");
    assert_eq!(name, "Bea Smith");

    // A message written in the cloud arrives; a deletion travels.
    let res = cloud(
        &cloud_state,
        "POST",
        &format!("/threads/{orders}/notes"),
        json!({ "body": "Make it fourteen", "kind": "comment" }),
        &me,
    )
    .await;
    assert!(res.status.is_success(), "{:?}", res.body);
    sqlx::query("UPDATE contacts SET deleted_at = now(), updated_at = now() + interval '1 second' WHERE id = $1")
        .bind(ana)
        .execute(&pool)
        .await
        .expect("delete");
    sync_now(&desktop, &dc).await;
    let fourteen: Option<Uuid> = sqlx::query_scalar(
        "SELECT author_user_id FROM notes WHERE thread_id = $1 AND body = 'Make it fourteen'",
    )
    .bind(orders)
    .fetch_optional(&desk_pool)
    .await
    .expect("notes");
    assert_eq!(fourteen, Some(local_owner));
    let gone: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM contacts WHERE id = $1")
            .bind(ana)
            .fetch_one(&desk_pool)
            .await
            .expect("contact");
    assert!(gone);

    // The key: only the sync endpoint takes it, and only for Sam's offices.
    let key = sqlx::query_scalar::<_, String>(
        "SELECT key_encrypted FROM sync_links WHERE deleted_at IS NULL",
    )
    .fetch_one(&desk_pool)
    .await
    .ok()
    .and_then(|k| airobot::crypto::decrypt(&k))
    .expect("key");
    let res = send_with_state(
        cloud_state.clone(),
        bearer_get_request("/api/offices", &key),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    let other = sign_in(&cloud_state, "kim@example.com", "kim").await;
    let res = cloud(
        &cloud_state,
        "POST",
        "/api/offices",
        json!({ "name": "Kim's" }),
        &other,
    )
    .await;
    let kims: Uuid = res.body["office"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let kim_chat: Uuid =
        sqlx::query_scalar("SELECT id FROM threads WHERE workspace_id = $1 AND kind = 'office'")
            .bind(kims)
            .fetch_one(&pool)
            .await
            .expect("chat");
    let forged = json!({
        "changes": {
            "workspaces": [{ "id": kims, "name": "Taken", "slug": "taken", "owner_user_id": sam,
                             "utc_offset_minutes": 0, "daily_plan": false, "paused": false,
                             "check_in": "off", "weekly_report": false,
                             "created_at": "2026-01-01T00:00:00Z", "updated_at": "2030-01-01T00:00:00Z" }],
            "notes": [{ "id": Uuid::new_v4(), "thread_id": kim_chat, "author_user_id": sam,
                        "body": "planted", "kind": "comment",
                        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z" }],
        },
        "cursors": {},
    });
    let res = send_with_state(
        cloud_state.clone(),
        bearer_json_request("POST", "/api/sync", forged, &key),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    assert_eq!(res.body["applied"], 0);
    let kim_name: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id = $1")
        .bind(kims)
        .fetch_one(&pool)
        .await
        .expect("kim");
    assert_eq!(kim_name, "Kim's");
    let planted: i64 = sqlx::query_scalar("SELECT count(*) FROM notes WHERE body = 'planted'")
        .fetch_one(&pool)
        .await
        .expect("notes");
    assert_eq!(planted, 0);
    let res = cloud(&cloud_state, "GET", "/api/sync/computers", json!({}), &me).await;
    assert_eq!(res.body["computers"][0]["name"], "Sam's laptop");

    // Stopping on the desktop retires the key in the cloud.
    let res = desk(&desktop, "POST", "/api/sync/disconnect", &dc, None).await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    assert_eq!(res.body["linked"], false);
    assert!(!airobot::sync::leaves_jobs_to_cloud(&desktop).await);
    let res = send_with_state(
        cloud_state.clone(),
        bearer_json_request(
            "POST",
            "/api/sync",
            json!({ "changes": {}, "cursors": {} }),
            &key,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);

    desk_pool.close().await;
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {db_name} WITH (FORCE)"))
        .execute(&admin)
        .await;
}

#[sqlx::test]
async fn the_cloud_side_needs_a_person_and_the_desktop_side_a_desktop(pool: PgPool) {
    let state = common::state(pool.clone());
    let me = sign_in(&state, "lee@example.com", "lee").await;
    // A server that isn't a desktop has nothing to link from.
    let res = send_with_state(
        state.clone(),
        authed_json_request(
            "POST",
            "/api/sync/connect",
            json!({ "cloud_url": "https://airo-bot.com" }),
            Some(&me),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    let res = send_with_state(
        state.clone(),
        authed_json_request("GET", "/api/sync/status", json!({}), Some(&me)),
    )
    .await;
    assert_eq!(res.body["available"], false);
    // Unknown and missing keys are refused.
    let res = send_with_state(
        state.clone(),
        bearer_json_request(
            "POST",
            "/api/sync",
            json!({}),
            "air_sync_00000000-0000-0000-0000-000000000000.nope",
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    // A link code is made without signing in, and expires unused.
    let res = send_with_state(
        state.clone(),
        common::json_request("POST", "/api/sync/link", json!({ "name": "Laptop" })),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{:?}", res.body);
    let device_code = res.body["device_code"]
        .as_str()
        .expect("device code")
        .to_string();
    let res = send_with_state(
        state.clone(),
        common::json_request(
            "POST",
            "/api/sync/link/token",
            json!({ "device_code": device_code }),
        ),
    )
    .await;
    assert_eq!(res.body["status"], "pending");
    // The bot sign-in flow doesn't hand out keys for it.
    let res = send_with_state(
        state.clone(),
        common::json_request(
            "POST",
            "/device/token",
            json!({ "device_code": device_code }),
        ),
    )
    .await;
    assert_eq!(res.body["status"], "expired");
}
