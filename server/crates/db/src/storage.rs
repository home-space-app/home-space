//! How much each user stores, and facts about the database for the server status.

use sqlx::FromRow;
use uuid::Uuid;

use crate::{Db, DbError, UserId};

/// Bytes a user stores, split the way they can free it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, FromRow)]
pub struct StorageUsage {
    /// Current content of live files.
    pub files: i64,
    /// Previous versions of files, binned or not.
    pub versions: i64,
    /// Files in the recycle bin.
    pub trash: i64,
}

impl StorageUsage {
    /// Everything the user stores.
    pub fn total(&self) -> i64 {
        self.files + self.versions + self.trash
    }
}

/// One user's storage, for the administration status.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct UserStorage {
    pub id: UserId,
    pub username: String,
    pub is_admin: bool,
    /// Has no password, so signs in only through an identity provider.
    pub external: bool,
    #[sqlx(flatten)]
    pub usage: StorageUsage,
}

/// A user's quota and what counts against it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, FromRow)]
pub struct QuotaUsage {
    /// The most the user may store, in bytes.
    pub quota: i64,
    /// Everything the user stores: live files, versions and the recycle bin.
    pub stored: i64,
    /// Bytes reserved by the user's unfinished uploads into their own files.
    pub reserved: i64,
}

impl QuotaUsage {
    /// Bytes still free under the quota; zero once it is reached or passed.
    pub fn room(&self) -> u64 {
        u64::try_from(self.quota - self.stored - self.reserved).unwrap_or(0)
    }
}

/// The database server, for the administration status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseInfo {
    /// e.g. `PostgreSQL 17.2`.
    pub version: String,
    pub migrations: i64,
}

/// The usage columns of the user aliased `u`.
macro_rules! usage_select {
    () => {
        "COALESCE((SELECT SUM(n.size) FROM nodes n \
            WHERE n.owner_id = u.id AND n.kind = 'file' AND n.trashed_at IS NULL), 0)::BIGINT AS files, \
         COALESCE((SELECT SUM(v.size) FROM file_versions v WHERE v.owner_id = u.id), 0)::BIGINT AS versions, \
         COALESCE((SELECT SUM(n.size) FROM nodes n \
            WHERE n.owner_id = u.id AND n.kind = 'file' AND n.trashed_at IS NOT NULL), 0)::BIGINT AS trash"
    };
}

/// Everything the user aliased `u` stores, as one sum.
macro_rules! stored_select {
    () => {
        "COALESCE((SELECT SUM(n.size) FROM nodes n \
            WHERE n.owner_id = u.id AND n.kind = 'file'), 0)::BIGINT \
         + COALESCE((SELECT SUM(v.size) FROM file_versions v WHERE v.owner_id = u.id), 0)::BIGINT"
    };
}

impl Db {
    /// What `user` stores; zeros for an unknown user.
    pub async fn storage_usage(&self, user: UserId) -> Result<StorageUsage, DbError> {
        let usage = sqlx::query_as::<_, StorageUsage>(concat!(
            "SELECT ",
            usage_select!(),
            " FROM users u WHERE u.id = $1"
        ))
        .bind(user)
        .fetch_optional(self.pool())
        .await?;
        Ok(usage.unwrap_or_default())
    }

    /// The quota of `user` and what counts against it, leaving out the unfinished upload
    /// `except`; `None` when the user has no quota (or does not exist).
    ///
    /// Uploads into a transfer are not counted: those bytes belong to the transfer until the
    /// recipient saves it.
    pub async fn quota_usage(
        &self,
        user: UserId,
        except: Option<Uuid>,
    ) -> Result<Option<QuotaUsage>, DbError> {
        let usage = sqlx::query_as::<_, QuotaUsage>(concat!(
            "SELECT u.quota_bytes AS quota, (",
            stored_select!(),
            ") AS stored, \
             COALESCE((SELECT SUM(up.size) FROM uploads up \
                WHERE up.owner_id = u.id AND up.transfer_item_id IS NULL \
                AND ($2::uuid IS NULL OR up.id <> $2)), 0)::BIGINT AS reserved \
             FROM users u WHERE u.id = $1 AND u.quota_bytes IS NOT NULL"
        ))
        .bind(user)
        .bind(except)
        .fetch_optional(self.pool())
        .await?;
        Ok(usage)
    }

    /// What every user stores, ordered by username.
    pub async fn storage_by_user(&self) -> Result<Vec<UserStorage>, DbError> {
        let users = sqlx::query_as::<_, UserStorage>(concat!(
            "SELECT u.id, u.username, u.is_admin, \
             u.password_hash IS NULL AS external, ",
            usage_select!(),
            " FROM users u ORDER BY u.username"
        ))
        .fetch_all(self.pool())
        .await?;
        Ok(users)
    }

    /// The database server's version and how many migrations it has applied.
    pub async fn database_info(&self) -> Result<DatabaseInfo, DbError> {
        let version: String = sqlx::query_scalar("SHOW server_version")
            .fetch_one(self.pool())
            .await?;
        let migrations: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success")
                .fetch_one(self.pool())
                .await?;
        let version = version.split_whitespace().next().unwrap_or_default();
        Ok(DatabaseInfo {
            version: format!("PostgreSQL {version}"),
            migrations,
        })
    }
}
