//! Copies of shared files and folders, saved into the reader's own tree.

use std::collections::HashMap;

use db::{Node, NodeId, NodeKind, SharedRoot, UserId};

use crate::{
    Actor, FilesError, MAX_ARCHIVE_ENTRIES, NodeService, StoreOptions, StoreOutcome,
    content::new_content, emitted, tree,
};

/// What saving copies of shared items made in the reader's tree.
#[derive(Debug, Default)]
pub struct SavedCopy {
    /// The folders that were created, parents first.
    pub folders: Vec<Node>,
    /// What storing each file did.
    pub files: Vec<StoreOutcome>,
}

impl NodeService {
    /// Copies `node_ids`, each reachable through `share`, into `caller`'s folder `parent` (the
    /// root when `None`), folders with everything inside. A name that is taken at the top gets
    /// a number. The whole copy must fit in `caller`'s quota.
    pub async fn copy_shared(
        &self,
        actor: Actor,
        share: SharedRoot,
        node_ids: &[NodeId],
        parent: Option<NodeId>,
    ) -> Result<SavedCopy, FilesError> {
        self.acting(actor.device)
            .copy_shared_as(actor.user, share, node_ids, parent)
            .await
    }

    async fn copy_file(
        &self,
        caller: UserId,
        owner: UserId,
        node: &Node,
        parent: Option<NodeId>,
    ) -> Result<StoreOutcome, FilesError> {
        let staged = self
            .content
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
        let stored = tree::store_file(
            &self.content.db,
            caller,
            parent,
            &node.name,
            content,
            options,
        )
        .await;
        self.content.finish_store(caller, staged, stored).await
    }

    async fn copy_shared_as(
        &self,
        caller: UserId,
        share: SharedRoot,
        node_ids: &[NodeId],
        parent: Option<NodeId>,
    ) -> Result<SavedCopy, FilesError> {
        if let Some(parent) = parent
            && self.content.db.get_node(caller, parent).await?.kind != NodeKind::Folder
        {
            return Err(FilesError::NotAFile);
        }
        let mut trees = Vec::with_capacity(node_ids.len());
        for &id in node_ids {
            let root = self.content.db.shared_node(share, id).await?;
            trees.push(self.content.db.subtree(share.owner_id, root.id).await?);
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
        self.content.check_quota(caller, None, bytes).await?;

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
                        let folder = self.content.save_folder(caller, into, &node.name).await?;
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
        self.content
            .events
            .publish(emitted::saved(&saved.folders, &saved.files))
            .await;
        Ok(saved)
    }
}
