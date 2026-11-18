use uuid::Uuid;

use crate::{Db, DbError, RefreshToken, TenantId};

macro_rules! token_columns {
    () => {
        "t.id, t.user_id, t.family_id, t.token_hash, t.expires_at, t.revoked_at"
    };
}

impl Db {
    /// Finds a refresh token of a user of `tenant` by its hash. A token of another tenant is
    /// not found, so replaying it on this tenant's domain cannot end its session.
    pub async fn find_refresh_token(
        &self,
        tenant: TenantId,
        token_hash: &[u8],
    ) -> Result<Option<RefreshToken>, DbError> {
        let token = sqlx::query_as::<_, RefreshToken>(concat!(
            "SELECT ",
            token_columns!(),
            " FROM refresh_tokens t JOIN users u ON u.id = t.user_id \
             WHERE t.token_hash = $1 AND u.tenant_id = $2"
        ))
        .bind(token_hash)
        .bind(tenant)
        .fetch_optional(self.pool())
        .await?;
        Ok(token)
    }

    /// Atomically revokes a token that is still active.
    ///
    /// Returns `false` when it was already revoked, so concurrent refreshes
    /// with the same token cannot both succeed.
    pub async fn revoke_refresh_token(&self, id: Uuid) -> Result<bool, DbError> {
        let updated = sqlx::query(
            "UPDATE refresh_tokens SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(id)
        .execute(self.pool())
        .await?;
        Ok(updated.rows_affected() == 1)
    }
}
