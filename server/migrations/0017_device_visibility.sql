-- Who can see a device and send files to it:
--   owner    only the user it belongs to (the default)
--   hidden   nobody, its owner included
--   everyone its owner and every other user
--   users    its owner and the users listed in device_viewers
ALTER TABLE devices
    ADD COLUMN visibility TEXT NOT NULL DEFAULT 'owner'
    CHECK (visibility IN ('owner', 'hidden', 'everyone', 'users'));

-- The other users a device with visibility 'users' is shown to. Rows are kept only while the
-- visibility is 'users'.
CREATE TABLE device_viewers (
    owner_id  UUID NOT NULL,
    device_id UUID NOT NULL,
    viewer_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (owner_id, device_id, viewer_id),
    FOREIGN KEY (owner_id, device_id) REFERENCES devices (user_id, id) ON DELETE CASCADE,
    CHECK (viewer_id <> owner_id)
);
CREATE INDEX device_viewers_viewer_idx ON device_viewers (viewer_id);
