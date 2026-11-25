//! Passkeys and security keys (WebAuthn): the credentials, the random handle a passkey names
//! its user with, and the ceremonies between their two requests.
//!
//! Credentials and ceremony states are webauthn-rs values stored as JSON text; this crate does
//! not look inside them.

use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

#[cfg(test)]
use crate::SecondFactorState;
use crate::second_factors::{has_totp, has_webauthn, remove_factors, replace_codes};
use crate::{Db, DbError, TenantId, UserId};

/// The most passkeys and security keys one user may have.
pub const MAX_WEBAUTHN_CREDENTIALS: i64 = 20;

/// A passkey or security key as stored.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct StoredCredential {
    pub id: Uuid,
    pub user_id: UserId,
    pub credential_id: Vec<u8>,
    /// The webauthn-rs credential, as JSON.
    pub credential: String,
    /// `passkey` or `security_key`.
    pub kind: String,
    /// The host it was made for.
    pub rp_id: String,
    pub name: String,
    pub created_at: OffsetDateTime,
    pub last_used_at: Option<OffsetDateTime>,
}

/// A credential to store.
#[derive(Clone, Copy, Debug)]
pub struct NewCredential<'a> {
    pub id: Uuid,
    pub credential_id: &'a [u8],
    /// The webauthn-rs credential, as JSON.
    pub credential: &'a str,
    pub kind: &'a str,
    pub rp_id: &'a str,
    pub name: &'a str,
}

/// A ceremony to store until its second request.
#[derive(Clone, Copy, Debug)]
pub struct NewCeremony<'a> {
    /// What the client sends back as `challenge_id`.
    pub id: Uuid,
    /// `None` for a passkey sign-in.
    pub user: Option<UserId>,
    /// `add_passkey`, `add_security_key`, `sign_in` or `reauthenticate`.
    pub kind: &'a str,
    pub origin: &'a str,
    /// The webauthn-rs state, as JSON.
    pub state: &'a str,
    pub expires_at: OffsetDateTime,
}

/// A ceremony waiting for its second request.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct Ceremony {
    pub kind: String,
    /// `None` for a passkey sign-in, which learns the user from the answer.
    pub user_id: Option<UserId>,
    /// The page that started it.
    pub origin: String,
    /// The webauthn-rs state, as JSON.
    pub state: String,
}

/// The columns of [`StoredCredential`].
macro_rules! credential_select {
    () => {
        "SELECT id, user_id, credential_id, credential::TEXT AS credential, kind, rp_id, name, \
         created_at, last_used_at FROM user_webauthn"
    };
}

impl Db {
    /// The random handle the user's passkeys carry, which becomes `new` if they have none yet.
    pub async fn webauthn_handle(&self, user: UserId, new: Uuid) -> Result<Uuid, DbError> {
        let handle = sqlx::query_scalar(
            "UPDATE users SET webauthn_handle = COALESCE(webauthn_handle, $2) \
             WHERE id = $1 RETURNING webauthn_handle",
        )
        .bind(user)
        .bind(new)
        .fetch_optional(self.pool())
        .await?;
        handle.ok_or(DbError::NotFound)
    }

    /// The user of `tenant` whose passkeys carry `handle`.
    pub async fn user_by_webauthn_handle(
        &self,
        tenant: TenantId,
        handle: Uuid,
    ) -> Result<Option<UserId>, DbError> {
        let user = sqlx::query_scalar(
            "SELECT id FROM users WHERE tenant_id = $1 AND webauthn_handle = $2",
        )
        .bind(tenant)
        .bind(handle)
        .fetch_optional(self.pool())
        .await?;
        Ok(user)
    }

    /// The user's passkeys and security keys, oldest first.
    pub async fn webauthn_credentials(
        &self,
        user: UserId,
    ) -> Result<Vec<StoredCredential>, DbError> {
        let credentials = sqlx::query_as::<_, StoredCredential>(concat!(
            credential_select!(),
            " WHERE user_id = $1 \
             ORDER BY created_at, id"
        ))
        .bind(user)
        .fetch_all(self.pool())
        .await?;
        Ok(credentials)
    }

    /// The credential of `tenant` with the authenticator's `credential_id`.
    pub async fn webauthn_credential_by_id(
        &self,
        tenant: TenantId,
        credential_id: &[u8],
    ) -> Result<Option<StoredCredential>, DbError> {
        let credential = sqlx::query_as::<_, StoredCredential>(concat!(
            credential_select!(),
            " \
             WHERE tenant_id = $1 AND credential_id = $2"
        ))
        .bind(tenant)
        .bind(credential_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(credential)
    }

    /// Whether a user of `tenant` has a passkey made for `rp_id`, so that one could sign in
    /// with it on a page there.
    pub async fn tenant_has_passkeys(
        &self,
        tenant: TenantId,
        rp_id: &str,
    ) -> Result<bool, DbError> {
        let found = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM user_webauthn \
             WHERE tenant_id = $1 AND kind = 'passkey' AND rp_id = $2)",
        )
        .bind(tenant)
        .bind(rp_id)
        .fetch_one(self.pool())
        .await?;
        Ok(found)
    }

    /// Stores a new credential of the user, in one transaction with giving them
    /// `recovery_codes` (hashes) when it is their first second factor; returns whether it was.
    /// `Conflict` when the user has [`MAX_WEBAUTHN_CREDENTIALS`] already or the authenticator
    /// registered this credential before.
    pub async fn add_webauthn_credential(
        &self,
        tenant: TenantId,
        user: UserId,
        credential: NewCredential<'_>,
        recovery_codes: &[Vec<u8>],
    ) -> Result<bool, DbError> {
        let mut tx = self.pool().begin().await?;
        // One user's additions take turns, so two at once cannot pass the limit together.
        sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
            .bind(user)
            .execute(&mut *tx)
            .await?;
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_webauthn WHERE user_id = $1")
                .bind(user)
                .fetch_one(&mut *tx)
                .await?;
        if count >= MAX_WEBAUTHN_CREDENTIALS {
            return Err(DbError::Conflict);
        }
        let first = count == 0 && !has_totp(&mut tx, user).await?;
        sqlx::query(
            "INSERT INTO user_webauthn \
             (id, tenant_id, user_id, credential_id, credential, kind, rp_id, name) \
             VALUES ($1, $2, $3, $4, $5::JSONB, $6, $7, $8)",
        )
        .bind(credential.id)
        .bind(tenant)
        .bind(user)
        .bind(credential.credential_id)
        .bind(credential.credential)
        .bind(credential.kind)
        .bind(credential.rp_id)
        .bind(credential.name)
        .execute(&mut *tx)
        .await?;
        if first {
            replace_codes(&mut tx, tenant, user, recovery_codes).await?;
        }
        tx.commit().await?;
        Ok(first)
    }

    /// Renames the user's credential `id`; `NotFound` if they have no such credential.
    pub async fn rename_webauthn_credential(
        &self,
        user: UserId,
        id: Uuid,
        name: &str,
    ) -> Result<(), DbError> {
        let renamed =
            sqlx::query("UPDATE user_webauthn SET name = $3 WHERE user_id = $1 AND id = $2")
                .bind(user)
                .bind(id)
                .bind(name)
                .execute(self.pool())
                .await?;
        if renamed.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Removes the user's credential `id`, and with it the recovery codes when it was their
    /// last second factor. `NotFound` if they have no such credential.
    pub async fn remove_webauthn_credential(&self, user: UserId, id: Uuid) -> Result<(), DbError> {
        let mut tx = self.pool().begin().await?;
        let removed = sqlx::query("DELETE FROM user_webauthn WHERE user_id = $1 AND id = $2")
            .bind(user)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if removed.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        if !has_webauthn(&mut tx, user).await? && !has_totp(&mut tx, user).await? {
            remove_factors(&mut tx, user).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Records a sign-in with credential `id`, with its new state (counter, backup flags) when
    /// it changed.
    pub async fn touch_webauthn_credential(
        &self,
        id: Uuid,
        credential: Option<&str>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE user_webauthn SET last_used_at = now(), \
             credential = COALESCE($2::JSONB, credential) WHERE id = $1",
        )
        .bind(id)
        .bind(credential)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Stores a ceremony waiting for its answer.
    pub async fn insert_webauthn_ceremony(
        &self,
        tenant: TenantId,
        ceremony: NewCeremony<'_>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO webauthn_ceremonies (id, tenant_id, user_id, kind, origin, state, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6::JSONB, $7)",
        )
        .bind(ceremony.id)
        .bind(tenant)
        .bind(ceremony.user)
        .bind(ceremony.kind)
        .bind(ceremony.origin)
        .bind(ceremony.state)
        .bind(ceremony.expires_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Takes the unexpired ceremony `id` of `tenant` if it is one of `kinds`, so it is answered
    /// only once.
    pub async fn take_webauthn_ceremony(
        &self,
        tenant: TenantId,
        id: Uuid,
        kinds: &[&str],
    ) -> Result<Option<Ceremony>, DbError> {
        let ceremony = sqlx::query_as::<_, Ceremony>(
            "DELETE FROM webauthn_ceremonies \
             WHERE id = $1 AND tenant_id = $2 AND kind = ANY($3) AND expires_at > now() \
             RETURNING kind, user_id, origin, state::TEXT AS state",
        )
        .bind(id)
        .bind(tenant)
        .bind(kinds)
        .fetch_optional(self.pool())
        .await?;
        Ok(ceremony)
    }
}

#[cfg(test)]
mod tests;
