//! All file services, built over one store.

use std::time::Duration;

use db::{Db, UserId};
use events::Events;

use crate::{
    BlobStore, DuplicateService, FilesError, NodeService, TransferService, TrashService,
    UploadService, VersionService, content::ContentStore,
};

/// Default size of one chunk of a resumable upload, well under the 100 MB request limit of
/// proxies like Cloudflare.
pub const DEFAULT_CHUNK_BYTES: u64 = 32 * 1024 * 1024;

/// How the file services are set up.
#[derive(Clone, Copy, Debug)]
pub struct FilesConfig {
    /// The largest file a single upload may hold.
    pub max_upload_bytes: u64,
    /// The chunk size resumable uploads ask clients for.
    pub chunk_bytes: u64,
    /// How long transfers are kept.
    pub transfer_ttl: Duration,
    /// The most bytes one transfer may hold; `None` for no limit.
    pub max_transfer_bytes: Option<u64>,
}

impl FilesConfig {
    /// Files of up to `max_upload_bytes`, and the defaults for everything else.
    pub fn new(max_upload_bytes: u64) -> Self {
        Self {
            max_upload_bytes,
            chunk_bytes: DEFAULT_CHUNK_BYTES,
            transfer_ttl: crate::DEFAULT_TRANSFER_TTL,
            max_transfer_bytes: None,
        }
    }
}

/// Every file service, sharing one database, blob store and event sink. Cheap to clone.
#[derive(Clone, Debug)]
pub struct FileServices {
    pub nodes: NodeService,
    pub versions: VersionService,
    pub trash: TrashService,
    pub uploads: UploadService,
    pub transfers: TransferService,
    pub duplicates: DuplicateService,
    content: ContentStore,
}

impl FileServices {
    pub fn new(db: Db, store: BlobStore, config: FilesConfig, events: Events) -> Self {
        let content = ContentStore::new(db, store, config.max_upload_bytes, events);
        let uploads = UploadService::new(content.clone(), config.chunk_bytes);
        Self {
            nodes: NodeService::new(content.clone()),
            versions: VersionService::new(content.clone()),
            trash: TrashService::new(content.clone()),
            transfers: TransferService::new(
                content.clone(),
                uploads.clone(),
                config.transfer_ttl,
                config.max_transfer_bytes,
            ),
            uploads,
            duplicates: DuplicateService::new(content.clone()),
            content,
        }
    }

    /// The blob store files are kept in.
    pub fn store(&self) -> &BlobStore {
        &self.content.store
    }

    /// Whether the database answers.
    pub async fn ping(&self) -> Result<(), FilesError> {
        Ok(self.content.db.ping().await?)
    }

    /// Deletes every user whose deletion is due, with all their files, versions, transfers
    /// and unfinished uploads. Returns the users deleted.
    ///
    /// The files go first and the user's rows last, so that a failure leaves the user in
    /// place and the next run retries. One user failing does not hold up the others.
    pub async fn delete_due_users(&self) -> Result<Vec<UserId>, FilesError> {
        let mut deleted = Vec::new();
        for user in self.content.db.due_deletions().await? {
            match self.delete_user(user).await {
                Ok(true) => deleted.push(user),
                // The deletion was cancelled since it was listed.
                Ok(false) => {}
                Err(err) => {
                    tracing::error!(%user, error = %err, "deleting a user failed; will retry");
                }
            }
        }
        Ok(deleted)
    }

    async fn delete_user(&self, user: UserId) -> Result<bool, FilesError> {
        let Some(left) = self.content.db.due_user_leftovers(user).await? else {
            return Ok(false);
        };
        self.content.store.remove_user(user).await?;
        for transfer in left.transfers {
            self.content.store.remove_transfer(transfer).await?;
        }
        for upload in left.uploads {
            self.content.store.remove_partial(upload).await?;
        }
        Ok(self.content.db.delete_due_user(user).await?)
    }
}

#[cfg(test)]
mod tests;
