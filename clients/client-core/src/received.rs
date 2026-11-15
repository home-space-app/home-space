//! Shares other users made for the signed-in user: "Shared with me". They are read-only; the
//! user opens and downloads them, or saves copies into their own files.

use time::OffsetDateTime;
use uuid::Uuid;

use protocol::{ReceivedShare, SharedNode, UserRef};

use crate::shared_link::shared_order;

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

/// "Shared by alex · View only. You can open and download, but not change anything.
/// Available until 30 Nov 2026.", with the date as the client formats it.
pub fn view_only_note(owner: &str, until: Option<&str>) -> String {
    let note = format!(
        "Shared by {owner} · View only. You can open and download, but not change anything."
    );
    match until {
        Some(date) => format!("{note} Available until {date}."),
        None => note,
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

    #[test]
    fn the_note_names_the_owner_and_the_end_date() {
        assert_eq!(
            view_only_note("alex", Some("30 Nov 2026")),
            "Shared by alex · View only. You can open and download, but not change anything. \
             Available until 30 Nov 2026."
        );
        assert!(!view_only_note("alex", None).contains("Available"));
    }

    #[test]
    fn the_save_title_names_one_item_or_counts_them() {
        assert_eq!(save_copy_title(&["a.pdf"]), "Save “a.pdf” to my files");
        assert_eq!(save_copy_title(&["a", "b"]), "Save 2 items to my files");
    }
}
