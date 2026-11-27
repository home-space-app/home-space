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

use auth::{AuthError, AuthService, FolderSecretKind};
use db::{
    Db, DbError, FolderKey, KeyLock, NewProtection, NewUnlocker, NodeId, PasswordLock,
    ProtectError, ProtectRefusal, Rewrapped, StoredProtection, StoredUnlocker, StoredVault,
    UnlockerSource, UserId,
};
use protocol::{
    AddUnlocker, ChangeFolderPassword, FolderLock, FolderPasswordChanged, FolderUnlocked,
    MAX_CREDENTIAL_ID_LEN, PRF_SALT_LEN, ProtectFolder, ProtectedFolder, ProtectionState,
    RotateVault, UnlockerKind, Vault, VaultUnlocker, WRAPPED_LEN, encoding::Base64Url,
    is_age_recipient, rules::password_problem,
};
use vault_core::{FolderKeys, FolderSecret, RecoveryKey};

/// How long a password folder stays unlocked for a session that stopped using it.
pub const FOLDER_UNLOCK_TTL: Duration = Duration::minutes(15);

use crate::AccountError;

mod names;

pub use names::Shown;

/// A password folder the server still has to encrypt or decrypt, with its keys.
pub struct ConvertingFolder {
    pub user: UserId,
    pub node: NodeId,
    pub state: ProtectionState,
    pub keys: FolderKeys,
}

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

    /// The user's protected folders.
    pub async fn folders(&self, user: UserId) -> Result<Vec<ProtectedFolder>, AccountError> {
        let stored = self.db.protected_folders(user).await?;
        Ok(stored.into_iter().map(folder_of).collect())
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
        self.check_names_fit(user, request.node_id, secret.keys()?)
            .await?;
        let recipient = secret.recipient()?.to_string();
        let sealed = self
            .auth
            .seal_folder_key(user, secret.to_raw().as_slice())?;
        drop(secret);
        let hash = self.auth.hash_folder_password(password).await?;
        let recovery = RecoveryKey::generate().to_display();
        let recovery_hash = self.auth.hash_folder_password(&recovery).await?;
        let lock = NewProtection::Password(PasswordLock {
            password_hash: &hash,
            recovery_hash: &recovery_hash,
            sealed_key: &sealed,
            recipient: &recipient,
        });
        let mut folder = self.store_protection(user, request.node_id, lock).await?;
        folder.recovery_key = Some(recovery.to_string());
        Ok(folder)
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
            .await
            .map_err(|err| match err {
                ProtectError::Refused(ProtectRefusal::NotFound) => AccountError::NotFound,
                ProtectError::Refused(ProtectRefusal::SpecialFolder) => {
                    AccountError::Protected("The recycle bin and Lost & Found cannot be protected.")
                }
                ProtectError::Refused(ProtectRefusal::Overlaps) => AccountError::Protected(
                    "The folder is protected, is inside a protected folder, or holds one.",
                ),
                ProtectError::Refused(ProtectRefusal::VaultChanged) => AccountError::VaultChanged,
                ProtectError::Db(err) => err.into(),
            })?;
        tracing::info!(user = %user, folder = %node, lock = stored.lock.as_str(), "folder protected");
        Ok(folder_of(stored))
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
            .check_folder_password(
                node,
                FolderSecretKind::Password,
                secrets.password_hash,
                password,
                address,
            )
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
        Ok(FolderSecret::from_raw(bytes).keys()?)
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
    pub async fn converting(&self) -> Result<Vec<ConvertingFolder>, AccountError> {
        let mut found = Vec::new();
        for (user, node, state) in self
            .db
            .converting_password_folders(self.auth.tenant())
            .await?
        {
            match self.folder_keys(user, node).await {
                Ok(keys) => found.push(ConvertingFolder {
                    user,
                    node,
                    state,
                    keys,
                }),
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
        for ConvertingFolder {
            user,
            node,
            state,
            keys,
        } in self.converting().await?
        {
            if state == ProtectionState::Unprotecting {
                if self
                    .decrypt_folder(nodes, user, node, files::Names::unsealing(node, keys))
                    .await?
                {
                    finished += 1;
                }
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

    /// Decrypts an `unprotecting` password folder and, once a pass finds nothing left to
    /// decrypt, drops its protection with its key. Whether it did.
    async fn decrypt_folder(
        &self,
        nodes: &files::NodeService,
        user: UserId,
        node: NodeId,
        names: files::Names,
    ) -> Result<bool, AccountError> {
        nodes.decrypt_folder(user, node, &names).await?;
        if nodes.decrypt_folder(user, node, &names).await? > 0 {
            return Ok(false);
        }
        match self
            .db
            .remove_protection(user, node, &[ProtectionState::Unprotecting])
            .await
        {
            Ok(()) => {
                tracing::info!(user = %user, folder = %node, "password folder unprotected");
                Ok(true)
            }
            // Another run finished it, or the folder went.
            Err(DbError::Stale | DbError::NotFound) => Ok(false),
            Err(err) => Err(err.into()),
        }
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
        let folder = self
            .db
            .protected_folder(user, node)
            .await?
            .ok_or(AccountError::NotFound)?;
        if folder.lock == FolderLock::Password {
            // The server decrypts it, for a session that knows the password.
            self.require_unlocked(session, node).await?;
        }
        self.change_state(
            user,
            node,
            ProtectionState::Protected,
            ProtectionState::Unprotecting,
        )
        .await
    }

    /// `Locked` unless `session` has the password folder `node` unlocked; using it keeps it so.
    async fn require_unlocked(&self, session: Uuid, node: NodeId) -> Result<(), AccountError> {
        let extend_to = OffsetDateTime::now_utc() + FOLDER_UNLOCK_TTL;
        if self
            .db
            .use_unlocked_folder(session, node, extend_to)
            .await?
        {
            Ok(())
        } else {
            Err(AccountError::Locked)
        }
    }

    /// `Locked` unless `session` has unlocked every password folder that is `node` or below it:
    /// for changes that take a whole subtree along, such as deleting it.
    pub async fn require_unlocked_below(
        &self,
        user: UserId,
        session: Uuid,
        node: NodeId,
    ) -> Result<(), AccountError> {
        for folder in self.db.password_folders_under(user, node).await? {
            self.require_unlocked(session, folder).await?;
        }
        Ok(())
    }

    /// Refuses with `refusal` when any of `nodes` is a protected folder or inside one.
    pub async fn refuse_protected(
        &self,
        user: UserId,
        nodes: &[NodeId],
        refusal: &'static str,
    ) -> Result<(), AccountError> {
        if self.db.any_protected(user, nodes).await? {
            Err(AccountError::Protected(refusal))
        } else {
            Ok(())
        }
    }

    /// Sets a new password for the password folder `node`, proven by its current password or,
    /// when that is forgotten, its recovery key, which is then replaced: signing in to the
    /// account is never enough. The folder's key stays, so nothing is encrypted again; the
    /// caller's session has the folder unlocked and every other one has to unlock it with the
    /// new password. Wrong proofs are throttled like wrong unlocks.
    pub async fn change_password(
        &self,
        user: UserId,
        session: Uuid,
        node: NodeId,
        request: &ChangeFolderPassword,
        address: Option<IpAddr>,
    ) -> Result<FolderPasswordChanged, AccountError> {
        self.auth.require_recent_sign_in(user, session).await?;
        let secrets = self
            .db
            .password_secrets(user, node)
            .await?
            .ok_or(AccountError::NotFound)?;
        if let Some(message) = password_problem(&request.password) {
            return Err(AccountError::Invalid {
                field: "/password",
                message,
            });
        }
        let new_recovery = match (&request.current_password, &request.recovery_key) {
            (Some(current), None) => {
                let kind = FolderSecretKind::Password;
                self.check_folder_secret(node, kind, secrets.password_hash, current, address)
                    .await?;
                None
            }
            (None, Some(typed)) => {
                let key = RecoveryKey::parse(typed).map_err(|_| AccountError::Invalid {
                    field: "/recovery_key",
                    message: "That is not a recovery key: 55 letters and digits in 11 groups.",
                })?;
                let (kind, typed) = (FolderSecretKind::RecoveryKey, key.to_display());
                self.check_folder_secret(node, kind, secrets.recovery_hash, &typed, address)
                    .await?;
                Some(RecoveryKey::generate().to_display())
            }
            _ => {
                return Err(AccountError::Invalid {
                    field: "/current_password",
                    message: "Give the folder's current password, or its recovery key.",
                });
            }
        };
        let hash = self.auth.hash_folder_password(&request.password).await?;
        let recovery_hash = match &new_recovery {
            Some(key) => Some(self.auth.hash_folder_password(key).await?),
            None => None,
        };
        self.db
            .set_folder_password(user, node, &hash, recovery_hash.as_deref(), session)
            .await?;
        let expires_at = OffsetDateTime::now_utc() + FOLDER_UNLOCK_TTL;
        self.db.unlock_folder(session, node, expires_at).await?;
        tracing::info!(user = %user, folder = %node, recovered = new_recovery.is_some(), "password folder's password changed");
        Ok(FolderPasswordChanged {
            recovery_key: new_recovery.map(|key| key.to_string()),
        })
    }

    /// Checks a folder's password, or recovery key, against its `hash`; `wrong-password` when
    /// it does not match.
    async fn check_folder_secret(
        &self,
        node: NodeId,
        kind: FolderSecretKind,
        hash: String,
        typed: &str,
        address: Option<IpAddr>,
    ) -> Result<(), AccountError> {
        match self
            .auth
            .check_folder_password(node, kind, hash, typed, address)
            .await
        {
            Err(AuthError::InvalidCredentials) => Err(AccountError::Auth(AuthError::WrongPassword)),
            other => Ok(other?),
        }
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
        self.require_key_lock(
            user,
            node,
            "The server drops a password folder's protection itself once it is decrypted.",
        )
        .await?;
        let states = [ProtectionState::Protecting, ProtectionState::Unprotecting];
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
        if folder.lock == FolderLock::Password {
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
        let changed = self.db.set_protection_state(user, node, &[from], to).await;
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
        Ok(folder_of(stored))
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

fn folder_of(stored: StoredProtection) -> ProtectedFolder {
    ProtectedFolder {
        node_id: stored.node_id,
        lock: stored.lock,
        wrapped_secret: stored.wrapped_secret.map(Base64Url::from),
        epoch: stored.epoch,
        recipient: stored.recipient,
        format: stored.format,
        state: stored.state,
        created_at: stored.created_at,
        recovery_key: None,
    }
}
