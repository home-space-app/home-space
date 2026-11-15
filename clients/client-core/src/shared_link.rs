//! A share link as its visitor sees it: the page's words and the order of the items.

use std::cmp::Ordering;

use protocol::{NodeId, NodeKind, SharedNode};

pub const SHARED_WITH_YOU: &str = "Shared with you";
pub const PASSWORD_PROTECTED: &str = "This link is password protected";
pub const PASSWORD: &str = "Password";
pub const OPEN: &str = "Open";
pub const WRONG_PASSWORD: &str = "Wrong password. Try again.";
pub const LINK_GONE: &str = "This link no longer works";
pub const LINK_GONE_NOTE: &str =
    "It expired or the owner stopped sharing. Ask the person who sent it for a new link.";
pub const EMPTY_FOLDER: &str = "This folder is empty.";
pub const DOWNLOAD: &str = "Download";
pub const DOWNLOAD_ALL: &str = "Download all";

/// "1 item" or "3 items".
pub fn item_count(count: u32) -> String {
    if count == 1 {
        "1 item".to_owned()
    } else {
        format!("{count} items")
    }
}

/// The line under the title of the shared items, e.g. "sam shared 3 items."
pub fn shared_summary(owner: &str, count: u32) -> String {
    format!("{owner} shared {}.", item_count(count))
}

/// What the password page asks of the visitor.
pub fn password_prompt(owner: &str, count: u32) -> String {
    let them = if count == 1 { "it" } else { "them" };
    format!(
        "{owner} shared {} with you. Enter the password {owner} gave you to open {them}.",
        item_count(count)
    )
}

/// "Shared by sam".
pub fn shared_by(owner: &str) -> String {
    format!("Shared by {owner}")
}

/// "Expires 19 Nov 2026", from a date the client formatted.
pub fn expires_on(date: &str) -> String {
    format!("Expires {date}")
}

/// The first letter of `owner`, for their avatar.
pub fn initial(owner: &str) -> String {
    owner
        .chars()
        .next()
        .map(|first| first.to_uppercase().collect())
        .unwrap_or_default()
}

/// Folders first, then by name, ignoring case.
pub fn sort_shared(items: &mut [SharedNode]) {
    items.sort_by(shared_order);
}

/// Folders before files, then by name, ignoring case.
pub fn shared_order(a: &SharedNode, b: &SharedNode) -> Ordering {
    folders_first(a.kind, b.kind).then_with(|| by_name(&a.name, &b.name))
}

/// How many folders of the open `trail` still lead to `path`: those need not be looked up
/// again.
pub fn kept_depth(trail: &[SharedNode], path: &[NodeId]) -> usize {
    trail
        .iter()
        .zip(path)
        .take_while(|(folder, id)| folder.id == **id)
        .count()
}

fn folders_first(a: NodeKind, b: NodeKind) -> Ordering {
    (a != NodeKind::Folder).cmp(&(b != NodeKind::Folder))
}

fn by_name(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    fn item(kind: NodeKind, name: &str) -> SharedNode {
        folder_with(Uuid::nil(), kind, name)
    }

    fn folder(id: u128) -> SharedNode {
        folder_with(Uuid::from_u128(id), NodeKind::Folder, "folder")
    }

    fn folder_with(id: Uuid, kind: NodeKind, name: &str) -> SharedNode {
        SharedNode {
            id: NodeId::from(id),
            kind,
            name: name.to_owned(),
            size: 0,
            mime: String::new(),
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn one_item_is_counted_in_the_singular() {
        assert_eq!(item_count(1), "1 item");
    }

    #[test]
    fn several_items_are_counted_in_the_plural() {
        assert_eq!(item_count(3), "3 items");
    }

    #[test]
    fn the_summary_names_the_owner_and_the_count() {
        assert_eq!(shared_summary("sam", 3), "sam shared 3 items.");
    }

    #[test]
    fn the_password_prompt_asks_to_open_several_items() {
        assert_eq!(
            password_prompt("sam", 3),
            "sam shared 3 items with you. Enter the password sam gave you to open them."
        );
    }

    #[test]
    fn the_password_prompt_asks_to_open_one_item() {
        assert_eq!(
            password_prompt("sam", 1),
            "sam shared 1 item with you. Enter the password sam gave you to open it."
        );
    }

    #[test]
    fn the_initial_is_the_first_letter_in_upper_case() {
        assert_eq!(initial("sam"), "S");
    }

    #[test]
    fn an_empty_name_has_no_initial() {
        assert_eq!(initial(""), "");
    }

    #[test]
    fn folders_come_before_files_and_names_ignore_case() {
        let mut items = vec![
            item(NodeKind::File, "b.txt"),
            item(NodeKind::Folder, "Photos"),
            item(NodeKind::File, "A.txt"),
            item(NodeKind::Folder, "invoices"),
        ];

        sort_shared(&mut items);

        let names: Vec<&str> = items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["invoices", "Photos", "A.txt", "b.txt"]);
    }

    fn ids(ids: &[u128]) -> Vec<NodeId> {
        ids.iter()
            .map(|id| NodeId::from(Uuid::from_u128(*id)))
            .collect()
    }

    #[test]
    fn going_deeper_keeps_the_whole_trail() {
        assert_eq!(kept_depth(&[folder(1), folder(2)], &ids(&[1, 2, 3])), 2);
    }

    #[test]
    fn going_up_keeps_the_folders_still_on_the_path() {
        assert_eq!(kept_depth(&[folder(1), folder(2)], &ids(&[1])), 1);
    }

    #[test]
    fn another_branch_keeps_only_the_shared_start() {
        assert_eq!(kept_depth(&[folder(1), folder(2)], &ids(&[1, 4])), 1);
    }

    #[test]
    fn nothing_opened_keeps_nothing() {
        assert_eq!(kept_depth(&[], &ids(&[1])), 0);
    }
}
