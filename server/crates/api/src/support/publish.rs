//! Telling the search index and the user's devices what a request changed. This goes away
//! once services publish their own domain events (S2 in docs/todo/server-refactoring.md).

use axum::{Json, http::StatusCode};
use uuid::Uuid;

use db::StoreOutcome;
use notifications::Event;
use protocol::{Node, NodeId, TransferState};

use super::convert::summary;
use crate::AppState;

pub(crate) async fn publish_stored(
    state: &AppState,
    owner: db::UserId,
    outcome: StoreOutcome,
) -> (StatusCode, Json<Node>) {
    match outcome {
        StoreOutcome::Created(node) => {
            state.index_name(&node).await;
            state.search.enqueue_content(owner, node.id);
            let node = Node::from(node);
            state
                .hub
                .publish(owner, Event::NodeCreated { node: node.clone() });
            (StatusCode::CREATED, Json(node))
        }
        StoreOutcome::Archived { node, .. } => {
            let node = Node::from(node);
            state
                .hub
                .publish(owner, Event::NodeUpdated { node: node.clone() });
            (StatusCode::OK, Json(node))
        }
        StoreOutcome::Versioned { node, .. } => {
            state.search.enqueue_content(owner, node.id);
            let node = Node::from(node);
            state
                .hub
                .publish(owner, Event::NodeUpdated { node: node.clone() });
            (StatusCode::OK, Json(node))
        }
        StoreOutcome::Unchanged(node) => (StatusCode::OK, Json(node.into())),
    }
}

/// Indexes and announces what a move or restore changed.
pub(crate) async fn publish_changes(
    state: &AppState,
    owner: db::UserId,
    updated: &[db::Node],
    deleted: &[db::Node],
    content_changed: Vec<NodeId>,
) {
    for node in updated {
        state.index_name(node).await;
        let event = Event::NodeUpdated {
            node: node.clone().into(),
        };
        state.hub.publish(owner, event);
    }
    for node in deleted {
        state.hub.publish(
            owner,
            Event::NodeDeleted {
                id: node.id,
                parent_id: node.parent_id,
            },
        );
    }
    for id in content_changed {
        state.search.enqueue_content(owner, id);
    }
}

/// Tells the recipient a transfer is ready, and the sender that it is.
pub(crate) fn publish_ready(state: &AppState, transfer: db::Transfer) {
    let (id, sender) = (transfer.id, transfer.sender_id);
    let recipient = transfer.recipient_id;
    state.hub.publish(
        recipient,
        Event::TransferReceived {
            transfer: summary(transfer),
        },
    );
    if sender != recipient {
        state.hub.publish(
            sender,
            Event::TransferChanged {
                id,
                state: TransferState::Ready,
            },
        );
    }
}

/// Tells both sides a transfer changed state.
pub(crate) fn publish_changed(
    hub: &notifications::Hub,
    id: Uuid,
    sender: db::UserId,
    recipient: db::UserId,
    to: TransferState,
) {
    hub.publish(sender, Event::TransferChanged { id, state: to });
    if recipient != sender {
        hub.publish(recipient, Event::TransferChanged { id, state: to });
    }
}

/// Tells `owner`'s devices about folders and files saved into their tree under `parent`, and
/// answers the ones at the top.
pub(crate) async fn publish_saved(
    state: &AppState,
    owner: db::UserId,
    parent: Option<NodeId>,
    folders: Vec<db::Node>,
    files: Vec<db::StoreOutcome>,
) -> Vec<Node> {
    let mut top = Vec::new();
    for folder in folders {
        state.index_name(&folder).await;
        let is_top = folder.parent_id == parent;
        let node = Node::from(folder);
        state
            .hub
            .publish(owner, Event::NodeCreated { node: node.clone() });
        if is_top {
            top.push(node);
        }
    }
    for outcome in files {
        let (_, Json(node)) = publish_stored(state, owner, outcome).await;
        if node.parent_id == parent {
            top.push(node);
        }
    }
    top
}
