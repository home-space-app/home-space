use protocol::{Node, NodeKind};

use crate::viewers::TEXT_LIKE_TYPES;

const IMAGE_PREFIX: &str = "image/";
const TEXT_PREFIX: &str = "text/";
const DOCUMENT_TYPES: &[&str] = &[
    "application/pdf",
    "application/rtf",
    "application/msword",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.oasis.opendocument.text",
];

/// The kind of icon that represents a node; each client maps it to its own asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeIcon {
    Folder,
    Image,
    Document,
    Other,
}

/// The icon that represents `node`, chosen by MIME type.
pub fn node_icon(node: &Node) -> NodeIcon {
    match (node.kind, node.mime.as_str()) {
        (NodeKind::Folder, _) => NodeIcon::Folder,
        (_, mime) if mime.starts_with(IMAGE_PREFIX) => NodeIcon::Image,
        (_, mime)
            if mime.starts_with(TEXT_PREFIX)
                || DOCUMENT_TYPES.contains(&mime)
                || TEXT_LIKE_TYPES.contains(&mime) =>
        {
            NodeIcon::Document
        }
        _ => NodeIcon::Other,
    }
}

#[cfg(test)]
mod tests {
    use protocol::NodeId;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    fn node(kind: NodeKind, mime: Option<&str>) -> Node {
        Node {
            id: NodeId::from(Uuid::nil()),
            parent_id: None,
            kind,
            name: String::new(),
            size: 0,
            sha256: None,
            mime: mime.unwrap_or_default().to_owned(),
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    #[test]
    fn folders_use_the_folder_icon() {
        assert_eq!(node_icon(&node(NodeKind::Folder, None)), NodeIcon::Folder);
    }

    #[test]
    fn images_use_the_image_icon() {
        assert_eq!(
            node_icon(&node(NodeKind::File, Some("image/png"))),
            NodeIcon::Image
        );
    }

    #[test]
    fn text_and_documents_use_the_document_icon() {
        let icons: Vec<_> = ["text/plain", "application/pdf"]
            .into_iter()
            .map(|mime| node_icon(&node(NodeKind::File, Some(mime))))
            .collect();
        assert_eq!(icons, [NodeIcon::Document, NodeIcon::Document]);
    }

    #[test]
    fn everything_else_uses_the_other_icon() {
        let icons: Vec<_> = [
            Some("application/zip"),
            Some("application/octet-stream"),
            None,
        ]
        .into_iter()
        .map(|mime| node_icon(&node(NodeKind::File, mime)))
        .collect();
        assert_eq!(icons, [NodeIcon::Other, NodeIcon::Other, NodeIcon::Other]);
    }
}
