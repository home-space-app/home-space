//! The password folders a session has unlocked, and the secrets that unlock them.

use time::OffsetDateTime;
use uuid::Uuid;

use super::PasswordSecrets;
use crate::{Db, DbError, NodeId, UserId};

impl Db {
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

    /// Forgets unlocks that have expired; returns how many.
    pub async fn expire_folder_unlocks(&self) -> Result<u64, DbError> {
        let expired = sqlx::query("DELETE FROM protected_unlocks WHERE expires_at <= now()")
            .execute(self.pool())
            .await?;
        Ok(expired.rows_affected())
    }
}
