use std::fmt::Write;

use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{NodeId, NodeKind, UserId};

/// A registered user.
#[derive(Clone, Debug, FromRow)]
pub struct User {
    pub id: UserId,
    pub username: String,
    /// `None` for users who only sign in through an identity provider.
    pub password_hash: Option<String>,
    pub created_at: OffsetDateTime,
    /// Sees the server status; gets no access to other users' files.
    pub is_admin: bool,
}

/// A stored refresh token (only its hash is kept).
#[derive(Clone, Debug, FromRow)]
pub struct RefreshToken {
    pub id: Uuid,
    pub user_id: UserId,
    pub family_id: Uuid,
    pub token_hash: Vec<u8>,
    pub expires_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
}

/// A file or folder in a user's tree.
#[derive(Clone, Debug, PartialEq, FromRow)]
pub struct Node {
    pub id: NodeId,
    pub owner_id: UserId,
    pub parent_id: Option<NodeId>,
    pub kind: NodeKind,
    pub name: String,
    pub size: i64,
    pub sha256: Option<Vec<u8>>,
    pub mime: String,
    pub version: i32,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    /// The device that wrote the content, or created the folder.
    pub edited_on: Option<Uuid>,
}

/// A previous version of a file; its content is stored under its own id.
#[derive(Clone, Debug, PartialEq, FromRow)]
pub struct FileVersion {
    pub id: Uuid,
    pub node_id: NodeId,
    pub owner_id: UserId,
    pub version: i32,
    pub size: i64,
    pub sha256: Vec<u8>,
    pub mime: String,
    pub modified_at: OffsetDateTime,
    pub archived_at: OffsetDateTime,
    /// The device that wrote this content.
    pub edited_on: Option<Uuid>,
}

impl From<FileVersion> for protocol::FileVersion {
    fn from(version: FileVersion) -> Self {
        Self {
            version: version.version,
            size: version.size,
            sha256: to_hex(&version.sha256),
            mime: version.mime,
            modified_at: version.modified_at,
            archived_at: version.archived_at,
            edited_on: version.edited_on,
        }
    }
}

impl From<Node> for protocol::Node {
    fn from(node: Node) -> Self {
        Self {
            id: node.id,
            parent_id: node.parent_id,
            kind: node.kind,
            name: node.name,
            size: node.size,
            sha256: node.sha256.as_deref().map(to_hex),
            mime: node.mime,
            version: node.version,
            created_at: node.created_at,
            updated_at: node.updated_at,
            edited_on: node.edited_on,
        }
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(sha256: Option<Vec<u8>>) -> Node {
        let now = OffsetDateTime::UNIX_EPOCH;
        Node {
            id: NodeId::new(),
            owner_id: UserId::new(),
            parent_id: None,
            kind: NodeKind::File,
            name: "a.txt".to_owned(),
            size: 1,
            sha256,
            mime: "text/plain".to_owned(),
            version: 1,
            created_at: now,
            updated_at: now,
            edited_on: None,
        }
    }

    #[test]
    fn sha256_serializes_as_lowercase_hex() {
        let wire = protocol::Node::from(node(Some(vec![0x0a, 0xff])));
        assert_eq!(wire.sha256.as_deref(), Some("0aff"));
    }

    #[test]
    fn missing_sha256_stays_none() {
        assert_eq!(protocol::Node::from(node(None)).sha256, None);
    }
}
