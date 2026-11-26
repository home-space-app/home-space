-- Which HOMESPACE_SECRETS_KEY a tenant's sealed secrets (authenticator apps, password-folder
-- keys) are sealed with: a known value sealed with that key. The server refuses to start with a
-- key that does not open it, so a key changed by hand cannot leave those secrets unreadable
-- unnoticed. `homespace-ctl secrets rotate` replaces it with the secrets it seals again.
CREATE TABLE secrets_key_checks (
    tenant_id  INTEGER PRIMARY KEY REFERENCES tenants (id) ON DELETE CASCADE,
    sealed     BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
