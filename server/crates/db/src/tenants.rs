use std::fmt;

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

/// Adds a tenant for tests that need a second one.
#[cfg(test)]
pub(crate) async fn add_test_tenant(db: &crate::Db) -> TenantId {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Other') RETURNING id")
        .fetch_one(db.pool())
        .await
        .expect("tenant")
}
