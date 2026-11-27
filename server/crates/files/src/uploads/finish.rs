//! Completing an upload: storing its bytes in the tree, or in the transfer it belongs to.

use sha2::Digest;
use uuid::Uuid;

use db::{NewContent, NodeKind, Upload, UserId};
use events::DomainEvent;
use protocol::TransferState;

use super::{ChunkOutcome, UploadService, slots::Hashing};
use crate::{FilesError, Names, StoreOptions, StoreOutcome, content::new_content, tree};

impl UploadService {
    /// Stores the complete upload in the tree or in its transfer.
    pub(super) async fn complete_upload(
        &self,
        owner: UserId,
        upload: &Upload,
        names: &Names,
        len: u64,
        sha256: Vec<u8>,
    ) -> Result<ChunkOutcome, FilesError> {
        match upload.transfer_item_id {
            Some(item) => self
                .finish_transfer_upload(owner, upload, item, sha256)
                .await
                .map(|ready| ChunkOutcome::Transferred { size: len, ready }),
            None => self
                .content
                .publish_stored(self.finish_upload(owner, upload, names, len, sha256).await)
                .await
                .map(ChunkOutcome::Complete),
        }
    }

    /// The SHA-256 of all the bytes of upload `id`, read back from its file when the running
    /// hash was lost.
    pub(super) async fn received_sha256(
        &self,
        hashing: Option<Hashing>,
        id: Uuid,
    ) -> Result<Vec<u8>, FilesError> {
        let hasher = match hashing {
            Some(state) => state.hasher,
            None => self.content.store.hash_partial(id).await?,
        };
        Ok(hasher.finalize().to_vec())
    }

    async fn finish_upload(
        &self,
        owner: UserId,
        upload: &Upload,
        names: &Names,
        size: u64,
        sha256: Vec<u8>,
    ) -> Result<StoreOutcome, FilesError> {
        // Checked again because what the owner stores may have grown, or their quota shrunk,
        // since the upload started.
        self.content
            .check_quota(owner, Some(upload.id), size)
            .await?;
        let staged = self.content.store.adopt_partial(upload.id, size, sha256);
        let name = names.show(NodeKind::File, &upload.name);
        let content = |name: &str| -> NewContent {
            new_content(&staged, upload.mime.as_deref(), name, upload.modified_at)
        };
        let stored = match upload.node_id {
            Some(node) => {
                tree::replace_content(&self.content.db, owner, node, content(&name)).await
            }
            None => {
                if let Some(parent) = upload.parent_id
                    && let Err(err) = self.content.db.get_node(owner, parent).await
                {
                    self.content.store.discard(staged).await;
                    return Err(err.into());
                }
                let options = StoreOptions {
                    keep_both: upload.keep_both,
                    version_of: upload.version_of,
                    auto_upload: upload.auto_upload,
                };
                tree::store_file(
                    &self.content.db,
                    owner,
                    upload.parent_id,
                    &name,
                    names,
                    content(&name),
                    options,
                )
                .await
            }
        };
        self.content.finish_store(owner, staged, stored).await
    }

    async fn finish_transfer_upload(
        &self,
        owner: UserId,
        upload: &Upload,
        item: Uuid,
        sha256: Vec<u8>,
    ) -> Result<Option<db::Transfer>, FilesError> {
        let (transfer, _) = self.content.db.sent_transfer_item(owner, item).await?;
        if transfer.state != TransferState::Uploading {
            return Err(FilesError::Gone);
        }
        self.content
            .store
            .commit_into_transfer(upload.id, transfer.id, item)
            .await?;
        match self.content.db.store_transfer_item(item, &sha256).await {
            Ok(ready) => {
                if let Some(transfer) = &ready {
                    self.content
                        .events
                        .publish_one(DomainEvent::TransferReady(transfer.clone()))
                        .await;
                }
                Ok(ready)
            }
            Err(db::DbError::NotFound) => {
                // Cancelled or expired meanwhile.
                self.content.remove_transfer_files(transfer.id).await;
                Err(FilesError::Gone)
            }
            Err(err) => Err(err.into()),
        }
    }
}
