//! The one place domain events turn into search index updates and websocket messages.

use futures_util::future::BoxFuture;

use events::{DomainEvent, EventSink};
use notifications::{Event, Hub};
use protocol::TransferState;
use search::SearchIndex;

use crate::support::convert::summary;

/// Keeps the search index and the users' devices up to date with every change.
#[derive(Clone, Debug)]
pub struct EventDispatcher {
    hub: Hub,
    search: SearchIndex,
}

impl EventDispatcher {
    pub fn new(hub: Hub, search: SearchIndex) -> Self {
        Self { hub, search }
    }

    async fn dispatch(&self, event: DomainEvent) {
        match event {
            DomainEvent::NodeCreated(node) => {
                self.index_name(&node).await;
                if node.kind == db::NodeKind::File {
                    self.search.enqueue_content(node.owner_id, node.id);
                }
                let owner = node.owner_id;
                self.hub
                    .publish(owner, Event::NodeCreated { node: node.into() });
            }
            DomainEvent::NodeUpdated(node) => {
                let owner = node.owner_id;
                self.hub
                    .publish(owner, Event::NodeUpdated { node: node.into() });
            }
            DomainEvent::NodeMoved(node) => {
                self.index_name(&node).await;
                let owner = node.owner_id;
                self.hub
                    .publish(owner, Event::NodeUpdated { node: node.into() });
            }
            DomainEvent::NodeDeleted(node) => self.hub.publish(
                node.owner_id,
                Event::NodeDeleted {
                    id: node.id,
                    parent_id: node.parent_id,
                },
            ),
            DomainEvent::ContentChanged { owner, node } => {
                self.search.enqueue_content(owner, node);
            }
            DomainEvent::TrashChanged { owner } => self.hub.publish(owner, Event::TrashChanged),
            DomainEvent::DevicesChanged { user } => self.hub.publish(user, Event::DevicesChanged),
            DomainEvent::ProtectionChanged { owner, node, state } => self.hub.publish(
                owner,
                Event::ProtectionChanged {
                    node_id: node,
                    state,
                },
            ),
            DomainEvent::TransferReady(transfer) => self.transfer_ready(transfer),
            DomainEvent::TransferChanged {
                id,
                sender,
                recipient,
                state,
            } => {
                self.hub
                    .publish(sender, Event::TransferChanged { id, state });
                if recipient != sender {
                    self.hub
                        .publish(recipient, Event::TransferChanged { id, state });
                }
            }
        }
    }

    /// Tells the recipient a transfer is ready, and the sender that it is.
    fn transfer_ready(&self, transfer: db::Transfer) {
        let (id, sender, recipient) = (transfer.id, transfer.sender_id, transfer.recipient_id);
        self.hub.publish(
            recipient,
            Event::TransferReceived {
                transfer: summary(transfer),
            },
        );
        if sender != recipient {
            let state = TransferState::Ready;
            self.hub
                .publish(sender, Event::TransferChanged { id, state });
        }
    }

    /// Indexes a node's name. A failure is logged: the change itself succeeded, and the next
    /// catch-up pass repairs the index.
    async fn index_name(&self, node: &db::Node) {
        if let Err(err) = self.search.index_name(node).await {
            tracing::warn!(node_id = %node.id, error = %err, "failed to index node name");
        }
    }
}

impl EventSink for EventDispatcher {
    fn publish(&self, events: Vec<DomainEvent>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            for event in events {
                self.dispatch(event).await;
            }
        })
    }
}
