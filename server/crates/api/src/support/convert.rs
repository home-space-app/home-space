//! Turning what the database and services return into what the API sends (D4 in
//! docs/todo/server-refactoring.md). Numbers the database keeps signed are clamped here, and
//! nowhere else.

use files::MoveBatch;
use protocol::{
    Account, AuthMethod, Conflict, MoveResponse, Node, PublicShare, ReceivedShare, Share,
    ShareRecipient, SharedNode, StorageUsage, TransferItem, TransferSummary, UserRef,
    encoding::to_hex,
};
use shares::{LinkView, ReceivedDetails, ShareDetails};

pub(crate) fn shared_node(node: db::Node) -> SharedNode {
    SharedNode {
        id: node.id,
        kind: node.kind,
        name: node.name,
        size: node.size,
        mime: node.mime,
        updated_at: node.updated_at,
    }
}

/// Converts the outcome of a move or restore to its wire form.
pub(crate) fn move_response(batch: MoveBatch) -> MoveResponse {
    MoveResponse {
        moved: batch.moved.into_iter().map(Node::from).collect(),
        skipped: batch.skipped.into_iter().map(|node| node.id).collect(),
        conflicts: batch
            .conflicts
            .into_iter()
            .map(|conflict| Conflict {
                path: conflict.path,
                source: Node::from(conflict.source),
                existing: conflict.existing.map(Node::from),
            })
            .collect(),
        target: batch.target.map(Node::from),
    }
}

/// Converts the database's sums, which are never negative.
pub(crate) fn usage(usage: db::StorageUsage) -> StorageUsage {
    StorageUsage {
        files: usage.files.max(0).unsigned_abs(),
        versions: usage.versions.max(0).unsigned_abs(),
        trash: usage.trash.max(0).unsigned_abs(),
    }
}

pub(crate) fn summary(transfer: db::Transfer) -> TransferSummary {
    TransferSummary {
        id: transfer.id,
        sender: UserRef {
            id: transfer.sender_id,
            username: transfer.sender_name,
        },
        sender_device: transfer.sender_device,
        sender_device_name: transfer.sender_device_name,
        recipient: UserRef {
            id: transfer.recipient_id,
            username: transfer.recipient_name,
        },
        recipient_device: transfer.recipient_device,
        recipient_device_name: transfer.recipient_device_name,
        state: transfer.state,
        total_bytes: u64::try_from(transfer.total_bytes).unwrap_or_default(),
        item_count: u32::try_from(transfer.item_count).unwrap_or_default(),
        file_count: u32::try_from(transfer.file_count).unwrap_or_default(),
        name: transfer.name,
        mime: transfer.mime,
        message: transfer.message,
        created_at: transfer.created_at,
        expires_at: transfer.expires_at,
    }
}

pub(crate) fn transfer_item(item: db::TransferItem) -> TransferItem {
    TransferItem {
        id: item.id,
        path: item.path,
        kind: item.kind,
        size: u64::try_from(item.size).unwrap_or_default(),
        sha256: item.sha256.as_deref().map(to_hex),
        mime: item.mime,
        modified_at: item.modified_at,
    }
}

pub(crate) fn account(found: db::Account) -> Account {
    Account {
        id: found.id,
        username: found.username,
        is_admin: found.is_admin,
        external: found.external,
        two_factor: found.authenticator_app || found.webauthn,
        factors: [
            (found.authenticator_app, AuthMethod::Totp),
            (found.webauthn, AuthMethod::Webauthn),
        ]
        .into_iter()
        .filter_map(|(has, method)| has.then_some(method))
        .collect(),
        created_at: found.created_at,
        disabled: found.disabled_at.is_some(),
        quota_bytes: found.quota_bytes.map(to_u64),
        delete_at: found.delete_at,
        usage: usage(found.usage),
        devices: u32::try_from(found.devices).unwrap_or(u32::MAX),
        last_seen_at: found.last_seen_at,
    }
}

fn to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

/// The owner's view of a share, with its live items and recipients.
pub(crate) fn owner_share(details: ShareDetails) -> Share {
    let ShareDetails {
        share,
        items,
        recipients,
    } = details;
    Share {
        id: share.id,
        items: items.into_iter().map(Node::from).collect(),
        has_password: share.has_password,
        permission: share.permission,
        token: share.token,
        expires_at: share.expires_at,
        created_at: share.created_at,
        recipients: recipients.into_iter().map(share_recipient).collect(),
    }
}

/// A link as its visitors see it.
pub(crate) fn public_share(view: LinkView) -> PublicShare {
    PublicShare {
        needs_password: view.items.is_none(),
        items: view
            .items
            .map(|items| items.into_iter().map(shared_node).collect()),
        owner: view.link.owner_name,
        item_count: u32::try_from(view.link.item_count).unwrap_or(u32::MAX),
        expires_at: view.link.expires_at,
    }
}

/// A share made for the caller, with its live items.
pub(crate) fn received_share(details: ReceivedDetails) -> ReceivedShare {
    let share = details.share;
    ReceivedShare {
        id: share.id,
        owner: UserRef {
            id: share.owner_id,
            username: share.owner_name,
        },
        items: details.items.into_iter().map(shared_node).collect(),
        expires_at: share.expires_at,
        shared_at: share.shared_at,
    }
}

fn share_recipient(recipient: db::ShareRecipient) -> ShareRecipient {
    ShareRecipient {
        user: UserRef {
            id: recipient.user_id,
            username: recipient.username,
        },
        permission: recipient.permission,
        added_at: recipient.created_at,
    }
}
