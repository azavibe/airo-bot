-- 0040_workspace_lessons.sql
-- Kiro Crew Persistent Lessons Memory & Herdr Mod Safe Schema Rollback

-- 1. Persistent Workspace Lessons (learned rules and constraints from rejections & corrections)
CREATE TABLE IF NOT EXISTS workspace_lessons (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    rule TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'approval_rejection', -- 'approval_rejection', 'user_correction', 'manual'
    context JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_workspace_lessons_ws ON workspace_lessons(workspace_id) WHERE deleted_at IS NULL;

-- 2. Mod Schema Fallback & Default Blueprint safety columns
ALTER TABLE office_apps
    ADD COLUMN IF NOT EXISTS default_schema JSONB,
    ADD COLUMN IF NOT EXISTS last_valid_schema JSONB;

-- Seed default_schema and last_valid_schema for existing apps
UPDATE office_apps
SET default_schema = ui_schema,
    last_valid_schema = ui_schema
WHERE default_schema IS NULL;
