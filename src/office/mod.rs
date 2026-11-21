pub mod access;
pub mod architect;
pub mod budget_gate;
pub mod context;
pub mod daily;
pub mod evolver;
pub mod fsm_dispatcher;
pub mod group;
pub mod kickoff;
pub mod memory;

use sqlx::PgPool;
use uuid::Uuid;

use crate::state::{AppState, NoteEvent};

const OFFICE_BOT_NAME: &str = "Office Bot";

/// The workspace owner's own "Office Bot" agent, created on first use.
/// Server-side office notes are always authored by an agent that belongs
/// to the owner of the thread they land in -- never by some other user's
/// agent. Its key is never shown to anyone, so it cannot authenticate.
pub async fn office_bot_agent(pool: &PgPool, owner_user_id: Uuid) -> Result<Uuid, sqlx::Error> {
    owned_bot(
        pool,
        owner_user_id,
        OFFICE_BOT_NAME,
        "office-bot",
        "Posts updates from your virtual office.",
    )
    .await
}

/// A bot the server makes for its owner (found by name, created once). Its
/// key is never shown to anyone, so it cannot authenticate as a client.
pub async fn owned_bot(
    pool: &PgPool,
    owner_user_id: Uuid,
    name: &str,
    handle_base: &str,
    instructions: &str,
) -> Result<Uuid, sqlx::Error> {
    let find = || {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM agents WHERE user_id = $1 AND name = $2 AND deleted_at IS NULL",
        )
        .bind(owner_user_id)
        .bind(name)
    };
    if let Some(id) = find().fetch_optional(pool).await? {
        return Ok(id);
    }

    let id = Uuid::new_v4();
    let (_raw_key, raw_secret, key_prefix) = crate::auth::generate_agent_key(id);
    let key_hash = crate::auth::secret::hash_secret(&raw_secret)
        .map_err(|e| sqlx::Error::Protocol(format!("hash bot key: {e}")))?;
    let handle = crate::handlers::unique_handle(pool, handle_base).await;
    sqlx::query(
        "INSERT INTO agents (id, user_id, name, handle, instructions, key_hash, key_prefix)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (user_id, name) WHERE deleted_at IS NULL DO NOTHING",
    )
    .bind(id)
    .bind(owner_user_id)
    .bind(name)
    .bind(handle)
    .bind(instructions)
    .bind(key_hash)
    .bind(key_prefix)
    .execute(pool)
    .await?;
    // A concurrent caller may have won the insert; read back whichever row exists.
    find().fetch_one(pool).await
}

/// The general assistant everyone has from their first sign-in, and its room.
pub const ASSISTANT_NAME: &str = "Assistant";
pub const ASSISTANT_ROOM: &str = "#assistant";
const ASSISTANT_INSTRUCTIONS: &str = "You are Assistant, a friendly general-purpose AI helper. Answer any question clearly and helpfully: writing, research, planning, coding, ideas or everyday questions. Keep answers concise unless asked for detail.";

/// Who answers in each office channel (channel, bot name, handle, character).
pub const OFFICE_ROLES: [(&str, &str, &str, &str); 6] = [
    (
        "#leads-crm",
        "Sales Lead",
        "sales-lead",
        "You run sales for this office: leads, follow-ups, pipeline and customer messages. Write ready-to-send drafts and keep the pipeline moving. Be concise.",
    ),
    (
        "#social-video",
        "Studio Producer",
        "studio",
        "You run social media and video for this office: post ideas, captions, scripts and a posting plan. Be creative and concise.",
    ),
    (
        "#finance-ops",
        "Finance Manager",
        "finance",
        "You run finance for this office: invoices, payments, budgets and cash flow. Be precise, flag risks, and keep answers short.",
    ),
    (
        "#support-desk",
        "Support Agent",
        "support",
        "You run customer support for this office: answer customer questions, draft friendly replies, and track open issues until they are solved. Be warm and concise.",
    ),
    (
        "#bookings",
        "Scheduler",
        "scheduler",
        "You run bookings for this office: appointments, reminders, reschedules and the calendar. Confirm times clearly and keep answers short.",
    ),
    (
        "#store-ops",
        "Store Manager",
        "store",
        "You run the online store for this office: orders, stock, products and customer questions about orders. Flag problems early and be concise.",
    ),
];

/// Gives a person the bots they need to chat from day one, without
/// changing anything they set up themselves. Safe to call on every sign-in:
/// - the general Assistant and its `#assistant` room (made once; a room or
///   Assistant the person deleted is not brought back),
/// - a bot in each office channel that has never had one, so a message in
///   `#leads-crm` and friends always gets an answer,
/// - a group chat for each office, with the Architect in it.
pub async fn ensure_starter_bots(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<(), crate::error::AppError> {
    let workspace = crate::handlers::airo::get_or_create_default_workspace(pool, user_id).await?;

    let room_ever_made: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM threads WHERE user_id = $1 AND title = $2)",
    )
    .bind(user_id)
    .bind(ASSISTANT_ROOM)
    .fetch_one(pool)
    .await?;
    // Made once, with its room: an Assistant the person deleted stays gone.
    if !room_ever_made {
        let assistant = owned_bot(
            pool,
            user_id,
            ASSISTANT_NAME,
            "assistant",
            ASSISTANT_INSTRUCTIONS,
        )
        .await?;
        let mut tx = pool.begin().await?;
        let thread_id: Uuid = sqlx::query_scalar(
            "INSERT INTO threads (title, slug, user_id, workspace_id, visibility, status)
             VALUES ($1, $2, $3, $4, 'private', 'open')
             RETURNING id",
        )
        .bind(ASSISTANT_ROOM)
        .bind(format!(
            "assistant-{}",
            &Uuid::new_v4().simple().to_string()[..6]
        ))
        .bind(user_id)
        .bind(workspace.id)
        .fetch_one(&mut *tx)
        .await?;
        // No welcome note: notes reach connected agents' inboxes, and a
        // greeting there would read as work. The app's empty room says it.
        sqlx::query("INSERT INTO thread_agents (thread_id, agent_id) VALUES ($1, $2)")
            .bind(thread_id)
            .bind(assistant)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }

    // Every office's channels, not only the first office's.
    for (room, name, handle, instructions) in OFFICE_ROLES {
        let unstaffed: Vec<Uuid> = sqlx::query_scalar(
            "SELECT t.id FROM threads t
             WHERE t.user_id = $1 AND t.title = $2 AND t.deleted_at IS NULL AND t.kind = 'room'
               AND NOT EXISTS (SELECT 1 FROM thread_agents ta WHERE ta.thread_id = t.id)
             LIMIT 50",
        )
        .bind(user_id)
        .bind(room)
        .fetch_all(pool)
        .await?;
        if unstaffed.is_empty() {
            continue;
        }
        let bot = owned_bot(pool, user_id, name, handle, instructions).await?;
        for thread_id in unstaffed {
            sqlx::query(
                "INSERT INTO thread_agents (thread_id, agent_id) VALUES ($1, $2)
                 ON CONFLICT DO NOTHING",
            )
            .bind(thread_id)
            .bind(bot)
            .execute(pool)
            .await?;
        }
    }

    // Every office the person owns has its group chat.
    let offices: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workspaces WHERE owner_user_id = $1 AND deleted_at IS NULL LIMIT 100",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    for office in offices {
        group::ensure_office_chat(pool, user_id, office).await?;
    }
    Ok(())
}

/// [`ensure_starter_bots`] for sign-in paths: a failure is logged and never
/// stops someone signing in.
pub async fn ensure_starter_bots_logged(pool: &PgPool, user_id: Uuid) {
    if let Err(error) = ensure_starter_bots(pool, user_id).await {
        tracing::warn!(?error, %user_id, "could not set up starter bots");
    }
}

/// Posts a status note from the thread owner's Office Bot and streams it to
/// live listeners. Failures are logged, never fatal to the caller.
pub async fn post_office_note(state: &AppState, thread_id: Uuid, body: &str) {
    if let Err(error) = try_post_office_note(state, thread_id, body, None).await {
        tracing::warn!(%error, %thread_id, "failed to post office note");
    }
}

/// [`post_office_note`] that reports failure and posts at most once per
/// `idempotency_key` in the room. `Ok(false)`: already posted (or the room
/// is gone).
pub async fn try_post_office_note(
    state: &AppState,
    thread_id: Uuid,
    body: &str,
    idempotency_key: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let Some(owner) = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM threads WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(thread_id)
    .fetch_optional(&state.pool)
    .await?
    else {
        return Ok(false);
    };
    let bot = office_bot_agent(&state.pool, owner).await?;

    let mut tx = state.pool.begin().await?;
    let inserted: Option<(Uuid, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "INSERT INTO notes (thread_id, author_agent_id, body, kind, idempotency_key)
         VALUES ($1, $2, $3, 'status'::note_kind, $4)
         ON CONFLICT (thread_id, idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING
         RETURNING id, created_at",
    )
    .bind(thread_id)
    .bind(bot)
    .bind(body)
    .bind(idempotency_key)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((note_id, created_at)) = inserted else {
        return Ok(false);
    };
    sqlx::query(
        "UPDATE threads SET note_count = note_count + 1, last_note_at = now(), updated_at = now()
         WHERE id = $1",
    )
    .bind(thread_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let _ = state.note_sender.send(NoteEvent::new(
        thread_id,
        note_id,
        None,
        Some(OFFICE_BOT_NAME),
        body,
        "status",
        created_at,
    ));
    Ok(true)
}
