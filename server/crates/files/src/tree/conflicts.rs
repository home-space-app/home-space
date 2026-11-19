//! Finding the files that would clash when nodes are moved into a folder, before anything
//! moves.
//!
//! Folders merge into same-named folders, also into one an earlier node moves there, so only
//! the files inside them can clash. [`ConflictPlanner`] holds that rule without touching the
//! database; [`plan_conflicts`] feeds it what is at each destination.

use std::collections::{HashMap, VecDeque};

use db::{DbError, Node, NodeId, NodeKind, Tx};

use super::MoveConflict;

/// A node on its way into folder `dest` (the root when `None`).
#[derive(Clone, Debug)]
pub(crate) struct Arrival {
    pub(crate) node: Node,
    pub(crate) dest: Option<NodeId>,
    /// Where the node is, relative to what is moved, e.g. `Photos/2024/a.jpg`.
    pub(crate) path: String,
}

/// A folder that merges into another one, so its children arrive there in turn.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Merge {
    pub(crate) folder: Node,
    pub(crate) into: NodeId,
    pub(crate) path: String,
}

impl Arrival {
    fn merge_into(self, into: NodeId) -> Merge {
        Merge {
            folder: self.node,
            into,
            path: self.path,
        }
    }

    fn conflict(self, existing: Option<Node>) -> MoveConflict {
        MoveConflict {
            source: self.node,
            path: self.path,
            existing,
        }
    }
}

/// An arrival that took a free name at its destination first.
struct Claim {
    arrival: Arrival,
    /// Whether a later arrival of the same name already reported it.
    reported: bool,
}

/// Collects the clashes of a move, one arrival at a time.
#[derive(Default)]
pub(crate) struct ConflictPlanner {
    claims: HashMap<(Option<NodeId>, NodeKind, String), Claim>,
    conflicts: Vec<MoveConflict>,
}

impl ConflictPlanner {
    /// Settles `arrival`, given the live node of its kind and name already at its destination.
    /// Returns the merge whose children arrive next, when the arrival is a folder that merges.
    pub(crate) fn settle(&mut self, arrival: Arrival, existing: Option<Node>) -> Option<Merge> {
        match (arrival.node.kind, existing) {
            (NodeKind::File, Some(existing)) => {
                self.conflicts.push(arrival.conflict(Some(existing)));
                None
            }
            (NodeKind::Folder, Some(existing)) => Some(arrival.merge_into(existing.id)),
            (_, None) => self.claim(arrival),
        }
    }

    /// Arrivals of the same kind and name meet at a free name: the first takes it, a later
    /// folder merges into it, and later files clash with it, the first one included.
    fn claim(&mut self, arrival: Arrival) -> Option<Merge> {
        let key = (arrival.dest, arrival.node.kind, arrival.node.name.clone());
        let Some(first) = self.claims.get_mut(&key) else {
            let claim = Claim {
                arrival,
                reported: false,
            };
            self.claims.insert(key, claim);
            return None;
        };
        if arrival.node.kind == NodeKind::Folder {
            return Some(arrival.merge_into(first.arrival.node.id));
        }
        if !first.reported {
            first.reported = true;
            self.conflicts.push(first.arrival.clone().conflict(None));
        }
        self.conflicts.push(arrival.conflict(None));
        None
    }

    /// The clashes found, ordered by path.
    pub(crate) fn finish(mut self) -> Vec<MoveConflict> {
        self.conflicts.sort_by(|a, b| a.path.cmp(&b.path));
        self.conflicts
    }
}

/// Finds the files of `sources` that would clash in `parent`, changing nothing.
pub(crate) async fn plan_conflicts(
    tx: &mut Tx,
    sources: &[Node],
    parent: Option<NodeId>,
) -> Result<Vec<MoveConflict>, DbError> {
    let mut planner = ConflictPlanner::default();
    let mut arrivals = sources
        .iter()
        .map(|node| Arrival {
            node: node.clone(),
            dest: parent,
            path: node.name.clone(),
        })
        .collect::<VecDeque<_>>();
    while let Some(arrival) = arrivals.pop_front() {
        let node = &arrival.node;
        let existing = tx
            .find_sibling(arrival.dest, node.kind, &node.name, Some(node.id))
            .await?;
        let Some(merge) = planner.settle(arrival, existing) else {
            continue;
        };
        for child in tx.lock_children(merge.folder.id).await? {
            arrivals.push_back(Arrival {
                path: format!("{}/{}", merge.path, child.name),
                dest: Some(merge.into),
                node: child,
            });
        }
    }
    Ok(planner.finish())
}

#[cfg(test)]
mod tests {
    use db::UserId;
    use time::OffsetDateTime;

    use super::*;

    fn node(kind: NodeKind, name: &str) -> Node {
        Node {
            id: NodeId::new(),
            owner_id: UserId(uuid::Uuid::nil()),
            parent_id: None,
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

    fn arrival(node: &Node, dest: Option<NodeId>, path: &str) -> Arrival {
        Arrival {
            node: node.clone(),
            dest,
            path: path.to_owned(),
        }
    }

    fn sources(conflicts: &[MoveConflict]) -> Vec<(NodeId, &str, Option<NodeId>)> {
        conflicts
            .iter()
            .map(|c| {
                let existing = c.existing.as_ref().map(|node| node.id);
                (c.source.id, c.path.as_str(), existing)
            })
            .collect()
    }

    #[test]
    fn a_file_onto_a_live_file_clashes_with_it() {
        let (moving, there) = (node(NodeKind::File, "a"), node(NodeKind::File, "a"));
        let mut planner = ConflictPlanner::default();
        let merge = planner.settle(arrival(&moving, None, "a"), Some(there.clone()));
        assert_eq!(merge, None);
        assert_eq!(
            sources(&planner.finish()),
            [(moving.id, "a", Some(there.id))]
        );
    }

    #[test]
    fn a_folder_onto_a_live_folder_merges_into_it() {
        let (moving, there) = (node(NodeKind::Folder, "d"), node(NodeKind::Folder, "d"));
        let mut planner = ConflictPlanner::default();
        let merge = planner.settle(arrival(&moving, None, "d"), Some(there.clone()));
        assert_eq!(
            merge.map(|m| (m.folder.id, m.into)),
            Some((moving.id, there.id))
        );
        assert!(planner.finish().is_empty());
    }

    #[test]
    fn nodes_at_free_names_move_without_clashing() {
        let mut planner = ConflictPlanner::default();
        for (kind, name) in [
            (NodeKind::File, "a"),
            (NodeKind::Folder, "a"),
            (NodeKind::File, "b"),
        ] {
            let moving = node(kind, name);
            assert_eq!(planner.settle(arrival(&moving, None, name), None), None);
        }
        assert!(planner.finish().is_empty());
    }

    #[test]
    fn files_meeting_at_a_free_name_all_clash_and_the_first_is_reported_once() {
        let files = [1, 2, 3].map(|_| node(NodeKind::File, "same"));
        let mut planner = ConflictPlanner::default();
        for (n, file) in files.iter().enumerate() {
            planner.settle(arrival(file, None, &format!("{n}/same")), None);
        }
        assert_eq!(
            sources(&planner.finish()),
            [
                (files[0].id, "0/same", None),
                (files[1].id, "1/same", None),
                (files[2].id, "2/same", None),
            ]
        );
    }

    #[test]
    fn a_folder_meeting_an_earlier_one_merges_into_it() {
        let (first, second) = (node(NodeKind::Folder, "d"), node(NodeKind::Folder, "d"));
        let mut planner = ConflictPlanner::default();
        assert_eq!(planner.settle(arrival(&first, None, "d"), None), None);
        let merge = planner.settle(arrival(&second, None, "d"), None);
        assert_eq!(merge.map(|m| m.into), Some(first.id));
    }

    #[test]
    fn the_same_name_in_different_folders_does_not_clash() {
        let (a, b) = (node(NodeKind::File, "x"), node(NodeKind::File, "x"));
        let mut planner = ConflictPlanner::default();
        planner.settle(arrival(&a, Some(NodeId::new()), "x"), None);
        planner.settle(arrival(&b, Some(NodeId::new()), "x"), None);
        assert!(planner.finish().is_empty());
    }

    #[test]
    fn conflicts_are_ordered_by_path() {
        let mut planner = ConflictPlanner::default();
        for path in ["b/z", "a/z"] {
            let file = node(NodeKind::File, "z");
            planner.settle(arrival(&file, None, path), Some(node(NodeKind::File, "z")));
        }
        let paths = planner
            .finish()
            .into_iter()
            .map(|c| c.path)
            .collect::<Vec<_>>();
        assert_eq!(paths, ["a/z", "b/z"]);
    }
}
