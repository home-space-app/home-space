//! Accounts as administrators manage them: state, quota, devices and deletion.

use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Db, DbError, StorageUsage, UserId};

/// One account in the administration's user list.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct Account {
    pub id: UserId,
    pub username: String,
    pub is_admin: bool,
    /// Has no password, so signs in only through an identity provider.
    pub external: bool,
    pub created_at: OffsetDateTime,
    pub disabled_at: Option<OffsetDateTime>,
    pub quota_bytes: Option<i64>,
    pub delete_at: Option<OffsetDateTime>,
    #[sqlx(flatten)]
    pub usage: StorageUsage,
    /// Devices the user has signed in from.
    pub devices: i64,
    /// When one of those devices was last seen.
    pub last_seen_at: Option<OffsetDateTime>,
}

/// What a user being deleted has on disk outside their own blob directories.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeletedAccount {
    /// Transfers the user sent or received; their copies go with the user.
    pub transfers: Vec<Uuid>,
    /// Their unfinished resumable uploads.
    pub uploads: Vec<Uuid>,
}

/// The columns of [`Account`] for the user aliased `u`.
macro_rules! account_select {
    () => {
        "SELECT u.id, u.username, u.is_admin, u.password_hash IS NULL AS external, \
         u.created_at, u.disabled_at, u.quota_bytes, u.delete_at, \
         COALESCE((SELECT SUM(n.size) FROM nodes n \
            WHERE n.owner_id = u.id AND n.kind = 'file' AND n.trashed_at IS NULL), 0)::BIGINT AS files, \
         COALESCE((SELECT SUM(v.size) FROM file_versions v WHERE v.owner_id = u.id), 0)::BIGINT AS versions, \
         COALESCE((SELECT SUM(n.size) FROM nodes n \
            WHERE n.owner_id = u.id AND n.kind = 'file' AND n.trashed_at IS NOT NULL), 0)::BIGINT AS trash, \
         (SELECT COUNT(*) FROM devices d WHERE d.user_id = u.id) AS devices, \
         (SELECT MAX(d.last_seen_at) FROM devices d WHERE d.user_id = u.id) AS last_seen_at \
         FROM users u"
    };
}

impl Db {
    /// Every account, ordered by username.
    pub async fn accounts(&self) -> Result<Vec<Account>, DbError> {
        let accounts =
            sqlx::query_as::<_, Account>(concat!(account_select!(), " ORDER BY u.username"))
                .fetch_all(self.pool())
                .await?;
        Ok(accounts)
    }

    /// One account; `NotFound` if there is no such user.
    pub async fn account(&self, id: UserId) -> Result<Account, DbError> {
        sqlx::query_as::<_, Account>(concat!(account_select!(), " WHERE u.id = $1"))
            .bind(id)
            .fetch_optional(self.pool())
            .await?
            .ok_or(DbError::NotFound)
    }

    /// Sets or, with `None`, removes a user's quota; `NotFound` if there is no such user.
    pub async fn set_quota(&self, id: UserId, quota_bytes: Option<i64>) -> Result<(), DbError> {
        let updated = sqlx::query("UPDATE users SET quota_bytes = $2 WHERE id = $1")
            .bind(id)
            .bind(quota_bytes)
            .execute(self.pool())
            .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Disables a user and ends all their sessions, or enables them again. Disabling an
    /// already disabled user keeps the time it was first disabled. `NotFound` if there is no
    /// such user; `Conflict` for enabling a user marked for deletion, who stays disabled
    /// until the deletion is cancelled.
    pub async fn set_disabled(&self, id: UserId, disabled: bool) -> Result<(), DbError> {
        let mut tx = self.pool().begin().await?;
        let marked = sqlx::query_scalar::<_, bool>(
            "UPDATE users SET disabled_at = CASE \
                 WHEN $2 OR delete_at IS NOT NULL THEN COALESCE(disabled_at, now()) END \
             WHERE id = $1 RETURNING delete_at IS NOT NULL",
        )
        .bind(id)
        .bind(disabled)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        if marked && !disabled {
            return Err(DbError::Conflict);
        }
        if disabled {
            end_sessions(&mut tx, id).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Marks a user for deletion at `delete_at`, which also disables them, and ends all their
    /// sessions. A user already marked keeps the earlier time. Returns when the user will be deleted; `NotFound` if
    /// there is no such user.
    pub async fn mark_for_deletion(
        &self,
        id: UserId,
        delete_at: OffsetDateTime,
    ) -> Result<OffsetDateTime, DbError> {
        let mut tx = self.pool().begin().await?;
        let due = sqlx::query_scalar::<_, OffsetDateTime>(
            "UPDATE users SET delete_at = COALESCE(delete_at, $2), \
             disabled_at = COALESCE(disabled_at, now()) WHERE id = $1 RETURNING delete_at",
        )
        .bind(id)
        .bind(delete_at)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        end_sessions(&mut tx, id).await?;
        tx.commit().await?;
        Ok(due)
    }

    /// Takes back a user's deletion and enables them again, even when they were disabled
    /// before; a user not marked is left as is. `NotFound` if there is no such user or they
    /// are already gone. Files the deletion job removed before that stay removed.
    pub async fn cancel_deletion(&self, id: UserId) -> Result<(), DbError> {
        let updated = sqlx::query(
            "UPDATE users SET delete_at = NULL, \
             disabled_at = CASE WHEN delete_at IS NULL THEN disabled_at END WHERE id = $1",
        )
        .bind(id)
        .execute(self.pool())
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Users whose deletion is due, the longest overdue first.
    pub async fn due_deletions(&self) -> Result<Vec<UserId>, DbError> {
        let users = sqlx::query_scalar::<_, UserId>(
            "SELECT id FROM users WHERE delete_at <= now() ORDER BY delete_at",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(users)
    }

    /// What a user whose deletion is due has on disk outside their own blob directories.
    /// `None` when the user is gone or no longer due, as when the deletion was cancelled.
    pub async fn due_user_leftovers(&self, id: UserId) -> Result<Option<DeletedAccount>, DbError> {
        let due = sqlx::query_scalar::<_, UserId>(
            "SELECT id FROM users WHERE id = $1 AND delete_at <= now()",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        if due.is_none() {
            return Ok(None);
        }
        let transfers = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM transfers WHERE sender_id = $1 OR recipient_id = $1",
        )
        .bind(id)
        .fetch_all(self.pool())
        .await?;
        let uploads = sqlx::query_scalar::<_, Uuid>("SELECT id FROM uploads WHERE owner_id = $1")
            .bind(id)
            .fetch_all(self.pool())
            .await?;
        Ok(Some(DeletedAccount { transfers, uploads }))
    }

    /// Deletes a user whose deletion is due, with everything the database holds of them.
    /// Returns whether there was such a user: not when the deletion was cancelled meanwhile.
    pub async fn delete_due_user(&self, id: UserId) -> Result<bool, DbError> {
        let mut tx = self.pool().begin().await?;
        // Everything else that belongs to the user goes with it through `ON DELETE CASCADE`.
        let deleted = sqlx::query("DELETE FROM users WHERE id = $1 AND delete_at <= now()")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if deleted.rows_affected() == 0 {
            return Ok(false);
        }
        // The change journal has no foreign key to users: its deferred triggers would write
        // rows for the deleted nodes at commit. Running them now lets the cleanup below catch
        // those rows too.
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
        for statement in [
            "DELETE FROM node_changes WHERE owner_id = $1",
            "DELETE FROM node_changes_pruned WHERE owner_id = $1",
        ] {
            sqlx::query(statement).bind(id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(true)
    }
}

/// Holds `users` back from being disabled, which marking for deletion does too, until `tx`
/// ends, so that nothing the transaction adds for them is missed by the deletion job, which
/// lists what a user has on disk only after they are marked. `NotFound` if one of them is
/// gone or disabled.
pub(crate) async fn lock_active_users(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    users: &[UserId],
) -> Result<(), DbError> {
    for user in users {
        // Disabling updates the row, so it waits for this lock; one that came first is seen
        // here once its transaction commits.
        sqlx::query_scalar::<_, UserId>(
            "SELECT id FROM users WHERE id = $1 AND disabled_at IS NULL FOR SHARE",
        )
        .bind(user)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(DbError::NotFound)?;
    }
    Ok(())
}

async fn end_sessions(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: UserId,
) -> Result<(), DbError> {
    sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use time::Duration;

    use super::*;

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_marked_user_is_deleted_only_once_due(pool: PgPool) {
        let db = Db::from_pool(pool.clone());
        let user = db.create_user("doomed", "hash").await.expect("user");
        let later = OffsetDateTime::now_utc() + Duration::minutes(10);
        db.mark_for_deletion(user.id, later).await.expect("mark");
        assert!(db.due_deletions().await.expect("due").is_empty());
        assert_eq!(
            db.due_user_leftovers(user.id).await.expect("leftovers"),
            None
        );
        assert!(!db.delete_due_user(user.id).await.expect("delete"));

        let earlier = OffsetDateTime::now_utc() - Duration::seconds(1);
        // Marking again keeps the first time.
        assert_eq!(
            db.mark_for_deletion(user.id, earlier).await.expect("mark"),
            db.account(user.id)
                .await
                .expect("account")
                .delete_at
                .expect("due")
        );
        db.cancel_deletion(user.id).await.expect("cancel");
        db.mark_for_deletion(user.id, earlier).await.expect("mark");
        assert_eq!(db.due_deletions().await.expect("due"), vec![user.id]);
        assert!(
            db.due_user_leftovers(user.id)
                .await
                .expect("leftovers")
                .is_some()
        );
        assert!(db.delete_due_user(user.id).await.expect("delete"));
        assert!(db.find_user(user.id).await.expect("find").is_none());
        // Nothing of the user is left behind, the change journal included.
        let journal: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM node_changes WHERE owner_id = $1)
                  + (SELECT count(*) FROM node_changes_pruned WHERE owner_id = $1)",
        )
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .expect("journal");
        assert_eq!(journal, 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_disabled_user_or_one_marked_for_deletion_gets_no_new_uploads(pool: PgPool) {
        let db = Db::from_pool(pool);
        let user = db.create_user("doomed", "hash").await.expect("user");
        let later = OffsetDateTime::now_utc() + Duration::minutes(10);
        db.mark_for_deletion(user.id, later).await.expect("mark");
        let new = crate::NewUpload {
            id: Uuid::now_v7(),
            parent_id: None,
            node_id: None,
            name: "a.txt".to_owned(),
            size: 1,
            mime: None,
            modified_at: None,
            device_id: None,
            keep_both: false,
            version_of: None,
            auto_upload: false,
            transfer_item_id: None,
        };
        assert!(matches!(
            db.create_upload(user.id, new.clone()).await,
            Err(DbError::NotFound)
        ));
        db.cancel_deletion(user.id).await.expect("cancel");
        db.set_disabled(user.id, true).await.expect("disable");
        assert!(matches!(
            db.create_upload(user.id, new.clone()).await,
            Err(DbError::NotFound)
        ));
        db.set_disabled(user.id, false).await.expect("enable");
        db.create_upload(user.id, new).await.expect("upload");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn disabling_keeps_the_first_time_and_enabling_clears_it(pool: PgPool) {
        let db = Db::from_pool(pool);
        let user = db.create_user("guest", "hash").await.expect("user");
        db.set_disabled(user.id, true).await.expect("disable");
        let first = db.account(user.id).await.expect("account").disabled_at;
        assert!(first.is_some());
        db.set_disabled(user.id, true).await.expect("disable");
        assert_eq!(
            db.account(user.id).await.expect("account").disabled_at,
            first
        );
        db.set_disabled(user.id, false).await.expect("enable");
        assert_eq!(
            db.account(user.id).await.expect("account").disabled_at,
            None
        );
    }
}
