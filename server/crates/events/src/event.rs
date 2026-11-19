use uuid::Uuid;

use db::{Node, NodeId, Transfer, UserId};
use protocol::TransferState;

/// One change a service made.
#[derive(Clone, Debug, PartialEq)]
pub enum DomainEvent {
    /// A folder or file was created; a file's content is new as well.
    NodeCreated(Node),
    /// A node changed in place, keeping its name and folder.
    NodeUpdated(Node),
    /// A node was renamed, moved or restored, so it is found under its new name.
    NodeMoved(Node),
    /// A node left the owner's tree: to the recycle bin, or replaced on a move.
    NodeDeleted(Node),
    /// A file's content changed, so its text is extracted again.
    ContentChanged { owner: UserId, node: NodeId },
    /// Files entered or left the owner's recycle bin.
    TrashChanged { owner: UserId },
    /// The devices `user` sees changed: one was renamed, signed out or shown to them.
    DevicesChanged { user: UserId },
    /// A transfer's files are all uploaded and it can be received.
    TransferReady(Transfer),
    /// A transfer was received, declined, cancelled, saved or expired.
    TransferChanged {
        id: Uuid,
        sender: UserId,
        recipient: UserId,
        state: TransferState,
    },
}

impl DomainEvent {
    /// The change of `transfer` to its current state.
    pub fn transfer_changed(transfer: &Transfer) -> Self {
        Self::TransferChanged {
            id: transfer.id,
            sender: transfer.sender_id,
            recipient: transfer.recipient_id,
            state: transfer.state,
        }
    }
}
