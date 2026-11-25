-- 0072_slack.sql
-- Talk to an office from Slack (issue #33). The owner creates a Slack app
-- from the manifest Airobot shows (its event and button URLs point at
-- this server), installs it and pastes the bot token and signing secret;
-- both are kept only encrypted (crypto::encrypt), since the server uses
-- them with Slack. A one-time code (an Argon2 hash, valid an hour) sent to
-- the app in a direct message links the owner's Slack account: their
-- messages there go to the office's group chat, the bots' answers come
-- back, and approvals arrive with Approve and Reject buttons. Slack calls
-- this server, so it needs a public address (not the desktop app).

CREATE TABLE IF NOT EXISTS slack_apps (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    owner_user_id UUID NOT NULL REFERENCES users (id),
    -- The Slack workspace and the app's bot user.
    team_id TEXT NOT NULL CHECK (char_length(team_id) <= 32),
    team_name TEXT CHECK (team_name IS NULL OR char_length(team_name) <= 200),
    bot_user_id TEXT NOT NULL CHECK (char_length(bot_user_id) <= 32),
    token_encrypted TEXT NOT NULL,
    signing_secret_encrypted TEXT NOT NULL,
    link_code_hash TEXT,
    link_expires_at TIMESTAMPTZ,
    -- The owner's Slack user and their direct-message channel with the app.
    slack_user_id TEXT CHECK (slack_user_id IS NULL OR char_length(slack_user_id) <= 32),
    channel_id TEXT CHECK (channel_id IS NULL OR char_length(channel_id) <= 32),
    linked_at TIMESTAMPTZ,
    approvals_sent_until TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error TEXT CHECK (last_error IS NULL OR char_length(last_error) <= 300),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS slack_apps_office_idx
    ON slack_apps (workspace_id) WHERE deleted_at IS NULL;
