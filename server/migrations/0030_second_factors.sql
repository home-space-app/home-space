-- Two-factor sign-in with an authenticator app (TOTP) and recovery codes.

-- One authenticator app per user. The secret is sealed with the tenant's HOMESPACE_SECRETS_KEY
-- (AES-256-GCM, nonce || ciphertext), so a database dump does not hand out second factors.
CREATE TABLE user_totp (
    tenant_id    INTEGER NOT NULL,
    user_id      UUID PRIMARY KEY,
    secret       BYTEA NOT NULL,
    -- The last time step a code was accepted for; codes of it and earlier steps are refused,
    -- so a code works once.
    last_step    BIGINT NOT NULL DEFAULT 0,
    -- NULL while the user has not yet entered a first code from the app.
    confirmed_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (tenant_id, user_id) REFERENCES users (tenant_id, id) ON DELETE CASCADE
);

-- One-time codes for when the authenticator app is lost, stored as SHA-256.
CREATE TABLE user_recovery_codes (
    tenant_id INTEGER NOT NULL,
    user_id   UUID NOT NULL,
    code_hash BYTEA NOT NULL,
    used_at   TIMESTAMPTZ,
    PRIMARY KEY (user_id, code_hash),
    FOREIGN KEY (tenant_id, user_id) REFERENCES users (tenant_id, id) ON DELETE CASCADE
);

-- A sign-in whose password was right, waiting for the second factor.
CREATE TABLE pending_logins (
    token_hash BYTEA PRIMARY KEY,
    tenant_id  INTEGER NOT NULL,
    user_id    UUID NOT NULL,
    -- Wrong codes so far; a few of them end the sign-in.
    attempts   INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    FOREIGN KEY (tenant_id, user_id) REFERENCES users (tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX pending_logins_expiry ON pending_logins (expires_at);
