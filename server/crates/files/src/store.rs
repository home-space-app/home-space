//! Blob storage on the local file system: file and version content, resumable uploads, and
//! files waiting in a transfer.

use std::{
    io,
    path::{Path, PathBuf},
};

use tokio::fs;
use uuid::Uuid;

use db::{NodeId, UserId};

const BLOBS_DIR: &str = "blobs";
const VERSIONS_DIR: &str = "versions";
const TMP_DIR: &str = "tmp";
const UPLOADS_DIR: &str = "uploads";
const TRANSFERS_DIR: &str = "transfers";
const HASH_BUFFER: usize = 1024 * 1024;

mod blobs;
mod partials;
mod transfers;

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

    fn partial_path(&self, upload: Uuid) -> PathBuf {
        self.root.join(UPLOADS_DIR).join(upload.to_string())
    }

    fn version_path(&self, owner: UserId, version: Uuid) -> PathBuf {
        self.root
            .join(VERSIONS_DIR)
            .join(owner.to_string())
            .join(version.to_string())
    }

    pub(crate) fn blob_path(&self, owner: UserId, node: NodeId) -> PathBuf {
        self.root
            .join(BLOBS_DIR)
            .join(owner.to_string())
            .join(node.to_string())
    }
}

/// `result`, with a file or directory that was missing already counted as removed.
fn ignore_missing(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Moves `from` to `to`, creating the directory `to` goes in.
async fn rename_into(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir).await?;
    }
    fs::rename(from, to).await
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
mod tests;
