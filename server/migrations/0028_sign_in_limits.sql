-- Browser sign-ins waiting on one tenant are counted before another is let in.
CREATE INDEX browser_logins_tenant_idx ON browser_logins (tenant_id, expires_at);

-- An app's address is short; the server checks it is at most 128 bytes before storing it.
ALTER TABLE browser_logins ADD CONSTRAINT browser_logins_client_redirect_len
    CHECK (length(client_redirect) <= 128);
