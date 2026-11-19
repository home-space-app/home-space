//! The previous versions of a user's files.

use tokio::fs;

use db::{FileVersion, Node, NodeId, UserId};

use crate::{FilesError, content::ContentStore};

/// The previous versions of a user's files.
#[derive(Clone, Debug)]
pub struct VersionService {
    pub(crate) content: ContentStore,
}

impl VersionService {
    pub(crate) fn new(content: ContentStore) -> Self {
        Self { content }
    }

    /// Lists the previous versions of a file, newest first.
    pub async fn versions(
        &self,
        owner: UserId,
        id: NodeId,
    ) -> Result<Vec<FileVersion>, FilesError> {
        Ok(self.content.db.list_versions(owner, id).await?)
    }

    /// Opens a previous version of a file for download.
    pub async fn download_version(
        &self,
        owner: UserId,
        id: NodeId,
        version: i32,
    ) -> Result<(Node, FileVersion, fs::File), FilesError> {
        let node = self.content.db.get_node(owner, id).await?;
        let meta = self.content.db.get_version(owner, id, version).await?;
        let file = self.content.store.open_version(owner, meta.id).await?;
        Ok((node, meta, file))
    }

    /// Deletes a previous version and its stored content.
    pub async fn delete_version(
        &self,
        owner: UserId,
        id: NodeId,
        version: i32,
    ) -> Result<(), FilesError> {
        let version_id = self.content.db.delete_version(owner, id, version).await?;
        self.content.remove_version_blob(owner, version_id).await;
        Ok(())
    }
}
