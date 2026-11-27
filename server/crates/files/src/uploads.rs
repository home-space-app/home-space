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

use db::{Upload, UserId};

use crate::{Actor, BodyError, FilesError, content::ContentStore};

mod create;
mod finish;
mod slots;
mod types;

use slots::{Hashing, Uploads};
pub use types::{ChunkOutcome, UploadRequest, UploadStatus, UploadTarget};

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
        self.abandon_upload(id).await;
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
}

fn declared_size(upload: &Upload) -> u64 {
    u64::try_from(upload.size).unwrap_or(0)
}

#[cfg(test)]
mod tests;
