use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::{broadcast, Notify};
use uuid::Uuid;

use crate::config::Config;
use crate::email::Mailer;
use crate::push::PushSender;

/// A note that was just written, fanned out to live listeners (SSE and
/// long-polling `/inbox`). `note` is the JSON the SPA appends directly; it
/// never carries pre-rendered HTML, so no consumer can be tricked into
/// injecting unescaped markup.
#[derive(Clone, Debug)]
pub struct NoteEvent {
    pub thread_id: Uuid,
    pub note_id: Uuid,
    pub note: serde_json::Value,
}

impl NoteEvent {
    /// Builds the event in the shape of the SPA's `Note` type.
    pub fn new(
        thread_id: Uuid,
        note_id: Uuid,
        author_user_id: Option<Uuid>,
        author_name: Option<&str>,
        body: &str,
        kind: &str,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        NoteEvent {
            thread_id,
            note_id,
            note: serde_json::json!({
                "id": note_id,
                "thread_id": thread_id,
                "body": body,
                "kind": kind,
                "author_user_id": author_user_id,
                "author_name": author_name,
                "author_kind": if author_user_id.is_some() { "human" } else { "agent" },
                "created_at": created_at,
            }),
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<Config>,
    pub mailer: Arc<Mailer>,
    pub push: Arc<PushSender>,
    pub note_sender: broadcast::Sender<NoteEvent>,
    /// App records that changed, for each office's live listeners.
    pub record_sender: broadcast::Sender<crate::office::records::RecordEvent>,
    /// Wakes the in-process FSM dispatcher immediately (webhook, approval,
    /// new server task) instead of waiting for its fallback poll. Air runs
    /// as a single replica, so an in-process signal is enough.
    pub dispatcher_wake: Arc<Notify>,
    /// The AI tool chosen on this computer, in local mode (desktop app).
    /// When set, replies and office tasks use it instead of the platform's
    /// key, and nothing is metered.
    pub local_ai: Option<crate::local::LocalAi>,
}
