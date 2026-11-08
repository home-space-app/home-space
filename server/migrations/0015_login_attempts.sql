-- Failed password sign-ins per submitted username, whether or not such a user exists, so that
-- guessing is slowed down without telling which usernames are real. A correct password
-- deletes the row.
CREATE TABLE login_attempts (
    username       TEXT PRIMARY KEY,
    -- Attempts since the last correct password; one in progress already counts.
    failures       INTEGER NOT NULL DEFAULT 0,
    -- No attempt is checked before this time.
    blocked_until  TIMESTAMPTZ,
    last_failed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
