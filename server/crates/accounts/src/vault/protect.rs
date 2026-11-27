//! Protecting a folder, and taking its protection away again.

use uuid::Uuid;

use db::{
    DbError, KeyLock, NewProtection, NodeId, PasswordLock, ProtectError, ProtectRefusal,
    StoredProtection, UserId,
};
use protocol::{
    FolderLock, ProtectFolder, ProtectedFolder, ProtectionState, encoding::Base64Url,
    is_age_recipient,
};
use vault_core::{FolderKeys, FolderSecret, RecoveryKey};

use super::{VaultService, check_password, check_wrapped};
use crate::AccountError;

impl VaultService {
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
            self.open_for(session, request.node_id).await?;
            return Ok(folder);
        }
        let wrapped = request
            .wrapped_secret
            .as_ref()
            .ok_or(AccountError::invalid(
                "/wrapped_secret",
                "A security-key lock needs the wrapped folder secret.",
            ))?;
        check_wrapped("/wrapped_secret", wrapped)?;
        let epoch = request.epoch.ok_or(AccountError::invalid(
            "/epoch",
            "A security-key lock needs the vault epoch.",
        ))?;
        let recipient = request
            .recipient
            .as_deref()
            .filter(|recipient| is_age_recipient(recipient))
            .ok_or(AccountError::invalid(
                "/recipient",
                "The recipient must be an age X25519 recipient (age1…).",
            ))?;
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
            return Err(AccountError::invalid(
                "/lock",
                "This server has no secrets key, so it cannot keep the key of a \
                 password folder.",
            ));
        }
        let password = request.password.as_deref().ok_or(AccountError::invalid(
            "/password",
            "A password lock needs the password.",
        ))?;
        check_password(password)?;
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
                return Err(AccountError::invalid(
                    "/node_id",
                    "A name inside the folder is longer than 159 bytes, the most a \
                     password folder holds. Rename it first.",
                ));
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

    pub(super) async fn change_state(
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
