use std::io;

use bytes::Bytes;
use futures_util::Stream;
use time::OffsetDateTime;
use tokio::fs;
use uuid::Uuid;

use db::{
    BlobOp, Db, DeletedNodes, FOLDER_MIME, FileVersion, MoveBatch, MoveOutcome, NewContent,
    NewNode, Node, NodeId, NodeKind, NodeProperties, NodeUpdate, OnConflict, QuotaUsage,
    StoreOutcome, TrashOutcome, TrashedNode, UserId,
};

use crate::{BlobStore, FilesError, store::StagedBlob, uploads::Uploads};

fn quota_exceeded(usage: QuotaUsage) -> FilesError {
    FilesError::QuotaExceeded {
        quota: u64::try_from(usage.quota).unwrap_or(0),
        available: usage.room(),
    }
}

const FALLBACK_MIME: &str = "application/octet-stream";
/// Default size of one chunk of a resumable upload, well under the 100 MB request limit of
/// proxies like Cloudflare.
pub const DEFAULT_CHUNK_BYTES: u64 = 32 * 1024 * 1024;

/// Ownership-scoped operations on a user's folders and files.
#[derive(Clone, Debug)]
pub struct FileService {
    pub(crate) db: Db,
    pub(crate) store: BlobStore,
    pub(crate) max_upload_bytes: u64,
    pub(crate) chunk_bytes: u64,
    pub(crate) uploads: Uploads,
    pub(crate) transfer_ttl: std::time::Duration,
    pub(crate) max_transfer_bytes: Option<u64>,
}

impl FileService {
    pub fn new(db: Db, store: BlobStore, max_upload_bytes: u64) -> Self {
        Self {
            db,
            store,
            max_upload_bytes,
            chunk_bytes: DEFAULT_CHUNK_BYTES,
            uploads: Uploads::default(),
            transfer_ttl: crate::transfers::DEFAULT_TRANSFER_TTL,
            max_transfer_bytes: None,
        }
    }

    /// Sets the chunk size resumable uploads ask clients for.
    #[must_use]
    pub fn with_chunk_bytes(mut self, chunk_bytes: u64) -> Self {
        self.chunk_bytes = chunk_bytes.max(1);
        self
    }

    /// The same service, recording `device` as the maker of the changes made through it.
    #[must_use]
    pub fn on_device(&self, device: Option<Uuid>) -> Self {
        Self {
            db: self.db.on_device(device),
            ..self.clone()
        }
    }

    /// The chunk size resumable uploads ask clients for.
    pub fn chunk_bytes(&self) -> u64 {
        self.chunk_bytes
    }

    /// The largest file a single upload may hold.
    pub fn max_upload_bytes(&self) -> u64 {
        self.max_upload_bytes
    }

    /// The blob store files are kept in.
    pub fn store(&self) -> &BlobStore {
        &self.store
    }

    /// Creates a folder under `parent` (or at the root).
    pub async fn create_folder(
        &self,
        owner: UserId,
        parent: Option<NodeId>,
        name: &str,
    ) -> Result<Node, FilesError> {
        let new = NewNode {
            id: NodeId::new(),
            parent_id: parent,
            kind: NodeKind::Folder,
            name: name.to_owned(),
            size: 0,
            sha256: None,
            mime: FOLDER_MIME.to_owned(),
        };
        Ok(self.db.create_node(owner, new).await?)
    }

    /// Stores a file streamed from `body`.
    ///
    /// A new name creates the file, identical content under an existing name changes nothing,
    /// and different content becomes a new version of the existing file. Content last changed
    /// at `modified_at`, before the existing file, is kept as one of its versions instead.
    pub async fn upload<S>(
        &self,
        owner: UserId,
        parent: Option<NodeId>,
        name: &str,
        content_type: Option<&str>,
        modified_at: Option<OffsetDateTime>,
        body: S,
    ) -> Result<StoreOutcome, FilesError>
    where
        S: Stream<Item = io::Result<Bytes>>,
    {
        if let Some(parent) = parent {
            self.db.get_node(owner, parent).await?;
        }
        let staged = self.stage_within_quota(owner, body).await?;
        let stored = self
            .db
            .store_file(
                owner,
                parent,
                name,
                new_content(&staged, content_type, name, modified_at),
            )
            .await;
        self.finish_store(owner, staged, stored).await
    }

    /// Replaces the content of an existing file; different content becomes a new version, or
    /// an older one when it was last changed before the current content.
    pub async fn replace_content<S>(
        &self,
        owner: UserId,
        id: NodeId,
        content_type: Option<&str>,
        modified_at: Option<OffsetDateTime>,
        body: S,
    ) -> Result<StoreOutcome, FilesError>
    where
        S: Stream<Item = io::Result<Bytes>>,
    {
        let current = self.db.get_node(owner, id).await?;
        if current.kind != NodeKind::File {
            return Err(FilesError::NotAFile);
        }
        let staged = self.stage_within_quota(owner, body).await?;
        let stored = self
            .db
            .replace_file_content(
                owner,
                id,
                new_content(&staged, content_type, &current.name, modified_at),
            )
            .await;
        self.finish_store(owner, staged, stored).await
    }

    /// Fails with [`FilesError::QuotaExceeded`] when `owner` has a quota and `needed` more
    /// bytes do not fit in it. The unfinished upload `except` is not counted as reserved.
    pub(crate) async fn check_quota(
        &self,
        owner: UserId,
        except: Option<Uuid>,
        needed: u64,
    ) -> Result<(), FilesError> {
        match self.db.quota_usage(owner, except).await? {
            Some(usage) if needed > usage.room() => Err(quota_exceeded(usage)),
            _ => Ok(()),
        }
    }

    /// Stages `body`, stopping it as soon as it outgrows the upload limit or what is left of
    /// `owner`'s quota.
    async fn stage_within_quota<S>(&self, owner: UserId, body: S) -> Result<StagedBlob, FilesError>
    where
        S: Stream<Item = io::Result<Bytes>>,
    {
        let usage = match self.db.quota_usage(owner, None).await? {
            Some(usage) if usage.room() < self.max_upload_bytes => usage,
            _ => return self.store.stage(body, self.max_upload_bytes).await,
        };
        match self.store.stage(body, usage.room()).await {
            Err(FilesError::TooLarge { .. }) => Err(quota_exceeded(usage)),
            staged => staged,
        }
    }

    pub(crate) async fn finish_store(
        &self,
        owner: UserId,
        staged: StagedBlob,
        stored: Result<StoreOutcome, db::DbError>,
    ) -> Result<StoreOutcome, FilesError> {
        let outcome = match stored {
            Ok(outcome) => outcome,
            Err(err) => {
                self.store.discard(staged).await;
                return Err(err.into());
            }
        };
        let committed = match &outcome {
            StoreOutcome::Unchanged(_) => {
                self.store.discard(staged).await;
                return Ok(outcome);
            }
            StoreOutcome::Created(node) => {
                let committed = self.store.commit(&staged, owner, node.id).await;
                if committed.is_err() {
                    self.rollback_node(owner, node.id).await;
                }
                committed
            }
            StoreOutcome::Versioned { node, archived } => {
                match self.store.archive(owner, node.id, *archived).await {
                    Ok(()) => self.store.commit(&staged, owner, node.id).await,
                    Err(err) => Err(err),
                }
            }
            StoreOutcome::Archived { archived, .. } => {
                self.store.commit_version(&staged, owner, *archived).await
            }
        };
        if let Err(err) = committed {
            self.store.discard(staged).await;
            return Err(err.into());
        }
        Ok(outcome)
    }

    /// Opens a file for download.
    pub async fn download(
        &self,
        owner: UserId,
        id: NodeId,
    ) -> Result<(Node, fs::File), FilesError> {
        let node = self.db.get_node(owner, id).await?;
        if node.kind == NodeKind::File {
            let file = self.store.open_blob(owner, id).await?;
            Ok((node, file))
        } else {
            Err(FilesError::NotAFile)
        }
    }

    /// Returns a node's predefined properties.
    pub async fn properties(
        &self,
        owner: UserId,
        id: NodeId,
    ) -> Result<NodeProperties, FilesError> {
        Ok(self.db.node_properties(owner, id).await?)
    }

    /// Returns a node's metadata.
    pub async fn get(&self, owner: UserId, id: NodeId) -> Result<Node, FilesError> {
        Ok(self.db.get_node(owner, id).await?)
    }

    /// Lists the ancestors of a node from the root down to its parent.
    pub async fn ancestors(&self, owner: UserId, id: NodeId) -> Result<Vec<Node>, FilesError> {
        Ok(self.db.list_ancestors(owner, id).await?)
    }

    /// Lists a folder (or the root when `parent` is `None`).
    pub async fn list(
        &self,
        owner: UserId,
        parent: Option<NodeId>,
    ) -> Result<Vec<Node>, FilesError> {
        Ok(self.db.list_children(owner, parent).await?)
    }

    /// Renames and/or moves a node.
    pub async fn update(
        &self,
        owner: UserId,
        id: NodeId,
        update: NodeUpdate,
    ) -> Result<MoveOutcome, FilesError> {
        let outcome = self.db.update_node(owner, id, update).await?;
        self.apply_blob_ops(owner, &outcome.blob_ops).await;
        Ok(outcome)
    }

    /// Moves several nodes into `parent` (the root when `None`); see [`Db::move_nodes`].
    pub async fn move_nodes(
        &self,
        owner: UserId,
        ids: &[NodeId],
        parent: Option<NodeId>,
        on_conflict: OnConflict,
    ) -> Result<MoveBatch, FilesError> {
        let batch = self.db.move_nodes(owner, ids, parent, on_conflict).await?;
        self.apply_blob_ops(owner, &batch.blob_ops).await;
        Ok(batch)
    }

    /// Lists the previous versions of a file, newest first.
    pub async fn versions(
        &self,
        owner: UserId,
        id: NodeId,
    ) -> Result<Vec<FileVersion>, FilesError> {
        Ok(self.db.list_versions(owner, id).await?)
    }

    /// Opens a previous version of a file for download.
    pub async fn download_version(
        &self,
        owner: UserId,
        id: NodeId,
        version: i32,
    ) -> Result<(Node, FileVersion, fs::File), FilesError> {
        let node = self.db.get_node(owner, id).await?;
        let meta = self.db.get_version(owner, id, version).await?;
        let file = self.store.open_version(owner, meta.id).await?;
        Ok((node, meta, file))
    }

    /// Deletes a previous version and its stored content.
    pub async fn delete_version(
        &self,
        owner: UserId,
        id: NodeId,
        version: i32,
    ) -> Result<(), FilesError> {
        let version_id = self.db.delete_version(owner, id, version).await?;
        if let Err(err) = self.store.remove_version(owner, version_id).await {
            tracing::warn!(%version_id, error = %err, "failed to remove version blob");
        }
        Ok(())
    }

    /// Deletes a node by moving its files to the recycle bin; their content stays stored.
    pub async fn delete(&self, owner: UserId, id: NodeId) -> Result<TrashOutcome, FilesError> {
        Ok(self.db.trash_node(owner, id).await?)
    }

    /// Lists the files in the recycle bin, most recently deleted first.
    pub async fn trash(&self, owner: UserId) -> Result<Vec<TrashedNode>, FilesError> {
        Ok(self.db.list_trash(owner).await?)
    }

    /// Moves binned files to Lost & Found; see [`Db::restore`].
    pub async fn restore(
        &self,
        owner: UserId,
        ids: &[NodeId],
        on_conflict: OnConflict,
    ) -> Result<MoveBatch, FilesError> {
        let batch = self.db.restore(owner, ids, on_conflict).await?;
        self.apply_blob_ops(owner, &batch.blob_ops).await;
        Ok(batch)
    }

    /// Permanently deletes every binned file and its stored content.
    pub async fn empty_trash(&self, owner: UserId) -> Result<(), FilesError> {
        let deleted = self.db.empty_trash(owner).await?;
        self.remove_content(owner, deleted).await;
        Ok(())
    }

    /// Permanently deletes one binned file and its stored content.
    pub async fn purge_trashed(&self, owner: UserId, id: NodeId) -> Result<(), FilesError> {
        let deleted = self.db.purge_trashed(owner, id).await?;
        self.remove_content(owner, deleted).await;
        Ok(())
    }

    /// Deletes every user whose deletion is due, with all their files, versions, transfers
    /// and unfinished uploads. Returns the users deleted.
    ///
    /// The files go first and the user's rows last, so that a failure leaves the user in
    /// place and the next run retries. One user failing does not hold up the others.
    pub async fn delete_due_users(&self) -> Result<Vec<UserId>, FilesError> {
        let mut deleted = Vec::new();
        for user in self.db.due_deletions().await? {
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
        let Some(left) = self.db.due_user_leftovers(user).await? else {
            return Ok(false);
        };
        self.store.remove_user(user).await?;
        for transfer in left.transfers {
            self.store.remove_transfer(transfer).await?;
        }
        for upload in left.uploads {
            self.store.remove_partial(upload).await?;
        }
        Ok(self.db.delete_due_user(user).await?)
    }

    async fn apply_blob_ops(&self, owner: UserId, ops: &[BlobOp]) {
        for op in ops {
            let result = match *op {
                BlobOp::Archive { node, version_id } => {
                    self.store.archive(owner, node, version_id).await
                }
                BlobOp::Promote { from, to } => self.store.promote(owner, from, to).await,
                BlobOp::Remove { node } => self.store.remove(owner, node).await,
            };
            if let Err(err) = result {
                tracing::error!(?op, error = %err, "failed to apply blob operation");
            }
        }
    }

    #[expect(
        clippy::cognitive_complexity,
        reason = "S5 in docs/todo/server-refactoring.md"
    )]
    async fn remove_content(&self, owner: UserId, deleted: DeletedNodes) {
        for file_id in deleted.files {
            if let Err(err) = self.store.remove(owner, file_id).await {
                tracing::warn!(%file_id, error = %err, "failed to remove blob");
            }
        }
        for version_id in deleted.versions {
            if let Err(err) = self.store.remove_version(owner, version_id).await {
                tracing::warn!(%version_id, error = %err, "failed to remove version blob");
            }
        }
    }

    async fn rollback_node(&self, owner: UserId, id: NodeId) {
        if let Err(err) = self.db.delete_node(owner, id).await {
            tracing::error!(node_id = %id, error = %err, "failed to roll back node");
        }
    }
}

fn size_as_i64(staged: &StagedBlob) -> i64 {
    i64::try_from(staged.size).unwrap_or(i64::MAX)
}

pub(crate) fn new_content(
    staged: &StagedBlob,
    content_type: Option<&str>,
    name: &str,
    modified_at: Option<OffsetDateTime>,
) -> NewContent {
    NewContent {
        size: size_as_i64(staged),
        sha256: staged.sha256.clone(),
        mime: resolve_mime(content_type, name),
        modified_at,
    }
}

/// Picks the MIME type to store for a file.
///
/// The type declared by the client wins unless it is missing, invalid or the generic
/// `application/octet-stream`, in which case it is guessed from the file name.
fn resolve_mime(declared: Option<&str>, name: &str) -> String {
    declared
        .and_then(|value| value.parse::<mime_guess::mime::Mime>().ok())
        .map(|mime| mime.essence_str().to_owned())
        .filter(|essence| essence != FALLBACK_MIME)
        .or_else(|| {
            mime_guess::from_path(name)
                .first()
                .map(|mime| mime.essence_str().to_owned())
        })
        .unwrap_or_else(|| FALLBACK_MIME.to_owned())
}

#[cfg(test)]
mod tests;
