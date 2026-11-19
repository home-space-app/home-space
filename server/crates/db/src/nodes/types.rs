//! Values passed to and returned by the node queries.

use crate::{Node, NodeId, NodeKind, Patch};
use time::OffsetDateTime;
use uuid::Uuid;

/// How [`Db::store_file_with`](crate::Db::store_file_with) stores a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreOptions {
    /// Give a file whose name is taken by different content a free name, instead of making it a
    /// new version.
    pub keep_both: bool,
    /// With `keep_both`: the one file with the name that does get a new version.
    pub version_of: Option<NodeId>,
    /// Record the writing device as the file's source, when it stored new content.
    pub auto_upload: bool,
}

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
    /// When the client last changed the content; `None` means now. A time in the future
    /// counts as now.
    pub modified_at: Option<OffsetDateTime>,
}

/// A blob operation the storage layer must perform after a database change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlobOp {
    /// Keep the current content of `node` as the stored content of a version.
    Archive { node: NodeId, version_id: Uuid },
    /// Make the content of node `from` the content of node `to`.
    Promote { from: NodeId, to: NodeId },
    /// Drop the content of a node that no longer exists.
    Remove { node: NodeId },
}

/// What storing file content did to the tree.
#[derive(Clone, Debug)]
pub enum StoreOutcome {
    Created(Node),
    /// The content matched the current one, nothing changed.
    Unchanged(Node),
    /// The previous content was kept as version `archived`.
    Versioned {
        node: Node,
        archived: Uuid,
    },
    /// The content was older than the current one and was kept as version `archived`; the
    /// current content did not change.
    Archived {
        node: Node,
        archived: Uuid,
    },
}

impl StoreOutcome {
    /// The file the content was stored in.
    pub fn into_node(self) -> Node {
        match self {
            Self::Created(node)
            | Self::Unchanged(node)
            | Self::Versioned { node, .. }
            | Self::Archived { node, .. } => node,
        }
    }
}

/// The effects of moving a node, including merges into same-named nodes.
#[derive(Clone, Debug)]
pub struct MoveOutcome {
    /// The node at the final location; it is the destination when a merge happened.
    pub node: Node,
    pub updated: Vec<Node>,
    pub deleted: Vec<Node>,
    pub content_changed: Vec<NodeId>,
    pub blob_ops: Vec<BlobOp>,
}

/// A file that clashes with a live file at its destination.
#[derive(Clone, Debug)]
pub struct MoveConflict {
    pub source: Node,
    /// Path of the file relative to what is moved, e.g. `Photos/2024/a.jpg`.
    pub path: String,
    /// The file in the way; `None` when two selected files share a name.
    pub existing: Option<Node>,
}

/// The effects of moving several nodes at once (also used to restore binned files).
#[derive(Clone, Debug, Default)]
pub struct MoveBatch {
    /// The destination folder; `None` for the root.
    pub target: Option<Node>,
    /// The selected nodes at their final location; a merge destination when they were merged.
    pub moved: Vec<Node>,
    /// Files not placed because of [`OnConflict::Skip`](crate::OnConflict::Skip): left in place, or binned by a move.
    pub skipped: Vec<Node>,
    /// The clashes found; only filled for [`OnConflict::Abort`](crate::OnConflict::Abort), in which case nothing changed.
    pub conflicts: Vec<MoveConflict>,
    pub updated: Vec<Node>,
    pub deleted: Vec<Node>,
    pub content_changed: Vec<NodeId>,
    pub blob_ops: Vec<BlobOp>,
    /// Whether skipped files were moved to the recycle bin; they are in `skipped` and `deleted`.
    pub binned: bool,
}

/// Ids whose stored content must be removed after a deletion.
#[derive(Clone, Debug, Default)]
pub struct DeletedNodes {
    pub files: Vec<NodeId>,
    pub versions: Vec<Uuid>,
}

/// Changes to apply to a node; `None` and [`Patch::Keep`] leave a field untouched.
#[derive(Clone, Debug, Default)]
pub struct NodeUpdate {
    pub name: Option<String>,
    /// [`Patch::Clear`] moves the node to the root.
    pub parent_id: Patch<NodeId>,
}
