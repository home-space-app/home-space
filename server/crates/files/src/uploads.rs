//! Resumable chunked uploads.
//!
//! An upload is a database row plus a file under `<root>/uploads` that grows with every
//! chunk; the file's length is the upload's offset. The SHA-256 of the received bytes is kept
//! in memory as they arrive so completing a large upload does not reread it; after a restart
//! or a failed write the hash is rebuilt from the file instead.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use bytes::Bytes;
use futures_util::Stream;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use db::{NewContent, NewUpload, NodeKind, Upload, UserId};
use protocol::TransferState;

use events::DomainEvent;

use crate::{
    Actor, BodyError, FilesError, StoreOptions, StoreOutcome,
    content::{ContentStore, new_content},
    tree,
};

mod types;
pub use types::{ChunkOutcome, UploadStatus, UploadTarget};

/// The running hash of an upload's received bytes.
struct Hashing {
    hasher: Sha256,
    hashed: u64,
}

/// One upload's in-memory state. Its lock also keeps chunks of the same upload in order.
type Slot = Arc<tokio::sync::Mutex<Option<Hashing>>>;

/// In-memory state of the uploads in progress on this server.
#[derive(Clone, Default)]
pub(crate) struct Uploads {
    slots: Arc<Mutex<HashMap<Uuid, Slot>>>,
}

impl fmt::Debug for Uploads {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Uploads").finish_non_exhaustive()
    }
}

impl Uploads {
    fn slot(&self, id: Uuid) -> Slot {
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        slots.entry(id).or_default().clone()
    }

    fn insert(&self, id: Uuid, hashing: Option<Hashing>) {
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        slots.insert(id, Arc::new(tokio::sync::Mutex::new(hashing)));
    }

    pub(crate) fn remove(&self, id: Uuid) {
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        slots.remove(&id);
    }

    #[cfg(test)]
    pub(crate) fn forget_all(&self) {
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        slots.clear();
    }
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
    #[expect(
        clippy::too_many_lines,
        reason = "K9 in docs/todo/server-refactoring.md"
    )]
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
        let mut transfer_item_id = None;
        let (parent_id, node_id, name, options) = match target {
            UploadTarget::New {
                parent,
                name,
                options,
            } => {
                if let Some(parent) = parent {
                    self.content.db.get_node(owner, parent).await?;
                }
                (parent, None, name, options)
            }
            UploadTarget::Replace(id) => {
                let current = self.content.db.get_node(owner, id).await?;
                if current.kind != NodeKind::File {
                    return Err(FilesError::NotAFile);
                }
                (None, Some(id), current.name, StoreOptions::default())
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
                transfer_item_id = Some(item_id);
                let name = item
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&item.path)
                    .to_owned();
                (None, None, name, StoreOptions::default())
            }
        };
        if transfer_item_id.is_none() {
            self.content.check_quota(owner, None, size).await?;
        }
        let id = Uuid::now_v7();
        let new = NewUpload {
            id,
            parent_id,
            node_id,
            name,
            size: i64::try_from(size).unwrap_or(i64::MAX),
            mime: content_type.map(str::to_owned),
            modified_at,
            device_id: device,
            keep_both: options.keep_both,
            version_of: options.version_of,
            auto_upload: options.auto_upload,
            transfer_item_id,
        };
        match self.content.db.create_upload(owner, new).await {
            Ok(_) => {}
            // The transfer's recipient is disabled or marked for deletion.
            Err(db::DbError::NotFound) if transfer_item_id.is_some() => {
                return Err(FilesError::Gone);
            }
            Err(err) => return Err(err.into()),
        }
        if let Err(err) = self.content.store.create_partial(id).await {
            self.forget_upload(id).await;
            return Err(err.into());
        }
        self.uploads.insert(
            id,
            Some(Hashing {
                hasher: Sha256::new(),
                hashed: 0,
            }),
        );
        Ok(UploadStatus {
            id,
            offset: 0,
            size,
        })
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

    #[expect(
        clippy::cognitive_complexity,
        reason = "K9 in docs/todo/server-refactoring.md"
    )]
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
        if offset == 0 && hashing.is_none() {
            *hashing = Some(Hashing {
                hasher: Sha256::new(),
                hashed: 0,
            });
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
        if len < size {
            return Ok(ChunkOutcome::Partial { offset: len });
        }
        let sha256 = match hashing.take() {
            Some(state) => state.hasher.finalize().to_vec(),
            None => self
                .content
                .store
                .hash_partial(id)
                .await?
                .finalize()
                .to_vec(),
        };
        let stored = match upload.transfer_item_id {
            Some(item) => self
                .finish_transfer_upload(owner, &upload, item, sha256)
                .await
                .map(|ready| ChunkOutcome::Transferred { size: len, ready }),
            None => self
                .content
                .publish_stored(self.finish_upload(owner, &upload, len, sha256).await)
                .await
                .map(ChunkOutcome::Complete),
        };
        drop(hashing);
        self.forget_upload(id).await;
        stored
    }
}

fn declared_size(upload: &Upload) -> u64 {
    u64::try_from(upload.size).unwrap_or(0)
}

#[cfg(test)]
mod tests;
