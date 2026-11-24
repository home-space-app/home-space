use sqlx::FromRow;
use time::OffsetDateTime;

use crate::{Db, DbError, TenantId};

/// An app waiting in the system browser for a signed-in user to let it sign in.
#[derive(Clone, Debug, FromRow)]
pub struct BrowserLogin {
    /// Where the browser goes with the code or the refusal.
    pub client_redirect: String,
    /// S256 challenge of the verifier the app presents with its code.
    pub client_challenge: String,
    /// The name the app gave for its device.
    pub device_name: Option<String>,
}

impl Db {
    /// Remembers a browser sign-in to `tenant` until `expires_at`, unless `tenant` already has
    /// `max_pending` unexpired ones waiting. Returns whether it was remembered.
    pub async fn insert_browser_login(
        &self,
        tenant: TenantId,
        flow_hash: &[u8],
        login: &BrowserLogin,
        expires_at: OffsetDateTime,
        max_pending: u32,
    ) -> Result<bool, DbError> {
        let inserted = sqlx::query(
            "INSERT INTO browser_logins \
             (tenant_id, flow_hash, client_redirect, client_challenge, device_name, expires_at) \
             SELECT $1, $2, $3, $4, $5, $6 \
             WHERE (SELECT count(*) FROM browser_logins \
                    WHERE tenant_id = $1 AND expires_at > now()) < $7",
        )
        .bind(tenant)
        .bind(flow_hash)
        .bind(&login.client_redirect)
        .bind(&login.client_challenge)
        .bind(login.device_name.as_deref())
        .bind(expires_at)
        .bind(i64::from(max_pending))
        .execute(self.pool())
        .await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// The unexpired browser sign-in of `tenant`, left in place.
    pub async fn browser_login(
        &self,
        tenant: TenantId,
        flow_hash: &[u8],
    ) -> Result<Option<BrowserLogin>, DbError> {
        let login = sqlx::query_as::<_, BrowserLogin>(
            "SELECT client_redirect, client_challenge, device_name FROM browser_logins \
             WHERE tenant_id = $1 AND flow_hash = $2 AND expires_at > now()",
        )
        .bind(tenant)
        .bind(flow_hash)
        .fetch_optional(self.pool())
        .await?;
        Ok(login)
    }

    /// Removes and returns the unexpired browser sign-in of `tenant`, so it is answered only
    /// once.
    pub async fn take_browser_login(
        &self,
        tenant: TenantId,
        flow_hash: &[u8],
    ) -> Result<Option<BrowserLogin>, DbError> {
        let login = sqlx::query_as::<_, BrowserLogin>(
            "DELETE FROM browser_logins \
             WHERE tenant_id = $1 AND flow_hash = $2 AND expires_at > now() \
             RETURNING client_redirect, client_challenge, device_name",
        )
        .bind(tenant)
        .bind(flow_hash)
        .fetch_optional(self.pool())
        .await?;
        Ok(login)
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use time::Duration;

    use super::*;

    fn login() -> BrowserLogin {
        BrowserLogin {
            client_redirect: "http://127.0.0.1:4711/callback".to_owned(),
            client_challenge: "challenge".to_owned(),
            device_name: Some("Laptop".to_owned()),
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_browser_login_is_answered_once_and_only_on_its_tenant(pool: PgPool) {
        let db = Db::from_pool(pool);
        let other = crate::tenants::add_test_tenant(&db).await;
        let expires_at = OffsetDateTime::now_utc() + Duration::minutes(10);
        db.insert_browser_login(other, b"flow", &login(), expires_at, 10)
            .await
            .expect("insert");

        assert!(
            db.browser_login(TenantId::DEFAULT, b"flow")
                .await
                .expect("db")
                .is_none()
        );
        assert!(
            db.take_browser_login(TenantId::DEFAULT, b"flow")
                .await
                .expect("db")
                .is_none()
        );
        let seen = db.browser_login(other, b"flow").await.expect("db");
        assert_eq!(seen.expect("there").device_name.as_deref(), Some("Laptop"));
        let taken = db
            .take_browser_login(other, b"flow")
            .await
            .expect("db")
            .expect("still there");
        assert_eq!(taken.client_redirect, "http://127.0.0.1:4711/callback");
        assert!(
            db.take_browser_login(other, b"flow")
                .await
                .expect("db")
                .is_none()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn an_expired_browser_login_is_gone_and_pruned(pool: PgPool) {
        let db = Db::from_pool(pool);
        let past = OffsetDateTime::now_utc() - Duration::seconds(1);
        db.insert_browser_login(TenantId::DEFAULT, b"old", &login(), past, 10)
            .await
            .expect("insert");
        assert!(
            db.browser_login(TenantId::DEFAULT, b"old")
                .await
                .expect("db")
                .is_none()
        );
        assert_eq!(db.prune_sign_ins().await.expect("prune"), 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_tenant_has_only_so_many_browser_logins_waiting(pool: PgPool) {
        let db = Db::from_pool(pool);
        let other = crate::tenants::add_test_tenant(&db).await;
        let soon = OffsetDateTime::now_utc() + Duration::minutes(10);
        let past = OffsetDateTime::now_utc() - Duration::seconds(1);
        let insert = |tenant, flow: &'static [u8], expires_at| {
            let db = db.clone();
            async move {
                db.insert_browser_login(tenant, flow, &login(), expires_at, 2)
                    .await
                    .expect("insert")
            }
        };

        assert!(insert(TenantId::DEFAULT, b"expired", past).await);
        assert!(insert(TenantId::DEFAULT, b"one", soon).await);
        assert!(insert(TenantId::DEFAULT, b"two", soon).await);
        assert!(!insert(TenantId::DEFAULT, b"three", soon).await);
        assert!(insert(other, b"three", soon).await);
        db.take_browser_login(TenantId::DEFAULT, b"one")
            .await
            .expect("db")
            .expect("there");
        assert!(insert(TenantId::DEFAULT, b"four", soon).await);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_long_redirect_is_not_stored(pool: PgPool) {
        let db = Db::from_pool(pool);
        let soon = OffsetDateTime::now_utc() + Duration::minutes(10);
        let long = BrowserLogin {
            client_redirect: format!("http://127.0.0.1:1/{}", "x".repeat(200)),
            ..login()
        };
        assert!(
            db.insert_browser_login(TenantId::DEFAULT, b"long", &long, soon, 10)
                .await
                .is_err()
        );
    }
}
