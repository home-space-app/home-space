//! The rules of the tree that need no database: names for clashing files and folders, when
//! content is new, and what a file holds after another is merged into it.

use db::{FileState, NewContent, Node};
use protocol::rules::MAX_NODE_NAME_CHARS;
use time::OffsetDateTime;

/// A longer "extension" is not one, and is not kept apart when a number is added to a name.
const MAX_NUMBERED_EXTENSION: usize = 32;

/// The file name `name` with a number before its extension, still within the limit on names:
/// `IMG_1.HEIC` becomes `IMG_1 (2).HEIC`.
pub(crate) fn numbered_file_name(name: &str, number: u32) -> String {
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 && name.len() - dot <= MAX_NUMBERED_EXTENSION => name.split_at(dot),
        _ => (name, ""),
    };
    let suffix = format!(" ({number}){extension}");
    let room = MAX_NODE_NAME_CHARS.saturating_sub(suffix.chars().count());
    let stem: String = stem.chars().take(room).collect();
    format!("{stem}{suffix}")
}

/// The folder name `name`, or `name (n)` for n > 0.
pub(crate) fn numbered_folder_name(name: &str, number: usize) -> String {
    if number == 0 {
        name.to_owned()
    } else {
        format!("{name} ({number})")
    }
}

/// Whether `file` already holds content of `size` bytes hashing to `sha256`.
pub(crate) fn same_content(file: &Node, size: i64, sha256: Option<&[u8]>) -> bool {
    file.size == size && file.sha256.as_deref() == sha256
}

/// `content` with a client time in the future counted as `now`: such a time would win every
/// later conflict.
pub(crate) fn not_after(content: NewContent, now: OffsetDateTime) -> NewContent {
    NewContent {
        modified_at: content.modified_at.map(|at| at.min(now)),
        ..content
    }
}

/// What becomes of content stored in an existing file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContentFate {
    /// It is the content the file holds already; nothing changes.
    Unchanged,
    /// It was changed before the current content, at this time, so it is kept as a version
    /// and the current content stays current. Neither side is lost.
    Older(OffsetDateTime),
    /// It becomes current, and the previous content a version.
    Current,
}

impl ContentFate {
    /// The fate of `content` stored in `file`.
    pub(crate) fn of(file: &Node, content: &NewContent) -> Self {
        if same_content(file, content.size, Some(&content.sha256)) {
            Self::Unchanged
        } else {
            match content.modified_at {
                Some(at) if at < file.updated_at => Self::Older(at),
                _ => Self::Current,
            }
        }
    }
}

/// How file `source` is merged into the same-named file `dest` it is moved onto.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileMerge {
    /// Whether both hold the same content, so no version is added.
    pub(crate) identical: bool,
    /// How much the version numbers of `source` are raised to follow those of `dest`.
    pub(crate) version_offset: i32,
    /// What `dest` holds afterwards.
    pub(crate) state: FileState,
}

impl FileMerge {
    /// The newer content of the two stays current. `dest` keeps its history and gains that of
    /// `source` after it; with different content its current content becomes a version first.
    /// The merged file was created when the older one was and changed when the newer one was.
    pub(crate) fn of(source: &Node, dest: &Node) -> Self {
        let identical = same_content(dest, source.size, source.sha256.as_deref());
        let (version_offset, version, current) = if identical {
            (dest.version - 1, dest.version + source.version - 1, dest)
        } else {
            (dest.version, dest.version + source.version, source)
        };
        Self {
            identical,
            version_offset,
            state: FileState {
                size: current.size,
                sha256: current.sha256.clone(),
                mime: current.mime.clone(),
                version,
                created_at: dest.created_at.min(source.created_at),
                updated_at: dest.updated_at.max(source.updated_at),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use db::{NodeId, NodeKind, UserId};
    use time::Duration;

    use super::*;

    fn file(version: i32, sha: u8, updated_at: OffsetDateTime) -> Node {
        Node {
            id: NodeId::new(),
            owner_id: UserId(uuid::Uuid::nil()),
            parent_id: None,
            kind: NodeKind::File,
            name: "a.txt".to_owned(),
            size: 1,
            sha256: Some(vec![sha; 32]),
            mime: format!("text/{sha}"),
            version,
            created_at: updated_at,
            updated_at,
            edited_on: None,
        }
    }

    fn content(sha: u8, modified_at: Option<OffsetDateTime>) -> NewContent {
        NewContent {
            size: 1,
            sha256: vec![sha; 32],
            mime: "text/plain".to_owned(),
            modified_at,
        }
    }

    #[test]
    fn the_number_goes_before_the_extension() {
        assert_eq!(numbered_file_name("IMG_0001.HEIC", 1), "IMG_0001 (1).HEIC");
        assert_eq!(numbered_file_name("a.tar.gz", 2), "a.tar (2).gz");
        assert_eq!(numbered_file_name("notes", 3), "notes (3)");
        assert_eq!(numbered_folder_name("Photos", 0), "Photos");
        assert_eq!(numbered_folder_name("Photos", 2), "Photos (2)");
    }

    #[test]
    fn a_leading_dot_or_an_overlong_extension_is_not_an_extension() {
        assert_eq!(numbered_file_name(".bashrc", 1), ".bashrc (1)");
        let long = format!("a.{}", "x".repeat(40));
        assert_eq!(numbered_file_name(&long, 1), format!("{long} (1)"));
    }

    #[test]
    fn a_numbered_name_stays_within_the_limit_on_names() {
        let name = format!("{}.jpg", "é".repeat(MAX_NODE_NAME_CHARS - 4));
        let numbered = numbered_file_name(&name, 12);
        assert_eq!(numbered.chars().count(), MAX_NODE_NAME_CHARS);
        assert!(numbered.ends_with(" (12).jpg"));
    }

    #[test]
    fn content_is_unchanged_older_or_current() {
        let now = OffsetDateTime::now_utc();
        let current = file(3, 1, now);
        let cases = [
            (content(1, None), ContentFate::Unchanged),
            (
                content(1, Some(now - Duration::days(1))),
                ContentFate::Unchanged,
            ),
            (content(2, None), ContentFate::Current),
            (content(2, Some(now)), ContentFate::Current),
            (
                content(2, Some(now - Duration::days(1))),
                ContentFate::Older(now - Duration::days(1)),
            ),
        ];
        for (content, fate) in cases {
            assert_eq!(ContentFate::of(&current, &content), fate, "{content:?}");
        }
    }

    #[test]
    fn a_time_in_the_future_counts_as_now() {
        let now = OffsetDateTime::now_utc();
        let later = not_after(content(1, Some(now + Duration::hours(1))), now);
        let earlier = not_after(content(1, Some(now - Duration::hours(1))), now);
        assert_eq!(later.modified_at, Some(now));
        assert_eq!(earlier.modified_at, Some(now - Duration::hours(1)));
        assert_eq!(not_after(content(1, None), now).modified_at, None);
    }

    #[test]
    fn merging_different_content_makes_the_source_current_after_the_history_of_both() {
        let old = OffsetDateTime::UNIX_EPOCH;
        let new = old + Duration::days(1);
        let (source, dest) = (file(2, 1, new), file(3, 2, old));
        let merge = FileMerge::of(&source, &dest);
        assert!(!merge.identical);
        assert_eq!((merge.version_offset, merge.state.version), (3, 5));
        assert_eq!(merge.state.sha256, source.sha256);
        assert_eq!(merge.state.mime, source.mime);
        assert_eq!((merge.state.created_at, merge.state.updated_at), (old, new));
    }

    #[test]
    fn merging_identical_content_adds_only_the_older_versions_of_the_source() {
        let at = OffsetDateTime::UNIX_EPOCH;
        let (source, dest) = (file(2, 1, at), file(3, 1, at));
        let merge = FileMerge::of(&source, &dest);
        assert!(merge.identical);
        assert_eq!((merge.version_offset, merge.state.version), (2, 4));
        assert_eq!(merge.state.mime, dest.mime);
    }
}
