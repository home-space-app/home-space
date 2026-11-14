//! The built-in viewers, registered per MIME type.
//!
//! Every client keeps one [`Viewers`], looks a file up in it and renders the [`ViewerKind`] it
//! gets back with its own widget. [`Viewers::shared`] holds the types every client shows; a
//! client extends it with the types only it can show. Adding a viewer means adding a
//! [`ViewerKind`], a registration in [`Viewers::shared`] and one match arm per client.

use std::borrow::Cow;

use protocol::{Node, NodeKind};

const WILDCARD: &str = "/*";
const PARAMETER_SEPARATOR: char = ';';
const MIB: u64 = 1024 * 1024;
const TEXT_MAX_BYTES: u64 = 2 * MIB;
const IMAGE_MAX_BYTES: u64 = 50 * MIB;

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
    /// A picture, shown read-only; the other pictures of its folder are a step away.
    Image,
}

impl ViewerKind {
    /// Whether the viewer steps to the previous and next file of the same kind in a folder.
    pub fn navigates(self) -> bool {
        match self {
            Self::Text => false,
            Self::Image => true,
        }
    }
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

#[derive(Debug, Clone)]
struct Registration {
    patterns: Vec<Cow<'static, str>>,
    viewer: Viewer,
}

const TEXT_PATTERNS: &[&str] = &["text/*"];

/// The image types every client decodes; a client adds the ones only it can show with
/// [`Viewers::with_types`].
pub const IMAGE_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/bmp",
];

/// The built-in viewers of a client and the MIME types each accepts.
///
/// [`Viewers::shared`] holds the types every client can show. A client that can show more,
/// such as an image format its platform decodes, adds them with [`Viewers::with_types`].
#[derive(Debug, Clone)]
pub struct Viewers {
    registrations: Vec<Registration>,
}

impl Default for Viewers {
    fn default() -> Self {
        Self::shared()
    }
}

impl Viewers {
    /// The viewers and types every client supports.
    pub fn shared() -> Self {
        let text = Registration {
            patterns: TEXT_PATTERNS
                .iter()
                .chain(TEXT_LIKE_TYPES)
                .map(|pattern| Cow::Borrowed(*pattern))
                .collect(),
            viewer: Viewer {
                kind: ViewerKind::Text,
                editing: Editing::Editable,
                max_bytes: TEXT_MAX_BYTES,
            },
        };
        let image = Registration {
            patterns: IMAGE_TYPES
                .iter()
                .map(|pattern| Cow::Borrowed(*pattern))
                .collect(),
            viewer: Viewer {
                kind: ViewerKind::Image,
                editing: Editing::ReadOnly,
                max_bytes: IMAGE_MAX_BYTES,
            },
        };
        Self {
            registrations: vec![text, image],
        }
    }

    /// These viewers with `types` also shown in the viewer of `kind`, with its editing mode and
    /// size limit. A type is an exact type (`image/tiff`) or a whole top-level type (`image/*`);
    /// the types already registered are matched first.
    pub fn with_types<I, S>(mut self, kind: ViewerKind, types: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<Cow<'static, str>>,
    {
        if let Some(registration) = self
            .registrations
            .iter_mut()
            .find(|registration| registration.viewer.kind == kind)
        {
            registration.patterns.extend(
                types
                    .into_iter()
                    .map(|pattern| Cow::Owned(pattern.into().to_ascii_lowercase())),
            );
        }
        self
    }

    /// The viewer for content of type `mime` that is `size` bytes long, if any.
    ///
    /// The first registration whose pattern matches wins. A pattern is either an exact type
    /// (`application/json`) or a whole top-level type (`text/*`); case and parameters such as
    /// `; charset=utf-8` are ignored. Content larger than the viewer's limit gets no viewer.
    pub fn viewer_for(&self, mime: &str, size: i64) -> Option<Viewer> {
        let essence = essence(mime);
        self.registrations
            .iter()
            .find(|registration| {
                registration
                    .patterns
                    .iter()
                    .any(|pattern| matches(pattern, &essence))
            })
            .map(|registration| registration.viewer)
            .filter(|viewer| u64::try_from(size).is_ok_and(|size| size <= viewer.max_bytes))
    }

    /// The viewer for `node`; folders have none.
    pub fn viewer_for_node(&self, node: &Node) -> Option<Viewer> {
        match node.kind {
            NodeKind::Folder => None,
            NodeKind::File => self.viewer_for(&node.mime, node.size),
        }
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
        Viewers::shared()
            .viewer_for(mime, 10)
            .map(|viewer| viewer.kind)
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
            "image/tiff",
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
        assert!(Viewers::shared().viewer_for("text/plain", limit).is_some());
        assert!(
            Viewers::shared()
                .viewer_for("text/plain", limit + 1)
                .is_none()
        );
    }

    #[test]
    fn negative_sizes_have_no_viewer() {
        assert!(Viewers::shared().viewer_for("text/plain", -1).is_none());
    }

    #[test]
    fn the_text_viewer_is_editable() {
        assert!(
            Viewers::shared()
                .viewer_for("text/plain", 0)
                .unwrap()
                .is_editable()
        );
    }

    #[test]
    fn folders_have_no_viewer() {
        assert!(
            Viewers::shared()
                .viewer_for_node(&node(NodeKind::Folder, "text/plain", 0))
                .is_none()
        );
    }

    #[test]
    fn files_use_their_mime_type() {
        let viewer = Viewers::shared().viewer_for_node(&node(NodeKind::File, "text/csv", 3));
        assert_eq!(viewer.map(|viewer| viewer.kind), Some(ViewerKind::Text));
    }

    #[test]
    fn shared_image_types_use_the_image_viewer() {
        let kinds: Vec<_> = IMAGE_TYPES.iter().map(|mime| kind(mime)).collect();
        assert_eq!(kinds, vec![Some(ViewerKind::Image); IMAGE_TYPES.len()]);
    }

    #[test]
    fn the_image_viewer_is_read_only() {
        let viewer = Viewers::shared().viewer_for("image/png", 0).unwrap();
        assert!(!viewer.is_editable());
    }

    #[test]
    fn only_images_navigate() {
        assert!(ViewerKind::Image.navigates());
        assert!(!ViewerKind::Text.navigates());
    }

    #[test]
    fn an_extra_type_uses_its_viewer() {
        let viewers = Viewers::shared().with_types(ViewerKind::Image, ["image/tiff"]);
        let viewer = viewers.viewer_for("image/tiff", 10);
        assert_eq!(viewer.map(|viewer| viewer.kind), Some(ViewerKind::Image));
    }

    #[test]
    fn an_extra_type_is_matched_without_case() {
        let viewers =
            Viewers::shared().with_types(ViewerKind::Image, [String::from("Image/X-Canon-CR2")]);
        assert!(viewers.viewer_for("image/x-canon-cr2", 10).is_some());
    }

    #[test]
    fn an_extra_type_keeps_the_viewers_limit() {
        let viewers = Viewers::shared().with_types(ViewerKind::Image, ["image/tiff"]);
        let limit = i64::try_from(IMAGE_MAX_BYTES).unwrap();
        assert!(viewers.viewer_for("image/tiff", limit).is_some());
        assert!(viewers.viewer_for("image/tiff", limit + 1).is_none());
    }

    #[test]
    fn extra_types_stay_with_the_instance_that_added_them() {
        let _extended = Viewers::shared().with_types(ViewerKind::Image, ["image/tiff"]);
        assert!(Viewers::shared().viewer_for("image/tiff", 10).is_none());
    }
}
