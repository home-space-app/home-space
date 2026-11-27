//! The server's own work on password folders: encrypting what they hold once protected, and
//! decrypting it once unprotected.

use db::{DbError, NodeId, UserId};
use protocol::ProtectionState;
use vault_core::FolderKeys;

use super::VaultService;
use crate::AccountError;

/// A password folder the server still has to encrypt or decrypt, with its keys.
pub struct ConvertingFolder {
    pub user: UserId,
    pub node: NodeId,
    pub state: ProtectionState,
    pub keys: FolderKeys,
}

impl VaultService {
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
}
