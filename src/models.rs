// These mirror their tables in full via `SELECT *` + `FromRow`, even where
// a given query site only reads a few columns -- simpler than hand-picking
// columns per query, at the cost of some fields not being read yet.
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct User {
    pub id: Uuid,
    pub handle: Option<String>,
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub bio: Option<String>,
    pub is_admin: bool,
    pub onboarding_done_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Session {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Agent {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub handle: Option<String>,
    pub instructions: Option<String>,
    #[serde(skip)]
    pub key_hash: String,
    pub key_prefix: String,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub provider_type: Option<String>,
    pub provider_model: Option<String>,
    #[serde(skip)]
    pub provider_api_key: Option<String>,
    /// Local mode: the tool on this computer this bot uses (NULL = default).
    #[sqlx(default)]
    pub ai_engine: Option<String>,
    /// Local mode: the model inside that tool (NULL = the tool's default).
    #[sqlx(default)]
    pub ai_model: Option<String>,
    pub workspace_id: Option<Uuid>,
    pub node_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LoginCode {
    pub id: Uuid,
    pub code: String,
    pub device_secret_hash: Option<String>,
    pub user_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub agent_name: Option<String>,
    pub approved_at: Option<DateTime<Utc>>,
    pub denied_at: Option<DateTime<Utc>>,
    pub token_delivered_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EmailToken {
    pub id: Uuid,
    pub email: String,
    pub token_hash: String,
    pub purpose: String,
    pub consumed_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Shared across every table the conventions call for it on (threads now,
/// procedures in Phase 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, serde::Deserialize)]
#[sqlx(type_name = "visibility", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Private,
    Unlisted,
    Public,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, serde::Deserialize)]
#[sqlx(type_name = "thread_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ThreadStatus {
    Open,
    Waiting,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, serde::Deserialize)]
#[sqlx(type_name = "note_kind", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum NoteKind {
    Status,
    Command,
    Comment,
    System,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Thread {
    pub id: Uuid,
    pub user_id: Uuid,
    pub title: String,
    pub slug: String,
    pub status: ThreadStatus,
    pub visibility: Visibility,
    pub archived_at: Option<DateTime<Utc>>,
    pub note_count: i32,
    pub last_note_at: Option<DateTime<Utc>>,
    pub board_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    /// `room` (one bot, one-to-one) or `office` (the office's group chat).
    pub kind: String,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Note {
    pub id: Uuid,
    pub seq: i64,
    pub thread_id: Uuid,
    pub author_agent_id: Option<Uuid>,
    pub author_user_id: Option<Uuid>,
    pub body: String,
    pub kind: NoteKind,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    /// One-tap buttons a bot offers with this note (see migration 0051).
    pub actions: Option<serde_json::Value>,
    /// Live app record cards (references, see migration 0064).
    #[sqlx(default)]
    pub cards: Option<serde_json::Value>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Procedure {
    pub id: Uuid,
    pub user_id: Uuid,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub body: String,
    pub check_description: String,
    pub kind: String,
    pub visibility: Visibility,
    pub published_at: Option<DateTime<Utc>>,
    pub current_version: i32,
    pub run_count: i64,
    pub success_count: i64,
    pub scored_run_count: i64,
    pub attempt_count: i64,
    pub comment_count: i64,
    pub board_id: Option<Uuid>,
    pub fork_of_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    #[serde(skip)]
    pub publish_idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct ProcedureVersion {
    pub id: Uuid,
    pub procedure_id: Uuid,
    pub version: i32,
    pub body: String,
    pub changelog: Option<String>,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, serde::Deserialize)]
#[sqlx(type_name = "run_outcome", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum RunOutcome {
    Pass,
    Fail,
    Abandoned,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Run {
    pub id: Uuid,
    pub procedure_id: Uuid,
    pub procedure_version: i32,
    pub agent_id: Option<Uuid>,
    pub user_id: Uuid,
    pub model: String,
    pub outcome: Option<RunOutcome>,
    pub status: String,
    pub current_step_order: i32,
    pub attempts: i32,
    pub duration_ms: Option<i64>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub thread_id: Option<Uuid>,
    pub questionnaire_answers: Option<serde_json::Value>,
    pub custom_input: Option<String>,
    pub workspace_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct WorkflowStep {
    pub id: Uuid,
    pub procedure_id: Uuid,
    pub version: i32,
    pub step_order: i32,
    pub title: String,
    pub instructions: String,
    pub agent_id: Option<Uuid>,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct RunStep {
    pub id: Uuid,
    pub run_id: Uuid,
    pub step_id: Option<Uuid>,
    pub step_order: i32,
    pub title: String,
    pub instructions: String,
    pub agent_id: Option<Uuid>,
    pub status: String,
    pub input: Option<String>,
    pub output: Option<String>,
    pub command_note_id: Option<Uuid>,
    pub result_note_id: Option<Uuid>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Follow {
    pub id: Uuid,
    pub follower_id: Uuid,
    pub followed_user_id: Uuid,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, serde::Deserialize)]
#[sqlx(type_name = "reaction_target", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ReactionTarget {
    Procedure,
    Note,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Reaction {
    pub id: Uuid,
    pub user_id: Uuid,
    pub target_type: ReactionTarget,
    pub target_id: Uuid,
    pub kind: String,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Comment {
    pub id: Uuid,
    pub user_id: Uuid,
    pub procedure_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub body: String,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Notification {
    pub id: Uuid,
    pub user_id: Uuid,
    pub kind: String,
    pub actor_user_id: Option<Uuid>,
    pub target_type: String,
    pub target_id: Uuid,
    pub read_at: Option<DateTime<Utc>>,
    pub pushed_at: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Board {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub user_id: Option<Uuid>,
    pub visibility: Visibility,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct BoardAgent {
    pub id: Uuid,
    pub board_id: Uuid,
    pub agent_id: Uuid,
    pub role: String,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct BoardMembership {
    pub id: Uuid,
    pub user_id: Uuid,
    pub board_id: Uuid,
    pub position: i64,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PushSubscription {
    pub id: Uuid,
    pub user_id: Uuid,
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub user_agent: Option<String>,
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Workspace {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub owner_user_id: Uuid,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Group {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Node {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub handle: Option<String>,
    pub node_type: String,
    pub protocol: String,
    pub status: String,
    pub owner_type: String,
    pub owner_id: Option<Uuid>,
    pub capabilities: serde_json::Value,
    pub metadata: serde_json::Value,
    pub last_seen_at: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub group_id: Option<Uuid>,
    pub thread_id: Option<Uuid>,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub payload: serde_json::Value,
    pub created_by_actor_type: String,
    pub created_by_actor_id: Option<Uuid>,
    pub assigned_agent_id: Option<Uuid>,
    pub assigned_node_id: Option<Uuid>,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    /// `agent` (claimed by an agent) or `server` (run by the in-process
    /// FSM dispatcher). See migration 0041.
    #[sqlx(default)]
    pub executor: String,
    #[sqlx(default)]
    pub fsm_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Lease {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub task_id: Uuid,
    pub holder_actor_type: String,
    pub holder_actor_id: Uuid,
    pub node_id: Option<Uuid>,
    pub resource_key: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub budget: serde_json::Value,
    pub permissions: serde_json::Value,
    pub safety_class: String,
    pub status: String,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct ContextPack {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub task_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub hash: String,
    pub payload: serde_json::Value,
    pub tool_allowlist: serde_json::Value,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Approval {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub task_id: Uuid,
    pub run_id: Option<Uuid>,
    pub requested_action: String,
    pub risk_level: String,
    pub status: String,
    pub requested_by: String,
    pub decided_by: Option<Uuid>,
    pub decided_at: Option<DateTime<Utc>>,
    pub reason: Option<String>,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub group_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub actor_type: String,
    pub actor_id: Option<Uuid>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub payload: serde_json::Value,
    pub result: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct OfficeApp {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub slug: String,
    pub title: String,
    pub icon: String,
    pub category: String,
    pub replaces_saas: Option<String>,
    pub ui_schema: serde_json::Value,
    pub default_schema: Option<serde_json::Value>,
    pub last_valid_schema: Option<serde_json::Value>,
    /// Archived: hidden from the office and its bots until restored.
    pub archived_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct WorkspaceLesson {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub rule: String,
    pub source: String,
    pub context: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct OfficeRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub app_id: Uuid,
    pub record_type: String,
    pub status: String,
    pub data: serde_json::Value,
    pub content_hash: Option<i64>,
    pub created_by_agent_id: Option<Uuid>,
    /// The contact this record is about, when it names one.
    pub contact_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct WorkspaceWallet {
    pub workspace_id: Uuid,
    pub plan_tier: String,
    pub credit_balance: i64,
    pub max_daily_credit_burn: i64,
    pub credits_burned_today: i64,
    pub dedicated_phone_number: Option<String>,
    pub updated_at: DateTime<Utc>,
    /// "daily" or "balance" when the next AI call would be refused, so the
    /// app can show the upgrade message before anyone hits it.
    #[sqlx(skip)]
    pub limit_reached: Option<crate::office::budget_gate::LimitHit>,
    #[sqlx(skip)]
    pub upgrade_url: Option<String>,
    /// Answers with web access used today, and the daily allowance
    /// (0 when web access is off).
    #[sqlx(skip)]
    pub web_runs_today: i64,
    #[sqlx(skip)]
    pub web_runs_limit: i64,
}
