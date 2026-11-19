//! Office kickoff: the team moves first.
//!
//! Right after an office is set up, its group chat is not empty: the
//! Architect says what will happen, each bot posts a short first-week plan
//! with one-tap buttons for the tasks it has not started, and the team
//! starts working at once, one bot after another, on its first task (with
//! web access, within the day's allowance). With a website, the Architect
//! first reads it and sums up the business, so every bot works from that.
//!
//! Plans are written here, not generated, so the kickoff is instant, costs
//! nothing until work starts, and reads the same every time.
//! TODO(air): let the Architect tailor the plans once the summary is in;
//! fixed plans are cheaper and predictable for a first release.

use serde_json::json;
use uuid::Uuid;

use crate::handlers::threads::{insert_bot_note, run_agent_reply, ReplyMode};
use crate::state::AppState;

/// What the team knows about the business on day one.
#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub name: String,
    pub business_type: Option<String>,
    pub website: Option<String>,
    pub focus: Option<String>,
}

impl Profile {
    fn business(&self) -> String {
        match &self.business_type {
            Some(b) if !b.trim().is_empty() && b != "Small business" => {
                format!("{} ({})", self.name, b.trim())
            }
            _ => self.name.clone(),
        }
    }

    fn context(&self) -> String {
        let mut out = String::new();
        if let Some(site) = &self.website {
            out.push_str(&format!(" Our website is {site}."));
        }
        if let Some(focus) = &self.focus {
            out.push_str(&format!(" What matters most this month: {focus}."));
        }
        out
    }
}

/// A bot's first-week plan, by bot name (see `office::OFFICE_ROLES`).
/// The first task puts work into the bot's own app (the office's CRM,
/// bookings, support desk, invoices, studio) for this business; nothing
/// asks the owner to research or buy other software.
pub fn starter_tasks(bot_name: &str, p: &Profile) -> Option<[String; 3]> {
    let biz = p.business();
    let ctx = p.context();
    let tasks = match bot_name {
        "Sales Lead" => [
            format!("Fill the Lead Pipeline with 5 realistic leads {biz} should win this month (the kinds of customers who buy from a business like this), each with a stage, an estimated value and the next step.{ctx}"),
            "Write a short, warm reply to a new enquiry and a follow-up to send 3 days later.".to_string(),
            "Pick the 3 leads most likely to buy and say what to do with each today.".to_string(),
        ],
        "Studio Producer" => [
            format!("Plan 5 posts for {biz} this week and add each one to the studio as a draft with its hook and platform.{ctx}"),
            format!("Write a 30-second video script about what makes {biz} special."),
            "Say when and where to post over the next two weeks, in a short list.".to_string(),
        ],
        "Finance Manager" => [
            format!("Set up Invoices & Cashflow for {biz}: add the invoices a business like this would send this month, with realistic clients, amounts and due dates.{ctx}"),
            "Write a friendly payment reminder and a firmer second one.".to_string(),
            format!("Name the 5 numbers {biz} should check every week, one line each."),
        ],
        "Support Agent" => [
            format!("Add the 3 customer requests {biz} most likely gets this week to the Support Inbox, each with a ready-to-send reply in its note.{ctx}"),
            "Write a reply for an unhappy customer that keeps them.".to_string(),
            format!("Write short answers to the 5 questions customers of {biz} ask most."),
        ],
        "Scheduler" => [
            format!("Add the next few typical bookings for {biz} to Appointments, and set its booking rules (hours, buffers, cancellations, reminders) in 4 short lines.{ctx}"),
            "Write the booking confirmation and the reminder message.".to_string(),
            "Write a no-show policy that is fair but firm, in 3 lines.".to_string(),
        ],
        "Store Manager" => [
            format!("Suggest 3 quick wins for {biz}'s online store, one line each.{ctx}"),
            "Write reply templates for order problems: late, wrong item, refund.".to_string(),
            "Suggest a weekly stock and orders check that takes 15 minutes.".to_string(),
        ],
        _ => return None,
    };
    Some(tasks)
}

/// A button that sends `message` as the person's own message.
pub fn send_action(label: &str, message: &str) -> serde_json::Value {
    json!({ "label": label, "kind": "send", "message": message })
}

/// A plan as the bot says it: three numbered tasks, the first already
/// started.
fn plan_text(tasks: &[String; 3]) -> String {
    format!(
        "My plan for this week:\n1. {}\n2. {}\n3. {}\n\nStarting on 1 now.",
        tasks[0], tasks[1], tasks[2]
    )
}

/// Kicks the office off in its group chat. Posts at once; the work itself
/// runs in the background, one task at a time.
pub async fn start(state: &AppState, user_id: Uuid, office_chat: Uuid, profile: Profile) {
    let pool = &state.pool;
    // Today's work is the kickoff; the first daily plan comes tomorrow.
    let _ = sqlx::query(
        "UPDATE workspaces SET last_plan_at = now(), updated_at = now()
         WHERE id = (SELECT workspace_id FROM threads WHERE id = $1)",
    )
    .bind(office_chat)
    .execute(pool)
    .await;
    let architect = match super::group::architect_bot(pool, user_id).await {
        Ok(id) => id,
        Err(error) => {
            tracing::warn!(?error, "kickoff: no Architect");
            return;
        }
    };
    let team = match super::group::roster(pool, office_chat, architect).await {
        Ok(team) => team,
        Err(error) => {
            tracing::warn!(?error, "kickoff: no roster");
            return;
        }
    };
    let plans: Vec<(Uuid, String, String, [String; 3])> = team
        .into_iter()
        .filter_map(|t| {
            starter_tasks(&t.name, &profile).map(|tasks| (t.id, t.name, t.handle, tasks))
        })
        .collect();

    let mut intro = format!(
        "Welcome to {}. I'm the Architect. Write here and I'll hand each request to the right bot, or @mention a bot to talk to it directly.",
        profile.name
    );
    if plans.is_empty() {
        intro.push_str(" This office has no bots yet: set it up for the work you want done, or add a bot with + next to its name.");
        insert_bot_note(
            pool,
            &state.note_sender,
            office_chat,
            architect,
            &intro,
            Some(json!([{ "label": "Set up this office", "kind": "setup" }])),
        )
        .await;
        return;
    }
    intro.push_str(" The team is starting on its first tasks now, and each bot posts what it finds here. Tap a button to give a bot its next task.");
    if profile.website.is_some() {
        intro.push_str(" First I'll read your website so everyone works from the same picture.");
    }
    insert_bot_note(
        pool,
        &state.note_sender,
        office_chat,
        architect,
        &intro,
        None,
    )
    .await;

    for (bot, _, handle, tasks) in &plans {
        let actions = json!([
            send_action("Do task 2", &format!("@{handle}: {}", tasks[1])),
            send_action("Do task 3", &format!("@{handle}: {}", tasks[2])),
        ]);
        insert_bot_note(
            pool,
            &state.note_sender,
            office_chat,
            *bot,
            &plan_text(tasks),
            Some(actions),
        )
        .await;
    }

    // The work: the website summary first (so it is in everyone's
    // conversation), then each bot's first task, one at a time.
    let state = state.clone();
    tokio::spawn(async move {
        if let Some(site) = profile.website.clone() {
            let task = format!(
                "Read {site} and sum up in 5 short bullets what {} sells, to whom, how, and what makes it different. End by asking me to correct anything you got wrong.",
                profile.name
            );
            run_agent_reply(
                state.clone(),
                office_chat,
                architect,
                user_id,
                ReplyMode::task(super::group::ARCHITECT_NAME, &task),
            )
            .await;
        }
        for (bot, name, _, tasks) in plans {
            run_agent_reply(
                state.clone(),
                office_chat,
                bot,
                user_id,
                ReplyMode::task(&name, &tasks[0]),
            )
            .await;
        }
    });
}

/// Checks and normalises a website the person typed: `example.com`
/// becomes `https://example.com`; anything that is not a plain public web
/// address is refused.
pub fn clean_website(raw: &str) -> Result<Option<String>, &'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > 200 || trimmed.chars().any(char::is_whitespace) {
        return Err("website must be a web address of at most 200 characters");
    }
    let url = if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        trimmed.to_string()
    } else if trimmed.contains("://") {
        return Err("website must start with http:// or https://");
    } else {
        format!("https://{trimmed}")
    };
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let host_only = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(&host);
    if !host_only.contains('.')
        || host_only.ends_with(".local")
        || host_only.ends_with(".internal")
        || host_only.parse::<std::net::IpAddr>().is_ok()
        || host_only == "localhost"
    {
        return Err("website must be a public web address");
    }
    Ok(Some(url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_area_bot_has_a_three_task_plan() {
        let p = Profile {
            name: "Bright Smiles".into(),
            business_type: Some("Dental clinic".into()),
            website: Some("https://brightsmiles.example".into()),
            focus: Some("More customers".into()),
        };
        for (_, name, _, _) in crate::office::OFFICE_ROLES {
            let tasks = starter_tasks(name, &p).unwrap_or_else(|| panic!("{name} has a plan"));
            assert!(
                tasks[0].contains("Bright Smiles (Dental clinic)"),
                "{name}: {}",
                tasks[0]
            );
            assert!(
                tasks[0].contains("brightsmiles.example"),
                "{name}: {}",
                tasks[0]
            );
        }
        assert!(starter_tasks("Assistant", &p).is_none());
    }

    #[test]
    fn websites_are_normalised_and_private_addresses_refused() {
        assert_eq!(
            clean_website("example.com").unwrap().as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            clean_website(" https://shop.example.co/about ")
                .unwrap()
                .as_deref(),
            Some("https://shop.example.co/about")
        );
        assert_eq!(clean_website("").unwrap(), None);
        for bad in [
            "localhost",
            "http://127.0.0.1",
            "https://10.0.0.5/admin",
            "printer.local",
            "ftp://example.com",
            "two words.com",
            "intranet",
        ] {
            assert!(clean_website(bad).is_err(), "{bad}");
        }
    }
}
