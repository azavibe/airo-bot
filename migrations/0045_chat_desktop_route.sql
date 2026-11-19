-- Answer a person's own chat messages with their desktop bot (the AI tool
-- they have signed in on their computer) instead of the server's AI.
-- Set and cleared by that desktop bot itself (POST /agent/chat-route).
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS chat_desktop_agent_id UUID REFERENCES agents (id);
