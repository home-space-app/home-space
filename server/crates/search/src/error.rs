//! Errors of the search crate.

use tokio::task;

use db::DbError;
use protocol::{AsProblem, ProblemType};

use crate::provider::ExtractError;

/// Errors from indexing or searching.
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("reading content failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Extract(#[from] ExtractError),
    #[error("content extraction task failed: {0}")]
    Task(#[from] task::JoinError),
}

/// Every search failure is the server's own: the query itself cannot be wrong.
impl AsProblem for SearchError {
    fn problem_type(&self) -> ProblemType {
        match self {
            Self::Db(_) | Self::Io(_) | Self::Extract(_) | Self::Task(_) => ProblemType::Internal,
        }
    }
}
