ALTER TABLE email_tokens ADD COLUMN idempotency_key TEXT;
CREATE UNIQUE INDEX email_tokens_retry_idx ON email_tokens (email, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
ALTER TABLE login_codes ADD COLUMN idempotency_key TEXT;
CREATE UNIQUE INDEX login_codes_retry_idx ON login_codes (idempotency_key)
    WHERE idempotency_key IS NOT NULL;
