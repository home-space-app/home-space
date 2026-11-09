-- Administrators see the server status and, later, manage other accounts. They get no access to
-- other users' files. The first one is made from the CLI: `homespace user admin <username>`.
ALTER TABLE users ADD COLUMN is_admin BOOLEAN NOT NULL DEFAULT false;

-- Storage usage sums previous versions per owner.
CREATE INDEX file_versions_owner_idx ON file_versions (owner_id);
