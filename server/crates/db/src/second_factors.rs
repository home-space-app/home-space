//! Second factors of local accounts: the authenticator app (TOTP), recovery codes, and the
//! sign-ins that wait for one of them.

use sqlx::FromRow;
use time::OffsetDateTime;

use crate::{Db, DbError, TenantId, UserId};

/// A user's authenticator app as stored.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct StoredTotp {
    /// Sealed with the tenant's secrets key.
    pub secret: Vec<u8>,
    /// The last time step a code was accepted for.
    pub last_step: i64,
    /// `None` while the user has not entered a first code from the app.
    pub confirmed_at: Option<OffsetDateTime>,
}

/// What a user has as second factors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, FromRow)]
pub struct SecondFactorState {
    /// A confirmed authenticator app.
    pub totp: bool,
    /// Recovery codes not used yet.
    pub recovery_codes_left: i64,
}

/// A sign-in that passed the password and waits for the second factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, FromRow)]
pub struct PendingLogin {
    pub user_id: UserId,
    /// Wrong codes so far.
    pub attempts: i32,
}

impl Db {
    /// Starts adding an authenticator app with `secret`, replacing one that was never
    /// confirmed. `Conflict` while a confirmed one exists.
    pub async fn start_totp(
        &self,
        tenant: TenantId,
        user: UserId,
        secret: &[u8],
    ) -> Result<(), DbError> {
        let stored = sqlx::query(
            "INSERT INTO user_totp (tenant_id, user_id, secret) VALUES ($1, $2, $3) \
             ON CONFLICT (user_id) DO UPDATE \
             SET secret = EXCLUDED.secret, last_step = 0, created_at = now() \
             WHERE user_totp.confirmed_at IS NULL",
        )
        .bind(tenant)
        .bind(user)
        .bind(secret)
        .execute(self.pool())
        .await?;
        if stored.rows_affected() == 0 {
            return Err(DbError::Conflict);
        }
        Ok(())
    }

    /// The user's authenticator app, confirmed or not.
    pub async fn totp(&self, user: UserId) -> Result<Option<StoredTotp>, DbError> {
        let totp = sqlx::query_as::<_, StoredTotp>(
            "SELECT secret, last_step, confirmed_at FROM user_totp WHERE user_id = $1",
        )
        .bind(user)
        .fetch_optional(self.pool())
        .await?;
        Ok(totp)
    }

    /// Confirms the user's new authenticator app with a code of time step `step` and gives
    /// them `recovery_codes` (hashes) in place of any they had, in one transaction.
    /// `NotFound` when there is no unconfirmed app or `step` was already used.
    pub async fn confirm_totp(
        &self,
        tenant: TenantId,
        user: UserId,
        step: i64,
        recovery_codes: &[Vec<u8>],
    ) -> Result<(), DbError> {
        let mut tx = self.pool().begin().await?;
        let confirmed = sqlx::query(
            "UPDATE user_totp SET confirmed_at = now(), last_step = $2 \
             WHERE user_id = $1 AND confirmed_at IS NULL AND last_step < $2",
        )
        .bind(user)
        .bind(step)
        .execute(&mut *tx)
        .await?;
        if confirmed.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        replace_codes(&mut tx, tenant, user, recovery_codes).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Records that a code of time step `step` was accepted, unless that step or a later one
    /// already was: then `false`, and the code must be refused. Two requests with one code
    /// cannot both get `true`.
    pub async fn accept_totp_step(&self, user: UserId, step: i64) -> Result<bool, DbError> {
        let accepted = sqlx::query(
            "UPDATE user_totp SET last_step = $2 \
             WHERE user_id = $1 AND confirmed_at IS NOT NULL AND last_step < $2",
        )
        .bind(user)
        .bind(step)
        .execute(self.pool())
        .await?;
        Ok(accepted.rows_affected() == 1)
    }

    /// Replaces the sealed secret, after the secrets key changed.
    pub async fn reseal_totp(&self, user: UserId, secret: &[u8]) -> Result<(), DbError> {
        sqlx::query("UPDATE user_totp SET secret = $2 WHERE user_id = $1")
            .bind(user)
            .bind(secret)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Gives the user `codes` (hashes) in place of the recovery codes they had.
    pub async fn replace_recovery_codes(
        &self,
        tenant: TenantId,
        user: UserId,
        codes: &[Vec<u8>],
    ) -> Result<(), DbError> {
        let mut tx = self.pool().begin().await?;
        replace_codes(&mut tx, tenant, user, codes).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Uses up the recovery code with `hash`: `false` if the user has no such code or it was
    /// used already.
    pub async fn use_recovery_code(&self, user: UserId, hash: &[u8]) -> Result<bool, DbError> {
        let used = sqlx::query(
            "UPDATE user_recovery_codes SET used_at = now() \
             WHERE user_id = $1 AND code_hash = $2 AND used_at IS NULL",
        )
        .bind(user)
        .bind(hash)
        .execute(self.pool())
        .await?;
        Ok(used.rows_affected() == 1)
    }

    /// What the user has as second factors.
    pub async fn second_factors(&self, user: UserId) -> Result<SecondFactorState, DbError> {
        let state = sqlx::query_as::<_, SecondFactorState>(
            "SELECT EXISTS (SELECT 1 FROM user_totp \
                WHERE user_id = $1 AND confirmed_at IS NOT NULL) AS totp, \
             (SELECT COUNT(*) FROM user_recovery_codes \
                WHERE user_id = $1 AND used_at IS NULL) AS recovery_codes_left",
        )
        .bind(user)
        .fetch_one(self.pool())
        .await?;
        Ok(state)
    }

    /// Removes the user's authenticator app and recovery codes, so they sign in with the
    /// password alone. Their sessions are kept.
    pub async fn remove_second_factors(&self, user: UserId) -> Result<(), DbError> {
        let mut tx = self.pool().begin().await?;
        remove_factors(&mut tx, user).await?;
        tx.commit().await?;
        Ok(())
    }

    /// An administrator's reset: removes the second factors of `user` of `tenant` and ends
    /// all their sessions. `NotFound` if the tenant has no such user.
    pub async fn reset_second_factors(
        &self,
        tenant: TenantId,
        user: UserId,
    ) -> Result<(), DbError> {
        let mut tx = self.pool().begin().await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND tenant_id = $2)",
        )
        .bind(user)
        .bind(tenant)
        .fetch_one(&mut *tx)
        .await?;
        if !exists {
            return Err(DbError::NotFound);
        }
        remove_factors(&mut tx, user).await?;
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Stores a sign-in of `user` that waits for its second factor.
    pub async fn insert_pending_login(
        &self,
        tenant: TenantId,
        token_hash: &[u8],
        user: UserId,
        expires_at: OffsetDateTime,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO pending_logins (token_hash, tenant_id, user_id, expires_at) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(token_hash)
        .bind(tenant)
        .bind(user)
        .bind(expires_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// The unexpired sign-in of `tenant` waiting with `token_hash`.
    pub async fn pending_login(
        &self,
        tenant: TenantId,
        token_hash: &[u8],
    ) -> Result<Option<PendingLogin>, DbError> {
        let pending = sqlx::query_as::<_, PendingLogin>(
            "SELECT user_id, attempts FROM pending_logins \
             WHERE tenant_id = $1 AND token_hash = $2 AND expires_at > now()",
        )
        .bind(tenant)
        .bind(token_hash)
        .fetch_optional(self.pool())
        .await?;
        Ok(pending)
    }

    /// Counts a wrong code for the waiting sign-in, returning the wrong codes so far; `None`
    /// when it is gone.
    pub async fn count_pending_login_failure(
        &self,
        token_hash: &[u8],
    ) -> Result<Option<i32>, DbError> {
        let attempts = sqlx::query_scalar(
            "UPDATE pending_logins SET attempts = attempts + 1 WHERE token_hash = $1 \
             RETURNING attempts",
        )
        .bind(token_hash)
        .fetch_optional(self.pool())
        .await?;
        Ok(attempts)
    }

    /// Removes the waiting sign-in, returning whether it was still there, so it finishes
    /// only once.
    pub async fn take_pending_login(&self, token_hash: &[u8]) -> Result<bool, DbError> {
        let taken = sqlx::query("DELETE FROM pending_logins WHERE token_hash = $1")
            .bind(token_hash)
            .execute(self.pool())
            .await?;
        Ok(taken.rows_affected() == 1)
    }
}

async fn replace_codes(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant: TenantId,
    user: UserId,
    codes: &[Vec<u8>],
) -> Result<(), DbError> {
    sqlx::query("DELETE FROM user_recovery_codes WHERE user_id = $1")
        .bind(user)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO user_recovery_codes (tenant_id, user_id, code_hash) \
         SELECT $1, $2, code_hash FROM UNNEST($3::BYTEA[]) AS codes (code_hash)",
    )
    .bind(tenant)
    .bind(user)
    .bind(codes)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn remove_factors(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user: UserId,
) -> Result<(), DbError> {
    for query in [
        "DELETE FROM user_totp WHERE user_id = $1",
        "DELETE FROM user_recovery_codes WHERE user_id = $1",
        "DELETE FROM pending_logins WHERE user_id = $1",
    ] {
        sqlx::query(query).bind(user).execute(&mut **tx).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
