//! File management for home-space: blob storage on disk plus ownership-scoped operations on
//! the folder/file tree, split by concern over one [`ContentStore`](content::ContentStore).

mod actor;
mod archive;
mod content;
mod copy;
mod duplicates;
mod emitted;
mod error;
mod nodes;
mod services;
mod store;
mod transfers;
mod trash;
mod tree;
mod uploads;
mod versions;

pub use actor::Actor;
pub use archive::{ArchiveEntry, MAX_ARCHIVE_ENTRIES};
pub use copy::SavedCopy;
pub use duplicates::{DuplicateService, MAX_GROUPS, MAX_OUTDATED};
pub use error::{BodyError, FilesError};
pub use nodes::NodeService;
pub use services::{DEFAULT_CHUNK_BYTES, FileServices, FilesConfig};
pub use store::BlobStore;
pub use transfers::{DEFAULT_TRANSFER_TTL, MAX_TRANSFER_ITEMS, SavedTransfer, TransferService};
pub use trash::TrashService;
pub use tree::{
    BlobOp, MoveBatch, MoveConflict, MoveOutcome, NodeUpdate, StoreOptions, StoreOutcome,
    TrashOutcome,
};
pub use uploads::{ChunkOutcome, UploadService, UploadStatus, UploadTarget};
pub use versions::VersionService;
