//! Resumable chunked uploads.
//!
//! An upload is a database row plus a file under `<root>/uploads` that grows with every
//! chunk; the file's length is the upload's offset. The SHA-256 of the received bytes is kept
//! in memory as they arrive so completing a large upload does not reread it; after a restart
//! or a failed write the hash is rebuilt from the file instead.

use std::{
    collections::{HashMap, HashSet},
    fmt, io,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use bytes::Bytes;
use futures_util::Stream;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use db::{NewContent, NewUpload, NodeId, NodeKind, StoreOptions, StoreOutcome, Upload, UserId};
use protocol::TransferState;

use crate::{FileService, FilesError, service::new_content};

/// What a resumable upload stores once complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadTarget {
    /// A file named `name` in `parent` (the root when `None`), stored like
    /// [`FileService::upload`].
    New {
        parent: Option<NodeId>,
        name: String,
        options: StoreOptions,
    },
    /// New content for an existing file, stored like [`FileService::replace_content`].
    Replace(NodeId),
    /// The bytes of a file of a transfer the caller is sending.
    TransferItem(Uuid),
}

/// Where an upload stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadStatus {
    pub id: Uuid,
    pub offset: u64,
    pub size: u64,
}

/// Result of appending a chunk.
#[derive(Debug)]
pub enum ChunkOutcome {
    /// More bytes are expected, starting at `offset`.
    Partial { offset: u64 },
    /// The last chunk arrived and the file was stored.
    Complete(StoreOutcome),
    /// The last chunk of a transfer's file arrived (`size` bytes in all); `ready` is the
    /// transfer when it was its last missing file.
    Transferred {
        size: u64,
        ready: Option<db::Transfer>,
    },
}

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

impl FileService {
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
        if size > self.max_upload_bytes {
            return Err(FilesError::TooLarge {
                limit: self.max_upload_bytes,
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
                    self.db.get_node(owner, parent).await?;
                }
                (parent, None, name, options)
            }
            UploadTarget::Replace(id) => {
                let current = self.db.get_node(owner, id).await?;
                if current.kind != NodeKind::File {
                    return Err(FilesError::NotAFile);
                }
                (None, Some(id), current.name, StoreOptions::default())
            }
            UploadTarget::TransferItem(item_id) => {
                let (transfer, item) = self.db.sent_transfer_item(owner, item_id).await?;
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
        match self.db.create_upload(owner, new).await {
            Ok(_) => {}
            // The transfer's recipient is disabled or marked for deletion.
            Err(db::DbError::NotFound) if transfer_item_id.is_some() => {
                return Err(FilesError::Gone);
            }
            Err(err) => return Err(err.into()),
        }
        if let Err(err) = self.store.create_partial(id).await {
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
        let upload = self.db.get_upload(owner, id).await?;
        let offset = self.store.partial_len(id).await?;
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
        owner: UserId,
        id: Uuid,
        offset: u64,
        body: S,
    ) -> Result<ChunkOutcome, FilesError>
    where
        S: Stream<Item = io::Result<Bytes>>,
    {
        let upload = self.db.get_upload(owner, id).await?;
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
            Err(_) => self.store.partial_len(id).await?,
        };
        if hashing.as_ref().is_some_and(|state| state.hashed != len) {
            *hashing = None;
        }
        if let Err(err) = self.db.touch_upload(id).await {
            tracing::warn!(upload_id = %id, error = %err, "failed to touch upload");
        }
        appended?;
        if len < size {
            return Ok(ChunkOutcome::Partial { offset: len });
        }
        let sha256 = match hashing.take() {
            Some(state) => state.hasher.finalize().to_vec(),
            None => self.store.hash_partial(id).await?.finalize().to_vec(),
        };
        let stored = match upload.transfer_item_id {
            Some(item) => self
                .finish_transfer_upload(owner, &upload, item, sha256)
                .await
                .map(|ready| ChunkOutcome::Transferred { size: len, ready }),
            None => self
                .finish_upload(owner, &upload, len, sha256)
                .await
                .map(ChunkOutcome::Complete),
        };
        drop(hashing);
        self.forget_upload(id).await;
        stored
    }

    /// Abandons an upload and the bytes it has received.
    pub async fn cancel_upload(&self, owner: UserId, id: Uuid) -> Result<(), FilesError> {
        self.db.get_upload(owner, id).await?;
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
        Ok(self.db.uploads(owner, device).await?)
    }

    /// Cancels every unfinished upload `device` started; returns how many there were.
    pub async fn cancel_device_uploads(
        &self,
        owner: UserId,
        device: Uuid,
    ) -> Result<usize, FilesError> {
        let uploads = self.db.uploads(owner, Some(device)).await?;
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
        let expired = self.db.expire_uploads(before).await?;
        for &id in &expired {
            self.uploads.remove(id);
            if let Err(err) = self.store.remove_partial(id).await {
                tracing::warn!(upload_id = %id, error = %err, "failed to remove expired upload");
            }
        }
        let live = self
            .db
            .upload_ids()
            .await?
            .into_iter()
            .collect::<HashSet<_>>();
        let swept = self.store.sweep(&live, older_than).await?;
        Ok(expired.len() + swept)
    }

    async fn finish_upload(
        &self,
        owner: UserId,
        upload: &Upload,
        size: u64,
        sha256: Vec<u8>,
    ) -> Result<StoreOutcome, FilesError> {
        let staged = self.store.adopt_partial(upload.id, size, sha256);
        let content = |name: &str| -> NewContent {
            new_content(&staged, upload.mime.as_deref(), name, upload.modified_at)
        };
        let stored = match upload.node_id {
            Some(node) => {
                self.db
                    .replace_file_content(owner, node, content(&upload.name))
                    .await
            }
            None => {
                if let Some(parent) = upload.parent_id
                    && let Err(err) = self.db.get_node(owner, parent).await
                {
                    self.store.discard(staged).await;
                    return Err(err.into());
                }
                let options = StoreOptions {
                    keep_both: upload.keep_both,
                    version_of: upload.version_of,
                    auto_upload: upload.auto_upload,
                };
                self.db
                    .store_file_with(
                        owner,
                        upload.parent_id,
                        &upload.name,
                        content(&upload.name),
                        options,
                    )
                    .await
            }
        };
        self.finish_store(owner, staged, stored).await
    }

    async fn finish_transfer_upload(
        &self,
        owner: UserId,
        upload: &Upload,
        item: Uuid,
        sha256: Vec<u8>,
    ) -> Result<Option<db::Transfer>, FilesError> {
        let (transfer, _) = self.db.sent_transfer_item(owner, item).await?;
        if transfer.state != TransferState::Uploading {
            return Err(FilesError::Gone);
        }
        self.store
            .commit_into_transfer(upload.id, transfer.id, item)
            .await?;
        match self.db.store_transfer_item(item, &sha256).await {
            Ok(ready) => Ok(ready),
            Err(db::DbError::NotFound) => {
                // Cancelled or expired meanwhile.
                self.remove_transfer_files(transfer.id).await;
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
        if let Err(err) = self.db.delete_upload(id).await {
            tracing::warn!(upload_id = %id, error = %err, "failed to delete upload record");
        }
        if let Err(err) = self.store.remove_partial(id).await {
            tracing::warn!(upload_id = %id, error = %err, "failed to remove upload file");
        }
    }
}

fn declared_size(upload: &Upload) -> u64 {
    u64::try_from(upload.size).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use futures_util::stream;
    use sqlx::PgPool;
    use tokio::io::AsyncReadExt;

    use db::Db;

    use super::*;
    use crate::BlobStore;

    struct Fixture {
        files: FileService,
        alice: UserId,
        bob: UserId,
        _dir: tempfile::TempDir,
    }

    async fn fixture(pool: PgPool) -> Fixture {
        let db = Db::from_pool(pool);
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).await.expect("store");
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let bob = db.create_user("bob", "x").await.expect("bob").id;
        Fixture {
            files: FileService::new(db, store, 1024).with_chunk_bytes(4),
            alice,
            bob,
            _dir: dir,
        }
    }

    fn chunk(text: &'static str) -> impl Stream<Item = io::Result<Bytes>> + use<> {
        stream::iter([Ok(Bytes::from_static(text.as_bytes()))])
    }

    /// Yields `text` and then fails, like a connection that drops mid-chunk.
    fn broken(text: &'static str) -> impl Stream<Item = io::Result<Bytes>> + use<> {
        stream::iter([
            Ok(Bytes::from_static(text.as_bytes())),
            Err(io::Error::other("connection reset")),
        ])
    }

    fn new_file(name: &str) -> UploadTarget {
        UploadTarget::New {
            parent: None,
            name: name.to_owned(),
            options: StoreOptions::default(),
        }
    }

    async fn start(fx: &Fixture, size: u64) -> Uuid {
        fx.files
            .create_upload(fx.alice, None, new_file("a.txt"), size, None, None)
            .await
            .expect("create")
            .id
    }

    async fn append(fx: &Fixture, id: Uuid, offset: u64, text: &'static str) -> ChunkOutcome {
        fx.files
            .append_chunk(fx.alice, id, offset, chunk(text))
            .await
            .expect("append")
    }

    fn completed(outcome: ChunkOutcome) -> db::Node {
        match outcome {
            ChunkOutcome::Complete(stored) => stored.into_node(),
            ChunkOutcome::Partial { offset } => panic!("still partial at {offset}"),
            ChunkOutcome::Transferred { .. } => panic!("stored into a transfer"),
        }
    }

    async fn content_of(fx: &Fixture, id: NodeId) -> String {
        let (_, mut file) = fx.files.download(fx.alice, id).await.expect("download");
        let mut content = String::new();
        file.read_to_string(&mut content).await.expect("read");
        content
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn chunks_store_the_same_file_as_a_single_upload(pool: PgPool) {
        let fx = fixture(pool).await;
        let id = start(&fx, 11).await;
        assert!(matches!(
            append(&fx, id, 0, "hell").await,
            ChunkOutcome::Partial { offset: 4 }
        ));
        assert!(matches!(
            append(&fx, id, 4, "o wo").await,
            ChunkOutcome::Partial { offset: 8 }
        ));
        let node = completed(append(&fx, id, 8, "rld").await);
        assert_eq!(content_of(&fx, node.id).await, "hello world");
        assert_eq!(node.sha256, Some(Sha256::digest(b"hello world").to_vec()));
        assert_eq!((node.size, node.mime.as_str()), (11, "text/plain"));
        assert!(matches!(
            fx.files.upload_status(fx.alice, id).await,
            Err(FilesError::NotFound)
        ));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_wrong_offset_is_rejected_with_the_current_one(pool: PgPool) {
        let fx = fixture(pool).await;
        let id = start(&fx, 8).await;
        append(&fx, id, 0, "abc").await;
        let result = fx.files.append_chunk(fx.alice, id, 1, chunk("x")).await;
        assert!(matches!(
            result,
            Err(FilesError::OffsetMismatch { current: 3 })
        ));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn an_interrupted_chunk_resumes_from_the_received_bytes(pool: PgPool) {
        let fx = fixture(pool).await;
        let id = start(&fx, 6).await;
        let result = fx.files.append_chunk(fx.alice, id, 0, broken("abc")).await;
        assert!(result.is_err());
        let status = fx.files.upload_status(fx.alice, id).await.expect("status");
        assert_eq!(status.offset, 3);
        let node = completed(append(&fx, id, 3, "def").await);
        assert_eq!(node.sha256, Some(Sha256::digest(b"abcdef").to_vec()));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn completing_after_a_restart_rehashes_the_file(pool: PgPool) {
        let fx = fixture(pool).await;
        let id = start(&fx, 6).await;
        append(&fx, id, 0, "abc").await;
        fx.files.uploads.forget_all();
        let node = completed(append(&fx, id, 3, "def").await);
        assert_eq!(node.sha256, Some(Sha256::digest(b"abcdef").to_vec()));
        assert_eq!(content_of(&fx, node.id).await, "abcdef");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn sizes_over_the_limit_are_rejected(pool: PgPool) {
        let fx = fixture(pool).await;
        let result = fx
            .files
            .create_upload(fx.alice, None, new_file("a.txt"), 1025, None, None)
            .await;
        assert!(matches!(result, Err(FilesError::TooLarge { limit: 1024 })));
        let id = start(&fx, 2).await;
        let result = fx.files.append_chunk(fx.alice, id, 0, chunk("abc")).await;
        assert!(matches!(result, Err(FilesError::TooLarge { limit: 2 })));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn other_users_cannot_see_or_feed_an_upload(pool: PgPool) {
        let fx = fixture(pool).await;
        let id = start(&fx, 4).await;
        assert!(matches!(
            fx.files.upload_status(fx.bob, id).await,
            Err(FilesError::NotFound)
        ));
        assert!(matches!(
            fx.files.append_chunk(fx.bob, id, 0, chunk("abcd")).await,
            Err(FilesError::NotFound)
        ));
        assert!(matches!(
            fx.files.cancel_upload(fx.bob, id).await,
            Err(FilesError::NotFound)
        ));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_replace_upload_versions_the_existing_file(pool: PgPool) {
        let fx = fixture(pool).await;
        let file = fx
            .files
            .upload(fx.alice, None, "a.txt", None, None, chunk("old"))
            .await
            .expect("upload")
            .into_node();
        let id = fx
            .files
            .create_upload(
                fx.alice,
                None,
                UploadTarget::Replace(file.id),
                5,
                None,
                None,
            )
            .await
            .expect("create")
            .id;
        append(&fx, id, 0, "new").await;
        let node = completed(append(&fx, id, 3, "er").await);
        assert_eq!((node.id, node.version), (file.id, 2));
        assert_eq!(content_of(&fx, file.id).await, "newer");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn invalid_names_are_rejected_up_front(pool: PgPool) {
        let fx = fixture(pool).await;
        let result = fx
            .files
            .create_upload(fx.alice, None, new_file("a/b"), 1, None, None)
            .await;
        assert!(matches!(result, Err(FilesError::InvalidName)));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn cancelling_a_devices_uploads_leaves_the_others(pool: PgPool) {
        let fx = fixture(pool).await;
        let laptop = Uuid::now_v7();
        let from_laptop = fx
            .files
            .create_upload(fx.alice, Some(laptop), new_file("a.txt"), 4, None, None)
            .await
            .expect("laptop")
            .id;
        let unnamed = start(&fx, 4).await;

        let cancelled = fx
            .files
            .cancel_device_uploads(fx.alice, laptop)
            .await
            .expect("cancel");

        assert_eq!(cancelled, 1);
        assert!(matches!(
            fx.files.upload_status(fx.alice, from_laptop).await,
            Err(FilesError::NotFound)
        ));
        assert!(fx.files.upload_status(fx.alice, unnamed).await.is_ok());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn cancelling_and_expiring_remove_the_upload(pool: PgPool) {
        let fx = fixture(pool).await;
        let cancelled = start(&fx, 4).await;
        fx.files
            .cancel_upload(fx.alice, cancelled)
            .await
            .expect("cancel");
        assert!(matches!(
            fx.files.upload_status(fx.alice, cancelled).await,
            Err(FilesError::NotFound)
        ));

        let stale = start(&fx, 4).await;
        append(&fx, stale, 0, "ab").await;
        let fresh = start(&fx, 4).await;
        assert_eq!(
            fx.files
                .expire_uploads(Duration::from_secs(3600))
                .await
                .expect("keep"),
            0
        );
        assert!(
            fx.files
                .expire_uploads(Duration::ZERO)
                .await
                .expect("expire")
                >= 2
        );
        for id in [stale, fresh] {
            assert!(matches!(
                fx.files.upload_status(fx.alice, id).await,
                Err(FilesError::NotFound)
            ));
        }
    }
}
