//! Protecting a folder, its state, and the lock that holds a user's protections in place.

use protocol::ProtectionState;
use sqlx::PgConnection;

use super::{
    NewPassword, NewProtection, ProtectError, ProtectRefusal, StoredProtection,
    unlockers::lock_vault,
};
use crate::{Db, DbError, NodeId, Tx, UserId};

/// The class of the advisory locks that hold one user's folder protections in place; the user
/// is the second key.
const PROTECTIONS_LOCK: i32 = 0x7072_6f74;

/// The columns of a [`StoredProtection`].
macro_rules! protection_columns {
    () => {
        "node_id, lock, wrapped_secret, epoch, recovery_wrapped_secret, kdf_salt, \
         kdf_memory_kib, kdf_iterations, kdf_parallelism, recipient, format, state, created_at"
    };
}

macro_rules! protection_select {
    () => {
        concat!("SELECT ", protection_columns!(), " FROM protected_folders")
    };
}

/// A recursive CTE `subtree (id)`: the node `$1` and everything below it.
macro_rules! subtree {
    () => {
        "WITH RECURSIVE subtree AS ( \
             SELECT id FROM nodes WHERE id = $1 \
             UNION ALL \
             SELECT n.id FROM nodes n JOIN subtree s ON n.parent_id = s.id \
         ) "
    };
}

impl Db {
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
    /// content waits in `protecting` until the client replaced it with its encrypted form. The
    /// folder and everything in it stop being shared and leave the search index.
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
        sqlx::query(concat!(
            subtree!(),
            "DELETE FROM search_entries WHERE node_id IN (SELECT id FROM subtree)"
        ))
        .bind(node)
        .execute(&mut *tx)
        .await?;
        let state = if empty {
            ProtectionState::Protected
        } else {
            ProtectionState::Protecting
        };
        let insert = match protection {
            NewProtection::SecurityKey(lock) => sqlx::query_as::<_, StoredProtection>(concat!(
                "INSERT INTO protected_folders \
                 (node_id, owner_id, lock, wrapped_secret, epoch, recipient, state) \
                 VALUES ($1, $2, 'security_key', $3, $4, $5, $6) \
                 RETURNING ",
                protection_columns!()
            ))
            .bind(node)
            .bind(user)
            .bind(lock.wrapped_secret)
            .bind(lock.epoch)
            .bind(lock.recipient)
            .bind(state),
            NewProtection::Password(lock) => sqlx::query_as::<_, StoredProtection>(concat!(
                "INSERT INTO protected_folders \
                 (node_id, owner_id, lock, wrapped_secret, recovery_wrapped_secret, kdf_salt, \
                  kdf_memory_kib, kdf_iterations, kdf_parallelism, recipient, state) \
                 VALUES ($1, $2, 'password', $3, $4, $5, $6, $7, $8, $9, $10) \
                 RETURNING ",
                protection_columns!()
            ))
            .bind(node)
            .bind(user)
            .bind(lock.wrapped_secret)
            .bind(lock.recovery_wrapped_secret)
            .bind(&lock.kdf.salt)
            .bind(lock.kdf.memory_kib)
            .bind(lock.kdf.iterations)
            .bind(lock.kdf.parallelism)
            .bind(lock.recipient)
            .bind(state),
        };
        let stored = insert.fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(stored)
    }

    /// Stores the password folder `node`'s secret wrapped for a new password, and for a new
    /// recovery key when given. `NotFound` unless it is one of the user's password folders.
    pub async fn set_folder_password(
        &self,
        user: UserId,
        node: NodeId,
        password: NewPassword<'_>,
    ) -> Result<StoredProtection, DbError> {
        let changed = sqlx::query_as::<_, StoredProtection>(concat!(
            "UPDATE protected_folders SET wrapped_secret = $3, kdf_salt = $4, \
             kdf_memory_kib = $5, kdf_iterations = $6, kdf_parallelism = $7, \
             recovery_wrapped_secret = COALESCE($8, recovery_wrapped_secret) \
             WHERE owner_id = $1 AND node_id = $2 AND lock = 'password' RETURNING ",
            protection_columns!()
        ))
        .bind(user)
        .bind(node)
        .bind(password.wrapped_secret)
        .bind(&password.kdf.salt)
        .bind(password.kdf.memory_kib)
        .bind(password.kdf.iterations)
        .bind(password.kdf.parallelism)
        .bind(password.recovery_wrapped_secret)
        .fetch_optional(self.pool())
        .await?;
        changed.ok_or(DbError::NotFound)
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
        let changed = sqlx::query_as::<_, StoredProtection>(concat!(
            "UPDATE protected_folders SET state = $3 WHERE owner_id = $1 AND node_id = $2 \
             RETURNING ",
            protection_columns!()
        ))
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
}

impl Tx {
    /// Keeps which of the owner's folders are protected as it is until the transaction ends.
    /// Moves take it first, before any row lock, then check with [`Tx::protected_folder_of`]
    /// that nothing crosses a protected folder's edge. Moves do not wait for each other;
    /// protecting a folder waits for the moves in progress, and they for it.
    pub async fn hold_protections(&mut self) -> Result<(), DbError> {
        sqlx::query("SELECT pg_advisory_xact_lock_shared($1, hashtext($2::text))")
            .bind(PROTECTIONS_LOCK)
            .bind(self.owner)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
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
    let special: bool = sqlx::query_scalar(concat!(
        subtree!(),
        "SELECT EXISTS (SELECT 1 FROM user_folders uf, subtree s \
                 WHERE uf.user_id = $2 AND (uf.trash_id = s.id OR uf.lost_and_found_id = s.id))"
    ))
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
    let overlaps = sqlx::query_scalar(concat!(
        subtree!(),
        "SELECT EXISTS (SELECT 1 FROM protected_folders p JOIN subtree s ON p.node_id = s.id)"
    ))
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
    sqlx::query(concat!(
        subtree!(),
        "DELETE FROM share_items WHERE node_id IN (SELECT id FROM subtree)"
    ))
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
