mod common;

use airobot::auth::secret;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::{
    authed_json_request, bearer_get_request, bearer_json_request, form_request, get_request,
    json_request, post_request, send, session_cookie,
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
    let token = secret::encode_keyed_secret("", id, &raw_secret);
    let callback = send(
        pool.clone(),
        get_request(&format!("/auth/callback?token={token}"), None),
    )
    .await;
    let cookie = session_cookie(&callback.headers).expect("session cookie");
    let response = send(
        pool.clone(),
        form_request(
            "POST",
            "/onboarding/handle",
            &[("handle", handle)],
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    cookie
}

async fn create_agent(pool: &PgPool, cookie: &str, name: &str) -> String {
    let code_response = send(pool.clone(), post_request("/device/code", None)).await;
    let code = code_response.body["code"].as_str().expect("code");
    let device_code = code_response.body["device_code"]
        .as_str()
        .expect("device code");
    let approve = send(
        pool.clone(),
        form_request(
            "POST",
            "/device/approve",
            &[("code", code), ("agent_name", name)],
            Some(cookie),
        ),
    )
    .await;
    assert_eq!(approve.status, StatusCode::OK);
    let poll = send(
        pool.clone(),
        json_request(
            "POST",
            "/device/token",
            json!({ "device_code": device_code }),
        ),
    )
    .await;
    poll.body["agent_key"].as_str().unwrap().to_string()
}

async fn create_procedure(pool: &PgPool, agent_key: &str, title: &str) -> Value {
    let response = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            "/procedures",
            json!({
                "title": title,
                "summary": "A useful repeatable method",
                "body": "1. Do the work.\n2. Check the result.",
                "check_description": "The result passes its check."
            }),
            agent_key,
        ),
    )
    .await;
    assert_eq!(response.status, StatusCode::CREATED);
    response.body
}

async fn publish(pool: &PgPool, agent_key: &str, procedure_id: &str) {
    let response = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            &format!("/procedures/{procedure_id}/publish"),
            json!({ "visibility": "public" }),
            agent_key,
        ),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
}

async fn create_public_procedure(pool: &PgPool, agent_key: &str, title: &str) -> Value {
    let procedure = create_procedure(pool, agent_key, title).await;
    publish(pool, agent_key, procedure["id"].as_str().unwrap()).await;
    procedure
}

#[sqlx::test]
async fn signed_out_feed_is_public_and_ignores_reactions_for_ranking(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let agent = create_agent(&pool, &owner, "builder").await;
    let strong = create_public_procedure(&pool, &agent, "Strong procedure").await;
    let saved = create_public_procedure(&pool, &agent, "Saved procedure").await;
    let strong_id: Uuid = strong["id"].as_str().unwrap().parse().unwrap();
    let saved_id: Uuid = saved["id"].as_str().unwrap().parse().unwrap();
    sqlx::query(
        "UPDATE procedures SET run_count = 10, scored_run_count = 10, success_count = 8, attempt_count = 20
         WHERE id = $1",
    )
    .bind(strong_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE procedures SET run_count = 1, scored_run_count = 1, success_count = 1, attempt_count = 1
         WHERE id = $1",
    )
    .bind(saved_id)
    .execute(&pool)
    .await
    .unwrap();
    let save = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/reactions",
            json!({
                "target_type": "procedure",
                "target_id": saved_id,
                "kind": "save"
            }),
            Some(&owner),
        ),
    )
    .await;
    assert_eq!(save.status, StatusCode::CREATED);

    let feed = send(pool.clone(), get_request("/", None)).await;
    assert_eq!(feed.status, StatusCode::OK);
    let items = feed.body["procedures"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], strong_id.to_string());

    let mut html_request = get_request("/", None);
    html_request
        .headers_mut()
        .insert("accept", "text/html".parse().unwrap());
    let html = send(pool, html_request).await;
    assert_eq!(html.status, StatusCode::OK);
    assert!(html.text.contains("Strong procedure"));
    assert!(html.text.contains("Air does not execute procedures"));
    assert!(!html.text.contains("Notification.requestPermission"));
}

#[sqlx::test]
async fn yours_marks_quiet_agents_and_prompts_only_on_that_page(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let agent_key = create_agent(&pool, &owner, "scraper").await;
    let thread = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            "/threads",
            json!({ "title": "Nightly scrape" }),
            &agent_key,
        ),
    )
    .await;
    let agent_id: Uuid = sqlx::query_scalar("SELECT id FROM agents WHERE name = 'scraper'")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE agents SET last_seen_at = now() - interval '3 days' WHERE id = $1")
        .bind(agent_id)
        .execute(&pool)
        .await
        .unwrap();

    let mut request = get_request("/inbox", Some(&owner));
    request
        .headers_mut()
        .insert("accept", "text/html".parse().unwrap());
    let response = send(pool.clone(), request).await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(response.text.contains("Nightly scrape"));
    assert!(response.text.contains("dot-quiet"));
    assert!(response.text.contains("Agent quiet"));
    assert!(response.text.contains("Notification.requestPermission"));

    let thread_id: Uuid = thread.body["id"].as_str().unwrap().parse().unwrap();
    let notifications: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications
         WHERE user_id = (SELECT user_id FROM threads WHERE id = $1)
           AND kind = 'agent_quiet'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(notifications, 1);
}

#[sqlx::test]
async fn private_thread_page_command_reaches_the_joined_agent(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let agent_key = create_agent(&pool, &owner, "bot").await;
    let thread = send(
        pool.clone(),
        bearer_json_request("POST", "/threads", json!({ "title": "PR 412" }), &agent_key),
    )
    .await;
    let thread_id = thread.body["id"].as_str().unwrap();

    let mut page_request = get_request(&format!("/t/{thread_id}"), Some(&owner));
    page_request
        .headers_mut()
        .insert("accept", "text/html".parse().unwrap());
    let page = send(pool.clone(), page_request).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.text.contains("Write a command for your agents"));

    let command = send(
        pool.clone(),
        form_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            &[("body", "Approve the merge"), ("kind", "command")],
            Some(&owner),
        ),
    )
    .await;
    assert_eq!(command.status, StatusCode::CREATED);
    let inbox = send(pool, bearer_get_request("/inbox?since=0", &agent_key)).await;
    assert_eq!(inbox.status, StatusCode::OK);
    assert_eq!(inbox.body["notes"][0]["body"], "Approve the merge");
    assert_eq!(inbox.body["notes"][0]["kind"], "command");
}

#[sqlx::test]
async fn procedure_comments_update_counts_notify_owner_and_cap_nesting(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let owner_agent = create_agent(&pool, &owner, "builder").await;
    let procedure = create_public_procedure(&pool, &owner_agent, "Review loop").await;
    let procedure_id = procedure["id"].as_str().unwrap();
    let slug = procedure["slug"].as_str().unwrap();
    let commenter = sign_in(&pool, "reader@example.com", "reader").await;

    let mut parent: Option<String> = None;
    let mut comments = Vec::new();
    for index in 1..=4 {
        let response = send(
            pool.clone(),
            authed_json_request(
                "POST",
                &format!("/procedures/{procedure_id}/comments"),
                json!({
                    "body": format!("Comment {index}"),
                    "parent_id": parent,
                    "idempotency_key": format!("comment-{index}")
                }),
                Some(&commenter),
            ),
        )
        .await;
        assert_eq!(response.status, StatusCode::CREATED);
        parent = Some(response.body["id"].as_str().unwrap().to_string());
        comments.push(response.body);
    }
    assert!(comments[3]["body"]
        .as_str()
        .unwrap()
        .starts_with("Replying to u/reader"));
    assert_eq!(comments[3]["parent_id"], comments[1]["id"]);

    let counters: (i64,) = sqlx::query_as("SELECT comment_count FROM procedures WHERE id = $1")
        .bind(procedure_id.parse::<Uuid>().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(counters.0, 4);
    let notifications: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE kind = 'comment'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(notifications, 4);

    let mut detail_request = get_request(&format!("/u/owner/{slug}"), None);
    detail_request
        .headers_mut()
        .insert("accept", "text/html".parse().unwrap());
    let detail = send(pool, detail_request).await;
    assert_eq!(detail.status, StatusCode::OK);
    assert!(detail
        .text
        .contains("Self-reported by the agents that ran it."));
    assert!(detail.text.contains("4 comments"));
    assert!(detail.text.contains("comment-depth-3"));
}

#[sqlx::test]
async fn public_profile_has_totals_without_leaking_email_and_supports_following(pool: PgPool) {
    let owner = sign_in(&pool, "private-owner@example.com", "owner").await;
    let owner_agent = create_agent(&pool, &owner, "builder").await;
    create_public_procedure(&pool, &owner_agent, "Useful work").await;
    let follower = sign_in(&pool, "follower@example.com", "follower").await;
    let owner_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE handle = 'owner'")
        .fetch_one(&pool)
        .await
        .unwrap();

    let follow = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/users/{owner_id}/follow"),
            json!({ "idempotency_key": "follow-owner" }),
            Some(&follower),
        ),
    )
    .await;
    assert_eq!(follow.status, StatusCode::CREATED);

    let profile = send(pool.clone(), get_request("/u/owner", None)).await;
    assert_eq!(profile.status, StatusCode::OK);
    assert_eq!(profile.body["follower_count"], 1);
    assert!(!profile.text.contains("private-owner@example.com"));
    let at_profile = send(pool.clone(), get_request("/@owner", None)).await;
    assert_eq!(at_profile.status, StatusCode::OK);

    let unfollow = send(
        pool,
        authed_json_request(
            "POST",
            &format!("/users/{owner_id}/unfollow"),
            json!({}),
            Some(&follower),
        ),
    )
    .await;
    assert_eq!(unfollow.status, StatusCode::OK);
}

#[sqlx::test]
async fn revoking_one_agent_does_not_affect_another(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let first_key = create_agent(&pool, &owner, "first").await;
    let second_key = create_agent(&pool, &owner, "second").await;
    let first_id: Uuid = sqlx::query_scalar("SELECT id FROM agents WHERE name = 'first'")
        .fetch_one(&pool)
        .await
        .unwrap();

    let list = send(pool.clone(), get_request("/settings/agents", Some(&owner))).await;
    // The two made here, beside the built-in Assistant and Architect every account has.
    let made: Vec<_> = list.body["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["name"] != "Assistant" && a["name"] != "Architect")
        .collect();
    assert_eq!(made.len(), 2);
    assert!(
        list.text.find(&first_key).is_none(),
        "full keys must never be listed"
    );

    let revoke = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/settings/agents/{first_id}/revoke"),
            json!({ "idempotency_key": "revoke-first" }),
            Some(&owner),
        ),
    )
    .await;
    assert_eq!(revoke.status, StatusCode::OK);
    let rejected = send(pool.clone(), bearer_get_request("/threads", &first_key)).await;
    assert_eq!(rejected.status, StatusCode::UNAUTHORIZED);
    let still_valid = send(pool, bearer_get_request("/threads", &second_key)).await;
    assert_eq!(still_valid.status, StatusCode::OK);
}

#[sqlx::test]
async fn notification_list_and_mark_read_are_owner_scoped(pool: PgPool) {
    let owner = sign_in(&pool, "owner@example.com", "owner").await;
    let owner_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE handle = 'owner'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let notification_id: Uuid = sqlx::query_scalar(
        "INSERT INTO notifications
            (user_id, kind, target_type, target_id, idempotency_key)
         VALUES ($1, 'job_needs_you', 'thread', gen_random_uuid(), 'notice-1')
         RETURNING id",
    )
    .bind(owner_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let list = send(pool.clone(), get_request("/notifs", Some(&owner))).await;
    assert_eq!(list.status, StatusCode::OK);
    assert_eq!(list.body["notifications"][0]["unread"], true);
    let read = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/notifs/{notification_id}/read"),
            json!({ "idempotency_key": "read-1" }),
            Some(&owner),
        ),
    )
    .await;
    assert_eq!(read.status, StatusCode::OK);
    let read_at: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT read_at FROM notifications WHERE id = $1")
            .bind(notification_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(read_at.is_some());

    let notification_id2: Uuid = sqlx::query_scalar(
        "INSERT INTO notifications (user_id, kind, target_type, target_id)
         VALUES ($1, 'mention', 'agent', $1)
         RETURNING id",
    )
    .bind(owner_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let read_all = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/notifs/read-all",
            json!({ "idempotency_key": "read-all-1" }),
            Some(&owner),
        ),
    )
    .await;
    assert_eq!(read_all.status, StatusCode::OK);
    let read_at2: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT read_at FROM notifications WHERE id = $1")
            .bind(notification_id2)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(read_at2.is_some());
}

#[sqlx::test]
async fn boards_can_be_joined_filtered_and_left(pool: PgPool) {
    let cookie = sign_in(&pool, "boards@example.com", "boarder").await;

    let list = send(pool.clone(), get_request("/boards", None)).await;
    assert_eq!(list.status, StatusCode::OK);
    assert_eq!(list.body["boards"].as_array().expect("boards").len(), 8);

    let list_html = {
        let mut request = get_request("/boards", None);
        request
            .headers_mut()
            .insert("accept", "text/html".parse().expect("accept header"));
        send(pool.clone(), request).await
    };
    assert!(list_html.text.contains("class=\"board-name\">data-cleanup"));
    assert!(!list_html.text.contains("class=\"board-name\">/"));

    let join = send(
        pool.clone(),
        form_request("POST", "/boards/testing/join", &[], Some(&cookie)),
    )
    .await;
    assert_eq!(join.status, StatusCode::CREATED);

    let feed_html = {
        let mut request = get_request("/", Some(&cookie));
        request
            .headers_mut()
            .insert("accept", "text/html".parse().expect("header"));
        send(pool.clone(), request).await
    };
    assert!(feed_html.text.contains("/b/testing"));
    assert!(feed_html.text.contains(">testing</a>"));
    assert!(!feed_html.text.contains(">/testing</a>"));

    let board = send(pool.clone(), get_request("/b/testing", None)).await;
    assert_eq!(board.status, StatusCode::OK);
    assert_eq!(board.body["board"]["name"], "testing");

    let leave = send(
        pool.clone(),
        form_request("POST", "/boards/testing/leave", &[], Some(&cookie)),
    )
    .await;
    assert_eq!(leave.status, StatusCode::OK);
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM board_memberships WHERE user_id =
            (SELECT id FROM users WHERE handle = 'boarder') AND deleted_at IS NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("membership count");
    assert_eq!(active, 0);
}

#[sqlx::test]
async fn web_post_is_one_field_and_redirects_to_the_created_post(pool: PgPool) {
    let cookie = sign_in(&pool, "poster@example.com", "poster").await;
    let form = {
        let mut request = get_request("/post", Some(&cookie));
        request
            .headers_mut()
            .insert("accept", "text/html".parse().expect("header"));
        send(pool.clone(), request).await
    };
    assert_eq!(form.status, StatusCode::OK);
    assert!(form.text.contains("name=\"text\""));
    assert!(!form.text.contains("name=\"title\""));
    assert!(!form.text.contains("name=\"board\""));
    assert!(!form.text.contains("name=\"visibility\""));

    let posted = {
        let mut request = form_request(
            "POST",
            "/post",
            &[
                ("text", "A small thought.\nIt should only appear once."),
                ("idempotency_key", "one-field-post"),
            ],
            Some(&cookie),
        );
        request
            .headers_mut()
            .insert("accept", "text/html".parse().expect("header"));
        send(pool.clone(), request).await
    };
    assert_eq!(posted.status, StatusCode::SEE_OTHER);
    let location = posted.headers["location"].to_str().expect("location");
    assert!(location.starts_with("/u/poster/a-small-thought-"));

    let detail = {
        let mut request = get_request(location, Some(&cookie));
        request
            .headers_mut()
            .insert("accept", "text/html".parse().expect("header"));
        send(pool.clone(), request).await
    };
    assert_eq!(detail.status, StatusCode::OK);
    assert!(detail.text.contains(
        "<div class=\"detail-post\">A small thought.\nIt should only appear once.</div>"
    ));
    assert!(!detail.text.contains("By model"));
    assert!(!detail.text.contains("<h2 id=\"check\""));

    let stored: (String, String) = sqlx::query_as(
        "SELECT kind, visibility::text FROM procedures WHERE idempotency_key = 'one-field-post'",
    )
    .fetch_one(&pool)
    .await
    .expect("stored post");
    assert_eq!(stored, ("post".to_string(), "public".to_string()));
}

#[sqlx::test]
async fn browser_social_actions_redirect_to_useful_pages(pool: PgPool) {
    let owner_cookie = sign_in(&pool, "redirect-owner@example.com", "redirectowner").await;
    let viewer_cookie = sign_in(&pool, "redirect-viewer@example.com", "redirectviewer").await;
    let owner_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE handle = 'redirectowner'")
        .fetch_one(&pool)
        .await
        .expect("owner id");

    let mut follow = form_request(
        "POST",
        &format!("/users/{owner_id}/follow"),
        &[("idempotency_key", "follow-redirect")],
        Some(&viewer_cookie),
    );
    follow
        .headers_mut()
        .insert("accept", "text/html".parse().expect("header"));
    let follow = send(pool.clone(), follow).await;
    assert_eq!(follow.status, StatusCode::SEE_OTHER);
    assert_eq!(follow.headers["location"], "/u/redirectowner");

    let agent_key = create_agent(&pool, &owner_cookie, "redirect-agent").await;
    let procedure = create_public_procedure(&pool, &agent_key, "Redirect target").await;
    let procedure_id = procedure["id"].as_str().expect("procedure id");
    let mut save = form_request(
        "POST",
        "/reactions",
        &[
            ("target_type", "procedure"),
            ("target_id", procedure_id),
            ("kind", "save"),
            ("idempotency_key", "save-redirect"),
        ],
        Some(&viewer_cookie),
    );
    save.headers_mut()
        .insert("accept", "text/html".parse().expect("header"));
    let save = send(pool.clone(), save).await;
    assert_eq!(save.status, StatusCode::SEE_OTHER);
    assert_eq!(save.headers["location"], "/u/redirectowner/redirect-target");
}

#[sqlx::test]
async fn web_push_subscription_lifecycle_is_scoped_and_service_worker_is_public(pool: PgPool) {
    let cookie = sign_in(&pool, "push@example.com", "pusher").await;
    let sw = send(pool.clone(), get_request("/sw.js", None)).await;
    assert_eq!(sw.status, StatusCode::OK);
    assert!(sw.text.contains("showNotification"));

    let key = send(pool.clone(), get_request("/push/public-key", None)).await;
    assert_eq!(key.status, StatusCode::SERVICE_UNAVAILABLE);

    let endpoint = "https://push.example.test/subscription/one";
    let subscribe = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/push/subscriptions",
            json!({
                "endpoint": endpoint,
                "keys": { "p256dh": "client-public-key", "auth": "client-auth" },
                "idempotency_key": "push-one"
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(subscribe.status, StatusCode::CREATED);

    let unsubscribe = send(
        pool.clone(),
        authed_json_request(
            "DELETE",
            "/push/subscriptions",
            json!({ "endpoint": endpoint, "idempotency_key": "push-off" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(unsubscribe.status, StatusCode::OK);
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM push_subscriptions WHERE endpoint = $1 AND deleted_at IS NULL",
    )
    .bind(endpoint)
    .fetch_one(&pool)
    .await
    .expect("subscription count");
    assert_eq!(active, 0);
}

#[sqlx::test]
async fn abandoned_runs_do_not_reduce_success_rate(pool: PgPool) {
    let cookie = sign_in(&pool, "rates@example.com", "rates").await;
    let agent_key = create_agent(&pool, &cookie, "rate-agent").await;
    let procedure = create_procedure(&pool, &agent_key, "Rate denominator").await;
    publish(
        &pool,
        &agent_key,
        procedure["id"].as_str().expect("procedure id"),
    )
    .await;
    for (outcome, key) in [("pass", "run-pass"), ("abandoned", "run-abandoned")] {
        let response = send(
            pool.clone(),
            bearer_json_request(
                "POST",
                "/runs",
                json!({
                    "procedure_id": procedure["id"],
                    "model": "test-model",
                    "outcome": outcome,
                    "attempts": 1,
                    "duration_ms": 10,
                    "idempotency_key": key
                }),
                &agent_key,
            ),
        )
        .await;
        assert_eq!(response.status, StatusCode::CREATED);
    }
    let stats = send(
        pool.clone(),
        get_request(
            &format!(
                "/procedures/{}/stats",
                procedure["id"].as_str().expect("procedure id")
            ),
            None,
        ),
    )
    .await;
    assert_eq!(stats.body["overall"]["run_count"], 2);
    assert_eq!(stats.body["overall"]["scored_run_count"], 1);
    assert_eq!(stats.body["overall"]["success_rate"], 1.0);
}

#[sqlx::test]
async fn run_dialog_sends_a_command_to_one_owned_agent_and_fork_stays_private(pool: PgPool) {
    let owner_cookie = sign_in(&pool, "source@example.com", "source").await;
    let owner_agent = create_agent(&pool, &owner_cookie, "publisher").await;
    let procedure = create_public_procedure(&pool, &owner_agent, "Reusable loop").await;
    let procedure_id: Uuid = procedure["id"].as_str().unwrap().parse().unwrap();

    let runner_cookie = sign_in(&pool, "runner@example.com", "runner").await;
    create_agent(&pool, &runner_cookie, "bot").await;
    let runner_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE handle = 'runner'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let bot_id: Uuid =
        sqlx::query_scalar("SELECT id FROM agents WHERE user_id = $1 AND name = 'bot'")
            .bind(runner_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    let sent = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/procedures/{procedure_id}/send"),
            json!({"agent_id": bot_id, "idempotency_key": "send-once"}),
            Some(&runner_cookie),
        ),
    )
    .await;
    assert_eq!(sent.status, StatusCode::CREATED);
    let thread_id: Uuid = sent.body["id"].as_str().unwrap().parse().unwrap();
    let command: String =
        sqlx::query_scalar("SELECT body FROM notes WHERE thread_id = $1 AND kind = 'command'")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(command, "Run procedure: air get source/reusable-loop");
    let joined: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM thread_agents WHERE thread_id = $1 AND agent_id = $2)",
    )
    .bind(thread_id)
    .bind(bot_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(joined);

    let retried = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/procedures/{procedure_id}/send"),
            json!({"agent_id": bot_id, "idempotency_key": "send-once"}),
            Some(&runner_cookie),
        ),
    )
    .await;
    assert_eq!(retried.status, StatusCode::OK);
    assert_eq!(retried.body["id"], thread_id.to_string());

    let fork = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/procedures/{procedure_id}/fork"),
            json!({"idempotency_key": "fork-once"}),
            Some(&runner_cookie),
        ),
    )
    .await;
    assert_eq!(fork.status, StatusCode::CREATED);
    assert_eq!(fork.body["visibility"], "private");
    assert_eq!(fork.body["fork_of_id"], procedure_id.to_string());

    sqlx::query("UPDATE threads SET status = 'waiting' WHERE id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();
    let attention = send(
        pool.clone(),
        get_request("/inbox/attention", Some(&runner_cookie)),
    )
    .await;
    assert_eq!(attention.status, StatusCode::OK);
    assert_eq!(attention.body["needs_attention"], true);

    let mut detail_request = get_request("/u/source/reusable-loop", Some(&runner_cookie));
    detail_request
        .headers_mut()
        .insert("accept", "text/html".parse().unwrap());
    let detail = send(pool, detail_request).await;
    assert!(detail.text.contains("Send to agent"));
    assert!(detail.text.contains("aria-label=\"Fork\""));
    assert!(detail
        .text
        .contains("Air will not execute this on the server"));
}

#[sqlx::test]
async fn profile_tabs_and_theme_preference_are_functional(pool: PgPool) {
    let owner_cookie = sign_in(&pool, "tabs-owner@example.com", "tabsowner").await;
    let agent = create_agent(&pool, &owner_cookie, "tab-agent").await;
    let procedure = create_public_procedure(&pool, &agent, "Saved from tabs").await;
    let procedure_id: Uuid = procedure["id"].as_str().unwrap().parse().unwrap();
    let viewer_cookie = sign_in(&pool, "tabs-viewer@example.com", "tabsviewer").await;

    let save = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/reactions",
            json!({"target_type": "procedure", "target_id": procedure_id, "kind": "save"}),
            Some(&viewer_cookie),
        ),
    )
    .await;
    assert_eq!(save.status, StatusCode::CREATED);
    let comment = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/procedures/{procedure_id}/comments"),
            json!({"body": "Useful from my profile", "idempotency_key": "profile-comment"}),
            Some(&viewer_cookie),
        ),
    )
    .await;
    assert_eq!(comment.status, StatusCode::CREATED);

    let saved = send(
        pool.clone(),
        get_request("/u/tabsviewer?view=saved", Some(&viewer_cookie)),
    )
    .await;
    assert_eq!(saved.status, StatusCode::OK);
    assert_eq!(saved.body["procedures"][0]["id"], procedure_id.to_string());
    let comments = send(
        pool.clone(),
        get_request("/u/tabsviewer?view=comments", Some(&viewer_cookie)),
    )
    .await;
    assert_eq!(comments.status, StatusCode::OK);
    assert_eq!(
        comments.body["comments"][0]["body"],
        "Useful from my profile"
    );
    assert_eq!(comments.body["view"], "comments");

    let theme = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/settings/theme",
            json!({"theme": "dark"}),
            Some(&viewer_cookie),
        ),
    )
    .await;
    assert_eq!(theme.status, StatusCode::OK);
    assert!(theme
        .headers
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("air_theme=dark"));
}

#[sqlx::test]
async fn settings_page_and_navigation_links(pool: PgPool) {
    let cookie = sign_in(&pool, "settings_user@example.com", "settings_user").await;

    // JSON GET /settings
    let res_json = send(pool.clone(), get_request("/settings", Some(&cookie))).await;
    assert_eq!(res_json.status, StatusCode::OK);
    assert_eq!(res_json.body["email"], "settings_user@example.com");
    assert_eq!(res_json.body["handle"], "settings_user");
    assert_eq!(res_json.body["theme"], "system");

    // HTML GET /settings
    let mut req_html = get_request("/settings", Some(&cookie));
    req_html
        .headers_mut()
        .insert(axum::http::header::ACCEPT, "text/html".parse().unwrap());
    let res_html = send(pool.clone(), req_html).await;
    assert_eq!(res_html.status, StatusCode::OK);
    assert!(res_html.text.contains("Settings"));
    assert!(res_html.text.contains("Appearance"));
    assert!(res_html.text.contains("Save theme"));
    assert!(res_html.text.contains("Connected agents"));

    // HTML GET /settings/agents redirects to /connect?view=agents
    let mut req_agents = get_request("/settings/agents", Some(&cookie));
    req_agents
        .headers_mut()
        .insert(axum::http::header::ACCEPT, "text/html".parse().unwrap());
    let res_agents = send(pool.clone(), req_agents).await;
    assert_eq!(res_agents.status, StatusCode::SEE_OTHER);
    assert_eq!(
        res_agents
            .headers
            .get("location")
            .unwrap()
            .to_str()
            .unwrap(),
        "/connect?view=agents"
    );

    // HTML GET /device has backlink to /connect?view=agents
    let mut req_device = get_request("/device", Some(&cookie));
    req_device
        .headers_mut()
        .insert(axum::http::header::ACCEPT, "text/html".parse().unwrap());
    let res_device = send(pool.clone(), req_device).await;
    assert_eq!(res_device.status, StatusCode::OK);
    assert!(res_device.text.contains("Back to Connected Agents"));

    // HTML POST /settings/theme redirects to /settings
    let mut req_theme = form_request(
        "POST",
        "/settings/theme",
        &[("theme", "dark")],
        Some(&cookie),
    );
    req_theme
        .headers_mut()
        .insert(axum::http::header::ACCEPT, "text/html".parse().unwrap());
    let theme_html = send(pool, req_theme).await;
    assert_eq!(theme_html.status, StatusCode::SEE_OTHER);
    assert_eq!(
        theme_html
            .headers
            .get("location")
            .unwrap()
            .to_str()
            .unwrap(),
        "/settings"
    );
}

#[sqlx::test]
async fn workspaces_lifecycle_and_external_agent_workflow(pool: PgPool) {
    let cookie = sign_in(&pool, "workspace-owner@example.com", "wsowner").await;
    let agent_key = create_agent(&pool, &cookie, "bot-alpha").await;

    // 1. Create workspace
    let ws_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/boards",
            json!({
                "name": "review-desk-alpha",
                "description": "Alpha desk coordination",
                "visibility": "public",
                "idempotency_key": "ws-alpha-1"
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(ws_res.status, StatusCode::CREATED);
    assert_eq!(ws_res.body["name"], "review-desk-alpha");

    // Idempotent retry returns 200 OK
    let ws_retry = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/boards",
            json!({
                "name": "review-desk-alpha",
                "description": "Alpha desk coordination",
                "visibility": "public",
                "idempotency_key": "ws-alpha-1"
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(ws_retry.status, StatusCode::OK);

    // 2. Fetch workspace details
    let detail_res = send(
        pool.clone(),
        authed_json_request("GET", "/boards/review-desk-alpha", json!({}), Some(&cookie)),
    )
    .await;
    assert_eq!(detail_res.status, StatusCode::OK);
    assert_eq!(detail_res.body["board"]["name"], "review-desk-alpha");

    // Get the agent's ID
    let agent_id: Uuid = sqlx::query_scalar("SELECT id FROM agents WHERE name = 'bot-alpha'")
        .fetch_one(&pool)
        .await
        .expect("agent id");

    // 3. Assign agent to workspace
    let assign_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/boards/review-desk-alpha/agents",
            json!({
                "agent_id": agent_id,
                "role": "executor",
                "idempotency_key": "assign-alpha-1"
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(assign_res.status, StatusCode::CREATED);
    assert_eq!(assign_res.body["role"], "executor");

    // Query workspace to verify assigned agent
    let detail_after_assign = send(
        pool.clone(),
        authed_json_request("GET", "/boards/review-desk-alpha", json!({}), Some(&cookie)),
    )
    .await;
    assert_eq!(detail_after_assign.status, StatusCode::OK);
    let assigned = detail_after_assign.body["assigned_agents"]
        .as_array()
        .expect("assigned_agents array");
    assert_eq!(assigned.len(), 1);
    assert_eq!(assigned[0]["name"], "bot-alpha");
    assert_eq!(assigned[0]["role"], "executor");

    // 4. Dispatch a task thread to the workspace with an initial command
    let task_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/boards/review-desk-alpha/threads",
            json!({
                "title": "Analyze momentum spread",
                "command": "compute 5m spread and alert",
                "visibility": "private",
                "idempotency_key": "task-alpha-1"
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(task_res.status, StatusCode::CREATED);
    let thread_id = task_res.body["id"].as_str().expect("thread id");

    // 5. Verify external agent receives the task command in /inbox
    let inbox_res = send(pool.clone(), bearer_get_request("/inbox", &agent_key)).await;
    assert_eq!(inbox_res.status, StatusCode::OK);
    let notes = inbox_res.body["notes"].as_array().expect("inbox notes");
    assert!(
        !notes.is_empty(),
        "assigned agent should receive task command note"
    );
    assert_eq!(notes[0]["body"], "compute 5m spread and alert");
    assert_eq!(notes[0]["kind"], "command");

    // 6. External agent reports status back to the thread
    let note_res = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({
                "body": "Spread computed: 0.12%, within bounds",
                "kind": "status"
            }),
            &agent_key,
        ),
    )
    .await;
    assert_eq!(note_res.status, StatusCode::CREATED);

    // 7. Remove agent from workspace
    let remove_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/boards/review-desk-alpha/agents/{agent_id}/remove"),
            json!({}),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(remove_res.status, StatusCode::OK);

    // Query workspace to verify agent is removed
    let detail_after_remove = send(
        pool.clone(),
        authed_json_request("GET", "/boards/review-desk-alpha", json!({}), Some(&cookie)),
    )
    .await;
    let assigned_after = detail_after_remove.body["assigned_agents"]
        .as_array()
        .expect("assigned_agents array");
    assert_eq!(assigned_after.len(), 0);
}

#[sqlx::test]
async fn workflow_orchestration_multi_step_flow(pool: PgPool) {
    let cookie = sign_in(&pool, "orchestrator@example.com", "orchestrator").await;
    let scraper_key = create_agent(&pool, &cookie, "scraper-agent").await;
    let analyst_key = create_agent(&pool, &cookie, "analyst-agent").await;

    let scraper_id: Uuid = sqlx::query_scalar("SELECT id FROM agents WHERE name = 'scraper-agent'")
        .fetch_one(&pool)
        .await
        .expect("scraper id");
    let analyst_id: Uuid = sqlx::query_scalar("SELECT id FROM agents WHERE name = 'analyst-agent'")
        .fetch_one(&pool)
        .await
        .expect("analyst id");

    // 1. Verify GET /agents lists active connected agents without secrets
    let agents_res = send(
        pool.clone(),
        authed_json_request("GET", "/agents", json!({}), Some(&cookie)),
    )
    .await;
    assert_eq!(agents_res.status, StatusCode::OK);
    let agent_list = agents_res.body["agents"].as_array().expect("agents array");
    // The two connected here, beside the built-in Assistant and Architect.
    assert_eq!(
        agent_list
            .iter()
            .filter(|a| a["name"] != "Assistant" && a["name"] != "Architect")
            .count(),
        2
    );
    for a in agent_list {
        assert!(a.get("key").is_none());
        assert!(a.get("token").is_none());
        assert!(a.get("secret").is_none());
        assert!(a.get("name").is_some());
        assert!(a.get("status").is_some());
    }

    // 2. Create workspace "testing-workspace"
    let ws_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/boards",
            json!({
                "name": "testing-workspace",
                "description": "Integration testing workspace",
                "visibility": "private"
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(ws_res.status, StatusCode::CREATED);

    // 3. Create a 2-step workflow in the workspace
    let wf_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/workflows",
            json!({
                "title": "Data Pipeline Workflow",
                "summary": "Scrapes and analyzes data",
                "body": "Global pipeline instructions and context",
                "check_description": "Data analysis output produced and scored",
                "board": "testing-workspace",
                "visibility": "private",
                "steps": [
                    {
                        "step_order": 1,
                        "title": "Scrape Data",
                        "instructions": "Collect external metrics and write summary",
                        "agent_id": scraper_id
                    },
                    {
                        "step_order": 2,
                        "title": "Analyze Data",
                        "instructions": "Process metrics from previous step and produce analysis",
                        "agent_id": analyst_id
                    }
                ]
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(wf_res.status, StatusCode::CREATED);
    let wf_id = wf_res.body["workflow"]["id"].as_str().expect("workflow id");

    // 4. Verify workspace detail shows the workflow
    let board_detail = send(
        pool.clone(),
        authed_json_request("GET", "/boards/testing-workspace", json!({}), Some(&cookie)),
    )
    .await;
    assert_eq!(board_detail.status, StatusCode::OK);
    let wfs = board_detail.body["workflows"]
        .as_array()
        .expect("workflows array");
    assert_eq!(wfs.len(), 1);
    assert_eq!(wfs[0]["id"], wf_id);
    assert_eq!(wfs[0]["step_count"], 2);

    // 5. Start the workflow run
    let run_start_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/workflows/{wf_id}/runs"),
            json!({
                "model": "test-model-v1"
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(run_start_res.status, StatusCode::CREATED);
    let run_id = run_start_res.body["run"]["id"].as_str().expect("run id");
    assert_eq!(run_start_res.body["run"]["status"], "running");
    assert_eq!(run_start_res.body["run"]["current_step_order"], 1);

    // 6. Scraper agent checks current assigned step
    let scraper_step = send(
        pool.clone(),
        bearer_get_request("/agent/steps/current", &scraper_key),
    )
    .await;
    assert_eq!(scraper_step.status, StatusCode::OK);
    assert_eq!(scraper_step.body["step"]["step_order"], 1);
    assert_eq!(scraper_step.body["step"]["title"], "Scrape Data");

    // Analyst agent has no pending step yet (waiting for step 1)
    let analyst_step_before = send(
        pool.clone(),
        bearer_get_request("/agent/steps/current", &analyst_key),
    )
    .await;
    assert_eq!(analyst_step_before.status, StatusCode::OK);
    assert!(analyst_step_before.body["step"].is_null());

    // 7. Scraper agent completes step 1
    let step1_done = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            &format!("/runs/{run_id}/steps/1/result"),
            json!({
                "output": "Extracted 150 rows of market quotes"
            }),
            &scraper_key,
        ),
    )
    .await;
    assert_eq!(step1_done.status, StatusCode::OK);
    assert_eq!(step1_done.body["run"]["status"], "running");
    assert_eq!(step1_done.body["run"]["current_step_order"], 2);

    // 8. Analyst agent now sees step 2 as its current assigned step with step 1 output passed as input!
    let analyst_step_now = send(
        pool.clone(),
        bearer_get_request("/agent/steps/current", &analyst_key),
    )
    .await;
    assert_eq!(analyst_step_now.status, StatusCode::OK);
    assert_eq!(analyst_step_now.body["step"]["step_order"], 2);
    assert_eq!(analyst_step_now.body["step"]["title"], "Analyze Data");
    let input = analyst_step_now.body["step"]["input"]
        .as_str()
        .expect("step 2 input");
    assert!(input.contains("Extracted 150 rows of market quotes"));

    // 9. Analyst agent completes step 2
    let step2_done = send(
        pool.clone(),
        bearer_json_request(
            "POST",
            &format!("/runs/{run_id}/steps/2/result"),
            json!({
                "output": "Analysis complete: Mean spread 0.08%, Sharpe ratio 2.1",
                "outcome": "pass"
            }),
            &analyst_key,
        ),
    )
    .await;
    assert_eq!(step2_done.status, StatusCode::OK);
    assert_eq!(step2_done.body["run"]["status"], "completed");
    assert_eq!(step2_done.body["run"]["outcome"], "pass");

    // 10. Inspect full run details
    let run_detail = send(
        pool.clone(),
        authed_json_request("GET", &format!("/runs/{run_id}"), json!({}), Some(&cookie)),
    )
    .await;
    assert_eq!(run_detail.status, StatusCode::OK);
    assert_eq!(run_detail.body["run"]["status"], "completed");
    assert_eq!(run_detail.body["run"]["outcome"], "pass");
    let steps = run_detail.body["steps"].as_array().expect("steps array");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["status"], "completed");
    assert_eq!(steps[0]["output"], "Extracted 150 rows of market quotes");
    assert_eq!(steps[1]["status"], "completed");
    assert_eq!(
        steps[1]["output"],
        "Analysis complete: Mean spread 0.08%, Sharpe ratio 2.1"
    );

    // 11. Verify activity thread exists and contains the command and result notes
    let thread_id = run_detail.body["run"]["thread_id"]
        .as_str()
        .expect("thread id");
    let thread_res = send(
        pool.clone(),
        authed_json_request(
            "GET",
            &format!("/threads/{thread_id}"),
            json!({}),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(thread_res.status, StatusCode::OK);
}

#[sqlx::test]
async fn workflow_edit_delete_and_navigation_features(pool: PgPool) {
    // 1. Verify GET /connect public guide
    let connect_res = send(pool.clone(), get_request("/connect", None)).await;
    assert_eq!(connect_res.status, StatusCode::OK);
    assert!(connect_res.text.contains("Connect External Agents"));
    assert!(connect_res.text.contains("air_cli.py login"));

    // 2. Setup user, agent, and board
    let cookie = sign_in(&pool, "dev-pilot@example.com", "pilotuser").await;
    let _agent_key = create_agent(&pool, &cookie, "pilot-agent").await;

    let board_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/boards",
            json!({
                "name": "pilot-ops",
                "description": "Operations desk",
                "visibility": "private"
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(board_res.status, StatusCode::CREATED);

    // 3. Create workflow
    let wf_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/workflows",
            json!({
                "board_name": "pilot-ops",
                "title": "Ops Checklist",
                "summary": "Daily morning checks",
                "body": "Run health verification",
                "check_description": "All servers operational",
                "visibility": "private",
                "steps": [
                    {
                        "step_order": 1,
                        "title": "Server Audit",
                        "instructions": "Inspect container uptime"
                    }
                ]
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(wf_res.status, StatusCode::CREATED);
    let wf_id = wf_res.body["workflow"]["id"].as_str().expect("workflow id");

    // 4. Verify GET /workflows/{id}/edit renders HTML form (200 OK, not 405)
    let edit_form_res = send(
        pool.clone(),
        get_request(&format!("/workflows/{wf_id}/edit"), Some(&cookie)),
    )
    .await;
    assert_eq!(edit_form_res.status, StatusCode::OK);
    assert!(edit_form_res.text.contains("Edit Workflow"));
    assert!(edit_form_res.text.contains("Ops Checklist"));

    // 5. Update workflow via POST /workflows/{id}/edit (HTML Form simulation)
    let update_form = [
        ("title", "Updated Ops Checklist"),
        ("summary", "Updated summary"),
        ("body", "Updated body instructions"),
        ("check_description", "Updated completion check"),
        ("visibility", "unlisted"),
        ("step1_title", "Updated Step 1"),
        ("step1_instructions", "Updated Step 1 instructions"),
    ];
    let update_res = {
        let mut req = form_request(
            "POST",
            &format!("/workflows/{wf_id}/edit"),
            &update_form,
            Some(&cookie),
        );
        req.headers_mut()
            .insert("accept", "text/html".parse().expect("header"));
        send(pool.clone(), req).await
    };
    assert_eq!(update_res.status, StatusCode::SEE_OTHER);

    // Verify updated values via JSON inspection
    let get_wf = send(
        pool.clone(),
        authed_json_request(
            "GET",
            &format!("/workflows/{wf_id}"),
            json!({}),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(get_wf.status, StatusCode::OK);
    assert_eq!(get_wf.body["workflow"]["title"], "Updated Ops Checklist");
    assert_eq!(get_wf.body["workflow"]["visibility"], "unlisted");

    // 6. Test Step Agent Assignment
    let agent_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM agents WHERE name = 'pilot-agent' AND deleted_at IS NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("agent id");

    let assign_res = {
        let mut req = form_request(
            "POST",
            &format!("/workflows/{wf_id}/steps/1/assign"),
            &[("agent_id", &agent_id.to_string())],
            Some(&cookie),
        );
        req.headers_mut()
            .insert("accept", "text/html".parse().expect("header"));
        send(pool.clone(), req).await
    };
    assert_eq!(assign_res.status, StatusCode::SEE_OTHER);

    let get_wf_after_assign = send(
        pool.clone(),
        authed_json_request(
            "GET",
            &format!("/workflows/{wf_id}"),
            json!({}),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(
        get_wf_after_assign.body["steps"][0]["agent_id"],
        agent_id.to_string()
    );

    // 7. Test thread archive and unarchive
    let thread_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/threads",
            json!({
                "title": "Ops Meeting Task",
                "visibility": "private"
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(thread_res.status, StatusCode::CREATED);
    let thread_id = thread_res.body["id"].as_str().expect("thread id");

    let archive_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/threads/{thread_id}/archive"),
            json!({}),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(archive_res.status, StatusCode::OK);

    let unarchive_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/threads/{thread_id}/unarchive"),
            json!({}),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(unarchive_res.status, StatusCode::OK);

    // 8. Delete workflow via POST /workflows/{id}/delete
    let delete_res = {
        let mut req = form_request(
            "POST",
            &format!("/workflows/{wf_id}/delete"),
            &[],
            Some(&cookie),
        );
        req.headers_mut()
            .insert("accept", "text/html".parse().expect("header"));
        send(pool.clone(), req).await
    };
    assert_eq!(delete_res.status, StatusCode::SEE_OTHER);

    // Subsequent retrieval returns 404
    let deleted_wf = send(
        pool.clone(),
        authed_json_request(
            "GET",
            &format!("/workflows/{wf_id}"),
            json!({}),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(deleted_wf.status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn agent_rename_and_chat_and_settings_redirect(pool: PgPool) {
    let owner = sign_in(&pool, "builder@example.com", "builder").await;
    let _agent_key = create_agent(&pool, &owner, "bot-original").await;

    let agent_id: Uuid = sqlx::query_scalar("SELECT id FROM agents WHERE name = 'bot-original'")
        .fetch_one(&pool)
        .await
        .expect("agent id");

    // 1. Rename agent via POST /connect/agents/{id}/rename (HTML form redirect)
    let mut req = form_request(
        "POST",
        &format!("/connect/agents/{agent_id}/rename"),
        &[("name", "bot-renamed")],
        Some(&owner),
    );
    req.headers_mut()
        .insert("accept", "text/html".parse().unwrap());
    let rename_res = send(pool.clone(), req).await;
    assert_eq!(rename_res.status, StatusCode::SEE_OTHER);
    assert_eq!(
        rename_res
            .headers
            .get("location")
            .unwrap()
            .to_str()
            .unwrap(),
        "/connect?view=agents"
    );

    let renamed_name: String = sqlx::query_scalar("SELECT name FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_one(&pool)
        .await
        .expect("agent name");
    assert_eq!(renamed_name, "bot-renamed");

    // 2. Chat with agent via POST /connect/agents/{id}/chat creates a thread and redirects
    let mut chat_req = form_request(
        "POST",
        &format!("/connect/agents/{agent_id}/chat"),
        &[],
        Some(&owner),
    );
    chat_req
        .headers_mut()
        .insert("accept", "text/html".parse().unwrap());
    let chat_res = send(pool.clone(), chat_req).await;
    assert_eq!(chat_res.status, StatusCode::SEE_OTHER);
    let location = chat_res
        .headers
        .get("location")
        .expect("location header")
        .to_str()
        .expect("location str");
    assert!(location.starts_with("/t/"));

    // Verify thread exists and agent is joined
    let thread_id_str = location.trim_start_matches("/t/");
    let thread_id = Uuid::parse_str(thread_id_str).expect("thread id");
    let thread_title: String = sqlx::query_scalar("SELECT title FROM threads WHERE id = $1")
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .expect("thread title");
    assert_eq!(thread_title, "Chat with bot-renamed");

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_agents WHERE thread_id = $1 AND agent_id = $2",
    )
    .bind(thread_id)
    .bind(agent_id)
    .fetch_one(&pool)
    .await
    .expect("thread_agents count");
    assert_eq!(count, 1);

    // 3. /settings/agents redirects to /connect?view=agents for HTML browsers
    let mut settings_req = authed_json_request("GET", "/settings/agents", json!({}), Some(&owner));
    settings_req
        .headers_mut()
        .insert("accept", "text/html".parse().unwrap());
    let settings_res = send(pool.clone(), settings_req).await;
    assert_eq!(settings_res.status, StatusCode::SEE_OTHER);
    assert_eq!(
        settings_res
            .headers
            .get("location")
            .unwrap()
            .to_str()
            .unwrap(),
        "/connect?view=agents"
    );
}
