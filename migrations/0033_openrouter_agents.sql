-- Migration 0033: Support OpenRouter agent connections and workflow run questionnaires

ALTER TABLE agents ADD COLUMN IF NOT EXISTS provider_type TEXT;
ALTER TABLE agents ADD COLUMN IF NOT EXISTS provider_model TEXT;
ALTER TABLE agents ADD COLUMN IF NOT EXISTS provider_api_key TEXT;

ALTER TABLE runs ADD COLUMN IF NOT EXISTS questionnaire_answers JSONB;
ALTER TABLE runs ADD COLUMN IF NOT EXISTS custom_input TEXT;
