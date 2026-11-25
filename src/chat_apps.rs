//! What every chat app (Telegram, Slack) does with an office the same way
//! (issue #33): post the owner's message in the office's group chat,
//! decide an approval as the owner, and list approvals to send.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::Caller;
use crate::handlers::JsonOrForm;
use crate::models::{NoteKind, User};
use crate::state::AppState;

async fn owner(pool: &PgPool, owner_user_id: Uuid) -> Result<User, String> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1 AND deleted_at IS NULL")
        .bind(owner_user_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "the office's owner is gone".to_string())
}

/// Posts `text` in the office's group chat as its owner and has the bots
/// answer, exactly as a message typed in the app. `key` (the chat app's
/// message id) makes a re-delivered message post once.
pub async fn post_in_office_chat(
    state: &AppState,
    owner_user_id: Uuid,
    office: Uuid,
    text: &str,
    key: &str,
) -> Result<(), String> {
    let user = owner(&state.pool, owner_user_id).await?;
    let chat = crate::office::group::ensure_office_chat(&state.pool, user.id, office)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let thread = crate::handlers::threads::find_thread(&state.pool, chat)
        .await
        .map_err(|e| format!("{e:?}"))?
        .ok_or("the office chat is gone")?;
    let text: String = text.chars().take(10_000).collect();
    crate::handlers::threads::post_and_answer(
        state,
        &thread,
        &Caller::User(user),
        &text,
        NoteKind::Status,
        Some(key),
        true,
    )
    .await
    .map_err(|e| format!("{e:?}"))?;
    Ok(())
}

/// Approves or rejects an approval of `office` as its owner, through the
/// same decision as the app. Err with a short reason when it can't.
pub async fn decide_as_owner(
    state: &AppState,
    owner_user_id: Uuid,
    office: Uuid,
    approval: Uuid,
    approve: bool,
) -> Result<(), &'static str> {
    let in_office: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM approvals WHERE id = $1 AND workspace_id = $2 AND deleted_at IS NULL)",
    )
    .bind(approval)
    .bind(office)
    .fetch_one(&state.pool)
    .await
    .map_err(|_| "That didn't work. Try in the app.")?;
    if !in_office {
        return Err("That approval is gone.");
    }
    let user = owner(&state.pool, owner_user_id)
        .await
        .map_err(|_| "That didn't work. Try in the app.")?;
    crate::handlers::airo::decide_approval(
        State(state.clone()),
        Caller::User(user),
        Path(approval),
        HeaderMap::new(),
        JsonOrForm(crate::handlers::airo::DecideApprovalPayload {
            decision: if approve { "approve" } else { "reject" }.to_string(),
            reason: None,
            idempotency_key: None,
            changes: None,
        }),
    )
    .await
    .map(|_| ())
    .map_err(|_| "That didn't work. Try in the app.")
}

/// A pending approval to send: id, what the bot asks, who asks, when.
pub struct PendingApproval {
    pub id: Uuid,
    pub action: String,
    pub asked_by: String,
    pub created_at: DateTime<Utc>,
}

/// The office's pending approvals created after `since`, oldest first.
pub async fn approvals_since(
    pool: &PgPool,
    office: Uuid,
    since: DateTime<Utc>,
) -> Vec<PendingApproval> {
    let rows: Vec<(Uuid, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, requested_action, requested_by, created_at FROM approvals
         WHERE workspace_id = $1 AND status = 'pending' AND deleted_at IS NULL
           AND created_at > $2
         ORDER BY created_at ASC LIMIT 10",
    )
    .bind(office)
    .bind(since)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|(id, action, by, created_at)| {
            let who = by.trim().trim_start_matches("agent:").replace('_', " ");
            PendingApproval {
                id,
                action,
                asked_by: if who.is_empty() {
                    "A bot".to_string()
                } else {
                    who
                },
                created_at,
            }
        })
        .collect()
}

/// The office whose group chat `thread` is, if any.
pub async fn office_of_chat(pool: &PgPool, thread: Uuid) -> Option<Uuid> {
    sqlx::query_scalar(
        "SELECT workspace_id FROM threads WHERE id = $1 AND kind = 'office' AND deleted_at IS NULL",
    )
    .bind(thread)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .flatten()
}
