use std::ops::Range;

use protocol::{Event, Node, NodeId, NodeKind};

/// How many breadcrumb folders are shown before the middle ones collapse.
pub const MAX_VISIBLE_CRUMBS: usize = 3;

/// The column a listing is ordered by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Modified,
}

/// A column and direction to order a listing by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
}

impl Sort {
    /// The sort after a header click: flips the direction of the active column,
    /// or switches to `key` ascending.
    pub fn toggled(self, key: SortKey) -> Self {
        Self {
            key,
            descending: self.key == key && !self.descending,
        }
    }
}

/// Orders nodes folders first, then by `sort`, breaking ties by case-insensitive name.
pub fn sort_nodes(nodes: &mut [Node], sort: Sort) {
    nodes.sort_by_cached_key(|node| {
        let name = node.name.to_lowercase();
        let primary = match sort.key {
            SortKey::Name => 0,
            SortKey::Size => node.size,
            SortKey::Modified => node.updated_at.unix_timestamp(),
        };
        let primary = if sort.descending { -primary } else { primary };
        (node.kind != NodeKind::Folder, primary, name)
    });
}

/// Applies a live event to the listing of `folder`.
pub fn apply_event(nodes: &mut Vec<Node>, folder: Option<NodeId>, event: &Event, sort: Sort) {
    match event {
        Event::NodeCreated { node } | Event::NodeUpdated { node } => {
            nodes.retain(|existing| existing.id != node.id);
            if node.parent_id == folder {
                nodes.push(node.clone());
                sort_nodes(nodes, sort);
            }
        }
        Event::NodeDeleted { id, .. } => nodes.retain(|existing| existing.id != *id),
        Event::TrashChanged
        | Event::DevicesChanged
        | Event::TransferReceived { .. }
        | Event::TransferChanged { .. }
        | Event::Resync => {}
    }
}

/// Whether `event` changes a folder on the breadcrumb path, which needs a refetch.
pub fn affects_path(path: &[Node], event: &Event) -> bool {
    match event {
        Event::NodeUpdated { node } => path.iter().any(|folder| folder.id == node.id),
        Event::NodeDeleted { id, .. } => path.iter().any(|folder| folder.id == *id),
        Event::NodeCreated { .. }
        | Event::TrashChanged
        | Event::DevicesChanged
        | Event::TransferReceived { .. }
        | Event::TransferChanged { .. }
        | Event::Resync => false,
    }
}

/// Whether `event` may make Lost & Found show up in Home, which leaves it out while it is
/// empty, so Home has to be fetched again.
pub fn reveals_lost_and_found(
    folder: Option<NodeId>,
    lost_and_found: NodeId,
    event: &Event,
) -> bool {
    folder.is_none()
        && match event {
            Event::NodeCreated { node } | Event::NodeUpdated { node } => {
                node.id == lost_and_found || node.parent_id == Some(lost_and_found)
            }
            Event::NodeDeleted { .. }
            | Event::TrashChanged
            | Event::DevicesChanged
            | Event::TransferReceived { .. }
            | Event::TransferChanged { .. }
            | Event::Resync => false,
        }
}

/// Whether moving `nodes` into `destination` (`None` for Home) would leave them where they are.
pub fn already_in(nodes: &[Node], destination: Option<NodeId>) -> bool {
    nodes.iter().all(|node| node.parent_id == destination)
}

/// The indices of breadcrumb folders hidden behind an overflow menu once a path of
/// `len` folders is deeper than [`MAX_VISIBLE_CRUMBS`]; the first and last stay visible.
pub fn collapsed_crumbs(len: usize) -> Option<Range<usize>> {
    (len > MAX_VISIBLE_CRUMBS).then(|| 1..len - 1)
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    use super::*;

    fn node(name: &str, kind: NodeKind, parent: Option<NodeId>) -> Node {
        Node {
            id: NodeId::from(uuid_from(name)),
            parent_id: parent,
            kind,
            name: name.to_owned(),
            size: 0,
            sha256: None,
            mime: String::new(),
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    fn uuid_from(name: &str) -> uuid::Uuid {
        let mut bytes = [0_u8; 16];
        for (slot, byte) in bytes.iter_mut().zip(name.bytes()) {
            *slot = byte;
        }
        uuid::Uuid::from_bytes(bytes)
    }

    fn names(nodes: &[Node]) -> Vec<&str> {
        nodes.iter().map(|node| node.name.as_str()).collect()
    }

    #[test]
    fn folders_sort_before_files_and_names_ignore_case() {
        let mut nodes = vec![
            node("b.txt", NodeKind::File, None),
            node("Zeta", NodeKind::Folder, None),
            node("alpha", NodeKind::Folder, None),
            node("A.txt", NodeKind::File, None),
        ];
        sort_nodes(&mut nodes, Sort::default());
        assert_eq!(names(&nodes), ["alpha", "Zeta", "A.txt", "b.txt"]);
    }

    #[test]
    fn size_sort_keeps_folders_first_and_can_descend() {
        let mut big = node("big", NodeKind::File, None);
        big.size = 10;
        let mut small = node("small", NodeKind::File, None);
        small.size = 1;
        let mut nodes = vec![big, small, node("dir", NodeKind::Folder, None)];
        let sort = Sort::default().toggled(SortKey::Size);
        sort_nodes(&mut nodes, sort);
        assert_eq!(names(&nodes), ["dir", "small", "big"]);
        sort_nodes(&mut nodes, sort.toggled(SortKey::Size));
        assert_eq!(names(&nodes), ["dir", "big", "small"]);
    }

    #[test]
    fn modified_sort_orders_by_update_time() {
        let mut old = node("old", NodeKind::File, None);
        old.updated_at = OffsetDateTime::UNIX_EPOCH;
        let mut new = node("new", NodeKind::File, None);
        new.updated_at = OffsetDateTime::UNIX_EPOCH + time::Duration::DAY;
        let mut nodes = vec![new, old];
        sort_nodes(&mut nodes, Sort::default().toggled(SortKey::Modified));
        assert_eq!(names(&nodes), ["old", "new"]);
    }

    #[test]
    fn toggling_the_active_column_flips_direction() {
        let sort = Sort::default();
        assert!(!sort.descending);
        assert!(sort.toggled(SortKey::Name).descending);
        assert!(!sort.toggled(SortKey::Size).descending);
    }

    #[test]
    fn restoring_into_lost_and_found_reveals_it_at_home_only() {
        let found = NodeId::from(uuid_from("found"));
        let restored = Event::NodeCreated {
            node: node("a.txt", NodeKind::File, Some(found)),
        };
        assert!(reveals_lost_and_found(None, found, &restored));
        assert!(!reveals_lost_and_found(Some(found), found, &restored));
        let elsewhere = Event::NodeCreated {
            node: node("a.txt", NodeKind::File, None),
        };
        assert!(!reveals_lost_and_found(None, found, &elsewhere));
    }

    #[test]
    fn moving_into_the_current_parent_is_pointless() {
        let parent = Some(NodeId::from(uuid_from("parent")));
        let nodes = [
            node("a.txt", NodeKind::File, parent),
            node("b.txt", NodeKind::File, parent),
        ];
        assert!(already_in(&nodes, parent));
        assert!(!already_in(&nodes, None));
        assert!(!already_in(
            &[node("c", NodeKind::File, None), nodes[0].clone()],
            parent
        ));
    }

    #[test]
    fn created_node_in_the_open_folder_is_inserted_in_order() {
        let mut nodes = vec![node("b.txt", NodeKind::File, None)];
        let event = Event::NodeCreated {
            node: node("a.txt", NodeKind::File, None),
        };
        apply_event(&mut nodes, None, &event, Sort::default());
        assert_eq!(names(&nodes), ["a.txt", "b.txt"]);
    }

    #[test]
    fn created_node_elsewhere_is_ignored() {
        let mut nodes = Vec::new();
        let other = Some(NodeId::from(uuid_from("other")));
        let event = Event::NodeCreated {
            node: node("a.txt", NodeKind::File, other),
        };
        apply_event(&mut nodes, None, &event, Sort::default());
        assert!(nodes.is_empty());
    }

    #[test]
    fn node_moved_out_of_the_open_folder_disappears() {
        let mut nodes = vec![node("a.txt", NodeKind::File, None)];
        let other = Some(NodeId::from(uuid_from("other")));
        let event = Event::NodeUpdated {
            node: node("a.txt", NodeKind::File, other),
        };
        apply_event(&mut nodes, None, &event, Sort::default());
        assert!(nodes.is_empty());
    }

    #[test]
    fn renamed_node_replaces_its_old_row() {
        let mut nodes = vec![node("a.txt", NodeKind::File, None)];
        let mut renamed = node("a.txt", NodeKind::File, None);
        renamed.name = "z.txt".to_owned();
        apply_event(
            &mut nodes,
            None,
            &Event::NodeUpdated { node: renamed },
            Sort::default(),
        );
        assert_eq!(names(&nodes), ["z.txt"]);
    }

    #[test]
    fn deleted_node_is_removed() {
        let file = node("a.txt", NodeKind::File, None);
        let event = Event::NodeDeleted {
            id: file.id,
            parent_id: None,
        };
        let mut nodes = vec![file];
        apply_event(&mut nodes, None, &event, Sort::default());
        assert!(nodes.is_empty());
    }

    #[test]
    fn deleting_a_breadcrumb_folder_needs_a_refetch() {
        let folder = node("docs", NodeKind::Folder, None);
        let event = Event::NodeDeleted {
            id: folder.id,
            parent_id: None,
        };
        assert!(affects_path(&[folder], &event));
        assert!(!affects_path(&[], &event));
    }

    #[test]
    fn short_paths_are_not_collapsed() {
        assert_eq!((collapsed_crumbs(0), collapsed_crumbs(3)), (None, None));
    }

    #[test]
    fn deep_paths_hide_the_middle_folders() {
        assert_eq!(
            (collapsed_crumbs(4), collapsed_crumbs(6)),
            (Some(1..3), Some(1..5))
        );
    }
}
