//! What resumable uploads store, report and return.

use db::NodeId;

use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Names, StoreOptions, StoreOutcome};

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

/// A resumable upload to start: `size` bytes for `target`, a new file's name kept as `names`
/// stores the names of the folder it goes to, with what is known of the content.
#[derive(Clone, Debug)]
pub struct UploadRequest<'a> {
    pub target: UploadTarget,
    pub names: &'a Names,
    pub size: u64,
    pub content_type: Option<&'a str>,
    pub modified_at: Option<OffsetDateTime>,
}

/// Where an upload stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadStatus {
    pub id: Uuid,
    pub offset: u64,
    pub size: u64,
}

/// Where an upload stores its file once complete, from
/// [`UploadService::destination`](crate::UploadService::destination).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadDestination {
    /// A file in this folder; the root when `None`.
    Folder(Option<NodeId>),
    /// A file of a transfer, outside the tree.
    Transfer,
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
