-- Files and folders sent to a device. The bytes live in `<storage_root>/transfers/<id>/<item_id>`,
-- a copy taken when the transfer was made; they are removed once the recipient downloads, saves
-- or declines it, or the sender cancels it. The row itself is removed at `expires_at`.
CREATE TABLE transfers (
    id               UUID PRIMARY KEY,
    sender_id        UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- NULL for clients that do not name their device. No foreign key: the transfer outlives a
    -- forgotten sending device.
    sender_device    UUID,
    recipient_id     UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    recipient_device UUID NOT NULL,
    state            TEXT NOT NULL DEFAULT 'uploading' CHECK (
        state IN ('uploading', 'ready', 'declined', 'downloaded', 'saved', 'cancelled')
    ),
    total_bytes      BIGINT NOT NULL CHECK (total_bytes >= 0),
    item_count       INT NOT NULL CHECK (item_count > 0),
    message          TEXT CHECK (length(message) <= 1000),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    ready_at         TIMESTAMPTZ,
    expires_at       TIMESTAMPTZ NOT NULL,
    -- When it was downloaded, saved, declined or cancelled.
    finished_at      TIMESTAMPTZ,
    FOREIGN KEY (recipient_id, recipient_device) REFERENCES devices (user_id, id) ON DELETE CASCADE
);
CREATE INDEX transfers_recipient_idx ON transfers (recipient_id, state);
CREATE INDEX transfers_sender_idx ON transfers (sender_id);
CREATE INDEX transfers_expires_idx ON transfers (expires_at);

CREATE TABLE transfer_items (
    id          UUID PRIMARY KEY,
    transfer_id UUID NOT NULL REFERENCES transfers (id) ON DELETE CASCADE,
    -- Relative and '/'-separated, checked by the server: 'Photos/2026/a.jpg'.
    path        TEXT NOT NULL CHECK (length(path) BETWEEN 1 AND 4096),
    kind        TEXT NOT NULL CHECK (kind IN ('file', 'folder')),
    size        BIGINT NOT NULL DEFAULT 0 CHECK (size >= 0),
    -- Set once a file's bytes are in.
    sha256      BYTEA,
    mime        TEXT,
    modified_at TIMESTAMPTZ,
    -- The node of the sender's tree it was copied from, if any.
    node_id     UUID,
    UNIQUE (transfer_id, path)
);

-- The transfer item a resumable upload brings the bytes of.
ALTER TABLE uploads
    ADD COLUMN transfer_item_id UUID REFERENCES transfer_items (id) ON DELETE CASCADE;
CREATE INDEX uploads_transfer_item_idx ON uploads (transfer_item_id);
