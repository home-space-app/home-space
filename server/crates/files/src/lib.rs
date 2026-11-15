//! File management for home-space: blob storage on disk plus ownership-scoped
//! operations on the folder/file tree.

mod archive;
mod copy;
mod error;
mod service;
mod store;
mod transfers;
mod uploads;

pub use archive::{ArchiveEntry, MAX_ARCHIVE_ENTRIES};
pub use copy::SavedCopy;
pub use error::FilesError;
pub use service::{DEFAULT_CHUNK_BYTES, FileService};
pub use store::BlobStore;
pub use transfers::{DEFAULT_TRANSFER_TTL, MAX_TRANSFER_ITEMS, SavedTransfer};
pub use uploads::{ChunkOutcome, UploadStatus, UploadTarget};
