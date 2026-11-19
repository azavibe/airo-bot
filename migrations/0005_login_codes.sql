-- TODO(air): the device flow uses one `code` for both the browser-facing
-- display and the CLI's poll key, unlike GitHub's device flow which splits
-- those into a short user_code and a separate high-entropy device_code.
-- Simpler to implement and matches the build plan's single-column schema;
-- the tradeoff is that knowing the code is enough to poll for the granted
-- key, so it leans on expiry (10 minutes) and rate limiting on
-- /device/token instead of a second secret. Revisit if that's not enough.
CREATE TABLE login_codes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    code TEXT NOT NULL,
    user_id UUID REFERENCES users (id),
    agent_id UUID REFERENCES agents (id),
    -- The name chosen in the browser's approve step. The agent row (and its
    -- key) isn't created until the first successful /device/token poll --
    -- see auth::device -- so the name has nowhere else to live until then.
    agent_name TEXT,
    approved_at TIMESTAMPTZ,
    denied_at TIMESTAMPTZ,
    -- Set the first (and only) time /device/token successfully hands back
    -- the agent key, so "returns the key exactly once" is enforceable.
    token_delivered_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX login_codes_code_key ON login_codes (code);
