-- The client version a device last signed in or connected with, and the server features it
-- requires, so the device list can show which devices need an update.
ALTER TABLE devices
    ADD COLUMN client_version text NULL,
    ADD COLUMN client_requires text[] NOT NULL DEFAULT '{}';
