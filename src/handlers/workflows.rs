use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{Caller, OptionalCaller};
use crate::error::AppError;
use crate::models::{Board, Note, Procedure, Run, RunOutcome, RunStep, Visibility, WorkflowStep};
use crate::state::AppState;

use super::{render, wants_html, JsonOrForm};

// ---------- Workflow Step Inputs and Types ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStepInput {
    #[serde(default)]
    pub step_order: Option<i32>,
    pub title: String,
    pub instructions: String,
    #[serde(default)]
    pub agent_id: Option<Uuid>,
    #[serde(default)]
    pub agent_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkflowRequest {
    pub title: String,
    pub summary: String,
    pub body: String,
    pub check_description: String,
    #[serde(default)]
    pub board: Option<String>,
    #[serde(default)]
    pub board_name: Option<String>,
    #[serde(default)]
    pub board_id: Option<Uuid>,
    #[serde(default = "default_private_visibility")]
    pub visibility: Visibility,
    #[serde(default)]
    pub steps: Vec<WorkflowStepInput>,
    #[serde(default)]
    pub step1_title: Option<String>,
    #[serde(default)]
    pub step1_instructions: Option<String>,
    #[serde(default)]
    pub step1_agent_id: Option<Uuid>,
    #[serde(default)]
    pub step2_title: Option<String>,
    #[serde(default)]
    pub step2_instructions: Option<String>,
    #[serde(default)]
    pub step2_agent_id: Option<Uuid>,
    #[serde(default)]
    pub step3_title: Option<String>,
    #[serde(default)]
    pub step3_instructions: Option<String>,
    #[serde(default)]
    pub step3_agent_id: Option<Uuid>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

fn default_private_visibility() -> Visibility {
    Visibility::Private
}

#[derive(Debug, Deserialize, Default)]
pub struct UpdateWorkflowRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub check_description: Option<String>,
    #[serde(default)]
    pub visibility: Option<Visibility>,
    #[serde(default)]
    pub steps: Option<Vec<WorkflowStepInput>>,
    #[serde(default)]
    pub changelog: Option<String>,
    #[serde(default)]
    pub step1_title: Option<String>,
    #[serde(default)]
    pub step1_instructions: Option<String>,
    #[serde(default)]
    pub step1_agent_id: Option<Uuid>,
    #[serde(default)]
    pub step2_title: Option<String>,
    #[serde(default)]
    pub step2_instructions: Option<String>,
    #[serde(default)]
    pub step2_agent_id: Option<Uuid>,
    #[serde(default)]
    pub step3_title: Option<String>,
    #[serde(default)]
    pub step3_instructions: Option<String>,
    #[serde(default)]
    pub step3_agent_id: Option<Uuid>,
    #[serde(default)]
    pub step4_title: Option<String>,
    #[serde(default)]
    pub step4_instructions: Option<String>,
    #[serde(default)]
    pub step4_agent_id: Option<Uuid>,
    #[serde(default)]
    pub step5_title: Option<String>,
    #[serde(default)]
    pub step5_instructions: Option<String>,
    #[serde(default)]
    pub step5_agent_id: Option<Uuid>,
    #[serde(default)]
    pub step6_title: Option<String>,
    #[serde(default)]
    pub step6_instructions: Option<String>,
    #[serde(default)]
    pub step6_agent_id: Option<Uuid>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct AssignStepRequest {
    #[serde(default)]
    pub agent_id: Option<Uuid>,
    #[serde(default)]
    pub agent_name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct StartRunRequest {
    #[serde(default)]
    pub procedure_version: Option<i32>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub q1: Option<String>,
    #[serde(default)]
    pub q2: Option<String>,
    #[serde(default)]
    pub q3: Option<String>,
    #[serde(default)]
    pub custom_input: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionnaireQuestion {
    pub title: String,
    pub name: String,
    pub description: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowQuestionnaire {
    pub title: String,
    pub description: String,
    pub questions: Vec<QuestionnaireQuestion>,
    pub custom_input_prompt: String,
}

pub fn get_workflow_questionnaire(title: &str, board_name: Option<&str>) -> WorkflowQuestionnaire {
    let lower = format!(
        "{} {}",
        title.to_lowercase(),
        board_name.unwrap_or("").to_lowercase()
    );

    if lower.contains("lead")
        || lower.contains("prospect")
        || lower.contains("outreach")
        || lower.contains("sales")
    {
        WorkflowQuestionnaire {
            title: "B2B Lead Generation & Outreach Questionnaire".to_string(),
            description: "Configure ICP, target personas, and value proposition before launching the lead prospecting workflow.".to_string(),
            questions: vec![
                QuestionnaireQuestion {
                    title: "Target Industry / ICP".to_string(),
                    name: "q1".to_string(),
                    description: "Select target vertical for company discovery and qualification:".to_string(),
                    options: vec![
                        "B2B SaaS (Series A-C)".to_string(),
                        "FinTech & Payment Infrastructure".to_string(),
                        "AI & Developer Tooling".to_string(),
                        "HealthTech & BioTech".to_string(),
                    ],
                },
                QuestionnaireQuestion {
                    title: "Target Persona & Decision Maker".to_string(),
                    name: "q2".to_string(),
                    description: "Key stakeholder to research and address in outreach messaging:".to_string(),
                    options: vec![
                        "VP of Engineering / CTO".to_string(),
                        "Head of Product / CPO".to_string(),
                        "Founder / CEO".to_string(),
                        "Head of Growth / Marketing".to_string(),
                    ],
                },
                QuestionnaireQuestion {
                    title: "Outreach Angle & Tone".to_string(),
                    name: "q3".to_string(),
                    description: "Communication framing for drafted cold outreach:".to_string(),
                    options: vec![
                        "Value-first (2-sentence problem hook)".to_string(),
                        "Direct ROI & Case Study Evidence".to_string(),
                        "LinkedIn / Warm Peer Curiosity".to_string(),
                        "Partnership & Co-marketing Hook".to_string(),
                    ],
                },
            ],
            custom_input_prompt: "Enter company criteria, geography, employee count range, or specific value proposition...".to_string(),
        }
    } else if lower.contains("startup") || lower.contains("idea") || lower.contains("validation") {
        WorkflowQuestionnaire {
            title: "Startup Idea Validation Questionnaire".to_string(),
            description: "Select target industry, core hypothesis, and falsification test criteria before running the research workflow.".to_string(),
            questions: vec![
                QuestionnaireQuestion {
                    title: "Problem Space & Market".to_string(),
                    name: "q1".to_string(),
                    description: "Select market sector to investigate:".to_string(),
                    options: vec![
                        "Autonomous AI Agents & Workflows".to_string(),
                        "Developer Tooling & Cloud Infra".to_string(),
                        "Vertical SMB Workflow Automation".to_string(),
                        "Consumer FinTech & Wealth Management".to_string(),
                    ],
                },
                QuestionnaireQuestion {
                    title: "Primary Validation Hypothesis".to_string(),
                    name: "q2".to_string(),
                    description: "Critical risk or hypothesis to test against market reality:".to_string(),
                    options: vec![
                        "High Willingness-to-Pay (B2B SaaS)".to_string(),
                        "Defensible Moat Against Big Tech Incumbents".to_string(),
                        "Falsification & Critical Kill-Switch Test".to_string(),
                        "Viral Growth Loop & Low CAC".to_string(),
                    ],
                },
                QuestionnaireQuestion {
                    title: "Target Customer Profile".to_string(),
                    name: "q3".to_string(),
                    description: "Intended buyer profile for early customer discovery:".to_string(),
                    options: vec![
                        "Solo Founders & Indie Developers".to_string(),
                        "SMB Operators & Small Agencies".to_string(),
                        "Mid-Market Engineering Teams".to_string(),
                        "Enterprise Decision Makers".to_string(),
                    ],
                },
            ],
            custom_input_prompt: "Enter core idea description, target user assumptions, or key competitors to analyze...".to_string(),
        }
    } else {
        WorkflowQuestionnaire {
            title: "Workflow Run Parameters".to_string(),
            description: "Configure run parameters and initial context for the assigned agents."
                .to_string(),
            questions: vec![
                QuestionnaireQuestion {
                    title: "Execution Depth".to_string(),
                    name: "q1".to_string(),
                    description: "Select thoroughness level for the workflow run:".to_string(),
                    options: vec![
                        "Standard Full Execution".to_string(),
                        "Preliminary Quick Audit".to_string(),
                        "Strict Validation with Kill Switches".to_string(),
                    ],
                },
                QuestionnaireQuestion {
                    title: "Output Format".to_string(),
                    name: "q2".to_string(),
                    description: "Desired reporting format for completed steps:".to_string(),
                    options: vec![
                        "Actionable Checklist & Summary".to_string(),
                        "Detailed Structured Evidence".to_string(),
                        "Technical Deep Dive with Code / Data".to_string(),
                    ],
                },
                QuestionnaireQuestion {
                    title: "Priority Focus".to_string(),
                    name: "q3".to_string(),
                    description: "Primary optimization objective for agent results:".to_string(),
                    options: vec![
                        "High Precision & Accuracy".to_string(),
                        "Speed & Fast Turnaround".to_string(),
                        "Comprehensive Edge Case Coverage".to_string(),
                    ],
                },
            ],
            custom_input_prompt:
                "Enter custom parameters, specific constraints, or background context..."
                    .to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct StepResultRequest {
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct WorkflowsQuery {
    #[serde(default)]
    pub board: Option<String>,
}

// ---------- Display Structs ----------

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowItem {
    pub id: Uuid,
    pub user_id: Uuid,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub body: String,
    pub check_description: String,
    pub visibility: String,
    pub current_version: i32,
    pub run_count: i64,
    pub success_count: i64,
    pub board_id: Option<Uuid>,
    pub board_name: Option<String>,
    pub step_count: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowStepItem {
    pub id: Uuid,
    pub step_order: i32,
    pub title: String,
    pub instructions: String,
    pub agent_id: Option<Uuid>,
    pub agent_name: Option<String>,
    pub agent_quiet: bool,
}

impl WorkflowStepItem {
    pub fn is_assigned_to(&self, agent_id: &Uuid) -> bool {
        self.agent_id.as_ref() == Some(agent_id)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RunSummaryItem {
    pub id: Uuid,
    pub procedure_id: Uuid,
    pub procedure_version: i32,
    pub status: String,
    pub outcome: Option<String>,
    pub current_step_order: i32,
    pub total_steps: i64,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub thread_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunStepDetailItem {
    pub id: Uuid,
    pub step_order: i32,
    pub title: String,
    pub instructions: String,
    pub agent_id: Option<Uuid>,
    pub agent_name: Option<String>,
    pub agent_quiet: bool,
    pub is_openrouter: bool,
    pub status: String,
    pub input: Option<String>,
    pub output: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunDetailItem {
    pub id: Uuid,
    pub procedure_id: Uuid,
    pub workflow_title: String,
    pub workflow_check: String,
    pub procedure_version: i32,
    pub status: String,
    pub outcome: Option<String>,
    pub current_step_order: i32,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub thread_id: Option<Uuid>,
    pub custom_input: Option<String>,
    pub steps: Vec<RunStepDetailItem>,
    pub is_owner: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActiveAgentItem {
    pub id: Uuid,
    pub name: String,
    pub handle: Option<String>,
    /// The bot's character (what it is told to be).
    pub instructions: Option<String>,
    /// Desktop app: the tool and model on this computer (None = default).
    pub ai_engine: Option<String>,
    pub ai_model: Option<String>,
    /// Website: the OpenRouter model it answers with.
    pub provider_model: Option<String>,
    /// Where its answers come from: `this_computer` (desktop app),
    /// `own_key` (its own OpenRouter key), `account_key` (the owner's
    /// OpenRouter key), `included` (the platform's AI and credits) or
    /// `none` (nothing can answer yet).
    pub ai_source: String,
    /// The model it really answers with on the website (None on the
    /// desktop app, where the tool on the computer decides).
    pub effective_model: Option<String>,
    pub key_prefix: String,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub status: String,
    pub quiet: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingAgentStepItem {
    pub run_id: Uuid,
    pub step_id: Option<Uuid>,
    pub step_order: i32,
    pub title: String,
    pub instructions: String,
    pub input: Option<String>,
    pub status: String,
    pub workflow_title: String,
    pub thread_id: Option<Uuid>,
}

// ---------- Helper Functions ----------

fn slugify(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut last_was_hyphen = true;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_hyphen = false;
        } else if !last_was_hyphen {
            slug.push('-');
            last_was_hyphen = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug.push_str("workflow");
    }
    slug.truncate(80);
    slug
}

pub async fn resolve_agent_id(
    pool: &PgPool,
    user_id: Uuid,
    agent_id: Option<Uuid>,
    agent_name: Option<&str>,
) -> Result<Option<Uuid>, AppError> {
    if let Some(id) = agent_id {
        return Ok(Some(id));
    }
    if let Some(name) = agent_name {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            let found = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM agents WHERE user_id = $1 AND name = $2 AND revoked_at IS NULL AND deleted_at IS NULL",
            )
            .bind(user_id)
            .bind(trimmed)
            .fetch_optional(pool)
            .await?;
            return Ok(found);
        }
    }
    Ok(None)
}

pub async fn fetch_workflow_steps(
    pool: &PgPool,
    procedure_id: Uuid,
    version: i32,
) -> Result<Vec<WorkflowStepItem>, AppError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        step_order: i32,
        title: String,
        instructions: String,
        agent_id: Option<Uuid>,
        agent_name: Option<String>,
        last_seen_at: Option<DateTime<Utc>>,
    }
    let rows = sqlx::query_as::<_, Row>(
        "SELECT ws.id, ws.step_order, ws.title, ws.instructions,
                ws.agent_id, a.name AS agent_name, a.last_seen_at
         FROM workflow_steps ws
         LEFT JOIN agents a ON a.id = ws.agent_id AND a.deleted_at IS NULL
         WHERE ws.procedure_id = $1 AND ws.version = $2 AND ws.deleted_at IS NULL
         ORDER BY ws.step_order ASC",
    )
    .bind(procedure_id)
    .bind(version)
    .fetch_all(pool)
    .await?;

    let now = Utc::now();
    Ok(rows
        .into_iter()
        .map(|r| {
            let quiet = r
                .last_seen_at
                .map(|t| now - t > chrono::Duration::hours(24))
                .unwrap_or(true);
            WorkflowStepItem {
                id: r.id,
                step_order: r.step_order,
                title: r.title,
                instructions: r.instructions,
                agent_id: r.agent_id,
                agent_name: r.agent_name,
                agent_quiet: quiet,
            }
        })
        .collect())
}

pub async fn fetch_procedure_runs(
    pool: &PgPool,
    procedure_id: Uuid,
) -> Result<Vec<RunSummaryItem>, AppError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        procedure_id: Uuid,
        procedure_version: i32,
        status: String,
        outcome: Option<RunOutcome>,
        current_step_order: i32,
        total_steps: i64,
        started_at: DateTime<Utc>,
        finished_at: Option<DateTime<Utc>>,
        thread_id: Option<Uuid>,
    }
    let rows = sqlx::query_as::<_, Row>(
        "SELECT r.id, r.procedure_id, r.procedure_version, r.status, r.outcome,
                r.current_step_order,
                (SELECT count(*) FROM run_steps rs WHERE rs.run_id = r.id AND rs.deleted_at IS NULL) AS total_steps,
                r.started_at, r.finished_at, r.thread_id
         FROM runs r
         WHERE r.procedure_id = $1 AND r.deleted_at IS NULL
         ORDER BY r.created_at DESC
         LIMIT 25",
    )
    .bind(procedure_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| RunSummaryItem {
            id: r.id,
            procedure_id: r.procedure_id,
            procedure_version: r.procedure_version,
            status: r.status,
            outcome: r.outcome.map(|o| match o {
                RunOutcome::Pass => "pass".to_string(),
                RunOutcome::Fail => "fail".to_string(),
                RunOutcome::Abandoned => "abandoned".to_string(),
            }),
            current_step_order: r.current_step_order,
            total_steps: r.total_steps,
            started_at: r.started_at,
            finished_at: r.finished_at,
            thread_id: r.thread_id,
        })
        .collect())
}

// ---------- Handlers: Workflows CRUD ----------

#[derive(Template)]
#[template(path = "workflow_detail.html")]
struct WorkflowDetailTemplate {
    workflow: WorkflowItem,
    steps: Vec<WorkflowStepItem>,
    runs: Vec<RunSummaryItem>,
    agents: Vec<crate::handlers::web::AgentOption>,
    questionnaire: WorkflowQuestionnaire,
    signed_in: bool,
    is_owner: bool,
    active_tab: &'static str,
}

#[derive(Template)]
#[template(path = "workflow_edit.html")]
struct WorkflowEditTemplate {
    workflow: WorkflowItem,
    board_name: Option<String>,
    steps: Vec<WorkflowStepItem>,
    agents: Vec<crate::handlers::web::AgentOption>,
    active_tab: &'static str,
}

#[derive(Template)]
#[template(path = "workflow_form.html")]
struct WorkflowFormTemplate {
    board_name: Option<String>,
    agents: Vec<crate::handlers::web::AgentOption>,
    active_tab: &'static str,
}

pub async fn new_workflow_form(
    State(state): State<AppState>,
    caller: Caller,
    Query(query): Query<WorkflowsQuery>,
) -> Result<Response, AppError> {
    let user_id = caller.user_id();
    let agents = crate::handlers::web::connected_agents(&state.pool, Some(user_id)).await?;
    render(WorkflowFormTemplate {
        board_name: query.board,
        agents,
        active_tab: "boards",
    })
    .map(IntoResponse::into_response)
}

fn visibility_str(v: Visibility) -> &'static str {
    match v {
        Visibility::Private => "private",
        Visibility::Unlisted => "unlisted",
        Visibility::Public => "public",
    }
}

pub async fn list_workflows(
    State(state): State<AppState>,
    OptionalCaller(caller): OptionalCaller,
    headers: HeaderMap,
    Query(query): Query<WorkflowsQuery>,
) -> Result<Response, AppError> {
    let viewer_id = caller.as_ref().map(Caller::user_id);
    let board_id = if let Some(ref name) = query.board {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM boards WHERE name = $1 AND deleted_at IS NULL",
        )
        .bind(name.trim().trim_start_matches('/'))
        .fetch_optional(&state.pool)
        .await?
    } else {
        None
    };

    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        user_id: Uuid,
        slug: String,
        title: String,
        summary: String,
        body: String,
        check_description: String,
        visibility: String,
        current_version: i32,
        run_count: i64,
        success_count: i64,
        board_id: Option<Uuid>,
        board_name: Option<String>,
        step_count: i64,
        created_at: DateTime<Utc>,
    }

    let rows = sqlx::query_as::<_, Row>(
        "SELECT p.id, p.user_id, p.slug, p.title, p.summary, p.body,
                p.check_description, p.visibility::text AS visibility, p.current_version,
                p.run_count, p.success_count, p.board_id,
                b.name::text AS board_name,
                (SELECT count(*) FROM workflow_steps ws WHERE ws.procedure_id = p.id AND ws.version = p.current_version AND ws.deleted_at IS NULL) AS step_count,
                p.created_at
         FROM procedures p
         LEFT JOIN boards b ON b.id = p.board_id AND b.deleted_at IS NULL
         WHERE p.deleted_at IS NULL
           AND p.kind = 'workflow'
           AND ($1::uuid IS NULL OR p.board_id = $1)
           AND (p.visibility = 'public' OR ($2::uuid IS NOT NULL AND p.user_id = $2))
         ORDER BY p.updated_at DESC",
    )
    .bind(board_id)
    .bind(viewer_id)
    .fetch_all(&state.pool)
    .await?;

    let items: Vec<WorkflowItem> = rows
        .into_iter()
        .map(|r| WorkflowItem {
            id: r.id,
            user_id: r.user_id,
            slug: r.slug,
            title: r.title,
            summary: r.summary,
            body: r.body,
            check_description: r.check_description,
            visibility: r.visibility,
            current_version: r.current_version,
            run_count: r.run_count,
            success_count: r.success_count,
            board_id: r.board_id,
            board_name: r.board_name,
            step_count: r.step_count,
            created_at: r.created_at,
        })
        .collect();

    if wants_html(&headers) {
        if let Some(bname) = query.board {
            return Ok(Redirect::to(&format!("/b/{bname}?view=workflows")).into_response());
        }
        return Ok(Redirect::to("/boards").into_response());
    }

    Ok(Json(json!({ "workflows": items })).into_response())
}

pub async fn get_workflow(
    State(state): State<AppState>,
    OptionalCaller(caller): OptionalCaller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let viewer_id = caller.as_ref().map(Caller::user_id);
    let procedure = sqlx::query_as::<_, Procedure>(
        "SELECT * FROM procedures WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    if procedure.visibility == Visibility::Private && Some(procedure.user_id) != viewer_id {
        return Err(AppError::Forbidden);
    }

    let board_name = if let Some(bid) = procedure.board_id {
        sqlx::query_scalar::<_, String>(
            "SELECT name::text FROM boards WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(bid)
        .fetch_optional(&state.pool)
        .await?
    } else {
        None
    };

    let steps = fetch_workflow_steps(&state.pool, procedure.id, procedure.current_version).await?;
    let runs = fetch_procedure_runs(&state.pool, procedure.id).await?;
    let is_owner = Some(procedure.user_id) == viewer_id;

    let item = WorkflowItem {
        id: procedure.id,
        user_id: procedure.user_id,
        slug: procedure.slug.clone(),
        title: procedure.title.clone(),
        summary: procedure.summary.clone(),
        body: procedure.body.clone(),
        check_description: procedure.check_description.clone(),
        visibility: visibility_str(procedure.visibility).to_string(),
        current_version: procedure.current_version,
        run_count: procedure.run_count,
        success_count: procedure.success_count,
        board_id: procedure.board_id,
        board_name,
        step_count: steps.len() as i64,
        created_at: procedure.created_at,
    };

    let agents = crate::handlers::web::connected_agents(&state.pool, viewer_id)
        .await
        .unwrap_or_default();

    let questionnaire = get_workflow_questionnaire(&procedure.title, item.board_name.as_deref());

    if wants_html(&headers) {
        return Ok(render(WorkflowDetailTemplate {
            workflow: item,
            steps,
            runs,
            agents,
            questionnaire,
            signed_in: viewer_id.is_some(),
            is_owner,
            active_tab: "boards",
        })?
        .into_response());
    }

    Ok(Json(json!({
        "workflow": item,
        "steps": steps,
        "runs": runs,
        "questionnaire": questionnaire
    }))
    .into_response())
}

pub async fn create_workflow(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    JsonOrForm(body): JsonOrForm<CreateWorkflowRequest>,
) -> Result<Response, AppError> {
    let user_id = caller.user_id();
    let title = body.title.trim();
    if title.is_empty() || title.chars().count() > 200 {
        return Err(AppError::BadRequest(
            "title must be 1-200 characters".to_string(),
        ));
    }
    let summary = body.summary.trim();
    let instructions = body.body.trim();
    let check_description = body.check_description.trim();
    if instructions.is_empty() {
        return Err(AppError::BadRequest(
            "instructions must not be empty".to_string(),
        ));
    }
    if check_description.is_empty() {
        return Err(AppError::BadRequest(
            "check_description must not be empty".to_string(),
        ));
    }

    let board_id = if let Some(bid) = body.board_id {
        Some(bid)
    } else if let Some(bname) = body.board_name.as_deref().or(body.board.as_deref()) {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM boards WHERE name = $1 AND deleted_at IS NULL",
        )
        .bind(bname.trim().trim_start_matches('/'))
        .fetch_optional(&state.pool)
        .await?
    } else {
        None
    };

    if let Some(key) = body.idempotency_key.as_deref().filter(|k| !k.is_empty()) {
        let existing = sqlx::query_as::<_, Procedure>(
            "SELECT * FROM procedures WHERE user_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL",
        )
        .bind(user_id)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?;

        if let Some(p) = existing {
            let steps = fetch_workflow_steps(&state.pool, p.id, p.current_version).await?;
            if wants_html(&headers) {
                return Ok(Redirect::to(&format!("/workflows/{}", p.id)).into_response());
            }
            return Ok((
                StatusCode::OK,
                Json(json!({ "workflow": p, "steps": steps })),
            )
                .into_response());
        }
    }

    let slug = format!(
        "{}-{}",
        slugify(title),
        &Uuid::new_v4().simple().to_string()[..6]
    );
    let visibility = body.visibility;

    let mut tx = state.pool.begin().await?;

    let procedure = sqlx::query_as::<_, Procedure>(
        "INSERT INTO procedures
            (user_id, slug, title, summary, body, check_description, kind,
             visibility, current_version, board_id, idempotency_key)
         VALUES ($1, $2, $3, $4, $5, $6, 'workflow', $7, 1, $8, $9)
         RETURNING *",
    )
    .bind(user_id)
    .bind(&slug)
    .bind(title)
    .bind(summary)
    .bind(instructions)
    .bind(check_description)
    .bind(visibility)
    .bind(board_id)
    .bind(body.idempotency_key.as_deref().filter(|k| !k.is_empty()))
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO procedure_versions (procedure_id, version, body, changelog)
         VALUES ($1, 1, $2, 'Initial workflow')",
    )
    .bind(procedure.id)
    .bind(instructions)
    .execute(&mut *tx)
    .await?;

    let mut steps = body.steps;
    if steps.is_empty() {
        if let (Some(t), Some(inst)) = (body.step1_title, body.step1_instructions) {
            if !t.trim().is_empty() {
                steps.push(WorkflowStepInput {
                    step_order: Some(1),
                    title: t,
                    instructions: inst,
                    agent_id: body.step1_agent_id,
                    agent_name: None,
                });
            }
        }
        if let (Some(t), Some(inst)) = (body.step2_title, body.step2_instructions) {
            if !t.trim().is_empty() {
                steps.push(WorkflowStepInput {
                    step_order: Some(2),
                    title: t,
                    instructions: inst,
                    agent_id: body.step2_agent_id,
                    agent_name: None,
                });
            }
        }
        if let (Some(t), Some(inst)) = (body.step3_title, body.step3_instructions) {
            if !t.trim().is_empty() {
                steps.push(WorkflowStepInput {
                    step_order: Some(3),
                    title: t,
                    instructions: inst,
                    agent_id: body.step3_agent_id,
                    agent_name: None,
                });
            }
        }
    }

    let mut created_steps = Vec::new();
    for (idx, step_input) in steps.iter().enumerate() {
        let order = step_input.step_order.unwrap_or((idx + 1) as i32);
        let resolved_agent = resolve_agent_id(
            &state.pool,
            user_id,
            step_input.agent_id,
            step_input.agent_name.as_deref(),
        )
        .await?;

        let step = sqlx::query_as::<_, WorkflowStep>(
            "INSERT INTO workflow_steps (procedure_id, version, step_order, title, instructions, agent_id)
             VALUES ($1, 1, $2, $3, $4, $5)
             RETURNING *",
        )
        .bind(procedure.id)
        .bind(order)
        .bind(step_input.title.trim())
        .bind(step_input.instructions.trim())
        .bind(resolved_agent)
        .fetch_one(&mut *tx)
        .await?;
        created_steps.push(step);
    }

    tx.commit().await?;

    if wants_html(&headers) {
        return Ok(Redirect::to(&format!("/workflows/{}", procedure.id)).into_response());
    }

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "workflow": procedure,
            "steps": created_steps
        })),
    )
        .into_response())
}

pub async fn update_workflow(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<UpdateWorkflowRequest>,
) -> Result<Response, AppError> {
    let user_id = caller.user_id();
    let procedure = sqlx::query_as::<_, Procedure>(
        "SELECT * FROM procedures WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    if procedure.user_id != user_id {
        return Err(AppError::Forbidden);
    }

    let title = body.title.as_deref().unwrap_or(&procedure.title).trim();
    let summary = body.summary.as_deref().unwrap_or(&procedure.summary).trim();
    let instructions = body.body.as_deref().unwrap_or(&procedure.body).trim();
    let check_description = body
        .check_description
        .as_deref()
        .unwrap_or(&procedure.check_description)
        .trim();
    let visibility = body.visibility.unwrap_or(procedure.visibility);
    let next_version = procedure.current_version + 1;

    let mut tx = state.pool.begin().await?;

    let updated_procedure = sqlx::query_as::<_, Procedure>(
        "UPDATE procedures
         SET title = $1, summary = $2, body = $3, check_description = $4,
             visibility = $5, current_version = $6, updated_at = now()
         WHERE id = $7
         RETURNING *",
    )
    .bind(title)
    .bind(summary)
    .bind(instructions)
    .bind(check_description)
    .bind(visibility)
    .bind(next_version)
    .bind(procedure.id)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO procedure_versions (procedure_id, version, body, changelog)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(procedure.id)
    .bind(next_version)
    .bind(instructions)
    .bind(body.changelog.as_deref().unwrap_or("Workflow updated"))
    .execute(&mut *tx)
    .await?;

    let mut form_steps = Vec::new();
    if let Some(t1) = body.step1_title.filter(|t| !t.trim().is_empty()) {
        form_steps.push(WorkflowStepInput {
            step_order: Some(1),
            title: t1,
            instructions: body.step1_instructions.unwrap_or_default(),
            agent_id: body.step1_agent_id,
            agent_name: None,
        });
    }
    if let Some(t2) = body.step2_title.filter(|t| !t.trim().is_empty()) {
        form_steps.push(WorkflowStepInput {
            step_order: Some(2),
            title: t2,
            instructions: body.step2_instructions.unwrap_or_default(),
            agent_id: body.step2_agent_id,
            agent_name: None,
        });
    }
    if let Some(t3) = body.step3_title.filter(|t| !t.trim().is_empty()) {
        form_steps.push(WorkflowStepInput {
            step_order: Some(3),
            title: t3,
            instructions: body.step3_instructions.unwrap_or_default(),
            agent_id: body.step3_agent_id,
            agent_name: None,
        });
    }
    if let Some(t4) = body.step4_title.filter(|t| !t.trim().is_empty()) {
        form_steps.push(WorkflowStepInput {
            step_order: Some(4),
            title: t4,
            instructions: body.step4_instructions.unwrap_or_default(),
            agent_id: body.step4_agent_id,
            agent_name: None,
        });
    }
    if let Some(t5) = body.step5_title.filter(|t| !t.trim().is_empty()) {
        form_steps.push(WorkflowStepInput {
            step_order: Some(5),
            title: t5,
            instructions: body.step5_instructions.unwrap_or_default(),
            agent_id: body.step5_agent_id,
            agent_name: None,
        });
    }
    if let Some(t6) = body.step6_title.filter(|t| !t.trim().is_empty()) {
        form_steps.push(WorkflowStepInput {
            step_order: Some(6),
            title: t6,
            instructions: body.step6_instructions.unwrap_or_default(),
            agent_id: body.step6_agent_id,
            agent_name: None,
        });
    }

    let effective_steps: Option<Vec<WorkflowStepInput>> = if let Some(steps) = body.steps {
        Some(steps)
    } else if !form_steps.is_empty() {
        Some(form_steps)
    } else {
        None
    };

    let mut updated_steps = Vec::new();
    if let Some(steps_input) = effective_steps {
        for (idx, step_input) in steps_input.iter().enumerate() {
            let order = step_input.step_order.unwrap_or((idx + 1) as i32);
            let resolved_agent = resolve_agent_id(
                &state.pool,
                user_id,
                step_input.agent_id,
                step_input.agent_name.as_deref(),
            )
            .await?;

            let step = sqlx::query_as::<_, WorkflowStep>(
                "INSERT INTO workflow_steps (procedure_id, version, step_order, title, instructions, agent_id)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 RETURNING *",
            )
            .bind(procedure.id)
            .bind(next_version)
            .bind(order)
            .bind(step_input.title.trim())
            .bind(step_input.instructions.trim())
            .bind(resolved_agent)
            .fetch_one(&mut *tx)
            .await?;
            updated_steps.push(step);
        }
    } else {
        // Carry forward previous steps into new version
        let prev_steps = sqlx::query_as::<_, WorkflowStep>(
            "SELECT * FROM workflow_steps
             WHERE procedure_id = $1 AND version = $2 AND deleted_at IS NULL
             ORDER BY step_order ASC",
        )
        .bind(procedure.id)
        .bind(procedure.current_version)
        .fetch_all(&mut *tx)
        .await?;

        for s in prev_steps {
            let step = sqlx::query_as::<_, WorkflowStep>(
                "INSERT INTO workflow_steps (procedure_id, version, step_order, title, instructions, agent_id)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 RETURNING *",
            )
            .bind(procedure.id)
            .bind(next_version)
            .bind(s.step_order)
            .bind(&s.title)
            .bind(&s.instructions)
            .bind(s.agent_id)
            .fetch_one(&mut *tx)
            .await?;
            updated_steps.push(step);
        }
    }

    tx.commit().await?;

    if wants_html(&headers) {
        return Ok(Redirect::to(&format!("/workflows/{}", procedure.id)).into_response());
    }

    Ok(Json(json!({
        "workflow": updated_procedure,
        "steps": updated_steps
    }))
    .into_response())
}

pub async fn edit_workflow_form(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let user_id = caller.user_id();
    let procedure = sqlx::query_as::<_, Procedure>(
        "SELECT * FROM procedures WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    if procedure.user_id != user_id {
        return Err(AppError::Forbidden);
    }

    let board_name = if let Some(bid) = procedure.board_id {
        sqlx::query_scalar::<_, String>(
            "SELECT name::text FROM boards WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(bid)
        .fetch_optional(&state.pool)
        .await?
    } else {
        None
    };

    let steps = fetch_workflow_steps(&state.pool, procedure.id, procedure.current_version).await?;
    let agents = crate::handlers::web::connected_agents(&state.pool, Some(user_id)).await?;

    let item = WorkflowItem {
        id: procedure.id,
        user_id: procedure.user_id,
        slug: procedure.slug,
        title: procedure.title,
        summary: procedure.summary,
        body: procedure.body,
        check_description: procedure.check_description,
        visibility: visibility_str(procedure.visibility).to_string(),
        current_version: procedure.current_version,
        run_count: procedure.run_count,
        success_count: procedure.success_count,
        board_id: procedure.board_id,
        board_name: board_name.clone(),
        step_count: steps.len() as i64,
        created_at: procedure.created_at,
    };

    Ok(render(WorkflowEditTemplate {
        workflow: item,
        board_name,
        steps,
        agents,
        active_tab: "boards",
    })?
    .into_response())
}

pub async fn delete_workflow(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let user_id = caller.user_id();
    let procedure = sqlx::query_as::<_, Procedure>(
        "SELECT * FROM procedures WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    if procedure.user_id != user_id {
        return Err(AppError::Forbidden);
    }

    sqlx::query("UPDATE procedures SET deleted_at = now(), updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;

    if wants_html(&headers) {
        if let Some(bid) = procedure.board_id {
            if let Some(bname) = sqlx::query_scalar::<_, String>(
                "SELECT name::text FROM boards WHERE id = $1 AND deleted_at IS NULL",
            )
            .bind(bid)
            .fetch_optional(&state.pool)
            .await?
            {
                return Ok(Redirect::to(&format!("/b/{bname}?view=workflows")).into_response());
            }
        }
        Ok(Redirect::to("/boards").into_response())
    } else {
        Ok(Json(json!({ "deleted": true, "id": id })).into_response())
    }
}

pub async fn assign_workflow_step(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path((workflow_id, step_order)): Path<(Uuid, i32)>,
    JsonOrForm(body): JsonOrForm<AssignStepRequest>,
) -> Result<Response, AppError> {
    let user_id = caller.user_id();
    let procedure = sqlx::query_as::<_, Procedure>(
        "SELECT * FROM procedures WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(workflow_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    if procedure.user_id != user_id {
        return Err(AppError::Forbidden);
    }

    let agent_id = resolve_agent_id(
        &state.pool,
        user_id,
        body.agent_id,
        body.agent_name.as_deref(),
    )
    .await?;

    sqlx::query(
        "UPDATE workflow_steps
         SET agent_id = $1, updated_at = now()
         WHERE procedure_id = $2 AND version = $3 AND step_order = $4 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .bind(workflow_id)
    .bind(procedure.current_version)
    .bind(step_order)
    .execute(&state.pool)
    .await?;

    if wants_html(&headers) {
        Ok(Redirect::to(&format!("/workflows/{workflow_id}")).into_response())
    } else {
        Ok(Json(json!({
            "assigned": true,
            "workflow_id": workflow_id,
            "step_order": step_order,
            "agent_id": agent_id
        }))
        .into_response())
    }
}

// ---------- Handlers: Run Orchestration Lifecycle ----------

#[derive(Template)]
#[template(path = "run_detail.html")]
struct RunDetailTemplate {
    run: RunDetailItem,
    has_openrouter_agent: bool,
    signed_in: bool,
    active_tab: &'static str,
}

pub async fn start_workflow_run(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<StartRunRequest>,
) -> Result<Response, AppError> {
    let user_id = caller.user_id();
    let procedure = sqlx::query_as::<_, Procedure>(
        "SELECT * FROM procedures WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    if procedure.visibility == Visibility::Private && procedure.user_id != user_id {
        return Err(AppError::Forbidden);
    }

    let version = body.procedure_version.unwrap_or(procedure.current_version);
    let steps = sqlx::query_as::<_, WorkflowStep>(
        "SELECT * FROM workflow_steps
         WHERE procedure_id = $1 AND version = $2 AND deleted_at IS NULL
         ORDER BY step_order ASC",
    )
    .bind(procedure.id)
    .bind(version)
    .fetch_all(&state.pool)
    .await?;

    if steps.is_empty() {
        return Err(AppError::BadRequest(
            "workflow has no steps defined".to_string(),
        ));
    }

    // A run only ever drives the runner's own agents. Running someone
    // else's public workflow must not dispatch commands to the author's
    // agents (or spend the author's provider key); those steps start
    // unassigned in the runner's thread instead.
    let step_agent_ids: Vec<Uuid> = steps.iter().filter_map(|s| s.agent_id).collect();
    let runner_agent_ids: Vec<Uuid> = if step_agent_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_scalar(
            "SELECT id FROM agents
             WHERE id = ANY($1) AND user_id = $2 AND revoked_at IS NULL AND deleted_at IS NULL",
        )
        .bind(&step_agent_ids)
        .bind(user_id)
        .fetch_all(&state.pool)
        .await?
    };
    let steps: Vec<WorkflowStep> = steps
        .into_iter()
        .map(|mut step| {
            if step
                .agent_id
                .is_some_and(|a| !runner_agent_ids.contains(&a))
            {
                step.agent_id = None;
            }
            step
        })
        .collect();

    if let Some(key) = body.idempotency_key.as_deref().filter(|k| !k.is_empty()) {
        let existing = sqlx::query_as::<_, Run>(
            "SELECT * FROM runs WHERE user_id = $1 AND idempotency_key = $2 AND deleted_at IS NULL",
        )
        .bind(user_id)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?;

        if let Some(run) = existing {
            if wants_html(&headers) {
                return Ok(Redirect::to(&format!("/runs/{}", run.id)).into_response());
            }
            return Ok((StatusCode::OK, Json(run)).into_response());
        }
    }

    // 1. Create Run Thread with distinct Run ID
    let run_id = Uuid::new_v4();
    let short_id = &run_id.simple().to_string()[..6];
    let thread_title = format!("{} — Run #{}", procedure.title, short_id);
    let thread_req = crate::handlers::threads::CreateThreadRequest {
        title: thread_title,
        visibility: Some(procedure.visibility),
        idempotency_key: None,
        board_id: procedure.board_id,
        board_name: None,
        command: None,
        workspace_id: None,
        agent_ids: Vec::new(),
    };
    let (thread, _) =
        crate::handlers::threads::create_thread_inner(&state, &caller, thread_req).await?;

    // 2. Ensure all assigned agents are joined to thread_agents
    for step in &steps {
        if let Some(agent_id) = step.agent_id {
            let _ = sqlx::query(
                "INSERT INTO thread_agents (thread_id, agent_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            )
            .bind(thread.id)
            .bind(agent_id)
            .execute(&state.pool)
            .await;
        }
    }

    let first_step = &steps[0];
    let model = body.model.unwrap_or_else(|| "external-agents".to_string());

    let q_spec = get_workflow_questionnaire(&procedure.title, None);
    let mut param_lines = Vec::new();
    let mut answers_map = serde_json::Map::new();

    if let Some(ref q1) = body.q1.filter(|s| !s.trim().is_empty()) {
        let q_title = q_spec
            .questions
            .first()
            .map(|q| q.title.as_str())
            .unwrap_or("Parameter 1");
        param_lines.push(format!("- {}: {}", q_title, q1));
        answers_map.insert(q_title.to_string(), serde_json::Value::String(q1.clone()));
    }
    if let Some(ref q2) = body.q2.filter(|s| !s.trim().is_empty()) {
        let q_title = q_spec
            .questions
            .get(1)
            .map(|q| q.title.as_str())
            .unwrap_or("Parameter 2");
        param_lines.push(format!("- {}: {}", q_title, q2));
        answers_map.insert(q_title.to_string(), serde_json::Value::String(q2.clone()));
    }
    if let Some(ref q3) = body.q3.filter(|s| !s.trim().is_empty()) {
        let q_title = q_spec
            .questions
            .get(2)
            .map(|q| q.title.as_str())
            .unwrap_or("Parameter 3");
        param_lines.push(format!("- {}: {}", q_title, q3));
        answers_map.insert(q_title.to_string(), serde_json::Value::String(q3.clone()));
    }
    if let Some(ref custom) = body.custom_input.as_ref().filter(|s| !s.trim().is_empty()) {
        param_lines.push(format!("- Custom Notes / Instructions: {}", custom));
    }

    let initial_context = if !param_lines.is_empty() {
        format!(
            "[Run Parameters & Questionnaire]\n{}\n",
            param_lines.join("\n")
        )
    } else {
        String::new()
    };

    let q_json = if answers_map.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(answers_map))
    };
    let custom_txt = body.custom_input.filter(|s| !s.trim().is_empty());

    // 3. Create Run Record
    let run = sqlx::query_as::<_, Run>(
        "INSERT INTO runs
            (id, procedure_id, procedure_version, agent_id, user_id, model,
             status, current_step_order, started_at, thread_id, idempotency_key,
             questionnaire_answers, custom_input)
         VALUES ($1, $2, $3, $4, $5, $6, 'running', 1, now(), $7, $8, $9, $10)
         RETURNING *",
    )
    .bind(run_id)
    .bind(procedure.id)
    .bind(version)
    .bind(first_step.agent_id)
    .bind(user_id)
    .bind(model)
    .bind(thread.id)
    .bind(body.idempotency_key.as_deref().filter(|k| !k.is_empty()))
    .bind(q_json)
    .bind(custom_txt)
    .fetch_one(&state.pool)
    .await?;

    // 4. Create RunSteps for all steps
    let mut created_run_steps = Vec::new();
    for (idx, step) in steps.iter().enumerate() {
        let initial_status = if idx == 0 { "dispatched" } else { "pending" };
        let step_input = if idx == 0 && !initial_context.is_empty() {
            Some(initial_context.trim().to_string())
        } else {
            None
        };
        let run_step = sqlx::query_as::<_, RunStep>(
            "INSERT INTO run_steps
                (run_id, step_id, step_order, title, instructions, agent_id, status, input)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING *",
        )
        .bind(run.id)
        .bind(step.id)
        .bind(step.step_order)
        .bind(&step.title)
        .bind(&step.instructions)
        .bind(step.agent_id)
        .bind(initial_status)
        .bind(step_input)
        .fetch_one(&state.pool)
        .await?;
        created_run_steps.push(run_step);
    }

    // 5. Dispatch Step 1 Command Note
    let step1 = &created_run_steps[0];
    let agent_name = if let Some(aid) = step1.agent_id {
        sqlx::query_scalar::<_, String>("SELECT name FROM agents WHERE id = $1")
            .bind(aid)
            .fetch_optional(&state.pool)
            .await?
            .unwrap_or_else(|| "agent".to_string())
    } else {
        "unassigned".to_string()
    };

    // Check if agent is online/offline
    let agent_last_seen = if let Some(aid) = step1.agent_id {
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT COALESCE(last_seen_at, created_at) FROM agents WHERE id = $1",
        )
        .bind(aid)
        .fetch_optional(&state.pool)
        .await?
        .flatten()
    } else {
        None
    };

    let is_offline = agent_last_seen
        .map(|t| Utc::now() - t > chrono::Duration::hours(24))
        .unwrap_or(false);
    let run_step_status = if is_offline && step1.agent_id.is_some() {
        "waiting"
    } else {
        "dispatched"
    };

    let command_body = if !initial_context.is_empty() {
        format!(
            "[Step 1: {}]\nAssigned Agent: {}\nStatus: {}\n\n{}\nInstructions:\n{}\n\nRun ID: {}\nStep Order: 1",
            step1.title, agent_name, run_step_status, initial_context, step1.instructions, run.id
        )
    } else {
        format!(
            "[Step 1: {}]\nAssigned Agent: {}\nStatus: {}\n\nInstructions:\n{}\n\nRun ID: {}\nStep Order: 1",
            step1.title, agent_name, run_step_status, step1.instructions, run.id
        )
    };

    let note = sqlx::query_as::<_, Note>(
        "INSERT INTO notes (thread_id, author_user_id, body, kind)
         VALUES ($1, $2, $3, 'command')
         RETURNING *",
    )
    .bind(thread.id)
    .bind(user_id)
    .bind(command_body)
    .fetch_one(&state.pool)
    .await?;

    sqlx::query(
        "UPDATE run_steps
         SET status = $1, command_note_id = $2, started_at = now()
         WHERE id = $3",
    )
    .bind(run_step_status)
    .bind(note.id)
    .bind(step1.id)
    .execute(&state.pool)
    .await?;

    if run_step_status == "waiting" {
        sqlx::query("UPDATE runs SET status = 'waiting' WHERE id = $1")
            .bind(run.id)
            .execute(&state.pool)
            .await?;
    }

    let is_step1_openrouter = if let Some(aid) = step1.agent_id {
        sqlx::query_scalar::<_, bool>(
            "SELECT COALESCE(provider_type = 'openrouter', false) FROM agents WHERE id = $1",
        )
        .bind(aid)
        .fetch_optional(&state.pool)
        .await?
        .unwrap_or(false)
    } else {
        false
    };

    if is_step1_openrouter {
        let _ = execute_openrouter_step_internal(&state, run.id, 1).await;
    }

    if wants_html(&headers) {
        return Ok(Redirect::to(&format!("/runs/{}", run.id)).into_response());
    }

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "run": run,
            "thread_id": thread.id,
            "steps": created_run_steps
        })),
    )
        .into_response())
}

pub async fn get_run(
    State(state): State<AppState>,
    OptionalCaller(caller): OptionalCaller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let viewer_id = caller.as_ref().map(Caller::user_id);
    let run = sqlx::query_as::<_, Run>("SELECT * FROM runs WHERE id = $1 AND deleted_at IS NULL")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let procedure = sqlx::query_as::<_, Procedure>(
        "SELECT * FROM procedures WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(run.procedure_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    if procedure.visibility == Visibility::Private && Some(run.user_id) != viewer_id {
        return Err(AppError::Forbidden);
    }

    #[derive(sqlx::FromRow)]
    struct StepRow {
        id: Uuid,
        step_order: i32,
        title: String,
        instructions: String,
        agent_id: Option<Uuid>,
        agent_name: Option<String>,
        last_seen_at: Option<DateTime<Utc>>,
        provider_type: Option<String>,
        status: String,
        input: Option<String>,
        output: Option<String>,
        started_at: Option<DateTime<Utc>>,
        finished_at: Option<DateTime<Utc>>,
    }

    let rows = sqlx::query_as::<_, StepRow>(
        "SELECT rs.id, rs.step_order, rs.title, rs.instructions,
                rs.agent_id, a.name AS agent_name, a.last_seen_at, a.provider_type,
                rs.status, rs.input, rs.output, rs.started_at, rs.finished_at
         FROM run_steps rs
         LEFT JOIN agents a ON a.id = rs.agent_id AND a.deleted_at IS NULL
         WHERE rs.run_id = $1 AND rs.deleted_at IS NULL
         ORDER BY rs.step_order ASC",
    )
    .bind(run.id)
    .fetch_all(&state.pool)
    .await?;

    let now = Utc::now();
    let steps: Vec<RunStepDetailItem> = rows
        .into_iter()
        .map(|r| {
            let is_or = r.provider_type.as_deref() == Some("openrouter");
            let quiet = if is_or {
                false
            } else {
                r.last_seen_at
                    .map(|t| now - t > chrono::Duration::minutes(15))
                    .unwrap_or(true)
            };
            RunStepDetailItem {
                id: r.id,
                step_order: r.step_order,
                title: r.title,
                instructions: r.instructions,
                agent_id: r.agent_id,
                agent_name: r.agent_name,
                agent_quiet: quiet,
                is_openrouter: is_or,
                status: r.status,
                input: r.input,
                output: r.output,
                started_at: r.started_at,
                finished_at: r.finished_at,
            }
        })
        .collect();

    let is_owner = Some(run.user_id) == viewer_id;

    let detail = RunDetailItem {
        id: run.id,
        procedure_id: run.procedure_id,
        workflow_title: procedure.title,
        workflow_check: procedure.check_description,
        procedure_version: run.procedure_version,
        status: run.status,
        outcome: run.outcome.map(|o| match o {
            RunOutcome::Pass => "pass".to_string(),
            RunOutcome::Fail => "fail".to_string(),
            RunOutcome::Abandoned => "abandoned".to_string(),
        }),
        current_step_order: run.current_step_order,
        started_at: run.started_at,
        finished_at: run.finished_at,
        thread_id: run.thread_id,
        custom_input: run.custom_input,
        steps,
        is_owner,
    };

    let has_openrouter_agent = if let Some(uid) = viewer_id {
        let has_agent = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE user_id = $1 AND provider_type = 'openrouter' AND deleted_at IS NULL)"
        )
        .bind(uid)
        .fetch_one(&state.pool)
        .await
        .unwrap_or(false);
        has_agent
            || std::env::var("OPENROUTER_API_KEY")
                .map(|k| !k.is_empty())
                .unwrap_or(false)
    } else {
        std::env::var("OPENROUTER_API_KEY")
            .map(|k| !k.is_empty())
            .unwrap_or(false)
    };

    if wants_html(&headers) {
        return Ok(render(RunDetailTemplate {
            run: detail,
            has_openrouter_agent,
            signed_in: viewer_id.is_some(),
            active_tab: "boards",
        })?
        .into_response());
    }

    Ok(Json(json!({
        "run": detail,
        "steps": detail.steps,
    }))
    .into_response())
}

pub type StepResultFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(Run, bool), AppError>> + Send>>;

pub fn submit_step_result_inner(
    state: &AppState,
    caller_agent_id: Option<Uuid>,
    caller_user_id: Option<Uuid>,
    run_id: Uuid,
    step_order: i32,
    output: Option<String>,
    outcome: Option<String>,
) -> StepResultFuture {
    let state = state.clone();
    Box::pin(async move {
        let run =
            sqlx::query_as::<_, Run>("SELECT * FROM runs WHERE id = $1 AND deleted_at IS NULL")
                .bind(run_id)
                .fetch_optional(&state.pool)
                .await?
                .ok_or(AppError::NotFound)?;

        let step = sqlx::query_as::<_, RunStep>(
            "SELECT * FROM run_steps WHERE run_id = $1 AND step_order = $2 AND deleted_at IS NULL",
        )
        .bind(run_id)
        .bind(step_order)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

        let thread_id = run.thread_id.ok_or(AppError::Internal)?;
        let output = output
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Step completed according to workflow instructions.")
            .to_string();
        let is_pass = outcome.as_deref().unwrap_or("pass") != "fail";

        // Write Status Note to Thread
        let author_agent_id = caller_agent_id.or(step.agent_id);
        let author_user_id = if author_agent_id.is_none() {
            caller_user_id.or(Some(run.user_id))
        } else {
            None
        };

        let agent_display = if let Some(aid) = step.agent_id {
            sqlx::query_scalar::<_, String>("SELECT name FROM agents WHERE id = $1")
                .bind(aid)
                .fetch_optional(&state.pool)
                .await?
                .unwrap_or_else(|| "agent".to_string())
        } else {
            "agent".to_string()
        };

        let note_body = format!(
            "[Step {} Result by {}]\nStatus: {}\n\nOutput:\n{}",
            step_order,
            agent_display,
            if is_pass { "completed" } else { "failed" },
            output
        );

        // Finish the step only if it isn't finished yet. An agent that sends
        // its result again (a retry after a timeout), or two at once, finds
        // it done and changes nothing: no second note, no second advance.
        let finished_now = sqlx::query(
            "UPDATE run_steps
             SET status = $1, output = $2, finished_at = now()
             WHERE id = $3 AND status NOT IN ('completed', 'failed')",
        )
        .bind(if is_pass { "completed" } else { "failed" })
        .bind(&output)
        .bind(step.id)
        .execute(&state.pool)
        .await?
        .rows_affected()
            == 1;
        if !finished_now {
            let current = sqlx::query_as::<_, Run>("SELECT * FROM runs WHERE id = $1")
                .bind(run.id)
                .fetch_one(&state.pool)
                .await?;
            return Ok((current, false));
        }

        let result_note = sqlx::query_as::<_, Note>(
            "INSERT INTO notes (thread_id, author_agent_id, author_user_id, body, kind)
             VALUES ($1, $2, $3, $4, 'status')
             RETURNING *",
        )
        .bind(thread_id)
        .bind(author_agent_id)
        .bind(author_user_id)
        .bind(note_body)
        .fetch_one(&state.pool)
        .await?;
        sqlx::query("UPDATE run_steps SET result_note_id = $1 WHERE id = $2")
            .bind(result_note.id)
            .bind(step.id)
            .execute(&state.pool)
            .await?;

        if !is_pass {
            // Run failed
            sqlx::query(
                "UPDATE runs
                 SET status = 'failed', outcome = 'fail', finished_at = now()
                 WHERE id = $1",
            )
            .bind(run.id)
            .execute(&state.pool)
            .await?;

            let _ = sqlx::query(
                "INSERT INTO notes (thread_id, author_user_id, body, kind)
                 VALUES ($1, $2, $3, 'system')",
            )
            .bind(thread_id)
            .bind(run.user_id)
            .bind(format!("Workflow run failed at Step {step_order}."))
            .execute(&state.pool)
            .await;

            let updated_run = sqlx::query_as::<_, Run>("SELECT * FROM runs WHERE id = $1")
                .bind(run.id)
                .fetch_one(&state.pool)
                .await?;
            return Ok((updated_run, false));
        }

        // Check for next step
        let next_step = sqlx::query_as::<_, RunStep>(
            "SELECT * FROM run_steps WHERE run_id = $1 AND step_order = $2 AND deleted_at IS NULL",
        )
        .bind(run.id)
        .bind(step_order + 1)
        .fetch_optional(&state.pool)
        .await?;

        let has_next = next_step.is_some();
        if let Some(next) = next_step {
            // Collect all completed prior step outputs
            let prior_steps = sqlx::query_as::<_, RunStep>(
                "SELECT * FROM run_steps WHERE run_id = $1 AND step_order <= $2 AND deleted_at IS NULL ORDER BY step_order ASC",
            )
            .bind(run.id)
            .bind(step_order)
            .fetch_all(&state.pool)
            .await?;

            let mut prior_context = String::new();
            if let Some(ref custom) = run.custom_input {
                prior_context.push_str(&format!("[Run Parameters / Custom Notes]\n{}\n\n", custom));
            }
            for ps in prior_steps {
                prior_context.push_str(&format!(
                    "Step {} ({}):\n{}\n\n",
                    ps.step_order,
                    ps.title,
                    ps.output.as_deref().unwrap_or("(no output)")
                ));
            }

            // Check if next agent is online
            let next_agent_name = if let Some(aid) = next.agent_id {
                sqlx::query_scalar::<_, String>("SELECT name FROM agents WHERE id = $1")
                    .bind(aid)
                    .fetch_optional(&state.pool)
                    .await?
                    .unwrap_or_else(|| "agent".to_string())
            } else {
                "unassigned".to_string()
            };

            let next_agent_last_seen = if let Some(aid) = next.agent_id {
                sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
                    "SELECT COALESCE(last_seen_at, created_at) FROM agents WHERE id = $1",
                )
                .bind(aid)
                .fetch_optional(&state.pool)
                .await?
                .flatten()
            } else {
                None
            };

            let is_next_offline = next_agent_last_seen
                .map(|t| Utc::now() - t > chrono::Duration::hours(24))
                .unwrap_or(false);

            let next_status = if is_next_offline && next.agent_id.is_some() {
                "waiting"
            } else {
                "dispatched"
            };

            let next_command = format!(
                "[Step {}: {}]\nAssigned Agent: {}\nStatus: {}\n\nInstructions:\n{}\n\nPrior Results:\n{}\nRun ID: {}\nStep Order: {}",
                next.step_order, next.title, next_agent_name, next_status, next.instructions, prior_context.trim(), run.id, next.step_order
            );

            let next_note = sqlx::query_as::<_, Note>(
                "INSERT INTO notes (thread_id, author_user_id, body, kind)
                 VALUES ($1, $2, $3, 'command')
                 RETURNING *",
            )
            .bind(thread_id)
            .bind(run.user_id)
            .bind(next_command)
            .fetch_one(&state.pool)
            .await?;

            sqlx::query(
                "UPDATE run_steps
                 SET status = $1, input = $2, command_note_id = $3, started_at = now()
                 WHERE id = $4",
            )
            .bind(next_status)
            .bind(prior_context.trim())
            .bind(next_note.id)
            .bind(next.id)
            .execute(&state.pool)
            .await?;

            sqlx::query(
                "UPDATE runs
                 SET status = $1, current_step_order = $2
                 WHERE id = $3",
            )
            .bind(if next_status == "waiting" {
                "waiting"
            } else {
                "running"
            })
            .bind(next.step_order)
            .bind(run.id)
            .execute(&state.pool)
            .await?;

            let is_next_openrouter = if let Some(aid) = next.agent_id {
                sqlx::query_scalar::<_, bool>(
                    "SELECT COALESCE(provider_type = 'openrouter', false) FROM agents WHERE id = $1",
                )
                .bind(aid)
                .fetch_optional(&state.pool)
                .await?
                .unwrap_or(false)
            } else {
                false
            };

            if is_next_openrouter {
                let state_c = state.clone();
                let r_id = run.id;
                let n_order = next.step_order;
                tokio::spawn(async move {
                    let _ = execute_openrouter_step_internal(&state_c, r_id, n_order).await;
                });
            }
        } else {
            // All steps completed!
            let procedure = sqlx::query_as::<_, Procedure>(
                "SELECT * FROM procedures WHERE id = $1 AND deleted_at IS NULL",
            )
            .bind(run.procedure_id)
            .fetch_one(&state.pool)
            .await?;

            let duration_ms = (Utc::now() - run.started_at).num_milliseconds().max(0);

            sqlx::query(
                "UPDATE runs
                 SET status = 'completed', outcome = 'pass', finished_at = now(), duration_ms = $1
                 WHERE id = $2",
            )
            .bind(duration_ms)
            .bind(run.id)
            .execute(&state.pool)
            .await?;

            sqlx::query(
                "UPDATE procedures
                 SET run_count = run_count + 1,
                     success_count = success_count + 1,
                     scored_run_count = scored_run_count + 1,
                     updated_at = now()
                 WHERE id = $1",
            )
            .bind(procedure.id)
            .execute(&state.pool)
            .await?;

            let completion_note = format!(
                "[Workflow Completed Successfully]\nOutcome: Pass\nDuration: {}ms\n\nCompletion check verified:\n{}",
                duration_ms, procedure.check_description
            );

            let _ = sqlx::query(
                "INSERT INTO notes (thread_id, author_user_id, body, kind)
                 VALUES ($1, $2, $3, 'system')",
            )
            .bind(thread_id)
            .bind(run.user_id)
            .bind(completion_note)
            .execute(&state.pool)
            .await;
        }

        let updated_run = sqlx::query_as::<_, Run>("SELECT * FROM runs WHERE id = $1")
            .bind(run.id)
            .fetch_one(&state.pool)
            .await?;

        Ok((updated_run, has_next))
    })
}

pub async fn execute_openrouter_step_internal(
    state: &AppState,
    run_id: Uuid,
    step_order: i32,
) -> Result<String, AppError> {
    let run = sqlx::query_as::<_, Run>("SELECT * FROM runs WHERE id = $1 AND deleted_at IS NULL")
        .bind(run_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let step = sqlx::query_as::<_, RunStep>(
        "SELECT * FROM run_steps WHERE run_id = $1 AND step_order = $2 AND deleted_at IS NULL",
    )
    .bind(run_id)
    .bind(step_order)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    // Determine OpenRouter API key and model. Only the run owner's own
    // agents' keys are ever used, never another user's.
    let (api_key, model) = if let Some(aid) = step.agent_id {
        let agent_info = sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT provider_model, provider_api_key FROM agents
             WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL AND deleted_at IS NULL",
        )
        .bind(aid)
        .bind(run.user_id)
        .fetch_optional(&state.pool)
        .await?
        .unwrap_or((None, None));

        let key = agent_info
            .1
            .and_then(|k| crate::crypto::decrypt(&k))
            .filter(|k| !k.is_empty())
            .or_else(|| std::env::var("OPENROUTER_API_KEY").ok())
            .unwrap_or_default();
        let model = agent_info
            .0
            .unwrap_or_else(|| crate::openrouter::DEFAULT_MODEL.to_string());
        (key, model)
    } else {
        let user_or_agent = sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT provider_model, provider_api_key FROM agents
             WHERE user_id = $1 AND provider_type = 'openrouter'
               AND revoked_at IS NULL AND deleted_at IS NULL
             ORDER BY created_at ASC LIMIT 1",
        )
        .bind(run.user_id)
        .fetch_optional(&state.pool)
        .await?
        .unwrap_or((None, None));

        let key = user_or_agent
            .1
            .and_then(|k| crate::crypto::decrypt(&k))
            .filter(|k| !k.is_empty())
            .or_else(|| std::env::var("OPENROUTER_API_KEY").ok())
            .unwrap_or_default();
        let model = user_or_agent
            .0
            .unwrap_or_else(|| crate::openrouter::DEFAULT_MODEL.to_string());
        (key, model)
    };

    if api_key.is_empty() {
        return Err(AppError::BadRequest(
            "No OpenRouter API key configured. Please add an OpenRouter agent in Settings -> Agents or set OPENROUTER_API_KEY.".to_string(),
        ));
    }

    // 1. Resolve workspace_id for budget & lessons
    let workspace_id = match run.workspace_id {
        Some(wid) => wid,
        None => {
            let ws =
                crate::handlers::airo::get_or_create_default_workspace(&state.pool, run.user_id)
                    .await?;
            ws.id
        }
    };

    // 2. Deterministic Budget Gate check and deduction
    let proposal = crate::office::budget_gate::ActionProposal {
        provider: "openrouter".into(),
        model_or_tool: model.clone(),
        estimated_credits: 25,
        raw_cost_micros: 250,
        requires_manual_approval: false,
        action_type: "workflow_step_execution".into(),
    };

    let spender = crate::office::spend::Spender::new(workspace_id, None);
    let gate = crate::office::budget_gate::check_and_deduct(
        &state.pool,
        workspace_id,
        &proposal,
        run.task_id,
        false,
        spender,
    )
    .await?;
    if !gate.passed {
        let reason = gate.reasons.join(", ");
        return Err(AppError::BadRequest(format!(
            "Budget gate check failed: {reason}"
        )));
    }

    // 3. Query persistent workspace lessons
    let lessons = sqlx::query_scalar::<_, String>(
        "SELECT rule FROM workspace_lessons WHERE workspace_id = $1 AND deleted_at IS NULL ORDER BY created_at DESC LIMIT 10",
    )
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let lessons_prompt = if !lessons.is_empty() {
        format!(
            "\n\n[Persistent Workspace Lessons & Guidelines - MUST FOLLOW]:\n{}",
            lessons
                .iter()
                .map(|l| format!("- {}", l))
                .collect::<Vec<_>>()
                .join("\n")
        )
    } else {
        String::new()
    };

    let system_prompt = format!(
        "You are an AI agent executing step {} ('{}') of an Air workflow. Follow instructions carefully and output the substantive result for this step.{}",
        step.step_order, step.title, lessons_prompt
    );

    let user_prompt = format!(
        "Step: {}\nInstructions:\n{}\n\nContext and Prior Steps:\n{}\n\nPlease provide your detailed output for this step.",
        step.title,
        step.instructions,
        step.input.as_deref().unwrap_or("(None)")
    );

    let output = match crate::openrouter::call_openrouter_with_usage(
        &api_key,
        &model,
        &system_prompt,
        &user_prompt,
    )
    .await
    {
        Ok(completion) => {
            crate::office::budget_gate::record_usage(
                &state.pool,
                gate.ledger_id,
                &completion.usage,
            )
            .await;
            Ok(completion.text)
        }
        Err(e) => {
            // Charged up front; the step never ran, so give the credits back.
            let _ = crate::office::budget_gate::refund_credit_charge(
                &state.pool,
                workspace_id,
                run.task_id,
                &proposal,
                spender,
            )
            .await;
            Err(e)
        }
    }
    .map_err(|e| {
        // The provider's error text can include its raw response; log
        // it, return only a short public message.
        tracing::warn!(error = %e, "OpenRouter step execution failed");
        AppError::ServiceUnavailable(
            "The AI provider could not complete this step. Try again shortly.".to_string(),
        )
    })?;

    let caller_agent_id = step.agent_id;
    let caller_user_id = Some(run.user_id);

    submit_step_result_inner(
        state,
        caller_agent_id,
        caller_user_id,
        run_id,
        step_order,
        Some(output.clone()),
        Some("pass".to_string()),
    )
    .await?;

    Ok(output)
}

pub async fn run_step_openrouter(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path((run_id, step_order)): Path<(Uuid, i32)>,
) -> Result<Response, AppError> {
    let run = sqlx::query_as::<_, Run>("SELECT * FROM runs WHERE id = $1 AND deleted_at IS NULL")
        .bind(run_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let authorized = match &caller {
        Caller::User(u) => u.id == run.user_id,
        Caller::Agent(a) => a.user_id == run.user_id,
    };
    if !authorized {
        return Err(AppError::Forbidden);
    }

    let output = execute_openrouter_step_internal(&state, run_id, step_order).await?;

    if wants_html(&headers) {
        return Ok(Redirect::to(&format!("/runs/{}", run_id)).into_response());
    }

    Ok(Json(json!({
        "status": "success",
        "run_id": run_id,
        "step_order": step_order,
        "output": output
    }))
    .into_response())
}

pub async fn submit_step_result(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path((run_id, step_order)): Path<(Uuid, i32)>,
    JsonOrForm(body): JsonOrForm<StepResultRequest>,
) -> Result<Response, AppError> {
    let run = sqlx::query_as::<_, Run>("SELECT * FROM runs WHERE id = $1 AND deleted_at IS NULL")
        .bind(run_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let step = sqlx::query_as::<_, RunStep>(
        "SELECT * FROM run_steps WHERE run_id = $1 AND step_order = $2 AND deleted_at IS NULL",
    )
    .bind(run_id)
    .bind(step_order)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    // Authorization: caller is either the assigned agent or run owner
    let authorized = match &caller {
        Caller::User(u) => u.id == run.user_id,
        Caller::Agent(a) => step.agent_id == Some(a.id) || a.user_id == run.user_id,
    };
    if !authorized {
        return Err(AppError::Forbidden);
    }

    let (caller_agent_id, caller_user_id) = match &caller {
        Caller::Agent(a) => (Some(a.id), None),
        Caller::User(u) => (None, Some(u.id)),
    };

    let (updated_run, has_next) = submit_step_result_inner(
        &state,
        caller_agent_id,
        caller_user_id,
        run_id,
        step_order,
        body.output,
        body.outcome,
    )
    .await?;

    if wants_html(&headers) {
        return Ok(Redirect::to(&format!("/runs/{}", run.id)).into_response());
    }

    Ok(Json(json!({
        "status": if updated_run.status == "failed" { "failed" } else { "success" },
        "step_order": step_order,
        "advanced": has_next,
        "run": updated_run,
    }))
    .into_response())
}

// ---------- Handlers: Connected Agents & Pending Steps API ----------

pub async fn list_connected_agents(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let user_id = caller.user_id();

    #[derive(sqlx::FromRow)]
    struct AgentRow {
        id: Uuid,
        name: String,
        handle: Option<String>,
        instructions: Option<String>,
        ai_engine: Option<String>,
        ai_model: Option<String>,
        provider_model: Option<String>,
        has_own_key: bool,
        key_prefix: String,
        last_seen_at: Option<DateTime<Utc>>,
        created_at: DateTime<Utc>,
    }

    // Mirrors the reply voice chain in threads.rs (`resolve_voice`).
    let account_key_model: Option<Option<String>> = sqlx::query_scalar(
        "SELECT provider_model FROM agents
         WHERE user_id = $1 AND provider_type = 'openrouter'
           AND provider_api_key IS NOT NULL AND provider_api_key <> ''
           AND deleted_at IS NULL AND revoked_at IS NULL
         ORDER BY created_at ASC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;
    let local = state.config.local.is_some();
    let platform = state
        .config
        .openrouter_api_key
        .as_deref()
        .is_some_and(|k| !k.trim().is_empty());
    let pick = |m: &Option<String>| m.clone().filter(|m| !m.trim().is_empty());

    let rows = sqlx::query_as::<_, AgentRow>(
        "SELECT id, name, handle, instructions, ai_engine, ai_model, provider_model,
                (provider_api_key IS NOT NULL AND provider_api_key <> '') AS has_own_key,
                key_prefix, last_seen_at, created_at
         FROM agents
         WHERE user_id = $1 AND revoked_at IS NULL AND deleted_at IS NULL
         ORDER BY name ASC",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;

    let now = Utc::now();
    let agents: Vec<ActiveAgentItem> = rows
        .into_iter()
        .map(|r| {
            let quiet = r
                .last_seen_at
                .or(Some(r.created_at))
                .map(|t| now - t > chrono::Duration::hours(24))
                .unwrap_or(false);
            let default_model = crate::openrouter::DEFAULT_MODEL.to_string();
            let (ai_source, effective_model) = if local {
                ("this_computer", None)
            } else if r.has_own_key {
                (
                    "own_key",
                    Some(pick(&r.provider_model).unwrap_or(default_model)),
                )
            } else if let Some(shared) = &account_key_model {
                (
                    "account_key",
                    Some(
                        pick(&r.provider_model)
                            .or_else(|| pick(shared))
                            .unwrap_or(default_model),
                    ),
                )
            } else if platform {
                ("included", Some(default_model))
            } else {
                ("none", None)
            };
            ActiveAgentItem {
                id: r.id,
                name: r.name,
                handle: r.handle,
                instructions: r.instructions,
                ai_engine: r.ai_engine,
                ai_model: r.ai_model,
                provider_model: r.provider_model,
                ai_source: ai_source.to_string(),
                effective_model,
                key_prefix: r.key_prefix,
                last_seen_at: r.last_seen_at,
                status: if quiet {
                    "quiet".to_string()
                } else {
                    "active".to_string()
                },
                quiet,
                created_at: r.created_at,
            }
        })
        .collect();

    if wants_html(&headers) {
        return Ok(Redirect::to("/settings/agents").into_response());
    }

    Ok(Json(json!({ "agents": agents })).into_response())
}

pub async fn agent_current_step(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Response, AppError> {
    let agent_id = match caller {
        Caller::Agent(ref a) => a.id,
        Caller::User(_) => return Err(AppError::Forbidden),
    };

    #[derive(sqlx::FromRow)]
    struct PendingRow {
        run_id: Uuid,
        step_id: Option<Uuid>,
        step_order: i32,
        title: String,
        instructions: String,
        input: Option<String>,
        status: String,
        workflow_title: String,
        thread_id: Option<Uuid>,
    }

    let rows = sqlx::query_as::<_, PendingRow>(
        "SELECT rs.run_id, rs.step_id, rs.step_order, rs.title, rs.instructions,
                rs.input, rs.status, p.title AS workflow_title, r.thread_id
         FROM run_steps rs
         JOIN runs r ON r.id = rs.run_id AND r.deleted_at IS NULL
         JOIN procedures p ON p.id = r.procedure_id AND p.deleted_at IS NULL
         WHERE rs.agent_id = $1
           AND rs.status IN ('dispatched', 'waiting')
           AND rs.deleted_at IS NULL
         ORDER BY rs.created_at ASC",
    )
    .bind(agent_id)
    .fetch_all(&state.pool)
    .await?;

    let pending: Vec<PendingAgentStepItem> = rows
        .into_iter()
        .map(|r| PendingAgentStepItem {
            run_id: r.run_id,
            step_id: r.step_id,
            step_order: r.step_order,
            title: r.title,
            instructions: r.instructions,
            input: r.input,
            status: r.status,
            workflow_title: r.workflow_title,
            thread_id: r.thread_id,
        })
        .collect();

    let first = pending.first().cloned();
    Ok(Json(json!({
        "step": first,
        "pending_steps": pending,
    }))
    .into_response())
}

pub async fn agent_heartbeat(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Response, AppError> {
    let agent = match caller {
        Caller::Agent(a) => a,
        Caller::User(_) => return Err(AppError::Forbidden),
    };

    let pending_count: i64 = sqlx::query_scalar(
        "SELECT COALESCE(count(*), 0) FROM run_steps
         WHERE agent_id = $1 AND status IN ('dispatched', 'waiting') AND deleted_at IS NULL",
    )
    .bind(agent.id)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(json!({
        "status": "online",
        "agent_id": agent.id,
        "agent_name": agent.name,
        "pending_steps": pending_count,
    }))
    .into_response())
}

// ---------- Handlers: Workspace Editing ----------

pub async fn edit_board(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(name): Path<String>,
    JsonOrForm(body): JsonOrForm<super::web::CreateBoardRequest>,
) -> Result<Response, AppError> {
    let user_id = caller.user_id();
    let board =
        sqlx::query_as::<_, Board>("SELECT * FROM boards WHERE name = $1 AND deleted_at IS NULL")
            .bind(name.trim().trim_start_matches('/'))
            .fetch_optional(&state.pool)
            .await?
            .ok_or(AppError::NotFound)?;

    if board.user_id != Some(user_id) {
        return Err(AppError::Forbidden);
    }

    let description = if body.description.trim().is_empty() {
        board.description
    } else {
        body.description.trim().to_string()
    };
    let visibility = body.visibility.unwrap_or(board.visibility);

    let updated = sqlx::query_as::<_, Board>(
        "UPDATE boards
         SET description = $1, visibility = $2, updated_at = now()
         WHERE id = $3
         RETURNING *",
    )
    .bind(description)
    .bind(visibility)
    .bind(board.id)
    .fetch_one(&state.pool)
    .await?;

    if wants_html(&headers) {
        return Ok(Redirect::to(&format!("/b/{}", updated.name)).into_response());
    }

    Ok(Json(json!({ "board": updated })).into_response())
}
