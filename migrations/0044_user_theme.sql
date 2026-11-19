-- A person's appearance settings (accent, background, panel and border
-- colours, corners, blur). Validated and normalised by PUT /api/me/theme;
-- NULL means the default Obsidian theme.
ALTER TABLE users ADD COLUMN IF NOT EXISTS ui_theme JSONB;
