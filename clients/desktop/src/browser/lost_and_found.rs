//! What Lost & Found holds, which decides whether the sidebar offers it.

use std::collections::HashSet;

use protocol::{Event, Node, NodeId};

/// The direct children of Lost & Found, kept current from the server events.
#[derive(Debug)]
pub struct LostAndFound {
    id: NodeId,
    children: HashSet<NodeId>,
}

impl LostAndFound {
    pub fn new(id: NodeId) -> Self {
        Self {
            id,
            children: HashSet::new(),
        }
    }

    pub fn id(&self) -> NodeId {
        self.id
    }

    /// Whether it holds anything: only then does the sidebar show it.
    pub fn has_files(&self) -> bool {
        !self.children.is_empty()
    }

    /// Takes in a fetched listing of Lost & Found.
    pub fn loaded(&mut self, children: &[Node]) {
        self.children = children.iter().map(|node| node.id).collect();
    }

    /// Applies a server event; returns whether the listing has to be fetched again.
    pub fn apply(&mut self, event: &Event) -> bool {
        match event {
            Event::NodeCreated { node } | Event::NodeUpdated { node } => {
                if node.parent_id == Some(self.id) {
                    self.children.insert(node.id);
                } else {
                    self.children.remove(&node.id);
                }
                false
            }
            Event::NodeDeleted { id, .. } => {
                self.children.remove(id);
                false
            }
            Event::Resync => true,
            Event::TrashChanged
            | Event::DevicesChanged
            | Event::TransferReceived { .. }
            | Event::TransferChanged { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use protocol::NodeKind;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    const FOUND: NodeId = NodeId(Uuid::from_u128(9));

    fn node(raw: u128, parent_id: Option<NodeId>) -> Node {
        Node {
            id: NodeId(Uuid::from_u128(raw)),
            parent_id,
            kind: NodeKind::File,
            name: "restored.txt".to_owned(),
            size: 0,
            sha256: None,
            mime: String::new(),
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    #[test]
    fn a_fetched_listing_decides_whether_it_has_files() {
        let mut found = LostAndFound::new(FOUND);
        assert!(!found.has_files());

        found.loaded(&[node(1, Some(FOUND))]);
        assert!(found.has_files());

        found.loaded(&[]);
        assert!(!found.has_files());
    }

    #[test]
    fn a_restored_file_gives_it_files() {
        let mut found = LostAndFound::new(FOUND);
        let restored = Event::NodeUpdated {
            node: node(1, Some(FOUND)),
        };

        assert!(!found.apply(&restored));

        assert!(found.has_files());
    }

    #[test]
    fn files_elsewhere_do_not_count() {
        let mut found = LostAndFound::new(FOUND);

        found.apply(&Event::NodeCreated {
            node: node(1, None),
        });

        assert!(!found.has_files());
    }

    #[test]
    fn moving_the_last_file_out_empties_it() {
        let mut found = LostAndFound::new(FOUND);
        found.loaded(&[node(1, Some(FOUND))]);

        found.apply(&Event::NodeUpdated {
            node: node(1, None),
        });

        assert!(!found.has_files());
    }

    #[test]
    fn it_stays_while_a_file_remains() {
        let mut found = LostAndFound::new(FOUND);
        found.loaded(&[node(1, Some(FOUND)), node(2, Some(FOUND))]);

        found.apply(&Event::NodeDeleted {
            id: NodeId(Uuid::from_u128(1)),
            parent_id: Some(FOUND),
        });
        assert!(found.has_files());

        found.apply(&Event::NodeDeleted {
            id: NodeId(Uuid::from_u128(2)),
            parent_id: Some(FOUND),
        });
        assert!(!found.has_files());
    }

    #[test]
    fn only_a_resync_needs_a_fetch() {
        let mut found = LostAndFound::new(FOUND);
        assert!(found.apply(&Event::Resync));
        assert!(!found.apply(&Event::TrashChanged));
    }
}
