//! The vault and protected folders of a user (feature `protected_folders`).
//!
//! The server checks shapes (lengths, the recipient's form, Argon2id bounds) and consistency,
//! never keys: everything it keeps is wrapped by a security key or a password it never sees.
//! Changes to what unlocks the vault need a recent sign-in, so a stolen access token cannot add
//! an attacker's unlocker or take the user's away.

use uuid::Uuid;

use auth::AuthService;
use db::{
    Db, DbError, NewLock, NewUnlocker, NodeId, ProtectRefusal, Rewrapped, StoredProtection,
    StoredUnlocker, StoredVault, UserId,
};
use protocol::{
    AddUnlocker, ChangeLock, FolderLock, KDF_SALT_LEN, PRF_SALT_LEN, PasswordKdf, ProtectFolder,
    ProtectedFolder, ProtectionState, RotateVault, UnlockerKind, Vault, VaultUnlocker, WRAPPED_LEN,
    encoding::Base64Url, is_age_recipient,
};

use crate::AccountError;

/// The vault and protected folders of one tenant's users.
#[derive(Clone, Debug)]
pub struct VaultService {
    db: Db,
    auth: AuthService,
}

impl VaultService {
    pub fn new(db: Db, auth: AuthService) -> Self {
        Self { db, auth }
    }

    /// The user's vault; `NotFound` before they enabled a key or a recovery key.
    pub async fn vault(&self, user: UserId) -> Result<Vault, AccountError> {
        let stored = self.db.vault(user).await?.ok_or(AccountError::NotFound)?;
        Ok(vault_of(stored))
    }

    /// Enables a security key or sets the recovery key, for the user of `session`.
    pub async fn add_unlocker(
        &self,
        user: UserId,
        session: Uuid,
        request: &AddUnlocker,
    ) -> Result<VaultUnlocker, AccountError> {
        self.auth.require_recent_sign_in(user, session).await?;
        check_wrapped("/wrapped_vault_key", &request.wrapped_vault_key)?;
        match request.kind {
            UnlockerKind::SecurityKey => {
                let credential = request.credential.ok_or(AccountError::Invalid {
                    field: "/credential",
                    message: "A security key needs its credential.",
                })?;
                let salt = request.prf_salt.as_ref().ok_or(AccountError::Invalid {
                    field: "/prf_salt",
                    message: "A security key needs its PRF salt.",
                })?;
                if salt.len() != PRF_SALT_LEN {
                    return Err(AccountError::Invalid {
                        field: "/prf_salt",
                        message: "The PRF salt must be 32 bytes.",
                    });
                }
                let key = self
                    .auth
                    .webauthn_keys(user)
                    .await?
                    .into_iter()
                    .find(|key| key.id == credential)
                    .ok_or(AccountError::Invalid {
                        field: "/credential",
                        message: "There is no such passkey or security key.",
                    })?;
                if !key.can_protect_folders() {
                    return Err(AccountError::Invalid {
                        field: "/credential",
                        message: "Only a security key made with PRF can unlock protected \
                                  folders. Add the key again from this server's web page.",
                    });
                }
            }
            UnlockerKind::Recovery => {
                if request.credential.is_some() || request.prf_salt.is_some() {
                    return Err(AccountError::Invalid {
                        field: "/kind",
                        message: "A recovery key has no credential or PRF salt.",
                    });
                }
            }
        }
        let id = Uuid::now_v7();
        let new = NewUnlocker {
            id,
            kind: request.kind.as_str(),
            credential: request.credential,
            prf_salt: request.prf_salt.as_ref().map(Base64Url::as_bytes),
            wrapped_vault_key: request.wrapped_vault_key.as_bytes(),
            epoch: request.epoch,
        };
        match self.db.add_vault_unlocker(user, new).await {
            Err(DbError::Stale) => return Err(AccountError::VaultChanged),
            Err(DbError::Conflict) => {
                return Err(AccountError::Invalid {
                    field: "/credential",
                    message: "This key already unlocks the protected folders.",
                });
            }
            Err(DbError::NotFound) => {
                return Err(AccountError::Invalid {
                    field: "/credential",
                    message: "There is no such passkey or security key.",
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
                prf_salt: unlocker.prf_salt.clone().map(|salt| salt.0),
                wrapped: unlocker.wrapped_vault_key.0.clone(),
            });
        }
        let mut folders = Vec::with_capacity(request.folders.len());
        for folder in &request.folders {
            check_wrapped("/folders", &folder.wrapped_secret)?;
            folders.push(Rewrapped {
                id: folder.node_id.0,
                prf_salt: None,
                wrapped: folder.wrapped_secret.0.clone(),
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

    /// The user's protected folders.
    pub async fn folders(&self, user: UserId) -> Result<Vec<ProtectedFolder>, AccountError> {
        self.db
            .protected_folders(user)
            .await?
            .into_iter()
            .map(folder_of)
            .collect()
    }

    /// Protects one of the user's folders. An empty one is protected at once; one with content
    /// stays `protecting` until a client has converted it. Needs a recent sign-in: with a
    /// stolen token alone, nobody may lock the user's files with their own key.
    pub async fn protect(
        &self,
        user: UserId,
        session: Uuid,
        request: &ProtectFolder,
    ) -> Result<ProtectedFolder, AccountError> {
        self.auth.require_recent_sign_in(user, session).await?;
        if !is_age_recipient(&request.recipient) {
            return Err(AccountError::Invalid {
                field: "/recipient",
                message: "The recipient must be an age X25519 recipient (age1…).",
            });
        }
        let kdf = lock_kdf(
            request.lock,
            &request.wrapped_secret,
            request.epoch,
            request.kdf.as_ref(),
        )?;
        let lock = NewLock {
            lock: request.lock.as_str(),
            wrapped_secret: request.wrapped_secret.as_bytes(),
            epoch: request.epoch,
            kdf: kdf.as_deref(),
        };
        let stored = self
            .db
            .protect_folder(user, request.node_id, lock, &request.recipient)
            .await?
            .map_err(|refusal| match refusal {
                ProtectRefusal::NotFound => AccountError::NotFound,
                ProtectRefusal::SpecialFolder => {
                    AccountError::Protected("The recycle bin and Lost & Found cannot be protected.")
                }
                ProtectRefusal::Overlaps => AccountError::Protected(
                    "The folder is protected, is inside a protected folder, or holds one.",
                ),
                ProtectRefusal::VaultChanged => AccountError::VaultChanged,
            })?;
        tracing::info!(user = %user, folder = %request.node_id, lock = request.lock.as_str(), "folder protected");
        folder_of(stored)
    }

    /// Locks a protected folder with something else, its secret and recipient unchanged.
    /// Needs a recent sign-in, since a wrong wrapped secret would lock the user out.
    pub async fn change_lock(
        &self,
        user: UserId,
        session: Uuid,
        node: NodeId,
        request: &ChangeLock,
    ) -> Result<ProtectedFolder, AccountError> {
        self.auth.require_recent_sign_in(user, session).await?;
        let kdf = lock_kdf(
            request.lock,
            &request.wrapped_secret,
            request.epoch,
            request.kdf.as_ref(),
        )?;
        let lock = NewLock {
            lock: request.lock.as_str(),
            wrapped_secret: request.wrapped_secret.as_bytes(),
            epoch: request.epoch,
            kdf: kdf.as_deref(),
        };
        let stored = match self.db.change_folder_lock(user, node, lock).await {
            Err(DbError::Stale) => return Err(AccountError::VaultChanged),
            other => other?,
        };
        tracing::info!(user = %user, folder = %node, lock = request.lock.as_str(), "folder lock changed");
        folder_of(stored)
    }
}

fn check_wrapped(field: &'static str, wrapped: &Base64Url) -> Result<(), AccountError> {
    if wrapped.len() == WRAPPED_LEN {
        Ok(())
    } else {
        Err(AccountError::Invalid {
            field,
            message: "A wrapped key must be 73 bytes.",
        })
    }
}

/// Checks that a lock carries what its kind needs; the Argon2id parameters as JSON for a
/// password.
fn lock_kdf(
    lock: FolderLock,
    wrapped: &Base64Url,
    epoch: Option<i32>,
    kdf: Option<&PasswordKdf>,
) -> Result<Option<String>, AccountError> {
    check_wrapped("/wrapped_secret", wrapped)?;
    match (lock, epoch, kdf) {
        (FolderLock::SecurityKey, Some(_), None) => Ok(None),
        (FolderLock::SecurityKey, _, _) => Err(AccountError::Invalid {
            field: "/epoch",
            message: "A security-key lock needs the vault epoch and no password parameters.",
        }),
        (FolderLock::Password, None, Some(kdf)) => match kdf.problem() {
            Some(message) => Err(AccountError::Invalid {
                field: "/kdf",
                message,
            }),
            None => Ok(Some(serde_json::to_string(kdf).map_err(|_| {
                AccountError::Invalid {
                    field: "/kdf",
                    message: "The password parameters cannot be stored.",
                }
            })?)),
        },
        (FolderLock::Password, _, _) => Err(AccountError::Invalid {
            field: "/kdf",
            message: "A password lock needs its parameters and no vault epoch.",
        }),
    }
}

fn vault_of(stored: StoredVault) -> Vault {
    Vault {
        epoch: stored.epoch,
        unlockers: stored
            .unlockers
            .into_iter()
            .filter_map(unlocker_of)
            .collect(),
    }
}

fn unlocker_of(stored: StoredUnlocker) -> Option<VaultUnlocker> {
    Some(VaultUnlocker {
        id: stored.id,
        kind: UnlockerKind::parse(&stored.kind)?,
        credential: stored.credential,
        credential_id: stored.credential_id.map(Base64Url),
        rp_id: stored.rp_id,
        prf_salt: stored.prf_salt.map(Base64Url),
        wrapped_vault_key: Base64Url(stored.wrapped_vault_key),
        created_at: stored.created_at,
    })
}

fn folder_of(stored: StoredProtection) -> Result<ProtectedFolder, AccountError> {
    let broken = || AccountError::Db(DbError::Invalid);
    let kdf = stored
        .kdf
        .map(|json| serde_json::from_str::<PasswordKdf>(&json))
        .transpose()
        .map_err(|_| broken())?;
    debug_assert!(
        kdf.as_ref()
            .is_none_or(|kdf| kdf.salt.len() == KDF_SALT_LEN)
    );
    Ok(ProtectedFolder {
        node_id: stored.node_id,
        lock: FolderLock::parse(&stored.lock).ok_or_else(broken)?,
        wrapped_secret: Base64Url(stored.wrapped_secret),
        epoch: stored.epoch,
        kdf,
        recipient: stored.recipient,
        format: stored.format,
        state: ProtectionState::parse(&stored.state).ok_or_else(broken)?,
        created_at: stored.created_at,
    })
}
