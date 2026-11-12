//! Sending files and folders to a device (`/transfers`).
//!
//! The sender creates a transfer with its whole manifest ([`CreateTransfer`]). Items taken from
//! the sender's tree (`node_id`) are copied at once; the others are uploaded with
//! `POST /uploads` and [`CreateUpload::transfer_item_id`](crate::CreateUpload::transfer_item_id).
//! Once every file has its bytes the transfer is [`TransferState::Ready`] and the recipient is
//! notified. The recipient downloads it and confirms with `POST /transfers/{id}/received`, saves
//! it into their tree with `POST /transfers/{id}/save`, or declines it.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{NodeId, NodeKind, UserRef};

/// Where a transfer stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "server",
    derive(sqlx::Type),
    sqlx(type_name = "text", rename_all = "lowercase")
)]
#[serde(rename_all = "lowercase")]
pub enum TransferState {
    /// Files from the sender's disk are still being uploaded.
    Uploading,
    /// Every file is in; the recipient can download or save it.
    Ready,
    /// The recipient declined it.
    Declined,
    /// The recipient downloaded it and confirmed.
    Downloaded,
    /// The recipient saved it into their tree.
    Saved,
    /// The sender cancelled it.
    Cancelled,
    /// It was removed after its time ran out. Only sent in [`crate::Event::TransferChanged`].
    Expired,
}

impl TransferState {
    /// Whether nothing more can happen to the transfer.
    pub fn is_finished(self) -> bool {
        !matches!(self, Self::Uploading | Self::Ready)
    }
}

/// One file or folder of a new transfer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTransferItem {
    /// Where it goes on the recipient's side, relative and `/`-separated: `Photos/2026/a.jpg`.
    /// Every folder in it must be an item of the transfer too.
    pub path: String,
    /// Ignored when `node_id` is set.
    pub kind: NodeKind,
    /// Size of a file in bytes; ignored for folders and when `node_id` is set.
    #[serde(default)]
    pub size: u64,
    /// A file or folder of the sender's tree to send; a folder brings everything in it.
    /// Without it, a file's bytes are uploaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub modified_at: Option<OffsetDateTime>,
}

/// Body of `POST /transfers`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTransfer {
    pub recipient_id: crate::UserId,
    pub recipient_device: Uuid,
    pub items: Vec<NewTransferItem>,
    /// An optional note for the recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// An item of a created transfer, as `POST /transfers` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedTransferItem {
    pub id: Uuid,
    pub path: String,
    /// Whether its bytes must still be uploaded with `POST /uploads`.
    pub upload_needed: bool,
}

/// Response of `POST /transfers`. Items from the sender's tree are expanded, so there can be
/// more items than were sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedTransfer {
    pub id: Uuid,
    pub state: TransferState,
    pub items: Vec<CreatedTransferItem>,
}

/// A transfer without its items, as lists and events carry it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferSummary {
    pub id: Uuid,
    pub sender: UserRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_device: Option<Uuid>,
    /// Name of the sending device, while the server still knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_device_name: Option<String>,
    pub recipient: UserRef,
    pub recipient_device: Uuid,
    pub recipient_device_name: String,
    pub state: TransferState,
    pub total_bytes: u64,
    /// Files and folders, counting everything inside folders.
    pub item_count: u32,
    /// Files only.
    #[serde(default)]
    pub file_count: u32,
    /// The path of the one file or folder at the top of the transfer, when there is only one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The media type of the only item, when the transfer is a single file: a client can tell a
    /// list of links (`text/uri-list`) from a file without fetching the items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// When it is removed, whatever its state.
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

/// A file or folder of a transfer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferItem {
    pub id: Uuid,
    pub path: String,
    pub kind: NodeKind,
    pub size: u64,
    /// Hex SHA-256 of a file's bytes, once they are in; check it after downloading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub modified_at: Option<OffsetDateTime>,
}

/// A transfer with its items, from `GET /transfers/{id}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    #[serde(flatten)]
    pub summary: TransferSummary,
    pub items: Vec<TransferItem>,
}

/// Query of `GET /transfers/incoming`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncomingQuery {
    /// Only the transfers sent to the device that asks.
    #[serde(default)]
    pub this_device: bool,
}

/// Body of `POST /transfers/{id}/save`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveTransfer {
    /// The folder to save into; the root when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<NodeId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_item_needs_only_a_path_and_a_kind() {
        let item: NewTransferItem =
            serde_json::from_str(r#"{"path":"a.txt","kind":"file","size":3}"#).expect("json");
        assert_eq!(item.node_id, None);
        assert_eq!(item.size, 3);
    }

    #[test]
    fn states_are_lowercase() {
        assert_eq!(
            serde_json::to_string(&TransferState::Downloaded).expect("json"),
            r#""downloaded""#
        );
        assert!(TransferState::Declined.is_finished());
        assert!(!TransferState::Ready.is_finished());
    }
}
