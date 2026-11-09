//! File management for home-space: blob storage on disk plus ownership-scoped
//! operations on the folder/file tree.

mod error;
mod service;
mod store;
mod transfers;
mod uploads;

pub use error::FilesError;
pub use service::{DEFAULT_CHUNK_BYTES, FileService};
pub use store::BlobStore;
pub use transfers::{DEFAULT_TRANSFER_TTL, MAX_TRANSFER_ITEMS, SavedTransfer};
pub use uploads::{ChunkOutcome, UploadStatus, UploadTarget};
