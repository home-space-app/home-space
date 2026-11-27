//! The vault of a user's security-key folders, its unlockers, and the protected folders.
//!
//! Everything stored here is opaque to the server: wrapped keys, PRF salts, Argon2id parameters
//! as JSON, and age recipients. The rules this module keeps are about consistency: every
//! unlocker and security-key folder is wrapped under the vault's current epoch, the last
//! unlocker of a vault with folders is never removed, and a folder is protected only where no
//! other protection overlaps it.

use std::collections::HashSet;

use std::fmt;

use protocol::{FolderLock, ProtectionState, UnlockerKind, redact::Redacted};
use sqlx::{FromRow, PgConnection};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Db, DbError, NodeId, NodeKind, Tx, UserId};

/// The class of the advisory locks that hold one user's folder protections in place; the user
/// is the second key.
const PROTECTIONS_LOCK: i32 = 0x7072_6f74;
/// The password folder that is `$1`, a node of the user `$2`, or holds it.
const ENCLOSING_PASSWORD_FOLDER: &str = "WITH RECURSIVE up AS ( \
         SELECT id, parent_id FROM nodes WHERE id = $1 AND owner_id = $2 \
         UNION ALL \
         SELECT n.id, n.parent_id FROM nodes n JOIN up ON n.id = up.parent_id \
     ) SELECT p.node_id, p.recipient, p.state FROM protected_folders p \
     JOIN up ON p.node_id = up.id WHERE p.lock = 'password' LIMIT 1";

/// A vault unlocker as stored; a folder key has its name, credential id, RP id and PRF salt.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct StoredUnlocker {
    pub id: Uuid,
    pub kind: UnlockerKind,
    pub name: Option<String>,
    pub credential_id: Option<Vec<u8>>,
    pub rp_id: Option<String>,
    pub prf_salt: Option<Vec<u8>>,
    pub wrapped_vault_key: Vec<u8>,
    pub created_at: OffsetDateTime,
}

/// A vault and its unlockers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredVault {
    pub epoch: i32,
    pub unlockers: Vec<StoredUnlocker>,
}

/// A folder key to add: a credential the client made for the vault alone.
#[derive(Clone, Copy, Debug)]
pub struct FolderKey<'a> {
    pub name: &'a str,
    pub credential_id: &'a [u8],
    pub rp_id: &'a str,
    pub prf_salt: &'a [u8],
}

/// What a new unlocker is.
#[derive(Clone, Copy, Debug)]
pub enum UnlockerSource<'a> {
    SecurityKey(FolderKey<'a>),
    /// The recovery key, which replaces the one before.
    Recovery,
}

/// An unlocker to add.
#[derive(Clone, Copy, Debug)]
pub struct NewUnlocker<'a> {
    pub id: Uuid,
    pub source: UnlockerSource<'a>,
    pub wrapped_vault_key: &'a [u8],
    /// The epoch the key is wrapped under: the vault's, or 1 to create it.
    pub epoch: i32,
}

/// An unlocker or folder secret wrapped with a rotated vault key.
#[derive(Clone, Debug)]
pub struct Rewrapped {
    pub id: Uuid,
    pub prf_salt: Option<Vec<u8>>,
    pub wrapped: Vec<u8>,
}

/// A protected folder as stored.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct StoredProtection {
    pub node_id: NodeId,
    pub lock: FolderLock,
    /// Security keys: the folder secret wrapped with the vault key of `epoch`.
    pub wrapped_secret: Option<Vec<u8>>,
    pub epoch: Option<i32>,
    /// The age recipient content is encrypted to.
    pub recipient: String,
    pub format: i32,
    pub state: ProtectionState,
    pub created_at: OffsetDateTime,
}

/// The lock of a folder protected with a security key: its secret wrapped with the vault key
/// of `epoch`, and the age recipient clients encrypt to.
#[derive(Clone, Copy, Debug)]
pub struct KeyLock<'a> {
    pub wrapped_secret: &'a [u8],
    pub epoch: i32,
    pub recipient: &'a str,
}

/// The lock of a folder the server encrypts: the hash its password is checked against, its
/// secret sealed with the server's key, and the recipient derived from that secret.
#[derive(Clone, Copy)]
pub struct PasswordLock<'a> {
    pub password_hash: &'a str,
    /// The hash of the folder's recovery key.
    pub recovery_hash: &'a str,
    pub sealed_key: &'a [u8],
    pub recipient: &'a str,
}

impl fmt::Debug for PasswordLock<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PasswordLock")
            .field("password_hash", &Redacted)
            .field("recovery_hash", &Redacted)
            .field("sealed_key", &Redacted)
            .field("recipient", &self.recipient)
            .finish()
    }
}

/// How a folder is protected.
#[derive(Clone, Copy, Debug)]
pub enum NewProtection<'a> {
    SecurityKey(KeyLock<'a>),
    Password(PasswordLock<'a>),
}

/// A password folder's secrets, to check a password and open its key.
#[derive(Clone, PartialEq, Eq, FromRow)]
pub struct PasswordSecrets {
    pub password_hash: String,
    pub recovery_hash: String,
    pub sealed_key: Vec<u8>,
}

impl fmt::Debug for PasswordSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PasswordSecrets")
            .field("password_hash", &Redacted)
            .field("recovery_hash", &Redacted)
            .field("sealed_key", &Redacted)
            .finish()
    }
}

/// The password folder that holds a node.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct EnclosingPasswordFolder {
    pub node_id: NodeId,
    pub recipient: String,
    pub state: ProtectionState,
}

/// A stored file to convert: its node and the ids of its older versions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderFile {
    pub node_id: NodeId,
    pub versions: Vec<Uuid>,
}

/// A node inside a folder being protected, as [`Db::folder_nodes`] lists it.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct FolderNode {
    pub id: NodeId,
    pub parent_id: Option<NodeId>,
    pub kind: NodeKind,
    pub name: String,
}

/// Why a folder cannot be protected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProtectRefusal {
    /// Not a live folder of the user.
    #[error("not a folder of the user")]
    NotFound,
    /// The recycle bin or Lost & Found, or a folder holding one.
    #[error("a special folder or one holding it")]
    SpecialFolder,
    /// Already protected, inside a protected folder, or holding one.
    #[error("overlaps another protection")]
    Overlaps,
    /// A security-key lock whose epoch is not the vault's, or with no vault.
    #[error("the vault changed")]
    VaultChanged,
}

/// Why [`Db::protect_folder`] did not protect a folder.
#[derive(Debug, thiserror::Error)]
pub enum ProtectError {
    #[error("folder not protected: {0}")]
    Refused(#[from] ProtectRefusal),
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<sqlx::Error> for ProtectError {
    fn from(err: sqlx::Error) -> Self {
        Self::Db(err.into())
    }
}

macro_rules! protection_select {
    () => {
        "SELECT node_id, lock, wrapped_secret, epoch, recipient, format, state, created_at \
         FROM protected_folders"
    };
}

impl Db {
    /// The user's vault with its unlockers, oldest first; `None` before the first unlocker.
    pub async fn vault(&self, user: UserId) -> Result<Option<StoredVault>, DbError> {
        let epoch: Option<i32> = sqlx::query_scalar("SELECT epoch FROM vaults WHERE user_id = $1")
            .bind(user)
            .fetch_optional(self.pool())
            .await?;
        let Some(epoch) = epoch else {
            return Ok(None);
        };
        let unlockers = sqlx::query_as::<_, StoredUnlocker>(
            "SELECT id, kind, name, credential_id, rp_id, prf_salt, wrapped_vault_key, created_at \
             FROM vault_unlockers WHERE user_id = $1 ORDER BY created_at, id",
        )
        .bind(user)
        .fetch_all(self.pool())
        .await?;
        Ok(Some(StoredVault { epoch, unlockers }))
    }

    /// Adds an unlocker, creating the vault at epoch 1 when the user has none; a recovery key
    /// replaces the one before. `Stale` when `epoch` is not the vault's (or not 1 for a new
    /// one), `Conflict` when the folder key's credential already unlocks the vault.
    pub async fn add_vault_unlocker(
        &self,
        user: UserId,
        new: NewUnlocker<'_>,
    ) -> Result<(), DbError> {
        let mut tx = self.begin().await?;
        match lock_vault(&mut tx, user).await? {
            Some(epoch) if epoch == new.epoch => {}
            Some(_) => return Err(DbError::Stale),
            None if new.epoch == 1 => {
                sqlx::query("INSERT INTO vaults (user_id) VALUES ($1)")
                    .bind(user)
                    .execute(&mut *tx)
                    .await?;
            }
            None => return Err(DbError::Stale),
        }
        let (kind, key) = match new.source {
            UnlockerSource::SecurityKey(key) => (UnlockerKind::SecurityKey, Some(key)),
            UnlockerSource::Recovery => {
                sqlx::query("DELETE FROM vault_unlockers WHERE user_id = $1 AND kind = $2")
                    .bind(user)
                    .bind(UnlockerKind::Recovery)
                    .execute(&mut *tx)
                    .await?;
                (UnlockerKind::Recovery, None)
            }
        };
        sqlx::query(
            "INSERT INTO vault_unlockers \
             (id, user_id, kind, name, credential_id, rp_id, prf_salt, wrapped_vault_key) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(new.id)
        .bind(user)
        .bind(kind)
        .bind(key.map(|key| key.name))
        .bind(key.map(|key| key.credential_id))
        .bind(key.map(|key| key.rp_id))
        .bind(key.map(|key| key.prf_salt))
        .bind(new.wrapped_vault_key)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Removes the unlocker `id`. `NotFound` if the user has none such, `Conflict` when it is
    /// the last one and folders are locked with the vault. Removing the last unlocker of a
    /// vault without folders removes the vault, so the next one starts a new vault.
    pub async fn remove_vault_unlocker(&self, user: UserId, id: Uuid) -> Result<(), DbError> {
        let mut tx = self.begin().await?;
        if lock_vault(&mut tx, user).await?.is_none() {
            return Err(DbError::NotFound);
        }
        let removed = sqlx::query("DELETE FROM vault_unlockers WHERE user_id = $1 AND id = $2")
            .bind(user)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if removed.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        drop_vault_if_unused(&mut tx, user).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Replaces the vault key: `unlockers` and `folders` carry everything wrapped with the new
    /// one, and `remove` the unlockers that go. `Stale` when `epoch` is not the vault's;
    /// `Invalid` unless every unlocker that stays and every security-key folder is listed
    /// exactly once; `Conflict` when no unlocker would stay while folders need one.
    pub async fn rotate_vault(
        &self,
        user: UserId,
        epoch: i32,
        unlockers: &[Rewrapped],
        folders: &[Rewrapped],
        remove: &[Uuid],
    ) -> Result<i32, DbError> {
        let mut tx = self.begin().await?;
        match lock_vault(&mut tx, user).await? {
            Some(current) if current == epoch => {}
            Some(_) => return Err(DbError::Stale),
            None => return Err(DbError::NotFound),
        }
        let existing: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM vault_unlockers WHERE user_id = $1")
                .bind(user)
                .fetch_all(&mut *tx)
                .await?;
        let removing: HashSet<Uuid> = remove.iter().copied().collect();
        if !removing.iter().all(|id| existing.contains(id)) {
            return Err(DbError::NotFound);
        }
        let staying: HashSet<Uuid> = existing
            .into_iter()
            .filter(|id| !removing.contains(id))
            .collect();
        if !same_ids(unlockers, &staying) {
            return Err(DbError::Invalid);
        }
        let locked: HashSet<Uuid> = sqlx::query_scalar(
            "SELECT node_id FROM protected_folders WHERE owner_id = $1 AND lock = 'security_key'",
        )
        .bind(user)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|id: NodeId| id.0)
        .collect();
        if !same_ids(folders, &locked) {
            return Err(DbError::Invalid);
        }
        if staying.is_empty() && !locked.is_empty() {
            return Err(DbError::Conflict);
        }
        let next = epoch + 1;
        sqlx::query("DELETE FROM vault_unlockers WHERE user_id = $1 AND id = ANY($2)")
            .bind(user)
            .bind(remove)
            .execute(&mut *tx)
            .await?;
        for unlocker in unlockers {
            sqlx::query(
                "UPDATE vault_unlockers SET wrapped_vault_key = $3, \
                 prf_salt = COALESCE($4, prf_salt) WHERE user_id = $1 AND id = $2",
            )
            .bind(user)
            .bind(unlocker.id)
            .bind(&unlocker.wrapped)
            .bind(unlocker.prf_salt.as_deref())
            .execute(&mut *tx)
            .await?;
        }
        for folder in folders {
            sqlx::query(
                "UPDATE protected_folders SET wrapped_secret = $3, epoch = $4 \
                 WHERE owner_id = $1 AND node_id = $2",
            )
            .bind(user)
            .bind(folder.id)
            .bind(&folder.wrapped)
            .bind(next)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("UPDATE vaults SET epoch = $2 WHERE user_id = $1")
            .bind(user)
            .bind(next)
            .execute(&mut *tx)
            .await?;
        drop_vault_if_unused(&mut tx, user).await?;
        tx.commit().await?;
        Ok(next)
    }

    /// The user's protected folders, oldest first.
    pub async fn protected_folders(&self, user: UserId) -> Result<Vec<StoredProtection>, DbError> {
        let folders = sqlx::query_as::<_, StoredProtection>(concat!(
            protection_select!(),
            " WHERE owner_id = $1 ORDER BY created_at, node_id"
        ))
        .bind(user)
        .fetch_all(self.pool())
        .await?;
        Ok(folders)
    }

    /// The protected folder `node`, if it is one of the user's.
    pub async fn protected_folder(
        &self,
        user: UserId,
        node: NodeId,
    ) -> Result<Option<StoredProtection>, DbError> {
        let folder = sqlx::query_as::<_, StoredProtection>(concat!(
            protection_select!(),
            " WHERE owner_id = $1 AND node_id = $2"
        ))
        .bind(user)
        .bind(node)
        .fetch_optional(self.pool())
        .await?;
        Ok(folder)
    }

    /// Protects the user's folder `node`. An empty folder is protected at once; one with
    /// content waits in `protecting` until it is replaced with its encrypted form, by the client
    /// (security key) or the server (password). The folder and everything in it stop being
    /// shared and leave the search index.
    pub async fn protect_folder(
        &self,
        user: UserId,
        node: NodeId,
        protection: NewProtection<'_>,
    ) -> Result<StoredProtection, ProtectError> {
        let mut tx = self.begin().await?;
        lock_protections(&mut tx, user).await?;
        if let Some(refusal) = protection_refusal(&mut tx, user, node).await? {
            return Err(refusal.into());
        }
        if let NewProtection::SecurityKey(lock) = protection
            && lock_vault(&mut tx, user).await? != Some(lock.epoch)
        {
            return Err(ProtectRefusal::VaultChanged.into());
        }
        let empty: bool =
            sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM nodes WHERE parent_id = $1)")
                .bind(node)
                .fetch_one(&mut *tx)
                .await?;
        unshare_subtree(&mut tx, user, node).await?;
        sqlx::query(
            "WITH RECURSIVE subtree AS ( \
                 SELECT id FROM nodes WHERE id = $1 \
                 UNION ALL \
                 SELECT n.id FROM nodes n JOIN subtree s ON n.parent_id = s.id \
             ) DELETE FROM search_entries WHERE node_id IN (SELECT id FROM subtree)",
        )
        .bind(node)
        .execute(&mut *tx)
        .await?;
        let state = if empty {
            ProtectionState::Protected
        } else {
            ProtectionState::Protecting
        };
        let insert = match protection {
            NewProtection::SecurityKey(lock) => sqlx::query_as::<_, StoredProtection>(
                "INSERT INTO protected_folders \
                 (node_id, owner_id, lock, wrapped_secret, epoch, recipient, state) \
                 VALUES ($1, $2, 'security_key', $3, $4, $5, $6) \
                 RETURNING node_id, lock, wrapped_secret, epoch, recipient, format, state, \
                 created_at",
            )
            .bind(node)
            .bind(user)
            .bind(lock.wrapped_secret)
            .bind(lock.epoch)
            .bind(lock.recipient)
            .bind(state),
            NewProtection::Password(lock) => sqlx::query_as::<_, StoredProtection>(
                "INSERT INTO protected_folders \
                 (node_id, owner_id, lock, password_hash, sealed_key, recipient, state, \
                  recovery_hash) \
                 VALUES ($1, $2, 'password', $3, $4, $5, $6, $7) \
                 RETURNING node_id, lock, wrapped_secret, epoch, recipient, format, state, \
                 created_at",
            )
            .bind(node)
            .bind(user)
            .bind(lock.password_hash)
            .bind(lock.sealed_key)
            .bind(lock.recipient)
            .bind(state)
            .bind(lock.recovery_hash),
        };
        let stored = insert.fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(stored)
    }

    /// Moves the protected folder `node` from state `from` to `to`. `NotFound` if it is not
    /// one of the user's, `Stale` when it is not in `from`.
    pub async fn set_protection_state(
        &self,
        user: UserId,
        node: NodeId,
        from: &[ProtectionState],
        to: ProtectionState,
    ) -> Result<StoredProtection, DbError> {
        let mut tx = self.begin().await?;
        lock_protections(&mut tx, user).await?;
        let state: ProtectionState = sqlx::query_scalar(
            "SELECT state FROM protected_folders WHERE owner_id = $1 AND node_id = $2 FOR UPDATE",
        )
        .bind(user)
        .bind(node)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        if !from.contains(&state) {
            return Err(DbError::Stale);
        }
        let changed = sqlx::query_as::<_, StoredProtection>(
            "UPDATE protected_folders SET state = $3 WHERE owner_id = $1 AND node_id = $2 \
             RETURNING node_id, lock, wrapped_secret, epoch, recipient, format, state, created_at",
        )
        .bind(user)
        .bind(node)
        .bind(to)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(changed)
    }

    /// Forgets that `node` is protected, while it is in one of the states `from`. `NotFound`
    /// if it is not one of the user's protected folders, `Stale` when it is in another state.
    pub async fn remove_protection(
        &self,
        user: UserId,
        node: NodeId,
        from: &[ProtectionState],
    ) -> Result<(), DbError> {
        let mut tx = self.begin().await?;
        lock_protections(&mut tx, user).await?;
        let removed = sqlx::query(
            "DELETE FROM protected_folders WHERE owner_id = $1 AND node_id = $2 AND state = ANY($3)",
        )
        .bind(user)
        .bind(node)
        .bind(from)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        if removed.rows_affected() == 1 {
            return Ok(());
        }
        match self.protected_folder(user, node).await? {
            Some(_) => Err(DbError::Stale),
            None => Err(DbError::NotFound),
        }
    }

    /// The password hash and sealed key of the user's password folder `node`.
    pub async fn password_secrets(
        &self,
        user: UserId,
        node: NodeId,
    ) -> Result<Option<PasswordSecrets>, DbError> {
        let secrets = sqlx::query_as::<_, PasswordSecrets>(
            "SELECT password_hash, recovery_hash, sealed_key FROM protected_folders \
             WHERE owner_id = $1 AND node_id = $2 AND lock = 'password'",
        )
        .bind(user)
        .bind(node)
        .fetch_optional(self.pool())
        .await?;
        Ok(secrets)
    }

    /// The password folder that is `node` or holds it, if any.
    pub async fn enclosing_password_folder(
        &self,
        user: UserId,
        node: NodeId,
    ) -> Result<Option<EnclosingPasswordFolder>, DbError> {
        let found = sqlx::query_as::<_, EnclosingPasswordFolder>(ENCLOSING_PASSWORD_FOLDER)
            .bind(node)
            .bind(user)
            .fetch_optional(self.pool())
            .await?;
        Ok(found)
    }

    /// Lets `session` use the password folder `node` until `expires_at`.
    pub async fn unlock_folder(
        &self,
        session: Uuid,
        node: NodeId,
        expires_at: OffsetDateTime,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO protected_unlocks (session_id, node_id, expires_at) VALUES ($1, $2, $3) \
             ON CONFLICT (session_id, node_id) DO UPDATE SET expires_at = EXCLUDED.expires_at",
        )
        .bind(session)
        .bind(node)
        .bind(expires_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Ends `session`'s use of the password folder `node`.
    pub async fn lock_folder(&self, session: Uuid, node: NodeId) -> Result<(), DbError> {
        sqlx::query("DELETE FROM protected_unlocks WHERE session_id = $1 AND node_id = $2")
            .bind(session)
            .bind(node)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Whether `session` has the password folder `node` unlocked; if so, it stays unlocked
    /// until `extend_to`, since it is being used.
    pub async fn use_unlocked_folder(
        &self,
        session: Uuid,
        node: NodeId,
        extend_to: OffsetDateTime,
    ) -> Result<bool, DbError> {
        let used = sqlx::query(
            "UPDATE protected_unlocks SET expires_at = GREATEST(expires_at, $3) \
             WHERE session_id = $1 AND node_id = $2 AND expires_at > now()",
        )
        .bind(session)
        .bind(node)
        .bind(extend_to)
        .execute(self.pool())
        .await?;
        Ok(used.rows_affected() == 1)
    }

    /// Forgets unlocks that have expired; returns how many.
    pub async fn expire_folder_unlocks(&self) -> Result<u64, DbError> {
        let expired = sqlx::query("DELETE FROM protected_unlocks WHERE expires_at <= now()")
            .execute(self.pool())
            .await?;
        Ok(expired.rows_affected())
    }

    /// The password folders of `tenant`'s users whose content the server is still encrypting
    /// or decrypting, with their state.
    pub async fn converting_password_folders(
        &self,
        tenant: crate::TenantId,
    ) -> Result<Vec<(UserId, NodeId, ProtectionState)>, DbError> {
        let folders = sqlx::query_as(
            "SELECT p.owner_id, p.node_id, p.state FROM protected_folders p \
             JOIN users u ON u.id = p.owner_id \
             WHERE u.tenant_id = $1 AND p.lock = 'password' AND p.state <> 'protected' \
             ORDER BY p.created_at",
        )
        .bind(tenant)
        .fetch_all(self.pool())
        .await?;
        Ok(folders)
    }

    /// The files inside the user's folder `node`, with the ids of their older versions.
    pub async fn folder_files(
        &self,
        user: UserId,
        node: NodeId,
    ) -> Result<Vec<FolderFile>, DbError> {
        let rows: Vec<(NodeId, Option<Uuid>)> = sqlx::query_as(
            "WITH RECURSIVE subtree AS ( \
                 SELECT id, kind FROM nodes WHERE id = $1 AND owner_id = $2 \
                 UNION ALL \
                 SELECT n.id, n.kind FROM nodes n JOIN subtree s ON n.parent_id = s.id \
             ) SELECT s.id, v.id FROM subtree s \
             LEFT JOIN file_versions v ON v.node_id = s.id \
             WHERE s.kind = 'file' ORDER BY s.id, v.version",
        )
        .bind(node)
        .bind(user)
        .fetch_all(self.pool())
        .await?;
        let mut files: Vec<FolderFile> = Vec::new();
        for (id, version) in rows {
            if files.last().is_none_or(|file| file.node_id != id) {
                files.push(FolderFile {
                    node_id: id,
                    versions: Vec::new(),
                });
            }
            if let (Some(file), Some(version)) = (files.last_mut(), version) {
                file.versions.push(version);
            }
        }
        Ok(files)
    }

    /// The nodes inside the user's folder `node`, not the folder itself, parents before their
    /// children.
    pub async fn folder_nodes(
        &self,
        user: UserId,
        node: NodeId,
    ) -> Result<Vec<FolderNode>, DbError> {
        let nodes = sqlx::query_as::<_, FolderNode>(
            "WITH RECURSIVE subtree AS ( \
                 SELECT id, parent_id, kind, name, 0 AS depth FROM nodes \
                 WHERE id = $1 AND owner_id = $2 \
                 UNION ALL \
                 SELECT n.id, n.parent_id, n.kind, n.name, s.depth + 1 FROM nodes n \
                 JOIN subtree s ON n.parent_id = s.id \
             ) SELECT id, parent_id, kind, name FROM subtree WHERE depth > 0 \
             ORDER BY depth, id",
        )
        .bind(node)
        .bind(user)
        .fetch_all(self.pool())
        .await?;
        Ok(nodes)
    }

    /// Renames the user's node `node` from `from` to `to`; `false` when its name is no longer
    /// `from`, and `Conflict` when `to` is taken.
    pub async fn rename_node_from(
        &self,
        user: UserId,
        node: NodeId,
        from: &str,
        to: &str,
    ) -> Result<bool, DbError> {
        let renamed =
            sqlx::query("UPDATE nodes SET name = $4 WHERE id = $1 AND owner_id = $2 AND name = $3")
                .bind(node)
                .bind(user)
                .bind(from)
                .bind(to)
                .execute(self.pool())
                .await?
                .rows_affected();
        Ok(renamed == 1)
    }

    /// Sets a new password hash for the user's password folder `node`, and a new recovery key
    /// hash when given, and locks it for every session but `keep`.
    pub async fn set_folder_password(
        &self,
        user: UserId,
        node: NodeId,
        password_hash: &str,
        recovery_hash: Option<&str>,
        keep: Uuid,
    ) -> Result<(), DbError> {
        let mut tx = self.begin().await?;
        let changed = sqlx::query(
            "UPDATE protected_folders SET password_hash = $3, \
             recovery_hash = COALESCE($4, recovery_hash) \
             WHERE owner_id = $1 AND node_id = $2 AND lock = 'password'",
        )
        .bind(user)
        .bind(node)
        .bind(password_hash)
        .bind(recovery_hash)
        .execute(&mut *tx)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(DbError::NotFound);
        }
        sqlx::query("DELETE FROM protected_unlocks WHERE node_id = $1 AND session_id <> $2")
            .bind(node)
            .bind(keep)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The user's password folders that are `node` or below it.
    pub async fn password_folders_under(
        &self,
        user: UserId,
        node: NodeId,
    ) -> Result<Vec<NodeId>, DbError> {
        let folders = sqlx::query_scalar(
            "WITH RECURSIVE subtree AS ( \
                 SELECT id FROM nodes WHERE id = $1 AND owner_id = $2 \
                 UNION ALL \
                 SELECT n.id FROM nodes n JOIN subtree s ON n.parent_id = s.id \
                 WHERE n.kind = 'folder' \
             ) SELECT p.node_id FROM protected_folders p JOIN subtree s ON s.id = p.node_id \
             WHERE p.lock = 'password'",
        )
        .bind(node)
        .bind(user)
        .fetch_all(self.pool())
        .await?;
        Ok(folders)
    }

    /// Whether any of the user's `nodes` is a protected folder or inside one.
    pub async fn any_protected(&self, user: UserId, nodes: &[NodeId]) -> Result<bool, DbError> {
        let ids: Vec<Uuid> = nodes.iter().map(|node| node.0).collect();
        let found = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM nodes n WHERE n.owner_id = $1 AND n.id = ANY($2) \
             AND (in_protected_folder(n.id) \
                  OR EXISTS (SELECT 1 FROM protected_folders p WHERE p.node_id = n.id)))",
        )
        .bind(user)
        .bind(&ids)
        .fetch_one(self.pool())
        .await?;
        Ok(found)
    }

    /// The user's password folders.
    pub async fn password_folders(&self, user: UserId) -> Result<Vec<NodeId>, DbError> {
        let folders = sqlx::query_scalar(
            "SELECT node_id FROM protected_folders WHERE owner_id = $1 AND lock = 'password'",
        )
        .bind(user)
        .fetch_all(self.pool())
        .await?;
        Ok(folders)
    }

    /// The password folders `session` has unlocked and not let expire. Unlike
    /// [`Db::use_unlocked_folder`] this does not extend them.
    pub async fn unlocked_folders(&self, session: Uuid) -> Result<Vec<NodeId>, DbError> {
        let folders = sqlx::query_scalar(
            "SELECT node_id FROM protected_unlocks WHERE session_id = $1 AND expires_at > now()",
        )
        .bind(session)
        .fetch_all(self.pool())
        .await?;
        Ok(folders)
    }

    /// For each of the user's folders `folders` that is a password folder or inside one, that
    /// password folder: what holds the nodes in it.
    pub async fn password_folders_over(
        &self,
        user: UserId,
        folders: &[NodeId],
    ) -> Result<Vec<(NodeId, NodeId)>, DbError> {
        if folders.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<Uuid> = folders.iter().map(|folder| folder.0).collect();
        let found = sqlx::query_as(
            "WITH RECURSIVE up AS ( \
                 SELECT id AS origin, id FROM nodes WHERE owner_id = $1 AND id = ANY($2) \
                 UNION ALL \
                 SELECT up.origin, n.parent_id FROM nodes n JOIN up ON n.id = up.id \
                 WHERE n.parent_id IS NOT NULL \
             ) SELECT DISTINCT up.origin, p.node_id FROM up \
             JOIN protected_folders p ON p.node_id = up.id AND p.lock = 'password'",
        )
        .bind(user)
        .bind(&ids)
        .fetch_all(self.pool())
        .await?;
        Ok(found)
    }
}

impl Tx {
    /// Keeps which of the owner's folders are protected, and in what state, as it is until the
    /// transaction ends. Stores into the tree take it first, before any row lock, then read
    /// with [`Tx::password_folder_of`] how the names they write must be stored: names chosen
    /// before a folder was protected, or stopped being, are refused rather than written in the
    /// wrong form. Stores do not wait for each other; protecting or unprotecting a folder waits
    /// for the stores in progress, and they for it.
    pub async fn hold_protections(&mut self) -> Result<(), DbError> {
        sqlx::query("SELECT pg_advisory_xact_lock_shared($1, hashtext($2::text))")
            .bind(PROTECTIONS_LOCK)
            .bind(self.owner)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    /// The password folder that is `node` or holds it; `None` for the root.
    pub async fn password_folder_of(
        &mut self,
        node: Option<NodeId>,
    ) -> Result<Option<EnclosingPasswordFolder>, DbError> {
        let Some(node) = node else {
            return Ok(None);
        };
        let found = sqlx::query_as::<_, EnclosingPasswordFolder>(ENCLOSING_PASSWORD_FOLDER)
            .bind(node)
            .bind(self.owner)
            .fetch_optional(&mut *self.conn)
            .await?;
        Ok(found)
    }
}

/// Takes `user`'s protections for the transaction, once the stores holding them (see
/// [`Tx::hold_protections`]) are done. First in the transaction, before any row lock, so that
/// it never waits for a store that waits for it.
async fn lock_protections(conn: &mut PgConnection, user: UserId) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2::text))")
        .bind(PROTECTIONS_LOCK)
        .bind(user)
        .execute(conn)
        .await?;
    Ok(())
}

/// Takes the user's vault row for the transaction; its epoch, or `None` without a vault.
async fn lock_vault(conn: &mut PgConnection, user: UserId) -> Result<Option<i32>, DbError> {
    let epoch = sqlx::query_scalar("SELECT epoch FROM vaults WHERE user_id = $1 FOR UPDATE")
        .bind(user)
        .fetch_optional(conn)
        .await?;
    Ok(epoch)
}

/// Removes the vault once it has no unlocker left; `Conflict` if folders still need it.
async fn drop_vault_if_unused(conn: &mut PgConnection, user: UserId) -> Result<(), DbError> {
    let (unlockers, folders): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM vault_unlockers WHERE user_id = $1), \
         (SELECT COUNT(*) FROM protected_folders WHERE owner_id = $1 AND lock = 'security_key')",
    )
    .bind(user)
    .fetch_one(&mut *conn)
    .await?;
    match (unlockers, folders) {
        (0, 0) => {
            sqlx::query("DELETE FROM vaults WHERE user_id = $1")
                .bind(user)
                .execute(conn)
                .await?;
            Ok(())
        }
        (0, _) => Err(DbError::Conflict),
        _ => Ok(()),
    }
}

/// Why `node` cannot be protected, if it cannot: not a live folder of the user, a special
/// folder or one holding one, or protection overlapping. Locks the folder's row.
async fn protection_refusal(
    tx: &mut PgConnection,
    user: UserId,
    node: NodeId,
) -> Result<Option<ProtectRefusal>, DbError> {
    let found: Option<NodeId> = sqlx::query_scalar(concat!(
        "SELECT id FROM nodes WHERE id = $1 AND owner_id = $2 AND kind = 'folder' AND ",
        visible!("nodes"),
        " FOR UPDATE"
    ))
    .bind(node)
    .bind(user)
    .fetch_optional(&mut *tx)
    .await?;
    if found.is_none() {
        return Ok(Some(ProtectRefusal::NotFound));
    }
    if enclosing_protection(tx, user, node).await?.is_some() {
        return Ok(Some(ProtectRefusal::Overlaps));
    }
    let special: bool = sqlx::query_scalar(
        "WITH RECURSIVE subtree AS ( \
                 SELECT id FROM nodes WHERE id = $1 \
                 UNION ALL \
                 SELECT n.id FROM nodes n JOIN subtree s ON n.parent_id = s.id \
             ) SELECT EXISTS (SELECT 1 FROM user_folders uf, subtree s \
                 WHERE uf.user_id = $2 AND (uf.trash_id = s.id OR uf.lost_and_found_id = s.id))",
    )
    .bind(node)
    .bind(user)
    .fetch_one(&mut *tx)
    .await?;
    if special {
        return Ok(Some(ProtectRefusal::SpecialFolder));
    }
    if overlaps_protection(tx, node).await? {
        return Ok(Some(ProtectRefusal::Overlaps));
    }
    Ok(None)
}

/// The protected folder that is `node` or holds it, with its state.
async fn enclosing_protection(
    conn: &mut PgConnection,
    user: UserId,
    node: NodeId,
) -> Result<Option<(NodeId, ProtectionState)>, DbError> {
    let found = sqlx::query_as(
        "WITH RECURSIVE up AS ( \
             SELECT id, parent_id FROM nodes WHERE id = $1 AND owner_id = $2 \
             UNION ALL \
             SELECT n.id, n.parent_id FROM nodes n JOIN up ON n.id = up.parent_id \
         ) SELECT p.node_id, p.state FROM protected_folders p JOIN up ON p.node_id = up.id \
         LIMIT 1",
    )
    .bind(node)
    .bind(user)
    .fetch_optional(conn)
    .await?;
    Ok(found)
}

/// Whether `node` is protected, or a folder in it is.
async fn overlaps_protection(conn: &mut PgConnection, node: NodeId) -> Result<bool, DbError> {
    let overlaps = sqlx::query_scalar(
        "WITH RECURSIVE subtree AS ( \
             SELECT id FROM nodes WHERE id = $1 \
             UNION ALL \
             SELECT n.id FROM nodes n JOIN subtree s ON n.parent_id = s.id \
         ) SELECT EXISTS (SELECT 1 FROM protected_folders p JOIN subtree s ON p.node_id = s.id)",
    )
    .bind(node)
    .fetch_one(conn)
    .await?;
    Ok(overlaps)
}

/// Ends the sharing of `node` and everything in it: their share items go, and shares left with
/// none.
async fn unshare_subtree(
    conn: &mut PgConnection,
    user: UserId,
    node: NodeId,
) -> Result<(), DbError> {
    sqlx::query(
        "WITH RECURSIVE subtree AS ( \
             SELECT id FROM nodes WHERE id = $1 \
             UNION ALL \
             SELECT n.id FROM nodes n JOIN subtree s ON n.parent_id = s.id \
         ) DELETE FROM share_items WHERE node_id IN (SELECT id FROM subtree)",
    )
    .bind(node)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "DELETE FROM shares s WHERE s.owner_id = $1 \
         AND NOT EXISTS (SELECT 1 FROM share_items i WHERE i.share_id = s.id)",
    )
    .bind(user)
    .execute(conn)
    .await?;
    Ok(())
}

/// Whether `items` names each id of `ids` exactly once and nothing else.
fn same_ids(items: &[Rewrapped], ids: &HashSet<Uuid>) -> bool {
    let given: HashSet<Uuid> = items.iter().map(|item| item.id).collect();
    given.len() == items.len() && &given == ids
}

#[cfg(test)]
mod tests;
