use db::DbError;

/// Errors returned by file operations.
#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    /// The node does not exist or belongs to another user.
    #[error("not found")]
    NotFound,
    /// A node with the same name already exists in the folder.
    #[error("a node with this name already exists")]
    Conflict,
    /// The name is empty, too long or contains forbidden characters.
    #[error("invalid name")]
    InvalidName,
    /// A folder cannot be moved into itself or its descendants.
    #[error("cannot move a folder into itself")]
    Cycle,
    /// Lost & Found, or a folder containing it, cannot be deleted.
    #[error("Lost & Found cannot be deleted")]
    LostAndFound,
    /// The operation requires a file but the node is a folder.
    #[error("node is not a file")]
    NotAFile,
    /// The upload exceeded the configured size limit.
    #[error("upload exceeds the maximum size of {limit} bytes")]
    TooLarge { limit: u64 },
    /// Storing the content would take the owner past their storage quota of `quota` bytes, of
    /// which `available` are left once their unfinished uploads are counted.
    #[error("not enough storage: {available} of your {quota} bytes are left")]
    QuotaExceeded { quota: u64, available: u64 },
    /// A chunk of a resumable upload does not start where the received bytes end.
    #[error("upload has {current} bytes; the chunk must start there")]
    OffsetMismatch { current: u64 },
    /// The caller may not send to that device.
    #[error("the device cannot receive files from you")]
    Forbidden,
    /// The transfer was declined, cancelled, downloaded or saved, or has expired.
    #[error("the transfer is no longer available")]
    Gone,
    /// The transfer's files are still being uploaded.
    #[error("the transfer is not ready yet")]
    NotReady,
    /// The manifest of a transfer is not acceptable.
    #[error("{0}")]
    InvalidTransfer(String),
    #[error("storage error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Db(DbError),
}

impl From<DbError> for FilesError {
    fn from(err: DbError) -> Self {
        match err {
            DbError::NotFound => Self::NotFound,
            DbError::Conflict => Self::Conflict,
            DbError::Invalid => Self::InvalidName,
            DbError::Cycle => Self::Cycle,
            DbError::LostAndFound => Self::LostAndFound,
            other => Self::Db(other),
        }
    }
}
