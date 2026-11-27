//! The vault of a user's security-key folders and its unlockers.

use std::collections::HashSet;

use protocol::UnlockerKind;
use sqlx::PgConnection;
use uuid::Uuid;

use super::{NewUnlocker, Rewrapped, StoredUnlocker, StoredVault, UnlockerSource};
use crate::{Db, DbError, NodeId, UserId};

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
}

/// Takes the user's vault row for the transaction; its epoch, or `None` without a vault.
pub(super) async fn lock_vault(
    conn: &mut PgConnection,
    user: UserId,
) -> Result<Option<i32>, DbError> {
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

/// Whether `items` names each id of `ids` exactly once and nothing else.
fn same_ids(items: &[Rewrapped], ids: &HashSet<Uuid>) -> bool {
    let given: HashSet<Uuid> = items.iter().map(|item| item.id).collect();
    given.len() == items.len() && &given == ids
}
