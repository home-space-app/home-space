use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{Db, DbError, FOLDER_MIME, LOST_AND_FOUND, NodeId, User, UserId};

macro_rules! user_columns {
    () => {
        "id, username, password_hash, created_at, is_admin, disabled_at, quota_bytes, delete_at"
    };
}

impl Db {
    /// Inserts a new user with their recycle bin and Lost & Found folders; `Conflict` if the
    /// username is taken.
    pub async fn create_user(&self, username: &str, password_hash: &str) -> Result<User, DbError> {
        let mut tx = self.pool().begin().await?;
        let user = insert_user(&mut tx, username, Some(password_hash)).await?;
        tx.commit().await?;
        Ok(user)
    }

    /// Inserts a password-less user linked to an identity provider account. `Conflict` if the
    /// username or the identity already exists.
    pub async fn create_user_with_identity(
        &self,
        username: &str,
        issuer: &str,
        subject: &str,
        email: Option<&str>,
    ) -> Result<User, DbError> {
        let mut tx = self.pool().begin().await?;
        let user = insert_user(&mut tx, username, None).await?;
        sqlx::query(
            "INSERT INTO user_identities (issuer, subject, user_id, email) VALUES ($1, $2, $3, $4)",
        )
        .bind(issuer)
        .bind(subject)
        .bind(user.id)
        .bind(email)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(user)
    }

    /// Finds the user linked to an identity provider account.
    pub async fn find_user_by_identity(
        &self,
        issuer: &str,
        subject: &str,
    ) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as::<_, User>(concat!(
            "SELECT ",
            user_columns!(),
            " FROM users WHERE id = \
             (SELECT user_id FROM user_identities WHERE issuer = $1 AND subject = $2)"
        ))
        .bind(issuer)
        .bind(subject)
        .fetch_optional(self.pool())
        .await?;
        Ok(user)
    }

    /// Finds a user by username.
    pub async fn find_user_by_username(&self, username: &str) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as::<_, User>(concat!(
            "SELECT ",
            user_columns!(),
            " FROM users WHERE username = $1"
        ))
        .bind(username)
        .fetch_optional(self.pool())
        .await?;
        Ok(user)
    }

    /// Finds a user by id.
    pub async fn find_user(&self, id: UserId) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as::<_, User>(concat!(
            "SELECT ",
            user_columns!(),
            " FROM users WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        Ok(user)
    }

    /// Lists all users ordered by username.
    pub async fn list_users(&self) -> Result<Vec<User>, DbError> {
        let users = sqlx::query_as::<_, User>(concat!(
            "SELECT ",
            user_columns!(),
            " FROM users ORDER BY username"
        ))
        .fetch_all(self.pool())
        .await?;
        Ok(users)
    }

    /// Grants or revokes administrator rights; `NotFound` if there is no such user.
    pub async fn set_admin(&self, id: UserId, is_admin: bool) -> Result<(), DbError> {
        let updated = sqlx::query("UPDATE users SET is_admin = $2 WHERE id = $1")
            .bind(id)
            .bind(is_admin)
            .execute(self.pool())
            .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Replaces a user's password hash and ends all their sessions.
    pub async fn set_password_hash(&self, id: UserId, password_hash: &str) -> Result<(), DbError> {
        let mut tx = self.pool().begin().await?;
        let updated = sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
            .bind(id)
            .bind(password_hash)
            .execute(&mut *tx)
            .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

impl Db {
    /// Replaces a user's password hash and ends every session except `keep`, the one that
    /// asked for the change.
    pub async fn change_password_hash(
        &self,
        id: UserId,
        password_hash: &str,
        keep: Uuid,
    ) -> Result<(), DbError> {
        let mut tx = self.pool().begin().await?;
        let updated = sqlx::query(
            "UPDATE users SET password_hash = $2 WHERE id = $1 AND password_hash IS NOT NULL",
        )
        .bind(id)
        .bind(password_hash)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND id <> $2")
            .bind(id)
            .bind(keep)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

/// Inserts a user with their recycle bin and Lost & Found folders inside `tx`.
async fn insert_user(
    tx: &mut Transaction<'_, Postgres>,
    username: &str,
    password_hash: Option<&str>,
) -> Result<User, DbError> {
    let user = sqlx::query_as::<_, User>(concat!(
        "INSERT INTO users (id, username, password_hash) VALUES ($1, $2, $3) RETURNING ",
        user_columns!(),
    ))
    .bind(UserId::new())
    .bind(username)
    .bind(password_hash)
    .fetch_one(&mut **tx)
    .await?;
    let trash_id = NodeId::new();
    let lost_and_found_id = NodeId::new();
    for (id, name) in [
        (trash_id, format!(".recycle-bin-{trash_id}")),
        (lost_and_found_id, LOST_AND_FOUND.to_owned()),
    ] {
        sqlx::query(
            "INSERT INTO nodes (id, owner_id, parent_id, kind, name, mime) \
             VALUES ($1, $2, NULL, 'folder', $3, $4)",
        )
        .bind(id)
        .bind(user.id)
        .bind(name)
        .bind(FOLDER_MIME)
        .execute(&mut **tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO user_folders (user_id, trash_id, lost_and_found_id) VALUES ($1, $2, $3)",
    )
    .bind(user.id)
    .bind(trash_id)
    .bind(lost_and_found_id)
    .execute(&mut **tx)
    .await?;
    Ok(user)
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;

    #[sqlx::test(migrations = "../../migrations")]
    async fn password_less_users_get_no_password(pool: PgPool) {
        let db = Db::from_pool(pool);
        let user = db
            .create_user_with_identity("sso", "https://idp.example", "sub", None)
            .await
            .expect("user");
        let result = db.change_password_hash(user.id, "hash", Uuid::nil()).await;
        assert!(matches!(result, Err(DbError::NotFound)));
        let found = db.find_user(user.id).await.expect("db").expect("user");
        assert!(found.password_hash.is_none());
    }
}
