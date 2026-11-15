//! Shares other users made for the signed-in user: "Shared with me". They are read-only; the
//! user opens and downloads them, or saves copies into their own files.

use time::OffsetDateTime;
use uuid::Uuid;

use protocol::{NodeId, NodeKind, ReceivedShare, SharedNode, UserRef};

use crate::{
    shared_link::shared_order,
    viewers::{Editing, Viewer, Viewers},
};

pub const SHARED_WITH_ME: &str = "Shared with me";
pub const SHARED_BY: &str = "Shared by";
pub const ACCESS: &str = "Access";
pub const EXPIRES: &str = "Expires";
pub const NEVER: &str = "Never";
pub const SAVE_TO_MY_FILES: &str = "Save to my files…";
pub const NOTHING_SHARED_WITH_ME: &str =
    "Nothing is shared with you yet. Files and folders other people share with you show up here.";
/// The access of every share for now: open and download, never change.
pub const VIEW_ONLY: &str = "Can view";

/// One item shared with the user, with the share it came through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedItem {
    pub share_id: Uuid,
    pub owner: UserRef,
    pub item: SharedNode,
    pub expires_at: Option<OffsetDateTime>,
}

/// Every item of `shares` as its own row: folders first, then by name.
pub fn shared_items(shares: &[ReceivedShare]) -> Vec<SharedItem> {
    let mut items: Vec<SharedItem> = shares
        .iter()
        .flat_map(|share| {
            share.items.iter().map(|item| SharedItem {
                share_id: share.id,
                owner: share.owner.clone(),
                item: item.clone(),
                expires_at: share.expires_at,
            })
        })
        .collect();
    items.sort_by(|a, b| shared_order(&a.item, &b.item));
    items
}

/// The share holding `share_id`, among `shares`.
pub fn find_share(shares: &[ReceivedShare], share_id: Uuid) -> Option<&ReceivedShare> {
    shares.iter().find(|share| share.id == share_id)
}

/// The built-in viewer for a shared `item`, read-only since shares cannot be changed; folders
/// and files no viewer shows have none.
pub fn shared_viewer(viewers: &Viewers, item: &SharedNode) -> Option<Viewer> {
    match item.kind {
        NodeKind::Folder => None,
        NodeKind::File => viewers
            .viewer_for(&item.mime, item.size)
            .map(|viewer| Viewer {
                editing: Editing::ReadOnly,
                ..viewer
            }),
    }
}

/// The shared files beside the one shown that open in the same viewer, such as the pictures
/// of a shared folder, in listing order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedGallery {
    items: Vec<SharedNode>,
    current: NodeId,
}

impl SharedGallery {
    /// The gallery of `current` among `listing`, the items beside it. `None` when `current`
    /// has no viewer, or its viewer does not step between files.
    pub fn new(viewers: &Viewers, listing: &[SharedNode], current: &SharedNode) -> Option<Self> {
        let kind = shared_viewer(viewers, current)
            .map(|viewer| viewer.kind)
            .filter(|kind| kind.navigates())?;
        let items = listing
            .iter()
            .filter(|item| shared_viewer(viewers, item).is_some_and(|viewer| viewer.kind == kind))
            .cloned()
            .collect();
        Some(Self {
            items,
            current: current.id,
        })
    }

    /// The file before the one shown; none at the first file.
    pub fn previous(&self) -> Option<&SharedNode> {
        self.index()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| self.items.get(index))
    }

    /// The file after the one shown; none at the last file.
    pub fn next(&self) -> Option<&SharedNode> {
        self.index()
            .and_then(|index| index.checked_add(1))
            .and_then(|index| self.items.get(index))
    }

    /// Where the file shown is, counting from 1, and how many files there are.
    pub fn position(&self) -> Option<(usize, usize)> {
        self.index().map(|index| (index + 1, self.items.len()))
    }

    /// Shows file `id` instead.
    pub fn move_to(&mut self, id: NodeId) {
        self.current = id;
    }

    fn index(&self) -> Option<usize> {
        self.items.iter().position(|item| item.id == self.current)
    }
}

/// "Save “report.pdf” to my files" or "Save 3 items to my files".
pub fn save_copy_title(names: &[&str]) -> String {
    match names {
        [name] => format!("Save “{name}” to my files"),
        _ => format!("Save {} items to my files", names.len()),
    }
}

#[cfg(test)]
mod tests {
    use protocol::{NodeId, NodeKind, UserId};

    use super::*;

    fn node(kind: NodeKind, name: &str) -> SharedNode {
        SharedNode {
            id: NodeId(Uuid::from_u128(name.len() as u128)),
            kind,
            name: name.to_owned(),
            size: 0,
            mime: String::new(),
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn share(number: u128, owner: &str, items: Vec<SharedNode>) -> ReceivedShare {
        ReceivedShare {
            id: Uuid::from_u128(number),
            owner: UserRef {
                id: UserId(Uuid::from_u128(number)),
                username: owner.to_owned(),
            },
            items,
            expires_at: None,
            shared_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn items_of_all_shares_are_listed_folders_first_by_name() {
        let shares = [
            share(1, "dana", vec![node(NodeKind::File, "tax.pdf")]),
            share(
                2,
                "alex",
                vec![
                    node(NodeKind::File, "video.mp4"),
                    node(NodeKind::Folder, "Holiday"),
                ],
            ),
        ];

        let rows = shared_items(&shares);

        let names: Vec<_> = rows.iter().map(|row| row.item.name.as_str()).collect();
        assert_eq!(names, ["Holiday", "tax.pdf", "video.mp4"]);
        assert_eq!(rows[1].owner.username, "dana");
        assert_eq!(rows[1].share_id, shares[0].id);
    }

    fn file(raw: u128, name: &str, mime: &str) -> SharedNode {
        SharedNode {
            id: NodeId(Uuid::from_u128(raw)),
            kind: NodeKind::File,
            name: name.to_owned(),
            size: 10,
            mime: mime.to_owned(),
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn a_shared_text_file_opens_read_only() {
        let viewer = shared_viewer(&Viewers::shared(), &file(1, "a.txt", "text/plain"));

        assert!(viewer.is_some_and(|viewer| !viewer.is_editable()));
    }

    #[test]
    fn folders_and_unknown_types_have_no_shared_viewer() {
        let viewers = Viewers::shared();

        assert!(shared_viewer(&viewers, &node(NodeKind::Folder, "Holiday")).is_none());
        assert!(shared_viewer(&viewers, &file(1, "a.zip", "application/zip")).is_none());
    }

    #[test]
    fn the_gallery_steps_through_the_pictures_only() {
        let listing = [
            file(1, "a.png", "image/png"),
            file(2, "notes.txt", "text/plain"),
            file(3, "b.jpg", "image/jpeg"),
        ];

        let mut gallery =
            SharedGallery::new(&Viewers::shared(), &listing, &listing[0]).expect("a gallery");

        assert_eq!(gallery.position(), Some((1, 2)));
        assert_eq!(gallery.next().map(|item| item.id), Some(listing[2].id));
        gallery.move_to(listing[2].id);
        assert_eq!(gallery.previous().map(|item| item.id), Some(listing[0].id));
        assert!(gallery.next().is_none());
    }

    #[test]
    fn text_files_have_no_gallery() {
        let listing = [file(1, "a.txt", "text/plain")];

        assert!(SharedGallery::new(&Viewers::shared(), &listing, &listing[0]).is_none());
    }

    #[test]
    fn the_save_title_names_one_item_or_counts_them() {
        assert_eq!(save_copy_title(&["a.pdf"]), "Save “a.pdf” to my files");
        assert_eq!(save_copy_title(&["a", "b"]), "Save 2 items to my files");
    }
}
