//! The vault and protected folders of a user (feature `protected_folders`).
//!
//! For security-key folders the server checks shapes (lengths, the recipient's form) and
//! consistency, never keys: everything it keeps is wrapped by a security key it never sees.
//! Password folders are the server's own: it keeps their key sealed with its secrets key and
//! opens it to sessions that gave the password.
//!
//! Changes to what unlocks a folder need a recent sign-in, so a stolen access token cannot add
//! an attacker's unlocker or take the user's away.

use std::net::IpAddr;

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use auth::{AuthError, AuthService};
use db::{
    Db, DbError, KeyLock, NewProtection, NewUnlocker, NodeId, PasswordLock, ProtectRefusal,
    Rewrapped, StoredProtection, StoredUnlocker, StoredVault, UserId,
};
use protocol::{
    AddUnlocker, FolderLock, FolderUnlocked, PRF_SALT_LEN, ProtectFolder, ProtectedFolder,
    ProtectionState, RotateVault, UnlockerKind, Vault, VaultUnlocker, WRAPPED_LEN,
    encoding::Base64Url, is_age_recipient, rules::password_problem,
};
use vault_core::{FolderKeys, FolderSecret};

/// How long a password folder stays unlocked for a session that stopped using it.
pub const FOLDER_UNLOCK_TTL: Duration = Duration::minutes(15);

use crate::AccountError;

mod names;

pub use names::Shown;

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

    /// Protects one of the user's folders. An empty one is protected at once. One with
    /// content stays `protecting` while the client replaces it: it downloads the files,
    /// deletes them and purges them from the recycle bin (their history goes with them), and
    /// uploads them encrypted, then calls [`Self::finish`]. Needs a recent sign-in: with a
    /// stolen token alone, nobody may lock the user's files with their own key.
    pub async fn protect(
        &self,
        user: UserId,
        session: Uuid,
        request: &ProtectFolder,
    ) -> Result<ProtectedFolder, AccountError> {
        self.auth.require_recent_sign_in(user, session).await?;
        if request.lock == FolderLock::Password {
            let folder = self.protect_with_password(user, request).await?;
            // Whoever just set the password has the folder open, as after an unlock.
            let expires_at = OffsetDateTime::now_utc() + FOLDER_UNLOCK_TTL;
            self.db
                .unlock_folder(session, request.node_id, expires_at)
                .await?;
            return Ok(folder);
        }
        let wrapped = request
            .wrapped_secret
            .as_ref()
            .ok_or(AccountError::Invalid {
                field: "/wrapped_secret",
                message: "A security-key lock needs the wrapped folder secret.",
            })?;
        check_wrapped("/wrapped_secret", wrapped)?;
        let epoch = request.epoch.ok_or(AccountError::Invalid {
            field: "/epoch",
            message: "A security-key lock needs the vault epoch.",
        })?;
        let recipient = request
            .recipient
            .as_deref()
            .filter(|recipient| is_age_recipient(recipient))
            .ok_or(AccountError::Invalid {
                field: "/recipient",
                message: "The recipient must be an age X25519 recipient (age1…).",
            })?;
        let lock = NewProtection::SecurityKey(KeyLock {
            wrapped_secret: wrapped.as_bytes(),
            epoch,
            recipient,
        });
        self.store_protection(user, request.node_id, lock).await
    }

    /// Protects a folder with a password: a new random key, sealed with the server's secrets
    /// key, and the password's hash. The server encrypts what the folder holds.
    async fn protect_with_password(
        &self,
        user: UserId,
        request: &ProtectFolder,
    ) -> Result<ProtectedFolder, AccountError> {
        if !self.auth.keeps_folder_keys() {
            return Err(AccountError::Invalid {
                field: "/lock",
                message: "This server has no secrets key, so it cannot keep the key of a \
                          password folder.",
            });
        }
        let password = request.password.as_deref().ok_or(AccountError::Invalid {
            field: "/password",
            message: "A password lock needs the password.",
        })?;
        if let Some(message) = password_problem(password) {
            return Err(AccountError::Invalid {
                field: "/password",
                message,
            });
        }
        let secret = FolderSecret::generate();
        self.check_names_fit(user, request.node_id, secret.keys())
            .await?;
        let recipient = secret.recipient().to_string();
        let sealed = self
            .auth
            .seal_folder_key(user, secret.to_raw().as_slice())?;
        drop(secret);
        let hash = self.auth.hash_folder_password(password).await?;
        let lock = NewProtection::Password(PasswordLock {
            password_hash: &hash,
            sealed_key: &sealed,
            recipient: &recipient,
        });
        self.store_protection(user, request.node_id, lock).await
    }

    /// Refuses a folder holding a name too long to be sealed, which the server would otherwise
    /// have to shorten.
    async fn check_names_fit(
        &self,
        user: UserId,
        folder: NodeId,
        keys: FolderKeys,
    ) -> Result<(), AccountError> {
        let names = files::Names::sealed(folder, keys);
        for node in self.db.folder_nodes(user, folder).await? {
            if names.store(node.kind, &node.name).is_err() {
                return Err(AccountError::Invalid {
                    field: "/node_id",
                    message: "A name inside the folder is longer than 159 bytes, the most a \
                              password folder holds. Rename it first.",
                });
            }
        }
        Ok(())
    }

    async fn store_protection(
        &self,
        user: UserId,
        node: NodeId,
        lock: NewProtection<'_>,
    ) -> Result<ProtectedFolder, AccountError> {
        let stored = self
            .db
            .protect_folder(user, node, lock)
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
        tracing::info!(user = %user, folder = %node, lock = %stored.lock, "folder protected");
        folder_of(stored)
    }

    /// Unlocks the password folder `node` for `session` with its `password`.
    pub async fn unlock(
        &self,
        user: UserId,
        session: Uuid,
        node: NodeId,
        password: &str,
        address: Option<IpAddr>,
    ) -> Result<FolderUnlocked, AccountError> {
        let secrets = self
            .db
            .password_secrets(user, node)
            .await?
            .ok_or(AccountError::NotFound)?;
        match self
            .auth
            .check_folder_password(node, secrets.password_hash, password, address)
            .await
        {
            Err(AuthError::InvalidCredentials) => {
                return Err(AccountError::Auth(AuthError::WrongPassword));
            }
            other => other?,
        }
        let expires_at = OffsetDateTime::now_utc() + FOLDER_UNLOCK_TTL;
        self.db.unlock_folder(session, node, expires_at).await?;
        tracing::info!(user = %user, folder = %node, "password folder unlocked");
        Ok(FolderUnlocked { expires_at })
    }

    /// Locks the password folder `node` for `session` again.
    pub async fn lock(
        &self,
        user: UserId,
        session: Uuid,
        node: NodeId,
    ) -> Result<(), AccountError> {
        if self.db.password_secrets(user, node).await?.is_none() {
            return Err(AccountError::NotFound);
        }
        self.db.lock_folder(session, node).await?;
        Ok(())
    }

    /// The keys to read `node` through `session`: `None` outside password folders, `Locked`
    /// in one the session has not unlocked. Using the folder keeps it unlocked.
    pub async fn read_keys(
        &self,
        user: UserId,
        session: Uuid,
        node: NodeId,
    ) -> Result<Option<FolderKeys>, AccountError> {
        let Some(folder) = self.db.enclosing_password_folder(user, node).await? else {
            return Ok(None);
        };
        let extend_to = OffsetDateTime::now_utc() + FOLDER_UNLOCK_TTL;
        if !self
            .db
            .use_unlocked_folder(session, folder.node_id, extend_to)
            .await?
        {
            return Err(AccountError::Locked);
        }
        self.folder_keys(user, folder.node_id).await.map(Some)
    }

    /// The keys of the password folder `node`, for the server's own work on it.
    pub async fn folder_keys(
        &self,
        user: UserId,
        node: NodeId,
    ) -> Result<FolderKeys, AccountError> {
        let secrets = self
            .db
            .password_secrets(user, node)
            .await?
            .ok_or(AccountError::NotFound)?;
        let raw = self.auth.open_folder_key(user, &secrets.sealed_key)?;
        let bytes: [u8; 32] = raw
            .as_slice()
            .try_into()
            .map_err(|_| AccountError::Auth(AuthError::Unsealing))?;
        Ok(FolderSecret::from_raw(bytes).keys())
    }

    /// Refuses to move `sources` into `target` (or the root) across the edge of a password
    /// folder: what is inside one is stored encrypted and everything else is not.
    pub async fn check_move(
        &self,
        user: UserId,
        sources: &[NodeId],
        target: Option<NodeId>,
    ) -> Result<(), AccountError> {
        let to = match target {
            Some(target) => self
                .db
                .enclosing_password_folder(user, target)
                .await?
                .map(|folder| folder.node_id),
            None => None,
        };
        for &source in sources {
            let from = match self.db.enclosing_password_folder(user, source).await? {
                // The folder itself moves with what it holds.
                Some(folder) if folder.node_id == source => to.map(|_| source),
                found => found.map(|folder| folder.node_id),
            };
            if from != to {
                return Err(AccountError::Protected(
                    "Items cannot be moved into or out of a password folder. Copy them instead.",
                ));
            }
        }
        Ok(())
    }

    /// The password folders of this tenant the server still has to encrypt or decrypt, with
    /// their keys.
    pub async fn converting(
        &self,
    ) -> Result<Vec<(UserId, NodeId, ProtectionState, FolderKeys)>, AccountError> {
        let mut found = Vec::new();
        for (user, node, state) in self
            .db
            .converting_password_folders(self.auth.tenant())
            .await?
        {
            let Some(state) = ProtectionState::parse(&state) else {
                continue;
            };
            match self.folder_keys(user, node).await {
                Ok(keys) => found.push((user, node, state, keys)),
                Err(err) => {
                    tracing::error!(user = %user, folder = %node, error = %err, "password folder key cannot be opened");
                }
            }
        }
        Ok(found)
    }

    /// Encrypts what this tenant's `protecting` password folders still hold in plain, with
    /// `nodes`, and marks each `protected` once it is done; returns how many it finished.
    /// Safe to run again, and at the same time as another run.
    pub async fn convert_folders(&self, nodes: &files::NodeService) -> Result<usize, AccountError> {
        let mut finished = 0;
        for (user, node, state, keys) in self.converting().await? {
            if state != ProtectionState::Protecting {
                continue;
            }
            nodes
                .encrypt_folder(user, node, &files::Names::sealed(node, keys))
                .await?;
            match self.finish_encrypting(user, node).await {
                Ok(()) => finished += 1,
                // Another run finished it, or the folder went.
                Err(AccountError::Protected(_) | AccountError::NotFound) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(finished)
    }

    /// Records that the server encrypted everything in a `protecting` password folder.
    pub async fn finish_encrypting(&self, user: UserId, node: NodeId) -> Result<(), AccountError> {
        self.change_state(
            user,
            node,
            ProtectionState::Protecting,
            ProtectionState::Protected,
        )
        .await
        .map(drop)
    }

    /// Records that the client has replaced the content of a `protecting` security-key folder
    /// with its encrypted form. The server finishes password folders itself.
    pub async fn finish(
        &self,
        user: UserId,
        node: NodeId,
    ) -> Result<ProtectedFolder, AccountError> {
        self.require_key_lock(
            user,
            node,
            "The server finishes protecting a password folder itself.",
        )
        .await?;
        self.change_state(
            user,
            node,
            ProtectionState::Protecting,
            ProtectionState::Protected,
        )
        .await
    }

    /// Starts unprotecting: the client replaces the content with its decrypted form, then
    /// calls [`Self::remove`]. Needs a recent sign-in, since the folder's secret goes at the
    /// end.
    pub async fn unprotect(
        &self,
        user: UserId,
        session: Uuid,
        node: NodeId,
    ) -> Result<ProtectedFolder, AccountError> {
        self.auth.require_recent_sign_in(user, session).await?;
        self.require_key_lock(user, node, "Password folders cannot be unprotected yet.")
            .await?;
        self.change_state(
            user,
            node,
            ProtectionState::Protected,
            ProtectionState::Unprotecting,
        )
        .await
    }

    /// Forgets a folder's protection once its content is plain again: after unprotecting, or to
    /// cancel protecting. A `protected` folder is refused, since its secret would go and its
    /// files could not be read any more. Needs a recent sign-in.
    pub async fn remove(
        &self,
        user: UserId,
        session: Uuid,
        node: NodeId,
    ) -> Result<(), AccountError> {
        self.auth.require_recent_sign_in(user, session).await?;
        self.require_key_lock(user, node, "Password folders cannot be unprotected yet.")
            .await?;
        let states = [
            ProtectionState::Protecting.as_str(),
            ProtectionState::Unprotecting.as_str(),
        ];
        match self.db.remove_protection(user, node, &states).await {
            Err(DbError::Stale) => {
                return Err(AccountError::Protected(
                    "Unprotect the folder first, so that its files are readable without the key.",
                ));
            }
            other => other?,
        }
        tracing::info!(user = %user, folder = %node, "folder protection removed");
        Ok(())
    }

    /// Refuses with `refusal` unless `node` is a security-key folder of the user; `NotFound`
    /// when it is not protected.
    async fn require_key_lock(
        &self,
        user: UserId,
        node: NodeId,
        refusal: &'static str,
    ) -> Result<(), AccountError> {
        let folder = self
            .db
            .protected_folder(user, node)
            .await?
            .ok_or(AccountError::NotFound)?;
        if folder.lock == FolderLock::Password.as_str() {
            return Err(AccountError::Protected(refusal));
        }
        Ok(())
    }

    async fn change_state(
        &self,
        user: UserId,
        node: NodeId,
        from: ProtectionState,
        to: ProtectionState,
    ) -> Result<ProtectedFolder, AccountError> {
        let changed = self
            .db
            .set_protection_state(user, node, &[from.as_str()], to.as_str())
            .await;
        let stored = match changed {
            Err(DbError::Stale) => {
                return Err(AccountError::Protected(match from {
                    ProtectionState::Protecting => "The folder is not being protected.",
                    _ => "The folder is not protected.",
                }));
            }
            other => other?,
        };
        tracing::info!(user = %user, folder = %node, state = to.as_str(), "folder protection changed");
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
    Ok(ProtectedFolder {
        node_id: stored.node_id,
        lock: FolderLock::parse(&stored.lock).ok_or_else(broken)?,
        wrapped_secret: stored.wrapped_secret.map(Base64Url),
        epoch: stored.epoch,
        recipient: stored.recipient,
        format: stored.format,
        state: ProtectionState::parse(&stored.state).ok_or_else(broken)?,
        created_at: stored.created_at,
    })
}
