//! The office group chat: one thread per office where all of the office's
//! bots listen, and the Architect decides who handles a message.
//!
//! No embeddings or search: the Architect is shown a short roster (each
//! bot's @handle and the first line of its character) with the
//! conversation, and answers either directly or with delegation lines of
//! the form `@handle: what to do`. Every bot answers in the same thread, so
//! they all see the same conversation and share its context.
//! TODO(air): a roster is cheap and fine for a dozen bots; past ~30 bots
//! per office, add a per-bot skills line or retrieval, and measure first.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub const ARCHITECT_NAME: &str = "Architect";
pub const ARCHITECT_HANDLE: &str = "architect";
const ARCHITECT_INSTRUCTIONS: &str = "You are the Architect of this virtual office. You help the owner plan the business, decide what to automate, and set up apps, workflows and bots. In the office group chat you are also the coordinator: when a message needs one of your teammates, you hand it to them; otherwise you answer yourself. Be practical and concise, and suggest the next concrete step.";

/// Delegations per message; keeps one message from fanning out.
const MAX_DELEGATIONS: usize = 3;
/// Characters of a bot's character shown to the Architect.
const ROSTER_LINE_CHARS: usize = 160;

/// The Architect bot of `user_id` (made once, shared by their offices).
pub async fn architect_bot(pool: &PgPool, user_id: Uuid) -> Result<Uuid, sqlx::Error> {
    super::owned_bot(
        pool,
        user_id,
        ARCHITECT_NAME,
        ARCHITECT_HANDLE,
        ARCHITECT_INSTRUCTIONS,
    )
    .await
}

/// The office's group chat, made on first use. Its members are the
/// Architect and every bot that has a room in the office.
pub async fn ensure_office_chat(
    pool: &PgPool,
    user_id: Uuid,
    workspace_id: Uuid,
) -> Result<Uuid, AppError> {
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM threads
         WHERE workspace_id = $1 AND kind = 'office' AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?;
    let thread_id = match existing {
        Some(id) => id,
        None => {
            let slug = format!("office-{}", &Uuid::new_v4().simple().to_string()[..8]);
            let inserted: Option<Uuid> = sqlx::query_scalar(
                "INSERT INTO threads (title, slug, user_id, workspace_id, visibility, status, kind)
                 VALUES ('#office', $1, $2, $3, 'private', 'open', 'office')
                 ON CONFLICT (workspace_id) WHERE kind = 'office' AND deleted_at IS NULL
                 DO NOTHING
                 RETURNING id",
            )
            .bind(slug)
            .bind(user_id)
            .bind(workspace_id)
            .fetch_optional(pool)
            .await?;
            match inserted {
                Some(id) => id,
                // A concurrent caller made it first.
                None => {
                    sqlx::query_scalar(
                        "SELECT id FROM threads
                         WHERE workspace_id = $1 AND kind = 'office' AND deleted_at IS NULL",
                    )
                    .bind(workspace_id)
                    .fetch_one(pool)
                    .await?
                }
            }
        }
    };
    sync_members(pool, user_id, workspace_id, thread_id).await?;
    Ok(thread_id)
}

/// Makes the Architect and every bot with a room in the office members of
/// the group chat. Safe to repeat.
pub async fn sync_members(
    pool: &PgPool,
    user_id: Uuid,
    workspace_id: Uuid,
    thread_id: Uuid,
) -> Result<(), AppError> {
    let architect = architect_bot(pool, user_id).await?;
    sqlx::query(
        "INSERT INTO thread_agents (thread_id, agent_id)
         SELECT $1, ta.agent_id
         FROM thread_agents ta
         JOIN threads t ON t.id = ta.thread_id
         JOIN agents a ON a.id = ta.agent_id
         WHERE t.workspace_id = $2 AND t.kind = 'room' AND t.deleted_at IS NULL
           AND a.user_id = $3 AND a.deleted_at IS NULL AND a.revoked_at IS NULL
         ON CONFLICT DO NOTHING",
    )
    .bind(thread_id)
    .bind(workspace_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO thread_agents (thread_id, agent_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(thread_id)
    .bind(architect)
    .execute(pool)
    .await?;
    Ok(())
}

/// A teammate the Architect can hand work to.
pub struct Teammate {
    pub id: Uuid,
    pub name: String,
    pub handle: String,
    pub about: String,
}

/// The office's bots other than `except` (the Architect), by name.
pub async fn roster(
    pool: &PgPool,
    thread_id: Uuid,
    except: Uuid,
) -> Result<Vec<Teammate>, AppError> {
    let rows = sqlx::query_as::<_, (Uuid, String, Option<String>, Option<String>)>(
        "SELECT a.id, a.name, a.handle, a.instructions
         FROM thread_agents ta
         JOIN agents a ON a.id = ta.agent_id
         WHERE ta.thread_id = $1 AND a.id <> $2
           AND a.deleted_at IS NULL AND a.revoked_at IS NULL
         ORDER BY a.name
         LIMIT 50",
    )
    .bind(thread_id)
    .bind(except)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, name, handle, instructions)| {
            let handle = handle?;
            let about = instructions
                .as_deref()
                .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
                .map(|text| text.chars().take(ROSTER_LINE_CHARS).collect::<String>())
                .unwrap_or_default();
            Some(Teammate {
                id,
                name,
                handle,
                about,
            })
        })
        .collect())
}

/// What the Architect is told about the team, appended to its character.
pub fn roster_prompt(team: &[Teammate]) -> String {
    let mut out = String::from("\n\nYou coordinate this office's group chat. Your teammates:\n");
    for t in team {
        out.push_str(&format!("- @{} ({}): {}\n", t.handle, t.name, t.about));
    }
    out.push_str(
        "\nIf one or more teammates should handle the owner's message, reply with one line per teammate in exactly this form: @handle: what you want them to do. You may put one short sentence before those lines. If nobody on the team is right, or the owner is talking to you, answer yourself in plain text and use no @handle lines.",
    );
    out
}

/// Splits the Architect's answer into delegations and its own words.
/// Only handles on the roster count; anything else stays as plain text.
pub fn parse_delegations(text: &str, team: &[Teammate]) -> (Vec<(Uuid, String, String)>, String) {
    let mut delegations: Vec<(Uuid, String, String)> = Vec::new();
    let mut own: Vec<&str> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim().trim_start_matches(['-', '*', ' ']);
        if let Some(rest) = trimmed.strip_prefix('@') {
            let handle: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            let after = rest[handle.len()..].trim_start();
            let task = after
                .strip_prefix(':')
                .or_else(|| after.strip_prefix('-'))
                .or_else(|| after.strip_prefix('—'))
                .map(str::trim);
            if let (Some(task), Some(mate)) = (
                task.filter(|t| !t.is_empty()),
                team.iter().find(|t| t.handle.eq_ignore_ascii_case(&handle)),
            ) {
                if delegations.len() < MAX_DELEGATIONS
                    && !delegations.iter().any(|d| d.0 == mate.id)
                {
                    delegations.push((mate.id, mate.name.clone(), task.to_string()));
                }
                continue;
            }
        }
        own.push(line);
    }
    (delegations, own.join("\n").trim().to_string())
}

/// `@handle` mentions in a person's message that name a bot on the roster.
pub fn mentioned(text: &str, team: &[Teammate]) -> Vec<Uuid> {
    let mut found: Vec<Uuid> = Vec::new();
    for word in text.split_whitespace() {
        if let Some(rest) = word.strip_prefix('@') {
            let handle = rest.trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
            if let Some(mate) = team.iter().find(|t| t.handle.eq_ignore_ascii_case(handle)) {
                if !found.contains(&mate.id) && found.len() < MAX_DELEGATIONS {
                    found.push(mate.id);
                }
            }
        }
    }
    found
}

/// A person wrote in the office group chat. An @mention goes straight to
/// that bot; anything else goes to the Architect, who answers or hands it on.
pub async fn on_person_message(
    state: &AppState,
    thread_id: Uuid,
    workspace_id: Uuid,
    user_id: Uuid,
    text: &str,
) {
    let result: Result<(), AppError> = async {
        sync_members(&state.pool, user_id, workspace_id, thread_id).await?;
        let architect = architect_bot(&state.pool, user_id).await?;
        let team = roster(&state.pool, thread_id, architect).await?;
        let direct = mentioned(text, &team);
        if direct.is_empty() {
            crate::handlers::threads::spawn_agent_reply(
                state.clone(),
                thread_id,
                architect,
                user_id,
                crate::handlers::threads::ReplyMode::route(),
            );
        } else {
            for bot in direct {
                crate::handlers::threads::spawn_agent_reply(
                    state.clone(),
                    thread_id,
                    bot,
                    user_id,
                    crate::handlers::threads::ReplyMode::default(),
                );
            }
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(?error, %thread_id, "office group chat could not route a message");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team() -> Vec<Teammate> {
        ["sales-lead", "finance"]
            .iter()
            .map(|h| Teammate {
                id: Uuid::new_v4(),
                name: h.to_string(),
                handle: h.to_string(),
                about: String::new(),
            })
            .collect()
    }

    #[test]
    fn delegations_are_read_and_unknown_handles_stay_text() {
        let team = team();
        let text = "On it.\n@sales-lead: draft a follow-up for Acme\n- @finance - chase invoice 12\n@ghost: do things\n@sales-lead: again";
        let (calls, own) = parse_delegations(text, &team);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, team[0].id);
        assert_eq!(calls[0].2, "draft a follow-up for Acme");
        assert_eq!(calls[1].0, team[1].id);
        assert_eq!(calls[1].2, "chase invoice 12");
        // The unknown handle and the repeated one are not delegations.
        assert!(own.starts_with("On it."), "{own}");
        assert!(own.contains("@ghost: do things"), "{own}");
    }

    #[test]
    fn at_most_three_bots_get_a_message() {
        let team: Vec<Teammate> = (0..5)
            .map(|i| Teammate {
                id: Uuid::new_v4(),
                name: format!("b{i}"),
                handle: format!("b{i}"),
                about: String::new(),
            })
            .collect();
        let text: String = (0..5).map(|i| format!("@b{i}: go\n")).collect();
        assert_eq!(parse_delegations(&text, &team).0.len(), MAX_DELEGATIONS);
    }

    #[test]
    fn mentions_find_bots_on_the_roster_only() {
        let team = team();
        let found = mentioned("hey @Finance, and @nobody, also @sales-lead!", &team);
        assert_eq!(found, vec![team[1].id, team[0].id]);
        assert!(mentioned("email me at a@b.com", &team).is_empty());
    }

    #[test]
    fn plain_answers_have_no_delegations() {
        let (calls, own) = parse_delegations("Here is my plan.\nStep one.", &team());
        assert!(calls.is_empty());
        assert_eq!(own, "Here is my plan.\nStep one.");
    }
}
