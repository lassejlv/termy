-- Stopping a sandbox captures a named disk checkpoint so the next session
-- boots from it instead of re-cloning and re-running setup.
ALTER TABLE projects ADD COLUMN IF NOT EXISTS checkpoint_key TEXT;
