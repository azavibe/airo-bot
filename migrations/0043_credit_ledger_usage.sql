-- Real usage for metered AI calls. Credits stay a flat per-call charge for
-- now; these columns record what each call actually cost the platform so
-- pricing can be set from data. raw_cost_micros on the charge row is
-- replaced by the provider-reported cost when it is known.
ALTER TABLE credit_ledger
    ADD COLUMN IF NOT EXISTS prompt_tokens BIGINT,
    ADD COLUMN IF NOT EXISTS completion_tokens BIGINT,
    ADD COLUMN IF NOT EXISTS usage_recorded_at TIMESTAMPTZ;
