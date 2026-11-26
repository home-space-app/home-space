//! Encrypting what a password folder already held, in place: the current content of every file
//! and every older version, so the version history is kept. Safe to run again after a stop: a
//! blob the folder's identity already opens is left as it is.

use db::{NodeId, UserId};
use vault_core::FolderKeys;

use crate::{FilesError, NodeService};

impl NodeService {
    /// Encrypts every blob of the files in `folder` that is not yet encrypted to it; returns how
    /// many it encrypted. Stores into the folder wait while a blob is being encrypted, and the
    /// other way round.
    pub async fn encrypt_folder(
        &self,
        owner: UserId,
        folder: NodeId,
        keys: &FolderKeys,
    ) -> Result<usize, FilesError> {
        let recipient = keys.recipient();
        let identity = keys.identity();
        let store = &self.content.store;
        let mut sealed = 0;
        for file in self.content.db.folder_files(owner, folder).await? {
            {
                let _writing = self.content.sealing.lock().await;
                if store
                    .seal_blob(owner, file.node_id, &recipient, identity)
                    .await?
                {
                    sealed += 1;
                }
            }
            for version in file.versions {
                let _writing = self.content.sealing.lock().await;
                if store
                    .seal_version(owner, version, &recipient, identity)
                    .await?
                {
                    sealed += 1;
                }
            }
        }
        if sealed > 0 {
            tracing::info!(user = %owner, folder = %folder, blobs = sealed, "password folder content encrypted");
        }
        Ok(sealed)
    }
}
