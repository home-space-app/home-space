-- Administrators manage accounts from the web: disable them, give them a quota and delete them.

-- A disabled user keeps their files but may not sign in; disabling ends their sessions.
ALTER TABLE users ADD COLUMN disabled_at TIMESTAMPTZ;
-- The most a user should store, in bytes; NULL for no quota. Recorded, not enforced yet.
ALTER TABLE users ADD COLUMN quota_bytes BIGINT CHECK (quota_bytes IS NULL OR quota_bytes >= 0);
-- A user marked for deletion may not sign in, and is deleted with all their files by a
-- background job once this time has passed. Clearing it cancels the deletion.
ALTER TABLE users ADD COLUMN delete_at TIMESTAMPTZ;

CREATE INDEX users_delete_at_idx ON users (delete_at) WHERE delete_at IS NOT NULL;
