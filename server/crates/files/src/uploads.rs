//! Resumable chunked uploads.
//!
//! An upload is a database row plus a file under `<root>/uploads` that grows with every
//! chunk; the file's length is the upload's offset. The SHA-256 of the received bytes is kept
//! in memory as they arrive so completing a large upload does not reread it; after a restart
//! or a failed write the hash is rebuilt from the file instead.

use std::{collections::HashSet, time::Duration};

use bytes::Bytes;
use futures_util::Stream;
use sha2::Digest;
use time::OffsetDateTime;
use uuid::Uuid;

use db::{NewContent, NewUpload, NodeId, NodeKind, Upload, UserId};
use protocol::TransferState;

use events::DomainEvent;

use crate::{
    Actor, BodyError, FilesError, StoreOptions, StoreOutcome,
    content::{ContentStore, new_content},
    tree,
};

mod slots;
mod types;

use slots::{Hashing, Uploads};
pub use types::{ChunkOutcome, UploadStatus, UploadTarget};

/// Where an upload's bytes go once they have all arrived.
#[derive(Default)]
struct Placement {
    parent_id: Option<NodeId>,
    node_id: Option<NodeId>,
    name: String,
    options: StoreOptions,
    transfer_item_id: Option<Uuid>,
}

/// Resumable uploads, of files of the user's tree and of transfers.
#[derive(Clone, Debug)]
pub struct UploadService {
    pub(crate) content: ContentStore,
    uploads: Uploads,
    /// The chunk size resumable uploads ask clients for.
    chunk_bytes: u64,
}

impl UploadService {
    pub(crate) fn new(content: ContentStore, chunk_bytes: u64) -> Self {
        Self {
            content,
            uploads: Uploads::default(),
            chunk_bytes: chunk_bytes.max(1),
        }
    }

    /// The same service, recording `device` as the maker of the changes made through it.
    pub(crate) fn acting(&self, device: Option<Uuid>) -> Self {
        Self {
            content: self.content.acting(device),
            uploads: self.uploads.clone(),
            chunk_bytes: self.chunk_bytes,
        }
    }

    /// The chunk size resumable uploads ask clients for.
    pub fn chunk_bytes(&self) -> u64 {
        self.chunk_bytes
    }

    /// Forgets the in-memory state of an upload that is gone.
    pub(crate) fn forget_slot(&self, id: Uuid) {
        self.uploads.remove(id);
    }

    /// Starts a resumable upload of `size` bytes from `device`, when the client named one.
    pub async fn create_upload(
        &self,
        owner: UserId,
        device: Option<Uuid>,
        target: UploadTarget,
        size: u64,
        content_type: Option<&str>,
        modified_at: Option<OffsetDateTime>,
    ) -> Result<UploadStatus, FilesError> {
        if size > self.content.max_upload_bytes {
            return Err(FilesError::TooLarge {
                limit: self.content.max_upload_bytes,
            });
        }
        let place = self.resolve_target(owner, target, size).await?;
        if place.transfer_item_id.is_none() {
            self.content.check_quota(owner, None, size).await?;
        }
        let id = Uuid::now_v7();
        let new = NewUpload {
            id,
            parent_id: place.parent_id,
            node_id: place.node_id,
            name: place.name,
            size: i64::try_from(size).unwrap_or(i64::MAX),
            mime: content_type.map(str::to_owned),
            modified_at,
            device_id: device,
            keep_both: place.options.keep_both,
            version_of: place.options.version_of,
            auto_upload: place.options.auto_upload,
            transfer_item_id: place.transfer_item_id,
        };
        self.open_upload(owner, new).await?;
        Ok(UploadStatus {
            id,
            offset: 0,
            size,
        })
    }

    /// Checks that `target` can take `size` bytes and works out where they will go.
    async fn resolve_target(
        &self,
        owner: UserId,
        target: UploadTarget,
        size: u64,
    ) -> Result<Placement, FilesError> {
        match target {
            UploadTarget::New {
                parent,
                name,
                options,
            } => {
                if let Some(parent) = parent {
                    self.content.db.get_node(owner, parent).await?;
                }
                Ok(Placement {
                    parent_id: parent,
                    name,
                    options,
                    ..Placement::default()
                })
            }
            UploadTarget::Replace(id) => {
                let current = self.content.db.get_node(owner, id).await?;
                if current.kind != NodeKind::File {
                    return Err(FilesError::NotAFile);
                }
                Ok(Placement {
                    node_id: Some(id),
                    name: current.name,
                    ..Placement::default()
                })
            }
            UploadTarget::TransferItem(item_id) => {
                let (transfer, item) = self.content.db.sent_transfer_item(owner, item_id).await?;
                if transfer.state != TransferState::Uploading {
                    return Err(FilesError::Gone);
                }
                if item.kind != NodeKind::File || item.sha256.is_some() {
                    return Err(FilesError::Conflict);
                }
                if u64::try_from(item.size).ok() != Some(size) {
                    return Err(FilesError::InvalidTransfer(format!(
                        "the item has {} bytes, not {size}",
                        item.size
                    )));
                }
                let name = item.path.rsplit('/').next().unwrap_or(&item.path);
                Ok(Placement {
                    name: name.to_owned(),
                    transfer_item_id: Some(item_id),
                    ..Placement::default()
                })
            }
        }
    }

    /// Records the upload `new` and creates its empty file, ready for the first chunk.
    async fn open_upload(&self, owner: UserId, new: NewUpload) -> Result<(), FilesError> {
        let id = new.id;
        let for_transfer = new.transfer_item_id.is_some();
        match self.content.db.create_upload(owner, new).await {
            Ok(_) => {}
            // The transfer's recipient is disabled or marked for deletion.
            Err(db::DbError::NotFound) if for_transfer => return Err(FilesError::Gone),
            Err(err) => return Err(err.into()),
        }
        if let Err(err) = self.content.store.create_partial(id).await {
            self.forget_upload(id).await;
            return Err(err.into());
        }
        self.uploads.insert(id, Some(Hashing::new()));
        Ok(())
    }

    /// Reports how many bytes of an upload have arrived.
    pub async fn upload_status(&self, owner: UserId, id: Uuid) -> Result<UploadStatus, FilesError> {
        let upload = self.content.db.get_upload(owner, id).await?;
        let offset = self.content.store.partial_len(id).await?;
        Ok(UploadStatus {
            id,
            offset,
            size: declared_size(&upload),
        })
    }

    /// Appends a chunk that starts at `offset`; the chunk that completes the upload stores the
    /// file and ends the upload, whether storing succeeds or not.
    pub async fn append_chunk<S>(
        &self,
        actor: Actor,
        id: Uuid,
        offset: u64,
        body: S,
    ) -> Result<ChunkOutcome, FilesError>
    where
        S: Stream<Item = Result<Bytes, BodyError>>,
    {
        self.acting(actor.device)
            .append_chunk_as(actor.user, id, offset, body)
            .await
    }

    /// Abandons an upload and the bytes it has received.
    pub async fn cancel_upload(&self, owner: UserId, id: Uuid) -> Result<(), FilesError> {
        self.content.db.get_upload(owner, id).await?;
        let slot = self.uploads.slot(id);
        let _guard = slot.lock().await;
        self.forget_upload(id).await;
        Ok(())
    }

    /// The unfinished uploads of `owner`, only those of `device` when given.
    pub async fn uploads(
        &self,
        owner: UserId,
        device: Option<Uuid>,
    ) -> Result<Vec<Upload>, FilesError> {
        Ok(self.content.db.uploads(owner, device).await?)
    }

    /// Cancels every unfinished upload `device` started; returns how many there were.
    pub async fn cancel_device_uploads(
        &self,
        owner: UserId,
        device: Uuid,
    ) -> Result<usize, FilesError> {
        let uploads = self.content.db.uploads(owner, Some(device)).await?;
        for upload in &uploads {
            match self.cancel_upload(owner, upload.id).await {
                Ok(()) | Err(FilesError::NotFound) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(uploads.len())
    }

    /// Ends the uploads untouched for `older_than` and removes upload leftovers on disk.
    ///
    /// Returns how many uploads and files were removed.
    pub async fn expire_uploads(&self, older_than: Duration) -> Result<usize, FilesError> {
        let before = OffsetDateTime::now_utc() - older_than;
        let expired = self.content.db.expire_uploads(before).await?;
        for &id in &expired {
            self.uploads.remove(id);
            if let Err(err) = self.content.store.remove_partial(id).await {
                tracing::warn!(upload_id = %id, error = %err, "failed to remove expired upload");
            }
        }
        let live = self
            .content
            .db
            .upload_ids()
            .await?
            .into_iter()
            .collect::<HashSet<_>>();
        let swept = self.content.store.sweep(&live, older_than).await?;
        Ok(expired.len() + swept)
    }

    async fn finish_upload(
        &self,
        owner: UserId,
        upload: &Upload,
        size: u64,
        sha256: Vec<u8>,
    ) -> Result<StoreOutcome, FilesError> {
        // Checked again because what the owner stores may have grown, or their quota shrunk,
        // since the upload started.
        self.content
            .check_quota(owner, Some(upload.id), size)
            .await?;
        let staged = self.content.store.adopt_partial(upload.id, size, sha256);
        let content = |name: &str| -> NewContent {
            new_content(&staged, upload.mime.as_deref(), name, upload.modified_at)
        };
        let stored = match upload.node_id {
            Some(node) => {
                tree::replace_content(&self.content.db, owner, node, content(&upload.name)).await
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
                    &upload.name,
                    content(&upload.name),
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

    /// Ends an upload once any chunk being written to it is done, whoever owns it.
    pub(crate) async fn abandon_upload(&self, id: Uuid) {
        let slot = self.uploads.slot(id);
        let _guard = slot.lock().await;
        self.forget_upload(id).await;
    }

    async fn forget_upload(&self, id: Uuid) {
        self.uploads.remove(id);
        if let Err(err) = self.content.db.delete_upload(id).await {
            tracing::warn!(upload_id = %id, error = %err, "failed to delete upload record");
        }
        if let Err(err) = self.content.store.remove_partial(id).await {
            tracing::warn!(upload_id = %id, error = %err, "failed to remove upload file");
        }
    }

    async fn append_chunk_as<S>(
        &self,
        owner: UserId,
        id: Uuid,
        offset: u64,
        body: S,
    ) -> Result<ChunkOutcome, FilesError>
    where
        S: Stream<Item = Result<Bytes, BodyError>>,
    {
        let upload = self.content.db.get_upload(owner, id).await?;
        let size = declared_size(&upload);
        let slot = self.uploads.slot(id);
        let mut hashing = slot.lock().await;
        let len = self
            .write_chunk(&mut hashing, id, offset, size, body)
            .await?;
        if len < size {
            return Ok(ChunkOutcome::Partial { offset: len });
        }
        let sha256 = self.received_sha256(hashing.take(), id).await?;
        let stored = self.complete_upload(owner, &upload, len, sha256).await;
        drop(hashing);
        self.forget_upload(id).await;
        stored
    }

    /// Appends `body` at `offset`, hashing it on the way when the hash so far is known.
    /// Returns how many bytes the upload holds now; a failed write leaves the hash unknown.
    async fn write_chunk<S>(
        &self,
        hashing: &mut Option<Hashing>,
        id: Uuid,
        offset: u64,
        size: u64,
        body: S,
    ) -> Result<u64, FilesError>
    where
        S: Stream<Item = Result<Bytes, BodyError>>,
    {
        if offset == 0 && hashing.is_none() {
            *hashing = Some(Hashing::new());
        }
        let appended = self
            .content
            .store
            .append(id, offset, size, body, |piece| {
                if let Some(state) = hashing.as_mut() {
                    state.hasher.update(piece);
                    state.hashed += piece.len() as u64;
                }
            })
            .await;
        let len = match &appended {
            Ok(len) => *len,
            Err(_) => self.content.store.partial_len(id).await?,
        };
        if hashing.as_ref().is_some_and(|state| state.hashed != len) {
            *hashing = None;
        }
        if let Err(err) = self.content.db.touch_upload(id).await {
            tracing::warn!(upload_id = %id, error = %err, "failed to touch upload");
        }
        appended?;
        Ok(len)
    }

    /// The SHA-256 of all the bytes of upload `id`, read back from its file when the running
    /// hash was lost.
    async fn received_sha256(
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

    /// Stores the complete upload in the tree or in its transfer.
    async fn complete_upload(
        &self,
        owner: UserId,
        upload: &Upload,
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
                .publish_stored(self.finish_upload(owner, upload, len, sha256).await)
                .await
                .map(ChunkOutcome::Complete),
        }
    }
}

fn declared_size(upload: &Upload) -> u64 {
    u64::try_from(upload.size).unwrap_or(0)
}

#[cfg(test)]
mod tests;
