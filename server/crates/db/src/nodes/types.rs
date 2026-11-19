//! Values passed to and returned by the node queries.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::{NodeId, NodeKind};

/// The predefined properties of a node.
#[derive(Clone, Debug, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct NodeProperties {
    /// The name of the device that last auto-uploaded the file.
    pub source_device: Option<String>,
}

/// Values for a node to insert.
#[derive(Clone, Debug)]
pub struct NewNode {
    pub id: NodeId,
    pub parent_id: Option<NodeId>,
    pub kind: NodeKind,
    pub name: String,
    pub size: i64,
    pub sha256: Option<Vec<u8>>,
    pub mime: String,
}

/// Content to store in a file.
#[derive(Clone, Debug)]
pub struct NewContent {
    pub size: i64,
    pub sha256: Vec<u8>,
    pub mime: String,
    /// When the client last changed the content; `None` means now.
    pub modified_at: Option<OffsetDateTime>,
}

/// Ids whose stored content must be removed after a deletion.
#[derive(Clone, Debug, Default)]
pub struct DeletedNodes {
    pub files: Vec<NodeId>,
    pub versions: Vec<Uuid>,
}
