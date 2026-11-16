-- A device's app extensions (a share sheet, a file provider) run in processes of their own and
-- cannot share the app's single-use refresh token, so each gets a session of its own on the
-- same device, named by its label. The app's own session has none.
--
-- An extension's session belongs to the app session that started it and ends with it. The
-- device id is chosen by the client, so it does not say whose extension a session is; the
-- parent does. An app session has at most one session per label.
ALTER TABLE sessions ADD COLUMN label TEXT;
ALTER TABLE sessions
    ADD COLUMN parent_session_id UUID REFERENCES sessions (id) ON DELETE CASCADE;
ALTER TABLE sessions
    ADD CONSTRAINT sessions_label_parent_check
    CHECK ((label IS NULL) = (parent_session_id IS NULL));

CREATE UNIQUE INDEX sessions_parent_label_idx
    ON sessions (parent_session_id, label)
    WHERE parent_session_id IS NOT NULL;
