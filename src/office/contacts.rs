//! Contacts: one record per person or customer an office deals with.
//!
//! A contact is unique by email within its office (then matched by phone,
//! then by exact name), so a lead, an invoice, a booking and a ticket for
//! the same customer all point at one contact (`office_records.contact_id`)
//! and its timeline shows them together. Bots never overwrite a contact:
//! when a record they add names a person, it is linked to the matching
//! contact or a new one is made. Facts about one customer are kept on that
//! contact (`memories.contact_id`), and a bot is shown a contact's card and
//! facts only when the conversation names that contact, so one client's
//! details don't reach another client's chat.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::Caller;
use crate::error::AppError;
use crate::state::AppState;

/// Contacts one office keeps; more are refused with a clear message.
const MAX_CONTACTS: i64 = 20_000;
/// Contacts a bot is shown because the conversation names them.
const CONTEXT_CONTACTS: i64 = 5;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Contact {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub company: Option<String>,
    pub tags: Vec<String>,
    pub notes: Option<String>,
    pub source: String,
    pub marketing_consent: String,
    pub consent_updated_at: Option<DateTime<Utc>>,
    pub last_contacted_at: Option<DateTime<Utc>>,
    pub created_by_agent_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const CONTACT_COLUMNS: &str = "id, workspace_id, name, email, phone, company, tags, notes, source, \
     marketing_consent, consent_updated_at, last_contacted_at, created_by_agent_id, created_at, updated_at";

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A plausible email, lower-cased, or `None`.
pub fn normalize_email(raw: &str) -> Option<String> {
    let email = raw.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let ok = !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && email.len() <= 254
        && !email.contains(char::is_whitespace);
    ok.then_some(email)
}

/// Digits (and a leading +) of a phone number with at least 6 digits.
pub fn normalize_phone(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let mut out = String::new();
    if trimmed.starts_with('+') {
        out.push('+');
    }
    out.extend(trimmed.chars().filter(char::is_ascii_digit));
    let digits = out.trim_start_matches('+').len();
    (6..=20).contains(&digits).then_some(out)
}

/// A person a record names: name, email, phone, company.
pub type Person = (String, Option<String>, Option<String>, Option<String>);

/// Record types whose plain `name` is a person (elsewhere it names a post,
/// a project or a product).
const PERSON_RECORDS: [&str; 4] = ["lead", "customer", "contact", "client"];

/// The person a record names, if any.
pub fn person_in(record_type: &str, data: &Map<String, Value>) -> Option<Person> {
    let text = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| data.get(*k).and_then(Value::as_str))
            .map(squash)
            .filter(|s| !s.is_empty())
    };
    let email = text(&["email", "client_email", "customer_email", "contact_email"])
        .and_then(|e| normalize_email(&e));
    let phone = text(&["phone", "client_phone", "customer_phone", "contact_phone"])
        .and_then(|p| normalize_phone(&p));
    let company = text(&["company", "business"]);
    let name = text(&[
        "contact",
        "client_name",
        "customer_name",
        "client",
        "customer",
    ])
    .or_else(|| {
        PERSON_RECORDS
            .contains(&record_type)
            .then(|| text(&["name"]))
            .flatten()
    })
    .or_else(|| email.clone())
    .or_else(|| phone.clone())?;
    let name: String = name.chars().take(200).collect();
    Some((name, email, phone, company))
}

/// The contact a person matches (email, then phone, then exact name), or a
/// new one. Never changes an existing contact except filling an empty email
/// or phone. Returns the contact's id.
#[allow(clippy::too_many_arguments)]
pub async fn find_or_create(
    pool: &PgPool,
    workspace_id: Uuid,
    name: &str,
    email: Option<&str>,
    phone: Option<&str>,
    company: Option<&str>,
    source: &str,
    agent_id: Option<Uuid>,
) -> Option<Uuid> {
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM contacts
         WHERE workspace_id = $1 AND deleted_at IS NULL
           AND (($2::text IS NOT NULL AND email = $2)
             OR ($3::text IS NOT NULL AND phone = $3)
             OR ($2::text IS NULL AND $3::text IS NULL AND lower(name) = lower($4)))
         ORDER BY (email = $2) DESC NULLS LAST, (phone = $3) DESC NULLS LAST, created_at ASC
         LIMIT 1",
    )
    .bind(workspace_id)
    .bind(email)
    .bind(phone)
    .bind(name)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    if let Some(id) = found {
        let _ = sqlx::query(
            "UPDATE contacts SET email = COALESCE(email, $2), phone = COALESCE(phone, $3),
                    updated_at = now()
             WHERE id = $1 AND (email IS NULL AND $2::text IS NOT NULL OR phone IS NULL AND $3::text IS NOT NULL)
               AND NOT EXISTS (SELECT 1 FROM contacts c2 WHERE c2.workspace_id = contacts.workspace_id
                               AND c2.deleted_at IS NULL AND c2.email = $2 AND c2.id <> $1)",
        )
        .bind(id)
        .bind(email)
        .bind(phone)
        .execute(pool)
        .await;
        return Some(id);
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM contacts WHERE workspace_id = $1 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .fetch_one(pool)
    .await
    .unwrap_or(MAX_CONTACTS);
    if count >= MAX_CONTACTS {
        return None;
    }
    sqlx::query_scalar(
        "INSERT INTO contacts (workspace_id, name, email, phone, company, source, created_by_agent_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (workspace_id, email) WHERE deleted_at IS NULL AND email IS NOT NULL
         DO UPDATE SET updated_at = now()
         RETURNING id",
    )
    .bind(workspace_id)
    .bind(name)
    .bind(email)
    .bind(phone)
    .bind(company)
    .bind(source)
    .bind(agent_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

/// Links a saved record to the contact it names (made if new).
pub async fn link_record(
    pool: &PgPool,
    workspace_id: Uuid,
    record_id: Uuid,
    record_type: &str,
    data: &Value,
    source: &str,
    agent_id: Option<Uuid>,
) {
    let Some(map) = data.as_object() else {
        return;
    };
    let Some((name, email, phone, company)) = person_in(record_type, map) else {
        return;
    };
    let Some(contact) = find_or_create(
        pool,
        workspace_id,
        &name,
        email.as_deref(),
        phone.as_deref(),
        company.as_deref(),
        source,
        agent_id,
    )
    .await
    else {
        return;
    };
    let _ = sqlx::query(
        "UPDATE office_records SET contact_id = $2 WHERE id = $1 AND contact_id IS NULL",
    )
    .bind(record_id)
    .bind(contact)
    .execute(pool)
    .await;
}

/// The contacts a conversation names, with what is known about them, for a
/// bot about to answer. Empty when the conversation names none.
pub async fn context_block(pool: &PgPool, workspace_id: Uuid, conversation: &str) -> String {
    if conversation.trim().is_empty() {
        return String::new();
    }
    let named: Vec<Contact> = sqlx::query_as(&format!(
        "SELECT {CONTACT_COLUMNS} FROM contacts
         WHERE workspace_id = $1 AND deleted_at IS NULL
           AND ((char_length(name) >= 3 AND strpos(lower($2), lower(name)) > 0)
             OR (email IS NOT NULL AND strpos(lower($2), lower(email::text)) > 0))
         ORDER BY char_length(name) DESC
         LIMIT $3"
    ))
    .bind(workspace_id)
    .bind(conversation)
    .bind(CONTEXT_CONTACTS)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    if named.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n## Contacts in this conversation\nWhat the office knows about the people named here.\n",
    );
    for c in named {
        let mut line = format!("- {}", c.name);
        for part in [c.company.as_deref(), c.email.as_deref(), c.phone.as_deref()]
            .into_iter()
            .flatten()
        {
            line.push_str(&format!(" · {part}"));
        }
        if c.marketing_consent == "unsubscribed" {
            line.push_str(" · has unsubscribed: no marketing email");
        }
        out.push_str(&line);
        out.push('\n');
        let facts: Vec<String> = sqlx::query_scalar(
            "SELECT body FROM memories WHERE contact_id = $1 AND deleted_at IS NULL
             ORDER BY pinned DESC, created_at DESC LIMIT 10",
        )
        .bind(c.id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        for fact in facts {
            out.push_str(&format!("  - {fact}\n"));
        }
        if let Some(notes) = c.notes.as_deref().filter(|n| !n.trim().is_empty()) {
            out.push_str(&format!(
                "  - Notes: {}\n",
                squash(notes).chars().take(300).collect::<String>()
            ));
        }
    }
    out
}

/// The contact named `name` in an office (exact, case-insensitive), for a
/// bot's `REMEMBER about <name>: ...`.
pub async fn named(pool: &PgPool, workspace_id: Uuid, name: &str) -> Option<Uuid> {
    sqlx::query_scalar(
        "SELECT id FROM contacts
         WHERE workspace_id = $1 AND deleted_at IS NULL
           AND (lower(name) = lower($2) OR email = $2)
         ORDER BY created_at ASC LIMIT 1",
    )
    .bind(workspace_id)
    .bind(name.trim())
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

// ---------- The owner's view ----------

async fn owned(state: &AppState, caller: &Caller, office: Uuid) -> Result<(), AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };
    crate::handlers::offices::owned_office(&state.pool, user.id, office).await?;
    Ok(())
}

/// A contact the caller owns (through its office).
async fn owned_contact(state: &AppState, caller: &Caller, id: Uuid) -> Result<Contact, AppError> {
    let Caller::User(user) = caller else {
        return Err(AppError::Forbidden);
    };
    let cols = CONTACT_COLUMNS
        .split(", ")
        .map(|c| format!("c.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    sqlx::query_as::<_, Contact>(&format!(
        "SELECT {cols} FROM contacts c JOIN workspaces w ON w.id = c.workspace_id
         WHERE c.id = $1 AND c.deleted_at IS NULL AND w.owner_user_id = $2 AND w.deleted_at IS NULL"
    ))
    .bind(id)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub q: Option<String>,
}

/// GET /api/offices/:id/contacts?q= -- newest first; `q` matches name,
/// email, phone or company.
pub async fn list_contacts(
    State(state): State<AppState>,
    caller: Caller,
    Path(office): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    owned(&state, &caller, office).await?;
    let q = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(|q| {
            let escaped: String = q
                .chars()
                .take(100)
                .flat_map(|c| match c {
                    '%' | '_' | '\\' => vec!['\\', c],
                    _ => vec![c],
                })
                .collect();
            format!("%{escaped}%")
        });
    let contacts: Vec<Contact> = sqlx::query_as(&format!(
        "SELECT {CONTACT_COLUMNS} FROM contacts
         WHERE workspace_id = $1 AND deleted_at IS NULL
           AND ($2::text IS NULL OR name ILIKE $2 OR email::text ILIKE $2 OR phone ILIKE $2 OR company ILIKE $2)
         ORDER BY updated_at DESC LIMIT 500"
    ))
    .bind(office)
    .bind(q)
    .fetch_all(&state.pool)
    .await?;
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM contacts WHERE workspace_id = $1 AND deleted_at IS NULL",
    )
    .bind(office)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(json!({ "contacts": contacts, "total": total })))
}

#[derive(Debug, Default, Deserialize)]
pub struct ContactInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub company: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub marketing_consent: Option<String>,
    pub idempotency_key: Option<String>,
}

struct Cleaned {
    name: Option<String>,
    email: Option<Option<String>>,
    phone: Option<Option<String>>,
    company: Option<Option<String>>,
    tags: Option<Vec<String>>,
    notes: Option<Option<String>>,
    consent: Option<String>,
}

fn clean_input(input: &ContactInput) -> Result<Cleaned, AppError> {
    let bad = |m: &str| AppError::BadRequest(m.to_string());
    let name = match input.name.as_deref() {
        Some(raw) => {
            let name = squash(raw);
            if name.is_empty() || name.chars().count() > 200 {
                return Err(bad("a contact's name must be 1-200 characters"));
            }
            Some(name)
        }
        None => None,
    };
    let email = match input.email.as_deref().map(str::trim) {
        Some("") => Some(None),
        Some(raw) => Some(Some(
            normalize_email(raw).ok_or_else(|| bad("that email doesn't look right"))?,
        )),
        None => None,
    };
    let phone = match input.phone.as_deref().map(str::trim) {
        Some("") => Some(None),
        Some(raw) => {
            Some(Some(normalize_phone(raw).ok_or_else(|| {
                bad("that phone number doesn't look right")
            })?))
        }
        None => None,
    };
    let company = match input.company.as_deref() {
        Some(raw) => {
            let c = squash(raw);
            if c.chars().count() > 200 {
                return Err(bad("company must be at most 200 characters"));
            }
            Some((!c.is_empty()).then_some(c))
        }
        None => None,
    };
    let tags = match &input.tags {
        Some(tags) => {
            let mut out: Vec<String> = tags
                .iter()
                .map(|t| squash(t))
                .filter(|t| !t.is_empty())
                .map(|t| t.chars().take(40).collect())
                .collect();
            out.sort();
            out.dedup();
            if out.len() > 20 {
                return Err(bad("at most 20 tags"));
            }
            Some(out)
        }
        None => None,
    };
    let notes = match input.notes.as_deref() {
        Some(raw) => {
            let n = raw.trim().to_string();
            if n.chars().count() > 4000 {
                return Err(bad("notes must be at most 4000 characters"));
            }
            Some((!n.is_empty()).then_some(n))
        }
        None => None,
    };
    let consent = match input.marketing_consent.as_deref() {
        Some(c @ ("unknown" | "subscribed" | "unsubscribed")) => Some(c.to_string()),
        Some(_) => {
            return Err(bad(
                "marketing_consent must be unknown, subscribed or unsubscribed",
            ))
        }
        None => None,
    };
    Ok(Cleaned {
        name,
        email,
        phone,
        company,
        tags,
        notes,
        consent,
    })
}

/// POST /api/offices/:id/contacts -- a new contact. An email the office
/// already has is refused (409) with that contact's id, so nothing is
/// duplicated.
pub async fn create_contact(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
    Json(input): Json<ContactInput>,
) -> Result<Json<Contact>, AppError> {
    let key = crate::idempotency::key(&headers, input.idempotency_key.as_deref())?;
    owned(&state, &caller, office).await?;
    let c = clean_input(&input)?;
    let name = c
        .name
        .ok_or_else(|| AppError::BadRequest("a contact needs a name".into()))?;
    if let Some(key) = key {
        if let Some(existing) = sqlx::query_as::<_, Contact>(&format!(
            "SELECT {CONTACT_COLUMNS} FROM contacts
             WHERE workspace_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL"
        ))
        .bind(office)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?
        {
            return Ok(Json(existing));
        }
    }
    let email = c.email.flatten();
    if let Some(email) = &email {
        let taken: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM contacts WHERE workspace_id = $1 AND email = $2 AND deleted_at IS NULL",
        )
        .bind(office)
        .bind(email)
        .fetch_optional(&state.pool)
        .await?;
        if let Some(id) = taken {
            return Err(AppError::Conflict(format!(
                "a contact with that email already exists ({id})"
            )));
        }
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM contacts WHERE workspace_id = $1 AND deleted_at IS NULL",
    )
    .bind(office)
    .fetch_one(&state.pool)
    .await?;
    if count >= MAX_CONTACTS {
        return Err(AppError::BadRequest(format!(
            "this office has {MAX_CONTACTS} contacts already"
        )));
    }
    let consent = c.consent.unwrap_or_else(|| "unknown".to_string());
    let contact = sqlx::query_as::<_, Contact>(&format!(
        "INSERT INTO contacts (workspace_id, name, email, phone, company, tags, notes, source,
                               marketing_consent, consent_updated_at, idempotency_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'owner', $8,
                 CASE WHEN $8 = 'unknown' THEN NULL ELSE now() END, $9)
         RETURNING {CONTACT_COLUMNS}"
    ))
    .bind(office)
    .bind(&name)
    .bind(&email)
    .bind(c.phone.flatten())
    .bind(c.company.flatten())
    .bind(c.tags.unwrap_or_default())
    .bind(c.notes.flatten())
    .bind(&consent)
    .bind(key)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(contact))
}

/// A record on a contact's timeline: id, app slug and title, status, data,
/// created and updated.
type TimelineRow = (
    Uuid,
    String,
    String,
    String,
    Value,
    DateTime<Utc>,
    DateTime<Utc>,
);

/// GET /api/contacts/:id -- the contact, its timeline (records linked to
/// it, newest first) and what the office remembers about it.
pub async fn get_contact(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let contact = owned_contact(&state, &caller, id).await?;
    let timeline: Vec<TimelineRow> = sqlx::query_as(
        "SELECT r.id, a.slug, a.title, r.status, r.data, r.created_at, r.updated_at
         FROM office_records r JOIN office_apps a ON a.id = r.app_id
         WHERE r.contact_id = $1 AND r.deleted_at IS NULL AND a.deleted_at IS NULL
         ORDER BY r.updated_at DESC LIMIT 100",
    )
    .bind(contact.id)
    .fetch_all(&state.pool)
    .await?;
    let memories: Vec<(Uuid, String, String, bool, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, body, source, pinned, created_at FROM memories
         WHERE contact_id = $1 AND deleted_at IS NULL
         ORDER BY pinned DESC, created_at DESC LIMIT 100",
    )
    .bind(contact.id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!({
        "contact": contact,
        "timeline": timeline.into_iter().map(|(id, slug, title, status, data, created, updated)| json!({
            "record_id": id, "app_slug": slug, "app_title": title, "status": status,
            "data": data, "created_at": created, "updated_at": updated,
        })).collect::<Vec<_>>(),
        "memories": memories.into_iter().map(|(id, body, source, pinned, created)| json!({
            "id": id, "body": body, "source": source, "pinned": pinned, "created_at": created,
        })).collect::<Vec<_>>(),
    })))
}

/// PATCH /api/contacts/:id -- the owner edits a contact (bots never do).
pub async fn update_contact(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<ContactInput>,
) -> Result<Json<Contact>, AppError> {
    crate::idempotency::key(&headers, input.idempotency_key.as_deref())?;
    let contact = owned_contact(&state, &caller, id).await?;
    let c = clean_input(&input)?;
    if let Some(Some(email)) = &c.email {
        let taken: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM contacts WHERE workspace_id = $1 AND email = $2
                           AND id <> $3 AND deleted_at IS NULL)",
        )
        .bind(contact.workspace_id)
        .bind(email)
        .bind(contact.id)
        .fetch_one(&state.pool)
        .await?;
        if taken {
            return Err(AppError::Conflict(
                "another contact has that email".to_string(),
            ));
        }
    }
    let updated = sqlx::query_as::<_, Contact>(&format!(
        "UPDATE contacts SET
            name = COALESCE($2, name),
            email = CASE WHEN $3 THEN $4 ELSE email END,
            phone = CASE WHEN $5 THEN $6 ELSE phone END,
            company = CASE WHEN $7 THEN $8 ELSE company END,
            tags = COALESCE($9, tags),
            notes = CASE WHEN $10 THEN $11 ELSE notes END,
            consent_updated_at = CASE WHEN $12::text IS NOT NULL AND $12 <> marketing_consent
                                      THEN now() ELSE consent_updated_at END,
            marketing_consent = COALESCE($12, marketing_consent),
            updated_at = now()
         WHERE id = $1
         RETURNING {CONTACT_COLUMNS}"
    ))
    .bind(contact.id)
    .bind(c.name)
    .bind(c.email.is_some())
    .bind(c.email.flatten())
    .bind(c.phone.is_some())
    .bind(c.phone.flatten())
    .bind(c.company.is_some())
    .bind(c.company.flatten())
    .bind(c.tags)
    .bind(c.notes.is_some())
    .bind(c.notes.flatten())
    .bind(c.consent)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(updated))
}

/// POST /api/contacts/:id/delete -- soft delete; linked records keep their
/// data and lose the link.
pub async fn delete_contact(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    crate::idempotency::key(&headers, None)?;
    let contact = owned_contact(&state, &caller, id).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE office_records SET contact_id = NULL WHERE contact_id = $1")
        .bind(contact.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE contacts SET deleted_at = now(), updated_at = now() WHERE id = $1")
        .bind(contact.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({ "status": "deleted", "id": contact.id })))
}

#[derive(Debug, Deserialize)]
pub struct ContactMemoryInput {
    pub body: String,
    pub idempotency_key: Option<String>,
}

/// POST /api/contacts/:id/memory -- a fact about this customer, shown to a
/// bot whenever the conversation names them.
pub async fn add_contact_memory(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<ContactMemoryInput>,
) -> Result<Json<Value>, AppError> {
    crate::idempotency::key(&headers, input.idempotency_key.as_deref())?;
    let contact = owned_contact(&state, &caller, id).await?;
    let body = squash(&input.body);
    if body.is_empty() || body.chars().count() > 500 {
        return Err(AppError::BadRequest(
            "a memory must be 1-500 characters".into(),
        ));
    }
    let memory_id: Uuid = sqlx::query_scalar(
        "INSERT INTO memories (workspace_id, contact_id, body, source)
         VALUES ($1, $2, $3, 'owner') RETURNING id",
    )
    .bind(contact.workspace_id)
    .bind(contact.id)
    .bind(&body)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(json!({ "id": memory_id, "body": body })))
}

/// POST /api/offices/:id/contacts/import -- adds the office's Stripe
/// customers as contacts (matched by their Stripe id, then by email), so
/// importing twice changes nothing.
pub async fn import_contacts(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    crate::idempotency::key(&headers, None)?;
    owned(&state, &caller, office).await?;
    let user_id = caller.user_id();
    let stripe: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM connections WHERE workspace_id = $1 AND provider = 'stripe'
           AND deleted_at IS NULL AND status = 'active'",
    )
    .bind(office)
    .fetch_all(&state.pool)
    .await?;
    if stripe.is_empty() {
        return Err(AppError::BadRequest(
            "connect Stripe to this office first".into(),
        ));
    }
    let (mut added, mut matched) = (0, 0);
    for connection in stripe {
        let result = crate::handlers::connections::read_for_owner(
            &state,
            office,
            user_id,
            connection,
            "stripe.customers",
            json!({ "max": 50 }),
        )
        .await
        .map_err(AppError::BadRequest)?;
        for customer in result["customers"].as_array().into_iter().flatten() {
            let Some(external) = customer["id"].as_str() else {
                continue;
            };
            let known: Option<Uuid> = sqlx::query_scalar(
                "SELECT x.contact_id FROM contact_external_ids x JOIN contacts c ON c.id = x.contact_id
                 WHERE x.workspace_id = $1 AND x.provider = 'stripe' AND x.external_id = $2
                   AND c.deleted_at IS NULL",
            )
            .bind(office)
            .bind(external)
            .fetch_optional(&state.pool)
            .await?;
            if known.is_some() {
                matched += 1;
                continue;
            }
            let email = customer["email"].as_str().and_then(normalize_email);
            let name = customer["name"]
                .as_str()
                .map(squash)
                .filter(|n| !n.is_empty())
                .or_else(|| email.clone());
            let Some(name) = name else {
                continue;
            };
            let existed: bool = match &email {
                Some(e) => sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM contacts WHERE workspace_id = $1 AND email = $2 AND deleted_at IS NULL)",
                )
                .bind(office)
                .bind(e)
                .fetch_one(&state.pool)
                .await?,
                None => false,
            };
            let Some(contact) = find_or_create(
                &state.pool,
                office,
                &name.chars().take(200).collect::<String>(),
                email.as_deref(),
                None,
                None,
                "stripe",
                None,
            )
            .await
            else {
                continue;
            };
            sqlx::query(
                "INSERT INTO contact_external_ids (workspace_id, contact_id, provider, external_id)
                 VALUES ($1, $2, 'stripe', $3) ON CONFLICT DO NOTHING",
            )
            .bind(office)
            .bind(contact)
            .bind(external)
            .execute(&state.pool)
            .await?;
            if existed {
                matched += 1;
            } else {
                added += 1;
            }
        }
    }
    Ok(Json(json!({ "added": added, "matched": matched })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_and_phones_are_normalized() {
        assert_eq!(
            normalize_email(" Ada@Example.COM "),
            Some("ada@example.com".into())
        );
        assert_eq!(normalize_email("not an email"), None);
        assert_eq!(normalize_email("a@b"), None);
        assert_eq!(
            normalize_phone("+1 (555) 014-4000"),
            Some("+15550144000".into())
        );
        assert_eq!(normalize_phone("12"), None);
    }

    #[test]
    fn a_record_names_its_person() {
        let data = json!({ "client_name": "Rosa's Cafe", "client_email": "Rosa@Cafe.example", "amount": 90 });
        let (name, email, phone, _) =
            person_in("invoice", data.as_object().expect("map")).expect("person");
        assert_eq!(name, "Rosa's Cafe");
        assert_eq!(email.as_deref(), Some("rosa@cafe.example"));
        assert!(phone.is_none());
        let none = json!({ "title": "Spring sale post" });
        assert!(person_in("video_post", none.as_object().expect("map")).is_none());
        // A project's name is not a person; a lead's is.
        let project = json!({ "name": "Kitchen refit" });
        assert!(person_in("project", project.as_object().expect("map")).is_none());
        let lead = json!({ "name": "Ada Lovelace" });
        assert_eq!(
            person_in("lead", lead.as_object().expect("map")).map(|p| p.0),
            Some("Ada Lovelace".into())
        );
    }
}
