-- Migration 0035: Airo multi-tenant coordination core
-- Adds workspaces, workspace_members, groups, tasks, nodes, leases,
-- context_packs, approvals, ratings, and audit_events, while linking
-- existing threads, procedures, runs, and agents into workspace isolation.

CREATE TABLE IF NOT EXISTS workspaces (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    slug TEXT NOT NULL,
    description TEXT,
    owner_user_id UUID NOT NULL REFERENCES users (id),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS workspaces_owner_slug_idx ON workspaces (owner_user_id, slug) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS workspaces_owner_idempotency_idx ON workspaces (owner_user_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS workspace_members (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    user_id UUID NOT NULL REFERENCES users (id),
    role TEXT NOT NULL DEFAULT 'member',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS workspace_members_unique_idx ON workspace_members (workspace_id, user_id) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS workspace_members_user_idx ON workspace_members (user_id) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS groups (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    name TEXT NOT NULL,
    slug TEXT NOT NULL,
    description TEXT,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS groups_workspace_slug_idx ON groups (workspace_id, slug) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS groups_workspace_idx ON groups (workspace_id) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS nodes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    name TEXT NOT NULL,
    node_type TEXT NOT NULL DEFAULT 'agent',
    protocol TEXT NOT NULL DEFAULT 'http',
    status TEXT NOT NULL DEFAULT 'idle',
    owner_type TEXT NOT NULL DEFAULT 'user',
    owner_id UUID,
    capabilities JSONB NOT NULL DEFAULT '[]'::jsonb,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    last_seen_at TIMESTAMPTZ,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS nodes_workspace_status_idx ON nodes (workspace_id, node_type, status) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS nodes_workspace_idx ON nodes (workspace_id) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS tasks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    group_id UUID REFERENCES groups (id),
    thread_id UUID REFERENCES threads (id),
    title TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    priority TEXT NOT NULL DEFAULT 'normal',
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_by_actor_type TEXT NOT NULL DEFAULT 'user',
    created_by_actor_id UUID,
    assigned_agent_id UUID REFERENCES agents (id),
    assigned_node_id UUID REFERENCES nodes (id),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS tasks_workspace_status_idx ON tasks (workspace_id, status, created_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS tasks_workspace_agent_idx ON tasks (workspace_id, assigned_agent_id, status) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS tasks_thread_idx ON tasks (thread_id) WHERE thread_id IS NOT NULL AND deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS tasks_idempotency_idx ON tasks (workspace_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS leases (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    task_id UUID NOT NULL REFERENCES tasks (id),
    holder_actor_type TEXT NOT NULL DEFAULT 'agent',
    holder_actor_id UUID NOT NULL,
    node_id UUID REFERENCES nodes (id),
    resource_key TEXT,
    starts_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    budget JSONB NOT NULL DEFAULT '{}'::jsonb,
    permissions JSONB NOT NULL DEFAULT '{}'::jsonb,
    safety_class TEXT NOT NULL DEFAULT 'standard',
    status TEXT NOT NULL DEFAULT 'active',
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS leases_task_status_idx ON leases (task_id, status) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS leases_workspace_idx ON leases (workspace_id, status) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS leases_active_expires_idx ON leases (expires_at) WHERE status = 'active' AND deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS context_packs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    task_id UUID NOT NULL REFERENCES tasks (id),
    agent_id UUID REFERENCES agents (id),
    hash TEXT NOT NULL,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    tool_allowlist JSONB NOT NULL DEFAULT '[]'::jsonb,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS context_packs_task_hash_idx ON context_packs (task_id, hash) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS context_packs_workspace_idx ON context_packs (workspace_id) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS approvals (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    task_id UUID NOT NULL REFERENCES tasks (id),
    run_id UUID REFERENCES runs (id),
    requested_action TEXT NOT NULL,
    risk_level TEXT NOT NULL DEFAULT 'low',
    status TEXT NOT NULL DEFAULT 'pending',
    requested_by TEXT NOT NULL,
    decided_by UUID REFERENCES users (id),
    decided_at TIMESTAMPTZ,
    reason TEXT,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS approvals_workspace_status_idx ON approvals (workspace_id, status, created_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS approvals_task_idx ON approvals (task_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS approvals_idempotency_idx ON approvals (workspace_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS ratings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    run_id UUID REFERENCES runs (id),
    agent_id UUID REFERENCES agents (id),
    procedure_id UUID REFERENCES procedures (id),
    stars INT NOT NULL DEFAULT 5 CHECK (stars >= 1 AND stars <= 5),
    success BOOLEAN NOT NULL DEFAULT true,
    reason_codes TEXT[] NOT NULL DEFAULT '{}',
    comments TEXT,
    created_by UUID REFERENCES users (id),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS ratings_agent_idx ON ratings (agent_id, created_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS ratings_procedure_idx ON ratings (procedure_id) WHERE procedure_id IS NOT NULL AND deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS audit_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID REFERENCES workspaces (id),
    group_id UUID REFERENCES groups (id),
    task_id UUID REFERENCES tasks (id),
    run_id UUID REFERENCES runs (id),
    actor_type TEXT NOT NULL,
    actor_id UUID,
    action TEXT NOT NULL,
    target_type TEXT,
    target_id UUID,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    result TEXT NOT NULL DEFAULT 'ok',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS audit_events_workspace_idx ON audit_events (workspace_id, created_at DESC);
CREATE INDEX IF NOT EXISTS audit_events_task_idx ON audit_events (task_id) WHERE task_id IS NOT NULL;

-- Link existing domain tables to workspaces
ALTER TABLE threads ADD COLUMN IF NOT EXISTS workspace_id UUID REFERENCES workspaces (id);
CREATE INDEX IF NOT EXISTS threads_workspace_idx ON threads (workspace_id, updated_at DESC) WHERE deleted_at IS NULL;

ALTER TABLE procedures ADD COLUMN IF NOT EXISTS workspace_id UUID REFERENCES workspaces (id);
CREATE INDEX IF NOT EXISTS procedures_workspace_idx ON procedures (workspace_id, created_at DESC) WHERE deleted_at IS NULL;

ALTER TABLE runs ADD COLUMN IF NOT EXISTS workspace_id UUID REFERENCES workspaces (id);
ALTER TABLE runs ADD COLUMN IF NOT EXISTS task_id UUID REFERENCES tasks (id);
CREATE INDEX IF NOT EXISTS runs_workspace_idx ON runs (workspace_id, created_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS runs_task_idx ON runs (task_id) WHERE task_id IS NOT NULL AND deleted_at IS NULL;

ALTER TABLE agents ADD COLUMN IF NOT EXISTS workspace_id UUID REFERENCES workspaces (id);
ALTER TABLE agents ADD COLUMN IF NOT EXISTS node_id UUID REFERENCES nodes (id);
CREATE INDEX IF NOT EXISTS agents_workspace_idx ON agents (workspace_id) WHERE deleted_at IS NULL;

-- Seed default workspaces for existing users who already exist
INSERT INTO workspaces (id, name, slug, owner_user_id)
SELECT gen_random_uuid(), 'Default Workspace', 'default', id
FROM users
ON CONFLICT DO NOTHING;

INSERT INTO workspace_members (workspace_id, user_id, role)
SELECT w.id, w.owner_user_id, 'owner'
FROM workspaces w
ON CONFLICT DO NOTHING;

-- Backfill existing threads, procedures, runs, agents into owner's default workspace
UPDATE threads t
SET workspace_id = w.id
FROM workspaces w
WHERE t.user_id = w.owner_user_id AND t.workspace_id IS NULL;

UPDATE procedures p
SET workspace_id = w.id
FROM workspaces w
WHERE p.user_id = w.owner_user_id AND p.workspace_id IS NULL;

UPDATE runs r
SET workspace_id = w.id
FROM workspaces w
WHERE r.user_id = w.owner_user_id AND r.workspace_id IS NULL;

UPDATE agents a
SET workspace_id = w.id
FROM workspaces w
WHERE a.user_id = w.owner_user_id AND a.workspace_id IS NULL;
