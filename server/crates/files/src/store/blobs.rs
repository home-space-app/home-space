//! The stored content of files and versions, and the staging it passes through.

use std::{io, path::Path};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use db::{NodeId, UserId};

use super::{BLOBS_DIR, BlobStore, StagedBlob, TMP_DIR, VERSIONS_DIR, ignore_missing, rename_into};
use crate::{BodyError, FilesError};

impl BlobStore {
    /// Streams `body` into a temp file, hashing it and enforcing `max_bytes`.
    pub async fn stage<S>(&self, body: S, max_bytes: u64) -> Result<StagedBlob, FilesError>
    where
        S: Stream<Item = Result<Bytes, BodyError>>,
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
        rename_into(&staged.path, &self.blob_path(owner, node)).await
    }

    /// Moves a staged blob into place as the stored content of a version.
    pub async fn commit_version(
        &self,
        staged: &StagedBlob,
        owner: UserId,
        version: Uuid,
    ) -> io::Result<()> {
        rename_into(&staged.path, &self.version_path(owner, version)).await
    }

    /// Stages the current content of a node of `owner` to be stored as another file: a hard
    /// link where the file system allows it, a copy otherwise. Blobs are only ever replaced by
    /// a rename, so the link keeps the content as it is now.
    pub async fn stage_copy(
        &self,
        owner: UserId,
        node: NodeId,
        size: u64,
        sha256: Vec<u8>,
    ) -> io::Result<StagedBlob> {
        let path = self.root.join(TMP_DIR).join(NodeId::new().to_string());
        let source = self.blob_path(owner, node);
        if fs::hard_link(&source, &path).await.is_err() {
            fs::copy(&source, &path).await?;
        }
        Ok(StagedBlob { path, size, sha256 })
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
        ignore_missing(fs::remove_file(self.blob_path(owner, node)).await)
    }

    /// Keeps the current content of `node` as the stored content of a version.
    pub async fn archive(&self, owner: UserId, node: NodeId, version: Uuid) -> io::Result<()> {
        rename_into(
            &self.blob_path(owner, node),
            &self.version_path(owner, version),
        )
        .await
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
        ignore_missing(fs::remove_file(self.version_path(owner, version)).await)
    }

    /// Removes the stored content of every file and version of `owner`; missing directories
    /// are not an error.
    pub async fn remove_user(&self, owner: UserId) -> io::Result<()> {
        for dir in [BLOBS_DIR, VERSIONS_DIR] {
            ignore_missing(fs::remove_dir_all(self.root.join(dir).join(owner.to_string())).await)?;
        }
        Ok(())
    }

    async fn write_stream<S>(
        path: &Path,
        body: S,
        max_bytes: u64,
    ) -> Result<(u64, Vec<u8>), FilesError>
    where
        S: Stream<Item = Result<Bytes, BodyError>>,
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
