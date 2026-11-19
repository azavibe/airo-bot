mod common;

use airobot::auth::secret;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::{authed_json_request, form_request, get_request, send, session_cookie};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn sign_in(pool: &PgPool, email: &str, handle: &str) -> String {
    let id = Uuid::new_v4();
    let raw_secret = secret::random_hex_token(32);
    let token_hash = secret::hash_secret(&raw_secret).expect("hash");
    let expires_at = Utc::now() + Duration::minutes(15);
    sqlx::query(
        "INSERT INTO email_tokens (id, email, token_hash, purpose, expires_at)
         VALUES ($1, $2, $3, 'login', $4)",
    )
    .bind(id)
    .bind(email)
    .bind(&token_hash)
    .bind(expires_at)
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

#[sqlx::test]
async fn workspace_isolation_blocks_cross_workspace_access(pool: PgPool) {
    let cookie_alice = sign_in(&pool, "alice@example.com", "alice").await;
    let cookie_bob = sign_in(&pool, "bob@example.com", "bob").await;

    // Alice initializes her default workspace and creates a task
    let task_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/tasks",
            json!({
                "title": "Alice Secret Task",
                "priority": "high",
                "idempotency_key": "alice-task-1"
            }),
            Some(&cookie_alice),
        ),
    )
    .await;
    assert_eq!(task_res.status, StatusCode::CREATED);
    let task_id = task_res.body["task"]["id"].as_str().expect("task id");

    // Alice can view her task
    let alice_view = send(
        pool.clone(),
        authed_json_request(
            "GET",
            &format!("/tasks/{task_id}"),
            json!({}),
            Some(&cookie_alice),
        ),
    )
    .await;
    assert_eq!(alice_view.status, StatusCode::OK);
    assert_eq!(alice_view.body["task"]["title"], "Alice Secret Task");

    // Bob cannot view Alice's task (Forbidden 403)
    let bob_view = send(
        pool.clone(),
        authed_json_request(
            "GET",
            &format!("/tasks/{task_id}"),
            json!({}),
            Some(&cookie_bob),
        ),
    )
    .await;
    assert_eq!(bob_view.status, StatusCode::FORBIDDEN);

    // Bob views /tasks in his workspace -> Alice's task is NOT visible
    let bob_tasks = send(
        pool.clone(),
        authed_json_request("GET", "/tasks?status=all", json!({}), Some(&cookie_bob)),
    )
    .await;
    assert_eq!(bob_tasks.status, StatusCode::OK);
    let tasks_arr = bob_tasks.body["tasks"].as_array().expect("tasks array");
    for t in tasks_arr {
        assert_ne!(t["id"], task_id);
    }
}

#[sqlx::test]
async fn atomic_task_claim_and_result_execution(pool: PgPool) {
    let cookie = sign_in(&pool, "bot_lead@example.com", "bot_lead").await;

    // Create a new task
    let task_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/tasks",
            json!({
                "title": "Process lead qualification",
                "priority": "urgent",
                "idempotency_key": "lead-task-101"
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(task_res.status, StatusCode::CREATED);
    let task_id = task_res.body["task"]["id"].as_str().expect("task id");
    assert_eq!(task_res.body["task"]["status"], "queued");

    // Two bots race to claim the same task concurrently
    let claim_req1 = authed_json_request(
        "POST",
        &format!("/tasks/{task_id}/claim"),
        json!({ "idempotency_key": "claim-attempt-1" }),
        Some(&cookie),
    );
    let claim_req2 = authed_json_request(
        "POST",
        &format!("/tasks/{task_id}/claim"),
        json!({ "idempotency_key": "claim-attempt-2" }),
        Some(&cookie),
    );

    let (res1, res2) = tokio::join!(
        send(pool.clone(), claim_req1),
        send(pool.clone(), claim_req2)
    );

    // Exactly one must succeed with 200, the other must fail with 409 Conflict
    let (winner, loser) = if res1.status == StatusCode::OK {
        (res1, res2)
    } else {
        (res2, res1)
    };
    assert_eq!(winner.status, StatusCode::OK);
    assert_eq!(winner.body["status"], "claimed");
    assert_eq!(winner.body["task"]["status"], "assigned");
    assert!(winner.body["lease_id"].as_str().is_some());
    assert!(winner.body["context_pack"]["hash"].as_str().is_some());

    assert_eq!(loser.status, StatusCode::CONFLICT);

    // Bot posts completed result
    let result_req = authed_json_request(
        "POST",
        &format!("/tasks/{task_id}/result"),
        json!({
            "status": "completed",
            "output": { "score": 92, "recommendation": "VIP outreach" },
            "notes": "Lead qualified successfully with score 92.",
            "idempotency_key": "res-lead-1"
        }),
        Some(&cookie),
    );
    let result_res = send(pool.clone(), result_req).await;
    assert_eq!(result_res.status, StatusCode::OK);
    assert_eq!(result_res.body["task"]["status"], "completed");

    // Verify task detail reflects completed status and notes
    let detail_res = send(
        pool.clone(),
        authed_json_request(
            "GET",
            &format!("/tasks/{task_id}"),
            json!({}),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(detail_res.status, StatusCode::OK);
    assert_eq!(detail_res.body["task"]["status"], "completed");
    let notes = detail_res.body["notes"].as_array().expect("notes array");
    assert!(!notes.is_empty());
    assert_eq!(
        notes[0]["body"],
        "Lead qualified successfully with score 92."
    );

    // Duplicate result post with same idempotency key is idempotent
    let duplicate_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/tasks/{task_id}/result"),
            json!({
                "status": "completed",
                "output": { "score": 92 },
                "notes": "Duplicate note",
                "idempotency_key": "res-lead-1"
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(duplicate_res.status, StatusCode::OK);

    // Note count remained 1 (no duplicate note inserted)
    let detail_res_after = send(
        pool.clone(),
        authed_json_request(
            "GET",
            &format!("/tasks/{task_id}"),
            json!({}),
            Some(&cookie),
        ),
    )
    .await;
    let notes_after = detail_res_after.body["notes"].as_array().expect("notes");
    assert_eq!(notes_after.len(), 1);
}

#[sqlx::test]
async fn approval_gate_lifecycle_and_idempotency(pool: PgPool) {
    let cookie = sign_in(&pool, "approver@example.com", "approver").await;

    // Create task
    let task_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/tasks",
            json!({
                "title": "Send VIP promotional SMS",
                "priority": "normal"
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(task_res.status, StatusCode::CREATED);
    let task_id = task_res.body["task"]["id"].as_str().expect("task id");
    let ws_id = task_res.body["task"]["workspace_id"]
        .as_str()
        .expect("workspace id");

    // Insert a pending approval for this task
    let approval_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO approvals (id, workspace_id, task_id, requested_action, risk_level, status, requested_by, created_at, updated_at)
         VALUES ($1, $2, $3, 'Send SMS campaign to 500 contacts', 'high', 'pending', 'agent:lead-expert', now(), now())"
    )
    .bind(approval_id)
    .bind(Uuid::parse_str(ws_id).expect("uuid"))
    .bind(Uuid::parse_str(task_id).expect("uuid"))
    .execute(&pool)
    .await
    .expect("insert approval");

    // List approvals via JSON
    let list_res = send(
        pool.clone(),
        authed_json_request("GET", "/approvals?status=pending", json!({}), Some(&cookie)),
    )
    .await;
    assert_eq!(list_res.status, StatusCode::OK);
    let approvals = list_res.body["approvals"]
        .as_array()
        .expect("approvals array");
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0]["id"], approval_id.to_string());
    assert_eq!(approvals[0]["status"], "pending");

    // Decide approval: approve
    let decide_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/approvals/{approval_id}/decide"),
            json!({
                "decision": "approved",
                "reason": "Campaign text and budget verified."
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(decide_res.status, StatusCode::OK);
    assert_eq!(decide_res.body["approval"]["status"], "approved");

    // Decide again: idempotent, does not fail or alter decision
    let decide_again = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/approvals/{approval_id}/decide"),
            json!({
                "decision": "approved",
                "reason": "Campaign text and budget verified."
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(decide_again.status, StatusCode::OK);
    assert_eq!(decide_again.body["approval"]["status"], "approved");

    // Query pending approvals again -> should now be empty
    let list_after = send(
        pool.clone(),
        authed_json_request("GET", "/approvals?status=pending", json!({}), Some(&cookie)),
    )
    .await;
    let pending_after = list_after.body["approvals"].as_array().expect("approvals");
    assert!(pending_after.is_empty());
}

#[sqlx::test]
async fn audit_trail_recorded_on_actions(pool: PgPool) {
    let cookie = sign_in(&pool, "auditor@example.com", "auditor").await;

    // Create a task
    let task_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/tasks",
            json!({
                "title": "Audit test task",
                "priority": "low"
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(task_res.status, StatusCode::CREATED);
    let task_id = task_res.body["task"]["id"].as_str().expect("task id");

    // Claim the task
    let claim_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/tasks/{task_id}/claim"),
            json!({}),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(claim_res.status, StatusCode::OK);

    // Complete the task
    let result_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/tasks/{task_id}/result"),
            json!({ "status": "completed" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(result_res.status, StatusCode::OK);

    // Fetch activity feed
    let act_res = send(
        pool.clone(),
        authed_json_request("GET", "/activity", json!({}), Some(&cookie)),
    )
    .await;
    assert_eq!(act_res.status, StatusCode::OK);
    let events = act_res.body["events"].as_array().expect("events array");
    let actions: Vec<&str> = events.iter().filter_map(|e| e["action"].as_str()).collect();

    assert!(
        actions.contains(&"task.created"),
        "Missing task.created audit"
    );
    assert!(
        actions.contains(&"task.claimed"),
        "Missing task.claimed audit"
    );
    assert!(
        actions.contains(&"task.completed"),
        "Missing task.completed audit"
    );
}

#[sqlx::test]
async fn nav_labels_match_urls_and_pages_are_uncacheable(pool: PgPool) {
    let cookie = sign_in(&pool, "nav@example.com", "natter").await;

    // The old workspace URL leads back to the inbox, and never as a
    // cacheable 301.
    let back = send(pool.clone(), get_request("/yours", Some(&cookie))).await;
    assert_eq!(back.status, StatusCode::SEE_OTHER);
    assert_eq!(
        back.headers
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );

    // The top nav uses the words the URLs already use.
    let mut page_req = get_request("/inbox", Some(&cookie));
    page_req
        .headers_mut()
        .insert("accept", "text/html".parse().expect("header value"));
    let page = send(pool.clone(), page_req).await;
    assert_eq!(page.status, StatusCode::OK);
    let body = page.text.clone();
    assert!(
        body.contains("href=\"/inbox\">Inbox"),
        "nav should read Inbox"
    );
    assert!(
        body.contains("href=\"/approvals\">Approvals"),
        "nav should read Approvals"
    );
    assert!(!body.contains(">Jobs<"), "Jobs label must be gone");
    assert!(
        !body.contains(">Decisions<"),
        "Decisions label must be gone"
    );
    assert_eq!(
        page.headers
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store"),
        "signed-in HTML must never be browser-cached"
    );
}

#[sqlx::test]
async fn attention_redirect_is_see_other_and_uncacheable(pool: PgPool) {
    // A permanent (301) redirect is cached forever by browsers, which is how
    // the pre-pivot /yours -> /inbox redirect outlived its code on user
    // machines. Navigation redirects must be See Other + no-store.
    let res = send(pool.clone(), get_request("/yours/attention", None)).await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store"),
        "redirects must not be cacheable"
    );
}

#[sqlx::test]
async fn human_assign_puts_queued_task_on_a_bot(pool: PgPool) {
    let cookie = sign_in(&pool, "assigner@example.com", "assigner").await;

    let task_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/tasks",
            json!({ "title": "Score the new lead" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(task_res.status, StatusCode::CREATED);
    let task_id: Uuid = task_res.body["task"]["id"]
        .as_str()
        .expect("task id")
        .parse()
        .expect("uuid");

    let user_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind("assigner@example.com")
        .fetch_one(&pool)
        .await
        .expect("user id");
    let agent_id = Uuid::new_v4();
    let key_hash = secret::hash_secret("assigner-bot-secret").expect("hash");
    sqlx::query(
        "INSERT INTO agents (id, user_id, name, key_hash, key_prefix)
         VALUES ($1, $2, 'assigner-bot', $3, 'assign')",
    )
    .bind(agent_id)
    .bind(user_id)
    .bind(&key_hash)
    .execute(&pool)
    .await
    .expect("insert agent");

    // The human-side assign lands the queued task on their bot.
    let res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/tasks/{task_id}/assign"),
            json!({ "agent_id": agent_id, "idempotency_key": "assign-1" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.body["task"]["status"], "assigned");

    // Retrying with the same key is a no-op success, not a 409 or a duplicate.
    let again = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/tasks/{task_id}/assign"),
            json!({ "agent_id": agent_id, "idempotency_key": "assign-1" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(again.status, StatusCode::OK);

    // The task left the queue: a different bot cannot be assigned to it.
    let other_id = Uuid::new_v4();
    let other_hash = secret::hash_secret("other-bot-secret").expect("hash");
    sqlx::query(
        "INSERT INTO agents (id, user_id, name, key_hash, key_prefix)
         VALUES ($1, $2, 'other-bot', $3, 'other')",
    )
    .bind(other_id)
    .bind(user_id)
    .bind(&other_hash)
    .execute(&pool)
    .await
    .expect("insert second agent");
    let steal = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/tasks/{task_id}/assign"),
            json!({ "agent_id": other_id }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(steal.status, StatusCode::CONFLICT);

    // Someone else's bot is refused outright.
    let cookie_mallory = sign_in(&pool, "mallory@example.com", "mallory").await;
    let foreign = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/tasks/{task_id}/assign"),
            json!({ "agent_id": agent_id }),
            Some(&cookie_mallory),
        ),
    )
    .await;
    assert_eq!(foreign.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn decision_page_shows_one_card_send_hold_and_counts(pool: PgPool) {
    let cookie = sign_in(&pool, "decider@example.com", "decider").await;

    let task_res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/tasks",
            json!({ "title": "Send the welcome note to Anna" }),
            Some(&cookie),
        ),
    )
    .await;
    let task_id: Uuid = task_res.body["task"]["id"]
        .as_str()
        .expect("task id")
        .parse()
        .expect("uuid");
    let ws_id: Uuid = task_res.body["task"]["workspace_id"]
        .as_str()
        .expect("workspace id")
        .parse()
        .expect("uuid");

    // Empty state first: it is the normal state.
    let mut empty_req = get_request("/approvals", Some(&cookie));
    empty_req
        .headers_mut()
        .insert("accept", "text/html".parse().expect("header value"));
    let empty = send(pool.clone(), empty_req).await;
    assert_eq!(empty.status, StatusCode::OK);
    assert!(empty.text.contains("Nothing needs you."));
    assert!(empty.text.contains("handled without you today"));

    sqlx::query(
        "INSERT INTO approvals (id, workspace_id, task_id, requested_action, risk_level, status, requested_by, created_at, updated_at)
         VALUES ($1, $2, $3, 'send the message', 'high', 'pending', 'agent:decider-bot', now(), now())",
    )
    .bind(Uuid::new_v4())
    .bind(ws_id)
    .bind(task_id)
    .execute(&pool)
    .await
    .expect("insert approval");

    let mut req = get_request("/approvals", Some(&cookie));
    req.headers_mut()
        .insert("accept", "text/html".parse().expect("header value"));
    let page = send(pool.clone(), req).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.text.contains("One approval needs you."));
    assert!(page.text.contains("can't do it on its own"));
    assert!(page.text.contains("welcome note to Anna"));
    assert!(page.text.contains("Give it to a different bot"));
    assert!(page.text.contains(">Send<"));
    assert!(page.text.contains(">Not yet<"));
    assert!(
        !page.text.contains("Needs Your OK"),
        "old label must be gone"
    );
    assert!(
        !page.text.contains(">Approve<"),
        "verbs name the action now"
    );
}

#[sqlx::test]
async fn admin_panel_is_gated_and_controls_onboarding(pool: PgPool) {
    // The earliest account bootstraps to platform admin; later ones cannot
    // see /admin at all.
    let owner = sign_in(&pool, "boss@example.com", "boss").await;
    let guest = sign_in(&pool, "guest@example.com", "guest").await;

    // /api/me reports effective admin, so the app shows the Admin screen to
    // the bootstrap owner before they have ever opened it.
    let me_owner = send(
        pool.clone(),
        authed_json_request("GET", "/api/me", json!({}), Some(&owner)),
    )
    .await;
    assert_eq!(me_owner.body["is_admin"], true);
    let me_guest = send(
        pool.clone(),
        authed_json_request("GET", "/api/me", json!({}), Some(&guest)),
    )
    .await;
    assert_eq!(me_guest.body["is_admin"], false);

    let ok = send(
        pool.clone(),
        authed_json_request("GET", "/admin", json!({}), Some(&owner)),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(ok.body["counts"]["users"].as_i64(), Some(2));

    let denied = send(
        pool.clone(),
        authed_json_request("GET", "/admin", json!({}), Some(&guest)),
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);

    // The tour is "every visit" by default while the product is being demoed.
    let st = send(
        pool.clone(),
        authed_json_request("GET", "/onboarding/state", json!({}), Some(&guest)),
    )
    .await;
    assert_eq!(st.status, StatusCode::OK);
    assert_eq!(st.body["mode"], "every");
    assert_eq!(st.body["is_admin"], false);

    // Only an admin changes the mode.
    let blocked = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/admin/settings",
            json!({ "onboarding_mode": "never", "idempotency_key": "x" }),
            Some(&guest),
        ),
    )
    .await;
    assert_eq!(blocked.status, StatusCode::FORBIDDEN);

    let save = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/admin/settings",
            json!({ "onboarding_mode": "once", "idempotency_key": "k1" }),
            Some(&owner),
        ),
    )
    .await;
    assert_eq!(save.status, StatusCode::OK);
    let st2 = send(
        pool.clone(),
        authed_json_request("GET", "/onboarding/state", json!({}), Some(&guest)),
    )
    .await;
    assert_eq!(st2.body["mode"], "once");

    // Dismissing the tour is recorded per person.
    let done = send(
        pool.clone(),
        authed_json_request("POST", "/onboarding/done", json!({}), Some(&guest)),
    )
    .await;
    assert_eq!(done.status, StatusCode::OK);
    let st3 = send(
        pool.clone(),
        authed_json_request("GET", "/onboarding/state", json!({}), Some(&guest)),
    )
    .await;
    assert_eq!(st3.body["done"], true);

    // Granting admin is explicit, so a retried submit cannot flip twice.
    let guest_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind("guest@example.com")
        .fetch_one(&pool)
        .await
        .expect("guest id");
    for _ in 0..2 {
        let grant = send(
            pool.clone(),
            authed_json_request(
                "POST",
                &format!("/admin/users/{guest_id}/admin"),
                json!({ "is_admin": true, "idempotency_key": "g1" }),
                Some(&owner),
            ),
        )
        .await;
        assert_eq!(grant.status, StatusCode::OK);
    }
    let st4 = send(
        pool.clone(),
        authed_json_request("GET", "/onboarding/state", json!({}), Some(&guest)),
    )
    .await;
    assert_eq!(st4.body["is_admin"], true);

    // The JSON panel lists people (with the caller marked) and stats the SPA
    // admin screen renders.
    let panel = send(
        pool.clone(),
        authed_json_request("GET", "/admin", json!({}), Some(&owner)),
    )
    .await;
    assert_eq!(panel.status, StatusCode::OK);
    assert_eq!(panel.body["counts"]["new_users_7d"].as_i64(), Some(2));
    let people = panel.body["users"].as_array().expect("users list");
    assert_eq!(people.len(), 2);
    let me = people
        .iter()
        .find(|u| u["email"] == "boss@example.com")
        .expect("owner row");
    assert_eq!(me["is_self"], true);
    assert_eq!(me["is_admin"], true);
    assert!(panel.body["agents"].is_array());

    // No admin can remove their own access, so the platform never ends up
    // with nobody to manage it.
    let owner_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind("boss@example.com")
        .fetch_one(&pool)
        .await
        .expect("owner id");
    let self_demote = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/admin/users/{owner_id}/admin"),
            json!({ "is_admin": false, "idempotency_key": "self" }),
            Some(&owner),
        ),
    )
    .await;
    assert_eq!(self_demote.status, StatusCode::BAD_REQUEST);
    let still_admin: bool = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
        .bind(owner_id)
        .fetch_one(&pool)
        .await
        .expect("owner flag");
    assert!(still_admin);

    // Another admin can still revoke it.
    let revoke_other = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/admin/users/{owner_id}/admin"),
            json!({ "is_admin": false, "idempotency_key": "other" }),
            Some(&guest),
        ),
    )
    .await;
    assert_eq!(revoke_other.status, StatusCode::OK);
}

#[sqlx::test]
async fn agents_get_handles_characters_and_are_searchable(pool: PgPool) {
    let cookie = sign_in(&pool, "gardener@example.com", "gardener").await;

    // Create a software agent: name, handle, character. No device involved.
    let res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/agents",
            json!({
                "name": "Cactus",
                "handle": "cactus",
                "instructions": "You are Cactus, a calm plant. Talk briefly about water and sun.",
                "idempotency_key": "cactus-1"
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.body["agent"]["handle"], "cactus");

    // Handles are unique: a second agent with the same handle gets a suffix.
    let dup = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/agents",
            json!({ "name": "Cactus two", "handle": "cactus", "idempotency_key": "cactus-2" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(dup.status, StatusCode::OK);
    assert_ne!(dup.body["agent"]["handle"], "cactus");

    // The page is searchable by @handle and shows the character state.
    let mut req = get_request("/agents?q=cactus", Some(&cookie));
    req.headers_mut()
        .insert("accept", "text/html".parse().expect("header value"));
    let page = send(pool.clone(), req).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.text.contains("@cactus"));
    assert!(page.text.contains("Has a character you wrote."));

    // The character is what the agent answers with.
    let ins: String = sqlx::query_scalar("SELECT instructions FROM agents WHERE handle = 'cactus'")
        .fetch_one(&pool)
        .await
        .expect("instructions");
    assert!(ins.contains("calm plant"));

    // Machine clients on the same URL still get JSON.
    let json = send(
        pool.clone(),
        authed_json_request("GET", "/agents", json!({}), Some(&cookie)),
    )
    .await;
    assert_eq!(json.status, StatusCode::OK);
}

#[sqlx::test]
async fn routing_memory_sends_jobs_where_you_sent_them(pool: PgPool) {
    let cookie = sign_in(&pool, "router@example.com", "router").await;

    let agent = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/agents",
            json!({ "name": "Grok", "handle": "grok", "idempotency_key": "g" }),
            Some(&cookie),
        ),
    )
    .await;
    let agent_id: Uuid = agent.body["agent"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("u");

    // Assign one welcome note by hand; the pattern is remembered.
    let t1 = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/tasks",
            json!({ "title": "Welcome note to Anna" }),
            Some(&cookie),
        ),
    )
    .await;
    let t1_id: Uuid = t1.body["task"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("u");
    let assign = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/tasks/{t1_id}/assign"),
            json!({ "agent_id": agent_id }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(assign.status, StatusCode::OK);

    // A job of the same kind now routes itself, visibly.
    let t2 = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/tasks",
            json!({ "title": "Welcome note to Marco" }),
            Some(&cookie),
        ),
    )
    .await;
    let t2_id: Uuid = t2.body["task"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("u");
    let (status, routed): (String, Option<Uuid>) =
        sqlx::query_as("SELECT status, assigned_agent_id FROM tasks WHERE id = $1")
            .bind(t2_id)
            .fetch_one(&pool)
            .await
            .expect("task");
    assert_eq!(
        status, "assigned",
        "the learned route is applied on creation"
    );
    assert_eq!(routed, Some(agent_id));

    // The reason is on the record.
    let reason: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE task_id = $1 AND action = 'task.assigned'",
    )
    .bind(t2_id)
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert_eq!(reason, 1);
}

#[sqlx::test]
async fn threads_get_topic_titles_by_hand_or_automatically(pool: PgPool) {
    let cookie = sign_in(&pool, "titler@example.com", "titler").await;

    // A sentence-shaped title gets rewritten without a voice: trimmed to
    // its first words as a topic.
    let long = "chat with AGI VPS or startup ideas validation and stuff";
    let res = send(
        pool.clone(),
        authed_json_request("POST", "/threads", json!({ "title": long }), Some(&cookie)),
    )
    .await;
    let thread_id: Uuid = res.body["id"].as_str().expect("id").parse().expect("u");

    let mut new_title = String::new();
    for _ in 0..60 {
        new_title = sqlx::query_scalar("SELECT title FROM threads WHERE id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("title");
        if new_title != long {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert_ne!(new_title, long, "the sentence becomes a topic");
    assert!(
        new_title.split_whitespace().count() <= 5,
        "topic titles stay short: {new_title}"
    );

    // And a human can always override by hand.
    let rename = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/threads/{thread_id}/rename"),
            json!({ "title": "Cactus check-in", "idempotency_key": "r1" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(rename.status, StatusCode::OK);
    let final_title: String = sqlx::query_scalar("SELECT title FROM threads WHERE id = $1")
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .expect("title");
    assert_eq!(final_title, "Cactus check-in");
}

#[sqlx::test]
async fn a_character_without_a_voice_says_so_once(pool: PgPool) {
    let cookie = sign_in(&pool, "mute@example.com", "mute").await;

    let agent = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/agents",
            json!({
                "name": "Cactus",
                "handle": "cactus",
                "instructions": "You are Cactus, a calm plant."
            }),
            Some(&cookie),
        ),
    )
    .await;
    let agent_id: Uuid = agent.body["agent"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("u");

    let res = send(
        pool.clone(),
        authed_json_request(
            "POST",
            "/threads",
            json!({ "title": "Chat" }),
            Some(&cookie),
        ),
    )
    .await;
    let thread_id: Uuid = res.body["id"].as_str().expect("id").parse().expect("u");
    sqlx::query(
        "INSERT INTO thread_agents (thread_id, agent_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(thread_id)
    .bind(agent_id)
    .execute(&pool)
    .await
    .expect("link");

    // Talking to it without any AI voice gets one honest note, not silence.
    let say = send(
        pool.clone(),
        authed_json_request(
            "POST",
            &format!("/threads/{thread_id}/notes"),
            json!({ "body": "How are you?", "kind": "command" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(say.status, StatusCode::CREATED);

    let mut voice_notes = 0;
    for _ in 0..60 {
        voice_notes = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM notes WHERE thread_id = $1 AND body LIKE '%needs an AI voice%'",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .expect("count");
        if voice_notes > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert_eq!(voice_notes, 1, "it says so exactly once");
}
