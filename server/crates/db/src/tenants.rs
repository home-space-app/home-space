use std::fmt;

use sqlx::FromRow;
use time::OffsetDateTime;

use crate::{Db, DbError};

/// Identifier of a tenant: a group of users served on its own domains, with its own signing
/// keys and identity provider. An integer, so that it fits in an environment variable name
/// (`HOMESPACE_JWT_SECRET_2`); it is never secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, sqlx::Type)]
#[sqlx(transparent)]
pub struct TenantId(pub i32);

impl TenantId {
    /// The tenant of every install from before tenants existed, and the only one in
    /// single-tenant mode.
    pub const DEFAULT: Self = Self(1);
    /// No tenant: marks rows every tenant shares, such as the count of wrong passwords from
    /// one address over all usernames.
    pub const SHARED: Self = Self(0);
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A tenant as stored: its settings and secrets live outside the database.
#[derive(Clone, Debug, FromRow)]
pub struct Tenant {
    pub id: TenantId,
    pub name: String,
    pub created_at: OffsetDateTime,
    /// When the tenant was disabled; it then answers no requests and nobody signs in to it.
    pub disabled_at: Option<OffsetDateTime>,
    /// When the tenant and all its users are deleted.
    pub delete_at: Option<OffsetDateTime>,
}

impl Tenant {
    /// Whether the tenant serves requests: neither disabled nor marked for deletion.
    pub fn is_active(&self) -> bool {
        self.disabled_at.is_none() && self.delete_at.is_none()
    }
}

/// A host a tenant answers on.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct TenantDomain {
    /// The host, lower-cased, with `:port` when it is not the default one.
    pub domain: String,
    pub tenant_id: TenantId,
    /// The domain the tenant's public URL is made from; one per tenant.
    pub is_primary: bool,
}

impl Db {
    /// Every tenant, by id.
    pub async fn list_tenants(&self) -> Result<Vec<Tenant>, DbError> {
        let tenants = sqlx::query_as::<_, Tenant>(
            "SELECT id, name, created_at, disabled_at, delete_at FROM tenants ORDER BY id",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(tenants)
    }

    /// Every domain of every tenant, by tenant and with the primary one first.
    pub async fn list_tenant_domains(&self) -> Result<Vec<TenantDomain>, DbError> {
        let domains = sqlx::query_as::<_, TenantDomain>(
            "SELECT domain, tenant_id, is_primary FROM tenant_domains \
             ORDER BY tenant_id, is_primary DESC, domain",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(domains)
    }
}

/// Adds a tenant for tests that need a second one.
#[cfg(test)]
pub(crate) async fn add_test_tenant(db: &crate::Db) -> TenantId {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Other') RETURNING id")
        .fetch_one(db.pool())
        .await
        .expect("tenant")
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;

    #[sqlx::test(migrations = "../../migrations")]
    async fn existing_installs_have_only_the_default_tenant(pool: PgPool) {
        let db = Db::from_pool(pool);
        let tenants = db.list_tenants().await.expect("tenants");
        assert_eq!(tenants.len(), 1);
        assert_eq!(tenants[0].id, TenantId::DEFAULT);
        assert!(tenants[0].is_active());
        assert!(db.list_tenant_domains().await.expect("domains").is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn new_tenants_never_take_the_default_id(pool: PgPool) {
        let db = Db::from_pool(pool);
        assert_eq!(add_test_tenant(&db).await, TenantId(2));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_tenant_has_one_primary_domain(pool: PgPool) {
        let db = Db::from_pool(pool.clone());
        let insert = |domain: &'static str, primary: bool| {
            sqlx::query(
                "INSERT INTO tenant_domains (domain, tenant_id, is_primary) VALUES ($1, 1, $2)",
            )
            .bind(domain)
            .bind(primary)
            .execute(&pool)
        };
        insert("files.example.com", true).await.expect("primary");
        insert("old.example.com", false).await.expect("alias");
        assert!(insert("new.example.com", true).await.is_err());
        assert!(insert("Files.Example.com", false).await.is_err());
        let domains = db.list_tenant_domains().await.expect("domains");
        assert_eq!(domains[0].domain, "files.example.com");
        assert!(domains[0].is_primary);
    }
}
