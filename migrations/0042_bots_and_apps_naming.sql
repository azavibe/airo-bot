-- Product vocabulary: "workers" are now "bots" (and "mini-apps" are "apps").
-- Apps need no data change (the tables were already office_apps/office_records).
-- Move the stored values that carried the old word so existing rows keep
-- matching the names the code now uses.

-- Default role for an agent assigned to a board.
ALTER TABLE board_agents ALTER COLUMN role SET DEFAULT 'bot';
UPDATE board_agents SET role = 'bot', updated_at = now() WHERE role = 'worker';

-- The local PC node is looked up by name when it is toggled, so an old row
-- would otherwise be orphaned and a duplicate created.
UPDATE nodes SET name = 'Local PC Bot', updated_at = now() WHERE name = 'Local PC Worker';
