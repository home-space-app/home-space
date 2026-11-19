//! Errors of the search crate.

use tokio::task;

use db::DbError;

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
