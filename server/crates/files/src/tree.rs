//! How the tree changes when content is stored and nodes move: the rules (what clashes, what
//! merges, which content stays current, what a free name is) and the steps of
//! [`db::Tx`] that carry them out, each change in one transaction.
//!
//! The services call these and then store bytes and publish events for what they did.

mod bin;
mod conflicts;
pub(crate) mod guard;
mod moves;
pub(crate) mod rules;
mod store;
mod types;

pub(crate) use bin::{restore, trash_node};
pub(crate) use moves::{move_nodes, update_node};
pub(crate) use store::{replace_content, store_file};
pub use types::{
    BlobOp, MoveBatch, MoveConflict, MoveOutcome, NodeUpdate, StoreOptions, StoreOutcome,
    TrashOutcome,
};

#[cfg(test)]
mod tests;
