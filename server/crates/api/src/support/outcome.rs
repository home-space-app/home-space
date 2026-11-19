//! The answers to what a file service call made. Publishing what changed is the service's
//! job; the handlers only answer.

use axum::{Json, http::StatusCode};

use db::StoreOutcome;
use protocol::{Node, NodeId};

/// The stored file, `201 Created` when it is new and `200 OK` when an existing one changed.
pub(crate) fn stored(outcome: StoreOutcome) -> (StatusCode, Json<Node>) {
    let status = match outcome {
        StoreOutcome::Created(_) => StatusCode::CREATED,
        StoreOutcome::Unchanged(_)
        | StoreOutcome::Versioned { .. }
        | StoreOutcome::Archived { .. } => StatusCode::OK,
    };
    (status, Json(outcome.into_node().into()))
}

/// The folders and files saved directly into `parent`, folders first.
pub(crate) fn saved_at_top(
    parent: Option<NodeId>,
    folders: Vec<db::Node>,
    files: Vec<StoreOutcome>,
) -> Vec<Node> {
    folders
        .into_iter()
        .chain(files.into_iter().map(StoreOutcome::into_node))
        .filter(|node| node.parent_id == parent)
        .map(Node::from)
        .collect()
}
