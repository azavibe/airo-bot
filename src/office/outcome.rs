//! Honest outcomes for a bot's jobs.
//!
//! A job is work a bot was given rather than a chat message: a task the
//! Architect handed it, a kickoff task, or a server task. The bot is asked
//! to end with one line saying how it went (`OUTCOME: done`, `OUTCOME:
//! nothing to do — why`, `OUTCOME: blocked — what it needs`, `OUTCOME:
//! needs your OK — what to approve`); the line leaves the message and an
//! italic line says it instead, and the job is kept as a `tasks` row
//! (`executor = 'chat'` for chat jobs) with its outcome, so the bot's
//! settings can list its recent work. A job that failed or hit a limit is
//! recorded too, so nothing reads as done that wasn't.

use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    NothingToDo,
    Blocked,
    NeedsOk,
    Failed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Done => "done",
            Outcome::NothingToDo => "nothing_to_do",
            Outcome::Blocked => "blocked",
            Outcome::NeedsOk => "needs_ok",
            Outcome::Failed => "failed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Outcome::Done => "Done",
            Outcome::NothingToDo => "Nothing to do",
            Outcome::Blocked => "Blocked",
            Outcome::NeedsOk => "Needs your OK",
            Outcome::Failed => "Failed",
        }
    }
}

/// What a bot doing a job is told about reporting it.
pub const INSTRUCTION: &str = "\n\n## Reporting this job\nEnd your message with one line saying honestly how the job went, exactly one of:\nOUTCOME: done\nOUTCOME: nothing to do — <why>\nOUTCOME: blocked — <what you need>\nOUTCOME: needs your OK — <what to approve>\nSay done only when the whole job is finished. Drafts waiting for the owner are \"needs your OK\"; missing access or information is \"blocked\".\n";

/// Splits the last `OUTCOME:` line off a reply: the reply without it, and
/// the outcome with its note (empty when none was given).
pub fn take(text: &str) -> (String, Option<(Outcome, String)>) {
    let lines: Vec<&str> = text.lines().collect();
    let Some(at) = lines.iter().rposition(|l| {
        l.trim()
            .trim_matches(|c| c == '*' || c == '_')
            .to_ascii_lowercase()
            .starts_with("outcome:")
    }) else {
        return (text.to_string(), None);
    };
    let line = lines[at].trim().trim_matches(|c| c == '*' || c == '_');
    let rest = line["outcome:".len()..]
        .trim()
        .trim_start_matches(['*', '_'])
        .trim();
    let lower = rest.to_lowercase();
    let (outcome, word_len) = [
        ("nothing to do", Outcome::NothingToDo),
        ("nothing_to_do", Outcome::NothingToDo),
        ("needs your ok", Outcome::NeedsOk),
        ("needs your okay", Outcome::NeedsOk),
        ("needs ok", Outcome::NeedsOk),
        ("needs_ok", Outcome::NeedsOk),
        ("needs approval", Outcome::NeedsOk),
        ("blocked", Outcome::Blocked),
        ("failed", Outcome::Failed),
        ("done", Outcome::Done),
    ]
    .iter()
    .find(|(word, _)| lower.starts_with(word))
    .map(|(word, o)| (*o, word.len()))
    .unwrap_or((Outcome::Done, 0));
    if word_len == 0 && !lower.is_empty() {
        // Not a line we understand: leave the message as it was.
        return (text.to_string(), None);
    }
    let note: String = rest
        .get(word_len..)
        .unwrap_or("")
        .trim_start_matches(|c: char| {
            c.is_whitespace() || matches!(c, '—' | '–' | '-' | ':' | '.' | ',')
        })
        .trim()
        .chars()
        .take(300)
        .collect();
    let mut kept: Vec<&str> = lines[..at].to_vec();
    kept.extend_from_slice(&lines[at + 1..]);
    (
        kept.join("\n").trim_end().to_string(),
        Some((outcome, note)),
    )
}

/// The line the chat shows in place of the `OUTCOME:` line.
pub fn line(outcome: Outcome, note: &str) -> String {
    if note.is_empty() {
        format!("*{}.*", outcome.label())
    } else {
        format!("*{}: {}*", outcome.label(), note.trim_end_matches('.'))
    }
}

/// The job's title from the line that gave it ("Architect (to Sales
/// Lead): Find 5 leads" -> "Find 5 leads").
pub fn title_of(task_line: &str) -> String {
    let job = task_line
        .split_once(": ")
        .map(|(_, rest)| rest)
        .unwrap_or(task_line)
        .trim();
    let title: String = job.chars().take(200).collect();
    if title.is_empty() {
        "A job".into()
    } else {
        title
    }
}

/// Keeps a finished chat job as a `tasks` row with its outcome. Best
/// effort: a failure is logged, never shown.
pub async fn record_job(
    pool: &PgPool,
    thread_id: Uuid,
    agent_id: Uuid,
    task_line: &str,
    outcome: Option<Outcome>,
    note: &str,
    credits: i64,
) {
    let failed = matches!(outcome, Some(Outcome::Failed));
    let result = sqlx::query(
        "INSERT INTO tasks (
            workspace_id, thread_id, title, status, payload, created_by_actor_type,
            assigned_agent_id, executor, fsm_state, outcome, outcome_note
         )
         SELECT COALESCE(t.workspace_id,
                    (SELECT w.id FROM workspaces w
                     WHERE w.owner_user_id = t.user_id AND w.deleted_at IS NULL
                     ORDER BY w.created_at ASC LIMIT 1)),
                t.id, $3, $4, $5, 'agent', $2, 'chat', $6, $7, $8
         FROM threads t WHERE t.id = $1",
    )
    .bind(thread_id)
    .bind(agent_id)
    .bind(title_of(task_line))
    .bind(if failed { "failed" } else { "completed" })
    .bind(json!({ "kind": "chat_job", "credits": credits }))
    .bind(if failed { "FAILED" } else { "COMPLETED" })
    .bind(outcome.map(Outcome::as_str))
    .bind((!note.is_empty()).then_some(note))
    .execute(pool)
    .await;
    if let Err(error) = result {
        tracing::warn!(?error, %thread_id, "could not record a bot's job");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_lines_are_read_and_removed() {
        let (body, got) =
            take("Here are 3 drafts.\n\nOUTCOME: needs your OK — send the 3 follow-ups");
        assert_eq!(body, "Here are 3 drafts.");
        assert_eq!(
            got,
            Some((Outcome::NeedsOk, "send the 3 follow-ups".into()))
        );
        let (_, got) = take("All set.\n**OUTCOME: done**");
        assert_eq!(got, Some((Outcome::Done, String::new())));
        let (_, got) = take("x\nOutcome: Nothing to do - no overdue invoices.");
        assert_eq!(
            got,
            Some((Outcome::NothingToDo, "no overdue invoices.".into()))
        );
        let (_, got) = take("x\nOUTCOME: blocked: connect Stripe first");
        assert_eq!(got, Some((Outcome::Blocked, "connect Stripe first".into())));
        let (body, got) = take("No outcome here.");
        assert_eq!((body.as_str(), got), ("No outcome here.", None));
        let (body, got) = take("x\nOUTCOME: maybe later");
        assert_eq!((body.as_str(), got), ("x\nOUTCOME: maybe later", None));
    }

    #[test]
    fn lines_and_titles() {
        assert_eq!(line(Outcome::Done, ""), "*Done.*");
        assert_eq!(
            line(Outcome::Blocked, "connect Stripe."),
            "*Blocked: connect Stripe*"
        );
        assert_eq!(
            title_of("Architect (to Sales Lead): Find 5 leads"),
            "Find 5 leads"
        );
        assert_eq!(
            title_of("Your task now, Finance Manager: Chase invoices"),
            "Chase invoices"
        );
    }
}
