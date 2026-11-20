//! The duplicates page: its words, and picking the copies to bin.

use protocol::{DuplicateGroup, Node, NodeId};

use crate::format::human_size;

pub const DUPLICATES_TAB: &str = "Duplicates";
pub const COPIES_TITLE: &str = "Identical files";
pub const OUTDATED_TITLE: &str = "Matches an old version";
pub const OUTDATED_HINT: &str =
    "These files hold the content another file had before it was changed.";
pub const NO_COPIES: &str = "No identical files.";
pub const NO_OUTDATED: &str = "No files match an old version of another file.";
pub const KEEP_ONLY_THIS: &str = "Keep only this";
pub const SHOW_IN_FOLDER: &str = "Show in folder";
pub const OPEN: &str = "Open";

/// `2 copies`, `3 copies`.
pub fn copies(count: usize) -> String {
    match count {
        1 => "1 copy".to_owned(),
        _ => format!("{count} copies"),
    }
}

/// `1.5 KB each · 3 KB wasted`.
pub fn group_summary(group: &DuplicateGroup) -> String {
    format!(
        "{} each · {} wasted",
        human_size(group.size),
        human_size(group.wasted())
    )
}

/// `Version 2 of report.txt`.
pub fn version_of(version: i32, source: &Node) -> String {
    format!("Version {version} of {}", source.name)
}

/// Every copy in the group but `keep`.
pub fn others(group: &DuplicateGroup, keep: NodeId) -> Vec<Node> {
    group
        .files
        .iter()
        .filter(|file| file.node.id != keep)
        .map(|file| file.node.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use protocol::{DuplicateFile, NodeKind};
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    fn file(name: &str) -> DuplicateFile {
        DuplicateFile {
            node: Node {
                id: NodeId(Uuid::from_u128(u128::from(name.as_bytes()[0]))),
                parent_id: None,
                kind: NodeKind::File,
                name: name.to_owned(),
                size: 2048,
                sha256: Some("ab".to_owned()),
                mime: "text/plain".to_owned(),
                version: 1,
                created_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: OffsetDateTime::UNIX_EPOCH,
                edited_on: None,
            },
            location: Vec::new(),
        }
    }

    fn group() -> DuplicateGroup {
        DuplicateGroup {
            sha256: "ab".to_owned(),
            size: 2048,
            files: vec![file("a"), file("b"), file("c")],
        }
    }

    #[test]
    fn keeping_one_copy_bins_the_rest() {
        let group = group();
        let keep = group.files[1].node.id;
        let names = others(&group, keep)
            .into_iter()
            .map(|node| node.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["a", "c"]);
    }

    #[test]
    fn labels_count_copies_and_waste() {
        assert_eq!(copies(1), "1 copy");
        assert_eq!(copies(3), "3 copies");
        assert_eq!(
            group_summary(&group()),
            format!("{} each · {} wasted", human_size(2048), human_size(4096))
        );
        assert_eq!(
            version_of(2, &file("report.txt").node),
            "Version 2 of report.txt"
        );
    }
}
