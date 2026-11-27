-- Password folders become end to end, like security-key folders: the client derives a key from
-- the folder's password (Argon2id) and wraps the folder secret with it, and with the folder's own
-- recovery key. The server keeps both wrapped copies and the Argon2id parameters, never the
-- password, a hash of it, or a key: it no longer encrypts, unlocks or converts these folders.
--
-- Password folders made before were encrypted by the server with a key this drops, so nothing
-- could open them again; protected folders were never released, so they are dropped with it.

DROP TABLE protected_unlocks;

CREATE TEMPORARY TABLE kept_protections ON COMMIT DROP AS
    SELECT node_id, owner_id, recipient, wrapped_secret, epoch, format, state, created_at
    FROM protected_folders WHERE lock = 'security_key';

DROP TABLE protected_folders;

-- A folder the user protected. The client encrypts names and content to the folder's age
-- recipient; the server keeps the folder secret only wrapped, and never has a key.
CREATE TABLE protected_folders (
    node_id                 UUID PRIMARY KEY REFERENCES nodes (id) ON DELETE CASCADE,
    owner_id                UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    lock                    TEXT NOT NULL CHECK (lock IN ('security_key', 'password')),
    -- The age X25519 recipient ("age1..."): what clients encrypt content to without a key.
    recipient               TEXT NOT NULL CHECK (recipient ~ '^age1[02-9ac-hj-np-z]{58}$'),
    -- The folder secret, wrapped with the vault key of `epoch` (security_key) or with the key
    -- the client derives from the folder's password (password).
    wrapped_secret          BYTEA NOT NULL CHECK (length(wrapped_secret) = 73),
    epoch                   INTEGER,
    -- password: the folder secret wrapped with the folder's recovery key, and the Argon2id
    -- parameters of the password.
    recovery_wrapped_secret BYTEA CHECK (length(recovery_wrapped_secret) = 73),
    kdf_salt                BYTEA CHECK (length(kdf_salt) = 16),
    kdf_memory_kib          INTEGER CHECK (kdf_memory_kib > 0),
    kdf_iterations          INTEGER CHECK (kdf_iterations > 0),
    kdf_parallelism         INTEGER CHECK (kdf_parallelism > 0),
    format                  INTEGER NOT NULL DEFAULT 1,
    -- 'protecting' and 'unprotecting' while a client replaces an existing folder's content.
    state                   TEXT NOT NULL
                            CHECK (state IN ('protecting', 'protected', 'unprotecting')),
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((lock = 'security_key') = (epoch IS NOT NULL)),
    CHECK ((lock = 'password') = (recovery_wrapped_secret IS NOT NULL AND kdf_salt IS NOT NULL
                                  AND kdf_memory_kib IS NOT NULL AND kdf_iterations IS NOT NULL
                                  AND kdf_parallelism IS NOT NULL)),
    CHECK (lock = 'password'
           OR (recovery_wrapped_secret IS NULL AND kdf_salt IS NULL AND kdf_memory_kib IS NULL
               AND kdf_iterations IS NULL AND kdf_parallelism IS NULL))
);

CREATE INDEX protected_folders_owner ON protected_folders (owner_id);

INSERT INTO protected_folders
    (node_id, owner_id, lock, recipient, wrapped_secret, epoch, format, state, created_at)
SELECT node_id, owner_id, 'security_key', recipient, wrapped_secret, epoch, format, state,
       created_at
FROM kept_protections;
