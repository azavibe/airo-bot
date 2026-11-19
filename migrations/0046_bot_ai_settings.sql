-- Which AI a bot thinks with, chosen per bot. In the desktop app (local
-- mode) `ai_engine` picks the tool on the person's computer and `ai_model`
-- the model inside it; NULL means "this computer's default tool" and "the
-- tool's own default model". Cloud chat keeps using `provider_model`.
ALTER TABLE agents
    ADD COLUMN IF NOT EXISTS ai_engine TEXT
        CHECK (ai_engine IN ('claude_code', 'codex', 'opencode', 'gemini', 'ollama')),
    ADD COLUMN IF NOT EXISTS ai_model TEXT
        CHECK (ai_model IS NULL OR char_length(ai_model) BETWEEN 1 AND 200);
