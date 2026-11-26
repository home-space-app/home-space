//! Keeping a tenant's secrets key and what it seals together: the check that tells whether the
//! configured key is the one the tenant's secrets are sealed with, and sealing all of them
//! again with a new key, which is the only way to change the key without losing them.

use db::{Db, DbError, SealedFolderKey, SealedSecrets, SealedTotp, TenantId, UserId};

use crate::{AuthError, SecretBox};

/// Why a secrets key cannot be used, or the secrets could not be sealed again.
#[derive(Debug, thiserror::Error)]
pub enum SecretsKeyError {
    /// Secrets are sealed with a key, and none is configured.
    #[error("this tenant's secrets are sealed with a secrets key, and none is set")]
    Missing,
    /// The configured key is not the one the secrets are sealed with.
    #[error("the secrets key set is not the one this tenant's secrets are sealed with")]
    Wrong,
    /// A sealed secret opens with none of the keys given; names it.
    #[error("{0} opens with none of the secrets keys given")]
    Unopened(String),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Db(#[from] DbError),
}

/// What [`check`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCheck {
    /// The key is the one the stored check was sealed with.
    Matches,
    /// There was no check yet; the key opens everything sealed, and a check for it is stored
    /// from now on.
    Recorded,
    /// No key is set and nothing is sealed.
    Unused,
}

/// Checks that `key` is the secrets key `tenant`'s secrets are sealed with, recording a
/// check for it the first time. The server does not serve a tenant this fails for: a key
/// changed by hand would leave its authenticator apps and password folders unreadable.
///
/// # Errors
///
/// [`SecretsKeyError::Missing`] without a key while something needs one,
/// [`SecretsKeyError::Wrong`] for another key, and database failures.
pub async fn check(
    db: &Db,
    tenant: TenantId,
    key: Option<&SecretBox>,
) -> Result<KeyCheck, SecretsKeyError> {
    if let Some(stored) = db.secrets_key_check(tenant).await? {
        let key = key.ok_or(SecretsKeyError::Missing)?;
        return if key.opens_check(tenant, &stored) {
            Ok(KeyCheck::Matches)
        } else {
            Err(SecretsKeyError::Wrong)
        };
    }
    // Before the first check: the key is right when it opens everything sealed.
    let sealed = db.sealed_secrets(tenant).await?;
    let Some(key) = key else {
        return if sealed.is_empty() {
            Ok(KeyCheck::Unused)
        } else {
            Err(SecretsKeyError::Missing)
        };
    };
    if reseal(tenant, &sealed, std::slice::from_ref(key), key).is_err() {
        return Err(SecretsKeyError::Wrong);
    }
    db.record_secrets_key_check(tenant, &key.seal_check(tenant)?)
        .await?;
    Ok(KeyCheck::Recorded)
}

/// What [`rotate`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rotation {
    /// Everything was sealed again with the new key, and everyone signed out.
    Rotated {
        authenticator_apps: usize,
        password_folders: usize,
        sessions_ended: u64,
    },
    /// The secrets are sealed with the new key already: an earlier rotation to it finished.
    AlreadyDone,
}

/// Seals everything of `tenant` sealed now again with `new`, stores the check for `new` and
/// ends every session of the tenant, all in one transaction. What is sealed now is opened
/// with whichever of `current` opens it: the configured key and, while installs move off it,
/// the old `HOMESPACE_SECRETS_KEY_PREVIOUS`.
///
/// The server must not run meanwhile, or it would go on sealing with the old key.
///
/// # Errors
///
/// [`SecretsKeyError::Wrong`] when none of `current` is the key of the stored check,
/// [`SecretsKeyError::Unopened`] when a secret opens with none of them, and database
/// failures. Nothing is changed then.
pub async fn rotate(
    db: &Db,
    tenant: TenantId,
    current: &[SecretBox],
    new: &SecretBox,
) -> Result<Rotation, SecretsKeyError> {
    let rotation = db.begin_secrets_rotation(tenant).await?;
    if let Some(stored) = &rotation.check {
        if new.opens_check(tenant, stored) {
            return Ok(Rotation::AlreadyDone);
        }
        if !current.iter().any(|key| key.opens_check(tenant, stored)) {
            return Err(if current.is_empty() {
                SecretsKeyError::Missing
            } else {
                SecretsKeyError::Wrong
            });
        }
    }
    let resealed = reseal(tenant, &rotation.sealed, current, new)?;
    let check = new.seal_check(tenant)?;
    let (authenticator_apps, password_folders) = (resealed.totp.len(), resealed.folders.len());
    let sessions_ended = rotation.finish(&check, &resealed).await?;
    Ok(Rotation::Rotated {
        authenticator_apps,
        password_folders,
        sessions_ended,
    })
}

/// `sealed`, each opened with whichever of `current` opens it and sealed with `new`.
fn reseal(
    tenant: TenantId,
    sealed: &SealedSecrets,
    current: &[SecretBox],
    new: &SecretBox,
) -> Result<SealedSecrets, SecretsKeyError> {
    let again = |user: UserId, sealed: &[u8], what: &dyn Fn() -> String| {
        let plain = current
            .iter()
            .find_map(|key| key.open(tenant, user, sealed).ok())
            .ok_or_else(|| SecretsKeyError::Unopened(what()))?;
        Ok::<_, SecretsKeyError>(new.seal(tenant, user, &plain)?)
    };
    let totp = sealed
        .totp
        .iter()
        .map(|row| {
            let what = || format!("the authenticator app of user {}", row.user);
            Ok(SealedTotp {
                user: row.user,
                sealed: again(row.user, &row.sealed, &what)?,
            })
        })
        .collect::<Result<_, SecretsKeyError>>()?;
    let folders = sealed
        .folders
        .iter()
        .map(|row| {
            let what = || format!("the key of password folder {}", row.node);
            Ok(SealedFolderKey {
                node: row.node,
                owner: row.owner,
                sealed: again(row.owner, &row.sealed, &what)?,
            })
        })
        .collect::<Result<_, SecretsKeyError>>()?;
    Ok(SealedSecrets { totp, folders })
}

#[cfg(test)]
mod tests;
