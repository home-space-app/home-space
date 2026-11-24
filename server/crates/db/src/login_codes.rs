use sqlx::FromRow;
use time::OffsetDateTime;

use crate::{Db, DbError, TenantId, UserId};

/// A one-time code issued to a client after a successful sign-in, through single sign-on or
/// the browser sign-in of an app.
#[derive(Clone, Debug, FromRow)]
pub struct LoginCode {
    pub user_id: UserId,
    /// S256 challenge of the verifier the client must present with the code.
    pub client_challenge: String,
}

impl Db {
    /// Stores the one-time code a client of `tenant` exchanges for tokens.
    pub async fn insert_login_code(
        &self,
        tenant: TenantId,
        code_hash: &[u8],
        user_id: UserId,
        client_challenge: &str,
        expires_at: OffsetDateTime,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO login_codes (tenant_id, code_hash, user_id, client_challenge, expires_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(tenant)
        .bind(code_hash)
        .bind(user_id)
        .bind(client_challenge)
        .bind(expires_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Removes and returns an unexpired login code of `tenant`, so a code works only once. A
    /// code of another tenant is left alone.
    pub async fn take_login_code(
        &self,
        tenant: TenantId,
        code_hash: &[u8],
    ) -> Result<Option<LoginCode>, DbError> {
        let code = sqlx::query_as::<_, LoginCode>(
            "DELETE FROM login_codes \
             WHERE tenant_id = $1 AND code_hash = $2 AND expires_at > now() \
             RETURNING user_id, client_challenge",
        )
        .bind(tenant)
        .bind(code_hash)
        .fetch_optional(self.pool())
        .await?;
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use time::Duration;

    use super::*;

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_login_code_is_taken_once_and_only_by_its_tenant(pool: PgPool) {
        let db = Db::from_pool(pool);
        let other = crate::tenants::add_test_tenant(&db).await;
        let alice = db.create_user(other, "alice", "x").await.expect("alice").id;
        let expires_at = OffsetDateTime::now_utc() + Duration::seconds(60);
        db.insert_login_code(other, b"code", alice, "challenge", expires_at)
            .await
            .expect("insert");

        let elsewhere = db
            .take_login_code(TenantId::DEFAULT, b"code")
            .await
            .expect("db");
        assert!(elsewhere.is_none());
        let here = db
            .take_login_code(other, b"code")
            .await
            .expect("db")
            .expect("still there for its own tenant");
        assert_eq!(here.user_id, alice);
        assert_eq!(here.client_challenge, "challenge");
        let again = db.take_login_code(other, b"code").await.expect("db");
        assert!(again.is_none(), "a code works only once");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn an_expired_login_code_is_refused_and_pruned(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = db
            .create_user(TenantId::DEFAULT, "alice", "x")
            .await
            .expect("alice")
            .id;
        let past = OffsetDateTime::now_utc() - Duration::seconds(1);
        db.insert_login_code(TenantId::DEFAULT, b"old", alice, "challenge", past)
            .await
            .expect("insert");
        let taken = db
            .take_login_code(TenantId::DEFAULT, b"old")
            .await
            .expect("db");
        assert!(taken.is_none());
        assert_eq!(db.prune_sign_ins().await.expect("prune"), 1);
    }
}
