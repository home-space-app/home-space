//! The security-key lock: the user's folder keys as the vault lists them, opening the vault
//! with one of them or with the vault's recovery key, enabling another folder key, setting the
//! recovery key, and protecting and unlocking a folder with the vault.
//!
//! Reaching the key itself is the platform's [`Authenticator`]: every function that asks it
//! blocks until the user touched the key, so clients call them off their interface's thread.

use protocol::{
    AddUnlocker, FolderLock, NodeId, ProtectFolder, ProtectedFolder, RewrappedFolder,
    RewrappedUnlocker, RotateVault, UnlockerKind, UserId, Vault, VaultUnlocker,
    encoding::Base64Url,
};
use uuid::Uuid;
use vault_core::{
    Authenticator, CredentialRef, FolderKeys, FolderSecret, PrfOutput, RecoveryKey, VaultKey,
    vault::{self, Unlocker},
};

use super::ProtectedError;

/// The epoch a new vault starts at.
const FIRST_EPOCH: i32 = 1;

/// One of the user's folder keys: a credential on a security key that opens the vault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderKey {
    /// The unlocker's id, to remove it.
    pub id: Uuid,
    /// What the user called the key.
    pub name: String,
    /// The credential, to ask the key for.
    pub credential_id: Vec<u8>,
    /// The RP id the credential was made for, passed to the key as it is.
    pub rp_id: String,
    prf_salt: [u8; 32],
    wrapped_vault_key: Vec<u8>,
}

impl FolderKey {
    /// The folder key `unlocker` is, when it is one and whole.
    pub fn from_unlocker(unlocker: &VaultUnlocker) -> Option<Self> {
        if unlocker.kind != UnlockerKind::SecurityKey {
            return None;
        }
        Some(Self {
            id: unlocker.id,
            name: unlocker.name.clone().unwrap_or_default(),
            credential_id: unlocker.credential_id.as_ref()?.as_bytes().to_vec(),
            rp_id: unlocker.rp_id.clone()?,
            prf_salt: unlocker.prf_salt.as_ref()?.as_bytes().try_into().ok()?,
            wrapped_vault_key: unlocker.wrapped_vault_key.as_bytes().to_vec(),
        })
    }

    /// The credential to ask the key for.
    pub fn credential(&self) -> CredentialRef<'_> {
        CredentialRef {
            credential_id: &self.credential_id,
            rp_id: &self.rp_id,
        }
    }
}

/// The folder keys of `vault`, in the order it lists them.
pub fn folder_keys(vault: &Vault) -> Vec<FolderKey> {
    vault
        .unlockers
        .iter()
        .filter_map(FolderKey::from_unlocker)
        .collect()
}

/// The vault's recovery key unlocker, when it has one.
pub fn recovery_unlocker(vault: &Vault) -> Option<&VaultUnlocker> {
    vault
        .unlockers
        .iter()
        .find(|unlocker| unlocker.kind == UnlockerKind::Recovery)
}

/// How the user opens the vault.
pub enum VaultProof<'a, A> {
    /// One of their folder keys, on the key `auth` reaches: one touch.
    Key {
        /// The plugged-in key that holds `key`.
        auth: &'a mut A,
        /// The folder key on it.
        key: &'a FolderKey,
    },
    /// The vault's recovery key, as typed.
    RecoveryKey(&'a str),
}

/// What answers for a recovery key, which needs no security key.
struct NoKey;

#[derive(Debug, thiserror::Error)]
#[error("no security key")]
struct NoKeyError;

impl Authenticator for NoKey {
    fn prf(&mut self, _: CredentialRef<'_>, _: &[u8; 32]) -> vault_core::Result<PrfOutput> {
        Err(vault_core::Error::Authenticator(Box::new(NoKeyError)))
    }
}

/// Opens the vault of `user` with `proof` and runs `action` with the vault key, which is
/// dropped when it returns.
pub fn with_vault<A: Authenticator, T>(
    vault: &Vault,
    user: UserId,
    proof: VaultProof<'_, A>,
    action: impl FnOnce(&VaultKey) -> Result<T, ProtectedError>,
) -> Result<T, ProtectedError> {
    let mut result = None;
    let opened = match proof {
        VaultProof::Key { auth, key } => vault::with_vault_key(
            auth,
            &user.0,
            Unlocker::SecurityKey {
                credential: key.credential(),
                prf_salt: &key.prf_salt,
                wrapped_vault_key: &key.wrapped_vault_key,
            },
            |vault_key| {
                result = Some(action(vault_key));
                Ok(())
            },
        )
        .map_err(|error| wrong(error, ProtectedError::KeyDoesNotOpen)),
        VaultProof::RecoveryKey(text) => {
            let unlocker = recovery_unlocker(vault).ok_or(ProtectedError::NoRecoveryKey)?;
            let key = RecoveryKey::parse(text).map_err(|_| ProtectedError::WrongRecoveryKey)?;
            vault::with_vault_key(
                &mut NoKey,
                &user.0,
                Unlocker::Recovery {
                    key: &key,
                    wrapped_vault_key: unlocker.wrapped_vault_key.as_bytes(),
                },
                |vault_key| {
                    result = Some(action(vault_key));
                    Ok(())
                },
            )
            .map_err(|error| wrong(error, ProtectedError::WrongRecoveryKey))
        }
    };
    opened?;
    result.unwrap_or(Err(ProtectedError::KeyDoesNotOpen))
}

/// Enables the credential `credential_id` that `auth`'s key just made for `rp_id` as the
/// folder key `name`: one touch. `vault_key` is the open vault's; `None` starts a new vault,
/// for the first unlocker. Returns what `POST /vault/unlockers` takes.
pub fn enable_folder_key(
    auth: &mut impl Authenticator,
    vault: Option<(&Vault, &VaultKey)>,
    user: UserId,
    name: &str,
    credential_id: &[u8],
    rp_id: &str,
) -> Result<AddUnlocker, ProtectedError> {
    let new;
    let (vault_key, epoch) = match vault {
        Some((vault, key)) => (key, vault.epoch),
        None => {
            new = VaultKey::generate();
            (&new, FIRST_EPOCH)
        }
    };
    let credential = CredentialRef {
        credential_id,
        rp_id,
    };
    let enabled = vault::enable_security_key(auth, credential, &user.0, vault_key)?;
    Ok(AddUnlocker {
        kind: UnlockerKind::SecurityKey,
        name: Some(name.trim().to_owned()),
        credential_id: Some(Base64Url::from(credential_id.to_vec())),
        rp_id: Some(rp_id.to_owned()),
        prf_salt: Some(Base64Url::from(enabled.prf_salt.to_vec())),
        wrapped_vault_key: Base64Url::from(enabled.wrapped_vault_key),
        epoch,
    })
}

/// A new recovery key for the open vault: what `POST /vault/unlockers` takes, and the key
/// to show once. It replaces the one before.
pub fn new_vault_recovery_key(
    vault: &Vault,
    vault_key: &VaultKey,
    user: UserId,
) -> Result<(AddUnlocker, RecoveryKey), ProtectedError> {
    let key = RecoveryKey::generate();
    let wrapped = vault::wrap_for_recovery(&key, &user.0, vault_key)?;
    let request = AddUnlocker {
        kind: UnlockerKind::Recovery,
        name: None,
        credential_id: None,
        rp_id: None,
        prf_salt: None,
        wrapped_vault_key: Base64Url::from(wrapped),
        epoch: vault.epoch,
    };
    Ok((request, key))
}

/// Protects `folder` with the open vault: a new folder secret wrapped by the vault key. Returns
/// what `POST /protected` takes and the folder's keys.
pub fn protect_with_vault(
    vault: &Vault,
    vault_key: &VaultKey,
    folder: NodeId,
) -> Result<(ProtectFolder, FolderKeys), ProtectedError> {
    let secret = FolderSecret::generate();
    let keys = secret.keys()?;
    let request = ProtectFolder {
        node_id: folder,
        lock: FolderLock::SecurityKey,
        wrapped_secret: Base64Url::from(secret.wrap(vault_key, &folder.0)?),
        recipient: keys.recipient().to_string(),
        epoch: Some(vault.epoch),
        kdf: None,
        recovery_wrapped_secret: None,
    };
    Ok((request, keys))
}

/// Opens the security-key folder `folder`, as `GET /protected` lists it, with the open vault.
/// Fails when the server's recipient for it is not its secret's.
pub fn open_with_vault(
    vault: &Vault,
    vault_key: &VaultKey,
    folder: &ProtectedFolder,
) -> Result<FolderKeys, ProtectedError> {
    if folder.lock != FolderLock::SecurityKey {
        return Err(ProtectedError::NotSecurityKey);
    }
    if folder.epoch.is_some_and(|epoch| epoch != vault.epoch) {
        return Err(ProtectedError::VaultChanged);
    }
    let keys = vault::open_folder(
        vault_key,
        vault::ProtectedFolder {
            node_id: &folder.node_id.0,
            wrapped_secret: folder.wrapped_secret.as_bytes(),
            recipient: &folder.recipient,
        },
    )?;
    Ok(keys)
}

/// What replacing the vault key gives: what `POST /vault/rotate` takes and, when the recovery
/// key could not be kept, a new one to add right after it (and to show once).
#[derive(Debug)]
pub struct Rotation {
    pub request: RotateVault,
    pub recovery: Option<(AddUnlocker, RecoveryKey)>,
}

/// How the user opens the vault to replace its key.
#[derive(Clone, Copy, Debug)]
pub enum RotationProof<'a> {
    /// A folder key that stays, on the authenticator that answers for every key that stays.
    Key(&'a FolderKey),
    /// The vault's recovery key as typed, which then stays too.
    RecoveryKey(&'a str),
}

/// Replaces the vault key of `user` and leaves out the unlockers in `remove`, such as a lost
/// key. Opens the vault with `proof` (one touch for a key), wraps the secret of every
/// security-key folder of `folders` (all of them, as `GET /protected` lists them) anew, then
/// asks every folder key that stays, on `auth`, for a new PRF salt: one touch each. The
/// recovery key stays when it is the proof; otherwise a new one replaces it.
pub fn rotate_vault(
    auth: &mut impl Authenticator,
    vault: &Vault,
    user: UserId,
    proof: RotationProof<'_>,
    folders: &[ProtectedFolder],
    remove: &[Uuid],
) -> Result<Rotation, ProtectedError> {
    let new = VaultKey::generate();
    let rewrap = |old: &VaultKey| -> Result<Vec<RewrappedFolder>, ProtectedError> {
        let mut rewrapped = Vec::new();
        for folder in folders
            .iter()
            .filter(|folder| folder.lock == FolderLock::SecurityKey)
        {
            if folder.epoch.is_some_and(|epoch| epoch != vault.epoch) {
                return Err(ProtectedError::VaultChanged);
            }
            let wrapped = vault::rewrap_folder_secret(
                old,
                &new,
                &folder.node_id.0,
                folder.wrapped_secret.as_bytes(),
            )?;
            rewrapped.push(RewrappedFolder {
                node_id: folder.node_id,
                wrapped_secret: Base64Url::from(wrapped),
            });
        }
        Ok(rewrapped)
    };
    let (rewrapped, typed) = match proof {
        RotationProof::Key(key) => {
            let proof = VaultProof::Key {
                auth: &mut *auth,
                key,
            };
            (with_vault(vault, user, proof, rewrap)?, None)
        }
        RotationProof::RecoveryKey(text) => {
            let proof = VaultProof::<NoKey>::RecoveryKey(text);
            (with_vault(vault, user, proof, rewrap)?, Some(text))
        }
    };

    let mut remove = remove.to_vec();
    let mut unlockers = Vec::new();
    for key in folder_keys(vault)
        .iter()
        .filter(|key| !remove.contains(&key.id))
    {
        let enabled = vault::enable_security_key(auth, key.credential(), &user.0, &new)?;
        unlockers.push(RewrappedUnlocker {
            id: key.id,
            prf_salt: Some(Base64Url::from(enabled.prf_salt.to_vec())),
            wrapped_vault_key: Base64Url::from(enabled.wrapped_vault_key),
        });
    }
    let mut replaced = None;
    if let Some(unlocker) =
        recovery_unlocker(vault).filter(|unlocker| !remove.contains(&unlocker.id))
    {
        match typed {
            Some(text) => {
                let key = RecoveryKey::parse(text).map_err(|_| ProtectedError::WrongRecoveryKey)?;
                let wrapped = vault::wrap_for_recovery(&key, &user.0, &new)?;
                unlockers.push(RewrappedUnlocker {
                    id: unlocker.id,
                    prf_salt: None,
                    wrapped_vault_key: Base64Url::from(wrapped),
                });
            }
            None => {
                remove.push(unlocker.id);
                // The server bumps the epoch by one; the new recovery key is added under it.
                let next = Vault {
                    epoch: vault.epoch + 1,
                    unlockers: Vec::new(),
                };
                replaced = Some(new_vault_recovery_key(&next, &new, user)?);
            }
        }
    }
    Ok(Rotation {
        request: RotateVault {
            epoch: vault.epoch,
            unlockers,
            folders: rewrapped,
            remove,
        },
        recovery: replaced,
    })
}

fn wrong(error: vault_core::Error, wrong: ProtectedError) -> ProtectedError {
    match error {
        vault_core::Error::Unwrap => wrong,
        other => other.into(),
    }
}
