-- Named users a share is for. They read the shared items once signed in; there is no link.
CREATE TABLE share_users (
    share_id   UUID NOT NULL REFERENCES shares (id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    permission TEXT NOT NULL DEFAULT 'read' CHECK (permission IN ('read')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (share_id, user_id)
);
CREATE INDEX share_users_user_idx ON share_users (user_id);
