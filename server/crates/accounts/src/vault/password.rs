//! Password folders: unlocking them for a session, their keys, and changing their password.

use std::net::IpAddr;

use time::OffsetDateTime;
use uuid::Uuid;

use auth::{AuthError, FolderSecretKind};
use db::{NodeId, PasswordSecrets, UserId};
use protocol::{ChangeFolderPassword, FolderPasswordChanged, FolderUnlocked};
use vault_core::{FolderKeys, FolderSecret, RecoveryKey};

use super::{FOLDER_UNLOCK_TTL, VaultService, check_password};
use crate::AccountError;

impl VaultService {
    /// Unlocks the password folder `node` for `session` with its `password`.
    pub async fn unlock(
        &self,
        user: UserId,
        session: Uuid,
        node: NodeId,
        password: &str,
        address: Option<IpAddr>,
    ) -> Result<FolderUnlocked, AccountError> {
        let secrets = self.password_secrets(user, node).await?;
        let kind = FolderSecretKind::Password;
        self.check_folder_secret(node, kind, secrets.password_hash, password, address)
            .await?;
        let expires_at = self.open_for(session, node).await?;
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
        self.password_secrets(user, node).await?;
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
        self.require_unlocked(session, folder.node_id).await?;
        self.folder_keys(user, folder.node_id).await.map(Some)
    }

    /// The keys of the password folder `node`, for the server's own work on it.
    pub async fn folder_keys(
        &self,
        user: UserId,
        node: NodeId,
    ) -> Result<FolderKeys, AccountError> {
        let secrets = self.password_secrets(user, node).await?;
        let raw = self.auth.open_folder_key(user, &secrets.sealed_key)?;
        let bytes: [u8; 32] = raw
            .as_slice()
            .try_into()
            .map_err(|_| AccountError::Auth(AuthError::Unsealing))?;
        Ok(FolderSecret::from_raw(bytes).keys()?)
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
        let secrets = self.password_secrets(user, node).await?;
        check_password(&request.password)?;
        let new_recovery = match (&request.current_password, &request.recovery_key) {
            (Some(current), None) => {
                let kind = FolderSecretKind::Password;
                self.check_folder_secret(node, kind, secrets.password_hash, current, address)
                    .await?;
                None
            }
            (None, Some(typed)) => {
                let key = RecoveryKey::parse(typed).map_err(|_| {
                    AccountError::invalid(
                        "/recovery_key",
                        "That is not a recovery key: 55 letters and digits in 11 groups.",
                    )
                })?;
                let (kind, typed) = (FolderSecretKind::RecoveryKey, key.to_display());
                self.check_folder_secret(node, kind, secrets.recovery_hash, &typed, address)
                    .await?;
                Some(RecoveryKey::generate().to_display())
            }
            _ => {
                return Err(AccountError::invalid(
                    "/current_password",
                    "Give the folder's current password, or its recovery key.",
                ));
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
        self.open_for(session, node).await?;
        tracing::info!(user = %user, folder = %node, recovered = new_recovery.is_some(), "password folder's password changed");
        Ok(FolderPasswordChanged {
            recovery_key: new_recovery.map(|key| key.to_string()),
        })
    }

    /// The secrets of the user's password folder `node`; `NotFound` when it is not one.
    async fn password_secrets(
        &self,
        user: UserId,
        node: NodeId,
    ) -> Result<PasswordSecrets, AccountError> {
        self.db
            .password_secrets(user, node)
            .await?
            .ok_or(AccountError::NotFound)
    }

    /// Opens the password folder `node` to `session` for [`FOLDER_UNLOCK_TTL`], or keeps it open
    /// that long; returns until when.
    pub(super) async fn open_for(
        &self,
        session: Uuid,
        node: NodeId,
    ) -> Result<OffsetDateTime, AccountError> {
        let expires_at = OffsetDateTime::now_utc() + FOLDER_UNLOCK_TTL;
        self.db.unlock_folder(session, node, expires_at).await?;
        Ok(expires_at)
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

    /// `Locked` unless `session` has the password folder `node` unlocked; using it keeps it so.
    pub(super) async fn require_unlocked(
        &self,
        session: Uuid,
        node: NodeId,
    ) -> Result<(), AccountError> {
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
}
