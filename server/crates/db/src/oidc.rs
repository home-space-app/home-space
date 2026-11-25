use sqlx::FromRow;
use time::OffsetDateTime;

use crate::{Db, DbError, TenantId};

/// A sign-in sent to the identity provider that has not come back yet.
#[derive(Clone, Debug, FromRow)]
pub struct OidcPending {
    pub nonce: String,
    pub idp_pkce_verifier: String,
    /// Where the client wants to be sent once the sign-in is done.
    pub client_redirect: String,
    /// S256 challenge of the verifier the client will present when exchanging its code.
    pub client_challenge: String,
}

impl Db {
    /// Remembers a sign-in to `tenant` until the provider sends the user back.
    pub async fn insert_oidc_pending(
        &self,
        tenant: TenantId,
        state_hash: &[u8],
        pending: &OidcPending,
        expires_at: OffsetDateTime,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO oidc_pending \
             (tenant_id, state_hash, nonce, idp_pkce_verifier, client_redirect, client_challenge, \
             expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(tenant)
        .bind(state_hash)
        .bind(&pending.nonce)
        .bind(&pending.idp_pkce_verifier)
        .bind(&pending.client_redirect)
        .bind(&pending.client_challenge)
        .bind(expires_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Removes and returns an unexpired pending sign-in to `tenant`, so a `state` works only
    /// once. One sent from another tenant is left alone and not returned.
    pub async fn take_oidc_pending(
        &self,
        tenant: TenantId,
        state_hash: &[u8],
    ) -> Result<Option<OidcPending>, DbError> {
        let pending = sqlx::query_as::<_, OidcPending>(
            "DELETE FROM oidc_pending \
             WHERE tenant_id = $1 AND state_hash = $2 AND expires_at > now() \
             RETURNING nonce, idp_pkce_verifier, client_redirect, client_challenge",
        )
        .bind(tenant)
        .bind(state_hash)
        .fetch_optional(self.pool())
        .await?;
        Ok(pending)
    }

    /// Deletes expired pending sign-ins, browser sign-ins, login codes and sign-ins waiting for a
    /// second factor, returning how many
    /// were removed.
    pub async fn prune_sign_ins(&self) -> Result<u64, DbError> {
        let mut removed = 0;
        for query in [
            "DELETE FROM oidc_pending WHERE expires_at <= now()",
            "DELETE FROM browser_logins WHERE expires_at <= now()",
            "DELETE FROM login_codes WHERE expires_at <= now()",
            "DELETE FROM pending_logins WHERE expires_at <= now()",
        ] {
            removed += sqlx::query(query)
                .execute(self.pool())
                .await?
                .rows_affected();
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use time::Duration;

    use super::*;

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_pending_sign_in_comes_back_only_to_its_tenant(pool: PgPool) {
        let db = Db::from_pool(pool);
        let other = crate::tenants::add_test_tenant(&db).await;
        let pending = OidcPending {
            nonce: "nonce".to_owned(),
            idp_pkce_verifier: "verifier".to_owned(),
            client_redirect: "https://files.example/".to_owned(),
            client_challenge: "challenge".to_owned(),
        };
        let expires_at = OffsetDateTime::now_utc() + Duration::minutes(10);
        db.insert_oidc_pending(TenantId::DEFAULT, b"state", &pending, expires_at)
            .await
            .expect("insert");
        let elsewhere = db.take_oidc_pending(other, b"state").await.expect("db");
        assert!(elsewhere.is_none());
        let here = db
            .take_oidc_pending(TenantId::DEFAULT, b"state")
            .await
            .expect("db")
            .expect("still there for its own tenant");
        assert_eq!(here.nonce, "nonce");
    }
}
