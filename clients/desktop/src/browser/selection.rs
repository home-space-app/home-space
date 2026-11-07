use std::collections::HashSet;

use iced::keyboard::Modifiers;

pub use client_core::selection::Coverage;
use client_core::selection::coverage;
use protocol::{Node, NodeId};

/// The selected rows of a listing, with the anchor that Shift+click extends from.
#[derive(Debug, Default)]
pub struct Selection {
    ids: HashSet<NodeId>,
    anchor: Option<NodeId>,
}

impl Selection {
    /// Applies a row click: plain click selects only `id`, Ctrl (Cmd on macOS) toggles it,
    /// Shift selects the range from the anchor to `id`.
    pub fn click(&mut self, id: NodeId, modifiers: Modifiers, nodes: &[Node]) {
        if modifiers.shift()
            && let Some(range) = self.anchor.and_then(|anchor| range_of(nodes, anchor, id))
        {
            if !modifiers.command() {
                self.ids.clear();
            }
            self.ids.extend(range);
        } else if modifiers.command() {
            self.toggle(id);
        } else {
            self.ids = HashSet::from([id]);
            self.anchor = Some(id);
        }
    }

    /// Adds `id` if absent, removes it otherwise, and moves the anchor to it.
    pub fn toggle(&mut self, id: NodeId) {
        if !self.ids.remove(&id) {
            self.ids.insert(id);
        }
        self.anchor = Some(id);
    }

    /// Makes `id` part of the selection; selects only it when it wasn't selected.
    pub fn ensure(&mut self, id: NodeId) {
        if !self.ids.contains(&id) {
            self.ids = HashSet::from([id]);
            self.anchor = Some(id);
        }
    }

    /// Moves a single selection to the next or previous row and returns that row's index. Does
    /// nothing at either end of the listing, or unless exactly one row is selected.
    pub fn step(&mut self, forward: bool, nodes: &[Node]) -> Option<usize> {
        if self.ids.len() != 1 {
            return None;
        }
        let index = nodes.iter().position(|node| self.ids.contains(&node.id))?;
        let next = if forward {
            index + 1
        } else {
            index.checked_sub(1)?
        };
        let id = nodes.get(next)?.id;
        self.ids = HashSet::from([id]);
        self.anchor = Some(id);
        Some(next)
    }

    /// Selects every node of the listing.
    pub fn select_all(&mut self, nodes: &[Node]) {
        self.ids = nodes.iter().map(|node| node.id).collect();
    }

    /// Selects every node when not all are selected, otherwise clears.
    pub fn toggle_all(&mut self, nodes: &[Node]) {
        if self.coverage(nodes) == Coverage::All {
            self.clear();
        } else {
            self.select_all(nodes);
        }
    }

    /// Deselects everything.
    pub fn clear(&mut self) {
        self.ids.clear();
        self.anchor = None;
    }

    /// Drops ids that are no longer listed.
    pub fn retain(&mut self, nodes: &[Node]) {
        self.ids
            .retain(|id| nodes.iter().any(|node| node.id == *id));
        if self
            .anchor
            .is_some_and(|anchor| !self.ids.contains(&anchor))
        {
            self.anchor = None;
        }
    }

    /// Whether `id` is selected.
    pub fn contains(&self, id: NodeId) -> bool {
        self.ids.contains(&id)
    }

    /// The selected nodes in listing order.
    pub fn selected<'a>(&self, nodes: &'a [Node]) -> Vec<&'a Node> {
        nodes
            .iter()
            .filter(|node| self.ids.contains(&node.id))
            .collect()
    }

    /// How much of `nodes` is selected.
    pub fn coverage(&self, nodes: &[Node]) -> Coverage {
        coverage(nodes.iter().map(|node| node.id), |id| {
            self.ids.contains(&id)
        })
    }
}

fn range_of(nodes: &[Node], from: NodeId, to: NodeId) -> Option<impl Iterator<Item = NodeId>> {
    let start = nodes.iter().position(|node| node.id == from)?;
    let end = nodes.iter().position(|node| node.id == to)?;
    let (low, high) = (start.min(end), start.max(end));
    Some(nodes[low..=high].iter().map(|node| node.id))
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;
    use uuid::Uuid;

    use protocol::NodeKind;

    use super::*;

    fn id(raw: u128) -> NodeId {
        NodeId(Uuid::from_u128(raw))
    }

    fn nodes(count: u128) -> Vec<Node> {
        (1..=count)
            .map(|raw| Node {
                id: id(raw),
                parent_id: None,
                kind: NodeKind::File,
                name: format!("file-{raw}"),
                size: 0,
                sha256: None,
                mime: String::new(),
                version: 1,
                created_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: OffsetDateTime::UNIX_EPOCH,
                edited_on: None,
            })
            .collect()
    }

    fn selected_ids(selection: &Selection, nodes: &[Node]) -> Vec<NodeId> {
        selection
            .selected(nodes)
            .into_iter()
            .map(|node| node.id)
            .collect()
    }

    #[test]
    fn plain_click_selects_only_that_row() {
        let nodes = nodes(3);
        let mut selection = Selection::default();
        selection.click(id(1), Modifiers::empty(), &nodes);

        selection.click(id(2), Modifiers::empty(), &nodes);

        assert_eq!(selected_ids(&selection, &nodes), [id(2)]);
    }

    #[test]
    fn ctrl_click_toggles_a_row() {
        let nodes = nodes(3);
        let mut selection = Selection::default();
        selection.click(id(1), Modifiers::empty(), &nodes);

        selection.click(id(3), Modifiers::COMMAND, &nodes);
        selection.click(id(1), Modifiers::COMMAND, &nodes);

        assert_eq!(selected_ids(&selection, &nodes), [id(3)]);
    }

    #[test]
    fn shift_click_selects_a_range_downwards() {
        let nodes = nodes(5);
        let mut selection = Selection::default();
        selection.click(id(2), Modifiers::empty(), &nodes);

        selection.click(id(4), Modifiers::SHIFT, &nodes);

        assert_eq!(selected_ids(&selection, &nodes), [id(2), id(3), id(4)]);
    }

    #[test]
    fn shift_click_selects_a_range_upwards() {
        let nodes = nodes(5);
        let mut selection = Selection::default();
        selection.click(id(4), Modifiers::empty(), &nodes);

        selection.click(id(1), Modifiers::SHIFT, &nodes);

        assert_eq!(
            selected_ids(&selection, &nodes),
            [id(1), id(2), id(3), id(4)]
        );
    }

    #[test]
    fn shift_click_without_an_anchor_selects_the_row() {
        let nodes = nodes(3);
        let mut selection = Selection::default();

        selection.click(id(2), Modifiers::SHIFT, &nodes);

        assert_eq!(selected_ids(&selection, &nodes), [id(2)]);
    }

    #[test]
    fn step_moves_a_single_selection() {
        let nodes = nodes(3);
        let mut selection = Selection::default();
        selection.click(id(2), Modifiers::empty(), &nodes);

        assert_eq!(selection.step(true, &nodes), Some(2));
        assert_eq!(selected_ids(&selection, &nodes), [id(3)]);
        assert_eq!(selection.step(false, &nodes), Some(1));
        assert_eq!(selected_ids(&selection, &nodes), [id(2)]);
    }

    #[test]
    fn step_stops_at_both_ends() {
        let nodes = nodes(2);
        let mut selection = Selection::default();
        selection.click(id(1), Modifiers::empty(), &nodes);
        assert_eq!(selection.step(false, &nodes), None);
        assert_eq!(selected_ids(&selection, &nodes), [id(1)]);

        selection.click(id(2), Modifiers::empty(), &nodes);
        assert_eq!(selection.step(true, &nodes), None);
        assert_eq!(selected_ids(&selection, &nodes), [id(2)]);
    }

    #[test]
    fn step_needs_exactly_one_selected_row() {
        let nodes = nodes(3);
        let mut selection = Selection::default();
        assert_eq!(selection.step(true, &nodes), None);

        selection.click(id(1), Modifiers::empty(), &nodes);
        selection.click(id(2), Modifiers::COMMAND, &nodes);
        assert_eq!(selection.step(true, &nodes), None);
        assert_eq!(selected_ids(&selection, &nodes), [id(1), id(2)]);
    }

    #[test]
    fn step_extends_from_the_row_it_moved_to() {
        let nodes = nodes(4);
        let mut selection = Selection::default();
        selection.click(id(1), Modifiers::empty(), &nodes);
        selection.step(true, &nodes);

        selection.click(id(4), Modifiers::SHIFT, &nodes);

        assert_eq!(selected_ids(&selection, &nodes), [id(2), id(3), id(4)]);
    }

    #[test]
    fn toggle_all_selects_then_clears() {
        let nodes = nodes(3);
        let mut selection = Selection::default();

        selection.toggle_all(&nodes);
        let all = selection.coverage(&nodes);
        selection.toggle_all(&nodes);

        assert_eq!(
            (all, selection.coverage(&nodes)),
            (Coverage::All, Coverage::None)
        );
    }

    #[test]
    fn retain_drops_rows_that_disappeared() {
        let mut listed = nodes(3);
        let mut selection = Selection::default();
        selection.select_all(&listed);

        listed.remove(1);
        selection.retain(&listed);

        assert_eq!(selection.selected(&listed).len(), 2);
        assert!(!selection.contains(id(2)));
    }

    #[test]
    fn ensure_keeps_an_existing_multi_selection() {
        let nodes = nodes(3);
        let mut selection = Selection::default();
        selection.select_all(&nodes);

        selection.ensure(id(2));

        assert_eq!(selection.selected(&nodes).len(), 3);
    }
}
