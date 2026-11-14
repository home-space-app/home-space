//! Stepping through the files of a folder that open in the same built-in viewer, such as the
//! pictures of a folder in the image viewer.

use protocol::{Event, Node, NodeId};

use crate::{
    listing::{Sort, apply_event, sort_nodes},
    viewers::{ViewerKind, Viewers},
};

/// The files of one folder that open in the same viewer as the file shown, in listing order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gallery {
    kind: ViewerKind,
    folder: Option<NodeId>,
    sort: Sort,
    nodes: Vec<Node>,
    current: NodeId,
}

impl Gallery {
    /// The gallery of `current` within `listing`, the contents of its folder.
    ///
    /// Returns `None` when `current` has no viewer, or its viewer does not step between files.
    /// The listing is sorted with `sort` and keeps the files `viewers` shows in the same kind
    /// of viewer as `current`.
    pub fn new(viewers: &Viewers, listing: &[Node], current: &Node, sort: Sort) -> Option<Self> {
        let kind = viewers
            .viewer_for_node(current)
            .map(|viewer| viewer.kind)
            .filter(|kind| kind.navigates())?;
        let mut nodes = same_kind(viewers, kind, listing.iter().cloned());
        sort_nodes(&mut nodes, sort);
        Some(Self {
            kind,
            folder: current.parent_id,
            sort,
            nodes,
            current: current.id,
        })
    }

    /// The file shown.
    pub fn current(&self) -> NodeId {
        self.current
    }

    /// The file before the one shown; none at the first file.
    pub fn previous(&self) -> Option<&Node> {
        self.index()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| self.nodes.get(index))
    }

    /// The file after the one shown; none at the last file.
    pub fn next(&self) -> Option<&Node> {
        self.index()
            .and_then(|index| index.checked_add(1))
            .and_then(|index| self.nodes.get(index))
    }

    /// Where the file shown is, counting from 1, and how many files there are. `None` while
    /// the file shown is not in the gallery.
    pub fn position(&self) -> Option<(usize, usize)> {
        self.index().map(|index| (index + 1, self.nodes.len()))
    }

    /// The files to step through, in order.
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// Shows file `id` instead.
    pub fn move_to(&mut self, id: NodeId) {
        self.current = id;
    }

    /// Applies a live event to the files of the folder.
    pub fn apply_event(&mut self, viewers: &Viewers, event: &Event) {
        apply_event(&mut self.nodes, self.folder, event, self.sort);
        self.nodes = same_kind(viewers, self.kind, self.nodes.drain(..));
    }

    fn index(&self) -> Option<usize> {
        self.nodes.iter().position(|node| node.id == self.current)
    }
}

fn same_kind(
    viewers: &Viewers,
    kind: ViewerKind,
    nodes: impl IntoIterator<Item = Node>,
) -> Vec<Node> {
    nodes
        .into_iter()
        .filter(|node| {
            viewers
                .viewer_for_node(node)
                .is_some_and(|viewer| viewer.kind == kind)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use protocol::NodeKind;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    const FOLDER: u128 = 100;

    fn node(id: u128, name: &str, kind: NodeKind, mime: &str) -> Node {
        Node {
            id: NodeId::from(Uuid::from_u128(id)),
            parent_id: Some(NodeId::from(Uuid::from_u128(FOLDER))),
            kind,
            name: name.to_owned(),
            size: 10,
            sha256: None,
            mime: mime.to_owned(),
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    fn image(id: u128, name: &str) -> Node {
        node(id, name, NodeKind::File, "image/png")
    }

    fn listing() -> Vec<Node> {
        vec![
            image(3, "c.png"),
            node(4, "notes.txt", NodeKind::File, "text/plain"),
            image(1, "a.png"),
            node(5, "photos", NodeKind::Folder, ""),
            image(2, "b.png"),
        ]
    }

    fn gallery_at(name: &str) -> Gallery {
        let listing = listing();
        let current = listing.iter().find(|node| node.name == name).unwrap();
        Gallery::new(&Viewers::shared(), &listing, current, Sort::default()).unwrap()
    }

    fn name(node: Option<&Node>) -> Option<&str> {
        node.map(|node| node.name.as_str())
    }

    #[test]
    fn only_files_of_the_same_viewer_are_kept_in_name_order() {
        let gallery = gallery_at("b.png");
        let names: Vec<_> = gallery
            .nodes
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(names, ["a.png", "b.png", "c.png"]);
    }

    #[test]
    fn the_neighbours_are_the_previous_and_next_images() {
        let gallery = gallery_at("b.png");
        assert_eq!(name(gallery.previous()), Some("a.png"));
        assert_eq!(name(gallery.next()), Some("c.png"));
    }

    #[test]
    fn the_first_image_has_no_previous() {
        assert_eq!(gallery_at("a.png").previous(), None);
    }

    #[test]
    fn the_last_image_has_no_next() {
        assert_eq!(gallery_at("c.png").next(), None);
    }

    #[test]
    fn the_position_counts_from_one() {
        assert_eq!(gallery_at("b.png").position(), Some((2, 3)));
    }

    #[test]
    fn moving_changes_the_neighbours() {
        let mut gallery = gallery_at("a.png");
        let next = gallery.next().unwrap().id;
        gallery.move_to(next);
        assert_eq!(gallery.position(), Some((2, 3)));
    }

    #[test]
    fn a_file_missing_from_the_listing_has_no_neighbours() {
        let current = image(9, "z.png");
        let gallery =
            Gallery::new(&Viewers::shared(), &listing(), &current, Sort::default()).unwrap();
        assert_eq!(gallery.position(), None);
        assert_eq!(gallery.next(), None);
    }

    #[test]
    fn text_files_have_no_gallery() {
        let listing = listing();
        let text = &listing[1];
        assert!(Gallery::new(&Viewers::shared(), &listing, text, Sort::default()).is_none());
    }

    #[test]
    fn a_new_image_in_the_folder_joins_in_order() {
        let mut gallery = gallery_at("b.png");
        let event = Event::NodeCreated {
            node: image(6, "bb.png"),
        };
        gallery.apply_event(&Viewers::shared(), &event);
        assert_eq!(name(gallery.next()), Some("bb.png"));
    }

    #[test]
    fn a_new_text_file_does_not_join() {
        let mut gallery = gallery_at("b.png");
        let event = Event::NodeCreated {
            node: node(6, "bb.txt", NodeKind::File, "text/plain"),
        };
        gallery.apply_event(&Viewers::shared(), &event);
        assert_eq!(gallery.position(), Some((2, 3)));
    }

    #[test]
    fn a_deleted_image_leaves() {
        let mut gallery = gallery_at("b.png");
        let event = Event::NodeDeleted {
            id: NodeId::from(Uuid::from_u128(3)),
            parent_id: None,
        };
        gallery.apply_event(&Viewers::shared(), &event);
        assert_eq!(gallery.next(), None);
    }
}
