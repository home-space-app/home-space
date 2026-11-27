-- Protected folders: end-to-end encrypted folders, locked with a security key or a password.
-- The server does no cryptography: it keeps wrapped keys, public keys and ciphertext that only
-- the user's clients can open (see docs/todo/protected-folders.md).

-- The user's vault for folders locked with a security key: its key is random, and every
-- unlocker holds a copy wrapped for it. Created with the first unlocker.
CREATE TABLE vaults (
    user_id    UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    -- Bumped when the vault key changes; unlockers and folder secrets are wrapped under it.
    epoch      INTEGER NOT NULL DEFAULT 1 CHECK (epoch >= 1),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- What opens the vault: the user's folder keys and at most one recovery key.
CREATE TABLE vault_unlockers (
    id                UUID PRIMARY KEY,
    user_id           UUID NOT NULL REFERENCES vaults (user_id) ON DELETE CASCADE,
    kind              TEXT NOT NULL CHECK (kind IN ('security_key', 'recovery')),
    -- security_key only: a folder key, a WebAuthn credential the client made for the vault
    -- alone. It never signs in, so it is not in user_webauthn, even when it is on the same
    -- physical key as one that does. Its name, its credential id and the RP id it was made
    -- for, which a client needs to ask the key.
    name              TEXT CHECK (char_length(name) BETWEEN 1 AND 64),
    credential_id     BYTEA CHECK (length(credential_id) BETWEEN 1 AND 1023),
    rp_id             TEXT CHECK (char_length(rp_id) BETWEEN 1 AND 253),
    -- security_key only: the PRF input the key answers for this vault.
    prf_salt          BYTEA CHECK (length(prf_salt) = 32),
    wrapped_vault_key BYTEA NOT NULL CHECK (length(wrapped_vault_key) = 73),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, credential_id),
    CHECK ((kind = 'security_key') = (name IS NOT NULL AND credential_id IS NOT NULL
                                      AND rp_id IS NOT NULL AND prf_salt IS NOT NULL)),
    CHECK (kind = 'security_key'
           OR (name IS NULL AND credential_id IS NULL AND rp_id IS NULL AND prf_salt IS NULL))
);

CREATE UNIQUE INDEX vault_unlockers_one_recovery ON vault_unlockers (user_id)
    WHERE kind = 'recovery';

-- A folder the user protected.
--
-- security_key: end to end. The client encrypts names and content to the folder's age
-- recipient; the server keeps the folder secret wrapped by the vault and never has a key.
-- password: the server encrypts names and content itself, with a random folder key it keeps
-- sealed with the server's secrets key, and opens it to whoever gives the folder's password.
-- Not end to end.
CREATE TABLE protected_folders (
    node_id        UUID PRIMARY KEY REFERENCES nodes (id) ON DELETE CASCADE,
    owner_id       UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    lock           TEXT NOT NULL CHECK (lock IN ('security_key', 'password')),
    -- The age X25519 recipient ("age1..."): what content is encrypted to without a key, by the
    -- client (security_key) or the server (password).
    recipient      TEXT NOT NULL CHECK (recipient ~ '^age1[02-9ac-hj-np-z]{58}$'),
    -- security_key: the folder secret wrapped with the vault key of `epoch`.
    wrapped_secret BYTEA CHECK (length(wrapped_secret) = 73),
    epoch          INTEGER,
    -- password: the Argon2id hash (PHC string) the password is checked against, the hash of the
    -- folder's recovery key, which resets a forgotten password, and the folder secret sealed
    -- with the server's secrets key.
    password_hash  TEXT,
    recovery_hash  TEXT,
    sealed_key     BYTEA,
    format         INTEGER NOT NULL DEFAULT 1,
    -- 'protecting' and 'unprotecting' while an existing folder's content is replaced.
    state          TEXT NOT NULL CHECK (state IN ('protecting', 'protected', 'unprotecting')),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((lock = 'security_key') = (wrapped_secret IS NOT NULL AND epoch IS NOT NULL)),
    CHECK ((lock = 'password') = (password_hash IS NOT NULL AND recovery_hash IS NOT NULL
                                  AND sealed_key IS NOT NULL)),
    CHECK (lock = 'security_key' OR (wrapped_secret IS NULL AND epoch IS NULL)),
    CHECK (lock = 'password'
           OR (password_hash IS NULL AND recovery_hash IS NULL AND sealed_key IS NULL))
);

CREATE INDEX protected_folders_owner ON protected_folders (owner_id);

-- The sessions that gave a password folder's password, until they lock it or stop using it.
CREATE TABLE protected_unlocks (
    session_id UUID NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    node_id    UUID NOT NULL REFERENCES protected_folders (node_id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (session_id, node_id)
);

CREATE INDEX protected_unlocks_expiry ON protected_unlocks (expires_at);

-- Whether `node` is inside a protected folder of either lock (below one, not the folder
-- itself). Search, duplicates, shares and transfers leave such nodes out.
CREATE FUNCTION in_protected_folder(node UUID) RETURNS boolean
LANGUAGE sql STABLE AS $$
    WITH RECURSIVE up AS (
        SELECT parent_id AS id FROM nodes WHERE id = node
        UNION ALL
        SELECT n.parent_id FROM nodes n JOIN up ON n.id = up.id WHERE n.parent_id IS NOT NULL
    )
    SELECT EXISTS (SELECT 1 FROM up JOIN protected_folders p ON p.node_id = up.id)
$$;

-- In a security-key folder, names and content are ciphertext the client made: names are the
-- base64url text of their AES-SIV sealing, content is age files. The server stores them like
-- any other node and never tells them apart.
