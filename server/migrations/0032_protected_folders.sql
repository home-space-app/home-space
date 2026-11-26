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

-- What opens the vault: the user's security keys (sign-in credentials with PRF) and at most one
-- recovery key.
CREATE TABLE vault_unlockers (
    id                UUID PRIMARY KEY,
    user_id           UUID NOT NULL REFERENCES vaults (user_id) ON DELETE CASCADE,
    kind              TEXT NOT NULL CHECK (kind IN ('security_key', 'recovery')),
    -- security_key only: the sign-in credential, which names the key and the RP id. No
    -- ON DELETE: removing a credential that is an unlocker removes the unlocker first, or is
    -- refused when it is the last one.
    credential        UUID UNIQUE REFERENCES user_webauthn (id),
    -- security_key only: the PRF input the key answers for this vault.
    prf_salt          BYTEA CHECK (length(prf_salt) = 32),
    wrapped_vault_key BYTEA NOT NULL CHECK (length(wrapped_vault_key) = 73),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((kind = 'security_key') = (credential IS NOT NULL AND prf_salt IS NOT NULL)),
    CHECK (kind = 'security_key' OR (credential IS NULL AND prf_salt IS NULL))
);

CREATE INDEX vault_unlockers_user ON vault_unlockers (user_id);
CREATE UNIQUE INDEX vault_unlockers_one_recovery ON vault_unlockers (user_id)
    WHERE kind = 'recovery';

-- A folder the user protected, and what its secret is wrapped with.
CREATE TABLE protected_folders (
    node_id        UUID PRIMARY KEY REFERENCES nodes (id) ON DELETE CASCADE,
    owner_id       UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    lock           TEXT NOT NULL CHECK (lock IN ('security_key', 'password')),
    wrapped_secret BYTEA NOT NULL CHECK (length(wrapped_secret) = 73),
    -- security_key only: the vault epoch the secret is wrapped under.
    epoch          INTEGER,
    -- password only: the Argon2id salt and parameters.
    kdf            JSONB,
    -- The folder's age X25519 recipient, "age1...": uploads encrypt to it without a key.
    recipient      TEXT NOT NULL CHECK (recipient ~ '^age1[02-9ac-hj-np-z]{58}$'),
    format         INTEGER NOT NULL DEFAULT 1,
    -- 'protecting' and 'unprotecting' while a client converts an existing folder.
    state          TEXT NOT NULL CHECK (state IN ('protecting', 'protected', 'unprotecting')),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((lock = 'security_key') = (epoch IS NOT NULL AND kdf IS NULL)),
    CHECK (lock = 'security_key' OR (epoch IS NULL AND kdf IS NOT NULL))
);

CREATE INDEX protected_folders_owner ON protected_folders (owner_id);

-- Nodes below a protected folder (not the folder itself). Their name is sealed by the client;
-- `name` then holds the base64url SHA-256 of `sealed_name`, which keeps names unique in a
-- folder without the server knowing them (sealing is deterministic per folder and name).
ALTER TABLE nodes ADD COLUMN sealed BOOLEAN NOT NULL DEFAULT false;
-- AES-SIV: a 16-byte tag and the name padded to a multiple of 32 bytes.
ALTER TABLE nodes ADD COLUMN sealed_name BYTEA CHECK (
    length(sealed_name) BETWEEN 48 AND 1040 AND length(sealed_name) % 32 = 16
);
ALTER TABLE nodes ADD CONSTRAINT nodes_sealed_name CHECK (sealed = (sealed_name IS NOT NULL));
