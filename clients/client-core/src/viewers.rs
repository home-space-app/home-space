//! The built-in viewers, registered per MIME type.
//!
//! Every client looks a file up here and renders the [`ViewerKind`] it gets back with its own
//! widget. Adding a viewer means adding a [`ViewerKind`], a row in [`REGISTRY`] and one match
//! arm per client.

use protocol::{Node, NodeKind};

const WILDCARD: &str = "/*";
const PARAMETER_SEPARATOR: char = ';';
const MIB: u64 = 1024 * 1024;
const TEXT_MAX_BYTES: u64 = 2 * MIB;

/// `application/*` types whose content is plain text.
pub const TEXT_LIKE_TYPES: &[&str] = &[
    "application/json",
    "application/ld+json",
    "application/xml",
    "application/javascript",
    "application/ecmascript",
    "application/x-javascript",
    "application/typescript",
    "application/x-sh",
    "application/x-csh",
    "application/x-shellscript",
    "application/x-yaml",
    "application/yaml",
    "application/toml",
    "application/x-toml",
    "application/sql",
    "application/x-sql",
    "application/x-httpd-php",
    "application/x-python",
    "application/x-perl",
    "application/x-ruby",
    "application/xhtml+xml",
    "application/rss+xml",
    "application/atom+xml",
    "application/x-tex",
    "application/x-latex",
    "application/x-ndjson",
    "application/x-subrip",
];

/// Which built-in viewer shows a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewerKind {
    /// Plain text, shown read-only with an Edit button.
    Text,
}

/// Whether a viewer can change the file it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editing {
    ReadOnly,
    Editable,
}

/// A built-in viewer and the files it accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewer {
    pub kind: ViewerKind,
    pub editing: Editing,
    /// Larger files are opened the way they were before built-in viewers.
    pub max_bytes: u64,
}

impl Viewer {
    /// Whether the viewer offers an Edit button.
    pub fn is_editable(&self) -> bool {
        self.editing == Editing::Editable
    }
}

struct Registration {
    patterns: &'static [&'static [&'static str]],
    viewer: Viewer,
}

const TEXT_PATTERNS: &[&str] = &["text/*"];

static REGISTRY: &[Registration] = &[Registration {
    patterns: &[TEXT_PATTERNS, TEXT_LIKE_TYPES],
    viewer: Viewer {
        kind: ViewerKind::Text,
        editing: Editing::Editable,
        max_bytes: TEXT_MAX_BYTES,
    },
}];

/// The viewer for content of type `mime` that is `size` bytes long, if any.
///
/// The first registration whose pattern matches wins. A pattern is either an exact type
/// (`application/json`) or a whole top-level type (`text/*`); case and parameters such as
/// `; charset=utf-8` are ignored. Content larger than the viewer's limit gets no viewer.
pub fn viewer_for(mime: &str, size: i64) -> Option<&'static Viewer> {
    let essence = essence(mime);
    REGISTRY
        .iter()
        .find(|registration| {
            registration
                .patterns
                .iter()
                .flat_map(|patterns| patterns.iter())
                .any(|pattern| matches(pattern, &essence))
        })
        .map(|registration| &registration.viewer)
        .filter(|viewer| u64::try_from(size).is_ok_and(|size| size <= viewer.max_bytes))
}

/// The viewer for `node`; folders have none.
pub fn viewer_for_node(node: &Node) -> Option<&'static Viewer> {
    match node.kind {
        NodeKind::Folder => None,
        NodeKind::File => viewer_for(&node.mime, node.size),
    }
}

fn essence(mime: &str) -> String {
    mime.split(PARAMETER_SEPARATOR)
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn matches(pattern: &str, essence: &str) -> bool {
    match pattern.strip_suffix(WILDCARD) {
        Some(top_level) => essence
            .strip_prefix(top_level)
            .is_some_and(|rest| rest.starts_with('/') && rest.len() > 1),
        None => pattern == essence,
    }
}

#[cfg(test)]
mod tests {
    use protocol::NodeId;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    fn node(kind: NodeKind, mime: &str, size: i64) -> Node {
        Node {
            id: NodeId::from(Uuid::nil()),
            parent_id: None,
            kind,
            name: String::new(),
            size,
            sha256: None,
            mime: mime.to_owned(),
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    fn kind(mime: &str) -> Option<ViewerKind> {
        viewer_for(mime, 10).map(|viewer| viewer.kind)
    }

    #[test]
    fn text_types_use_the_text_viewer() {
        let kinds: Vec<_> = ["text/plain", "text/markdown", "text/x-rust"]
            .into_iter()
            .map(kind)
            .collect();
        assert_eq!(kinds, [Some(ViewerKind::Text); 3]);
    }

    #[test]
    fn text_like_application_types_use_the_text_viewer() {
        assert_eq!(kind("application/json"), Some(ViewerKind::Text));
    }

    #[test]
    fn case_and_parameters_are_ignored() {
        assert_eq!(kind("Text/Plain; charset=utf-8"), Some(ViewerKind::Text));
    }

    #[test]
    fn other_types_have_no_viewer() {
        let kinds: Vec<_> = [
            "application/zip",
            "image/png",
            "",
            "text",
            "text/",
            "textual/x",
        ]
        .into_iter()
        .map(kind)
        .collect();
        assert_eq!(kinds, [None; 6]);
    }

    #[test]
    fn content_over_the_limit_has_no_viewer() {
        let limit = i64::try_from(TEXT_MAX_BYTES).unwrap();
        assert!(viewer_for("text/plain", limit).is_some());
        assert!(viewer_for("text/plain", limit + 1).is_none());
    }

    #[test]
    fn negative_sizes_have_no_viewer() {
        assert!(viewer_for("text/plain", -1).is_none());
    }

    #[test]
    fn the_text_viewer_is_editable() {
        assert!(viewer_for("text/plain", 0).unwrap().is_editable());
    }

    #[test]
    fn folders_have_no_viewer() {
        assert!(viewer_for_node(&node(NodeKind::Folder, "text/plain", 0)).is_none());
    }

    #[test]
    fn files_use_their_mime_type() {
        let viewer = viewer_for_node(&node(NodeKind::File, "text/csv", 3));
        assert_eq!(viewer.map(|viewer| viewer.kind), Some(ViewerKind::Text));
    }
}
