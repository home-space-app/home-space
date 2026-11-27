use sqlx::FromRow;
use time::OffsetDateTime;

use crate::{Authentication, Db, DbError, TenantId, UserId};

/// A one-time code issued to a client after a successful sign-in, through single sign-on or
/// the browser sign-in of an app.
#[derive(Clone, Debug, FromRow)]
pub struct LoginCode {
    pub user_id: UserId,
    /// S256 challenge of the verifier the client must present with the code.
    pub client_challenge: String,
    /// How the user signed in, for the session the code starts.
    #[sqlx(flatten)]
    pub authentication: Authentication,
}

impl Db {
    /// Stores the one-time code a client of `tenant` exchanges for tokens.
    pub async fn insert_login_code(
        &self,
        tenant: TenantId,
        code_hash: &[u8],
        code: &LoginCode,
        expires_at: OffsetDateTime,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO login_codes \
             (tenant_id, code_hash, user_id, client_challenge, auth_methods, authenticated_at, \
              second_factor_at, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(tenant)
        .bind(code_hash)
        .bind(code.user_id)
        .bind(&code.client_challenge)
        .bind(&code.authentication.auth_methods)
        .bind(code.authentication.authenticated_at)
        .bind(code.authentication.second_factor_at)
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
             RETURNING user_id, client_challenge, auth_methods, authenticated_at, \
             second_factor_at",
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

    fn code(user_id: UserId) -> LoginCode {
        LoginCode {
            user_id,
            client_challenge: "challenge".to_owned(),
            authentication: Authentication {
                auth_methods: vec!["pwd".to_owned()],
                authenticated_at: OffsetDateTime::now_utc() - Duration::minutes(5),
                second_factor_at: None,
            },
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_login_code_is_taken_once_and_only_by_its_tenant(pool: PgPool) {
        let db = Db::from_pool(pool);
        let other = crate::tenants::add_test_tenant(&db).await;
        let alice = db.create_user(other, "alice", "x").await.expect("alice").id;
        let expires_at = OffsetDateTime::now_utc() + Duration::seconds(60);
        let issued = code(alice);
        db.insert_login_code(other, b"code", &issued, expires_at)
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
        let issued = issued.authentication;
        assert_eq!(here.authentication.auth_methods, issued.auth_methods);
        let drift = here.authentication.authenticated_at - issued.authenticated_at;
        assert!(drift.abs() < Duration::milliseconds(1), "kept as issued");
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
        db.insert_login_code(TenantId::DEFAULT, b"old", &code(alice), past)
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
