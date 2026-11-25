// Google Drive folders as office documents (issue #34): a linked folder's
// files become documents and are kept in step (new, changed, removed),
// files without text are skipped, and a failure is told to the owner once.
// A small local server plays the Drive API.
mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use airobot::auth::secret;
use airobot::connectors::ConnectorApps;
use airobot::office::documents;
use airobot::state::AppState;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use chrono::{Duration, Utc};
use common::{
    authed_json_request, form_request, get_request, send, send_with_state, session_cookie,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const FOLDER: &str = "folder_1234567890";
const TOKEN: &str = "drive-token";

#[derive(Clone)]
struct FakeFile {
    name: String,
    mime: String,
    modified: String,
    content: String,
}

#[derive(Default)]
struct FakeDrive {
    files: BTreeMap<String, FakeFile>,
    broken: bool,
}

type Drive = Arc<Mutex<FakeDrive>>;

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == format!("Bearer {TOKEN}"))
}

async fn fake_drive(drive: Drive) -> String {
    let list = {
        let drive = drive.clone();
        move |headers: HeaderMap, Query(q): Query<BTreeMap<String, String>>| {
            let drive = drive.clone();
            async move {
                let d = drive.lock().expect("drive");
                if !authorized(&headers) {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                if d.broken {
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
                assert!(q["q"].contains(&format!("'{FOLDER}' in parents")), "{q:?}");
                let files: Vec<Value> = d
                    .files
                    .iter()
                    .map(|(id, f)| {
                        json!({
                            "id": id, "name": f.name, "mimeType": f.mime,
                            "modifiedTime": f.modified,
                            "webViewLink": format!("https://drive.google.com/file/d/{id}/view"),
                            "size": f.content.len().to_string(),
                        })
                    })
                    .collect();
                axum::Json(json!({ "files": files })).into_response()
            }
        }
    };
    let file = {
        let drive = drive.clone();
        move |headers: HeaderMap,
              Path(id): Path<String>,
              Query(q): Query<BTreeMap<String, String>>| {
            let drive = drive.clone();
            async move {
                let d = drive.lock().expect("drive");
                if !authorized(&headers) {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                if id == FOLDER {
                    return axum::Json(json!({
                        "id": FOLDER, "name": "Shop documents",
                        "mimeType": "application/vnd.google-apps.folder"
                    }))
                    .into_response();
                }
                match (d.files.get(&id), q.get("alt").map(String::as_str)) {
                    (Some(f), Some("media")) => f.content.clone().into_response(),
                    (Some(f), _) => {
                        axum::Json(json!({ "id": id, "name": f.name, "mimeType": f.mime }))
                            .into_response()
                    }
                    (None, _) => StatusCode::NOT_FOUND.into_response(),
                }
            }
        }
    };
    let export = {
        let drive = drive.clone();
        move |headers: HeaderMap, Path(id): Path<String>| {
            let drive = drive.clone();
            async move {
                let d = drive.lock().expect("drive");
                if !authorized(&headers) {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                match d.files.get(&id) {
                    Some(f) => f.content.clone().into_response(),
                    None => StatusCode::NOT_FOUND.into_response(),
                }
            }
        }
    };
    let app = axum::Router::new()
        .route("/www.googleapis.com/drive/v3/files", get(list))
        .route("/www.googleapis.com/drive/v3/files/{id}", get(file))
        .route(
            "/www.googleapis.com/drive/v3/files/{id}/export",
            get(export),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    format!("http://{addr}")
}

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
    state: &AppState,
    method: &str,
    uri: &str,
    body: Value,
    cookie: &str,
) -> (StatusCode, Value) {
    let res = send_with_state(
        state.clone(),
        authed_json_request(method, uri, body, Some(cookie)),
    )
    .await;
    (res.status, res.body)
}

fn file(name: &str, mime: &str, modified: &str, content: &str) -> FakeFile {
    FakeFile {
        name: name.into(),
        mime: mime.into(),
        modified: modified.into(),
        content: content.into(),
    }
}

/// A Google connection for the office, with the given scopes and a token.
async fn google_connection(
    pool: &PgPool,
    office: Uuid,
    owner_email: &str,
    scopes: &[&str],
) -> Uuid {
    let owner: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(owner_email)
        .fetch_one(pool)
        .await
        .expect("owner");
    let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
    sqlx::query_scalar(
        "INSERT INTO connections
            (workspace_id, created_by_user_id, provider, label, external_account_id, scopes, access_token_enc)
         VALUES ($1, $2, 'google', 'owner@gmail.com', $3, $4, $5) RETURNING id",
    )
    .bind(office)
    .bind(owner)
    .bind(Uuid::new_v4().to_string())
    .bind(&scopes)
    .bind(airobot::crypto::encrypt(TOKEN).expect("encrypt"))
    .fetch_one(pool)
    .await
    .expect("connection")
}

async fn docs(state: &AppState, cookie: &str, office: Uuid) -> Vec<Value> {
    let (_, body) = call(
        state,
        "GET",
        &format!("/api/offices/{office}/documents"),
        json!({}),
        cookie,
    )
    .await;
    body["documents"].as_array().cloned().unwrap_or_default()
}

#[sqlx::test]
async fn a_drive_folder_becomes_documents_and_stays_in_step(pool: PgPool) {
    let drive: Drive = Arc::new(Mutex::new(FakeDrive::default()));
    {
        let mut d = drive.lock().expect("drive");
        d.files.insert(
            "doc_prices_0001".into(),
            file(
                "Price list",
                "application/vnd.google-apps.document",
                "2026-11-01T10:00:00Z",
                "A haircut costs $30.",
            ),
        );
        d.files.insert(
            "txt_hours_00001".into(),
            file(
                "hours",
                "text/plain",
                "2026-11-01T10:00:00Z",
                "Open 9 to 5, Monday to Saturday.",
            ),
        );
        d.files.insert(
            "img_photo_00001".into(),
            file("shop.png", "image/png", "2026-11-01T10:00:00Z", "\u{89}PNG"),
        );
    }
    let origin = fake_drive(drive.clone()).await;
    let mut state = common::state(pool.clone());
    let mut config = (*state.config).clone();
    config.connectors = ConnectorApps {
        test_origin: Some(origin),
        ..ConnectorApps::default()
    };
    state.config = Arc::new(config);

    let email = "drive@example.com";
    let cookie = sign_in(&pool, email, "driver").await;
    let (_, made) = call(
        &state,
        "POST",
        "/api/offices",
        json!({ "name": "Cut & Co" }),
        &cookie,
    )
    .await;
    let office: Uuid = made["office"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let drive_scope = "https://www.googleapis.com/auth/drive.readonly";
    let conn = google_connection(&pool, office, email, &[drive_scope]).await;
    let link = format!("https://drive.google.com/drive/folders/{FOLDER}?usp=sharing");
    let folders = format!("/api/offices/{office}/document-folders");

    // Linked: two files become documents, the photo is skipped.
    let (status, res) = call(
        &state,
        "POST",
        &folders,
        json!({ "folder": link, "connection_id": conn, "idempotency_key": "link-1" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res:?}");
    assert_eq!(res["folder"]["name"], "Shop documents");
    assert_eq!(res["report"]["added"], 2, "{res:?}");
    assert_eq!(res["report"]["skipped"].as_array().map(Vec::len), Some(1));
    let folder = res["folder"]["id"].as_str().expect("folder").to_string();
    let listed = docs(&state, &cookie, office).await;
    let titles: Vec<&str> = listed.iter().filter_map(|d| d["title"].as_str()).collect();
    assert!(
        titles.contains(&"Price list") && titles.contains(&"hours"),
        "{titles:?}"
    );
    assert!(listed.iter().all(|d| d["kind"] == "drive"));
    // Bots answer from them.
    let told = documents::context_block(&pool, office, Uuid::new_v4(), "haircut price").await;
    assert!(told.contains("A haircut costs $30."), "{told}");

    // The same key gives the same folder; the same folder can't be linked twice.
    let (status, again) = call(
        &state,
        "POST",
        &folders,
        json!({ "folder": link, "connection_id": conn, "idempotency_key": "link-1" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["folder"]["id"], folder.as_str());
    let (status, _) = call(
        &state,
        "POST",
        &folders,
        json!({ "folder": link, "connection_id": conn }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Read again: nothing changed.
    let sync = format!("/api/document-folders/{folder}/sync");
    let (status, res) = call(&state, "POST", &sync, json!({}), &cookie).await;
    assert_eq!(status, StatusCode::OK, "{res:?}");
    assert_eq!(res["report"]["unchanged"], 2);

    // A file changed, another left the folder.
    {
        let mut d = drive.lock().expect("drive");
        let f = d.files.get_mut("doc_prices_0001").expect("doc");
        f.modified = "2026-11-20T10:00:00Z".into();
        f.content = "A haircut costs $35 from December.".into();
        d.files.remove("txt_hours_00001");
    }
    let (_, res) = call(&state, "POST", &sync, json!({}), &cookie).await;
    assert_eq!(res["report"]["updated"], 1, "{res:?}");
    assert_eq!(res["report"]["removed"], 1, "{res:?}");
    let told = documents::context_block(&pool, office, Uuid::new_v4(), "haircut price").await;
    assert!(told.contains("$35") && !told.contains("$30"), "{told}");
    assert_eq!(docs(&state, &cookie, office).await.len(), 1);

    // Drive fails: the documents stay, the owner is told once.
    drive.lock().expect("drive").broken = true;
    let (status, err) = call(&state, "POST", &sync, json!({}), &cookie).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err:?}");
    call(&state, "POST", &sync, json!({}), &cookie).await;
    let told_owner: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notes n JOIN threads t ON t.id = n.thread_id
         WHERE t.workspace_id = $1 AND t.kind = 'office' AND n.body LIKE '%Shop documents%couldn''t be read again%'",
    )
    .bind(office)
    .fetch_one(&pool)
    .await
    .expect("notes");
    assert_eq!(told_owner, 1);
    assert_eq!(docs(&state, &cookie, office).await.len(), 1);
    let (_, listed) = call(&state, "GET", &folders, json!({}), &cookie).await;
    assert!(listed["folders"][0]["sync_error"].is_string(), "{listed:?}");

    // Fixed, and due again after a day: the background read clears it.
    drive.lock().expect("drive").broken = false;
    sqlx::query(
        "UPDATE document_folders SET synced_at = now() - interval '25 hours' WHERE id = $1::uuid",
    )
    .bind(&folder)
    .execute(&pool)
    .await
    .expect("clock");
    assert!(documents::sync_due(&state).await.expect("tick") >= 1);
    let (_, listed) = call(&state, "GET", &folders, json!({}), &cookie).await;
    assert!(listed["folders"][0]["sync_error"].is_null(), "{listed:?}");

    // Nobody else reads or syncs it.
    let other = sign_in(&pool, "nosy@example.com", "nosy").await;
    let (status, _) = call(&state, "POST", &sync, json!({}), &other).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&state, "GET", &folders, json!({}), &other).await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN,
        "{status}"
    );

    // Unlinked: its documents go too.
    let (status, res) = call(
        &state,
        "POST",
        &format!("/api/document-folders/{folder}/delete"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res:?}");
    assert_eq!(res["documents_removed"], 1);
    assert!(docs(&state, &cookie, office).await.is_empty());
}

#[sqlx::test]
async fn a_folder_needs_drive_permission_and_a_real_folder(pool: PgPool) {
    let drive: Drive = Arc::new(Mutex::new(FakeDrive::default()));
    let origin = fake_drive(drive).await;
    let mut state = common::state(pool.clone());
    let mut config = (*state.config).clone();
    config.connectors = ConnectorApps {
        test_origin: Some(origin),
        ..ConnectorApps::default()
    };
    state.config = Arc::new(config);
    let email = "perm@example.com";
    let cookie = sign_in(&pool, email, "perm").await;
    let (_, made) = call(
        &state,
        "POST",
        "/api/offices",
        json!({ "name": "Perm" }),
        &cookie,
    )
    .await;
    let office: Uuid = made["office"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let folders = format!("/api/offices/{office}/document-folders");

    // A Google connection with only Gmail.
    let gmail = google_connection(
        &pool,
        office,
        email,
        &["https://www.googleapis.com/auth/gmail.readonly"],
    )
    .await;
    let (status, err) = call(
        &state,
        "POST",
        &folders,
        json!({ "folder": FOLDER, "connection_id": gmail }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["error"]
            .as_str()
            .unwrap_or_default()
            .contains("Drive permission"),
        "{err:?}"
    );

    let drive_conn = google_connection(
        &pool,
        office,
        email,
        &["https://www.googleapis.com/auth/drive.readonly"],
    )
    .await;
    for bad in ["https://example.com/x", "not a link"] {
        let (status, _) = call(
            &state,
            "POST",
            &folders,
            json!({ "folder": bad, "connection_id": drive_conn }),
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    // A folder Drive doesn't know.
    let (status, err) = call(
        &state,
        "POST",
        &folders,
        json!({ "folder": "missing_folder_0001", "connection_id": drive_conn }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err:?}");
    // Another office's connection isn't usable here.
    let (_, other_office) = call(
        &state,
        "POST",
        "/api/offices",
        json!({ "name": "Other" }),
        &cookie,
    )
    .await;
    let other_office: Uuid = other_office["office"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let (status, _) = call(
        &state,
        "POST",
        &format!("/api/offices/{other_office}/document-folders"),
        json!({ "folder": FOLDER, "connection_id": drive_conn }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
