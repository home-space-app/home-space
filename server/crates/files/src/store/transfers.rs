//! Files sent to a device, waiting in the transfer until the recipient takes them.

use std::{
    collections::HashSet,
    io,
    path::PathBuf,
    time::{Duration, SystemTime},
};

use tokio::fs;
use uuid::Uuid;

use db::{NodeId, UserId};

use super::{BlobStore, StagedBlob, TRANSFERS_DIR, ignore_missing};

impl BlobStore {
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
        ignore_missing(fs::remove_file(&target).await)?;
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
        ignore_missing(fs::remove_dir_all(self.transfer_dir(transfer)).await)
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
}
