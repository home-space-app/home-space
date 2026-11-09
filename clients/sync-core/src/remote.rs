use std::collections::{HashMap, HashSet};

use api_client::{ApiError, Client};
use protocol::{Change, ErrorCode, Event, NodeId, NodeKind};

use super::model::{Mapping, RemoteNode};

/// The remote tree as last seen, kept current from the server's change journal.
#[derive(Debug, Clone, Default)]
pub struct Mirror {
    pub nodes: HashMap<NodeId, RemoteNode>,
    pub cursor: Option<i64>,
}

/// How the mirror changed in a refresh, for the store to record.
#[derive(Debug, Default)]
pub struct Delta {
    pub replace: bool,
    pub upserted: Vec<RemoteNode>,
    pub removed: Vec<NodeId>,
}

impl Mirror {
    /// Brings the mirror up to date: from the journal when it has a cursor, otherwise (or
    /// when the journal was pruned past it) by reading every mapped folder again.
    pub async fn refresh(
        &mut self,
        client: &Client,
        mappings: &[Mapping],
    ) -> Result<Delta, ApiError> {
        if let Some(cursor) = self.cursor {
            match self.catch_up(client, cursor).await {
                Err(ApiError::Api {
                    code: ErrorCode::Gone,
                    ..
                }) => {}
                other => return other,
            }
        }
        self.reload(client, mappings).await
    }

    /// Reads every mapped folder from scratch, taking the cursor first so that changes made
    /// meanwhile are caught up on next time.
    pub async fn reload(
        &mut self,
        client: &Client,
        mappings: &[Mapping],
    ) -> Result<Delta, ApiError> {
        let cursor = client.changes(None).await?.cursor;
        let mapped: HashSet<Option<NodeId>> = mappings.iter().map(|m| m.remote).collect();
        let mut nodes = HashMap::new();
        for mapping in mappings.iter().filter(|m| m.local.is_some()) {
            if let Some(id) = mapping.remote {
                match client.get(id).await {
                    Ok(node) => nodes.insert(id, RemoteNode::from(node)),
                    // The mapped folder is gone; its mapping simply has nothing to sync.
                    Err(ApiError::Api {
                        code: ErrorCode::NotFound,
                        ..
                    }) => continue,
                    Err(err) => return Err(err),
                };
            }
            // A mapped file has nothing under it.
            let is_file = mapping
                .remote
                .and_then(|id| nodes.get(&id))
                .is_some_and(|node| node.kind == NodeKind::File);
            if is_file {
                continue;
            }
            for node in walk(client, mapping.remote, &mapped).await? {
                nodes.insert(node.id, node);
            }
        }
        // Folders set to not sync are kept so their own node is known for the badges.
        for id in mappings
            .iter()
            .filter(|m| m.local.is_none())
            .filter_map(|m| m.remote)
        {
            if let Some(node) = self.nodes.get(&id) {
                nodes.entry(id).or_insert_with(|| node.clone());
            }
        }
        self.nodes = nodes;
        self.cursor = Some(cursor);
        Ok(Delta {
            replace: true,
            upserted: self.nodes.values().cloned().collect(),
            removed: Vec::new(),
        })
    }

    /// Adds the mapped node `id` and what is under it, not descending into other `mapped` folders.
    /// A node gone from the server adds nothing.
    pub async fn read_mapped(
        &mut self,
        client: &Client,
        id: NodeId,
        mapped: &HashSet<Option<NodeId>>,
    ) -> Result<Delta, ApiError> {
        let mut delta = Delta::default();
        let node = match client.get(id).await {
            Ok(node) => RemoteNode::from(node),
            Err(ApiError::Api {
                code: ErrorCode::NotFound,
                ..
            }) => return Ok(delta),
            Err(err) => return Err(err),
        };
        let is_folder = node.kind == NodeKind::Folder;
        delta.upserted.push(node);
        if is_folder {
            delta.upserted.extend(walk(client, Some(id), mapped).await?);
        }
        self.nodes
            .extend(delta.upserted.iter().map(|node| (node.id, node.clone())));
        Ok(delta)
    }

    async fn catch_up(&mut self, client: &Client, mut cursor: i64) -> Result<Delta, ApiError> {
        let mut delta = Delta::default();
        loop {
            let page = client.changes(Some(cursor)).await?;
            for change in page.changes {
                match change {
                    Change::Upserted { node, .. } => {
                        let node = RemoteNode::from(node);
                        let previous = self.nodes.insert(node.id, node.clone());
                        let arrived =
                            previous.is_none_or(|previous| previous.parent != node.parent);
                        delta.upserted.push(node.clone());
                        // A folder moved in brings content the journal does not list again.
                        if node.kind == NodeKind::Folder && arrived {
                            for inner in walk(client, Some(node.id), &HashSet::new()).await? {
                                self.nodes.insert(inner.id, inner.clone());
                                delta.upserted.push(inner);
                            }
                        }
                    }
                    Change::Deleted { id, .. } => delta.removed.extend(self.remove(id)),
                }
            }
            cursor = page.cursor;
            if !page.more {
                break;
            }
        }
        self.cursor = Some(cursor);
        Ok(delta)
    }

    /// Whether `event` tells nothing new: the node is already held as it is now, as with the
    /// engine's own changes coming back, or the event is about the recycle bin or devices,
    /// which sync does not follow.
    pub fn already_has(&self, event: &Event) -> bool {
        match event {
            Event::NodeCreated { node } | Event::NodeUpdated { node } => self
                .nodes
                .get(&node.id)
                .is_some_and(|held| *held == RemoteNode::from(node.clone())),
            Event::NodeDeleted { id, .. } => !self.nodes.contains_key(id),
            Event::TrashChanged
            | Event::DevicesChanged
            | Event::TransferReceived { .. }
            | Event::TransferChanged { .. } => true,
            Event::Resync => false,
        }
    }

    /// Removes `id` and everything under it; returns what was removed.
    fn remove(&mut self, id: NodeId) -> Vec<NodeId> {
        let mut removed = Vec::new();
        let mut pending = vec![id];
        while let Some(id) = pending.pop() {
            let Some(node) = self.nodes.remove(&id) else {
                continue;
            };
            removed.push(id);
            if node.kind != NodeKind::Folder {
                continue;
            }
            pending.extend(
                self.nodes
                    .values()
                    .filter(|node| node.parent == Some(id))
                    .map(|node| node.id),
            );
        }
        removed
    }
}

/// Everything under `top`, not descending into the `mapped` folders, which are read on
/// their own (or not at all).
async fn walk(
    client: &Client,
    top: Option<NodeId>,
    mapped: &HashSet<Option<NodeId>>,
) -> Result<Vec<RemoteNode>, ApiError> {
    let mut found = Vec::new();
    let mut pending = vec![top];
    while let Some(parent) = pending.pop() {
        for node in client.list(parent).await? {
            let node = RemoteNode::from(node);
            if node.kind == NodeKind::Folder && !mapped.contains(&Some(node.id)) {
                pending.push(Some(node.id));
            }
            found.push(node);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;
    use uuid::Uuid;

    use protocol::Node;

    use super::*;

    fn file(name: &str) -> Node {
        Node {
            id: NodeId::from(Uuid::from_u128(1)),
            parent_id: None,
            kind: NodeKind::File,
            name: name.to_owned(),
            size: 3,
            sha256: Some("abc".to_owned()),
            mime: "text/plain".to_owned(),
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    fn holding(node: &Node) -> Mirror {
        Mirror {
            nodes: HashMap::from([(node.id, RemoteNode::from(node.clone()))]),
            cursor: Some(1),
        }
    }

    #[test]
    fn a_change_already_applied_is_not_news() {
        let node = file("a.txt");

        assert!(holding(&node).already_has(&Event::NodeUpdated { node }));
    }

    #[test]
    fn a_change_to_a_held_node_is_news() {
        let mirror = holding(&file("a.txt"));

        assert!(!mirror.already_has(&Event::NodeUpdated {
            node: file("b.txt")
        }));
    }

    #[test]
    fn deleting_a_node_no_longer_held_is_not_news() {
        let node = file("a.txt");
        let mirror = Mirror::default();

        assert!(mirror.already_has(&Event::NodeDeleted {
            id: node.id,
            parent_id: None,
        }));
        assert!(!holding(&node).already_has(&Event::NodeDeleted {
            id: node.id,
            parent_id: None,
        }));
    }

    #[test]
    fn a_resync_is_always_news() {
        assert!(!Mirror::default().already_has(&Event::Resync));
    }
}
