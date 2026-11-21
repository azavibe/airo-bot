//! What an office's bots know about the business, and how they put work
//! into the office's own apps.
//!
//! Every bot answering in an office is told the business it works for and
//! the apps it has (the office's CRM, bookings, support desk, invoices and
//! studio live inside Airobot). A bot fills an app by writing lines of the
//! form `ADD <app-slug> {json}` in its answer; those lines are saved as
//! records and replaced by a short "Added ..." line, so the owner sees work
//! done in the apps rather than advice about other software.

use serde_json::{json, Map, Value};
use sqlx::PgPool;
use uuid::Uuid;

/// Records one answer may add.
const MAX_RECORDS_PER_ANSWER: usize = 10;
/// Fields kept per record, and characters per text value.
const MAX_FIELDS: usize = 20;
const MAX_VALUE_CHARS: usize = 500;

/// A record a bot wrote: which app (index into the office's apps) and its data.
pub type FoundRecord = (usize, Map<String, Value>);
/// An office row: id, name, business type, website, focus.
type OfficeRow = (Uuid, String, Option<String>, Option<String>, Option<String>);

/// An app a bot can fill: where records go and what they look like.
#[derive(Debug, Clone)]
pub struct AppInfo {
    pub id: Uuid,
    pub slug: String,
    pub title: String,
    pub record_type: String,
    pub fields: Vec<String>,
    /// Kanban stages, when the app has a board.
    pub stages: Vec<String>,
}

impl AppInfo {
    /// Reads where records go from an app's layout: the first view with a
    /// record type, its columns and card fields, and its board stages.
    pub fn from_schema(id: Uuid, slug: String, title: String, schema: &Value) -> Option<Self> {
        let views = schema["views"].as_array()?;
        let record_type = views
            .iter()
            .find_map(|v| v["record_type"].as_str())?
            .to_string();
        let mut fields: Vec<String> = Vec::new();
        let mut push = |f: &str| {
            if !f.is_empty() && !fields.iter().any(|x| x == f) {
                fields.push(f.to_string());
            }
        };
        let mut stages = Vec::new();
        for view in views
            .iter()
            .filter(|v| v["record_type"] == record_type.as_str())
        {
            for col in view["columns"].as_array().into_iter().flatten() {
                if let Some(key) = col["key"].as_str() {
                    push(key);
                }
                if view["type"] == "kanban" {
                    if let Some(stage) = col.as_str() {
                        if !stages.iter().any(|s: &String| s == stage) {
                            stages.push(stage.to_string());
                        }
                    }
                }
            }
            for key in ["group_by_field", "media_field", "caption_field"] {
                if let Some(f) = view[key].as_str() {
                    push(f);
                }
            }
            for list in ["card_fields", "metrics_fields"] {
                for f in view[list].as_array().into_iter().flatten() {
                    if let Some(f) = f.as_str() {
                        push(f);
                    }
                }
            }
        }
        Some(AppInfo {
            id,
            slug,
            title,
            record_type,
            fields,
            stages,
        })
    }

    /// The status a new record gets: its stage when that is a board stage
    /// (the board groups by status), else its own status, else "active".
    fn status_for(&self, data: &Map<String, Value>) -> String {
        let named = |key: &str| data.get(key).and_then(Value::as_str).map(str::trim);
        if let Some(stage) = named("stage").or_else(|| named("status")) {
            if let Some(s) = self.stages.iter().find(|s| s.eq_ignore_ascii_case(stage)) {
                return s.clone();
            }
            if self.stages.is_empty() && !stage.is_empty() && stage.chars().count() <= 64 {
                return stage.to_string();
            }
        }
        self.stages
            .first()
            .cloned()
            .unwrap_or_else(|| "active".to_string())
    }
}

/// The business an office runs, as its owner described it.
#[derive(Debug, Clone, Default)]
pub struct Business {
    pub workspace_id: Uuid,
    pub name: String,
    pub business_type: Option<String>,
    pub website: Option<String>,
    pub focus: Option<String>,
}

/// The office a room belongs to (rooms made before offices belong to the
/// owner's first office).
pub async fn business_of_thread(pool: &PgPool, thread_id: Uuid) -> Option<Business> {
    let row: Option<OfficeRow> = sqlx::query_as(
        "SELECT w.id, w.name, w.business_type, w.website, w.focus
             FROM threads t
             JOIN workspaces w ON w.id = COALESCE(t.workspace_id, (
                 SELECT w2.id FROM workspaces w2
                 WHERE w2.owner_user_id = t.user_id AND w2.deleted_at IS NULL
                 ORDER BY w2.created_at ASC LIMIT 1))
             WHERE t.id = $1 AND w.deleted_at IS NULL",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let (workspace_id, name, business_type, website, focus) = row?;
    Some(Business {
        workspace_id,
        name,
        business_type: business_type.filter(|b| !b.trim().is_empty() && b != "Small business"),
        website,
        focus,
    })
}

pub async fn apps_of(pool: &PgPool, workspace_id: Uuid) -> Vec<AppInfo> {
    let rows: Vec<(Uuid, String, String, Value)> = sqlx::query_as(
        "SELECT id, slug, title, ui_schema FROM office_apps
         WHERE workspace_id = $1 AND deleted_at IS NULL AND archived_at IS NULL
         ORDER BY created_at ASC LIMIT 20",
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .filter_map(|(id, slug, title, schema)| AppInfo::from_schema(id, slug, title, &schema))
        .collect()
}

/// What an office bot is told about its office, appended to its character.
pub fn prompt(business: &Business, apps: &[AppInfo]) -> String {
    let mut out = format!("\n\n## Your office\nYou work for {}", business.name);
    if let Some(kind) = &business.business_type {
        out.push_str(&format!(", a {kind}"));
    }
    out.push('.');
    if let Some(site) = &business.website {
        out.push_str(&format!(" Website: {site}."));
    }
    if let Some(focus) = &business.focus {
        out.push_str(&format!(" What matters most this month: {focus}."));
    }
    out.push_str(
        " If the name says what the business is (a hotel, a clinic, a bakery), work from that.\n\
         Airobot is this business's own back office: its CRM, bookings, support desk, invoices and social studio are the apps below, inside Airobot. \
         Never recommend, compare or price other software (HubSpot, Calendly, Zendesk, QuickBooks and the like), and never treat software or CRM as what this business sells.\n",
    );
    if !apps.is_empty() {
        out.push_str("\n## Apps you can fill\n");
        for app in apps {
            out.push_str(&format!(
                "- {} \"{}\": a {} has {}",
                app.slug,
                app.title,
                app.record_type,
                app.fields.join(", ")
            ));
            if !app.stages.is_empty() {
                out.push_str(&format!("; stage is one of {}", app.stages.join(", ")));
            }
            out.push('\n');
        }
        out.push_str(&format!(
            "To add records, write one line per record, exactly: ADD <app> {{json object with those fields}}. At most {MAX_RECORDS_PER_ANSWER} per message. \
             Use realistic values that fit this business. The lines are saved into the app and hidden from the chat, so also say in one short sentence what you added.\n",
        ));
    }
    out.push_str(
        "\n## How to write\n\
         - Do the work, don't describe it. Write for a busy owner: a few short lines or a short list, no reports.\n\
         - Ask the owner at most one question per message, and only if you cannot continue without the answer. Put it on its own last line starting with \"Question: \". Otherwise make a sensible assumption and say what you assumed.\n",
    );
    out
}

/// The office prompt for a bot answering in `thread_id`, if the room is in
/// an office.
pub async fn prompt_for_thread(pool: &PgPool, thread_id: Uuid) -> Option<String> {
    let business = business_of_thread(pool, thread_id).await?;
    let apps = apps_of(pool, business.workspace_id).await;
    Some(prompt(&business, &apps))
}

/// Records found in an answer: (index into `apps`, data).
pub fn parse_records(text: &str, apps: &[AppInfo]) -> (Vec<FoundRecord>, String) {
    let mut found = Vec::new();
    let mut kept: Vec<&str> = Vec::new();
    for line in text.lines() {
        let trimmed = line
            .trim()
            .trim_start_matches(['-', '*', '`', ' '])
            .trim_end_matches('`');
        let parsed = trimmed
            .strip_prefix("ADD ")
            .or_else(|| trimmed.strip_prefix("Add "))
            .or_else(|| trimmed.strip_prefix("add "))
            .and_then(|rest| {
                let rest = rest.trim();
                let (slug, json_part) = rest.split_once(char::is_whitespace)?;
                let index = apps
                    .iter()
                    .position(|a| a.slug.eq_ignore_ascii_case(slug))?;
                let object = serde_json::from_str::<Value>(json_part.trim()).ok()?;
                Some((index, object.as_object()?.clone()))
            });
        match parsed {
            Some((index, data)) => {
                if found.len() < MAX_RECORDS_PER_ANSWER {
                    found.push((index, clean(data)));
                }
            }
            None => kept.push(line),
        }
    }
    (found, kept.join("\n").trim().to_string())
}

/// Keeps a record small: a limited number of fields, short text, plain
/// values only.
fn clean(data: Map<String, Value>) -> Map<String, Value> {
    data.into_iter()
        .filter(|(k, _)| !k.is_empty() && k.len() <= 64)
        .filter_map(|(k, v)| {
            let v = match v {
                Value::String(s) => Value::String(s.chars().take(MAX_VALUE_CHARS).collect()),
                Value::Number(_) | Value::Bool(_) => v,
                _ => return None,
            };
            Some((k, v))
        })
        .take(MAX_FIELDS)
        .collect()
}

/// Saves the records a bot wrote into its office's apps and returns what
/// the chat shows instead, plus a button to the apps when anything was
/// added. Without records the answer is unchanged.
pub async fn apply_records(
    pool: &PgPool,
    thread_id: Uuid,
    agent_id: Uuid,
    text: &str,
) -> (String, Option<Value>) {
    if !text.lines().any(|l| {
        l.trim()
            .trim_start_matches(['-', '*', '`', ' '])
            .to_ascii_lowercase()
            .starts_with("add ")
    }) {
        return (text.to_string(), None);
    }
    let Some(business) = business_of_thread(pool, thread_id).await else {
        return (text.to_string(), None);
    };
    let apps = apps_of(pool, business.workspace_id).await;
    let (records, rest) = parse_records(text, &apps);
    if records.is_empty() {
        return (text.to_string(), None);
    }
    let mut added: Vec<(String, usize)> = Vec::new();
    for (index, data) in records {
        let app = &apps[index];
        let status = app.status_for(&data);
        let mut data = Value::Object(data);
        if !app.stages.is_empty() && app.fields.iter().any(|f| f == "stage") {
            data["stage"] = json!(status);
        }
        let saved = sqlx::query(
            "INSERT INTO office_records (workspace_id, app_id, record_type, status, data, created_by_agent_id)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(business.workspace_id)
        .bind(app.id)
        .bind(&app.record_type)
        .bind(&status)
        .bind(&data)
        .bind(agent_id)
        .execute(pool)
        .await;
        match saved {
            Ok(_) => match added.iter_mut().find(|(t, _)| *t == app.title) {
                Some(entry) => entry.1 += 1,
                None => added.push((app.title.clone(), 1)),
            },
            Err(error) => tracing::warn!(?error, app = %app.slug, "bot record not saved"),
        }
    }
    if added.is_empty() {
        return (rest, None);
    }
    let summary = added
        .iter()
        .map(|(title, n)| format!("{n} to {title}"))
        .collect::<Vec<_>>()
        .join(", ");
    let body = if rest.is_empty() {
        format!("Added {summary}.")
    } else {
        format!("{rest}\n\n*Added {summary}.*")
    };
    (
        body,
        Some(json!([{ "label": "Open apps", "kind": "open", "target": "apps" }])),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crm() -> AppInfo {
        AppInfo::from_schema(
            Uuid::new_v4(),
            "leads-pipeline".into(),
            "Lead Pipeline".into(),
            &json!({
                "views": [
                    { "type": "kanban", "record_type": "lead", "group_by_field": "stage",
                      "columns": ["New Lead", "Qualified", "Won"], "card_fields": ["name", "estimated_value"] },
                    { "type": "datatable", "record_type": "lead",
                      "columns": [{ "key": "name" }, { "key": "phone" }, { "key": "stage" }] }
                ]
            }),
        )
        .expect("app")
    }

    #[test]
    fn layouts_say_where_records_go() {
        let app = crm();
        assert_eq!(app.record_type, "lead");
        assert_eq!(app.stages, vec!["New Lead", "Qualified", "Won"]);
        for f in ["name", "phone", "stage", "estimated_value"] {
            assert!(app.fields.iter().any(|x| x == f), "{f}: {:?}", app.fields);
        }
    }

    #[test]
    fn add_lines_become_records_and_leave_the_chat() {
        let apps = vec![crm()];
        let text = "Here are three leads.\nADD leads-pipeline {\"name\": \"Tour group\", \"stage\": \"qualified\", \"estimated_value\": 900}\n- `ADD leads-pipeline {\"name\": \"Wedding party\"}`\nADD unknown-app {\"name\": \"x\"}\nADD leads-pipeline not json";
        let (records, rest) = parse_records(text, &apps);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].1["name"], "Tour group");
        assert_eq!(apps[0].status_for(&records[0].1), "Qualified");
        assert_eq!(apps[0].status_for(&records[1].1), "New Lead");
        assert!(rest.starts_with("Here are three leads."));
        assert!(rest.contains("unknown-app"), "unknown apps stay as text");
        assert!(rest.contains("not json"));
        assert!(!rest.contains("Tour group"));
    }

    #[test]
    fn records_are_kept_small() {
        let mut big = Map::new();
        big.insert("name".into(), json!("x".repeat(2000)));
        big.insert("nested".into(), json!({ "a": 1 }));
        let cleaned = clean(big);
        assert_eq!(
            cleaned["name"].as_str().map(str::len),
            Some(MAX_VALUE_CHARS)
        );
        assert!(!cleaned.contains_key("nested"));
    }

    #[test]
    fn the_prompt_names_the_business_and_the_rules() {
        let business = Business {
            workspace_id: Uuid::new_v4(),
            name: "Bamboo Hotel".into(),
            business_type: None,
            website: None,
            focus: None,
        };
        let text = prompt(&business, &[crm()]);
        assert!(text.contains("You work for Bamboo Hotel."));
        assert!(text.contains("ADD <app>"));
        assert!(text.contains("leads-pipeline"));
        assert!(text.contains("Question: "));
        assert!(text.contains("Never recommend"));
    }
}
