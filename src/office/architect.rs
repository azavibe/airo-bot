//! Virtual-office onboarding and the server-driven Apps API.
//!
//! Apps belong to an office: the apps endpoints take `?office=<id>` (checked
//! against the caller's access) and default to the caller's first office.
//! Credits stay with the first office, so onboarding can't mint extra
//! wallets.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::auth::Caller;
use crate::error::AppError;
use crate::handlers::airo::get_or_create_default_workspace;
use crate::models::{OfficeApp, OfficeRecord, WorkspaceLesson, WorkspaceWallet};
use crate::state::AppState;

const APP_COLUMNS: &str = "id, workspace_id, slug, title, icon, category, replaces_saas, ui_schema, default_schema, last_valid_schema, archived_at, created_at, updated_at";
const RECORD_COLUMNS: &str = "id, workspace_id, app_id, record_type, status, data, content_hash, created_by_agent_id, contact_id, created_at, updated_at";
const WALLET_SELECT: &str = "SELECT workspace_id, plan_tier, credit_balance, max_daily_credit_burn,
        CASE WHEN burn_day < CURRENT_DATE THEN 0 ELSE credits_burned_today END AS credits_burned_today,
        dedicated_phone_number, updated_at
 FROM workspace_wallets WHERE workspace_id = $1";
const MAX_RECORDS: i64 = 500;

/// 64-bit FNV-1a over the record's canonical JSON. Stable across Rust
/// releases (std's `DefaultHasher` is not), so stored hashes keep matching.
fn calculate_content_hash(data: &serde_json::Value) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data.to_string().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash as i64
}

fn check_len(value: &str, field: &str, max: usize) -> Result<(), AppError> {
    if value.chars().count() > max {
        return Err(AppError::BadRequest(format!(
            "{field} must be at most {max} characters"
        )));
    }
    Ok(())
}

fn valid_slug(slug: &str) -> bool {
    (1..=64).contains(&slug.len())
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `?office=<id>`: the office whose apps a request is about.
#[derive(Debug, Deserialize, Default)]
pub struct OfficeQuery {
    #[serde(default)]
    pub office: Option<Uuid>,
}

/// The office an apps request is about: the one named (after checking the
/// caller may use it), else the caller's first office.
async fn app_workspace(
    state: &AppState,
    caller: &Caller,
    office: Option<Uuid>,
) -> Result<Uuid, AppError> {
    match office {
        Some(id) => {
            crate::handlers::airo::ensure_workspace_access(&state.pool, caller, id).await?;
            Ok(id)
        }
        None => primary_workspace_id(state, caller).await,
    }
}

async fn primary_workspace_id(state: &AppState, caller: &Caller) -> Result<Uuid, AppError> {
    Ok(
        get_or_create_default_workspace(&state.pool, caller.user_id())
            .await?
            .id,
    )
}

async fn find_app(state: &AppState, workspace_id: Uuid, slug: &str) -> Result<OfficeApp, AppError> {
    sqlx::query_as::<_, OfficeApp>(&format!(
        "SELECT {APP_COLUMNS} FROM office_apps
         WHERE workspace_id = $1 AND slug = $2 AND deleted_at IS NULL"
    ))
    .bind(workspace_id)
    .bind(slug)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)
}

// ---------- Onboarding ----------

#[derive(Debug, Deserialize)]
pub struct OnboardRequest {
    pub business_type: String,
    pub replaced_saas: Vec<String>,
    pub goals: String,
    pub company_name: Option<String>,
    pub idempotency_key: Option<String>,
    /// What the office handles (`sales`, `marketing`, `finance`, `support`,
    /// `bookings`, `store`): each gets a room with its own bot and, where
    /// there is one, an app. Absent: the original four rooms.
    #[serde(default)]
    pub teams: Option<Vec<String>>,
    /// Set up this office (one of the caller's) instead of the first one.
    /// Its rooms and bots go there; apps and credits stay account-wide.
    pub workspace_id: Option<Uuid>,
    /// The business's website; the Architect reads it at kickoff.
    #[serde(default)]
    pub website: Option<String>,
    /// What matters most this month ("More customers", ...).
    #[serde(default)]
    pub focus: Option<String>,
    /// The person's offset from UTC in minutes (east positive), for the
    /// Architect's morning plan.
    #[serde(default)]
    pub utc_offset_minutes: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct OnboardResponse {
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub channels: Vec<String>,
    pub apps_created: Vec<String>,
    pub credit_balance: i64,
    pub status: String,
}

struct AppBlueprint {
    slug: &'static str,
    title: &'static str,
    icon: &'static str,
    category: &'static str,
    replaces: &'static str,
    label: &'static str,
    schema: serde_json::Value,
}

/// The areas an office can handle: (id, room without `#`, app keyword).
/// Each room's bot is in `office::OFFICE_ROLES`.
pub const TEAMS: &[(&str, &str, &str)] = &[
    ("sales", "leads-crm", "crm"),
    ("marketing", "social-video", "social"),
    // Finance brings Quotes too: a quote becomes an invoice.
    ("finance", "finance-ops", "finance quotes"),
    ("support", "support-desk", "support"),
    ("bookings", "bookings", "scheduling"),
    // Projects get their own room and Project Manager; quotes and reviews
    // are apps for the sales and support rooms' bots.
    ("projects", "projects", "projects"),
    ("quotes", "leads-crm", "quotes"),
    ("reviews", "support-desk", "reviews"),
    // TODO(air): no store app yet; Shopify's own admin is the store, and
    // bots reach it through the Shopify connection.
    ("store", "store-ops", ""),
];

/// The original setup, used when a request names no teams.
const LEGACY_CHANNELS: [&str; 3] = ["leads-crm", "social-video", "finance-ops"];

/// `default_when_empty`: with no tools named, start with the CRM and the
/// studio (the original setup). Off when teams were chosen explicitly.
fn blueprints(saas: &[String], default_when_empty: bool) -> Vec<AppBlueprint> {
    let any = |words: &[&str]| saas.iter().any(|s| words.iter().any(|w| s.contains(w)));
    let defaults = default_when_empty && saas.is_empty();
    let mut out = Vec::new();

    if defaults || any(&["hubspot", "gohighlevel", "crm"]) {
        out.push(AppBlueprint {
            slug: "leads-pipeline",
            title: "Lead Pipeline & CRM",
            icon: "Kanban",
            category: "crm",
            replaces: "HubSpot / GoHighLevel",
            label: "Lead Pipeline & CRM (HubSpot/GHL Replaced)",
            schema: json!({
                "version": "1.0",
                "layout": "split_view",
                "header_metrics": [
                    { "id": "m1", "label": "Total Active Leads", "query_record_type": "lead", "aggregation": "count" },
                    { "id": "m2", "label": "Pipeline Value", "query_record_type": "lead", "aggregation": "sum", "field": "estimated_value" }
                ],
                "views": [
                    {
                        "id": "v_kanban", "type": "kanban", "title": "Pipeline Board", "record_type": "lead",
                        "group_by_field": "stage",
                        "columns": ["New Lead", "Qualified", "Quote Sent", "Won"],
                        "card_fields": ["name", "phone", "estimated_value", "last_agent_note"]
                    },
                    {
                        "id": "v_table", "type": "datatable", "title": "All Records", "record_type": "lead",
                        "columns": [
                            { "key": "name", "label": "Customer Name", "type": "text" },
                            { "key": "phone", "label": "Phone", "type": "phone" },
                            { "key": "stage", "label": "Stage", "type": "badge" },
                            { "key": "estimated_value", "label": "Value ($)", "type": "currency" }
                        ]
                    },
                    {
                        "id": "v_chart", "type": "chart", "title": "Pipeline value by stage", "record_type": "lead",
                        "chart_type": "bar", "x_axis": "stage", "y_axis": "estimated_value",
                        "order": ["New Lead", "Qualified", "Quote Sent", "Won"]
                    }
                ],
                "quick_actions": [
                    { "label": "Find new leads", "ask": "Find 5 new leads that fit this business and add them to the Lead Pipeline." },
                    { "label": "Follow up quiet leads", "ask": "Which leads in the Lead Pipeline have gone quiet? Draft a short follow-up for each." }
                ]
            }),
        });
    }

    if defaults || any(&["buffer", "content", "social", "video"]) {
        out.push(AppBlueprint {
            slug: "social-video-studio",
            title: "Social Studio",
            icon: "Video",
            category: "marketing",
            replaces: "Buffer / Hootsuite",
            label: "Social Studio (Buffer Replaced)",
            schema: json!({
                "version": "1.0",
                "layout": "split_view",
                "header_metrics": [
                    { "id": "m1", "label": "Queued Clips", "query_record_type": "video_post", "aggregation": "count" },
                    { "id": "m2", "label": "Avg Retention", "query_record_type": "video_post", "aggregation": "avg", "field": "retention_pct" }
                ],
                "views": [{
                    "id": "v_media", "type": "media_gallery", "title": "Generated Videos (Wan 3.0 / Social)",
                    "record_type": "video_post", "media_field": "video_url", "caption_field": "script_hook",
                    "metrics_fields": ["views", "retention_pct", "platform"]
                }],
                "quick_actions": [
                    { "label": "Plan this week's posts", "ask": "Plan this week's social posts in the Social studio: a hook, a caption and the platform for each." }
                ]
            }),
        });
    }

    if any(&["xero", "quickbooks", "invoice", "finance"]) {
        out.push(AppBlueprint {
            slug: "invoices-cashflow",
            title: "Invoices & Cashflow",
            icon: "FileText",
            category: "finance",
            replaces: "Xero / QuickBooks",
            label: "Invoices & Cashflow (Xero Replaced)",
            schema: json!({
                "version": "1.0",
                "layout": "table_view",
                "header_metrics": [
                    { "id": "m1", "label": "Outstanding Invoices", "query_record_type": "invoice", "aggregation": "count" }
                ],
                "views": [
                    {
                        "id": "v_table", "type": "datatable", "title": "Invoices & Receivables", "record_type": "invoice",
                        "columns": [
                            { "key": "invoice_number", "label": "Invoice #", "type": "text" },
                            { "key": "client_name", "label": "Client", "type": "text" },
                            { "key": "amount", "label": "Amount ($)", "type": "currency" },
                            { "key": "due_date", "label": "Due Date", "type": "date" },
                            { "key": "status", "label": "Status", "type": "badge" }
                        ]
                    },
                    {
                        "id": "v_chart", "type": "chart", "title": "Invoiced by month", "record_type": "invoice",
                        "chart_type": "bar", "x_axis": "due_date", "y_axis": "amount"
                    }
                ],
                "quick_actions": [
                    { "label": "Chase overdue invoices", "ask": "List the overdue invoices and draft a polite reminder for each. Drafts only." },
                    { "label": "Cashflow this month", "ask": "Summarise this month's cashflow from Invoices & Cashflow: billed, paid, overdue, and what to do next." }
                ]
            }),
        });
    }

    if any(&["zendesk", "support", "helpdesk", "freshdesk"]) {
        out.push(AppBlueprint {
            slug: "support-inbox",
            title: "Support Inbox",
            icon: "LifeBuoy",
            category: "support",
            replaces: "Zendesk / Freshdesk",
            label: "Support Inbox",
            schema: json!({
                "version": "1.0",
                "layout": "split_view",
                "header_metrics": [
                    { "id": "m1", "label": "Open Tickets", "query_record_type": "ticket", "aggregation": "count" }
                ],
                "views": [
                    {
                        "id": "v_kanban", "type": "kanban", "title": "Tickets", "record_type": "ticket",
                        "group_by_field": "stage",
                        "columns": ["New", "Waiting on us", "Waiting on customer", "Solved"],
                        "card_fields": ["customer", "subject", "channel", "last_agent_note"]
                    },
                    {
                        "id": "v_table", "type": "datatable", "title": "All Tickets", "record_type": "ticket",
                        "columns": [
                            { "key": "customer", "label": "Customer", "type": "text" },
                            { "key": "subject", "label": "Subject", "type": "text" },
                            { "key": "channel", "label": "Channel", "type": "text" },
                            { "key": "stage", "label": "Stage", "type": "badge" }
                        ]
                    },
                    {
                        "id": "v_chart", "type": "chart", "title": "Tickets by stage", "record_type": "ticket",
                        "chart_type": "bar", "x_axis": "stage",
                        "order": ["New", "Waiting on us", "Waiting on customer", "Solved"]
                    }
                ],
                "quick_actions": [
                    { "label": "Draft replies", "ask": "Draft a reply to each new ticket in the Support Inbox. Drafts only." }
                ]
            }),
        });
    }

    if any(&["calendly", "scheduling", "booking", "appointment"]) {
        out.push(AppBlueprint {
            slug: "appointments",
            title: "Appointments",
            icon: "CalendarDays",
            category: "scheduling",
            replaces: "Calendly / Acuity",
            label: "Appointments",
            schema: json!({
                "version": "1.0",
                "layout": "table_view",
                "header_metrics": [
                    { "id": "m1", "label": "Upcoming", "query_record_type": "appointment", "aggregation": "count" }
                ],
                "views": [
                    {
                        "id": "v_table", "type": "datatable", "title": "Appointments", "record_type": "appointment",
                        "columns": [
                            { "key": "client", "label": "Client", "type": "text" },
                            { "key": "service", "label": "Service", "type": "text" },
                            { "key": "starts_at", "label": "When", "type": "date" },
                            { "key": "phone", "label": "Phone", "type": "phone" },
                            { "key": "status", "label": "Status", "type": "badge" }
                        ]
                    },
                    {
                        "id": "v_chart", "type": "chart", "title": "Bookings by month", "record_type": "appointment",
                        "chart_type": "spline", "x_axis": "starts_at"
                    }
                ],
                "quick_actions": [
                    { "label": "Confirm tomorrow", "ask": "List tomorrow's appointments and draft a confirmation message for each." }
                ]
            }),
        });
    }
    if any(&["project", "asana", "trello", "monday", "tasks"]) {
        out.push(AppBlueprint {
            slug: "projects-tasks",
            title: "Projects & Tasks",
            icon: "FolderKanban",
            category: "operations",
            replaces: "Asana / Trello",
            label: "Projects & Tasks",
            schema: json!({
                "version": "1.0",
                "layout": "split_view",
                "header_metrics": [
                    { "id": "m1", "label": "Projects", "query_record_type": "project", "aggregation": "count" },
                    { "id": "m2", "label": "Project value", "query_record_type": "project", "aggregation": "sum", "field": "value" }
                ],
                "views": [
                    {
                        "id": "v_kanban", "type": "kanban", "title": "Projects", "record_type": "project",
                        "group_by_field": "stage",
                        "columns": ["Backlog", "In Progress", "Waiting on Client", "Done"],
                        "card_fields": ["name", "client", "due_date", "next_milestone"]
                    },
                    {
                        "id": "v_table", "type": "datatable", "title": "All projects", "record_type": "project",
                        "columns": [
                            { "key": "name", "label": "Project", "type": "text" },
                            { "key": "client", "label": "Client", "type": "text" },
                            { "key": "due_date", "label": "Due", "type": "date" },
                            { "key": "value", "label": "Value ($)", "type": "currency" },
                            { "key": "next_milestone", "label": "Next milestone", "type": "text" },
                            { "key": "stage", "label": "Stage", "type": "badge" }
                        ]
                    },
                    {
                        "id": "v_chart", "type": "chart", "title": "Projects by stage", "record_type": "project",
                        "chart_type": "bar", "x_axis": "stage",
                        "order": ["Backlog", "In Progress", "Waiting on Client", "Done"]
                    }
                ],
                "quick_actions": [
                    { "label": "What's late?", "ask": "Which projects in Projects & Tasks are late or at risk? Say what each one needs next." },
                    { "label": "Client updates", "ask": "Draft a short status update for each client with a project in progress. Drafts only." }
                ]
            }),
        });
    }

    if any(&["quote", "proposal", "estimate", "pandadoc"]) {
        out.push(AppBlueprint {
            slug: "quotes-proposals",
            title: "Quotes & Proposals",
            icon: "FileSignature",
            category: "sales",
            replaces: "PandaDoc / Proposify",
            label: "Quotes & Proposals",
            schema: json!({
                "version": "1.0",
                "layout": "split_view",
                "header_metrics": [
                    { "id": "m1", "label": "Quotes", "query_record_type": "quote", "aggregation": "count" },
                    { "id": "m2", "label": "Quoted value", "query_record_type": "quote", "aggregation": "sum", "field": "total" }
                ],
                "views": [
                    {
                        "id": "v_kanban", "type": "kanban", "title": "Quotes", "record_type": "quote",
                        "group_by_field": "stage",
                        "columns": ["Draft", "Sent", "Approved", "Declined"],
                        "card_fields": ["title", "client_name", "total", "expires_on"]
                    },
                    {
                        "id": "v_table", "type": "datatable", "title": "All quotes", "record_type": "quote",
                        "columns": [
                            { "key": "title", "label": "Quote", "type": "text" },
                            { "key": "client_name", "label": "Client", "type": "text" },
                            { "key": "total", "label": "Total ($)", "type": "currency" },
                            { "key": "expires_on", "label": "Expires", "type": "date" },
                            { "key": "stage", "label": "Stage", "type": "badge" }
                        ]
                    },
                    {
                        "id": "v_chart", "type": "chart", "title": "Quoted value by stage", "record_type": "quote",
                        "chart_type": "bar", "x_axis": "stage", "y_axis": "total",
                        "order": ["Draft", "Sent", "Approved", "Declined"]
                    }
                ],
                // Sending a quote always waits for the owner's OK.
                "approval_stages": ["Sent"],
                "convert": [
                    {
                        "label": "Convert to invoice", "to_app": "invoices-cashflow", "when_stage": "Approved",
                        "map": { "client_name": "client_name", "amount": "total", "description": "title" },
                        "set": { "status": "Draft", "invoice_number": "INV-{ref}", "due_date": "{today+14}" }
                    },
                    {
                        "label": "Start a project", "to_app": "projects-tasks", "when_stage": "Approved",
                        "map": { "name": "title", "client": "client_name", "value": "total" },
                        "set": { "stage": "Backlog" }
                    }
                ],
                "quick_actions": [
                    { "label": "Draft a quote", "ask": "Draft a quote with line items for the most promising lead and add it to Quotes & Proposals as a Draft." },
                    { "label": "Chase open quotes", "ask": "Which quotes in Quotes & Proposals were sent but not answered? Draft a friendly follow-up for each. Drafts only." }
                ]
            }),
        });
    }

    if any(&["review", "reputation", "birdeye", "trustpilot"]) {
        out.push(AppBlueprint {
            slug: "reviews-reputation",
            title: "Reviews & Reputation",
            icon: "Star",
            category: "marketing",
            replaces: "Birdeye / Podium",
            label: "Reviews & Reputation",
            schema: json!({
                "version": "1.0",
                "layout": "split_view",
                "header_metrics": [
                    { "id": "m1", "label": "Reviews", "query_record_type": "review", "aggregation": "count" }
                ],
                "views": [
                    {
                        "id": "v_kanban", "type": "kanban", "title": "Reviews", "record_type": "review",
                        "group_by_field": "stage",
                        "columns": ["To request", "Requested", "Reviewed", "Replied"],
                        "card_fields": ["customer", "source", "rating", "review_text"]
                    },
                    {
                        "id": "v_table", "type": "datatable", "title": "All reviews", "record_type": "review",
                        "columns": [
                            { "key": "customer", "label": "Customer", "type": "text" },
                            { "key": "source", "label": "Where", "type": "text" },
                            { "key": "rating", "label": "Rating", "type": "text" },
                            { "key": "review_text", "label": "Review", "type": "text" },
                            { "key": "reply_draft", "label": "Reply", "type": "text" },
                            { "key": "stage", "label": "Stage", "type": "badge" }
                        ]
                    },
                    {
                        "id": "v_chart", "type": "chart", "title": "Reviews by rating", "record_type": "review",
                        "chart_type": "bar", "x_axis": "rating",
                        "order": ["1", "2", "3", "4", "5"]
                    }
                ],
                // A review request or a public reply goes out only with the owner's OK.
                "approval_stages": ["Requested", "Replied"],
                "quick_actions": [
                    { "label": "Ask for reviews", "ask": "Pick the 3 happiest recent customers and add a review request for each to Reviews & Reputation (stage To request), with a short, personal message." },
                    { "label": "Draft replies", "ask": "Draft a reply to each review in Reviews & Reputation that has none yet, thankful and specific." }
                ]
            }),
        });
    }
    out
}

/// POST /api/office/onboard
///
/// Provisions the caller's primary workspace as a virtual office. Safe to
/// repeat: the wallet is created once and never re-granted, channels and
/// seed workflows are created once, and only apps not yet present are
/// added. Runs in one transaction.
pub async fn onboard_workspace(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Json(payload): Json<OnboardRequest>,
) -> Result<Json<OnboardResponse>, AppError> {
    crate::idempotency::key(&headers, payload.idempotency_key.as_deref())?;
    let teams: Option<Vec<&(&str, &str, &str)>> = match &payload.teams {
        None => None,
        Some(asked) => {
            if asked.is_empty() || asked.len() > TEAMS.len() {
                return Err(AppError::BadRequest(
                    "choose at least one area for the office".into(),
                ));
            }
            let chosen: Vec<_> = TEAMS
                .iter()
                .filter(|team| asked.iter().any(|a| a == team.0))
                .collect();
            if chosen.len() != asked.len() {
                return Err(AppError::BadRequest(
                    "teams must be among sales, marketing, finance, support, bookings, projects, quotes, reviews, store"
                        .into(),
                ));
            }
            Some(chosen)
        }
    };
    // Choosing areas is enough to set up an office; the business type only
    // flavours the starter workflows.
    let business_type = match payload.business_type.trim() {
        "" if teams.is_some() => "Small business",
        other => other,
    };
    if business_type.is_empty() {
        return Err(AppError::BadRequest("business_type is required".into()));
    }
    check_len(business_type, "business_type", 100)?;
    check_len(&payload.goals, "goals", 2000)?;
    if payload.replaced_saas.len() > 20 {
        return Err(AppError::BadRequest(
            "at most 20 tools can be replaced".into(),
        ));
    }
    let company_name = payload
        .company_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{business_type} HQ"));
    check_len(&company_name, "company_name", 100)?;
    let website = crate::office::kickoff::clean_website(payload.website.as_deref().unwrap_or(""))
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let focus = payload
        .focus
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(str::to_string);
    if let Some(f) = &focus {
        check_len(f, "focus", 200)?;
    }
    let utc_offset = payload
        .utc_offset_minutes
        .filter(|m| (-840..=840).contains(m));

    let user_id = caller.user_id();
    let workspace = get_or_create_default_workspace(&state.pool, user_id).await?;
    // The office whose rooms are set up (and renamed after the company).
    let office_id = match payload.workspace_id {
        Some(id) if id != workspace.id => {
            crate::handlers::airo::ensure_workspace_access(&state.pool, &caller, id).await?;
            id
        }
        _ => workspace.id,
    };
    let mut tx = state.pool.begin().await?;

    sqlx::query(
        "UPDATE workspaces SET name = $1, description = $2,
                business_type = $4,
                website = COALESCE($5, website),
                focus = COALESCE($6, focus),
                utc_offset_minutes = COALESCE($7, utc_offset_minutes),
                updated_at = now()
         WHERE id = $3",
    )
    .bind(&company_name)
    .bind(format!("Airobot Office for {}", payload.goals.trim()))
    .bind(office_id)
    .bind(business_type)
    .bind(&website)
    .bind(&focus)
    .bind(utc_offset)
    .execute(&mut *tx)
    .await?;

    // Created once with the plan's monthly credits; never reset here.
    sqlx::query(
        "INSERT INTO workspace_wallets (workspace_id) VALUES ($1)
         ON CONFLICT (workspace_id) DO NOTHING",
    )
    .bind(workspace.id)
    .execute(&mut *tx)
    .await?;

    // First setup of this office: no starter workflows yet.
    let first_time: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS(
             SELECT 1 FROM procedures WHERE workspace_id = $1 AND deleted_at IS NULL
         )",
    )
    .bind(office_id)
    .fetch_one(&mut *tx)
    .await?;

    // The office's group chat is made below; these are the one-to-one rooms.
    let mut channel_names: Vec<&str> = Vec::new();
    match &teams {
        Some(chosen) => channel_names.extend(chosen.iter().map(|t| t.1)),
        None => channel_names.extend(LEGACY_CHANNELS),
    }
    let mut channels = Vec::new();
    for ch in channel_names {
        let title = format!("#{ch}");
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM threads WHERE workspace_id = $1 AND title = $2 AND deleted_at IS NULL LIMIT 1",
        )
        .bind(office_id)
        .bind(&title)
        .fetch_optional(&mut *tx)
        .await?;
        if existing.is_none() {
            sqlx::query(
                "INSERT INTO threads (title, slug, user_id, workspace_id, visibility, status)
                 VALUES ($1, $2, $3, $4, 'private', 'open')",
            )
            .bind(&title)
            .bind(format!(
                "{ch}-{}",
                &Uuid::new_v4().simple().to_string()[..6]
            ))
            .bind(user_id)
            .bind(office_id)
            .execute(&mut *tx)
            .await?;
        }
        channels.push(title);
    }

    let mut saas: Vec<String> = payload
        .replaced_saas
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    if let Some(chosen) = &teams {
        saas.extend(
            chosen
                .iter()
                .filter(|t| !t.2.is_empty())
                .map(|t| t.2.to_string()),
        );
    }
    let mut apps_created = Vec::new();
    for app in blueprints(&saas, teams.is_none()) {
        let app_id: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO office_apps (
                workspace_id, slug, title, icon, category, replaces_saas, ui_schema, default_schema, last_valid_schema
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $7, $7)
             ON CONFLICT (workspace_id, slug) WHERE deleted_at IS NULL DO NOTHING
             RETURNING id",
        )
        .bind(office_id)
        .bind(app.slug)
        .bind(app.title)
        .bind(app.icon)
        .bind(app.category)
        .bind(app.replaces)
        .bind(&app.schema)
        .fetch_optional(&mut *tx)
        .await?;
        // Apps start empty: the office's bots fill them with records that
        // fit this business (see office::context), not fixed samples.
        if app_id.is_none() {
            continue; // already provisioned
        }
        apps_created.push(app.label.to_string());
    }

    if first_time {
        seed_procedures(
            &mut tx,
            user_id,
            workspace.id,
            business_type,
            &payload.goals,
        )
        .await?;
    }

    let credit_balance: i64 =
        sqlx::query_scalar("SELECT credit_balance FROM workspace_wallets WHERE workspace_id = $1")
            .bind(workspace.id)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;

    // Every channel gets a bot that answers (the Architect here, a sales,
    // studio and finance bot in the others).
    crate::office::ensure_starter_bots(&state.pool, user_id).await?;
    // The group chat, with the Architect and every bot of the new rooms.
    let office_chat =
        crate::office::group::ensure_office_chat(&state.pool, user_id, office_id).await?;

    // The team moves first: plans, buttons, and the first tasks started.
    if first_time {
        crate::office::kickoff::start(
            &state,
            user_id,
            office_chat,
            crate::office::kickoff::Profile {
                name: company_name.clone(),
                business_type: Some(business_type.to_string()),
                website: website.clone(),
                focus: focus.clone(),
            },
        )
        .await;
    }

    Ok(Json(OnboardResponse {
        workspace_id: office_id,
        workspace_name: company_name,
        channels,
        apps_created,
        credit_balance,
        status: "active".to_string(),
    }))
}

async fn seed_procedures(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    workspace_id: Uuid,
    business_type: &str,
    goals: &str,
) -> Result<(), AppError> {
    let suffix = || Uuid::new_v4().simple().to_string()[..6].to_string();
    let goals_line = match goals.trim() {
        "" => format!("2. Check the lead fits what {business_type} sells and who it serves.\n"),
        g => format!("2. Verify qualification against {business_type} business goals: {g}.\n"),
    };
    let proc_body = format!(
        "1. Ingest contact name, phone, and company from incoming record.\n\
         {goals_line}\
         3. Generate tailored outreach script emphasizing ROI.\n\
         4. Require human sign-off before sending outbound communication."
    );
    let proc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO procedures (
            user_id, workspace_id, slug, title, summary, body, check_description,
            visibility, kind, current_version, run_count, success_count
         )
         VALUES ($1, $2, $3, $4, $5, $6, 'Strict ICP conformance and verified phone number', 'private', 'procedure', 1, 0, 0)
         RETURNING id",
    )
    .bind(user_id)
    .bind(workspace_id)
    .bind(format!("lead-qualifier-{}", suffix()))
    .bind(format!("{business_type} Lead Outreach & ICP Classifier"))
    .bind(format!("Autonomous lead qualification and personalized pitch for {business_type}"))
    .bind(&proc_body)
    .fetch_one(&mut **tx)
    .await?;

    let wf_body = "Multi-stage autonomous pipeline from lead capture to invoice dispatch";
    let wf_id: Uuid = sqlx::query_scalar(
        "INSERT INTO procedures (
            user_id, workspace_id, slug, title, summary, body, check_description,
            visibility, kind, current_version, run_count, success_count
         )
         VALUES ($1, $2, $3, $4, $5, $6, 'All stages verified', 'private', 'workflow', 1, 0, 0)
         RETURNING id",
    )
    .bind(user_id)
    .bind(workspace_id)
    .bind(format!("pipeline-workflow-{}", suffix()))
    .bind(format!("{business_type} End-to-End Pipeline Workflow"))
    .bind(format!(
        "End-to-end multi-agent pipeline for {business_type}"
    ))
    .bind(wf_body)
    .fetch_one(&mut **tx)
    .await?;

    // Runs reference (procedure_id, version), so version 1 must exist.
    for (id, body) in [(proc_id, proc_body.as_str()), (wf_id, wf_body)] {
        sqlx::query(
            "INSERT INTO procedure_versions (procedure_id, version, body, changelog)
             VALUES ($1, 1, $2, 'Created by onboarding')",
        )
        .bind(id)
        .bind(body)
        .execute(&mut **tx)
        .await?;
    }

    let steps = [
        (
            1,
            "Capture & Triage Lead",
            "Parse inbound lead details and insert into CRM Kanban",
        ),
        (
            2,
            "Generate Customized Proposal",
            "Synthesize value proposition tailored to lead needs",
        ),
        (
            3,
            "Human Approval Gate",
            "Pause and wait for founder review in Approvals Inbox",
        ),
    ];
    for (order, title, instructions) in steps {
        sqlx::query(
            "INSERT INTO workflow_steps (procedure_id, version, step_order, title, instructions)
             VALUES ($1, 1, $2, $3, $4)",
        )
        .bind(wf_id)
        .bind(order)
        .bind(title)
        .bind(instructions)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

// ---------- Apps ----------

/// GET /api/office/apps
pub async fn list_office_apps(
    State(state): State<AppState>,
    caller: Caller,
    Query(office): Query<OfficeQuery>,
) -> Result<Json<Vec<OfficeApp>>, AppError> {
    let workspace_id = app_workspace(&state, &caller, office.office).await?;
    let apps = sqlx::query_as::<_, OfficeApp>(&format!(
        "SELECT {APP_COLUMNS} FROM office_apps
         WHERE workspace_id = $1 AND deleted_at IS NULL
         ORDER BY created_at ASC"
    ))
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(apps))
}

#[derive(Debug, Deserialize)]
pub struct CreateOfficeAppRequest {
    pub slug: String,
    pub title: String,
    pub icon: Option<String>,
    pub category: Option<String>,
    pub replaces_saas: Option<String>,
    pub ui_schema: serde_json::Value,
    pub idempotency_key: Option<String>,
}

/// POST /api/office/apps
pub async fn create_office_app(
    State(state): State<AppState>,
    caller: Caller,
    Query(office): Query<OfficeQuery>,
    headers: HeaderMap,
    Json(payload): Json<CreateOfficeAppRequest>,
) -> Result<Json<OfficeApp>, AppError> {
    let key = crate::idempotency::key(&headers, payload.idempotency_key.as_deref())?;
    if !valid_slug(&payload.slug) {
        return Err(AppError::BadRequest(
            "slug must be 1-64 lowercase letters, digits or dashes".into(),
        ));
    }
    let title = payload.title.trim();
    if title.is_empty() {
        return Err(AppError::BadRequest("title is required".into()));
    }
    check_len(title, "title", 120)?;
    validate_mod_schema(&payload.ui_schema)
        .map_err(|err| AppError::BadRequest(format!("Invalid Mod schema: {err}")))?;

    let workspace_id = app_workspace(&state, &caller, office.office).await?;
    if let Some(key) = key {
        if let Some(existing) = sqlx::query_as::<_, OfficeApp>(&format!(
            "SELECT {APP_COLUMNS} FROM office_apps
             WHERE workspace_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL"
        ))
        .bind(workspace_id)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?
        {
            return Ok(Json(existing));
        }
    }

    let created = sqlx::query_as::<_, OfficeApp>(&format!(
        "INSERT INTO office_apps (
            workspace_id, slug, title, icon, category, replaces_saas, ui_schema, default_schema, last_valid_schema, idempotency_key
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $7, $7, $8)
         ON CONFLICT DO NOTHING
         RETURNING {APP_COLUMNS}"
    ))
    .bind(workspace_id)
    .bind(&payload.slug)
    .bind(title)
    .bind(payload.icon.as_deref().unwrap_or("LayoutDashboard"))
    .bind(payload.category.as_deref().unwrap_or("crm"))
    .bind(payload.replaces_saas.as_deref())
    .bind(&payload.ui_schema)
    .bind(key)
    .fetch_optional(&state.pool)
    .await?;
    created
        .map(Json)
        .ok_or_else(|| AppError::Conflict("an app with this slug already exists".into()))
}

/// GET /api/office/apps/{slug}/records -- newest first, capped.
pub async fn get_app_records(
    State(state): State<AppState>,
    caller: Caller,
    Query(office): Query<OfficeQuery>,
    Path(slug): Path<String>,
) -> Result<Json<Vec<OfficeRecord>>, AppError> {
    let workspace_id = app_workspace(&state, &caller, office.office).await?;
    let app = find_app(&state, workspace_id, &slug).await?;
    let records = sqlx::query_as::<_, OfficeRecord>(&format!(
        "SELECT {RECORD_COLUMNS} FROM office_records
         WHERE app_id = $1 AND deleted_at IS NULL
         ORDER BY created_at DESC
         LIMIT $2"
    ))
    .bind(app.id)
    .bind(MAX_RECORDS)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(records))
}

#[derive(Debug, Deserialize)]
pub struct CreateRecordRequest {
    pub record_type: String,
    pub status: Option<String>,
    pub data: serde_json::Value,
    pub idempotency_key: Option<String>,
}

/// POST /api/office/apps/{slug}/records
pub async fn create_app_record(
    State(state): State<AppState>,
    caller: Caller,
    Query(office): Query<OfficeQuery>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Json(payload): Json<CreateRecordRequest>,
) -> Result<Json<OfficeRecord>, AppError> {
    let key = crate::idempotency::key(&headers, payload.idempotency_key.as_deref())?;
    let record_type = payload.record_type.trim();
    if record_type.is_empty() {
        return Err(AppError::BadRequest("record_type is required".into()));
    }
    check_len(record_type, "record_type", 64)?;
    let status = payload
        .status
        .as_deref()
        .unwrap_or("active")
        .trim()
        .to_string();
    check_len(&status, "status", 64)?;
    if !payload.data.is_object() {
        return Err(AppError::BadRequest("data must be a JSON object".into()));
    }

    let workspace_id = app_workspace(&state, &caller, office.office).await?;
    let app = find_app(&state, workspace_id, &slug).await?;
    let created_by_agent = match &caller {
        Caller::Agent(a) => Some(a.id),
        Caller::User(_) => None,
    };

    if let Some(key) = key {
        if let Some(existing) = sqlx::query_as::<_, OfficeRecord>(&format!(
            "SELECT {RECORD_COLUMNS} FROM office_records
             WHERE workspace_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL"
        ))
        .bind(workspace_id)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?
        {
            return Ok(Json(existing));
        }
    }

    let content_hash = calculate_content_hash(&payload.data);
    let duplicate: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM office_records
            WHERE app_id = $1 AND content_hash = $2 AND status <> 'archived' AND deleted_at IS NULL
         )",
    )
    .bind(app.id)
    .bind(content_hash)
    .fetch_one(&state.pool)
    .await?;
    if duplicate {
        return Err(AppError::Conflict(
            "duplicate record with identical content already exists".to_string(),
        ));
    }

    let record = sqlx::query_as::<_, OfficeRecord>(&format!(
        "INSERT INTO office_records (
            workspace_id, app_id, record_type, status, data, content_hash, created_by_agent_id, idempotency_key
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING {RECORD_COLUMNS}"
    ))
    .bind(workspace_id)
    .bind(app.id)
    .bind(record_type)
    .bind(&status)
    .bind(&payload.data)
    .bind(content_hash)
    .bind(created_by_agent)
    .bind(key)
    .fetch_one(&state.pool)
    .await?;
    // The person it names becomes (or joins) a contact.
    super::contacts::link_record(
        &state.pool,
        workspace_id,
        record.id,
        &record.record_type,
        &record.data,
        if created_by_agent.is_some() {
            "bot"
        } else {
            "owner"
        },
        created_by_agent,
    )
    .await;
    let record = sqlx::query_as::<_, OfficeRecord>(&format!(
        "SELECT {RECORD_COLUMNS} FROM office_records WHERE id = $1"
    ))
    .bind(record.id)
    .fetch_one(&state.pool)
    .await?;
    crate::office::records::emit_for(&state, record.id, "created").await;
    Ok(Json(record))
}

#[derive(Debug, Deserialize)]
pub struct PatchRecordRequest {
    pub status: Option<String>,
    pub data: Option<serde_json::Value>,
    pub idempotency_key: Option<String>,
}

/// PATCH /api/office/records/{id} -- idempotent by nature (sets values).
pub async fn patch_app_record(
    State(state): State<AppState>,
    caller: Caller,
    Query(office): Query<OfficeQuery>,
    headers: HeaderMap,
    Path(record_id): Path<Uuid>,
    Json(payload): Json<PatchRecordRequest>,
) -> Result<Json<OfficeRecord>, AppError> {
    crate::idempotency::key(&headers, payload.idempotency_key.as_deref())?;
    if let Some(status) = &payload.status {
        check_len(status, "status", 64)?;
    }
    if payload.data.as_ref().is_some_and(|d| !d.is_object()) {
        return Err(AppError::BadRequest("data must be a JSON object".into()));
    }
    let workspace_id = app_workspace(&state, &caller, office.office).await?;
    let new_hash = payload.data.as_ref().map(calculate_content_hash);

    let updated = sqlx::query_as::<_, OfficeRecord>(&format!(
        "UPDATE office_records
         SET status = COALESCE($1, status),
             data = COALESCE($2, data),
             content_hash = COALESCE($3, content_hash),
             updated_at = now()
         WHERE id = $4 AND workspace_id = $5 AND deleted_at IS NULL
         RETURNING {RECORD_COLUMNS}"
    ))
    .bind(payload.status.as_deref())
    .bind(payload.data.as_ref())
    .bind(new_hash)
    .bind(record_id)
    .bind(workspace_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    crate::office::records::emit_for(&state, updated.id, "updated").await;
    Ok(Json(updated))
}

/// Body of the archive and delete routes: only an idempotency key.
#[derive(Debug, Default, Deserialize)]
pub struct IdempotentBody {
    pub idempotency_key: Option<String>,
}

/// Sets or clears an app's `archived_at`. An archived app is hidden from
/// the office and its bots (they stop filling it) until it is restored.
async fn set_app_archived(
    state: &AppState,
    caller: &Caller,
    office: Option<Uuid>,
    headers: &HeaderMap,
    slug: &str,
    body: Option<IdempotentBody>,
    archived: bool,
) -> Result<Json<OfficeApp>, AppError> {
    let body = body.unwrap_or_default();
    crate::idempotency::key(headers, body.idempotency_key.as_deref())?;
    let workspace_id = app_workspace(state, caller, office).await?;
    let app = find_app(state, workspace_id, slug).await?;
    let updated = sqlx::query_as::<_, OfficeApp>(&format!(
        "UPDATE office_apps
         SET archived_at = CASE WHEN $2 THEN COALESCE(archived_at, now()) ELSE NULL END,
             updated_at = now()
         WHERE id = $1
         RETURNING {APP_COLUMNS}"
    ))
    .bind(app.id)
    .bind(archived)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(updated))
}

/// POST /api/office/apps/{slug}/archive
pub async fn archive_office_app(
    State(state): State<AppState>,
    caller: Caller,
    Query(office): Query<OfficeQuery>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    body: Option<Json<IdempotentBody>>,
) -> Result<Json<OfficeApp>, AppError> {
    let body = body.map(|Json(b)| b);
    let app = set_app_archived(&state, &caller, office.office, &headers, &slug, body, true).await?;
    crate::office::records::emit(&state, app.workspace_id, Uuid::nil(), &app.slug, "app");
    Ok(app)
}

/// POST /api/office/apps/{slug}/unarchive
pub async fn unarchive_office_app(
    State(state): State<AppState>,
    caller: Caller,
    Query(office): Query<OfficeQuery>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    body: Option<Json<IdempotentBody>>,
) -> Result<Json<OfficeApp>, AppError> {
    let body = body.map(|Json(b)| b);
    let app =
        set_app_archived(&state, &caller, office.office, &headers, &slug, body, false).await?;
    crate::office::records::emit(&state, app.workspace_id, Uuid::nil(), &app.slug, "app");
    Ok(app)
}

/// POST /api/office/apps/{slug}/delete -- soft-deletes the app and its
/// records. Nothing is erased; the slug can be installed again afterwards.
pub async fn delete_office_app(
    State(state): State<AppState>,
    caller: Caller,
    Query(office): Query<OfficeQuery>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    body: Option<Json<IdempotentBody>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let workspace_id = app_workspace(&state, &caller, office.office).await?;
    let app = find_app(&state, workspace_id, &slug).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "UPDATE office_records SET deleted_at = now(), updated_at = now()
         WHERE app_id = $1 AND deleted_at IS NULL",
    )
    .bind(app.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE office_apps SET deleted_at = now(), updated_at = now() WHERE id = $1")
        .bind(app.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    crate::office::records::emit(&state, workspace_id, Uuid::nil(), &app.slug, "app");
    Ok(Json(
        json!({ "status": "deleted", "id": app.id, "slug": app.slug }),
    ))
}

/// POST /api/office/records/{id}/delete -- soft-deletes one record.
pub async fn delete_app_record(
    State(state): State<AppState>,
    caller: Caller,
    Query(office): Query<OfficeQuery>,
    headers: HeaderMap,
    Path(record_id): Path<Uuid>,
    body: Option<Json<IdempotentBody>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let workspace_id = app_workspace(&state, &caller, office.office).await?;
    let deleted: Option<Uuid> = sqlx::query_scalar(
        "UPDATE office_records SET deleted_at = now(), updated_at = now()
         WHERE id = $1 AND workspace_id = $2 AND deleted_at IS NULL
         RETURNING id",
    )
    .bind(record_id)
    .bind(workspace_id)
    .fetch_optional(&state.pool)
    .await?;
    let id = deleted.ok_or(AppError::NotFound)?;
    crate::office::records::emit_for(&state, id, "deleted").await;
    Ok(Json(json!({ "status": "deleted", "id": id })))
}

/// GET /api/office/wallet -- the primary workspace's wallet, provisioned
/// with the plan default the first time it is read.
pub async fn get_workspace_wallet(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<WorkspaceWallet>, AppError> {
    let workspace_id = primary_workspace_id(&state, &caller).await?;
    sqlx::query(
        "INSERT INTO workspace_wallets (workspace_id) VALUES ($1)
         ON CONFLICT (workspace_id) DO NOTHING",
    )
    .bind(workspace_id)
    .execute(&state.pool)
    .await?;
    let mut wallet = sqlx::query_as::<_, WorkspaceWallet>(WALLET_SELECT)
        .bind(workspace_id)
        .fetch_one(&state.pool)
        .await?;
    wallet.limit_reached = crate::office::budget_gate::next_call_limit(
        wallet.credit_balance,
        wallet.credits_burned_today,
        wallet.max_daily_credit_burn,
    );
    wallet.upgrade_url = state.config.upgrade_url.clone();
    wallet.web_runs_today = crate::web::runs_today(&state.pool, workspace_id).await?;
    wallet.web_runs_limit = if state.config.web.enabled {
        state.config.web.runs_per_day
    } else {
        0
    };
    Ok(Json(wallet))
}

/// Validates declarative UI schema for Herdr-style Mods.
/// Rejects illegal structures before they can reach the database.
pub fn validate_mod_schema(schema: &serde_json::Value) -> Result<(), String> {
    let obj = schema.as_object().ok_or("Schema must be a JSON object")?;

    let views = obj
        .get("views")
        .and_then(|v| v.as_array())
        .ok_or("Schema must contain a 'views' array")?;

    if views.is_empty() {
        return Err("'views' array cannot be empty".to_string());
    }
    if views.len() > 20 {
        return Err("at most 20 views are allowed".to_string());
    }

    for (idx, view) in views.iter().enumerate() {
        let v_obj = view
            .as_object()
            .ok_or_else(|| format!("View at index {idx} must be an object"))?;

        v_obj
            .get("id")
            .and_then(|s| s.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| format!("View at index {idx} requires a non-empty 'id' string"))?;

        let v_type = v_obj
            .get("type")
            .and_then(|s| s.as_str())
            .ok_or_else(|| format!("View at index {idx} requires a 'type' string"))?;

        let valid_types = ["kanban", "datatable", "media_gallery", "chart"];
        if !valid_types.contains(&v_type) {
            return Err(format!(
                "View at index {idx} has invalid type '{v_type}'. Allowed: {}",
                valid_types.join(", ")
            ));
        }

        v_obj
            .get("record_type")
            .and_then(|s| s.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                format!("View at index {idx} requires a non-empty 'record_type' string")
            })?;

        if v_type == "chart" {
            if let Some(kind) = v_obj.get("chart_type") {
                if !matches!(kind.as_str(), Some("bar" | "spline")) {
                    return Err(format!(
                        "Chart view at index {idx} has 'chart_type' other than 'bar' or 'spline'"
                    ));
                }
            }
            for key in ["x_axis", "y_axis"] {
                if let Some(field) = v_obj.get(key) {
                    if !field
                        .as_str()
                        .is_some_and(|f| !f.trim().is_empty() && f.len() <= 64)
                    {
                        return Err(format!(
                            "Chart view at index {idx} needs '{key}' to be a field name"
                        ));
                    }
                }
            }
        }

        if v_type == "kanban" || v_type == "datatable" {
            let label = if v_type == "kanban" {
                "Kanban"
            } else {
                "Datatable"
            };
            let cols = v_obj
                .get("columns")
                .and_then(|c| c.as_array())
                .ok_or_else(|| format!("{label} view at index {idx} requires a 'columns' array"))?;
            if cols.is_empty() {
                return Err(format!(
                    "{label} view at index {idx} requires at least one column"
                ));
            }
        }
    }

    if let Some(metrics) = obj.get("header_metrics") {
        if !metrics.is_array() {
            return Err("'header_metrics' must be an array".to_string());
        }
    }

    // Stages a bot may only move a record into with the owner's OK.
    if let Some(stages) = obj.get("approval_stages") {
        let ok = stages.as_array().is_some_and(|s| {
            s.len() <= 10
                && s.iter().all(|v| {
                    v.as_str()
                        .is_some_and(|t| !t.trim().is_empty() && t.len() <= 64)
                })
        });
        if !ok {
            return Err("'approval_stages' must be a list of at most 10 stage names".to_string());
        }
    }

    // Conversions: a record of this app becomes one in another app (a
    // quote into an invoice), mapping fields by name.
    if let Some(rules) = obj.get("convert") {
        let rules = rules
            .as_array()
            .ok_or_else(|| "'convert' must be an array".to_string())?;
        if rules.len() > 4 {
            return Err("at most 4 conversions are allowed".to_string());
        }
        for (idx, rule) in rules.iter().enumerate() {
            let text_ok = |key: &str, max: usize| {
                rule.get(key)
                    .and_then(|v| v.as_str())
                    .is_some_and(|t| !t.trim().is_empty() && t.chars().count() <= max)
            };
            if !text_ok("label", 40) || !text_ok("to_app", 64) {
                return Err(format!(
                    "Conversion at index {idx} needs a 'label' and a 'to_app'"
                ));
            }
            if rule.get("when_stage").is_some() && !text_ok("when_stage", 64) {
                return Err(format!(
                    "Conversion at index {idx} has an invalid 'when_stage'"
                ));
            }
            let map_ok = rule.get("map").is_none_or(|m| {
                m.as_object().is_some_and(|m| {
                    m.len() <= 20
                        && m.values()
                            .all(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                })
            });
            let set_ok = rule.get("set").is_none_or(|s| {
                s.as_object().is_some_and(|s| {
                    s.len() <= 10
                        && s.values()
                            .all(|v| v.is_string() || v.is_number() || v.is_boolean())
                })
            });
            if !map_ok || !set_ok {
                return Err(format!(
                    "Conversion at index {idx} needs 'map' as field names and 'set' as plain values"
                ));
            }
        }
    }

    // Quick actions: each asks the office's team for one job (`ask`, or
    // the label when absent) in the office chat.
    if let Some(actions) = obj.get("quick_actions") {
        let actions = actions
            .as_array()
            .ok_or_else(|| "'quick_actions' must be an array".to_string())?;
        if actions.len() > 6 {
            return Err("at most 6 quick actions are allowed".to_string());
        }
        for (idx, action) in actions.iter().enumerate() {
            let label_ok = action
                .get("label")
                .and_then(|l| l.as_str())
                .map(str::trim)
                .is_some_and(|l| !l.is_empty() && l.chars().count() <= 60);
            if !label_ok {
                return Err(format!(
                    "Quick action at index {idx} needs a 'label' of 1-60 characters"
                ));
            }
            if let Some(ask) = action.get("ask") {
                if !ask
                    .as_str()
                    .is_some_and(|a| !a.trim().is_empty() && a.chars().count() <= 500)
                {
                    return Err(format!(
                        "Quick action at index {idx} has an 'ask' that is not 1-500 characters"
                    ));
                }
            }
        }
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct UpdateModSchemaRequest {
    pub ui_schema: serde_json::Value,
    pub idempotency_key: Option<String>,
}

/// PUT /api/office/apps/{slug}/schema -- validated before it is stored;
/// the previous valid schema is kept as the rollback target.
pub async fn update_mod_schema(
    State(state): State<AppState>,
    caller: Caller,
    Query(office): Query<OfficeQuery>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Json(payload): Json<UpdateModSchemaRequest>,
) -> Result<Json<OfficeApp>, AppError> {
    crate::idempotency::key(&headers, payload.idempotency_key.as_deref())?;
    validate_mod_schema(&payload.ui_schema)
        .map_err(|err| AppError::BadRequest(format!("Invalid Mod schema: {err}")))?;
    let workspace_id = app_workspace(&state, &caller, office.office).await?;
    let app = find_app(&state, workspace_id, &slug).await?;
    set_schema(&state, app.id, &payload.ui_schema).await
}

/// POST /api/office/apps/{slug}/reset -- back to the default blueprint.
pub async fn reset_mod_schema(
    State(state): State<AppState>,
    caller: Caller,
    Query(office): Query<OfficeQuery>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<Json<OfficeApp>, AppError> {
    crate::idempotency::key(&headers, None)?;
    let workspace_id = app_workspace(&state, &caller, office.office).await?;
    let app = find_app(&state, workspace_id, &slug).await?;
    let schema = app.default_schema.clone().unwrap_or(app.ui_schema.clone());
    set_schema(&state, app.id, &schema).await
}

async fn set_schema(
    state: &AppState,
    app_id: Uuid,
    schema: &serde_json::Value,
) -> Result<Json<OfficeApp>, AppError> {
    let updated = sqlx::query_as::<_, OfficeApp>(&format!(
        "UPDATE office_apps
         SET ui_schema = $1, last_valid_schema = $1, updated_at = now()
         WHERE id = $2
         RETURNING {APP_COLUMNS}"
    ))
    .bind(schema)
    .bind(app_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(updated))
}

// ---------- Workspace lessons ----------

/// GET /api/workspace/lessons
pub async fn list_workspace_lessons(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Vec<WorkspaceLesson>>, AppError> {
    let workspace_id = primary_workspace_id(&state, &caller).await?;
    let lessons = sqlx::query_as::<_, WorkspaceLesson>(
        "SELECT id, workspace_id, rule, source, context, created_at, updated_at, deleted_at
         FROM workspace_lessons
         WHERE workspace_id = $1 AND deleted_at IS NULL
         ORDER BY created_at DESC
         LIMIT 200",
    )
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(lessons))
}

#[derive(Debug, Deserialize)]
pub struct CreateLessonRequest {
    pub rule: String,
    pub source: Option<String>,
    pub context: Option<serde_json::Value>,
    pub idempotency_key: Option<String>,
}

/// POST /api/workspace/lessons
pub async fn create_workspace_lesson(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Json(payload): Json<CreateLessonRequest>,
) -> Result<Json<WorkspaceLesson>, AppError> {
    let key = crate::idempotency::key(&headers, payload.idempotency_key.as_deref())?;
    let rule = payload.rule.trim();
    if rule.is_empty() {
        return Err(AppError::BadRequest("Rule text cannot be empty".into()));
    }
    check_len(rule, "rule", 2000)?;
    let source = match payload.source.as_deref().unwrap_or("manual") {
        s @ ("manual" | "user_correction" | "approval_rejection") => s,
        _ => {
            return Err(AppError::BadRequest(
                "source must be manual, user_correction or approval_rejection".into(),
            ))
        }
    };
    let workspace_id = primary_workspace_id(&state, &caller).await?;

    let inserted = sqlx::query_as::<_, WorkspaceLesson>(
        "INSERT INTO workspace_lessons (workspace_id, rule, source, context, idempotency_key)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (workspace_id, idempotency_key) WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL
         DO NOTHING
         RETURNING id, workspace_id, rule, source, context, created_at, updated_at, deleted_at",
    )
    .bind(workspace_id)
    .bind(rule)
    .bind(source)
    .bind(payload.context.unwrap_or_else(|| json!({})))
    .bind(key)
    .fetch_optional(&state.pool)
    .await?;
    if let Some(lesson) = inserted {
        return Ok(Json(lesson));
    }
    // Replay of an idempotency key: return the original lesson.
    let existing = sqlx::query_as::<_, WorkspaceLesson>(
        "SELECT id, workspace_id, rule, source, context, created_at, updated_at, deleted_at
         FROM workspace_lessons
         WHERE workspace_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .bind(key)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(existing))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn every_blueprint_is_a_valid_schema_with_a_chart_and_actions() {
        let all: Vec<String> = ["crm", "content", "invoice", "support", "booking"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let apps = blueprints(&all, false);
        assert_eq!(apps.len(), 5);
        for app in &apps {
            validate_mod_schema(&app.schema).unwrap_or_else(|e| panic!("{}: {e}", app.slug));
            let actions = app.schema["quick_actions"].as_array().expect("actions");
            assert!(!actions.is_empty(), "{} has a quick action", app.slug);
            assert!(actions.iter().all(|a| a["ask"].is_string()), "{}", app.slug);
        }
        let charts = apps
            .iter()
            .filter(|a| {
                a.schema["views"]
                    .as_array()
                    .is_some_and(|v| v.iter().any(|v| v["type"] == "chart"))
            })
            .count();
        assert_eq!(charts, 4, "every app but the media studio has a chart");
    }

    #[test]
    fn the_new_apps_are_valid_and_carry_their_rules() {
        let all: Vec<String> = ["projects", "quotes", "reviews"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let apps = blueprints(&all, false);
        let slugs: Vec<&str> = apps.iter().map(|a| a.slug).collect();
        assert_eq!(
            slugs,
            ["projects-tasks", "quotes-proposals", "reviews-reputation"]
        );
        for app in &apps {
            validate_mod_schema(&app.schema).unwrap_or_else(|e| panic!("{}: {e}", app.slug));
        }
        let quotes = &apps[1].schema;
        assert_eq!(quotes["approval_stages"][0], "Sent");
        assert_eq!(quotes["convert"][0]["to_app"], "invoices-cashflow");
        assert_eq!(quotes["convert"][1]["to_app"], "projects-tasks");
        assert_eq!(
            apps[2].schema["approval_stages"],
            json!(["Requested", "Replied"])
        );
    }

    #[test]
    fn conversions_and_approval_stages_are_checked() {
        let base = |extra: (&str, Value)| {
            let mut s = json!({ "version": "1.0", "layout": "split_view",
                "views": [{ "id": "k", "type": "kanban", "title": "Q", "record_type": "quote",
                            "columns": ["Draft", "Sent"], "card_fields": ["title"] }] });
            s[extra.0] = extra.1;
            s
        };
        let ok = json!([{ "label": "To invoice", "to_app": "invoices-cashflow", "when_stage": "Sent",
                          "map": { "amount": "total" }, "set": { "status": "Draft" } }]);
        assert!(validate_mod_schema(&base(("convert", ok))).is_ok());
        for bad in [
            json!({}),
            json!([{ "label": "x" }]),
            json!([{ "label": "x", "to_app": "y", "map": { "a": 1 } }]),
            json!([{ "label": "x", "to_app": "y", "set": { "a": { "deep": 1 } } }]),
            json!([{ "label": "x", "to_app": "y", "when_stage": "" }]),
        ] {
            assert!(
                validate_mod_schema(&base(("convert", bad.clone()))).is_err(),
                "{bad}"
            );
        }
        assert!(validate_mod_schema(&base(("approval_stages", json!(["Sent"])))).is_ok());
        assert!(validate_mod_schema(&base(("approval_stages", json!("Sent")))).is_err());
        assert!(validate_mod_schema(&base(("approval_stages", json!([""])))).is_err());
    }

    #[test]
    fn charts_and_quick_actions_are_checked() {
        let with = |view: Value, actions: Value| json!({ "version": "1.0", "layout": "split_view", "views": [view], "quick_actions": actions });
        let chart = json!({ "id": "c", "type": "chart", "title": "T", "record_type": "lead", "chart_type": "bar", "x_axis": "stage" });
        assert!(validate_mod_schema(&with(chart.clone(), json!([]))).is_ok());
        let mut pie = chart.clone();
        pie["chart_type"] = json!("pie");
        assert!(validate_mod_schema(&with(pie, json!([]))).is_err());
        let mut bad_axis = chart.clone();
        bad_axis["x_axis"] = json!(3);
        assert!(validate_mod_schema(&with(bad_axis, json!([]))).is_err());
        assert!(validate_mod_schema(&with(chart.clone(), json!([{ "label": "" }]))).is_err());
        assert!(
            validate_mod_schema(&with(chart.clone(), json!([{ "label": "Go", "ask": "" }])))
                .is_err()
        );
        assert!(validate_mod_schema(&with(chart.clone(), json!({}))).is_err());
        let seven: Vec<Value> = (0..7)
            .map(|i| json!({ "label": format!("A{i}") }))
            .collect();
        assert!(validate_mod_schema(&with(chart, json!(seven))).is_err());
    }

    #[test]
    fn content_hash_is_stable_fnv1a() {
        // FNV-1a of "{}" is fixed; a change here would orphan stored hashes.
        assert_eq!(
            calculate_content_hash(&json!({})),
            0x08f4_4b07_b590_1a25_u64 as i64
        );
        assert_ne!(
            calculate_content_hash(&json!({"a": 1})),
            calculate_content_hash(&json!({"a": 2}))
        );
    }

    #[test]
    fn slugs_are_restricted() {
        assert!(valid_slug("leads-pipeline"));
        assert!(!valid_slug("Leads"));
        assert!(!valid_slug(""));
        assert!(!valid_slug("a/b"));
    }
}

#[derive(Debug, Deserialize)]
pub struct InstallAppRequest {
    /// `crm`, `social`, `finance`, `support`, `scheduling`, `projects`,
    /// `quotes` or `reviews`.
    pub kind: String,
    pub idempotency_key: Option<String>,
}

const INSTALLABLE: [&str; 8] = [
    "crm",
    "social",
    "finance",
    "support",
    "scheduling",
    "projects",
    "quotes",
    "reviews",
];

/// POST /api/office/apps/install?office= -- adds one ready-made app to an
/// office. Unlike office setup it renames nothing, makes no rooms or bots
/// and starts no work; installing an app the office has is a no-op.
pub async fn install_office_app(
    State(state): State<AppState>,
    caller: Caller,
    Query(office): Query<OfficeQuery>,
    headers: HeaderMap,
    Json(payload): Json<InstallAppRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::idempotency::key(&headers, payload.idempotency_key.as_deref())?;
    let kind = payload.kind.trim().to_ascii_lowercase();
    if !INSTALLABLE.contains(&kind.as_str()) {
        return Err(AppError::BadRequest(
            "kind must be one of crm, social, finance, support, scheduling, projects, quotes, reviews"
                .into(),
        ));
    }
    let workspace_id = app_workspace(&state, &caller, office.office).await?;
    let mut created = Vec::new();
    let mut slug = None;
    for app in blueprints(&[kind], false) {
        slug.get_or_insert(app.slug);
        let inserted: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO office_apps (
                workspace_id, slug, title, icon, category, replaces_saas, ui_schema, default_schema, last_valid_schema
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $7, $7)
             ON CONFLICT (workspace_id, slug) WHERE deleted_at IS NULL DO NOTHING
             RETURNING id",
        )
        .bind(workspace_id)
        .bind(app.slug)
        .bind(app.title)
        .bind(app.icon)
        .bind(app.category)
        .bind(app.replaces)
        .bind(&app.schema)
        .fetch_optional(&state.pool)
        .await?;
        if inserted.is_some() {
            created.push(app.title.to_string());
        }
    }
    Ok(Json(json!({ "slug": slug, "apps_created": created })))
}
