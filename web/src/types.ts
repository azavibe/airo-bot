export interface WorkspaceWallet {
  workspace_id: string;
  plan_tier: string;
  credit_balance: number;
  max_daily_credit_burn: number;
  credits_burned_today: number;
  dedicated_phone_number?: string;
  updated_at: string;
  /** Set when the next platform-paid AI call would be refused. */
  limit_reached?: 'daily' | 'balance' | null;
  upgrade_url?: string | null;
  /** Answers with web access used today, and the daily allowance (0: off). */
  web_runs_today?: number;
  web_runs_limit?: number;
}

export interface Thread {
  id: string;
  title: string;
  user_id: string;
  workspace_id?: string | null;
  /** `office`: the office's group chat; `room`: one bot's room. */
  kind?: 'room' | 'office';
  /** The bots that answer in this room. */
  agent_ids?: string[];
  /** Archived: listed under "Archived" in its office until restored. */
  archived_at?: string | null;
  state: string;
  created_at: string;
  updated_at: string;
}

export interface Note {
  id: string;
  thread_id: string;
  body: string;
  author_user_id?: string;
  author_agent_id?: string;
  author_name?: string;
  author_kind: 'human' | 'agent';
  created_at: string;
  /** One-tap buttons a bot offers with this note. */
  actions?: NoteAction[] | null;
  /** Live app record cards: references the app draws from the records. */
  cards?: NoteCardRef[] | null;
}

/** A conversion rule of an app: a record in `when_stage` can become a
 *  record in `to_app` (`POST /api/office/records/:id/convert`). */
export interface Conversion {
  label: string;
  to_app: string;
  when_stage?: string;
}

/** A reference to an app record a message added or changed. The card is
 *  drawn from the record as it is now, never from a copy. */
export interface NoteCardRef {
  type: 'app_record_card';
  app_slug: string;
  record_id: string;
  /** `created`, `updated` (applied) or `proposed_update` (waits for OK). */
  action: 'created' | 'updated' | 'proposed_update' | string;
  /** The record's title when the message was written (for tombstones). */
  title?: string;
  changed_fields?: string[];
  approval_id?: string;
}

/** A record as `GET /api/office/records` returns it: live, or a tombstone. */
export interface RecordView {
  id: string;
  app_slug?: string;
  app_title?: string;
  record_type?: string;
  status?: string;
  data?: Record<string, any>;
  contact?: { id: string; name: string | null } | null;
  created_by_agent_id?: string | null;
  updated_at?: string;
  /** Set when the record is deleted, or its app archived or deleted. */
  gone?: 'deleted' | 'archived' | null;
  /** Changes a bot proposed that wait for the owner's OK. */
  pending?: { approval_id: string; changes: Record<string, any> | null; requested_by: string; created_at: string }[];
}

/** `send`: sends `message` as your own message; `setup`: opens office
 *  setup for this office; `open`: opens the screen named by `target`
 *  (a tab, or the `account` / `connections` section of Settings);
 *  `forget`: forgets the memory whose id is `target` (a bot's Undo). */
export interface NoteAction {
  label: string;
  kind: 'send' | 'setup' | 'open' | 'forget' | 'keep_milestone' | 'start_job' | 'stop_job' | 'doc_outdated';
  message?: string;
  target?: string;
}

export interface AppSchema {
  version: string;
  layout: 'split_view' | 'table_view' | 'media_view';
  header_metrics?: Array<{
    id: string;
    label: string;
    query_record_type: string;
    aggregation: 'count' | 'sum' | 'avg';
    field?: string;
  }>;
  views: Array<{
    id: string;
    type: 'kanban' | 'datatable' | 'media_gallery' | 'chart';
    title: string;
    record_type: string;
    group_by_field?: string;
    columns?: any[];
    card_fields?: string[];
    media_field?: string;
    caption_field?: string;
    metrics_fields?: string[];
    /** chart: `bar` or `spline`, the field along x, and the number summed
     *  (or "count"). */
    chart_type?: 'bar' | 'spline';
    x_axis?: string;
    y_axis?: string;
    order?: string[];
  }>;
  /** Stages a bot may only move a record into with the owner's OK. */
  approval_stages?: string[];
  /** Ways a record becomes one in another app (a quote into an invoice). */
  convert?: Conversion[];
  /** Buttons that ask the office's team to do a job; `ask` is the message
   *  sent to the office chat (the label when absent). */
  quick_actions?: Array<{
    label: string;
    ask?: string;
    trigger_workflow_slug?: string;
  }>;
}

export interface OfficeApp {
  id: string;
  workspace_id: string;
  slug: string;
  title: string;
  icon: string;
  category: string;
  replaces_saas?: string;
  ui_schema: AppSchema;
  default_schema?: AppSchema;
  last_valid_schema?: AppSchema;
  /** Archived: hidden from the office and its bots until restored. */
  archived_at?: string | null;
  created_at: string;
  updated_at: string;
}

export interface WorkspaceLesson {
  id: string;
  workspace_id: string;
  rule: string;
  source: string;
  context: Record<string, any>;
  created_at: string;
  updated_at: string;
}

export interface AgentTaskPlan {
  id: string;
  title: string;
  status: 'queued' | 'running' | 'waiting_approval' | 'completed' | 'failed';
  fsm_state?: string;
  steps: Array<{
    order: number;
    title: string;
    status: 'completed' | 'running' | 'pending';
  }>;
}

export interface OfficeRecord {
  id: string;
  workspace_id: string;
  app_id: string;
  record_type: string;
  status: string;
  data: Record<string, any>;
  content_hash?: number;
  created_by_agent_id?: string;
  created_at: string;
  updated_at: string;
}

export interface Approval {
  id: string;
  task_id?: string;
  action_summary: string;
  reason: string;
  estimated_credits: number;
  status: 'pending' | 'approved' | 'rejected';
  agent_name?: string;
  created_at: string;
}

export interface Agent {
  id: string;
  name: string;
  handle?: string;
  instructions?: string;
  key_prefix: string;
  last_seen_at?: string;
  provider_type?: string;
  provider_model?: string;
  /** Desktop app: the AI tool on this computer this bot uses (null = default). */
  ai_engine?: LocalEngineId | null;
  /** Desktop app: the model inside that tool (null = the tool's default). */
  ai_model?: string | null;
  /** Where its answers come from (see the server's ActiveAgentItem). */
  ai_source?: 'this_computer' | 'own_key' | 'account_key' | 'included' | 'none';
  /** Website: the model it really answers with. */
  effective_model?: string | null;
}

/** A separate place for rooms and bots (a workspace on the server). */
export interface Office {
  id: string;
  name: string;
  /** The first office: can be renamed, never deleted. */
  primary: boolean;
  room_count: number;
  /** The Architect plans each morning and the bots start on it. */
  daily_plan?: boolean;
  /** Stopped: the office's bots start no new work. */
  paused?: boolean;
  /** What the business is ("Dental clinic"), as set up or edited. */
  business_type?: string | null;
  website?: string | null;
  focus?: string | null;
  /** Where the business is going; every bot works toward it. */
  goal?: string | null;
  /** What it is like: hours, prices, tone, policies. */
  about?: string | null;
  /** Hard monthly spending cap in credits; none when null. */
  monthly_cap_credits?: number | null;
  /** Credits its paid AI calls cost this month. */
  month_spent_credits?: number;
  /** The cap stopped paid work this month (until raised or the 1st). */
  cap_reached?: boolean;
}

/** Something an office's bots answer from: a file, pasted text or a
 *  web page, kept as text. */
export interface OfficeDocument {
  id: string;
  workspace_id: string;
  title: string;
  kind: 'file' | 'text' | 'page';
  filename?: string | null;
  source_url?: string | null;
  chars: number;
  created_at: string;
  updated_at: string;
  /** Marked outdated: bots don't see or cite it. */
  outdated_at?: string | null;
  /** Website pages: when last read again, and why that failed. */
  synced_at?: string | null;
  sync_error?: string | null;
}

/** Which tab the Edit office dialog opens on. */
export type EditOfficeTab = 'profile' | 'goal' | 'memory' | 'documents';

/** A fact an office remembers: the whole office's, or one bot's. */
export interface Memory {
  id: string;
  workspace_id: string;
  /** Set: only this bot knows it. */
  agent_id?: string | null;
  body: string;
  /** Who saved it: the owner, or a bot (shown in chat with Undo). */
  source: 'owner' | 'bot';
  created_by_agent_id?: string | null;
  pinned: boolean;
  created_at: string;
  updated_at: string;
}

/** A rule learned from the owner's approval decisions. */
export interface Lesson {
  id: string;
  rule: string;
  source: string;
  created_at: string;
}

export type LocalEngineId = 'claude_code' | 'codex' | 'opencode' | 'gemini' | 'ollama';

export type Visibility = 'private' | 'unlisted' | 'public';

export interface Procedure {
  id: string;
  user_id?: string;
  visibility?: Visibility;
  slug: string;
  title: string;
  summary: string;
  body: string;
  check_description: string;
  current_version: number;
  run_count: number;
  success_count: number;
  updated_at: string;
}

export interface UserProfile {
  authenticated: boolean;
  id?: string;
  handle?: string;
  email?: string;
  display_name?: string;
  avatar_url?: string;
  is_admin?: boolean;
  is_agent?: boolean;
  ui_theme?: Record<string, unknown> | null;
  /** Airobot running inside the desktop app, on this computer only. */
  local_mode?: boolean;
}
