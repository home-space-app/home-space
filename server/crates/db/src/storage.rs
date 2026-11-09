//! How much each user stores, and facts about the database for the server status.

use sqlx::FromRow;

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
