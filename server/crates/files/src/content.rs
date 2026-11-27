//! What every file service shares: the database, the blob store, the quota, and where the
//! changes they make are published.

use bytes::Bytes;
use futures_util::Stream;
use time::OffsetDateTime;
use uuid::Uuid;

use db::{Db, DeletedNodes, NewContent, Node, NodeId, NodeKind, QuotaUsage, UserId};
use events::Events;
use protocol::ProtectionState;

use crate::{
    BlobOp, BlobStore, BodyError, FilesError, Names, StoreOutcome, emitted,
    store::StagedBlob,
    tree::{guard, rules::numbered_folder_name},
};

pub(crate) const FALLBACK_MIME: &str = "application/octet-stream";
/// How many locks the stores and conversions of all users are spread over, so that encrypting
/// one user's folder does not hold up the stores of most others.
const SEALING_LOCKS: usize = 64;
/// How many numbered names (`Photos (2)`, `Photos (3)`, ...) are tried for a taken one.
const MAX_NUMBERED_NAMES: usize = 1000;

fn quota_exceeded(usage: QuotaUsage) -> FilesError {
    FilesError::QuotaExceeded {
        quota: u64::try_from(usage.quota).unwrap_or(0),
        available: usage.room(),
    }
}

/// The content of the users' trees: their nodes in the database and their bytes in the blob
/// store. Cheap to clone.
#[derive(Clone, Debug)]
pub(crate) struct ContentStore {
    pub(crate) db: Db,
    pub(crate) store: BlobStore,
    /// The largest file a single upload may hold.
    pub(crate) max_upload_bytes: u64,
    /// Where every change is published, for search and the user's devices.
    pub(crate) events: Events,
    /// Stores hold their owner's lock (see [`Self::sealing`]) shared from reading the folder's
    /// state until the blob is committed; the conversion that encrypts or decrypts a folder in
    /// place holds it exclusively for each blob. So neither overwrites what the other just
    /// wrote, and a store never lands in the form the folder had before its state changed.
    sealing_locks: std::sync::Arc<[std::sync::Arc<tokio::sync::RwLock<()>>]>,
}

impl ContentStore {
    pub(crate) fn new(db: Db, store: BlobStore, max_upload_bytes: u64, events: Events) -> Self {
        Self {
            db,
            store,
            max_upload_bytes,
            events,
            sealing_locks: (0..SEALING_LOCKS)
                .map(|_| std::sync::Arc::default())
                .collect(),
        }
    }

    /// The lock that keeps stores of `owner`'s blobs and conversions of their folders apart.
    pub(crate) fn sealing(&self, owner: UserId) -> std::sync::Arc<tokio::sync::RwLock<()>> {
        let mut hasher = std::hash::DefaultHasher::new();
        std::hash::Hash::hash(&owner, &mut hasher);
        let count = u64::try_from(self.sealing_locks.len()).unwrap_or(u64::MAX);
        let index = usize::try_from(std::hash::Hasher::finish(&hasher) % count).unwrap_or(0);
        self.sealing_locks[index].clone()
    }

    /// The same content, recording `device` as the maker of the changes made through it.
    pub(crate) fn acting(&self, device: Option<Uuid>) -> Self {
        Self {
            db: self.db.on_device(device),
            ..self.clone()
        }
    }

    /// Creates a folder without publishing it, for changes that publish all they made at once.
    /// `name` is stored as `names`, the names of `parent`, say.
    pub(crate) async fn insert_folder(
        &self,
        owner: UserId,
        parent: Option<NodeId>,
        name: &str,
        names: &Names,
    ) -> Result<Node, FilesError> {
        let mut tx = self.db.transaction(owner).await?;
        guard::hold(&mut tx).await?;
        guard::check_names(&mut tx, parent, names).await?;
        let folder = tx.insert_folder(parent, name).await?;
        tx.commit().await?;
        Ok(folder)
    }

    /// Publishes what storing a file changed, when it succeeded.
    pub(crate) async fn publish_stored(
        &self,
        stored: Result<StoreOutcome, FilesError>,
    ) -> Result<StoreOutcome, FilesError> {
        if let Ok(outcome) = &stored {
            self.events.publish(emitted::stored(outcome)).await;
        }
        stored
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
    pub(crate) async fn stage_within_quota<S>(
        &self,
        owner: UserId,
        body: S,
    ) -> Result<StagedBlob, FilesError>
    where
        S: Stream<Item = Result<Bytes, BodyError>>,
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
        stored: Result<StoreOutcome, FilesError>,
    ) -> Result<StoreOutcome, FilesError> {
        let outcome = match stored {
            Ok(outcome) => outcome,
            Err(err) => {
                self.store.discard(staged).await;
                return Err(err);
            }
        };
        let node = match &outcome {
            StoreOutcome::Unchanged(_) => {
                self.store.discard(staged).await;
                return Ok(outcome);
            }
            StoreOutcome::Created(node)
            | StoreOutcome::Versioned { node, .. }
            | StoreOutcome::Archived { node, .. } => node.id,
        };
        let (staged, _sealing) = match self.seal_for(owner, node, staged).await {
            Ok(sealed) => sealed,
            Err(err) => {
                if let StoreOutcome::Created(node) = &outcome {
                    self.rollback_node(owner, node.id).await;
                }
                return Err(err);
            }
        };
        let committed = match &outcome {
            StoreOutcome::Unchanged(_) => Ok(()),
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

    /// The blob to store for `node`: `staged` encrypted to its folder's recipient when it is
    /// inside a password folder, as it is otherwise. While that folder's existing content is
    /// still being encrypted, also the guard that keeps the conversion from writing the same
    /// blobs meanwhile; it is held until the blob is in place.
    async fn seal_for(
        &self,
        owner: UserId,
        node: NodeId,
        staged: StagedBlob,
    ) -> Result<(StagedBlob, Option<tokio::sync::OwnedRwLockReadGuard<()>>), FilesError> {
        // Held until the blob is committed, and taken before the folder's state is read, also
        // outside password folders: one may be protected meanwhile. A conversion takes it
        // exclusively for each blob, so once it starts, every store that saw the state before
        // has landed (a plain blob it then seals) and every later one sees the new state.
        let guard = self.sealing(owner).read_owned().await;
        let folder = match self.db.enclosing_password_folder(owner, node).await {
            Ok(Some(folder)) => folder,
            Ok(None) => return Ok((staged, Some(guard))),
            Err(err) => {
                self.store.discard(staged).await;
                return Err(err.into());
            }
        };
        let recipient = match vault_core::content::parse_recipient(&folder.recipient) {
            Ok(recipient) => recipient,
            Err(err) => {
                self.store.discard(staged).await;
                return Err(FilesError::Io(std::io::Error::other(err)));
            }
        };
        let guard = Some(guard);
        // A folder being unprotected is decrypted in place; what arrives meanwhile stays plain.
        if folder.state == ProtectionState::Unprotecting {
            return Ok((staged, guard));
        }
        let sealed = self.store.seal_staged(staged, recipient).await?;
        Ok((sealed, guard))
    }

    pub(crate) async fn apply_blob_ops(&self, owner: UserId, ops: &[BlobOp]) {
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

    /// Removes the stored bytes of deleted files and versions. A failure is logged: the rows
    /// are gone, so the bytes are only space the next sweep can reclaim.
    pub(crate) async fn remove_content(&self, owner: UserId, deleted: DeletedNodes) {
        for file_id in deleted.files {
            if let Err(err) = self.store.remove(owner, file_id).await {
                tracing::warn!(%file_id, error = %err, "failed to remove blob");
            }
        }
        for version_id in deleted.versions {
            self.remove_version_blob(owner, version_id).await;
        }
    }

    /// Removes the stored bytes of a deleted version; a failure is logged.
    pub(crate) async fn remove_version_blob(&self, owner: UserId, version_id: Uuid) {
        if let Err(err) = self.store.remove_version(owner, version_id).await {
            tracing::warn!(%version_id, error = %err, "failed to remove version blob");
        }
    }

    pub(crate) async fn rollback_node(&self, owner: UserId, id: NodeId) {
        if let Err(err) = self.db.delete_node(owner, id).await {
            tracing::error!(node_id = %id, error = %err, "failed to roll back node");
        }
    }

    /// Creates the folder `name` in `parent`, or `name (2)` and so on when it is taken, with
    /// the name stored as `names` says.
    pub(crate) async fn save_folder(
        &self,
        owner: UserId,
        parent: Option<NodeId>,
        name: &str,
        names: &Names,
    ) -> Result<Node, FilesError> {
        for n in 0..MAX_NUMBERED_NAMES {
            let candidate = names.store(NodeKind::Folder, &numbered_folder_name(name, n))?;
            match self.insert_folder(owner, parent, &candidate, names).await {
                Err(FilesError::Conflict) => {}
                other => return other,
            }
        }
        Err(FilesError::Conflict)
    }

    pub(crate) async fn remove_transfer_files(&self, id: Uuid) {
        if let Err(err) = self.store.remove_transfer(id).await {
            tracing::warn!(transfer_id = %id, error = %err, "failed to remove transfer files");
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
pub(crate) fn resolve_mime(declared: Option<&str>, name: &str) -> String {
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
