//! The vault key and what unlocks it: security keys and the recovery key.

use uuid::Uuid;

use db::{
    DbError, FolderKey, NewUnlocker, Rewrapped, StoredUnlocker, StoredVault, UnlockerSource, UserId,
};
use protocol::{
    AddUnlocker, MAX_CREDENTIAL_ID_LEN, PRF_SALT_LEN, RotateVault, UnlockerKind, Vault,
    VaultUnlocker, encoding::Base64Url,
};

use super::{VaultService, check_wrapped};
use crate::AccountError;

impl VaultService {
    /// The user's vault; `NotFound` before they enabled a key or a recovery key.
    pub async fn vault(&self, user: UserId) -> Result<Vault, AccountError> {
        let stored = self.db.vault(user).await?.ok_or(AccountError::NotFound)?;
        Ok(vault_of(stored))
    }

    /// Enables a folder key or sets the recovery key, for the user of `session`. A folder key's
    /// `rp_id` must be a host `own_host` accepts, where the tenant's pages are.
    pub async fn add_unlocker(
        &self,
        user: UserId,
        session: Uuid,
        request: &AddUnlocker,
        own_host: impl Fn(&str) -> bool,
    ) -> Result<VaultUnlocker, AccountError> {
        self.auth.require_recent_sign_in(user, session).await?;
        check_wrapped("/wrapped_vault_key", &request.wrapped_vault_key)?;
        let source = match request.kind {
            UnlockerKind::SecurityKey => {
                UnlockerSource::SecurityKey(folder_key(request, own_host)?)
            }
            UnlockerKind::Recovery => {
                if request.name.is_some()
                    || request.credential_id.is_some()
                    || request.rp_id.is_some()
                    || request.prf_salt.is_some()
                {
                    return Err(AccountError::Invalid {
                        field: "/kind",
                        message: "A recovery key has no name, credential or PRF salt.",
                    });
                }
                UnlockerSource::Recovery
            }
        };
        let id = Uuid::now_v7();
        let new = NewUnlocker {
            id,
            source,
            wrapped_vault_key: request.wrapped_vault_key.as_bytes(),
            epoch: request.epoch,
        };
        match self.db.add_vault_unlocker(user, new).await {
            Err(DbError::Stale) => return Err(AccountError::VaultChanged),
            Err(DbError::Conflict) => {
                return Err(AccountError::Invalid {
                    field: "/credential_id",
                    message: "This key already unlocks the protected folders.",
                });
            }
            other => other?,
        }
        tracing::info!(user = %user, kind = request.kind.as_str(), "vault unlocker added");
        self.vault(user)
            .await?
            .unlockers
            .into_iter()
            .find(|unlocker| unlocker.id == id)
            .ok_or(AccountError::NotFound)
    }

    /// Removes an unlocker; refused while it is the last one of protected folders.
    pub async fn remove_unlocker(
        &self,
        user: UserId,
        session: Uuid,
        id: Uuid,
    ) -> Result<(), AccountError> {
        self.auth.require_recent_sign_in(user, session).await?;
        match self.db.remove_vault_unlocker(user, id).await {
            Err(DbError::Conflict) => return Err(AccountError::LastUnlocker),
            other => other?,
        }
        tracing::info!(user = %user, "vault unlocker removed");
        Ok(())
    }

    /// Replaces the vault key, with everything wrapped under the new one.
    pub async fn rotate(
        &self,
        user: UserId,
        session: Uuid,
        request: &RotateVault,
    ) -> Result<Vault, AccountError> {
        self.auth.require_recent_sign_in(user, session).await?;
        let mut unlockers = Vec::with_capacity(request.unlockers.len());
        for unlocker in &request.unlockers {
            check_wrapped("/unlockers", &unlocker.wrapped_vault_key)?;
            if unlocker
                .prf_salt
                .as_ref()
                .is_some_and(|salt| salt.len() != PRF_SALT_LEN)
            {
                return Err(AccountError::Invalid {
                    field: "/unlockers",
                    message: "A PRF salt must be 32 bytes.",
                });
            }
            unlockers.push(Rewrapped {
                id: unlocker.id,
                prf_salt: unlocker
                    .prf_salt
                    .as_ref()
                    .map(|salt| salt.as_bytes().to_vec()),
                wrapped: unlocker.wrapped_vault_key.as_bytes().to_vec(),
            });
        }
        let mut folders = Vec::with_capacity(request.folders.len());
        for folder in &request.folders {
            check_wrapped("/folders", &folder.wrapped_secret)?;
            folders.push(Rewrapped {
                id: folder.node_id.0,
                prf_salt: None,
                wrapped: folder.wrapped_secret.as_bytes().to_vec(),
            });
        }
        let rotated = self
            .db
            .rotate_vault(user, request.epoch, &unlockers, &folders, &request.remove)
            .await;
        match rotated {
            Err(DbError::Stale) => return Err(AccountError::VaultChanged),
            Err(DbError::Conflict) => return Err(AccountError::LastUnlocker),
            Err(DbError::Invalid) => {
                return Err(AccountError::Invalid {
                    field: "",
                    message: "List every unlocker that stays and every folder locked with a \
                              security key, each once.",
                });
            }
            other => other?,
        };
        tracing::info!(user = %user, "vault key rotated");
        self.vault(user).await
    }
}

/// The folder key `request` enables, once each of its fields is there and fits.
fn folder_key<'a>(
    request: &'a AddUnlocker,
    own_host: impl Fn(&str) -> bool,
) -> Result<FolderKey<'a>, AccountError> {
    let missing = |field| AccountError::Invalid {
        field,
        message: "A folder key needs its name, credential id, RP id and PRF salt.",
    };
    let name = request.name.as_deref().ok_or_else(|| missing("/name"))?;
    let credential_id = request
        .credential_id
        .as_ref()
        .ok_or_else(|| missing("/credential_id"))?;
    let rp_id = request.rp_id.as_deref().ok_or_else(|| missing("/rp_id"))?;
    let prf_salt = request
        .prf_salt
        .as_ref()
        .ok_or_else(|| missing("/prf_salt"))?;
    if credential_id.is_empty() || credential_id.len() > MAX_CREDENTIAL_ID_LEN {
        return Err(AccountError::Invalid {
            field: "/credential_id",
            message: "A credential id is 1 to 1023 bytes.",
        });
    }
    if !own_host(rp_id) {
        return Err(AccountError::Invalid {
            field: "/rp_id",
            message: "The key must be made for an address of this server.",
        });
    }
    if prf_salt.len() != PRF_SALT_LEN {
        return Err(AccountError::Invalid {
            field: "/prf_salt",
            message: "The PRF salt must be 32 bytes.",
        });
    }
    Ok(FolderKey {
        name: name.trim(),
        credential_id: credential_id.as_bytes(),
        rp_id,
        prf_salt: prf_salt.as_bytes(),
    })
}

fn vault_of(stored: StoredVault) -> Vault {
    Vault {
        epoch: stored.epoch,
        unlockers: stored.unlockers.into_iter().map(unlocker_of).collect(),
    }
}

fn unlocker_of(stored: StoredUnlocker) -> VaultUnlocker {
    VaultUnlocker {
        id: stored.id,
        kind: stored.kind,
        name: stored.name,
        credential_id: stored.credential_id.map(Base64Url::from),
        rp_id: stored.rp_id,
        prf_salt: stored.prf_salt.map(Base64Url::from),
        wrapped_vault_key: Base64Url::from(stored.wrapped_vault_key),
        created_at: stored.created_at,
    }
}
