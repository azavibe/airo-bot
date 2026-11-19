-- 1. Server-Driven Mini-Apps (Dynamic UI Dashboards generated on the fly by agents)
CREATE TABLE IF NOT EXISTS office_apps (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    slug TEXT NOT NULL,
    title TEXT NOT NULL,
    icon TEXT NOT NULL DEFAULT 'LayoutDashboard',
    category TEXT NOT NULL DEFAULT 'crm', -- 'crm', 'marketing', 'finance', 'operations'
    replaces_saas TEXT,                   -- e.g., 'HubSpot', 'GoHighLevel', 'Buffer', 'Xero'
    ui_schema JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(workspace_id, slug)
);

-- 2. Universal Office Records (Stores Leads, Deals, Social Video Queue, Invoices per Mini-App)
CREATE TABLE IF NOT EXISTS office_records (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    app_id UUID NOT NULL REFERENCES office_apps(id) ON DELETE CASCADE,
    record_type TEXT NOT NULL,            -- e.g., 'lead', 'video_post', 'invoice', 'call_log'
    status TEXT NOT NULL DEFAULT 'active',
    data JSONB NOT NULL DEFAULT '{}'::jsonb,
    content_hash BIGINT,                  -- 64-bit FNV-1a or semantic hash to prevent duplicate generation
    created_by_agent_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_office_records_app_status ON office_records(app_id, record_type, status);
CREATE INDEX IF NOT EXISTS idx_office_records_data_gin ON office_records USING GIN (data);

-- 3. Workspace Compute Wallets & Credit Ledger ($399/mo Subscription + API Metering)
CREATE TABLE IF NOT EXISTS workspace_wallets (
    workspace_id UUID PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
    plan_tier TEXT NOT NULL DEFAULT 'smb_pro_399',
    credit_balance BIGINT NOT NULL DEFAULT 50000,      -- 50,000 monthly credits
    max_daily_credit_burn BIGINT NOT NULL DEFAULT 5000, -- Deterministic safety cap
    credits_burned_today BIGINT NOT NULL DEFAULT 0,
    dedicated_phone_number TEXT,                       -- Reserved for V2 Twilio/SIP provisioning
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS credit_ledger (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    task_id UUID REFERENCES tasks(id) ON DELETE SET NULL,
    provider TEXT NOT NULL,               -- 'openrouter', 'wan3_video', 'zernio_social', 'voice_tts'
    model_or_tool TEXT NOT NULL,
    credits_charged BIGINT NOT NULL,
    raw_cost_micros BIGINT NOT NULL,      -- Actual USD cost in micro-dollars (1/1,000,000 USD)
    idempotency_key TEXT UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 4. Zero-VM Async I/O Checkpoint columns on Tasks
ALTER TABLE tasks
    ADD COLUMN IF NOT EXISTS fsm_state TEXT NOT NULL DEFAULT 'QUEUED',
    -- Valid states: 'QUEUED', 'CLAIMED', 'WAITING_IO', 'WAITING_APPROVAL', 'COMPLETED', 'FAILED'
    ADD COLUMN IF NOT EXISTS external_job_id TEXT,     -- Webhook correlation ID (e.g. Wan 3.0 / Fal / Replicate job ID)
    ADD COLUMN IF NOT EXISTS state_payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN IF NOT EXISTS wake_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_tasks_fsm_wake ON tasks(fsm_state, wake_at)
    WHERE fsm_state IN ('QUEUED', 'WAITING_IO');

-- 5. Self-Learning Engine (EvoAgentX / TextGrad Prompt Evolution History)
CREATE TABLE IF NOT EXISTS procedure_evolutions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    procedure_id UUID NOT NULL REFERENCES procedures(id) ON DELETE CASCADE,
    workspace_id UUID REFERENCES workspaces(id) ON DELETE CASCADE,
    previous_version INT NOT NULL,
    new_version INT NOT NULL,
    pass_rate_before DOUBLE PRECISION NOT NULL,
    evaluation_gradient TEXT NOT NULL,    -- LLM diagnosis of why previous runs failed checks or got low engagement
    mutated_steps_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
