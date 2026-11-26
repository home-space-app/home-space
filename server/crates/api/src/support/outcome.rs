//! The answers to what a file service call made. Publishing what changed is the service's
//! job; the handlers only answer.

use axum::{Json, http::StatusCode};

use files::{Names, StoreOutcome};
use protocol::{Node, NodeId};

/// The stored file, `201 Created` when it is new and `200 OK` when an existing one changed,
/// with its name as `names`, those of its folder, show it.
pub(crate) fn stored(outcome: StoreOutcome, names: &Names) -> (StatusCode, Json<Node>) {
    let status = match outcome {
        StoreOutcome::Created(_) => StatusCode::CREATED,
        StoreOutcome::Unchanged(_)
        | StoreOutcome::Versioned { .. }
        | StoreOutcome::Archived { .. } => StatusCode::OK,
    };
    (status, Json(names.show_node(outcome.into_node()).into()))
}

/// The folders and files saved directly into `parent`, folders first, with their names as
/// `names`, those of `parent`, show them.
pub(crate) fn saved_at_top(
    parent: Option<NodeId>,
    names: &Names,
    folders: Vec<db::Node>,
    files: Vec<StoreOutcome>,
) -> Vec<Node> {
    folders
        .into_iter()
        .chain(files.into_iter().map(StoreOutcome::into_node))
        .filter(|node| node.parent_id == parent)
        .map(|node| names.show_node(node).into())
        .collect()
}
