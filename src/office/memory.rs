//! What an office remembers, and what its bots see of each other's work.
//!
//! - **Memory**: short facts the whole office (or one bot) always knows. The
//!   owner sees, edits and deletes every fact. A bot saves one by writing a
//!   `REMEMBER: <fact>` line; the line leaves the chat and the bot's message
//!   says "Remembered: ..." with an Undo button, so nothing is saved
//!   silently.
//! - **Lessons**: rules learned from the owner's approval decisions
//!   (`workspace_lessons`), now read by every bot, not only server tasks.
//! - **Recently in this office**: the newest messages from the office's
//!   other rooms, so rooms share context without another AI call.
//!
//! Everything lives in Postgres; there are no embeddings.
//! TODO(air): rooms longer than the 20 messages a reply sees have no
//! rolling summary yet; memory and the cross-room digest cover the facts
//! that matter. A summary would cost an AI call per long room.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::Caller;
use crate::error::AppError;
use crate::state::AppState;

/// Facts one office keeps; more are refused with a clear message.
pub const MAX_MEMORIES: i64 = 200;
/// Characters of one fact.
const MAX_FACT_CHARS: usize = 500;
/// Facts a bot may save in one message.
const MAX_SAVED_PER_ANSWER: usize = 3;
/// Characters of remembered facts in a prompt (pinned first, then newest).
const PROMPT_MEMORY_CHARS: usize = 4000;
/// Newest lessons in a prompt.
const PROMPT_LESSONS: i64 = 5;
/// Messages from other rooms in a prompt, and characters of each.
const RECENT_NOTES: i64 = 12;
const RECENT_NOTE_CHARS: usize = 220;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Memory {
    pub id: Uuid,
    pub workspace_id: Uuid,
    /// Set: only this bot knows it. Empty: the whole office.
    pub agent_id: Option<Uuid>,
    pub body: String,
    /// `owner` or `bot`.
    pub source: String,
    pub created_by_agent_id: Option<Uuid>,
    pub thread_id: Option<Uuid>,
    pub pinned: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const MEMORY_COLUMNS: &str =
    "id, workspace_id, agent_id, body, source, created_by_agent_id, thread_id, pinned, created_at, updated_at";

fn cut(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max {
        return text;
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn clean_fact(raw: &str) -> Result<String, AppError> {
    let fact = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if fact.is_empty() || fact.chars().count() > MAX_FACT_CHARS {
        return Err(AppError::BadRequest(format!(
            "a memory must be 1-{MAX_FACT_CHARS} characters"
        )));
    }
    Ok(fact)
}

// ---------- What a bot is told ----------

/// The memory, lessons and recent work an office bot is shown, appended to
/// its office prompt. Empty sections are left out.
pub async fn prompt_block(
    pool: &PgPool,
    workspace_id: Uuid,
    thread_id: Uuid,
    agent_id: Uuid,
) -> String {
    let mut out = String::new();

    let facts: Vec<(String, bool)> = sqlx::query_as(
        "SELECT body, pinned FROM memories
         WHERE workspace_id = $1 AND deleted_at IS NULL AND contact_id IS NULL
           AND (agent_id IS NULL OR agent_id = $2)
         ORDER BY pinned DESC, created_at DESC
         LIMIT 100",
    )
    .bind(workspace_id)
    .bind(agent_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    if !facts.is_empty() {
        out.push_str(
            "\n## What this office remembers\nFacts the owner (or a bot) saved. Follow them; they win over your own assumptions.\n",
        );
        let mut used = 0;
        for (fact, _) in facts {
            let line = format!("- {fact}\n");
            used += line.len();
            if used > PROMPT_MEMORY_CHARS {
                break;
            }
            out.push_str(&line);
        }
    }

    let lessons: Vec<String> = sqlx::query_scalar(
        "SELECT rule FROM workspace_lessons
         WHERE workspace_id = $1 AND deleted_at IS NULL
         ORDER BY created_at DESC LIMIT $2",
    )
    .bind(workspace_id)
    .bind(PROMPT_LESSONS)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    if !lessons.is_empty() {
        out.push_str("\n## Rules learned from the owner's decisions\n");
        for rule in lessons {
            out.push_str(&format!("- {}\n", cut(&rule, 300)));
        }
    }

    let recent = recent_elsewhere(pool, workspace_id, thread_id).await;
    if !recent.is_empty() {
        out.push_str(
            "\n## Recently in this office's other rooms\nNewest first. Use it for context, and don't redo work another bot already did.\n",
        );
        for line in recent {
            out.push_str(&format!("- {line}\n"));
        }
    }

    out.push_str(
        "\n## Remembering\nWhen the owner tells you something lasting about the business (a price, opening hours, a preference, a rule), save it: write one line exactly `REMEMBER: <the fact in one sentence>`. The line is saved to the office's memory and hidden from the chat. Don't save one-off details or facts already listed above.\n",
    );
    out
}

/// "[room] author: message" for the newest messages in the office's other
/// rooms over the last week.
async fn recent_elsewhere(pool: &PgPool, workspace_id: Uuid, thread_id: Uuid) -> Vec<String> {
    let rows: Vec<(String, String, Option<String>, i64, String, String)> = sqlx::query_as(
        "SELECT t.title, t.kind,
                (SELECT a2.name FROM thread_agents ta JOIN agents a2 ON a2.id = ta.agent_id
                  WHERE ta.thread_id = t.id AND a2.deleted_at IS NULL
                  ORDER BY a2.created_at LIMIT 1) AS bot,
                (SELECT COUNT(*) FROM thread_agents ta JOIN agents a3 ON a3.id = ta.agent_id
                  WHERE ta.thread_id = t.id AND a3.deleted_at IS NULL) AS bots,
                COALESCE(a.name, 'Owner') AS author,
                n.body
         FROM notes n
         JOIN threads t ON t.id = n.thread_id
         LEFT JOIN agents a ON a.id = n.author_agent_id
         WHERE t.workspace_id = $1 AND t.id <> $2
           AND t.deleted_at IS NULL AND n.deleted_at IS NULL
           AND n.created_at > now() - interval '7 days'
         ORDER BY n.created_at DESC
         LIMIT $3",
    )
    .bind(workspace_id)
    .bind(thread_id)
    .bind(RECENT_NOTES)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|(title, kind, bot, bots, author, body)| {
            let room = if kind == "office" {
                "group chat".to_string()
            } else if bots == 1 {
                bot.unwrap_or_else(|| title.trim_start_matches('#').to_string())
            } else {
                title.trim_start_matches('#').to_string()
            };
            format!("[{room}] {author}: {}", cut(&body, RECENT_NOTE_CHARS))
        })
        .collect()
}

// ---------- Facts a bot saves ----------

/// `REMEMBER: ...` lines in a bot's answer: the answer without them, and
/// the facts. Other text is unchanged.
pub fn parse_remember(text: &str) -> (Vec<String>, String) {
    let mut facts = Vec::new();
    let mut kept = Vec::new();
    for line in text.lines() {
        let trimmed = line
            .trim()
            .trim_start_matches(['-', '*', '`', ' '])
            .trim_end_matches('`');
        let fact = trimmed
            .get(..9)
            .filter(|p| p.eq_ignore_ascii_case("remember:"))
            .map(|_| trimmed[9..].trim());
        match fact {
            Some(f) if !f.is_empty() => {
                if facts.len() < MAX_SAVED_PER_ANSWER {
                    facts.push(cut(f, 300));
                }
            }
            _ => kept.push(line),
        }
    }
    (facts, kept.join("\n").trim().to_string())
}

/// Saves the facts a bot wrote, skipping ones the office already knows.
/// Returns the answer without the lines, and (id, fact) for each saved one.
pub async fn apply_remember(
    pool: &PgPool,
    thread_id: Uuid,
    agent_id: Uuid,
    text: &str,
) -> (String, Vec<(Uuid, String)>) {
    let (facts, rest) = parse_remember(text);
    if facts.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let Some(business) = super::context::business_of_thread(pool, thread_id).await else {
        return (rest, Vec::new());
    };
    let mut saved = Vec::new();
    for fact in facts {
        // `REMEMBER about <contact>: <fact>` is kept on that contact.
        let (contact, body) = match about_contact(&fact) {
            Some((name, rest)) => {
                match super::contacts::named(pool, business.workspace_id, name).await {
                    Some(contact) => (Some(contact), rest.to_string()),
                    None => (None, fact.clone()),
                }
            }
            None => (None, fact.clone()),
        };
        let id: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO memories (workspace_id, body, source, created_by_agent_id, thread_id, contact_id)
             SELECT $1, $2, 'bot', $3, $4, $6
             WHERE NOT EXISTS (SELECT 1 FROM memories
                               WHERE workspace_id = $1 AND deleted_at IS NULL
                                 AND contact_id IS NOT DISTINCT FROM $6
                                 AND lower(body) = lower($2))
               AND ($6 IS NOT NULL OR (SELECT COUNT(*) FROM memories
                    WHERE workspace_id = $1 AND deleted_at IS NULL AND contact_id IS NULL) < $5)
             RETURNING id",
        )
        .bind(business.workspace_id)
        .bind(&body)
        .bind(agent_id)
        .bind(thread_id)
        .bind(MAX_MEMORIES)
        .bind(contact)
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(?error, "bot memory not saved");
            None
        });
        if let Some(id) = id {
            saved.push((id, fact));
        }
    }
    (rest, saved)
}

/// `about <name>: <fact>` as (name, fact).
fn about_contact(fact: &str) -> Option<(&str, &str)> {
    let rest = fact
        .get(..6)
        .filter(|p| p.eq_ignore_ascii_case("about "))
        .map(|_| &fact[6..])?;
    let (name, body) = rest.split_once(':')?;
    let (name, body) = (name.trim(), body.trim());
    (!name.is_empty() && !body.is_empty()).then_some((name, body))
}

/// The "Remembered: ..." lines and Undo buttons for saved facts.
pub fn remembered_note(saved: &[(Uuid, String)]) -> (String, Vec<Value>) {
    let text = saved
        .iter()
        .map(|(_, fact)| format!("*Remembered: {fact}*"))
        .collect::<Vec<_>>()
        .join("\n");
    let buttons = saved
        .iter()
        .map(|(id, fact)| {
            let label = if saved.len() == 1 {
                "Undo".to_string()
            } else {
                format!("Forget \"{}\"", cut(fact, 24))
            };
            json!({ "label": label, "kind": "forget", "target": id })
        })
        .collect();
    (text, buttons)
}

// ---------- The owner's view ----------

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct LessonItem {
    pub id: Uuid,
    pub rule: String,
    pub source: String,
    pub created_at: DateTime<Utc>,
}

/// GET /api/offices/:id/memory -- the office's facts (office-wide and each
/// bot's), and the lessons learned from its approvals.
pub async fn list_memory(
    State(state): State<AppState>,
    caller: Caller,
    Path(office_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let Caller::User(user) = &caller else {
        return Err(AppError::Forbidden);
    };
    crate::handlers::offices::owned_office(&state.pool, user.id, office_id).await?;
    let memories = sqlx::query_as::<_, Memory>(&format!(
        "SELECT {MEMORY_COLUMNS} FROM memories
         WHERE workspace_id = $1 AND deleted_at IS NULL AND contact_id IS NULL
         ORDER BY pinned DESC, created_at DESC
         LIMIT $2"
    ))
    .bind(office_id)
    .bind(MAX_MEMORIES)
    .fetch_all(&state.pool)
    .await?;
    let lessons = sqlx::query_as::<_, LessonItem>(
        "SELECT id, rule, source, created_at FROM workspace_lessons
         WHERE workspace_id = $1 AND deleted_at IS NULL
         ORDER BY created_at DESC LIMIT 200",
    )
    .bind(office_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        json!({ "memories": memories, "lessons": lessons, "limit": MAX_MEMORIES }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct CreateMemoryRequest {
    pub body: String,
    /// Only this bot (one of the owner's) remembers it.
    #[serde(default)]
    pub agent_id: Option<Uuid>,
    #[serde(default)]
    pub pinned: Option<bool>,
    pub idempotency_key: Option<String>,
}

/// POST /api/offices/:id/memory
pub async fn create_memory(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(office_id): Path<Uuid>,
    Json(body): Json<CreateMemoryRequest>,
) -> Result<Json<Memory>, AppError> {
    let Caller::User(user) = &caller else {
        return Err(AppError::Forbidden);
    };
    let key = crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    crate::handlers::offices::owned_office(&state.pool, user.id, office_id).await?;
    let fact = clean_fact(&body.body)?;
    if let Some(agent_id) = body.agent_id {
        let mine: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE id = $1 AND user_id = $2 AND deleted_at IS NULL)",
        )
        .bind(agent_id)
        .bind(user.id)
        .fetch_one(&state.pool)
        .await?;
        if !mine {
            return Err(AppError::NotFound);
        }
    }
    if let Some(key) = key {
        if let Some(existing) = sqlx::query_as::<_, Memory>(&format!(
            "SELECT {MEMORY_COLUMNS} FROM memories
             WHERE workspace_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL"
        ))
        .bind(office_id)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?
        {
            return Ok(Json(existing));
        }
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM memories
         WHERE workspace_id = $1 AND deleted_at IS NULL AND contact_id IS NULL",
    )
    .bind(office_id)
    .fetch_one(&state.pool)
    .await?;
    if count >= MAX_MEMORIES {
        return Err(AppError::BadRequest(format!(
            "this office remembers {MAX_MEMORIES} things already; delete one first"
        )));
    }
    let saved = sqlx::query_as::<_, Memory>(&format!(
        "INSERT INTO memories (workspace_id, agent_id, body, source, pinned, idempotency_key)
         VALUES ($1, $2, $3, 'owner', $4, $5)
         RETURNING {MEMORY_COLUMNS}"
    ))
    .bind(office_id)
    .bind(body.agent_id)
    .bind(&fact)
    .bind(body.pinned.unwrap_or(false))
    .bind(key)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(saved))
}

/// A memory the caller owns (through its office).
async fn owned_memory(pool: &PgPool, user_id: Uuid, id: Uuid) -> Result<Memory, AppError> {
    sqlx::query_as::<_, Memory>(&format!(
        "SELECT {} FROM memories m
         JOIN workspaces w ON w.id = m.workspace_id
         WHERE m.id = $1 AND m.deleted_at IS NULL AND w.owner_user_id = $2 AND w.deleted_at IS NULL",
        MEMORY_COLUMNS
            .split(", ")
            .map(|c| format!("m.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)
}

#[derive(Debug, Deserialize)]
pub struct UpdateMemoryRequest {
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub pinned: Option<bool>,
    pub idempotency_key: Option<String>,
}

/// PATCH /api/memories/:id -- edit a fact or pin it (pinned facts come
/// first and always reach the bots).
pub async fn update_memory(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateMemoryRequest>,
) -> Result<Json<Memory>, AppError> {
    let Caller::User(user) = &caller else {
        return Err(AppError::Forbidden);
    };
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let memory = owned_memory(&state.pool, user.id, id).await?;
    let fact = match body.body.as_deref() {
        Some(raw) => Some(clean_fact(raw)?),
        None => None,
    };
    let updated = sqlx::query_as::<_, Memory>(&format!(
        "UPDATE memories SET body = COALESCE($2, body), pinned = COALESCE($3, pinned), updated_at = now()
         WHERE id = $1
         RETURNING {MEMORY_COLUMNS}"
    ))
    .bind(memory.id)
    .bind(fact)
    .bind(body.pinned)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(updated))
}

/// POST /api/memories/:id/delete -- forget a fact (also the chat's Undo).
pub async fn delete_memory(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let Caller::User(user) = &caller else {
        return Err(AppError::Forbidden);
    };
    crate::idempotency::key(&headers, None)?;
    let memory = owned_memory(&state.pool, user.id, id).await?;
    sqlx::query("UPDATE memories SET deleted_at = now(), updated_at = now() WHERE id = $1")
        .bind(memory.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "status": "deleted", "id": memory.id })))
}

/// POST /api/lessons/:id/delete -- drop a rule learned from a decision.
pub async fn delete_lesson(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let Caller::User(user) = &caller else {
        return Err(AppError::Forbidden);
    };
    crate::idempotency::key(&headers, None)?;
    let gone: Option<Uuid> = sqlx::query_scalar(
        "UPDATE workspace_lessons l SET deleted_at = now(), updated_at = now()
         FROM workspaces w
         WHERE l.id = $1 AND l.deleted_at IS NULL AND w.id = l.workspace_id
           AND w.owner_user_id = $2 AND w.deleted_at IS NULL
         RETURNING l.id",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?;
    let id = gone.ok_or(AppError::NotFound)?;
    Ok(Json(json!({ "status": "deleted", "id": id })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_lines_are_taken_out_of_the_answer() {
        let text = "Noted.\nREMEMBER: We close at 6pm on Saturdays.\n- remember: Deliveries only within 10 km\nRemember to call Ada.\nREMEMBER:";
        let (facts, rest) = parse_remember(text);
        assert_eq!(
            facts,
            vec![
                "We close at 6pm on Saturdays.",
                "Deliveries only within 10 km"
            ]
        );
        assert!(rest.starts_with("Noted."));
        assert!(
            rest.contains("Remember to call Ada."),
            "ordinary text stays"
        );
        assert!(!rest.contains("6pm"));
    }

    #[test]
    fn at_most_three_facts_per_answer_and_long_ones_are_cut() {
        let text = (0..5)
            .map(|n| format!("REMEMBER: fact {n} {}", "x".repeat(400)))
            .collect::<Vec<_>>()
            .join("\n");
        let (facts, rest) = parse_remember(&text);
        assert_eq!(facts.len(), 3);
        assert!(facts.iter().all(|f| f.chars().count() <= 300));
        assert!(rest.is_empty());
    }

    #[test]
    fn one_saved_fact_gets_an_undo_button() {
        let id = Uuid::new_v4();
        let (text, buttons) = remembered_note(&[(id, "Open 9-5".into())]);
        assert_eq!(text, "*Remembered: Open 9-5*");
        assert_eq!(buttons[0]["label"], "Undo");
        assert_eq!(buttons[0]["kind"], "forget");
        assert_eq!(buttons[0]["target"], id.to_string());
    }
}
