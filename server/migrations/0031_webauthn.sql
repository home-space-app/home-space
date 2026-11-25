-- Passkeys and security keys (WebAuthn).

-- A passkey signs in on its own (discoverable, the user verified by PIN or biometrics); a
-- security key only replaces the code of the authenticator app after the password.
CREATE TABLE user_webauthn (
    id            UUID PRIMARY KEY,
    tenant_id     INTEGER NOT NULL,
    user_id       UUID NOT NULL,
    credential_id BYTEA NOT NULL,
    -- The webauthn-rs credential: public key, counter, backup flags. Nothing secret.
    credential    JSONB NOT NULL,
    kind          TEXT NOT NULL CHECK (kind IN ('passkey', 'security_key')),
    -- The host the credential was made for; browsers only offer it there.
    rp_id         TEXT NOT NULL,
    name          TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at  TIMESTAMPTZ,
    UNIQUE (tenant_id, credential_id),
    FOREIGN KEY (tenant_id, user_id) REFERENCES users (tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX user_webauthn_user ON user_webauthn (user_id);

-- The random handle a passkey carries to name its user, instead of the user's id or name. Set
-- when the first key is added.
ALTER TABLE users ADD COLUMN webauthn_handle UUID UNIQUE;

-- A sign-in waiting for its second factor also keeps the security key challenge it was sent.
ALTER TABLE pending_logins ADD COLUMN webauthn_state JSONB;
ALTER TABLE pending_logins ADD COLUMN webauthn_origin TEXT;

-- A WebAuthn ceremony between its two requests: adding a key, a passkey sign-in (no user yet),
-- or confirming it is the user. Used once.
CREATE TABLE webauthn_ceremonies (
    id         UUID PRIMARY KEY,
    tenant_id  INTEGER NOT NULL,
    user_id    UUID,
    kind       TEXT NOT NULL CHECK (kind IN ('add_passkey', 'add_security_key', 'sign_in', 'reauthenticate')),
    -- The page that started it; the answer must come from the same one.
    origin     TEXT NOT NULL,
    state      JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    FOREIGN KEY (tenant_id, user_id) REFERENCES users (tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX webauthn_ceremonies_expiry ON webauthn_ceremonies (expires_at);
