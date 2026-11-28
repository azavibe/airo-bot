//! Desktop and cloud sync (issue #28).
//!
//! The desktop app links to an Airobot Cloud (or self-hosted) account: it
//! asks the cloud for a code, the person allows it in the cloud's Settings
//! (signed in there), and the desktop receives a sync key (kept on the cloud
//! only as an Argon2 hash, on the desktop only through `crypto::encrypt`).
//! Then, every minute and on **Sync now**, the desktop sends the rows that
//! changed on it and gets the cloud's changes back, over one endpoint:
//! offices, rooms (and which bots answer there), messages, bots, apps and
//! their records, contacts, memory and documents. No queue and no second
//! database: each side keeps `changed_at` per row (a trigger) and cursors.
//!
//! - **The cloud is the source of truth.** The newest edit of a row wins
//!   (`updated_at`); on a tie the cloud's copy stays.
//! - **Messages are only added**, never merged; deleting one travels as its
//!   soft delete, like every other row.
//! - **What never leaves the computer:** bot keys, the AI tool and model a
//!   bot uses here, spending caps, and connections with their keys.
//! - **The same thing on both sides** (the person's Architect, an app or a
//!   contact's email made on both) becomes one: the desktop keeps a map of
//!   its id to the cloud's (`sync_id_map`). Offices and rooms whose short
//!   names (slugs) clash are renamed, never merged.
//! - **Every row is checked** against the account: a key only ever reads or
//!   writes its own person's offices, bots and rooms.
//!
//! While a desktop is linked, the cloud runs the morning plan, check-ins and
//! weekly reports for the offices; the desktop doesn't, so they aren't posted
//! twice.
//!
//! TODO(air): removing a bot from a room, workflows, schedules, lessons and
//! Drive-folder documents don't sync yet, and messages that arrive by sync
//! show after a refresh rather than live.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::{Acquire, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth::{secret, Caller};
use crate::error::AppError;
use crate::state::AppState;

const KEY_PREFIX: &str = "air_sync_";
/// How often a linked desktop syncs.
const EVERY: Duration = Duration::from_secs(60);
/// Exchanges per sync, each up to a page per table.
const MAX_ROUNDS: usize = 10;
/// Rows changed this recently are sent again next time, in case a slower
/// transaction committed an older change after them.
const OVERLAP_SECONDS: i64 = 120;
/// The largest exchange accepted (a page of documents can be large).
pub const BODY_LIMIT: usize = 32 * 1024 * 1024;

/// A synced table: its columns (the rest stay on each side), the columns
/// that hold a person's id, and those that point at another synced table.
struct Table {
    name: &'static str,
    cols: &'static [&'static str],
    users: &'static [&'static str],
    refs: &'static [(&'static str, &'static str)],
    page: i64,
}

/// In the order rows are applied: everything after the tables it points at.
const TABLES: &[Table] = &[
    Table {
        name: "workspaces",
        cols: &[
            "id",
            "name",
            "slug",
            "description",
            "owner_user_id",
            "website",
            "focus",
            "business_type",
            "utc_offset_minutes",
            "daily_plan",
            "paused",
            "goal",
            "about",
            "check_in",
            "weekly_report",
            "language",
            "last_plan_at",
            "last_check_in_at",
            "last_report_at",
            "created_at",
            "updated_at",
            "deleted_at",
        ],
        users: &["owner_user_id"],
        refs: &[],
        page: 500,
    },
    Table {
        name: "agents",
        cols: &[
            "id",
            "user_id",
            "name",
            "handle",
            "instructions",
            "workspace_id",
            "documents_only",
            "permission_mode",
            "revoked_at",
            "created_at",
            "updated_at",
            "deleted_at",
        ],
        users: &["user_id"],
        refs: &[("workspace_id", "workspaces")],
        page: 500,
    },
    Table {
        name: "threads",
        cols: &[
            "id",
            "user_id",
            "title",
            "slug",
            "status",
            "visibility",
            "archived_at",
            "workspace_id",
            "kind",
            "created_at",
            "updated_at",
            "deleted_at",
        ],
        users: &["user_id"],
        refs: &[("workspace_id", "workspaces")],
        page: 500,
    },
    Table {
        name: "thread_agents",
        cols: &[
            "id",
            "thread_id",
            "agent_id",
            "joined_at",
            "created_at",
            "updated_at",
        ],
        users: &[],
        refs: &[("thread_id", "threads"), ("agent_id", "agents")],
        page: 500,
    },
    Table {
        name: "office_apps",
        cols: &[
            "id",
            "workspace_id",
            "slug",
            "title",
            "icon",
            "category",
            "replaces_saas",
            "ui_schema",
            "default_schema",
            "last_valid_schema",
            "archived_at",
            "created_at",
            "updated_at",
            "deleted_at",
        ],
        users: &[],
        refs: &[("workspace_id", "workspaces")],
        page: 500,
    },
    Table {
        name: "contacts",
        cols: &[
            "id",
            "workspace_id",
            "name",
            "email",
            "phone",
            "company",
            "tags",
            "notes",
            "source",
            "marketing_consent",
            "consent_updated_at",
            "last_contacted_at",
            "created_by_agent_id",
            "created_at",
            "updated_at",
            "deleted_at",
        ],
        users: &[],
        refs: &[
            ("workspace_id", "workspaces"),
            ("created_by_agent_id", "agents"),
        ],
        page: 500,
    },
    Table {
        name: "office_records",
        cols: &[
            "id",
            "workspace_id",
            "app_id",
            "record_type",
            "status",
            "data",
            "content_hash",
            "created_by_agent_id",
            "contact_id",
            "created_at",
            "updated_at",
            "deleted_at",
        ],
        users: &[],
        refs: &[
            ("workspace_id", "workspaces"),
            ("app_id", "office_apps"),
            ("created_by_agent_id", "agents"),
            ("contact_id", "contacts"),
        ],
        page: 500,
    },
    Table {
        name: "notes",
        cols: &[
            "id",
            "thread_id",
            "author_agent_id",
            "author_user_id",
            "body",
            "kind",
            "actions",
            "cards",
            "created_at",
            "updated_at",
            "deleted_at",
        ],
        users: &["author_user_id"],
        refs: &[("thread_id", "threads"), ("author_agent_id", "agents")],
        page: 500,
    },
    Table {
        name: "memories",
        cols: &[
            "id",
            "workspace_id",
            "agent_id",
            "body",
            "source",
            "created_by_agent_id",
            "thread_id",
            "pinned",
            "contact_id",
            "created_at",
            "updated_at",
            "deleted_at",
        ],
        users: &[],
        refs: &[
            ("workspace_id", "workspaces"),
            ("agent_id", "agents"),
            ("created_by_agent_id", "agents"),
            ("thread_id", "threads"),
            ("contact_id", "contacts"),
        ],
        page: 500,
    },
    Table {
        name: "documents",
        cols: &[
            "id",
            "workspace_id",
            "title",
            "kind",
            "filename",
            "source_url",
            "body",
            "chars",
            "visibility",
            "created_by_user_id",
            "outdated_at",
            "synced_at",
            "sync_error",
            "sync_failures",
            "created_at",
            "updated_at",
            "deleted_at",
        ],
        users: &["created_by_user_id"],
        refs: &[("workspace_id", "workspaces")],
        // Up to 200,000 characters each.
        page: 25,
    },
];

const OFFICES_OF: &str = "SELECT id FROM workspaces WHERE owner_user_id = $1";

/// The rows of `owner`'s account in `table` (`$1` is the owner).
fn scope(table: &str) -> String {
    let threads = format!(
        "SELECT t.id FROM threads t WHERE t.workspace_id IN ({OFFICES_OF}) \
         OR (t.workspace_id IS NULL AND t.user_id = $1)"
    );
    match table {
        "workspaces" => "x.owner_user_id = $1".to_string(),
        "agents" => "x.user_id = $1".to_string(),
        "threads" => format!(
            "(x.workspace_id IN ({OFFICES_OF}) OR (x.workspace_id IS NULL AND x.user_id = $1))"
        ),
        "thread_agents" | "notes" => format!("x.thread_id IN ({threads})"),
        "documents" => format!("x.workspace_id IN ({OFFICES_OF}) AND x.kind <> 'drive'"),
        _ => format!("x.workspace_id IN ({OFFICES_OF})"),
    }
}

/// The same check for a row already here, in an upsert (`$3` is the owner).
fn existing_scope(table: &str) -> String {
    let offices = "SELECT id FROM workspaces WHERE owner_user_id = $3";
    match table {
        "workspaces" => "workspaces.owner_user_id = $3".to_string(),
        "agents" => "agents.user_id = $3".to_string(),
        "threads" => format!(
            "(threads.workspace_id IN ({offices}) OR (threads.workspace_id IS NULL AND threads.user_id = $3))"
        ),
        "notes" => format!(
            "notes.thread_id IN (SELECT t.id FROM threads t WHERE t.workspace_id IN ({offices}) \
             OR (t.workspace_id IS NULL AND t.user_id = $3))"
        ),
        other => format!("{other}.workspace_id IN ({offices})"),
    }
}

fn upsert_sql(t: &Table) -> String {
    let mut insert: Vec<&str> = t.cols.to_vec();
    if t.name == "agents" {
        // A bot made by sync has no key here: nothing can sign in as it.
        insert.extend(["key_hash", "key_prefix"]);
    }
    let list = insert.join(", ");
    if t.name == "thread_agents" {
        return format!(
            "INSERT INTO thread_agents ({list}) SELECT {list} FROM jsonb_populate_record(NULL::thread_agents, $1)
             ON CONFLICT DO NOTHING RETURNING id"
        );
    }
    let changing: Vec<&str> = t
        .cols
        .iter()
        .copied()
        .filter(|c| *c != "id" && *c != "created_at")
        .collect();
    let sets = changing
        .iter()
        .map(|c| format!("{c} = EXCLUDED.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mine = changing
        .iter()
        .map(|c| format!("{}.{c}", t.name))
        .collect::<Vec<_>>()
        .join(", ");
    let theirs = changing
        .iter()
        .map(|c| format!("EXCLUDED.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO {name} ({list}) SELECT {list} FROM jsonb_populate_record(NULL::{name}, $1)
         ON CONFLICT (id) DO UPDATE SET {sets}
         WHERE {scope} AND ({name}.updated_at < EXCLUDED.updated_at
            OR ($2 AND {name}.updated_at = EXCLUDED.updated_at AND ({mine}) IS DISTINCT FROM ({theirs})))
         RETURNING id",
        name = t.name,
        scope = existing_scope(t.name),
    )
}

fn uuid_of(row: &Map<String, Value>, col: &str) -> Option<Uuid> {
    row.get(col)?.as_str()?.parse().ok()
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

/// Ids that are the same thing under another id on the other side, per
/// table: `incoming -> here`.
type IdMap = HashMap<(&'static str, Uuid), Uuid>;

/// Rewrites a row's id and pointers from the other side's ids to this
/// side's, and its people: `people` maps the other side's ids for the same
/// person to this side's.
fn translate(t: &Table, row: &mut Map<String, Value>, map: &IdMap, people: &HashMap<Uuid, Uuid>) {
    if let Some(id) = uuid_of(row, "id") {
        if let Some(to) = map.get(&(t.name, id)) {
            row.insert("id".into(), json!(to));
        }
    }
    for (col, target) in t.refs {
        if let Some(id) = uuid_of(row, col) {
            if let Some(to) = map.get(&(*target, id)) {
                row.insert((*col).into(), json!(to));
            }
        }
    }
    for col in t.users {
        if let Some(id) = uuid_of(row, col) {
            if let Some(to) = people.get(&id) {
                row.insert((*col).into(), json!(to));
            }
        }
    }
}

// ---------- Export ----------

struct Export {
    changes: Map<String, Value>,
    cursors: Map<String, Value>,
    /// A table had more than a page: ask again.
    more: bool,
}

/// The rows of `owner`'s account changed since `cursors`, a page per table,
/// in table order: when a table fills its page, the tables after it wait
/// for the next exchange, so a row never arrives before what it points at.
async fn export(
    pool: &PgPool,
    owner: Uuid,
    cursors: &Map<String, Value>,
) -> Result<Export, sqlx::Error> {
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT now()").fetch_one(pool).await?;
    let settled = now - chrono::Duration::seconds(OVERLAP_SECONDS);
    let mut changes = Map::new();
    let mut new_cursors = cursors.clone();
    let mut more = false;
    for t in TABLES {
        let since = cursors
            .get(t.name)
            .and_then(Value::as_str)
            .unwrap_or("-infinity")
            .to_string();
        let rows: Vec<(Value, DateTime<Utc>)> = sqlx::query_as(&format!(
            "SELECT to_jsonb(x), x.changed_at FROM {name} x
             WHERE {scope} AND x.changed_at > $2::timestamptz
             ORDER BY x.changed_at, x.id LIMIT {page}",
            name = t.name,
            scope = scope(t.name),
            page = t.page,
        ))
        .bind(owner)
        .bind(&since)
        .fetch_all(pool)
        .await?;
        let full = rows.len() as i64 == t.page;
        if let Some((_, last)) = rows.last() {
            // Past a full page, go on from its last row; otherwise stay a
            // little behind now, so late commits aren't missed.
            let next = if full { *last } else { (*last).min(settled) };
            let since_at: Option<DateTime<Utc>> = since.parse().ok();
            let next = since_at.map_or(next, |s| next.max(s));
            new_cursors.insert(t.name.into(), json!(stamp(next)));
        }
        let kept: Vec<Value> = rows
            .into_iter()
            .filter_map(|(row, _)| {
                let mut row = row.as_object()?.clone();
                row.retain(|k, _| t.cols.contains(&k.as_str()));
                Some(Value::Object(row))
            })
            .collect();
        changes.insert(t.name.into(), Value::Array(kept));
        if full {
            more = true;
            break;
        }
    }
    Ok(Export {
        changes,
        cursors: new_cursors,
        more,
    })
}

// ---------- Apply ----------

/// What an exchange's rows did here.
#[derive(Default)]
struct Applied {
    /// Rows that are the same as one already here: (table, incoming, here).
    merges: Vec<(&'static str, Uuid, Uuid)>,
    applied: usize,
    refused: usize,
}

/// What `owner` may point at here, loaded once per exchange and grown as
/// rows arrive.
struct Scope {
    owner: Uuid,
    offices: HashSet<Uuid>,
    agents: HashSet<Uuid>,
    threads: HashSet<Uuid>,
    apps: HashMap<Uuid, Uuid>,
    contacts: HashMap<Uuid, Uuid>,
    people: HashSet<Uuid>,
}

impl Scope {
    async fn load(
        tx: &mut Transaction<'_, Postgres>,
        owner: Uuid,
        people: HashSet<Uuid>,
    ) -> Result<Self, sqlx::Error> {
        let offices: Vec<Uuid> = sqlx::query_scalar(OFFICES_OF)
            .bind(owner)
            .fetch_all(&mut **tx)
            .await?;
        let agents: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM agents WHERE user_id = $1")
            .bind(owner)
            .fetch_all(&mut **tx)
            .await?;
        let threads: Vec<Uuid> = sqlx::query_scalar(&format!(
            "SELECT x.id FROM threads x WHERE {}",
            scope("threads")
        ))
        .bind(owner)
        .fetch_all(&mut **tx)
        .await?;
        let apps: Vec<(Uuid, Uuid)> = sqlx::query_as(&format!(
            "SELECT id, workspace_id FROM office_apps WHERE workspace_id IN ({OFFICES_OF})"
        ))
        .bind(owner)
        .fetch_all(&mut **tx)
        .await?;
        let contacts: Vec<(Uuid, Uuid)> = sqlx::query_as(&format!(
            "SELECT id, workspace_id FROM contacts WHERE workspace_id IN ({OFFICES_OF})"
        ))
        .bind(owner)
        .fetch_all(&mut **tx)
        .await?;
        let mut people = people;
        people.insert(owner);
        Ok(Self {
            owner,
            offices: offices.into_iter().collect(),
            agents: agents.into_iter().collect(),
            threads: threads.into_iter().collect(),
            apps: apps.into_iter().collect(),
            contacts: contacts.into_iter().collect(),
            people,
        })
    }

    fn office(&self, row: &Map<String, Value>) -> Option<Uuid> {
        uuid_of(row, "workspace_id").filter(|w| self.offices.contains(w))
    }

    fn agent_ok(&self, row: &Map<String, Value>, col: &str) -> bool {
        match row.get(col) {
            None | Some(Value::Null) => true,
            _ => uuid_of(row, col).is_some_and(|a| self.agents.contains(&a)),
        }
    }

    fn person_ok(&self, row: &Map<String, Value>, col: &str) -> bool {
        match row.get(col) {
            None | Some(Value::Null) => true,
            _ => uuid_of(row, col).is_some_and(|p| self.people.contains(&p)),
        }
    }

    /// Whether `row` of `table` is the owner's to write.
    fn allows(&self, table: &str, row: &Map<String, Value>) -> bool {
        match table {
            "workspaces" => uuid_of(row, "owner_user_id") == Some(self.owner),
            "agents" => {
                uuid_of(row, "user_id") == Some(self.owner)
                    && (row.get("workspace_id").is_none_or(Value::is_null)
                        || self.office(row).is_some())
            }
            "threads" => {
                let room_of_office = self.office(row).is_some();
                let personal = row.get("workspace_id").is_none_or(Value::is_null)
                    && uuid_of(row, "user_id") == Some(self.owner);
                (room_of_office || personal) && self.person_ok(row, "user_id")
            }
            "thread_agents" => {
                uuid_of(row, "thread_id").is_some_and(|t| self.threads.contains(&t))
                    && uuid_of(row, "agent_id").is_some_and(|a| self.agents.contains(&a))
            }
            "office_apps" => self.office(row).is_some(),
            "contacts" => self.office(row).is_some() && self.agent_ok(row, "created_by_agent_id"),
            "office_records" => {
                let Some(office) = self.office(row) else {
                    return false;
                };
                let app_ok =
                    uuid_of(row, "app_id").and_then(|a| self.apps.get(&a)) == Some(&office);
                let contact_ok = row.get("contact_id").is_none_or(Value::is_null)
                    || uuid_of(row, "contact_id").and_then(|c| self.contacts.get(&c))
                        == Some(&office);
                app_ok && contact_ok && self.agent_ok(row, "created_by_agent_id")
            }
            "notes" => {
                uuid_of(row, "thread_id").is_some_and(|t| self.threads.contains(&t))
                    && self.agent_ok(row, "author_agent_id")
                    && self.person_ok(row, "author_user_id")
            }
            "memories" => {
                let Some(office) = self.office(row) else {
                    return false;
                };
                let thread_ok = row.get("thread_id").is_none_or(Value::is_null)
                    || uuid_of(row, "thread_id").is_some_and(|t| self.threads.contains(&t));
                let contact_ok = row.get("contact_id").is_none_or(Value::is_null)
                    || uuid_of(row, "contact_id").and_then(|c| self.contacts.get(&c))
                        == Some(&office);
                thread_ok
                    && contact_ok
                    && self.agent_ok(row, "agent_id")
                    && self.agent_ok(row, "created_by_agent_id")
            }
            "documents" => {
                self.office(row).is_some()
                    && row.get("kind").and_then(Value::as_str) != Some("drive")
                    && self.person_ok(row, "created_by_user_id")
            }
            _ => false,
        }
    }

    /// A row that arrived is now something later rows may point at.
    fn learn(&mut self, table: &str, id: Uuid, row: &Map<String, Value>) {
        match table {
            "workspaces" => {
                self.offices.insert(id);
            }
            "agents" => {
                self.agents.insert(id);
            }
            "threads" => {
                self.threads.insert(id);
            }
            "office_apps" => {
                if let Some(office) = uuid_of(row, "workspace_id") {
                    self.apps.insert(id, office);
                }
            }
            "contacts" => {
                if let Some(office) = uuid_of(row, "workspace_id") {
                    self.contacts.insert(id, office);
                }
            }
            _ => {}
        }
    }
}

fn live(row: &Map<String, Value>) -> bool {
    row.get("deleted_at").is_none_or(Value::is_null)
}

fn suffixed(base: &str, id: Uuid) -> String {
    format!("{base}-{}", &id.simple().to_string()[..4])
}

async fn exists(
    tx: &mut Transaction<'_, Postgres>,
    table: &str,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(&format!(
        "SELECT EXISTS(SELECT 1 FROM {table} WHERE id = $1)"
    ))
    .bind(id)
    .fetch_one(&mut **tx)
    .await
}

enum Plan {
    Upsert,
    /// The same thing is already here under this id.
    Merge(Uuid),
}

/// Makes room for a row whose short name is taken here (renaming it), or
/// finds the same thing already here under another id.
async fn prepare(
    tx: &mut Transaction<'_, Postgres>,
    t: &Table,
    row: &mut Map<String, Value>,
    owner: Uuid,
) -> Result<Plan, sqlx::Error> {
    let Some(id) = uuid_of(row, "id") else {
        return Ok(Plan::Upsert);
    };
    let text = |row: &Map<String, Value>, col: &str| {
        row.get(col)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    match t.name {
        "workspaces" if live(row) => {
            let taken: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE owner_user_id = $1 AND slug = $2
                               AND deleted_at IS NULL AND id <> $3)",
            )
            .bind(owner)
            .bind(text(row, "slug"))
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
            if taken {
                let slug = suffixed(&text(row, "slug"), id);
                row.insert("slug".into(), json!(slug));
            }
        }
        "agents" if live(row) => {
            let same: Option<Uuid> = sqlx::query_scalar(
                "SELECT id FROM agents WHERE user_id = $1 AND name = $2 AND deleted_at IS NULL AND id <> $3",
            )
            .bind(owner)
            .bind(text(row, "name"))
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
            if let Some(same) = same {
                if !exists(tx, t.name, id).await? {
                    return Ok(Plan::Merge(same));
                }
            }
            if let Some(handle) = row
                .get("handle")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                let taken: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM agents WHERE handle = $1 AND deleted_at IS NULL AND id <> $2)",
                )
                .bind(&handle)
                .bind(id)
                .fetch_one(&mut **tx)
                .await?;
                if taken {
                    row.insert("handle".into(), json!(suffixed(&handle, id)));
                }
            }
        }
        "threads" if live(row) => {
            if text(row, "kind") == "office" {
                let chat: Option<Uuid> = sqlx::query_scalar(
                    "SELECT id FROM threads WHERE workspace_id = $1 AND kind = 'office'
                       AND deleted_at IS NULL AND id <> $2",
                )
                .bind(uuid_of(row, "workspace_id"))
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?;
                if let Some(chat) = chat {
                    if !exists(tx, t.name, id).await? {
                        return Ok(Plan::Merge(chat));
                    }
                }
            }
            let taken: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM threads WHERE user_id = $1 AND slug = $2
                               AND deleted_at IS NULL AND id <> $3)",
            )
            .bind(uuid_of(row, "user_id"))
            .bind(text(row, "slug"))
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
            if taken {
                let slug = suffixed(&text(row, "slug"), id);
                row.insert("slug".into(), json!(slug));
            }
        }
        "office_apps" if live(row) => {
            let same: Option<Uuid> = sqlx::query_scalar(
                "SELECT id FROM office_apps WHERE workspace_id = $1 AND slug = $2
                   AND deleted_at IS NULL AND id <> $3",
            )
            .bind(uuid_of(row, "workspace_id"))
            .bind(text(row, "slug"))
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
            if let Some(same) = same {
                if !exists(tx, t.name, id).await? {
                    return Ok(Plan::Merge(same));
                }
            }
        }
        "contacts" if live(row) && row.get("email").is_some_and(|e| !e.is_null()) => {
            let same: Option<Uuid> = sqlx::query_scalar(
                "SELECT id FROM contacts WHERE workspace_id = $1 AND email = $2
                   AND deleted_at IS NULL AND id <> $3",
            )
            .bind(uuid_of(row, "workspace_id"))
            .bind(text(row, "email"))
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
            if let Some(same) = same {
                if !exists(tx, t.name, id).await? {
                    return Ok(Plan::Merge(same));
                }
            }
        }
        _ => {}
    }
    Ok(Plan::Upsert)
}

#[allow(clippy::too_many_arguments)]
/// Applies rows from the other side, in table order, each only if it is
/// `owner`'s and newer than the copy here (or, with `incoming_wins_ties`,
/// as new and different). `quiet` (the desktop) keeps rows written here
/// from being sent back. `people`: the other people the rows may name.
async fn apply(
    pool: &PgPool,
    owner: Uuid,
    changes: &Map<String, Value>,
    map: &mut IdMap,
    person_map: &HashMap<Uuid, Uuid>,
    people: HashSet<Uuid>,
    incoming_wins_ties: bool,
    quiet: bool,
) -> Result<Applied, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if quiet {
        sqlx::query("SELECT set_config('airobot.sync_quiet', 'on', true)")
            .execute(&mut *tx)
            .await?;
    }
    let mut scope = Scope::load(&mut tx, owner, people).await?;
    let mut out = Applied::default();
    let mut threads_touched: HashSet<Uuid> = HashSet::new();
    let mut documents_touched: Vec<Uuid> = Vec::new();
    for t in TABLES {
        let Some(rows) = changes.get(t.name).and_then(Value::as_array) else {
            continue;
        };
        let sql = upsert_sql(t);
        for row in rows.iter().take(t.page as usize) {
            let Some(row) = row.as_object() else {
                continue;
            };
            let mut row = row.clone();
            row.retain(|k, _| t.cols.contains(&k.as_str()));
            translate(t, &mut row, map, person_map);
            let Some(incoming) = uuid_of(&row, "id") else {
                out.refused += 1;
                continue;
            };
            if !scope.allows(t.name, &row) {
                out.refused += 1;
                continue;
            }
            let mut step = (&mut *tx).begin().await?;
            let plan = match prepare(&mut step, t, &mut row, owner).await {
                Ok(plan) => plan,
                Err(error) => {
                    tracing::warn!(?error, table = t.name, "a synced row couldn't be checked");
                    step.rollback().await?;
                    out.refused += 1;
                    continue;
                }
            };
            if let Plan::Merge(here) = plan {
                step.commit().await?;
                map.insert((t.name, incoming), here);
                out.merges.push((t.name, incoming, here));
                continue;
            }
            if t.name == "agents" {
                row.insert("key_hash".into(), json!("sync:no-key"));
                row.insert("key_prefix".into(), json!("sync"));
            }
            let saved = if t.name == "thread_agents" {
                sqlx::query_scalar::<_, Uuid>(&sql)
                    .bind(Value::Object(row.clone()))
                    .fetch_optional(&mut *step)
                    .await
            } else {
                sqlx::query_scalar::<_, Uuid>(&sql)
                    .bind(Value::Object(row.clone()))
                    .bind(incoming_wins_ties)
                    .bind(owner)
                    .fetch_optional(&mut *step)
                    .await
            };
            match saved {
                Ok(Some(id)) => {
                    step.commit().await?;
                    out.applied += 1;
                    scope.learn(t.name, id, &row);
                    if t.name == "notes" {
                        if let Some(thread) = uuid_of(&row, "thread_id") {
                            threads_touched.insert(thread);
                        }
                    }
                    if t.name == "documents" {
                        documents_touched.push(id);
                    }
                }
                Ok(None) => {
                    // Not newer (or already here): nothing to do.
                    step.commit().await?;
                    scope.learn(t.name, incoming, &row);
                }
                Err(error) => {
                    tracing::warn!(?error, table = t.name, "a synced row wasn't applied");
                    step.rollback().await?;
                    out.refused += 1;
                }
            }
        }
    }
    // Counts and order of rooms follow their messages.
    if !threads_touched.is_empty() {
        let ids: Vec<Uuid> = threads_touched.into_iter().collect();
        sqlx::query(
            "UPDATE threads t SET note_count = s.n, last_note_at = s.last
             FROM (SELECT thread_id, count(*) FILTER (WHERE deleted_at IS NULL)::int AS n,
                          max(created_at) AS last
                   FROM notes WHERE thread_id = ANY($1) GROUP BY thread_id) s
             WHERE t.id = s.thread_id",
        )
        .bind(&ids)
        .execute(&mut *tx)
        .await?;
    }
    // A document's passages are rebuilt from its text.
    for id in documents_touched {
        let doc: Option<(Uuid, String, String)> =
            sqlx::query_as("SELECT workspace_id, title, body FROM documents WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some((office, title, body)) = doc {
            sqlx::query("DELETE FROM document_chunks WHERE document_id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            let parts = crate::office::documents::chunks(&body);
            crate::office::documents::write_chunks(&mut tx, id, office, &title, &parts).await?;
        }
    }
    tx.commit().await?;
    Ok(out)
}

/// The other people (not `owner`) named by rows, with their handles, so the
/// desktop can show who wrote a message.
async fn people_in(
    pool: &PgPool,
    owner: Uuid,
    changes: &Map<String, Value>,
) -> Result<Vec<Value>, sqlx::Error> {
    let mut ids: HashSet<Uuid> = HashSet::new();
    for t in TABLES {
        for row in changes
            .get(t.name)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
        {
            for col in t.users {
                if let Some(id) = uuid_of(row, col).filter(|id| *id != owner) {
                    ids.insert(id);
                }
            }
        }
    }
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = ids.into_iter().collect();
    let rows: Vec<(Uuid, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, handle, display_name FROM users WHERE id = ANY($1)")
            .bind(&ids)
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, handle, name)| json!({ "id": id, "handle": handle, "display_name": name }))
        .collect())
}

// ---------- The cloud: linking a computer ----------

fn cloud_only(state: &AppState) -> Result<(), AppError> {
    if state.config.local.is_some() {
        return Err(AppError::NotFound);
    }
    Ok(())
}

fn person(caller: &Caller) -> Result<Uuid, AppError> {
    match caller {
        Caller::User(user) => Ok(user.id),
        Caller::Agent(_) => Err(AppError::Forbidden),
    }
}

#[derive(Deserialize, Default)]
pub struct LinkRequest {
    #[serde(default)]
    pub name: Option<String>,
    pub idempotency_key: Option<String>,
}

/// POST /api/sync/link -- a computer asks to link: a code for the person to
/// allow in Settings, and a secret only the computer has to fetch its key.
pub async fn create_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<LinkRequest>>,
) -> Result<Response, AppError> {
    cloud_only(&state)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    let key = crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let name: String = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("A computer")
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect();
    for _ in 0..5 {
        let id = Uuid::new_v4();
        let code = secret::random_code(crate::handlers::device::CODE_LEN);
        let raw_device_secret = secret::random_hex_token(32);
        let device_code = secret::encode_keyed_secret("air_device_", id, &raw_device_secret);
        let hash = secret::hash_secret(&raw_device_secret).map_err(|_| AppError::Internal)?;
        let made = sqlx::query(
            "INSERT INTO login_codes (id, code, device_secret_hash, agent_name, purpose, expires_at, idempotency_key)
             VALUES ($1, $2, $3, $4, 'sync', $5, $6)",
        )
        .bind(id)
        .bind(&code)
        .bind(hash)
        .bind(&name)
        .bind(Utc::now() + crate::auth::DEVICE_CODE_TTL)
        .bind(key)
        .execute(&state.pool)
        .await;
        match made {
            Ok(_) => {
                return Ok((
                    StatusCode::CREATED,
                    Json(json!({
                        "code": crate::handlers::device::format_code_for_display(&code),
                        "device_code": device_code,
                        "link_url": format!(
                            "{}/settings?sync_code={code}",
                            state.config.app_base_url.trim_end_matches('/')
                        ),
                        "expires_in": crate::auth::DEVICE_CODE_TTL.num_seconds(),
                    })),
                )
                    .into_response());
            }
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(AppError::Internal)
}

#[derive(Deserialize)]
pub struct CodeQuery {
    pub code: String,
}

/// GET /api/sync/link?code= -- what the person is asked to allow.
pub async fn show_link(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<CodeQuery>,
) -> Result<Json<Value>, AppError> {
    cloud_only(&state)?;
    person(&caller)?;
    let code = crate::handlers::device::normalize_code(&q.code);
    let name: Option<String> = sqlx::query_scalar(
        "SELECT agent_name FROM login_codes
         WHERE code = $1 AND purpose = 'sync' AND approved_at IS NULL AND denied_at IS NULL
           AND expires_at > now()",
    )
    .bind(&code)
    .fetch_optional(&state.pool)
    .await?
    .flatten();
    let name = name.ok_or(AppError::NotFound)?;
    Ok(Json(
        json!({ "name": name, "code": crate::handlers::device::format_code_for_display(&code) }),
    ))
}

#[derive(Deserialize)]
pub struct CodeBody {
    pub code: String,
    pub idempotency_key: Option<String>,
}

/// POST /api/sync/link/approve -- the signed-in person allows the computer.
pub async fn approve_link(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Json(body): Json<CodeBody>,
) -> Result<Json<Value>, AppError> {
    cloud_only(&state)?;
    let user = person(&caller)?;
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let code = crate::handlers::device::normalize_code(&body.code);
    let done = sqlx::query(
        "UPDATE login_codes SET user_id = $1, approved_at = now(), updated_at = now()
         WHERE code = $2 AND purpose = 'sync' AND approved_at IS NULL AND denied_at IS NULL
           AND expires_at > now()",
    )
    .bind(user)
    .bind(&code)
    .execute(&state.pool)
    .await?;
    if done.rows_affected() == 0 {
        return Err(AppError::BadRequest(
            "this code is invalid, expired, or already used".into(),
        ));
    }
    Ok(Json(json!({ "approved": true })))
}

/// POST /api/sync/link/deny
pub async fn deny_link(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Json(body): Json<CodeBody>,
) -> Result<Json<Value>, AppError> {
    cloud_only(&state)?;
    person(&caller)?;
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let code = crate::handlers::device::normalize_code(&body.code);
    sqlx::query(
        "UPDATE login_codes SET denied_at = now(), updated_at = now()
         WHERE code = $1 AND purpose = 'sync' AND approved_at IS NULL AND denied_at IS NULL",
    )
    .bind(&code)
    .execute(&state.pool)
    .await?;
    Ok(Json(json!({ "denied": true })))
}

#[derive(Deserialize)]
pub struct TokenBody {
    pub device_code: String,
    pub idempotency_key: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PendingCode {
    device_secret_hash: Option<String>,
    user_id: Option<Uuid>,
    agent_name: Option<String>,
    denied_at: Option<DateTime<Utc>>,
    token_delivered_at: Option<DateTime<Utc>>,
    expires_at: DateTime<Utc>,
}

/// POST /api/sync/link/token -- the computer, until allowed: then its key,
/// handed over once.
pub async fn link_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TokenBody>,
) -> Result<Json<Value>, AppError> {
    cloud_only(&state)?;
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let expired = Json(json!({ "status": "expired" }));
    let Some((id, raw)) = secret::decode_keyed_secret("air_device_", &body.device_code) else {
        return Ok(expired);
    };
    let mut tx = state.pool.begin().await?;
    let row: Option<PendingCode> = sqlx::query_as(
        "SELECT device_secret_hash, user_id, agent_name, denied_at, token_delivered_at, expires_at
         FROM login_codes WHERE id = $1 AND purpose = 'sync' FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(code) = row else {
        return Ok(expired);
    };
    if !code
        .device_secret_hash
        .is_some_and(|h| secret::verify_secret(&raw, &h))
        || code.expires_at < Utc::now()
        || code.token_delivered_at.is_some()
    {
        return Ok(expired);
    }
    if code.denied_at.is_some() {
        return Ok(Json(json!({ "status": "denied" })));
    }
    let name = code.agent_name;
    let Some(user) = code.user_id else {
        return Ok(Json(json!({ "status": "pending" })));
    };
    let key_id = Uuid::new_v4();
    let raw_secret = secret::random_hex_token(32);
    let key_hash = secret::hash_secret(&raw_secret).map_err(|_| AppError::Internal)?;
    sqlx::query("INSERT INTO sync_keys (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
        .bind(key_id)
        .bind(user)
        .bind(name.unwrap_or_else(|| "A computer".into()))
        .bind(key_hash)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE login_codes SET token_delivered_at = now(), updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    let handle: Option<String> = sqlx::query_scalar("SELECT handle FROM users WHERE id = $1")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({
        "status": "approved",
        "sync_key": secret::encode_keyed_secret(KEY_PREFIX, key_id, &raw_secret),
        "user_id": user,
        "handle": handle,
    })))
}

/// GET /api/sync/computers -- the person's linked computers.
pub async fn computers(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Value>, AppError> {
    cloud_only(&state)?;
    let user = person(&caller)?;
    let rows: Vec<LinkedComputer> = sqlx::query_as(
        "SELECT id, name, last_used_at, created_at FROM sync_keys
         WHERE user_id = $1 AND revoked_at IS NULL AND deleted_at IS NULL ORDER BY created_at DESC",
    )
    .bind(user)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!({
        "computers": rows.iter().map(|c| json!({
            "id": c.id, "name": c.name, "last_synced_at": c.last_used_at, "linked_at": c.created_at,
        })).collect::<Vec<_>>(),
    })))
}

#[derive(sqlx::FromRow)]
struct LinkedComputer {
    id: Uuid,
    name: String,
    last_used_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

/// POST /api/sync/computers/:id/revoke -- the computer stops syncing.
pub async fn revoke_computer(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    cloud_only(&state)?;
    let user = person(&caller)?;
    let done = sqlx::query(
        "UPDATE sync_keys SET revoked_at = now(), updated_at = now()
         WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(user)
    .execute(&state.pool)
    .await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "revoked": true })))
}

/// The person a sync key belongs to, and the key's id.
async fn key_owner(state: &AppState, headers: &HeaderMap) -> Result<(Uuid, Uuid), AppError> {
    let raw = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(AppError::Unauthorized)?;
    let (id, raw_secret) =
        secret::decode_keyed_secret(KEY_PREFIX, raw.trim()).ok_or(AppError::Unauthorized)?;
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT user_id, key_hash FROM sync_keys
         WHERE id = $1 AND revoked_at IS NULL AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let (user, hash) = row.ok_or(AppError::Unauthorized)?;
    if !secret::verify_secret_memoized(&raw_secret, &hash) {
        return Err(AppError::Unauthorized);
    }
    let gone: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND deleted_at IS NOT NULL)",
    )
    .bind(user)
    .fetch_one(&state.pool)
    .await?;
    if gone {
        return Err(AppError::Unauthorized);
    }
    Ok((user, id))
}

/// POST /api/sync/revoke -- a computer unlinks itself.
pub async fn revoke_own_key(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    cloud_only(&state)?;
    let (_, key) = key_owner(&state, &headers).await?;
    sqlx::query("UPDATE sync_keys SET revoked_at = now(), updated_at = now() WHERE id = $1")
        .bind(key)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "revoked": true })))
}

#[derive(Deserialize)]
pub struct ExchangeRequest {
    #[serde(default)]
    pub changes: Map<String, Value>,
    /// Where the computer's copy of the cloud is, per table.
    #[serde(default)]
    pub cursors: Map<String, Value>,
    pub idempotency_key: Option<String>,
}

/// POST /api/sync -- a linked computer's changes in, the cloud's out.
/// Applying is idempotent (newest wins), so a repeated exchange is safe.
pub async fn exchange(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ExchangeRequest>,
) -> Result<Json<Value>, AppError> {
    cloud_only(&state)?;
    let (owner, key) = key_owner(&state, &headers).await?;
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    // People who may appear in the owner's offices: the owner and members.
    let members: Vec<Uuid> = sqlx::query_scalar(&format!(
        "SELECT DISTINCT user_id FROM workspace_members
         WHERE workspace_id IN ({OFFICES_OF}) AND deleted_at IS NULL"
    ))
    .bind(owner)
    .fetch_all(&state.pool)
    .await?;
    let mut map = IdMap::new();
    let applied = apply(
        &state.pool,
        owner,
        &body.changes,
        &mut map,
        &HashMap::new(),
        members.into_iter().collect(),
        false,
        false,
    )
    .await?;
    let out = export(&state.pool, owner, &body.cursors).await?;
    let people = people_in(&state.pool, owner, &out.changes).await?;
    sqlx::query("UPDATE sync_keys SET last_used_at = now(), updated_at = now() WHERE id = $1")
        .bind(key)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({
        "applied": applied.applied,
        "refused": applied.refused,
        "merges": applied.merges.iter().map(|(table, local, cloud)| json!({
            "table": table, "local": local, "cloud": cloud,
        })).collect::<Vec<_>>(),
        "changes": out.changes,
        "cursors": out.cursors,
        "more": out.more,
        "people": people,
    })))
}

// ---------- The desktop ----------

fn desktop_only(state: &AppState) -> Result<(), AppError> {
    if state.config.local.is_none() {
        return Err(AppError::NotFound);
    }
    Ok(())
}

fn client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// A cloud address: https, or http to this computer (a server run here).
fn cloud_url(raw: &str) -> Result<String, AppError> {
    let bad =
        || AppError::BadRequest("enter your Airobot address, like https://airo-bot.com".into());
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| bad())?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if !(url.scheme() == "https" || (url.scheme() == "http" && local)) || url.host_str().is_none() {
        return Err(bad());
    }
    Ok(url.origin().ascii_serialization())
}

async fn cloud_call(
    base: &str,
    path: &str,
    key: Option<&str>,
    body: Value,
) -> Result<Value, String> {
    let mut req = client().post(format!("{base}{path}")).json(&body);
    if let Some(key) = key {
        req = req.bearer_auth(key);
    }
    let res = req
        .send()
        .await
        .map_err(|e| format!("Airobot Cloud couldn't be reached ({})", e.without_url()))?;
    let status = res.status();
    let data: Value = res.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        Ok(data)
    } else if status == StatusCode::UNAUTHORIZED {
        Err("Airobot Cloud no longer accepts this computer; link it again".into())
    } else {
        let why = data["error"].as_str().unwrap_or("an error");
        Err(format!(
            "Airobot Cloud said: {}",
            why.chars().take(200).collect::<String>()
        ))
    }
}

#[derive(sqlx::FromRow)]
struct LinkRow {
    id: Uuid,
    owner_user_id: Uuid,
    cloud_url: String,
    cloud_user_id: Option<Uuid>,
    cloud_handle: Option<String>,
    key_encrypted: Option<String>,
    device_code_encrypted: Option<String>,
    link_code: Option<String>,
    link_url: Option<String>,
    link_expires_at: Option<DateTime<Utc>>,
    cursors: Value,
    last_synced_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
}

async fn link_of(pool: &PgPool) -> Result<Option<LinkRow>, sqlx::Error> {
    sqlx::query_as::<_, LinkRow>(
        "SELECT id, owner_user_id, cloud_url, cloud_user_id, cloud_handle, key_encrypted,
                device_code_encrypted, link_code, link_url, link_expires_at, cursors,
                last_synced_at, last_error
         FROM sync_links WHERE deleted_at IS NULL",
    )
    .fetch_optional(pool)
    .await
}

/// Whether this desktop is linked to a cloud that runs the offices' daily
/// jobs (the morning plan, check-ins, reports).
pub async fn leaves_jobs_to_cloud(state: &AppState) -> bool {
    if state.config.local.is_none() {
        return false;
    }
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM sync_links WHERE deleted_at IS NULL AND key_encrypted IS NOT NULL)",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or(false)
}

async fn note_link_error(pool: &PgPool, link: Uuid, error: Option<&str>) {
    let error = error.map(|e| e.chars().take(300).collect::<String>());
    let _ = sqlx::query("UPDATE sync_links SET last_error = $2, updated_at = now() WHERE id = $1")
        .bind(link)
        .bind(error)
        .execute(pool)
        .await;
}

/// The pending link: asks the cloud once whether the person allowed it.
async fn check_pending(pool: &PgPool, link: &LinkRow) -> Result<(), String> {
    let Some(device_code) = link
        .device_code_encrypted
        .as_deref()
        .and_then(crate::crypto::decrypt)
    else {
        return Ok(());
    };
    let answer = cloud_call(
        &link.cloud_url,
        "/api/sync/link/token",
        None,
        json!({ "device_code": device_code }),
    )
    .await?;
    match answer["status"].as_str() {
        Some("approved") => {
            let key = answer["sync_key"].as_str().ok_or("no key came back")?;
            let sealed = crate::crypto::encrypt(key).ok_or("the key couldn't be stored")?;
            let user: Option<Uuid> = answer["user_id"].as_str().and_then(|u| u.parse().ok());
            sqlx::query(
                "UPDATE sync_links SET key_encrypted = $2, cloud_user_id = $3, cloud_handle = $4,
                        device_code_encrypted = NULL, link_code = NULL, link_url = NULL,
                        link_expires_at = NULL, last_error = NULL, updated_at = now()
                 WHERE id = $1",
            )
            .bind(link.id)
            .bind(sealed)
            .bind(user)
            .bind(answer["handle"].as_str())
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
            Ok(())
        }
        Some("pending") => Ok(()),
        Some("denied") => {
            let _ = sqlx::query(
                "UPDATE sync_links SET deleted_at = now(), updated_at = now() WHERE id = $1",
            )
            .bind(link.id)
            .execute(pool)
            .await;
            Err("the link wasn't allowed".into())
        }
        _ => {
            let _ = sqlx::query(
                "UPDATE sync_links SET deleted_at = now(), updated_at = now() WHERE id = $1",
            )
            .bind(link.id)
            .execute(pool)
            .await;
            Err("the code expired; start again".into())
        }
    }
}

fn status_json(link: Option<&LinkRow>) -> Value {
    let linked = link.is_some_and(|l| l.key_encrypted.is_some());
    json!({
        "available": true,
        "linked": linked,
        "pending": link.filter(|l| l.key_encrypted.is_none()).map(|l| json!({
            "code": l.link_code, "link_url": l.link_url, "expires_at": l.link_expires_at,
        })),
        "cloud_url": link.map(|l| l.cloud_url.clone()),
        "cloud_handle": link.and_then(|l| l.cloud_handle.clone()),
        "last_synced_at": link.and_then(|l| l.last_synced_at),
        "last_error": link.and_then(|l| l.last_error.clone()),
    })
}

fn owner_only(caller: &Caller) -> Result<Uuid, AppError> {
    person(caller)
}

/// GET /api/sync/status -- the desktop's link (checking a pending one).
pub async fn status(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Value>, AppError> {
    if state.config.local.is_none() {
        return Ok(Json(json!({ "available": false })));
    }
    owner_only(&caller)?;
    let mut link = link_of(&state.pool).await?;
    if let Some(l) = link.as_ref().filter(|l| l.key_encrypted.is_none()) {
        if let Err(why) = check_pending(&state.pool, l).await {
            tracing::info!(why, "sync link not allowed");
            let mut gone = status_json(None);
            gone["last_error"] = json!(why);
            return Ok(Json(gone));
        }
        link = link_of(&state.pool).await?;
    }
    Ok(Json(status_json(link.as_ref())))
}

#[derive(Deserialize)]
pub struct ConnectRequest {
    pub cloud_url: String,
    #[serde(default)]
    pub name: Option<String>,
    pub idempotency_key: Option<String>,
}

/// POST /api/sync/connect -- start linking to a cloud: a code to allow there.
pub async fn connect(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Json(body): Json<ConnectRequest>,
) -> Result<Json<Value>, AppError> {
    desktop_only(&state)?;
    let owner = owner_only(&caller)?;
    let key = crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let base = cloud_url(&body.cloud_url)?;
    if let Some(existing) = link_of(&state.pool).await? {
        if existing.key_encrypted.is_some() {
            return Err(AppError::Conflict("this computer is already linked".into()));
        }
    }
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("Desktop app")
        .to_string();
    let made = cloud_call(&base, "/api/sync/link", None, json!({ "name": name }))
        .await
        .map_err(|e| AppError::BadRequest(format!("That didn't work. {e}")))?;
    let (Some(code), Some(device_code), Some(link_url)) = (
        made["code"].as_str(),
        made["device_code"].as_str(),
        made["link_url"].as_str(),
    ) else {
        return Err(AppError::BadRequest(
            "That address didn't answer like Airobot.".into(),
        ));
    };
    let sealed = crate::crypto::encrypt(device_code).ok_or_else(|| {
        tracing::error!("could not encrypt a sync device code");
        AppError::Internal
    })?;
    let expires =
        Utc::now() + chrono::Duration::seconds(made["expires_in"].as_i64().unwrap_or(600));
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "UPDATE sync_links SET deleted_at = now(), updated_at = now() WHERE deleted_at IS NULL",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO sync_links (owner_user_id, cloud_url, device_code_encrypted, link_code, link_url,
                                 link_expires_at, idempotency_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(owner)
    .bind(&base)
    .bind(sealed)
    .bind(code)
    .bind(link_url)
    .bind(expires)
    .bind(key)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({ "code": code, "link_url": link_url, "cloud_url": base }),
    ))
}

/// POST /api/sync/disconnect -- stop syncing; the cloud forgets the key.
pub async fn disconnect(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Value>, AppError> {
    desktop_only(&state)?;
    owner_only(&caller)?;
    if let Some(link) = link_of(&state.pool).await? {
        if let Some(key) = link
            .key_encrypted
            .as_deref()
            .and_then(crate::crypto::decrypt)
        {
            if let Err(why) =
                cloud_call(&link.cloud_url, "/api/sync/revoke", Some(&key), json!({})).await
            {
                tracing::info!(why, "the cloud wasn't told the computer unlinked");
            }
        }
        sqlx::query("UPDATE sync_links SET deleted_at = now(), updated_at = now() WHERE id = $1")
            .bind(link.id)
            .execute(&state.pool)
            .await?;
    }
    Ok(Json(status_json(None)))
}

/// POST /api/sync/now
pub async fn sync_now(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Value>, AppError> {
    desktop_only(&state)?;
    owner_only(&caller)?;
    if let Err(why) = sync_once(&state).await {
        return Err(AppError::BadRequest(why));
    }
    Ok(Json(status_json(link_of(&state.pool).await?.as_ref())))
}

/// One sync with the cloud: exchanges until both sides are caught up (or
/// `MAX_ROUNDS`). Returns how many rows came in.
pub async fn sync_once(state: &AppState) -> Result<usize, String> {
    let pool = &state.pool;
    let link = link_of(pool)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("this computer isn't linked")?;
    let (Some(key), Some(cloud_user)) = (
        link.key_encrypted
            .as_deref()
            .and_then(crate::crypto::decrypt),
        link.cloud_user_id,
    ) else {
        return Err("this computer isn't linked yet".into());
    };
    let result = rounds(pool, &link, &key, cloud_user).await;
    match &result {
        Ok(_) => {
            let _ = sqlx::query(
                "UPDATE sync_links SET last_synced_at = now(), last_error = NULL, updated_at = now() WHERE id = $1",
            )
            .bind(link.id)
            .execute(pool)
            .await;
        }
        Err(why) => note_link_error(pool, link.id, Some(why)).await,
    }
    result
}

async fn load_map(pool: &PgPool) -> Result<(IdMap, IdMap), sqlx::Error> {
    let rows: Vec<(String, Uuid, Uuid)> =
        sqlx::query_as("SELECT table_name, local_id, cloud_id FROM sync_id_map")
            .fetch_all(pool)
            .await?;
    let mut to_cloud = IdMap::new();
    let mut to_local = IdMap::new();
    for (table, local, cloud) in rows {
        if let Some(t) = TABLES.iter().find(|t| t.name == table) {
            to_cloud.insert((t.name, local), cloud);
            to_local.insert((t.name, cloud), local);
        }
    }
    Ok((to_cloud, to_local))
}

async fn remember_merge(pool: &PgPool, table: &str, local: Uuid, cloud: Uuid) {
    let _ = sqlx::query(
        "INSERT INTO sync_id_map (table_name, local_id, cloud_id) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(table)
    .bind(local)
    .bind(cloud)
    .execute(pool)
    .await;
}

async fn rounds(
    pool: &PgPool,
    link: &LinkRow,
    key: &str,
    cloud_user: Uuid,
) -> Result<usize, String> {
    let owner = link.owner_user_id;
    let mut cursors = link.cursors.as_object().cloned().unwrap_or_default();
    let mut received = 0;
    for _ in 0..MAX_ROUNDS {
        let (to_cloud, mut to_local) = load_map(pool).await.map_err(|e| e.to_string())?;
        let push_cursors = cursors
            .get("push")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let pull_cursors = cursors
            .get("pull")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut out = export(pool, owner, &push_cursors)
            .await
            .map_err(|e| e.to_string())?;
        // This computer's ids and owner, as the cloud knows them.
        let me_to_cloud = HashMap::from([(owner, cloud_user)]);
        for t in TABLES {
            if let Some(rows) = out.changes.get_mut(t.name).and_then(Value::as_array_mut) {
                for row in rows.iter_mut().filter_map(Value::as_object_mut) {
                    translate(t, row, &to_cloud, &me_to_cloud);
                }
            }
        }
        let answer = cloud_call(
            &link.cloud_url,
            "/api/sync",
            Some(key),
            json!({ "changes": out.changes, "cursors": pull_cursors }),
        )
        .await?;
        for m in answer["merges"].as_array().into_iter().flatten() {
            let (Some(table), Some(local), Some(cloud)) = (
                m["table"].as_str(),
                m["local"].as_str().and_then(|v| v.parse::<Uuid>().ok()),
                m["cloud"].as_str().and_then(|v| v.parse::<Uuid>().ok()),
            ) else {
                continue;
            };
            if let Some(t) = TABLES.iter().find(|t| t.name == table) {
                remember_merge(pool, t.name, local, cloud).await;
                to_local.insert((t.name, cloud), local);
            }
        }
        // Other people in the account's offices: shown by their handles.
        let mut people = HashSet::new();
        for p in answer["people"].as_array().into_iter().flatten() {
            let Some(id) = p["id"].as_str().and_then(|v| v.parse::<Uuid>().ok()) else {
                continue;
            };
            if id == cloud_user {
                continue;
            }
            let name = p["handle"]
                .as_str()
                .or(p["display_name"].as_str())
                .map(|n| n.chars().take(64).collect::<String>());
            let _ = sqlx::query(
                "INSERT INTO users (id, email, display_name) VALUES ($1, $2, $3)
                 ON CONFLICT (id) DO UPDATE SET display_name = EXCLUDED.display_name",
            )
            .bind(id)
            .bind(format!("{id}@sync.airobot.local"))
            .bind(name)
            .execute(pool)
            .await;
            people.insert(id);
        }
        let incoming = answer["changes"].as_object().cloned().unwrap_or_default();
        let cloud_to_me = HashMap::from([(cloud_user, owner)]);
        let applied = apply(
            pool,
            owner,
            &incoming,
            &mut to_local,
            &cloud_to_me,
            people,
            true,
            true,
        )
        .await
        .map_err(|e| e.to_string())?;
        for (table, incoming_id, here) in applied.merges {
            // A cloud row that is the same as one made here.
            remember_merge(pool, table, here, incoming_id).await;
        }
        received += applied.applied;
        cursors.insert("push".into(), Value::Object(out.cursors));
        cursors.insert(
            "pull".into(),
            answer["cursors"]
                .as_object()
                .cloned()
                .map(Value::Object)
                .unwrap_or_default(),
        );
        sqlx::query("UPDATE sync_links SET cursors = $2, updated_at = now() WHERE id = $1")
            .bind(link.id)
            .bind(Value::Object(cursors.clone()))
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
        if !out.more && answer["more"] != true {
            break;
        }
    }
    Ok(received)
}

/// Background loop on the desktop: syncs every minute while linked.
pub async fn run_loop(state: AppState) {
    if state.config.local.is_none() {
        return;
    }
    loop {
        tokio::time::sleep(EVERY).await;
        let linked = matches!(link_of(&state.pool).await, Ok(Some(l)) if l.key_encrypted.is_some());
        if !linked {
            continue;
        }
        match sync_once(&state).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(rows = n, "synced with the cloud"),
            Err(why) => tracing::warn!(why, "sync with the cloud failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_addresses_are_https_or_this_computer() {
        assert_eq!(
            cloud_url("https://airo-bot.com/").unwrap(),
            "https://airo-bot.com"
        );
        assert_eq!(
            cloud_url("https://office.example:8443/x").unwrap(),
            "https://office.example:8443"
        );
        assert_eq!(
            cloud_url("http://127.0.0.1:8099").unwrap(),
            "http://127.0.0.1:8099"
        );
        assert!(cloud_url("http://airo-bot.com").is_err());
        assert!(cloud_url("ftp://airo-bot.com").is_err());
        assert!(cloud_url("not a url").is_err());
    }

    #[test]
    fn secrets_and_local_choices_never_travel() {
        let agents = TABLES.iter().find(|t| t.name == "agents").expect("agents");
        for private in [
            "key_hash",
            "key_prefix",
            "provider_api_key",
            "provider_type",
            "provider_model",
            "ai_engine",
            "ai_model",
            "monthly_cap_credits",
        ] {
            assert!(!agents.cols.contains(&private), "{private}");
        }
        let offices = TABLES
            .iter()
            .find(|t| t.name == "workspaces")
            .expect("workspaces");
        assert!(!offices.cols.contains(&"monthly_cap_credits"));
    }

    #[test]
    fn tables_come_after_what_they_point_at() {
        for (i, t) in TABLES.iter().enumerate() {
            for (_, target) in t.refs {
                let at = TABLES
                    .iter()
                    .position(|x| x.name == *target)
                    .expect("target");
                assert!(at < i, "{} points at {target} before it", t.name);
            }
        }
    }

    #[test]
    fn a_tie_goes_to_the_cloud_only_on_the_desktop() {
        let sql = upsert_sql(TABLES.iter().find(|t| t.name == "notes").expect("notes"));
        assert!(sql.contains("notes.updated_at < EXCLUDED.updated_at"));
        assert!(sql.contains("$2 AND notes.updated_at = EXCLUDED.updated_at"));
        assert!(sql.contains("notes.thread_id IN"));
    }
}
