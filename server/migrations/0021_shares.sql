-- A user's files and folders shared with others. How they reach them lives in its own table:
-- `share_links` for anyone holding a link; sharing with named users comes later.
CREATE TABLE shares (
    id         UUID PRIMARY KEY,
    owner_id   UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- NULL for a share that does not expire.
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX shares_owner_idx ON shares (owner_id);

CREATE TABLE share_items (
    share_id UUID NOT NULL REFERENCES shares (id) ON DELETE CASCADE,
    node_id  UUID NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
    PRIMARY KEY (share_id, node_id)
);
CREATE INDEX share_items_node_idx ON share_items (node_id);

-- Anyone with the token reads the shared items, and everything inside the shared folders,
-- without signing in. The token is kept as it is, not hashed, so that the owner can copy the
-- link again later.
CREATE TABLE share_links (
    share_id         UUID PRIMARY KEY REFERENCES shares (id) ON DELETE CASCADE,
    token            TEXT NOT NULL UNIQUE CHECK (length(token) BETWEEN 32 AND 128),
    -- argon2 PHC string; NULL for a link without a password.
    password_hash    TEXT,
    -- Raised whenever the password changes, so that access granted for an old one ends.
    password_version INT NOT NULL DEFAULT 0,
    permission       TEXT NOT NULL DEFAULT 'read' CHECK (permission IN ('read'))
);
