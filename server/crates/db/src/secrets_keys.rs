//! What a tenant keeps sealed with its secrets key, the check that tells which key that is, and
//! replacing all of it at once when the key is rotated.

use sqlx::{Postgres, Transaction};

use crate::{Db, DbError, TenantId, UserId};

/// An authenticator app secret as stored, sealed for `user`.
#[derive(Clone, Debug, PartialEq, Eq, sqlx::FromRow)]
pub struct SealedTotp {
    pub user: UserId,
    pub sealed: Vec<u8>,
}

/// Everything of a tenant sealed with its secrets key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SealedSecrets {
    pub totp: Vec<SealedTotp>,
}

impl SealedSecrets {
    pub fn is_empty(&self) -> bool {
        self.totp.is_empty()
    }
}

/// A rotation of a tenant's secrets key under way: the tables it rewrites are locked against
/// writes until it is finished or dropped, so nothing sealed with the old key slips in.
pub struct SecretsRotation {
    tx: Transaction<'static, Postgres>,
    tenant: TenantId,
    /// The check stored before the rotation.
    pub check: Option<Vec<u8>>,
    /// What is sealed now; [`finish`](Self::finish) must be given all of it sealed again.
    pub sealed: SealedSecrets,
}

impl Db {
    /// The sealed check of `tenant`'s secrets key; `None` before one was recorded.
    pub async fn secrets_key_check(&self, tenant: TenantId) -> Result<Option<Vec<u8>>, DbError> {
        let check =
            sqlx::query_scalar("SELECT sealed FROM secrets_key_checks WHERE tenant_id = $1")
                .bind(tenant)
                .fetch_optional(self.pool())
                .await?;
        Ok(check)
    }

    /// Records `sealed` as `tenant`'s check unless it has one; whether it was recorded.
    pub async fn record_secrets_key_check(
        &self,
        tenant: TenantId,
        sealed: &[u8],
    ) -> Result<bool, DbError> {
        let recorded = sqlx::query(
            "INSERT INTO secrets_key_checks (tenant_id, sealed) VALUES ($1, $2) \
             ON CONFLICT (tenant_id) DO NOTHING",
        )
        .bind(tenant)
        .bind(sealed)
        .execute(self.pool())
        .await?;
        Ok(recorded.rows_affected() == 1)
    }

    /// Everything of `tenant` sealed with its secrets key.
    pub async fn sealed_secrets(&self, tenant: TenantId) -> Result<SealedSecrets, DbError> {
        let mut conn = self.pool().acquire().await?;
        read_sealed(&mut conn, tenant).await
    }

    /// Ends every session of `tenant`'s users, for a rotation of its signing secret alone (it
    /// has no secrets key); returns how many were ended.
    pub async fn end_tenant_sessions(&self, tenant: TenantId) -> Result<u64, DbError> {
        let mut conn = self.pool().acquire().await?;
        end_tenant_sessions(&mut conn, tenant).await
    }

    /// Starts rotating `tenant`'s secrets key: locks what is sealed with it and reads it.
    pub async fn begin_secrets_rotation(
        &self,
        tenant: TenantId,
    ) -> Result<SecretsRotation, DbError> {
        let mut tx = self.begin().await?;
        // Blocks every write to these tables (reads go on) until the rotation ends.
        sqlx::query(
            "LOCK TABLE user_totp, secrets_key_checks \
             IN SHARE ROW EXCLUSIVE MODE",
        )
        .execute(&mut *tx)
        .await?;
        let check =
            sqlx::query_scalar("SELECT sealed FROM secrets_key_checks WHERE tenant_id = $1")
                .bind(tenant)
                .fetch_optional(&mut *tx)
                .await?;
        let sealed = read_sealed(&mut tx, tenant).await?;
        Ok(SecretsRotation {
            tx,
            tenant,
            check,
            sealed,
        })
    }
}

impl SecretsRotation {
    /// Stores `resealed`, everything read when the rotation began sealed with the new key,
    /// and `check` for that key, and ends every session of the tenant. Returns how many
    /// sessions were ended. Nothing is kept unless all of it is.
    ///
    /// # Errors
    ///
    /// [`DbError::Stale`] when `resealed` leaves out or adds something, or names it for
    /// another owner.
    pub async fn finish(mut self, check: &[u8], resealed: &SealedSecrets) -> Result<u64, DbError> {
        if !same_rows(&self.sealed, resealed) {
            return Err(DbError::Stale);
        }
        for totp in &resealed.totp {
            sqlx::query("UPDATE user_totp SET secret = $3 WHERE tenant_id = $1 AND user_id = $2")
                .bind(self.tenant)
                .bind(totp.user)
                .bind(&totp.sealed)
                .execute(&mut *self.tx)
                .await?;
        }
        sqlx::query(
            "INSERT INTO secrets_key_checks (tenant_id, sealed) VALUES ($1, $2) \
             ON CONFLICT (tenant_id) DO UPDATE SET sealed = EXCLUDED.sealed, created_at = now()",
        )
        .bind(self.tenant)
        .bind(check)
        .execute(&mut *self.tx)
        .await?;
        let ended = end_tenant_sessions(&mut self.tx, self.tenant).await?;
        self.tx.commit().await?;
        Ok(ended)
    }
}

/// Whether `resealed` names exactly the rows of `sealed`, in the same order.
fn same_rows(sealed: &SealedSecrets, resealed: &SealedSecrets) -> bool {
    let totp = sealed.totp.iter().map(|row| row.user);
    totp.eq(resealed.totp.iter().map(|row| row.user))
}

async fn read_sealed(
    conn: &mut sqlx::PgConnection,
    tenant: TenantId,
) -> Result<SealedSecrets, DbError> {
    let totp = sqlx::query_as::<_, SealedTotp>(
        "SELECT user_id AS user, secret AS sealed FROM user_totp \
         WHERE tenant_id = $1 ORDER BY user_id",
    )
    .bind(tenant)
    .fetch_all(conn)
    .await?;
    Ok(SealedSecrets { totp })
}

/// Ends every session of the tenant's users: everyone has to sign in again, on every device.
async fn end_tenant_sessions(
    conn: &mut sqlx::PgConnection,
    tenant: TenantId,
) -> Result<u64, DbError> {
    let ended = sqlx::query(
        "DELETE FROM sessions s USING users u WHERE u.id = s.user_id AND u.tenant_id = $1",
    )
    .bind(tenant)
    .execute(conn)
    .await?;
    Ok(ended.rows_affected())
}

#[cfg(test)]
mod tests;
