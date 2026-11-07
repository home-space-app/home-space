use std::collections::HashSet;

use iced::keyboard::Modifiers;

pub use client_core::selection::Coverage;
use client_core::selection::coverage;
use protocol::{Node, NodeId};

/// The selected rows of a listing, with the anchor that Shift+click and Shift+Arrow extend
/// from and the cursor, the end that Shift+Arrow moves.
#[derive(Debug, Default)]
pub struct Selection {
    ids: HashSet<NodeId>,
    anchor: Option<NodeId>,
    cursor: Option<NodeId>,
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
            self.cursor = Some(id);
        } else if modifiers.command() {
            self.toggle(id);
        } else {
            self.only(id);
        }
    }

    /// Selects only `id`, which becomes both the anchor and the cursor.
    fn only(&mut self, id: NodeId) {
        self.ids = HashSet::from([id]);
        self.anchor = Some(id);
        self.cursor = Some(id);
    }

    /// Adds `id` if absent, removes it otherwise, and moves the anchor to it.
    pub fn toggle(&mut self, id: NodeId) {
        if !self.ids.remove(&id) {
            self.ids.insert(id);
        }
        self.anchor = Some(id);
        self.cursor = Some(id);
    }

    /// Makes `id` part of the selection; selects only it when it wasn't selected.
    pub fn ensure(&mut self, id: NodeId) {
        if !self.ids.contains(&id) {
            self.only(id);
        }
    }

    /// Moves the selection to the row after or before the cursor, selecting only that row, and
    /// returns its index. Without a cursor, a single selected row stands in for it. Does nothing
    /// at either end of the listing or when there is nothing to move from.
    pub fn step(&mut self, forward: bool, nodes: &[Node]) -> Option<usize> {
        let index = self.cursor_index(nodes)?;
        let next = neighbour(index, forward, nodes.len())?;
        self.only(nodes[next].id);
        Some(next)
    }

    /// Moves the cursor one row the way Finder's Shift+Arrow does: away from the anchor the
    /// next row is selected, towards it the row left behind is deselected, so the anchor itself
    /// stays selected. With nothing selected it selects the first or last row. Returns the index
    /// of the row the cursor moved to.
    pub fn extend(&mut self, forward: bool, nodes: &[Node]) -> Option<usize> {
        let Some(cursor) = self.cursor_index(nodes) else {
            let index = if forward {
                0
            } else {
                nodes.len().checked_sub(1)?
            };
            self.only(nodes.get(index)?.id);
            return Some(index);
        };
        let anchor = self
            .anchor
            .and_then(|anchor| position(nodes, anchor))
            .unwrap_or(cursor);
        let next = neighbour(cursor, forward, nodes.len())?;
        let towards_anchor = if forward {
            cursor < anchor
        } else {
            cursor > anchor
        };
        if towards_anchor {
            self.ids.remove(&nodes[cursor].id);
        } else {
            self.ids.insert(nodes[next].id);
        }
        self.anchor = Some(nodes[anchor].id);
        self.cursor = Some(nodes[next].id);
        Some(next)
    }

    /// Where the cursor is in `nodes`, or the single selected row when there is no cursor.
    fn cursor_index(&self, nodes: &[Node]) -> Option<usize> {
        self.cursor
            .filter(|cursor| self.ids.contains(cursor))
            .and_then(|cursor| position(nodes, cursor))
            .or_else(|| match self.ids.len() {
                1 => nodes.iter().position(|node| self.ids.contains(&node.id)),
                _ => None,
            })
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
        self.cursor = None;
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
        if self
            .cursor
            .is_some_and(|cursor| !self.ids.contains(&cursor))
        {
            self.cursor = None;
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

fn position(nodes: &[Node], id: NodeId) -> Option<usize> {
    nodes.iter().position(|node| node.id == id)
}

/// The index one row after or before `index` in a listing of `len` rows.
fn neighbour(index: usize, forward: bool, len: usize) -> Option<usize> {
    if forward {
        Some(index + 1).filter(|next| *next < len)
    } else {
        index.checked_sub(1)
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
    fn step_needs_something_to_move_from() {
        let nodes = nodes(3);
        let mut selection = Selection::default();
        assert_eq!(selection.step(true, &nodes), None);

        selection.select_all(&nodes);
        assert_eq!(selection.step(true, &nodes), None);
        assert_eq!(selection.selected(&nodes).len(), 3);
    }

    #[test]
    fn step_collapses_a_multi_selection_from_the_cursor() {
        let nodes = nodes(5);
        let mut selection = Selection::default();
        selection.click(id(2), Modifiers::empty(), &nodes);
        selection.extend(true, &nodes);
        selection.extend(true, &nodes);

        assert_eq!(selection.step(true, &nodes), Some(4));
        assert_eq!(selected_ids(&selection, &nodes), [id(5)]);
    }

    #[test]
    fn shift_arrows_grow_and_shrink_without_dropping_the_anchor() {
        let nodes = nodes(5);
        let mut selection = Selection::default();
        selection.click(id(2), Modifiers::empty(), &nodes);

        assert_eq!(selection.extend(true, &nodes), Some(2));
        assert_eq!(selection.extend(true, &nodes), Some(3));
        assert_eq!(selected_ids(&selection, &nodes), [id(2), id(3), id(4)]);

        selection.extend(false, &nodes);
        assert_eq!(selected_ids(&selection, &nodes), [id(2), id(3)]);
        selection.extend(false, &nodes);
        assert_eq!(selected_ids(&selection, &nodes), [id(2)]);
    }

    #[test]
    fn shift_arrows_cross_the_anchor() {
        let nodes = nodes(5);
        let mut selection = Selection::default();
        selection.click(id(3), Modifiers::empty(), &nodes);

        selection.extend(false, &nodes);
        selection.extend(false, &nodes);
        assert_eq!(selected_ids(&selection, &nodes), [id(1), id(2), id(3)]);

        selection.extend(true, &nodes);
        selection.extend(true, &nodes);
        selection.extend(true, &nodes);
        assert_eq!(selected_ids(&selection, &nodes), [id(3), id(4)]);
    }

    #[test]
    fn shift_arrows_stop_at_both_ends() {
        let nodes = nodes(2);
        let mut selection = Selection::default();
        selection.click(id(1), Modifiers::empty(), &nodes);
        assert_eq!(selection.extend(false, &nodes), None);

        selection.extend(true, &nodes);
        assert_eq!(selection.extend(true, &nodes), None);
        assert_eq!(selected_ids(&selection, &nodes), [id(1), id(2)]);
    }

    #[test]
    fn shift_arrows_without_a_selection_pick_an_end() {
        let nodes = nodes(3);
        let mut selection = Selection::default();
        assert_eq!(selection.extend(true, &nodes), Some(0));
        assert_eq!(selected_ids(&selection, &nodes), [id(1)]);

        selection.clear();
        assert_eq!(selection.extend(false, &nodes), Some(2));
        assert_eq!(selected_ids(&selection, &nodes), [id(3)]);
        assert_eq!(Selection::default().extend(true, &[]), None);
    }

    #[test]
    fn shift_arrows_continue_from_a_shift_click() {
        let nodes = nodes(5);
        let mut selection = Selection::default();
        selection.click(id(2), Modifiers::empty(), &nodes);
        selection.click(id(4), Modifiers::SHIFT, &nodes);

        selection.extend(false, &nodes);

        assert_eq!(selected_ids(&selection, &nodes), [id(2), id(3)]);
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
