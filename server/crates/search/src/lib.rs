//! Full-text search for home-space: a per-node index over names, extended with file content
//! through pluggable [`ContentProvider`]s.

mod error;
mod index;
mod provider;
mod terms;

pub use error::SearchError;
pub use index::{CatchUp, SearchIndex, Worker};
pub use provider::{ContentProvider, ExtractError};
