//! Live app records in chat.
//!
//! A message that adds or changes a record carries a *card reference*
//! (`notes.cards`: `{type: "app_record_card", app_slug, record_id, action,
//! title, changed_fields?, approval_id?}`), never a copy of the record: the
//! app draws the card from the record as it is now, read through
//! `GET /api/office/records?ids=` and kept fresh by the office's live events
//! (`GET /api/office/events`). A deleted record (or one in an archived app)
//! comes back as a tombstone, so an old card says so instead of showing
//! stale data.
//!
//! Bots change records with `UPDATE <app> <record id> {json}` lines. A
//! Trusted bot's change is applied at once; an Ask first bot's change waits
//! as an approval tied to the record (`approvals.record_id`, with the
//! proposed `changes`), and its card offers Approve, Edit and Reject.

use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::auth::Caller;
use crate::error::AppError;
use crate::office::context::AppInfo;
use crate::state::AppState;

/// Records one request may ask for.
const MAX_IDS: usize = 50;
/// `UPDATE` lines read from one message.
const MAX_UPDATES: usize = 5;

/// A record that changed, for the office's live listeners.
#[derive(Clone, Debug, Serialize)]
pub struct RecordEvent {
    pub workspace_id: Uuid,
    pub record_id: Uuid,
    pub app_slug: String,
    /// `created`, `updated`, `deleted`, `pending` (a change waits for an
    /// OK) or `app` (its app was archived, restored or deleted).
    pub kind: &'static str,
}

/// Tells the office's live listeners a record changed. Nobody listening is
/// fine.
pub fn emit(
    state: &AppState,
    workspace_id: Uuid,
    record_id: Uuid,
    app_slug: &str,
    kind: &'static str,
) {
    let _ = state.record_sender.send(RecordEvent {
        workspace_id,
        record_id,
        app_slug: app_slug.to_string(),
        kind,
    });
}

/// [`emit`] for a record known only by id.
pub async fn emit_for(state: &AppState, record_id: Uuid, kind: &'static str) {
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT r.workspace_id, a.slug FROM office_records r
         JOIN office_apps a ON a.id = r.app_id WHERE r.id = $1",
    )
    .bind(record_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    if let Some((workspace_id, slug)) = row {
        emit(state, workspace_id, record_id, &slug, kind);
    }
}

/// A card reference for a message.
pub fn card(
    app_slug: &str,
    record_id: Uuid,
    action: &str,
    title: &str,
    changed_fields: &[String],
    approval_id: Option<Uuid>,
) -> Value {
    let mut c = json!({
        "type": "app_record_card",
        "app_slug": app_slug,
        "record_id": record_id,
        "action": action,
        "title": title,
    });
    if !changed_fields.is_empty() {
        c["changed_fields"] = json!(changed_fields);
    }
    if let Some(id) = approval_id {
        c["approval_id"] = json!(id);
    }
    c
}

/// A record's display title: its first name-like field.
pub fn title_of(data: &Value) -> String {
    for key in [
        "name",
        "title",
        "client_name",
        "client",
        "customer",
        "subject",
        "invoice_number",
        "script_hook",
    ] {
        if let Some(t) = data.get(key).and_then(|v| v.as_str()).map(str::trim) {
            if !t.is_empty() {
                return t.chars().take(80).collect();
            }
        }
    }
    "A record".into()
}

// ---------------------------------------------------------------- reading

#[derive(Debug, Deserialize)]
pub struct RecordsQuery {
    pub office: Uuid,
    /// Comma-separated record ids.
    pub ids: String,
}

/// A change waiting for an OK: approval, record, changes, by whom, when.
type PendingRow = (
    Uuid,
    Uuid,
    Option<Value>,
    String,
    chrono::DateTime<chrono::Utc>,
);

type RecordRow = (
    Uuid,
    String,
    String,
    bool,
    String,
    String,
    Value,
    Option<Uuid>,
    Option<String>,
    Option<Uuid>,
    chrono::DateTime<chrono::Utc>,
    bool,
);

/// GET /api/office/records?office=&ids= -- records as they are now, for
/// cards: each with its app, contact, and any change waiting for an OK.
/// An id that is deleted, unknown or another office's comes back as a
/// tombstone (`gone: "deleted"`); one in an archived app as `gone:
/// "archived"`.
pub async fn get_records(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<RecordsQuery>,
) -> Result<Json<Value>, AppError> {
    let Caller::User(user) = &caller else {
        return Err(AppError::Forbidden);
    };
    crate::handlers::offices::owned_office(&state.pool, user.id, q.office).await?;
    let ids: Vec<Uuid> = q
        .ids
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().parse::<Uuid>())
        .collect::<Result<_, _>>()
        .map_err(|_| AppError::BadRequest("ids are record ids, separated by commas".into()))?;
    if ids.len() > MAX_IDS {
        return Err(AppError::BadRequest(format!("at most {MAX_IDS} ids")));
    }
    let rows: Vec<RecordRow> = sqlx::query_as(
        "SELECT r.id, a.slug, a.title, (a.archived_at IS NOT NULL OR a.deleted_at IS NOT NULL),
                r.record_type, r.status, r.data, r.contact_id, c.name, r.created_by_agent_id,
                r.updated_at, r.deleted_at IS NOT NULL
         FROM office_records r
         JOIN office_apps a ON a.id = r.app_id
         LEFT JOIN contacts c ON c.id = r.contact_id AND c.deleted_at IS NULL
         WHERE r.id = ANY($1) AND r.workspace_id = $2",
    )
    .bind(&ids)
    .bind(q.office)
    .fetch_all(&state.pool)
    .await?;
    let pending: Vec<PendingRow> = sqlx::query_as(
        "SELECT id, record_id, changes, requested_by, created_at FROM approvals
             WHERE record_id = ANY($1) AND workspace_id = $2
               AND status = 'pending' AND deleted_at IS NULL
             ORDER BY created_at",
    )
    .bind(&ids)
    .bind(q.office)
    .fetch_all(&state.pool)
    .await?;
    let records: Vec<Value> = ids
        .iter()
        .map(|id| match rows.iter().find(|r| r.0 == *id) {
            Some((
                id,
                slug,
                app_title,
                app_gone,
                record_type,
                status,
                data,
                contact_id,
                contact_name,
                by_agent,
                updated_at,
                deleted,
            )) => {
                let gone = if *deleted {
                    Some("deleted")
                } else if *app_gone {
                    Some("archived")
                } else {
                    None
                };
                json!({
                    "id": id,
                    "app_slug": slug,
                    "app_title": app_title,
                    "record_type": record_type,
                    "status": status,
                    "data": data,
                    "contact": contact_id.map(|cid| json!({ "id": cid, "name": contact_name })),
                    "created_by_agent_id": by_agent,
                    "updated_at": updated_at,
                    "gone": gone,
                    "pending": pending.iter().filter(|p| p.1 == *id).map(|p| json!({
                        "approval_id": p.0,
                        "changes": p.2,
                        "requested_by": p.3,
                        "created_at": p.4,
                    })).collect::<Vec<_>>(),
                })
            }
            None => json!({ "id": id, "gone": "deleted" }),
        })
        .collect();
    Ok(Json(json!({ "records": records })))
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub office: Uuid,
}

/// GET /api/office/events?office= -- the office's record changes, live
/// (server-sent events named `record`).
pub async fn office_events(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<EventsQuery>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>>, AppError>
{
    let Caller::User(user) = &caller else {
        return Err(AppError::Forbidden);
    };
    crate::handlers::offices::owned_office(&state.pool, user.id, q.office).await?;
    let office = q.office;
    let rx = state.record_sender.subscribe();
    let stream =
        tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(move |item| match item {
            Ok(event) if event.workspace_id == office => {
                Some(Ok(Event::default().event("record").data(
                    json!({
                        "record_id": event.record_id,
                        "app_slug": event.app_slug,
                        "kind": event.kind,
                    })
                    .to_string(),
                )))
            }
            _ => None,
        });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

// ---------------------------------------------------------------- bot updates

/// An `UPDATE <app> <id> {json}` line: the app's index, the id (or its
/// first 8 characters) and the fields to change.
type FoundUpdate = (usize, String, Map<String, Value>);

/// Reads `UPDATE` lines for the given apps; returns them and the text
/// without them.
pub fn parse_updates(text: &str, apps: &[AppInfo]) -> (Vec<FoundUpdate>, String) {
    let mut found = Vec::new();
    let mut kept = Vec::new();
    for line in text.lines() {
        let trimmed = line
            .trim()
            .trim_start_matches(['-', '*', '`', ' '])
            .trim_end_matches('`');
        let parsed = trimmed.strip_prefix("UPDATE ").and_then(|rest| {
            let (slug, rest) = rest.trim().split_once(char::is_whitespace)?;
            let (id, json_part) = rest.trim().split_once(char::is_whitespace)?;
            let id = id.trim_matches(['[', ']']).to_ascii_lowercase();
            let id_ok = (id.len() == 8 && id.chars().all(|c| c.is_ascii_hexdigit()))
                || id.parse::<Uuid>().is_ok();
            if !id_ok {
                return None;
            }
            let index = apps.iter().position(|a| a.slug == slug)?;
            let Value::Object(map) = serde_json::from_str::<Value>(json_part.trim()).ok()? else {
                return None;
            };
            (!map.is_empty()).then_some((index, id, map))
        });
        match parsed {
            Some(u) if found.len() < MAX_UPDATES => found.push(u),
            _ => kept.push(line),
        }
    }
    (found, kept.join("\n").trim().to_string())
}

/// What a value looks like in a short "a → b" line.
fn short(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "empty".into(),
        Some(Value::String(s)) => s.chars().take(40).collect(),
        Some(other) => other.to_string().chars().take(40).collect(),
    }
}

/// Applies (Trusted bot) or proposes (Ask first) a bot's `UPDATE` lines.
/// Returns the reply without them (plus a line saying what happened) and
/// the cards for the message.
pub async fn apply_updates(
    state: &AppState,
    thread_id: Uuid,
    agent_id: Uuid,
    text: &str,
) -> (String, Vec<Value>) {
    if !text.contains("UPDATE ") {
        return (text.to_string(), Vec::new());
    }
    let pool = &state.pool;
    let Some(business) = crate::office::context::business_of_thread(pool, thread_id).await else {
        return (text.to_string(), Vec::new());
    };
    let access = crate::office::access::effective(pool, business.workspace_id, agent_id).await;
    let apps: Vec<AppInfo> = crate::office::context::apps_of(pool, business.workspace_id)
        .await
        .into_iter()
        .filter(|a| access.may_use_app(&a.slug))
        .collect();
    let (updates, rest) = parse_updates(text, &apps);
    if updates.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let (agent_name, mode): (String, String) =
        sqlx::query_as("SELECT name, permission_mode FROM agents WHERE id = $1")
            .bind(agent_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| ("A bot".into(), "ask_first".into()));
    let trusted = mode == "trusted";
    let mut cards = Vec::new();
    let mut lines = Vec::new();
    for (index, id, changes) in updates {
        let app = &apps[index];
        let found: Vec<(Uuid, Value, String)> = sqlx::query_as(
            "SELECT id, data, status FROM office_records
             WHERE app_id = $1 AND deleted_at IS NULL AND left(id::text, $3) = $2
             LIMIT 2",
        )
        .bind(app.id)
        .bind(&id)
        .bind(id.len() as i32)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        let [(record_id, data, current_status)] = found.as_slice() else {
            lines.push(format!(
                "*Couldn't find record {id} in {} to update.*",
                app.title
            ));
            continue;
        };
        let changed: Vec<String> = changes
            .iter()
            .filter(|(k, v)| data.get(k.as_str()) != Some(v))
            .map(|(k, _)| k.clone())
            .collect();
        if changed.is_empty() {
            continue;
        }
        let changes: Map<String, Value> = changes
            .into_iter()
            .filter(|(k, _)| changed.contains(k))
            .collect();
        let title = title_of(data);
        let summary = changed
            .iter()
            .map(|k| format!("{k}: {} → {}", short(data.get(k)), short(changes.get(k))))
            .collect::<Vec<_>>()
            .join(", ");
        // Moving into a stage that needs the owner's OK (a quote being
        // sent) waits for it, whatever the bot's mode.
        let mut merged = data.as_object().cloned().unwrap_or_default();
        for (k, v) in &changes {
            merged.insert(k.clone(), v.clone());
        }
        let next_status = app.status_for(&merged);
        let gated = next_status != *current_status && app.approval_stages.contains(&next_status);
        if trusted && !gated {
            if apply_change(pool, app, *record_id, &changes).await.is_ok() {
                emit(
                    state,
                    business.workspace_id,
                    *record_id,
                    &app.slug,
                    "updated",
                );
                cards.push(card(
                    &app.slug, *record_id, "updated", &title, &changed, None,
                ));
                lines.push(format!("*Updated {title} in {}: {summary}.*", app.title));
            }
        } else {
            match propose_change(
                pool,
                business.workspace_id,
                thread_id,
                agent_id,
                &agent_name,
                app,
                *record_id,
                &title,
                &summary,
                &changes,
            )
            .await
            {
                Ok(approval_id) => {
                    emit(
                        state,
                        business.workspace_id,
                        *record_id,
                        &app.slug,
                        "pending",
                    );
                    cards.push(card(
                        &app.slug,
                        *record_id,
                        "proposed_update",
                        &title,
                        &changed,
                        Some(approval_id),
                    ));
                    lines.push(format!(
                        "*Asked to update {title} in {}: {summary}. It waits for your OK.*",
                        app.title
                    ));
                }
                Err(error) => tracing::warn!(?error, "could not propose a record change"),
            }
        }
    }
    let note = lines.join("\n");
    let body = match (rest.is_empty(), note.is_empty()) {
        (true, _) => note,
        (false, true) => rest,
        (false, false) => format!("{rest}\n\n{note}"),
    };
    (body, cards)
}

/// Merges `changes` into a record; a stage change moves its status too.
pub async fn apply_change(
    pool: &PgPool,
    app: &AppInfo,
    record_id: Uuid,
    changes: &Map<String, Value>,
) -> Result<(), sqlx::Error> {
    let data: Value = sqlx::query_scalar("SELECT data FROM office_records WHERE id = $1")
        .bind(record_id)
        .fetch_one(pool)
        .await?;
    let mut merged = data.as_object().cloned().unwrap_or_default();
    for (k, v) in changes {
        merged.insert(k.clone(), v.clone());
    }
    let status = app.status_for(&merged);
    let mut merged = Value::Object(merged);
    if !app.stages.is_empty() && app.fields.iter().any(|f| f == "stage") {
        merged["stage"] = json!(status);
    }
    sqlx::query(
        "UPDATE office_records SET data = $2, status = $3, updated_at = now()
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(record_id)
    .bind(&merged)
    .bind(&status)
    .execute(pool)
    .await?;
    Ok(())
}

/// Keeps a bot's change as a job waiting for approval, tied to the record.
#[allow(clippy::too_many_arguments)]
async fn propose_change(
    pool: &PgPool,
    office: Uuid,
    thread_id: Uuid,
    agent_id: Uuid,
    agent_name: &str,
    app: &AppInfo,
    record_id: Uuid,
    title: &str,
    summary: &str,
    changes: &Map<String, Value>,
) -> Result<Uuid, sqlx::Error> {
    let what: String = format!("Update {title} in {}: {summary}", app.title)
        .chars()
        .take(200)
        .collect();
    let mut tx = pool.begin().await?;
    let task_id: Uuid = sqlx::query_scalar(
        "INSERT INTO tasks (workspace_id, thread_id, title, status, payload, created_by_actor_type,
                            created_by_actor_id, assigned_agent_id, executor, fsm_state)
         VALUES ($1, $2, $3, 'waiting_approval', $4, 'agent', $5, $5, 'chat', 'WAITING_APPROVAL')
         RETURNING id",
    )
    .bind(office)
    .bind(thread_id)
    .bind(&what)
    .bind(json!({ "kind": "record_change", "record_id": record_id, "app_slug": app.slug, "changes": changes }))
    .bind(agent_id)
    .fetch_one(&mut *tx)
    .await?;
    let approval_id: Uuid = sqlx::query_scalar(
        "INSERT INTO approvals (workspace_id, task_id, requested_action, risk_level, requested_by,
                               record_id, changes)
         VALUES ($1, $2, $3, 'low', $4, $5, $6)
         RETURNING id",
    )
    .bind(office)
    .bind(task_id)
    .bind(&what)
    .bind(agent_name)
    .bind(record_id)
    .bind(Value::Object(changes.clone()))
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(approval_id)
}

/// After the owner decided an approval tied to a record: an approved
/// change is applied (with the owner's edits, when given), and its job
/// finishes either way.
pub async fn finish_record_change(
    state: &AppState,
    approval_id: Uuid,
    approved: bool,
    edited: Option<&Map<String, Value>>,
) -> Result<(), AppError> {
    let pool = &state.pool;
    let row: Option<(Uuid, Uuid, Uuid, Option<Value>)> = sqlx::query_as(
        "SELECT ap.task_id, ap.record_id, ap.workspace_id, ap.changes FROM approvals ap
         WHERE ap.id = $1 AND ap.record_id IS NOT NULL",
    )
    .bind(approval_id)
    .fetch_optional(pool)
    .await?;
    let Some((task_id, record_id, office, changes)) = row else {
        return Ok(());
    };
    if approved {
        let changes = match edited {
            Some(e) => e.clone(),
            None => changes
                .and_then(|c| c.as_object().cloned())
                .unwrap_or_default(),
        };
        let app: Option<(Uuid, String, String, Value)> = sqlx::query_as(
            "SELECT a.id, a.slug, a.title, a.ui_schema FROM office_apps a
             JOIN office_records r ON r.app_id = a.id
             WHERE r.id = $1 AND a.workspace_id = $2",
        )
        .bind(record_id)
        .bind(office)
        .fetch_optional(pool)
        .await?;
        let app = app
            .and_then(|(id, slug, title, schema)| AppInfo::from_schema(id, slug, title, &schema));
        if let Some(app) = app {
            apply_change(pool, &app, record_id, &changes).await?;
        }
        sqlx::query(
            "UPDATE tasks SET status = 'completed', fsm_state = 'COMPLETED', outcome = 'done',
                    outcome_note = $2, updated_at = now()
             WHERE id = $1",
        )
        .bind(task_id)
        .bind(if edited.is_some() {
            Some("approved with your edits")
        } else {
            None
        })
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE tasks SET status = 'failed', fsm_state = 'FAILED',
                    outcome_note = 'you rejected it', updated_at = now()
             WHERE id = $1",
        )
        .bind(task_id)
        .execute(pool)
        .await?;
    }
    emit_for(state, record_id, "updated").await;
    Ok(())
}

// ---------------------------------------------------------------- convert

#[derive(Debug, Deserialize)]
pub struct OfficeParam {
    pub office: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct ConvertRequest {
    /// The app to make the new record in (one the source app's `convert`
    /// rules name).
    pub to_app: String,
    pub idempotency_key: Option<String>,
}

/// A `set` value with its placeholders filled: `{ref}` (a short reference
/// from the new record's id), `{today}` and `{today+N}` (dates).
fn fill(value: &Value, reference: &str) -> Value {
    let Some(text) = value.as_str() else {
        return value.clone();
    };
    let today = chrono::Utc::now().date_naive();
    let mut out = text
        .replace("{ref}", reference)
        .replace("{today}", &today.to_string());
    while let Some(start) = out.find("{today+") {
        let Some(end) = out[start..].find('}') else {
            break;
        };
        let days: i64 = out[start + 7..start + end].parse().unwrap_or(0);
        let date = today + chrono::Duration::days(days.clamp(0, 3650));
        out.replace_range(start..start + end + 1, &date.to_string());
    }
    Value::String(out)
}

/// POST /api/office/records/:id/convert?office= -- makes a record in
/// another app from this one, as the source app's `convert` rule says (a
/// quote into an invoice or a project): its fields mapped by name, its
/// fixed values set, its contact kept. Converting the same record to the
/// same app again returns the record made the first time.
pub async fn convert_record(
    State(state): State<AppState>,
    caller: Caller,
    headers: axum::http::HeaderMap,
    Query(q): Query<OfficeParam>,
    axum::extract::Path(record_id): axum::extract::Path<Uuid>,
    Json(body): Json<ConvertRequest>,
) -> Result<Json<Value>, AppError> {
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let Caller::User(user) = &caller else {
        return Err(AppError::Forbidden);
    };
    crate::handlers::offices::owned_office(&state.pool, user.id, q.office).await?;
    let pool = &state.pool;
    type Source = (String, Value, Option<Uuid>, String, String, Value);
    let source: Option<Source> = sqlx::query_as(
        "SELECT r.status, r.data, r.contact_id, a.slug, a.title, a.ui_schema
         FROM office_records r JOIN office_apps a ON a.id = r.app_id
         WHERE r.id = $1 AND r.workspace_id = $2 AND r.deleted_at IS NULL
           AND a.deleted_at IS NULL AND a.archived_at IS NULL",
    )
    .bind(record_id)
    .bind(q.office)
    .fetch_optional(pool)
    .await?;
    let Some((status, data, contact_id, source_slug, source_title, schema)) = source else {
        return Err(AppError::NotFound);
    };
    let rule = schema["convert"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|r| r["to_app"].as_str() == Some(body.to_app.as_str()))
        .cloned()
        .ok_or_else(|| AppError::BadRequest("this record can't be converted to that app".into()))?;
    if let Some(stage) = rule["when_stage"].as_str() {
        if status != stage {
            return Err(AppError::BadRequest(format!(
                "only a record in {stage} can be converted; move it there first"
            )));
        }
    }
    // Converted before: the same record again.
    if let Some(done) = data["converted_to"][body.to_app.as_str()]
        .as_str()
        .and_then(|s| s.parse::<Uuid>().ok())
    {
        let existing: Option<(String, Value)> = sqlx::query_as(
            "SELECT status, data FROM office_records WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(done)
        .fetch_optional(pool)
        .await?;
        if let Some((target_status, target_data)) = existing {
            return Ok(Json(json!({
                "record": { "id": done, "app_slug": body.to_app, "status": target_status, "data": target_data },
                "source_id": record_id,
                "already": true,
            })));
        }
    }
    let target: Option<(Uuid, String, String, Value)> = sqlx::query_as(
        "SELECT id, slug, title, ui_schema FROM office_apps
         WHERE workspace_id = $1 AND slug = $2 AND deleted_at IS NULL AND archived_at IS NULL",
    )
    .bind(q.office)
    .bind(&body.to_app)
    .fetch_optional(pool)
    .await?;
    let Some((target_id, target_slug, target_title, target_schema)) = target else {
        return Err(AppError::BadRequest(format!(
            "add the {} app to this office first",
            body.to_app
        )));
    };
    let app = AppInfo::from_schema(
        target_id,
        target_slug.clone(),
        target_title.clone(),
        &target_schema,
    )
    .ok_or_else(|| AppError::BadRequest("that app has no record type".into()))?;

    let new_id = Uuid::new_v4();
    let reference = new_id.simple().to_string()[..6].to_ascii_uppercase();
    let mut fields = Map::new();
    for (to_key, from_key) in rule["map"].as_object().into_iter().flatten() {
        if let Some(v) = from_key.as_str().and_then(|k| data.get(k)) {
            if !v.is_null() {
                fields.insert(to_key.clone(), v.clone());
            }
        }
    }
    for (key, value) in rule["set"].as_object().into_iter().flatten() {
        fields.insert(key.clone(), fill(value, &reference));
    }
    fields.insert(
        "converted_from".into(),
        json!(format!("{source_title}: {}", title_of(&data))),
    );
    let target_status = app.status_for(&fields);
    let mut fields = Value::Object(fields);
    if !app.stages.is_empty() && app.fields.iter().any(|f| f == "stage") {
        fields["stage"] = json!(target_status);
    }

    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO office_records (id, workspace_id, app_id, record_type, status, data, contact_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(new_id)
    .bind(q.office)
    .bind(target_id)
    .bind(&app.record_type)
    .bind(&target_status)
    .bind(&fields)
    .bind(contact_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE office_records
         SET data = jsonb_set(data, '{converted_to}',
                COALESCE(data->'converted_to', '{}'::jsonb) || jsonb_build_object($2::text, $3::text)),
             updated_at = now()
         WHERE id = $1",
    )
    .bind(record_id)
    .bind(&target_slug)
    .bind(new_id.to_string())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    emit(&state, q.office, new_id, &target_slug, "created");
    emit(&state, q.office, record_id, &source_slug, "updated");
    Ok(Json(json!({
        "record": { "id": new_id, "app_slug": target_slug, "app_title": target_title, "status": target_status, "data": fields },
        "source_id": record_id,
        "already": false,
    })))
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
                      "columns": ["New Lead", "Qualified", "Won"], "card_fields": ["name", "estimated_value"] }
                ]
            }),
        )
        .expect("app")
    }

    #[test]
    fn update_lines_are_read() {
        let apps = vec![crm()];
        let text = "Moved them.\nUPDATE leads-pipeline 1a2b3c4d {\"stage\": \"Won\"}\n- `UPDATE leads-pipeline [9f8e7d6c] {\"estimated_value\": 900}`\nUPDATE other-app 1a2b3c4d {\"x\": 1}\nUPDATE leads-pipeline nothex!! {\"x\": 1}\nUPDATE leads-pipeline 1a2b3c4d {}";
        let (found, rest) = parse_updates(text, &apps);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].1, "1a2b3c4d");
        assert_eq!(found[0].2["stage"], "Won");
        assert_eq!(found[1].1, "9f8e7d6c");
        assert!(rest.starts_with("Moved them."));
        assert!(rest.contains("other-app"), "unknown apps stay as text");
        assert!(!rest.contains("9f8e7d6c"));
    }

    #[test]
    fn cards_and_titles() {
        let id = Uuid::new_v4();
        let c = card(
            "leads-pipeline",
            id,
            "updated",
            "Rosa",
            &["stage".into()],
            None,
        );
        assert_eq!(c["type"], "app_record_card");
        assert_eq!(c["changed_fields"][0], "stage");
        assert!(c.get("approval_id").is_none());
        assert_eq!(title_of(&json!({ "client_name": "Acme" })), "Acme");
        assert_eq!(title_of(&json!({})), "A record");
    }
}
