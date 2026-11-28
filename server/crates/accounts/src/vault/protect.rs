//! Protecting a folder, and taking its protection away again.

use uuid::Uuid;

use db::{
    DbError, KeyLock, NewProtection, NodeId, PasswordLock, ProtectError, ProtectRefusal, StoredKdf,
    StoredProtection, UserId,
};
use protocol::{
    FolderLock, PasswordKdf, ProtectFolder, ProtectedFolder, ProtectionState, encoding::Base64Url,
    is_age_recipient,
};

use super::{VaultService, check_kdf, check_wrapped};
use crate::AccountError;

impl VaultService {
    /// The user's protected folders.
    pub async fn folders(&self, user: UserId) -> Result<Vec<ProtectedFolder>, AccountError> {
        let stored = self.db.protected_folders(user).await?;
        Ok(stored.into_iter().map(folder_of).collect())
    }

    /// Protects one of the user's folders with a secret the client made and wrapped: with the
    /// vault key (security key) or the folder's password and its recovery key (password). An
    /// empty one is protected at once. One with content stays `protecting` while the client
    /// replaces it: it downloads the files, deletes them (their history goes with them), and
    /// uploads them encrypted, then calls [`Self::finish`]. Needs a recent sign-in: with a
    /// stolen token alone, nobody may lock the user's files with their own key.
    pub async fn protect(
        &self,
        user: UserId,
        session: Uuid,
        request: &ProtectFolder,
    ) -> Result<ProtectedFolder, AccountError> {
        self.auth.require_recent_sign_in(user, session).await?;
        check_wrapped("/wrapped_secret", &request.wrapped_secret)?;
        if !is_age_recipient(&request.recipient) {
            return Err(AccountError::invalid(
                "/recipient",
                "The recipient must be an age X25519 recipient (age1…).",
            ));
        }
        match request.lock {
            FolderLock::SecurityKey => {
                let epoch = request.epoch.ok_or(AccountError::invalid(
                    "/epoch",
                    "A security-key lock needs the vault epoch.",
                ))?;
                let lock = NewProtection::SecurityKey(KeyLock {
                    wrapped_secret: request.wrapped_secret.as_bytes(),
                    epoch,
                    recipient: &request.recipient,
                });
                self.store_protection(user, request.node_id, lock).await
            }
            FolderLock::Password => {
                let kdf = request.kdf.as_ref().ok_or(AccountError::invalid(
                    "/kdf",
                    "A password lock needs the password's parameters.",
                ))?;
                let kdf = check_kdf("/kdf", kdf)?;
                let recovery =
                    request
                        .recovery_wrapped_secret
                        .as_ref()
                        .ok_or(AccountError::invalid(
                            "/recovery_wrapped_secret",
                            "A password lock needs the secret wrapped by its recovery key.",
                        ))?;
                check_wrapped("/recovery_wrapped_secret", recovery)?;
                let lock = NewProtection::Password(PasswordLock {
                    wrapped_secret: request.wrapped_secret.as_bytes(),
                    recovery_wrapped_secret: recovery.as_bytes(),
                    kdf: &kdf,
                    recipient: &request.recipient,
                });
                self.store_protection(user, request.node_id, lock).await
            }
        }
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
        self.changed(user, node, Some(stored.state)).await;
        Ok(folder_of(stored))
    }

    /// Records that the client has replaced the content of a `protecting` folder with its
    /// encrypted form.
    pub async fn finish(
        &self,
        user: UserId,
        node: NodeId,
    ) -> Result<ProtectedFolder, AccountError> {
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
        self.changed(user, node, None).await;
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
        self.changed(user, node, Some(stored.state)).await;
        Ok(folder_of(stored))
    }
}

pub(super) fn folder_of(stored: StoredProtection) -> ProtectedFolder {
    let kdf = stored.kdf().map(kdf_of);
    ProtectedFolder {
        node_id: stored.node_id,
        lock: stored.lock,
        wrapped_secret: Base64Url::from(stored.wrapped_secret),
        epoch: stored.epoch,
        kdf,
        recovery_wrapped_secret: stored.recovery_wrapped_secret.map(Base64Url::from),
        recipient: stored.recipient,
        format: stored.format,
        state: stored.state,
        created_at: stored.created_at,
    }
}

/// The parameters as clients read them; the database keeps them as checked on the way in, so
/// they are never negative.
fn kdf_of(stored: StoredKdf) -> PasswordKdf {
    let count = |value: i32| u32::try_from(value).unwrap_or_default();
    PasswordKdf {
        salt: Base64Url::from(stored.salt),
        memory_kib: count(stored.memory_kib),
        iterations: count(stored.iterations),
        parallelism: count(stored.parallelism),
    }
}
