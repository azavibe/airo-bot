-- 1. Update procedures kind check to allow 'workflow'
-- 2. Create workflow_steps table to store ordered steps for procedures/workflows.
-- 3. Extend runs table to support active workflow runs (nullable outcome, nullable finished_at, status, current_step_order).
-- 4. Create run_steps table to track per-step execution, dispatched commands, and returned results.

ALTER TABLE procedures DROP CONSTRAINT IF EXISTS procedures_kind_check;
ALTER TABLE procedures ADD CONSTRAINT procedures_kind_check CHECK (kind IN ('procedure', 'post', 'workflow'));



CREATE TABLE workflow_steps (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    procedure_id UUID NOT NULL REFERENCES procedures (id),
    version INT NOT NULL DEFAULT 1,
    step_order INT NOT NULL,
    title TEXT NOT NULL,
    instructions TEXT NOT NULL,
    agent_id UUID REFERENCES agents (id),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX workflow_steps_procedure_idx ON workflow_steps (procedure_id, version, step_order) WHERE deleted_at IS NULL;
CREATE INDEX workflow_steps_agent_idx ON workflow_steps (agent_id) WHERE deleted_at IS NULL;

ALTER TABLE runs ALTER COLUMN agent_id DROP NOT NULL;
ALTER TABLE runs ALTER COLUMN outcome DROP NOT NULL;
ALTER TABLE runs ALTER COLUMN finished_at DROP NOT NULL;
ALTER TABLE runs ALTER COLUMN duration_ms DROP NOT NULL;
ALTER TABLE runs ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'running';
ALTER TABLE runs ADD COLUMN IF NOT EXISTS current_step_order INT NOT NULL DEFAULT 1;

CREATE INDEX IF NOT EXISTS runs_procedure_idx ON runs (procedure_id, created_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS runs_thread_idx ON runs (thread_id) WHERE deleted_at IS NULL;

CREATE TABLE run_steps (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id UUID NOT NULL REFERENCES runs (id),
    step_id UUID REFERENCES workflow_steps (id),
    step_order INT NOT NULL,
    title TEXT NOT NULL,
    instructions TEXT NOT NULL,
    agent_id UUID REFERENCES agents (id),
    status TEXT NOT NULL DEFAULT 'pending',
    input TEXT,
    output TEXT,
    command_note_id UUID REFERENCES notes (id),
    result_note_id UUID REFERENCES notes (id),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX run_steps_run_idx ON run_steps (run_id, step_order) WHERE deleted_at IS NULL;
CREATE INDEX run_steps_agent_idx ON run_steps (agent_id, status) WHERE deleted_at IS NULL;
