-- How a session's user proved who they are, with the `amr` names of RFC 8176 (`pwd`, `oidc`,
-- later `otp`, `hwk`, `swk`, `user`), and the last time they did: at sign-in, or when they
-- confirmed it was them for a sensitive change. Refreshing tokens does not move it.
ALTER TABLE sessions ADD COLUMN auth_methods TEXT[] NOT NULL DEFAULT '{pwd}';
ALTER TABLE sessions ADD COLUMN authenticated_at TIMESTAMPTZ;

-- Sessions from before were started with a password, or by single sign-on for users who have
-- none, when they were created.
UPDATE sessions s SET auth_methods = '{oidc}' FROM users u
    WHERE u.id = s.user_id AND u.password_hash IS NULL;
UPDATE sessions SET authenticated_at = created_at;
ALTER TABLE sessions ALTER COLUMN authenticated_at SET NOT NULL;
ALTER TABLE sessions ALTER COLUMN auth_methods DROP DEFAULT;

-- A one-time code carries how its user signed in, so the session it starts can say the same.
-- The codes waiting now were issued by single sign-on or by a web session that signed in
-- with a password; they last a minute, so the first is close enough.
ALTER TABLE login_codes ADD COLUMN auth_methods TEXT[] NOT NULL DEFAULT '{oidc}';
ALTER TABLE login_codes ADD COLUMN authenticated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE login_codes ALTER COLUMN auth_methods DROP DEFAULT;
ALTER TABLE login_codes ALTER COLUMN authenticated_at DROP DEFAULT;
