//! What resumable uploads store, report and return.

use db::NodeId;

use crate::{StoreOptions, StoreOutcome};
use uuid::Uuid;

/// What a resumable upload stores once complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadTarget {
    /// A file named `name` in `parent` (the root when `None`), stored like
    /// [`FileService::upload`].
    New {
        parent: Option<NodeId>,
        name: String,
        options: StoreOptions,
    },
    /// New content for an existing file, stored like [`FileService::replace_content`].
    Replace(NodeId),
    /// The bytes of a file of a transfer the caller is sending.
    TransferItem(Uuid),
}

/// Where an upload stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadStatus {
    pub id: Uuid,
    pub offset: u64,
    pub size: u64,
}

/// Result of appending a chunk.
#[derive(Debug)]
pub enum ChunkOutcome {
    /// More bytes are expected, starting at `offset`.
    Partial { offset: u64 },
    /// The last chunk arrived and the file was stored.
    Complete(StoreOutcome),
    /// The last chunk of a transfer's file arrived (`size` bytes in all); `ready` is the
    /// transfer when it was its last missing file.
    Transferred {
        size: u64,
        ready: Option<db::Transfer>,
    },
}
