-- Run / Stop for an office: while paused, its bots start no new work
-- (no replies, no kickoff tasks, no daily plan). Work already running ends.
ALTER TABLE workspaces ADD COLUMN IF NOT EXISTS paused BOOLEAN NOT NULL DEFAULT false;
