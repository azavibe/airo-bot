-- 0071_telegram.sql
-- Talk to an office from Telegram (issue #33). The owner makes a bot with
-- @BotFather and pastes its token; the token is kept only encrypted
-- (crypto::encrypt), since the server must present it to Telegram. A
-- one-time code (kept as an Argon2 hash, valid an hour) links the owner's
-- own Telegram chat: messages there go to the office's group chat, the
-- bots' answers come back, and approvals arrive with Approve and Reject
-- buttons. The server asks Telegram for new messages itself (long
-- polling), so no public address is needed.

CREATE TABLE IF NOT EXISTS telegram_bots (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    owner_user_id UUID NOT NULL REFERENCES users (id),
    -- Telegram's id and @username for the bot.
    bot_id BIGINT NOT NULL,
    bot_username TEXT NOT NULL CHECK (char_length(bot_username) <= 64),
    token_encrypted TEXT NOT NULL,
    link_code_hash TEXT,
    link_expires_at TIMESTAMPTZ,
    -- The owner's chat with the bot, once linked.
    chat_id BIGINT,
    linked_at TIMESTAMPTZ,
    -- The next Telegram update to ask for.
    update_offset BIGINT NOT NULL DEFAULT 0,
    -- Approvals created up to here were already sent.
    approvals_sent_until TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error TEXT CHECK (last_error IS NULL OR char_length(last_error) <= 300),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

-- One Telegram bot per office, and a bot serves one office (Telegram
-- gives each bot's messages to one reader).
CREATE UNIQUE INDEX IF NOT EXISTS telegram_bots_office_idx
    ON telegram_bots (workspace_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS telegram_bots_bot_idx
    ON telegram_bots (bot_id) WHERE deleted_at IS NULL;
