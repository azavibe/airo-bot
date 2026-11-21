-- 0060_spend_caps.sql
-- Hard monthly spending caps, in credits: one per office and, optionally,
-- one per bot. Every metered AI call is checked against them before it is
-- made, and a call that would go past a cap is refused. When an office
-- reaches its cap, its autonomous work (morning plans, queued tasks) waits
-- until the next month or until the owner raises the cap, and the office
-- chat says so once (cap_notice_month: the month the cap was reached; it is
-- cleared when the cap changes). The ledger now records which office and
-- which bot each charge was for, so spend can be shown per office and per
-- bot.

ALTER TABLE workspaces
    ADD COLUMN IF NOT EXISTS monthly_cap_credits BIGINT
        CHECK (monthly_cap_credits IS NULL OR monthly_cap_credits > 0),
    ADD COLUMN IF NOT EXISTS cap_notice_month DATE;

ALTER TABLE agents
    ADD COLUMN IF NOT EXISTS monthly_cap_credits BIGINT
        CHECK (monthly_cap_credits IS NULL OR monthly_cap_credits > 0),
    ADD COLUMN IF NOT EXISTS cap_notice_month DATE;

ALTER TABLE credit_ledger
    ADD COLUMN IF NOT EXISTS office_id UUID REFERENCES workspaces (id),
    ADD COLUMN IF NOT EXISTS agent_id UUID REFERENCES agents (id);

-- Earlier charges were for the workspace that paid them.
UPDATE credit_ledger SET office_id = workspace_id WHERE office_id IS NULL;

CREATE INDEX IF NOT EXISTS credit_ledger_office_month_idx
    ON credit_ledger (office_id, created_at);
CREATE INDEX IF NOT EXISTS credit_ledger_agent_month_idx
    ON credit_ledger (agent_id, created_at)
    WHERE agent_id IS NOT NULL;
