//! The vault of a user's security-key folders, its unlockers, and the protected folders.
//!
//! Everything stored here is opaque to the server: wrapped keys, PRF salts, Argon2id parameters
//! as JSON, and age recipients. The rules this module keeps are about consistency: every
//! unlocker and security-key folder is wrapped under the vault's current epoch, the last
//! unlocker of a vault with folders is never removed, and a folder is protected only where no
//! other protection overlaps it.

use std::collections::HashSet;

use sqlx::{FromRow, PgConnection};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Db, DbError, NodeId, UserId};

/// A vault unlocker as stored, with its security key's credential id and RP id.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct StoredUnlocker {
    pub id: Uuid,
    /// `security_key` or `recovery`.
    pub kind: String,
    pub credential: Option<Uuid>,
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

/// An unlocker to add.
#[derive(Clone, Copy, Debug)]
pub struct NewUnlocker<'a> {
    pub id: Uuid,
    /// `security_key` or `recovery`.
    pub kind: &'a str,
    pub credential: Option<Uuid>,
    pub prf_salt: Option<&'a [u8]>,
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
    /// `security_key` or `password`.
    pub lock: String,
    /// Security keys: the folder secret wrapped with the vault key of `epoch`.
    pub wrapped_secret: Option<Vec<u8>>,
    pub epoch: Option<i32>,
    /// Security keys: the age recipient.
    pub recipient: Option<String>,
    pub format: i32,
    /// `protecting`, `protected` or `unprotecting`.
    pub state: String,
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

/// Why a folder cannot be protected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectRefusal {
    /// Not a live folder of the user.
    NotFound,
    /// The recycle bin or Lost & Found, or a folder holding one.
    SpecialFolder,
    /// Already protected, inside a protected folder, or holding one.
    Overlaps,
    /// A security-key lock whose epoch is not the vault's, or with no vault.
    VaultChanged,
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
            "SELECT u.id, u.kind, u.credential, w.credential_id, w.rp_id, u.prf_salt, \
             u.wrapped_vault_key, u.created_at \
             FROM vault_unlockers u LEFT JOIN user_webauthn w ON w.id = u.credential \
             WHERE u.user_id = $1 ORDER BY u.created_at, u.id",
        )
        .bind(user)
        .fetch_all(self.pool())
        .await?;
        Ok(Some(StoredVault { epoch, unlockers }))
    }

    /// Adds an unlocker, creating the vault at epoch 1 when the user has none; a recovery key
    /// replaces the one before. `Stale` when `epoch` is not the vault's (or not 1 for a new
    /// one), `NotFound` when the credential is not the user's, `Conflict` when it already
    /// unlocks the vault.
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
        if let Some(credential) = new.credential {
            let owned: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM user_webauthn WHERE id = $1 AND user_id = $2)",
            )
            .bind(credential)
            .bind(user)
            .fetch_one(&mut *tx)
            .await?;
            if !owned {
                return Err(DbError::NotFound);
            }
        }
        if new.kind == "recovery" {
            sqlx::query("DELETE FROM vault_unlockers WHERE user_id = $1 AND kind = 'recovery'")
                .bind(user)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(
            "INSERT INTO vault_unlockers \
             (id, user_id, kind, credential, prf_salt, wrapped_vault_key) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(new.id)
        .bind(user)
        .bind(new.kind)
        .bind(new.credential)
        .bind(new.prf_salt)
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

    /// Protects the user's folder `node` with a security key. An empty folder is protected at
    /// once; one with content waits in `protecting` until the client has replaced it with its
    /// encrypted form. The folder and everything in it stop being shared.
    pub async fn protect_folder(
        &self,
        user: UserId,
        node: NodeId,
        lock: KeyLock<'_>,
    ) -> Result<Result<StoredProtection, ProtectRefusal>, DbError> {
        let mut tx = self.begin().await?;
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
            return Ok(Err(ProtectRefusal::NotFound));
        }
        if enclosing_protection(&mut tx, user, node).await?.is_some() {
            return Ok(Err(ProtectRefusal::Overlaps));
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
            return Ok(Err(ProtectRefusal::SpecialFolder));
        }
        if overlaps_protection(&mut tx, node).await? {
            return Ok(Err(ProtectRefusal::Overlaps));
        }
        if lock_vault(&mut tx, user).await? != Some(lock.epoch) {
            return Ok(Err(ProtectRefusal::VaultChanged));
        }
        let empty: bool =
            sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM nodes WHERE parent_id = $1)")
                .bind(node)
                .fetch_one(&mut *tx)
                .await?;
        unshare_subtree(&mut tx, user, node).await?;
        let stored = sqlx::query_as::<_, StoredProtection>(
            "INSERT INTO protected_folders \
             (node_id, owner_id, lock, wrapped_secret, epoch, recipient, state) \
             VALUES ($1, $2, 'security_key', $3, $4, $5, $6) \
             RETURNING node_id, lock, wrapped_secret, epoch, recipient, format, state, created_at",
        )
        .bind(node)
        .bind(user)
        .bind(lock.wrapped_secret)
        .bind(lock.epoch)
        .bind(lock.recipient)
        .bind(if empty { "protected" } else { "protecting" })
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Ok(stored))
    }

    /// Moves the protected folder `node` from state `from` to `to`. `NotFound` if it is not
    /// one of the user's, `Stale` when it is not in `from`.
    pub async fn set_protection_state(
        &self,
        user: UserId,
        node: NodeId,
        from: &[&str],
        to: &str,
    ) -> Result<StoredProtection, DbError> {
        let mut tx = self.begin().await?;
        let state: String = sqlx::query_scalar(
            "SELECT state FROM protected_folders WHERE owner_id = $1 AND node_id = $2 FOR UPDATE",
        )
        .bind(user)
        .bind(node)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        if !from.contains(&state.as_str()) {
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
        from: &[&str],
    ) -> Result<(), DbError> {
        let removed = sqlx::query(
            "DELETE FROM protected_folders WHERE owner_id = $1 AND node_id = $2 AND state = ANY($3)",
        )
        .bind(user)
        .bind(node)
        .bind(from)
        .execute(self.pool())
        .await?;
        if removed.rows_affected() == 1 {
            return Ok(());
        }
        match self.protected_folder(user, node).await? {
            Some(_) => Err(DbError::Stale),
            None => Err(DbError::NotFound),
        }
    }
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
pub(crate) async fn drop_vault_if_unused(
    conn: &mut PgConnection,
    user: UserId,
) -> Result<(), DbError> {
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

/// The protected folder that is `node` or holds it, with its state.
async fn enclosing_protection(
    conn: &mut PgConnection,
    user: UserId,
    node: NodeId,
) -> Result<Option<(NodeId, String)>, DbError> {
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
