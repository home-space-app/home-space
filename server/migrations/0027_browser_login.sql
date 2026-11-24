-- The one-time codes a client trades for a session are no longer only for single sign-on: an
-- app that signed in through the server's own web page gets one too.
ALTER TABLE oidc_codes RENAME TO login_codes;
ALTER TABLE login_codes RENAME CONSTRAINT oidc_codes_pkey TO login_codes_pkey;

-- A code is redeemed only on its own tenant, without looking up its user.
ALTER TABLE login_codes ADD COLUMN tenant_id INTEGER;
UPDATE login_codes c SET tenant_id = u.tenant_id FROM users u WHERE u.id = c.user_id;
ALTER TABLE login_codes ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE login_codes DROP CONSTRAINT oidc_codes_user_id_fkey;
ALTER TABLE login_codes ADD CONSTRAINT login_codes_user_fkey FOREIGN KEY (tenant_id, user_id)
    REFERENCES users (tenant_id, id) ON DELETE CASCADE;

-- An app waiting in the system browser for a signed-in user to let it sign in.
CREATE TABLE browser_logins (
    flow_hash        BYTEA PRIMARY KEY,
    tenant_id        INTEGER NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    client_redirect  TEXT NOT NULL,
    client_challenge TEXT NOT NULL,
    device_name      TEXT CHECK (length(device_name) BETWEEN 1 AND 64),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at       TIMESTAMPTZ NOT NULL
);
