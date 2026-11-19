-- Platform admin and the onboarding tour.
--
-- is_admin gates /admin (platform management: onboarding mode, members,
-- workers). Bootstrap rule (src/db.rs): the first account on a fresh
-- install becomes admin, and installs that predate this migration get
-- their earliest account promoted exactly once.
--
-- onboarding_done_at records that the guided tour was dismissed under
-- onboarding_mode = 'once'; mode 'every' shows it on every visit (useful
-- while demonstrating the product) and 'never' disables it.
ALTER TABLE users ADD COLUMN IF NOT EXISTS is_admin BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE users ADD COLUMN IF NOT EXISTS onboarding_done_at TIMESTAMPTZ;

CREATE TABLE IF NOT EXISTS app_settings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    key TEXT NOT NULL UNIQUE,
    value TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
