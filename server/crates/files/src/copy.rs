//! Copies of shared files and folders, saved into the reader's own tree.

use std::collections::HashMap;

use db::{Node, NodeId, NodeKind, SharedRoot, StoreOptions, StoreOutcome, UserId};

use crate::{FileService, FilesError, MAX_ARCHIVE_ENTRIES, service::new_content};

/// What saving copies of shared items made in the reader's tree.
#[derive(Debug, Default)]
pub struct SavedCopy {
    /// The folders that were created, parents first.
    pub folders: Vec<Node>,
    /// What storing each file did.
    pub files: Vec<StoreOutcome>,
}

impl FileService {
    /// Copies `node_ids`, each reachable through `share`, into `caller`'s folder `parent` (the
    /// root when `None`), folders with everything inside. A name that is taken at the top gets
    /// a number. The whole copy must fit in `caller`'s quota.
    pub async fn copy_shared(
        &self,
        caller: UserId,
        share: SharedRoot,
        node_ids: &[NodeId],
        parent: Option<NodeId>,
    ) -> Result<SavedCopy, FilesError> {
        if let Some(parent) = parent
            && self.db.get_node(caller, parent).await?.kind != NodeKind::Folder
        {
            return Err(FilesError::NotAFile);
        }
        let mut trees = Vec::with_capacity(node_ids.len());
        for &id in node_ids {
            let root = self.db.shared_node(share, id).await?;
            trees.push(self.db.subtree(share.owner_id, root.id).await?);
        }
        let count: usize = trees.iter().map(Vec::len).sum();
        if count > MAX_ARCHIVE_ENTRIES {
            return Err(FilesError::TooManyItems {
                limit: MAX_ARCHIVE_ENTRIES,
            });
        }
        let bytes = trees
            .iter()
            .flatten()
            .filter(|entry| entry.node.kind == NodeKind::File)
            .map(|entry| u64::try_from(entry.node.size).unwrap_or(0))
            .sum();
        self.check_quota(caller, None, bytes).await?;

        let mut saved = SavedCopy::default();
        for tree in trees {
            let mut placed = HashMap::<String, NodeId>::new();
            for entry in tree {
                let into = match entry.rel.rsplit_once('/') {
                    Some((dir, _)) => Some(*placed.get(dir).ok_or(FilesError::NotFound)?),
                    None => parent,
                };
                let node = entry.node;
                match node.kind {
                    NodeKind::Folder => {
                        let folder = self.save_folder(caller, into, &node.name).await?;
                        placed.insert(entry.rel, folder.id);
                        saved.folders.push(folder);
                    }
                    NodeKind::File => {
                        let outcome = self.copy_file(caller, share.owner_id, &node, into).await?;
                        saved.files.push(outcome);
                    }
                }
            }
        }
        Ok(saved)
    }

    async fn copy_file(
        &self,
        caller: UserId,
        owner: UserId,
        node: &Node,
        parent: Option<NodeId>,
    ) -> Result<StoreOutcome, FilesError> {
        let staged = self
            .store
            .stage_copy(
                owner,
                node.id,
                u64::try_from(node.size).unwrap_or(0),
                node.sha256.clone().unwrap_or_default(),
            )
            .await?;
        let content = new_content(&staged, Some(&node.mime), &node.name, Some(node.updated_at));
        let options = StoreOptions {
            keep_both: true,
            ..StoreOptions::default()
        };
        let stored = self
            .db
            .store_file_with(caller, parent, &node.name, content, options)
            .await;
        self.finish_store(caller, staged, stored).await
    }
}
