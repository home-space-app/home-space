-- Failed sign-ins are counted per username and where they came from, so that someone guessing
-- a password slows only themselves down and cannot keep the owner of the username out.
-- The source is the client's address for a sign-in ('' when it is unknown), the device for a
-- sign-in from one the user has used before, and the session for a password change. Rows from
-- before count as source '': they keep blocking only clients whose address is unknown.
ALTER TABLE login_attempts ADD COLUMN source TEXT NOT NULL DEFAULT '';
ALTER TABLE login_attempts DROP CONSTRAINT login_attempts_pkey;
ALTER TABLE login_attempts ADD PRIMARY KEY (username, source);
