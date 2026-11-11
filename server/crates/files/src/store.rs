use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
};

use uuid::Uuid;

use db::{NodeId, UserId};

use crate::FilesError;

const BLOBS_DIR: &str = "blobs";
const VERSIONS_DIR: &str = "versions";
const TMP_DIR: &str = "tmp";
const UPLOADS_DIR: &str = "uploads";
const TRANSFERS_DIR: &str = "transfers";
const HASH_BUFFER: usize = 1024 * 1024;

/// A fully received upload waiting to be committed into the store.
#[derive(Debug)]
pub struct StagedBlob {
    path: PathBuf,
    pub size: u64,
    pub sha256: Vec<u8>,
}

/// Blob storage on the local file system.
///
/// Layout: `<root>/blobs/<user_id>/<node_id>` and `<root>/versions/<user_id>/<version_id>`;
/// single-request uploads are staged in `<root>/tmp` and resumable ones grow in
/// `<root>/uploads/<upload_id>`; files sent to a device wait in
/// `<root>/transfers/<transfer_id>/<item_id>`.
#[derive(Clone, Debug)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    /// Creates the directory layout under `root` if it does not exist.
    pub async fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join(BLOBS_DIR)).await?;
        fs::create_dir_all(root.join(VERSIONS_DIR)).await?;
        fs::create_dir_all(root.join(TMP_DIR)).await?;
        fs::create_dir_all(root.join(UPLOADS_DIR)).await?;
        fs::create_dir_all(root.join(TRANSFERS_DIR)).await?;
        Ok(Self { root })
    }

    /// The directory everything is stored under.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Size of the filesystem holding the root and the bytes still available on it; `None`
    /// where the platform cannot tell.
    pub async fn disk_space(&self) -> io::Result<Option<protocol::DiskSpace>> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || disk_space(&root))
            .await
            .map_err(io::Error::other)?
    }

    /// Streams `body` into a temp file, hashing it and enforcing `max_bytes`.
    pub async fn stage<S>(&self, body: S, max_bytes: u64) -> Result<StagedBlob, FilesError>
    where
        S: Stream<Item = io::Result<Bytes>>,
    {
        let path = self.root.join(TMP_DIR).join(NodeId::new().to_string());
        let staged = Self::write_stream(&path, body, max_bytes).await;
        match staged {
            Ok((size, sha256)) => Ok(StagedBlob { path, size, sha256 }),
            Err(err) => {
                let _ = fs::remove_file(&path).await;
                Err(err)
            }
        }
    }

    /// Moves a staged blob into its final place, replacing any previous content.
    /// On error the staged blob is left in place for [`Self::discard`].
    pub async fn commit(&self, staged: &StagedBlob, owner: UserId, node: NodeId) -> io::Result<()> {
        let target = self.blob_path(owner, node);
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir).await?;
        }
        fs::rename(&staged.path, target).await
    }

    /// Moves a staged blob into place as the stored content of a version.
    pub async fn commit_version(
        &self,
        staged: &StagedBlob,
        owner: UserId,
        version: Uuid,
    ) -> io::Result<()> {
        let target = self.version_path(owner, version);
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir).await?;
        }
        fs::rename(&staged.path, target).await
    }

    /// Discards a staged blob.
    pub async fn discard(&self, staged: StagedBlob) {
        let _ = fs::remove_file(&staged.path).await;
    }

    /// Opens the stored content of a node.
    pub async fn open_blob(&self, owner: UserId, node: NodeId) -> io::Result<fs::File> {
        fs::File::open(self.blob_path(owner, node)).await
    }

    /// Removes the stored content of a node; a missing blob is not an error.
    pub async fn remove(&self, owner: UserId, node: NodeId) -> io::Result<()> {
        match fs::remove_file(self.blob_path(owner, node)).await {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }

    /// Keeps the current content of `node` as the stored content of a version.
    pub async fn archive(&self, owner: UserId, node: NodeId, version: Uuid) -> io::Result<()> {
        let target = self.version_path(owner, version);
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir).await?;
        }
        fs::rename(self.blob_path(owner, node), target).await
    }

    /// Makes the content of node `from` the content of node `to`, replacing it.
    pub async fn promote(&self, owner: UserId, from: NodeId, to: NodeId) -> io::Result<()> {
        fs::rename(self.blob_path(owner, from), self.blob_path(owner, to)).await
    }

    /// Opens the stored content of a version.
    pub async fn open_version(&self, owner: UserId, version: Uuid) -> io::Result<fs::File> {
        fs::File::open(self.version_path(owner, version)).await
    }

    /// Removes the stored content of a version; a missing blob is not an error.
    pub async fn remove_version(&self, owner: UserId, version: Uuid) -> io::Result<()> {
        match fs::remove_file(self.version_path(owner, version)).await {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }

    /// Removes the stored content of every file and version of `owner`; missing directories
    /// are not an error.
    pub async fn remove_user(&self, owner: UserId) -> io::Result<()> {
        for dir in [BLOBS_DIR, VERSIONS_DIR] {
            match fs::remove_dir_all(self.root.join(dir).join(owner.to_string())).await {
                Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
                _ => {}
            }
        }
        Ok(())
    }

    /// Creates the empty file a resumable upload grows in.
    pub async fn create_partial(&self, upload: Uuid) -> io::Result<()> {
        fs::File::create(self.partial_path(upload)).await.map(drop)
    }

    /// How many bytes of a resumable upload have arrived.
    pub async fn partial_len(&self, upload: Uuid) -> Result<u64, FilesError> {
        match fs::metadata(self.partial_path(upload)).await {
            Ok(meta) => Ok(meta.len()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Err(FilesError::NotFound),
            Err(err) => Err(err.into()),
        }
    }

    /// Appends `body` to a resumable upload that currently holds exactly `offset` bytes and
    /// may grow to at most `size`, passing every piece to `on_written` once it is written.
    ///
    /// Returns the new length. On error, the pieces written before it stay in the file.
    pub async fn append<S>(
        &self,
        upload: Uuid,
        offset: u64,
        size: u64,
        body: S,
        mut on_written: impl FnMut(&[u8]),
    ) -> Result<u64, FilesError>
    where
        S: Stream<Item = io::Result<Bytes>>,
    {
        let mut file = match fs::OpenOptions::new()
            .append(true)
            .open(self.partial_path(upload))
            .await
        {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Err(FilesError::NotFound),
            Err(err) => return Err(err.into()),
        };
        let current = file.metadata().await?.len();
        if current != offset {
            return Err(FilesError::OffsetMismatch { current });
        }
        futures_util::pin_mut!(body);
        let mut len = offset;
        let written = async {
            while let Some(chunk) = body.next().await {
                let chunk = chunk?;
                if len + chunk.len() as u64 > size {
                    return Err(FilesError::TooLarge { limit: size });
                }
                file.write_all(&chunk).await?;
                len += chunk.len() as u64;
                on_written(&chunk);
            }
            Ok(())
        }
        .await;
        let flushed = file.flush().await;
        written?;
        flushed?;
        file.sync_all().await?;
        Ok(len)
    }

    /// Hashes everything a resumable upload has received.
    pub async fn hash_partial(&self, upload: Uuid) -> io::Result<Sha256> {
        let mut file = fs::File::open(self.partial_path(upload)).await?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0; HASH_BUFFER];
        loop {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                return Ok(hasher);
            }
            hasher.update(&buffer[..read]);
        }
    }

    /// Turns a completed resumable upload into a staged blob ready to commit.
    pub fn adopt_partial(&self, upload: Uuid, size: u64, sha256: Vec<u8>) -> StagedBlob {
        StagedBlob {
            path: self.partial_path(upload),
            size,
            sha256,
        }
    }

    /// Removes a resumable upload's file; a missing one is not an error.
    pub async fn remove_partial(&self, upload: Uuid) -> io::Result<()> {
        match fs::remove_file(self.partial_path(upload)).await {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }

    /// Removes leftovers untouched for `older_than`: staged files of interrupted
    /// single-request uploads, and upload files whose id is not in `live`.
    ///
    /// Returns how many files were removed.
    pub async fn sweep(&self, live: &HashSet<Uuid>, older_than: Duration) -> io::Result<usize> {
        let cutoff = SystemTime::now()
            .checked_sub(older_than)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let staged = Self::sweep_dir(&self.root.join(TMP_DIR), cutoff, |_| true).await?;
        let orphans = Self::sweep_dir(&self.root.join(UPLOADS_DIR), cutoff, |name| {
            name.parse::<Uuid>().map_or(true, |id| !live.contains(&id))
        })
        .await?;
        Ok(staged + orphans)
    }

    async fn sweep_dir(
        dir: &Path,
        cutoff: SystemTime,
        removable: impl Fn(&str) -> bool,
    ) -> io::Result<usize> {
        let mut removed = 0;
        let mut entries = fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let meta = entry.metadata().await?;
            let stale = meta.modified().is_ok_and(|at| at < cutoff);
            let name = entry.file_name();
            if meta.is_file() && stale && removable(&name.to_string_lossy()) {
                match fs::remove_file(entry.path()).await {
                    Ok(()) => removed += 1,
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err),
                }
            }
        }
        Ok(removed)
    }

    /// Puts the current content of a node into a transfer: a hard link where the file system
    /// allows it, a copy otherwise. Blobs are only ever replaced by a rename, never written in
    /// place, so the link keeps the content as it is now.
    pub async fn link_into_transfer(
        &self,
        owner: UserId,
        node: NodeId,
        transfer: Uuid,
        item: Uuid,
    ) -> io::Result<()> {
        let target = self.transfer_item_path(transfer, item);
        fs::create_dir_all(self.transfer_dir(transfer)).await?;
        match fs::remove_file(&target).await {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
        let source = self.blob_path(owner, node);
        if fs::hard_link(&source, &target).await.is_err() {
            fs::copy(&source, &target).await?;
        }
        Ok(())
    }

    /// Moves a completed resumable upload into a transfer.
    pub async fn commit_into_transfer(
        &self,
        upload: Uuid,
        transfer: Uuid,
        item: Uuid,
    ) -> io::Result<()> {
        fs::create_dir_all(self.transfer_dir(transfer)).await?;
        fs::rename(
            self.partial_path(upload),
            self.transfer_item_path(transfer, item),
        )
        .await
    }

    /// Opens a file of a transfer.
    pub async fn open_transfer_item(&self, transfer: Uuid, item: Uuid) -> io::Result<fs::File> {
        fs::File::open(self.transfer_item_path(transfer, item)).await
    }

    /// Turns a file of a transfer into a staged blob, to be stored in the recipient's tree.
    pub fn adopt_transfer_item(
        &self,
        transfer: Uuid,
        item: Uuid,
        size: u64,
        sha256: Vec<u8>,
    ) -> StagedBlob {
        StagedBlob {
            path: self.transfer_item_path(transfer, item),
            size,
            sha256,
        }
    }

    /// Removes every file of a transfer; a missing one is not an error.
    pub async fn remove_transfer(&self, transfer: Uuid) -> io::Result<()> {
        match fs::remove_dir_all(self.transfer_dir(transfer)).await {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }

    /// Removes transfer directories untouched for `older_than` whose id is not in `live`,
    /// as left behind by a crash. Returns how many were removed.
    pub async fn sweep_transfers(
        &self,
        live: &HashSet<Uuid>,
        older_than: Duration,
    ) -> io::Result<usize> {
        let cutoff = SystemTime::now()
            .checked_sub(older_than)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let mut removed = 0;
        let mut entries = fs::read_dir(self.root.join(TRANSFERS_DIR)).await?;
        while let Some(entry) = entries.next_entry().await? {
            let meta = entry.metadata().await?;
            let stale = meta.modified().is_ok_and(|at| at < cutoff);
            let name = entry.file_name();
            let orphan = name
                .to_string_lossy()
                .parse::<Uuid>()
                .map_or(true, |id| !live.contains(&id));
            if stale && orphan {
                let removal = if meta.is_dir() {
                    fs::remove_dir_all(entry.path()).await
                } else {
                    fs::remove_file(entry.path()).await
                };
                match removal {
                    Ok(()) => removed += 1,
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err),
                }
            }
        }
        Ok(removed)
    }

    fn transfer_dir(&self, transfer: Uuid) -> PathBuf {
        self.root.join(TRANSFERS_DIR).join(transfer.to_string())
    }

    /// Where the bytes of a file of a transfer are.
    pub fn transfer_item_path(&self, transfer: Uuid, item: Uuid) -> PathBuf {
        self.transfer_dir(transfer).join(item.to_string())
    }

    fn partial_path(&self, upload: Uuid) -> PathBuf {
        self.root.join(UPLOADS_DIR).join(upload.to_string())
    }

    fn version_path(&self, owner: UserId, version: Uuid) -> PathBuf {
        self.root
            .join(VERSIONS_DIR)
            .join(owner.to_string())
            .join(version.to_string())
    }

    fn blob_path(&self, owner: UserId, node: NodeId) -> PathBuf {
        self.root
            .join(BLOBS_DIR)
            .join(owner.to_string())
            .join(node.to_string())
    }

    async fn write_stream<S>(
        path: &Path,
        body: S,
        max_bytes: u64,
    ) -> Result<(u64, Vec<u8>), FilesError>
    where
        S: Stream<Item = io::Result<Bytes>>,
    {
        futures_util::pin_mut!(body);
        let mut file = fs::File::create(path).await?;
        let mut hasher = Sha256::new();
        let mut size = 0u64;
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
            size += chunk.len() as u64;
            if size > max_bytes {
                return Err(FilesError::TooLarge { limit: max_bytes });
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.sync_all().await?;
        Ok((size, hasher.finalize().to_vec()))
    }
}

#[cfg(unix)]
fn disk_space(path: &Path) -> io::Result<Option<protocol::DiskSpace>> {
    let stats = rustix::fs::statvfs(path)?;
    let block = stats.f_frsize;
    Ok(Some(protocol::DiskSpace {
        total: stats.f_blocks.saturating_mul(block),
        available: stats.f_bavail.saturating_mul(block),
    }))
}

#[cfg(not(unix))]
fn disk_space(_path: &Path) -> io::Result<Option<protocol::DiskSpace>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use futures_util::stream;

    use super::*;

    fn body(parts: &[&'static str]) -> impl Stream<Item = io::Result<Bytes>> + use<> {
        stream::iter(
            parts
                .iter()
                .map(|part| Ok(Bytes::from_static(part.as_bytes())))
                .collect::<Vec<_>>(),
        )
    }

    #[tokio::test]
    async fn disk_space_is_reported_for_the_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).await.expect("open");
        let space = store.disk_space().await.expect("statvfs");
        if cfg!(unix) {
            let space = space.expect("space");
            assert!(space.total > 0);
            assert!(space.available <= space.total);
        }
    }

    #[tokio::test]
    async fn staged_blob_reports_size_and_hash() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).await.expect("open");
        let staged = store.stage(body(&["hel", "lo"]), 100).await.expect("stage");
        assert_eq!(staged.size, 5);
        assert_eq!(staged.sha256, Sha256::digest(b"hello").to_vec());
    }

    #[tokio::test]
    async fn oversized_upload_is_rejected_and_cleaned_up() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).await.expect("open");
        let result = store.stage(body(&["hello"]), 4).await;
        assert!(matches!(result, Err(FilesError::TooLarge { limit: 4 })));
        let mut leftovers = fs::read_dir(dir.path().join(TMP_DIR))
            .await
            .expect("read_dir");
        assert!(leftovers.next_entry().await.expect("entry").is_none());
    }

    #[tokio::test]
    async fn committed_blob_can_be_read_and_removed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).await.expect("open");
        let (owner, node) = (UserId::new(), NodeId::new());
        let staged = store.stage(body(&["data"]), 100).await.expect("stage");
        store.commit(&staged, owner, node).await.expect("commit");
        assert!(store.open_blob(owner, node).await.is_ok());
        store.remove(owner, node).await.expect("remove");
        assert!(store.open_blob(owner, node).await.is_err());
    }

    #[tokio::test]
    async fn appended_chunks_hash_like_a_single_stage() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).await.expect("open");
        let id = Uuid::now_v7();
        store.create_partial(id).await.expect("create");
        let mut hasher = Sha256::new();
        let len = store
            .append(id, 0, 5, body(&["he", "l"]), |piece| hasher.update(piece))
            .await
            .expect("first");
        assert_eq!(len, 3);
        let len = store
            .append(id, 3, 5, body(&["lo"]), |piece| hasher.update(piece))
            .await
            .expect("second");
        assert_eq!(len, 5);
        let digest = Sha256::digest(b"hello").to_vec();
        assert_eq!(hasher.finalize().to_vec(), digest);
        let rehashed = store.hash_partial(id).await.expect("hash");
        assert_eq!(rehashed.finalize().to_vec(), digest);
    }

    #[tokio::test]
    async fn append_at_the_wrong_offset_reports_the_current_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).await.expect("open");
        let id = Uuid::now_v7();
        store.create_partial(id).await.expect("create");
        store
            .append(id, 0, 10, body(&["abc"]), |_| ())
            .await
            .expect("append");
        let result = store.append(id, 1, 10, body(&["x"]), |_| ()).await;
        assert!(matches!(
            result,
            Err(FilesError::OffsetMismatch { current: 3 })
        ));
    }

    #[tokio::test]
    async fn append_past_the_declared_size_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).await.expect("open");
        let id = Uuid::now_v7();
        store.create_partial(id).await.expect("create");
        let result = store.append(id, 0, 4, body(&["abc", "de"]), |_| ()).await;
        assert!(matches!(result, Err(FilesError::TooLarge { limit: 4 })));
        assert_eq!(store.partial_len(id).await.expect("len"), 3);
    }

    #[tokio::test]
    async fn sweep_removes_only_stale_orphans() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).await.expect("open");
        let (live, orphan) = (Uuid::now_v7(), Uuid::now_v7());
        store.create_partial(live).await.expect("live");
        store.create_partial(orphan).await.expect("orphan");
        let known = HashSet::from([live]);
        assert_eq!(
            store
                .sweep(&known, Duration::from_secs(3600))
                .await
                .expect("fresh"),
            0
        );
        assert_eq!(store.sweep(&known, Duration::ZERO).await.expect("stale"), 1);
        assert_eq!(store.partial_len(live).await.expect("live"), 0);
        assert!(matches!(
            store.partial_len(orphan).await,
            Err(FilesError::NotFound)
        ));
    }
}
