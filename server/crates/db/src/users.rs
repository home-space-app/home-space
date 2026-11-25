use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{Db, DbError, FOLDER_MIME, LOST_AND_FOUND, NodeId, TenantId, User, UserId};

macro_rules! user_columns {
    () => {
        "id, tenant_id, username, password_hash, created_at, is_admin, disabled_at, quota_bytes, delete_at"
    };
}

impl Db {
    /// Inserts a new user of `tenant` with their recycle bin and Lost & Found folders;
    /// `Conflict` if the username is taken in that tenant.
    pub async fn create_user(
        &self,
        tenant: TenantId,
        username: &str,
        password_hash: &str,
    ) -> Result<User, DbError> {
        let mut tx = self.pool().begin().await?;
        let user = insert_user(&mut tx, tenant, username, Some(password_hash)).await?;
        tx.commit().await?;
        Ok(user)
    }

    /// Inserts a password-less user of `tenant` linked to an identity provider account.
    /// `Conflict` if the username or the identity already exists in that tenant.
    pub async fn create_user_with_identity(
        &self,
        tenant: TenantId,
        username: &str,
        issuer: &str,
        subject: &str,
        email: Option<&str>,
    ) -> Result<User, DbError> {
        let mut tx = self.pool().begin().await?;
        let user = insert_user(&mut tx, tenant, username, None).await?;
        sqlx::query(
            "INSERT INTO user_identities (tenant_id, issuer, subject, user_id, email) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(tenant)
        .bind(issuer)
        .bind(subject)
        .bind(user.id)
        .bind(email)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(user)
    }

    /// Finds the user of `tenant` linked to an identity provider account.
    pub async fn find_user_by_identity(
        &self,
        tenant: TenantId,
        issuer: &str,
        subject: &str,
    ) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as::<_, User>(concat!(
            "SELECT ",
            user_columns!(),
            " FROM users WHERE id = \
             (SELECT user_id FROM user_identities \
             WHERE tenant_id = $1 AND issuer = $2 AND subject = $3)"
        ))
        .bind(tenant)
        .bind(issuer)
        .bind(subject)
        .fetch_optional(self.pool())
        .await?;
        Ok(user)
    }

    /// Finds a user of `tenant` by username.
    pub async fn find_user_by_username(
        &self,
        tenant: TenantId,
        username: &str,
    ) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as::<_, User>(concat!(
            "SELECT ",
            user_columns!(),
            " FROM users WHERE tenant_id = $1 AND username = $2"
        ))
        .bind(tenant)
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

    /// Lists the users of `tenant` ordered by username.
    pub async fn list_users(&self, tenant: TenantId) -> Result<Vec<User>, DbError> {
        let users = sqlx::query_as::<_, User>(concat!(
            "SELECT ",
            user_columns!(),
            " FROM users WHERE tenant_id = $1 ORDER BY username"
        ))
        .bind(tenant)
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
    /// asked for the change, and the sessions `keep` started for its extensions. The current
    /// password was given for it, so `keep`'s user has just confirmed it is them.
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
        // The sessions `keep` started for its extensions stay signed in with it. They are told
        // apart by their parent, never by the device id, which any session can claim. When
        // `keep` is an extension's session its app session stays too, since ending that would
        // end `keep` with it.
        sqlx::query(
            "DELETE FROM sessions s WHERE s.user_id = $1 AND s.id <> $2 \
             AND s.parent_session_id IS DISTINCT FROM $2 \
             AND s.id IS DISTINCT FROM (SELECT parent_session_id FROM sessions WHERE id = $2)",
        )
        .bind(id)
        .bind(keep)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE sessions SET authenticated_at = now() WHERE id = $1 AND user_id = $2")
            .bind(keep)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

/// Inserts a user with their recycle bin and Lost & Found folders inside `tx`.
async fn insert_user(
    tx: &mut Transaction<'_, Postgres>,
    tenant: TenantId,
    username: &str,
    password_hash: Option<&str>,
) -> Result<User, DbError> {
    let user = sqlx::query_as::<_, User>(concat!(
        "INSERT INTO users (id, tenant_id, username, password_hash) VALUES ($1, $2, $3, $4) \
         RETURNING ",
        user_columns!(),
    ))
    .bind(UserId::new())
    .bind(tenant)
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
            .create_user_with_identity(TenantId::DEFAULT, "sso", "https://idp.example", "sub", None)
            .await
            .expect("user");
        let result = db.change_password_hash(user.id, "hash", Uuid::nil()).await;
        assert!(matches!(result, Err(DbError::NotFound)));
        let found = db.find_user(user.id).await.expect("db").expect("user");
        assert!(found.password_hash.is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn usernames_are_unique_per_tenant(pool: PgPool) {
        let db = Db::from_pool(pool);
        let other = crate::tenants::add_test_tenant(&db).await;
        let here = db
            .create_user(TenantId::DEFAULT, "admin", "hash")
            .await
            .expect("user");
        let there = db.create_user(other, "admin", "hash").await.expect("user");
        assert_eq!(there.tenant_id, other);
        assert!(matches!(
            db.create_user(other, "admin", "hash").await,
            Err(DbError::Conflict)
        ));
        let found = db
            .find_user_by_username(TenantId::DEFAULT, "admin")
            .await
            .expect("db")
            .expect("user");
        assert_eq!(found.id, here.id);
        let listed = db.list_users(other).await.expect("users");
        assert_eq!(
            listed.iter().map(|user| user.id).collect::<Vec<_>>(),
            [there.id]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn identities_are_per_tenant(pool: PgPool) {
        let db = Db::from_pool(pool);
        let other = crate::tenants::add_test_tenant(&db).await;
        let issuer = "https://idp.example";
        let here = db
            .create_user_with_identity(TenantId::DEFAULT, "maria", issuer, "sub", None)
            .await
            .expect("user");
        let there = db
            .create_user_with_identity(other, "maria", issuer, "sub", None)
            .await
            .expect("the same account may sign in to another tenant");
        let found = db
            .find_user_by_identity(other, issuer, "sub")
            .await
            .expect("db")
            .expect("user");
        assert_eq!(found.id, there.id);
        assert_ne!(found.id, here.id);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn an_identity_takes_its_users_tenant(pool: PgPool) {
        let db = Db::from_pool(pool.clone());
        let other = crate::tenants::add_test_tenant(&db).await;
        let user = db
            .create_user(TenantId::DEFAULT, "alice", "hash")
            .await
            .expect("user");
        let linked = sqlx::query(
            "INSERT INTO user_identities (tenant_id, issuer, subject, user_id) \
             VALUES ($1, 'https://idp.example', 'sub', $2)",
        )
        .bind(other)
        .bind(user.id)
        .execute(&pool)
        .await;
        assert!(linked.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn new_users_must_name_their_tenant(pool: PgPool) {
        let inserted = sqlx::query("INSERT INTO users (id, username) VALUES ($1, 'alice')")
            .bind(UserId::new())
            .execute(&pool)
            .await;
        assert!(inserted.is_err());
    }
}
