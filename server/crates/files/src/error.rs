use db::DbError;
use protocol::{AsProblem, Problem, ProblemType};

/// Why the bytes of a request body stopped coming, as the stream that carries them says.
#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    /// The client, or a proxy in front of the server, dropped the connection before the body
    /// was complete.
    #[error("the request body was cut off")]
    Interrupted,
    /// More bytes came than the request may carry.
    #[error("the body exceeds the maximum size of {limit} bytes")]
    TooLarge { limit: u64 },
}

impl From<BodyError> for FilesError {
    fn from(err: BodyError) -> Self {
        match err {
            BodyError::Interrupted => Self::BodyInterrupted,
            BodyError::TooLarge { limit } => Self::TooLarge { limit },
        }
    }
}

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
    /// Not allowed for what a protected folder holds; the text says why.
    #[error("{0}")]
    Protected(&'static str),
    /// The name is too long to be sealed inside a password folder.
    #[error("a name inside a password folder can be at most 159 bytes long")]
    NameTooLongToProtect,
    /// A folder cannot be moved into itself or its descendants.
    #[error("cannot move a folder into itself")]
    Cycle,
    /// Lost & Found, or a folder containing it, cannot be deleted.
    #[error("Lost & Found cannot be deleted")]
    LostAndFound,
    /// The operation requires a file but the node is a folder.
    #[error("node is not a file")]
    NotAFile,
    /// The request body stopped arriving; bytes received before stay where they were written.
    #[error("the request body was cut off")]
    BodyInterrupted,
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
    /// The changes asked for were pruned from the journal; the client lists everything again.
    #[error("these changes are no longer kept; list everything again")]
    ChangesPruned,
    /// A zip would hold more than `limit` entries.
    #[error("too many items to download at once; at most {limit} fit")]
    TooManyItems { limit: usize },
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
            other => Self::Db(other),
        }
    }
}

impl AsProblem for FilesError {
    fn problem_type(&self) -> ProblemType {
        match self {
            Self::NotFound => ProblemType::NotFound,
            Self::Conflict => ProblemType::NameTaken,
            Self::InvalidName | Self::NameTooLongToProtect => ProblemType::InvalidName,
            Self::Cycle => ProblemType::MoveIntoItself,
            Self::Protected(_) => ProblemType::Protected,
            Self::LostAndFound => ProblemType::LostAndFound,
            Self::NotAFile => ProblemType::NotAFile,
            Self::BodyInterrupted => ProblemType::BodyInterrupted,
            Self::TooLarge { .. } => ProblemType::TooLarge,
            Self::QuotaExceeded { .. } => ProblemType::QuotaExceeded,
            Self::OffsetMismatch { .. } => ProblemType::OffsetMismatch,
            Self::Forbidden => ProblemType::Forbidden,
            Self::Gone => ProblemType::TransferGone,
            Self::NotReady => ProblemType::TransferNotReady,
            Self::InvalidTransfer(_) => ProblemType::InvalidTransfer,
            Self::TooManyItems { .. } => ProblemType::TooManyItems,
            Self::ChangesPruned => ProblemType::ChangesPruned,
            Self::Io(_) | Self::Db(_) => ProblemType::Internal,
        }
    }

    fn extend(&self, problem: &mut Problem) {
        match *self {
            Self::TooLarge { limit } => problem.limit = Some(limit),
            Self::TooManyItems { limit } => problem.limit = u64::try_from(limit).ok(),
            Self::QuotaExceeded { quota, available } => {
                problem.quota = Some(quota);
                problem.available = Some(available);
            }
            Self::OffsetMismatch { current } => problem.current_offset = Some(current),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_errors_mean_one_thing_each() {
        assert!(matches!(
            FilesError::from(DbError::NotFound),
            FilesError::NotFound
        ));
        assert!(matches!(
            FilesError::from(DbError::Conflict),
            FilesError::Conflict
        ));
        assert_eq!(
            FilesError::ChangesPruned.problem_type(),
            ProblemType::ChangesPruned
        );
        assert_eq!(
            FilesError::from(DbError::Sqlx(sqlx::Error::PoolTimedOut)).problem_type(),
            ProblemType::Internal
        );
    }
}
