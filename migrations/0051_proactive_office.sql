-- A proactive office: bots may use the web (counted per account per day),
-- bot notes may carry one-tap actions, and an office keeps a short profile
-- the team works from (website, focus, business type) and when the
-- Architect last posted its daily plan.

CREATE TABLE IF NOT EXISTS web_usage (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    day DATE NOT NULL DEFAULT CURRENT_DATE,
    runs INTEGER NOT NULL DEFAULT 0 CHECK (runs >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, day)
);

-- One-tap buttons on a note: [{"label": "...", "kind": "send"|"setup"|"open", "message"?: "...", "target"?: "..."}]
ALTER TABLE notes ADD COLUMN IF NOT EXISTS actions JSONB;

ALTER TABLE workspaces
    ADD COLUMN IF NOT EXISTS website TEXT CHECK (website IS NULL OR char_length(website) <= 200),
    ADD COLUMN IF NOT EXISTS focus TEXT CHECK (focus IS NULL OR char_length(focus) <= 200),
    ADD COLUMN IF NOT EXISTS business_type TEXT CHECK (business_type IS NULL OR char_length(business_type) <= 100),
    -- The owner's offset from UTC (minutes east), for "8 in the morning".
    ADD COLUMN IF NOT EXISTS utc_offset_minutes INTEGER NOT NULL DEFAULT 0
        CHECK (utc_offset_minutes BETWEEN -840 AND 840),
    ADD COLUMN IF NOT EXISTS daily_plan BOOLEAN NOT NULL DEFAULT true,
    ADD COLUMN IF NOT EXISTS last_plan_at TIMESTAMPTZ;
