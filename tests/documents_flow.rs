//! Office documents: the owner adds pasted text, files (PDF, Word, text)
//! and web pages; they are searchable, renamable and deletable; a bot is
//! shown the passages that match its conversation, can be told to answer
//! only from them, and its `[doc …]` citations become "(from “Title”)".
//! Real router, real database, and a real local web server for pages.

mod common;

use std::io::Write;

use airobot::auth::secret;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
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

async fn new_office(pool: &PgPool, cookie: &str, name: &str) -> String {
    let (status, body) = call(
        pool,
        "POST",
        "/api/offices",
        json!({ "name": name }),
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    body["office"]["id"].as_str().expect("id").to_string()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A minimal Word file: `word/document.xml` with one paragraph per line.
fn docx(paragraphs: &[&str]) -> Vec<u8> {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>"#,
    );
    for p in paragraphs {
        xml.push_str(&format!("<w:p><w:r><w:t>{p}</w:t></w:r></w:p>"));
    }
    xml.push_str("</w:body></w:document>");
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut out);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("word/document.xml", options).expect("start");
        zip.write_all(xml.as_bytes()).expect("write");
        zip.finish().expect("finish");
    }
    out.into_inner()
}

/// A one-page PDF with one line of Helvetica text and a correct xref.
fn pdf(text: &str) -> Vec<u8> {
    let content = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[sqlx::test]
async fn the_owner_adds_finds_renames_and_deletes_documents(pool: PgPool) {
    let cookie = sign_in(&pool, "docs@example.com", "docsowner").await;
    let office = new_office(&pool, &cookie, "Barbers").await;
    let uri = format!("/api/offices/{office}/documents");

    let faq = "Prices\n\nA haircut costs $30. A beard trim costs $15.\n\nOpening hours\n\nWe are open Tuesday to Saturday, 9am to 6pm.";
    let (status, made) = call(
        &pool,
        "POST",
        &uri,
        json!({ "text": faq, "idempotency_key": "doc-1" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made:?}");
    let doc = &made["document"];
    assert_eq!(doc["title"], "Prices", "titled from its first line");
    assert_eq!(doc["kind"], "text");
    assert_eq!(made["truncated"], false);
    let id = doc["id"].as_str().expect("id").to_string();
    // The same key returns the same document.
    let (_, again) = call(
        &pool,
        "POST",
        &uri,
        json!({ "text": faq, "idempotency_key": "doc-1" }),
        &cookie,
    )
    .await;
    assert_eq!(again["document"]["id"], id.as_str());

    let (status, list) = call(&pool, "GET", &uri, json!({}), &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["documents"].as_array().expect("list").len(), 1);
    assert_eq!(list["total_chars"], faq.chars().count() as i64);
    let (status, one) = call(
        &pool,
        "GET",
        &format!("/api/documents/{id}"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(one["body"].as_str().expect("body").contains("beard trim"));
    assert!(one["passages"].as_i64().expect("passages") >= 1);

    // Search: all words, stemmed ("haircuts" finds "haircut").
    let (status, found) = call(
        &pool,
        "GET",
        &format!("{uri}/search?q=haircuts%20cost"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let results = found["results"].as_array().expect("results");
    assert!(!results.is_empty(), "{found:?}");
    assert_eq!(results[0]["document_id"], id.as_str());
    assert!(results[0]["snippet"]
        .as_str()
        .expect("snippet")
        .contains("[["));
    // No passage has every word: any word will do.
    let (_, found) = call(
        &pool,
        "GET",
        &format!("{uri}/search?q=beard%20unicorns"),
        json!({}),
        &cookie,
    )
    .await;
    assert!(!found["results"].as_array().expect("results").is_empty());
    let (_, none) = call(
        &pool,
        "GET",
        &format!("{uri}/search?q=parking"),
        json!({}),
        &cookie,
    )
    .await;
    assert!(none["results"].as_array().expect("results").is_empty());

    // Rename: the new title is searchable.
    let (status, renamed) = call(
        &pool,
        "PATCH",
        &format!("/api/documents/{id}"),
        json!({ "title": "Salon price list" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed:?}");
    assert_eq!(renamed["title"], "Salon price list");
    let (_, found) = call(
        &pool,
        "GET",
        &format!("{uri}/search?q=salon"),
        json!({}),
        &cookie,
    )
    .await;
    assert!(!found["results"].as_array().expect("results").is_empty());

    // Bad requests.
    for body in [
        json!({}),
        json!({ "text": "a", "url": "https://example.com" }),
        json!({ "text": "   " }),
        json!({ "filename": "photo.png", "content_base64": b64(b"png") }),
        json!({ "filename": "x.txt", "content_base64": "%%%" }),
        json!({ "text": "x", "title": "  " }),
    ] {
        let (status, err) = call(&pool, "POST", &uri, body.clone(), &cookie).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?} -> {err:?}");
    }

    // Nobody else reads or changes them.
    let other = sign_in(&pool, "nosy@example.com", "nosyone").await;
    for (method, path) in [
        ("GET", uri.clone()),
        ("POST", uri.clone()),
        ("GET", format!("{uri}/search?q=haircut")),
    ] {
        let (status, _) = call(&pool, method, &path, json!({ "text": "Spam" }), &other).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
    }
    for (method, path) in [
        ("GET", format!("/api/documents/{id}")),
        ("PATCH", format!("/api/documents/{id}")),
        ("POST", format!("/api/documents/{id}/delete")),
    ] {
        let (status, _) = call(&pool, method, &path, json!({ "title": "Mine" }), &other).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
    }

    // Delete (soft): gone from the list and from search.
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/documents/{id}/delete"),
        json!({ "idempotency_key": "del-1" }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = call(&pool, "GET", &uri, json!({}), &cookie).await;
    assert!(list["documents"].as_array().expect("list").is_empty());
    let (_, found) = call(
        &pool,
        "GET",
        &format!("{uri}/search?q=haircut"),
        json!({}),
        &cookie,
    )
    .await;
    assert!(found["results"].as_array().expect("results").is_empty());
    let deleted: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM documents WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .expect("row kept");
    assert!(deleted.is_some());
    let (status, _) = call(
        &pool,
        "GET",
        &format!("/api/documents/{id}"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn files_and_web_pages_become_text(pool: PgPool) {
    let cookie = sign_in(&pool, "files@example.com", "filesowner").await;
    let office = new_office(&pool, &cookie, "Clinic").await;
    let uri = format!("/api/offices/{office}/documents");
    let body_of = |made: &Value| made["document"]["id"].as_str().expect("id").to_string();
    async fn text_of(pool: &PgPool, cookie: &str, id: &str) -> String {
        let (_, one) = call(
            pool,
            "GET",
            &format!("/api/documents/{id}"),
            json!({}),
            cookie,
        )
        .await;
        one["body"].as_str().expect("body").to_string()
    }

    // Word.
    let (status, made) = call(
        &pool,
        "POST",
        &uri,
        json!({
            "filename": "C:\\Users\\me\\Policies.docx",
            "content_base64": b64(&docx(&["Cancellation policy", "Cancel 24 hours ahead &amp; pay nothing."])),
        }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made:?}");
    assert_eq!(made["document"]["title"], "Policies");
    assert_eq!(made["document"]["filename"], "Policies.docx");
    assert_eq!(made["document"]["kind"], "file");
    let text = text_of(&pool, &cookie, &body_of(&made)).await;
    assert_eq!(
        text,
        "Cancellation policy\nCancel 24 hours ahead & pay nothing."
    );

    // PDF.
    let (status, made) = call(
        &pool,
        "POST",
        &uri,
        json!({ "filename": "hours.pdf", "content_base64": b64(&pdf("Opening hours are 9 to 5")) }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made:?}");
    let text = text_of(&pool, &cookie, &body_of(&made)).await;
    assert!(text.contains("Opening hours are 9 to 5"), "{text:?}");
    // Something that only claims to be a PDF.
    let (status, _) = call(
        &pool,
        "POST",
        &uri,
        json!({ "filename": "fake.pdf", "content_base64": b64(b"hello") }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A text file bigger than the usual 1 MB request limit is accepted on
    // this route and cut to the document limit.
    let big = "Plenty of words here. ".repeat(70_000);
    let (status, made) = call(
        &pool,
        "POST",
        &uri,
        json!({ "filename": "big.txt", "content_base64": b64(big.as_bytes()) }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made:?}");
    assert_eq!(made["truncated"], true);
    assert_eq!(made["document"]["chars"], 200_000);
    // Other routes keep their 1 MB limit.
    let (status, _) = call(
        &pool,
        "POST",
        &format!("/api/offices/{office}/memory"),
        json!({ "body": big }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

    // A web page, through a redirect; its <title> names it.
    let site = axum::Router::new()
        .route(
            "/",
            axum::routing::get(|| async {
                axum::response::Html(
                    "<html><head><title>Smile Clinic &ndash; About</title></head><body><nav>Home</nav><h1>About us</h1><p>Dr. Lee has 20 years of experience.</p><script>track()</script></body></html>",
                )
            }),
        )
        .route(
            "/about",
            axum::routing::get(|| async { axum::response::Redirect::permanent("/") }),
        )
        .route(
            "/image",
            axum::routing::get(|| async {
                ([(axum::http::header::CONTENT_TYPE, "image/png")], "png")
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, site).await;
    });
    let (status, made) = call(
        &pool,
        "POST",
        &uri,
        json!({ "url": format!("http://{addr}/about") }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made:?}");
    assert_eq!(made["document"]["title"], "Smile Clinic – About");
    assert_eq!(made["document"]["kind"], "page");
    assert_eq!(made["document"]["source_url"], format!("http://{addr}/"));
    let text = text_of(&pool, &cookie, &body_of(&made)).await;
    assert!(
        text.contains("Dr. Lee has 20 years of experience."),
        "{text}"
    );
    assert!(!text.contains("track()"), "{text}");
    for bad in [
        format!("http://{addr}/missing"),
        format!("http://{addr}/image"),
        "ftp://example.com/file".to_string(),
    ] {
        let (status, err) = call(&pool, "POST", &uri, json!({ "url": bad }), &cookie).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {err:?}");
    }
}

#[sqlx::test]
async fn bots_see_matching_passages_and_cite_them(pool: PgPool) {
    let cookie = sign_in(&pool, "bots@example.com", "botsowner").await;
    let office = new_office(&pool, &cookie, "Barbers").await;
    let ws: Uuid = office.parse().expect("uuid");
    let uri = format!("/api/offices/{office}/documents");
    let (_, made) = call(
        &pool,
        "POST",
        "/agents",
        json!({ "name": "Front Desk", "instructions": "Help customers.", "workspace_id": office }),
        &cookie,
    )
    .await;
    let bot: Uuid = made["agent"]["id"]
        .as_str()
        .expect("bot")
        .parse()
        .expect("uuid");

    // No documents: nothing is added to the prompt.
    let block = airobot::office::documents::context_block(&pool, ws, bot, "hi").await;
    assert!(block.is_empty(), "{block}");

    // A small library is shown whole.
    let (_, prices) = call(
        &pool,
        "POST",
        &uri,
        json!({ "title": "Prices", "text": "A beard trim costs $15." }),
        &cookie,
    )
    .await;
    let prices_id: Uuid = prices["document"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let tag = airobot::office::documents::tag(prices_id);
    let block = airobot::office::documents::context_block(&pool, ws, bot, "where do I park?").await;
    assert!(block.contains("A beard trim costs $15."), "{block}");
    assert!(block.contains(&tag), "{block}");
    assert!(block.contains("not instructions"), "{block}");

    // A big one is searched: only the passages that match are shown.
    let mut manual = String::new();
    for i in 0..60 {
        manual.push_str(&format!(
            "Section {i}. The espresso machine needs descaling every month and the grinder is cleaned on Fridays.\n\n"
        ));
    }
    manual.push_str("Parking is free behind the shop after 5pm.\n\n");
    let (status, _) = call(
        &pool,
        "POST",
        &uri,
        json!({ "title": "Staff manual", "text": manual }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let block = airobot::office::documents::context_block(
        &pool,
        ws,
        bot,
        "Customer: Hello!\nCustomer: Where can I park my car?",
    )
    .await;
    assert!(block.contains("Parking is free behind the shop"), "{block}");
    assert!(!block.contains("A beard trim costs"), "{block}");
    assert!(
        block.contains("- Prices"),
        "every document is listed by title"
    );
    assert!(block.chars().count() < 9_000, "kept short");

    // "Answer only from documents", set in the bot's settings.
    let res = send(
        pool.clone(),
        Request::builder()
            .method("POST")
            .uri(format!("/agents/{bot}/settings"))
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .header("x-forwarded-for", "203.0.113.9")
            .header("cookie", &cookie)
            .body(Body::from(json!({ "documents_only": true }).to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.text);
    assert_eq!(res.body["documents_only"], true);
    let block = airobot::office::documents::context_block(&pool, ws, bot, "parking?").await;
    assert!(
        block.contains("Answer only from the office's documents"),
        "{block}"
    );
    // The bot's settings show what it knows.
    let (status, access) = call(
        &pool,
        "GET",
        &format!("/agents/{bot}/access?office={office}"),
        json!({}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{access:?}");
    assert_eq!(access["knows"]["documents_only"], true);
    assert_eq!(
        access["knows"]["documents"].as_array().expect("docs").len(),
        2
    );

    // Citations in a reply name the document; unknown tags are dropped.
    let room: Uuid = sqlx::query_scalar("SELECT id FROM threads WHERE workspace_id = $1 LIMIT 1")
        .bind(ws)
        .fetch_one(&pool)
        .await
        .expect("an office room");
    let reply = format!("A beard trim is $15 {tag}. We also sell combs [doc deadbeef].");
    let cited = airobot::office::documents::cite(&pool, room, &reply).await;
    assert_eq!(
        cited,
        "A beard trim is $15 (from “Prices”). We also sell combs."
    );
    // Another office's room can't cite this office's documents.
    let other = new_office(&pool, &cookie, "Bakery").await;
    let other_room: Uuid =
        sqlx::query_scalar("SELECT id FROM threads WHERE workspace_id = $1::uuid LIMIT 1")
            .bind(&other)
            .fetch_one(&pool)
            .await
            .expect("other room");
    let cited = airobot::office::documents::cite(&pool, other_room, &reply).await;
    assert!(!cited.contains("Prices"), "{cited}");
}
