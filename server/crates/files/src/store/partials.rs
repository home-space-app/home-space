//! Resumable uploads, which grow in a file of their own chunk by chunk.

use std::{
    collections::HashSet,
    io,
    path::Path,
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

use super::{BlobStore, HASH_BUFFER, StagedBlob, TMP_DIR, UPLOADS_DIR, ignore_missing};
use crate::{BodyError, FilesError};

impl BlobStore {
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
        S: Stream<Item = Result<Bytes, BodyError>>,
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
        ignore_missing(fs::remove_file(self.partial_path(upload)).await)
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
}
