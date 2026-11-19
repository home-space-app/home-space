//! Transfers as the database stores them, and the values that create one.

use crate::{Node, NodeId, NodeKind, UserId};
use protocol::TransferState;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

/// A transfer with the names of the users and devices it is between.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct Transfer {
    pub id: Uuid,
    pub sender_id: UserId,
    pub sender_name: String,
    pub sender_device: Option<Uuid>,
    pub sender_device_name: Option<String>,
    pub recipient_id: UserId,
    pub recipient_name: String,
    pub recipient_device: Uuid,
    pub recipient_device_name: String,
    pub state: TransferState,
    pub total_bytes: i64,
    pub item_count: i32,
    pub file_count: i64,
    /// The path of the only top-level item, when there is one.
    pub name: Option<String>,
    /// The media type of the only item, when the transfer is a single file.
    pub mime: Option<String>,
    pub message: Option<String>,
    pub created_at: OffsetDateTime,
    pub ready_at: Option<OffsetDateTime>,
    pub expires_at: OffsetDateTime,
    pub finished_at: Option<OffsetDateTime>,
}

/// A file or folder of a transfer.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct TransferItem {
    pub id: Uuid,
    pub transfer_id: Uuid,
    pub path: String,
    pub kind: NodeKind,
    pub size: i64,
    /// Set once a file's bytes are in.
    pub sha256: Option<Vec<u8>>,
    pub mime: Option<String>,
    pub modified_at: Option<OffsetDateTime>,
    pub node_id: Option<NodeId>,
}

/// A transfer to record.
#[derive(Clone, Debug)]
pub struct NewTransfer {
    pub id: Uuid,
    pub sender: UserId,
    pub sender_device: Option<Uuid>,
    pub recipient: UserId,
    pub recipient_device: Uuid,
    pub message: Option<String>,
    pub expires_at: OffsetDateTime,
    /// Files with `sha256` set already have their bytes in place.
    pub items: Vec<TransferItem>,
}

/// Which of a recipient's waiting transfers to list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncomingScope {
    /// Only those sent to this device.
    ToDevice(Uuid),
    /// Those sent to any device, except the ones this device sent: a device that sends to
    /// another one of the user's lists the transfer as sent, not as received.
    AllExceptFrom(Option<Uuid>),
}

/// A transfer removed because its time ran out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpiredTransfer {
    pub id: Uuid,
    pub sender_id: UserId,
    pub recipient_id: UserId,
}

/// A node of a subtree with its path below the subtree's root (`""` for the root itself,
/// `"/a/b"` below it).
#[derive(Clone, Debug, PartialEq, FromRow)]
pub struct SubtreeNode {
    #[sqlx(flatten)]
    pub node: Node,
    pub rel: String,
}
