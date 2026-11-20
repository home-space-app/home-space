-- Finding files with the same content: live files by hash, and previous versions by hash.
CREATE INDEX nodes_owner_sha256_idx ON nodes (owner_id, sha256)
    WHERE trashed_at IS NULL AND kind = 'file' AND sha256 IS NOT NULL;
CREATE INDEX file_versions_owner_sha256_idx ON file_versions (owner_id, sha256);
