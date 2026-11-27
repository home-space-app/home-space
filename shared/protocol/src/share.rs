//! Read-only shares (`/shares`, `/shared`, `/public/shares/{token}`).
//!
//! A share is either for named users, who read it once signed in through `/shared`, or a link. The owner makes a link to one or more of their files and folders with `POST /shares`.
//! Anyone with the link reads the shared items through `/public/shares/{token}` without
//! signing in. A link with a password answers only its metadata until it is unlocked with
//! `POST /public/shares/{token}/unlock`, whose access token goes in the
//! [`SHARE_ACCESS`](crate::paths::SHARE_ACCESS) header, or the `access` query parameter where
//! headers cannot be sent. An expired link answers 410 Gone, a removed one 404.

use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    Node, NodeId, NodeKind, Patch, UserId, UserRef,
    redact::{Redacted, redacted},
};

/// Body of `POST /shares`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateShare {
    /// The owner's files and folders to share; folders are shared with everything inside.
    pub node_ids: Vec<NodeId>,
    /// Asked for before anything is shown; `None` for no password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// When the link stops working; `None` for never.
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub expires_at: Option<OffsetDateTime>,
    /// The users to share with; empty for a link. A share with users has no password.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recipients: Vec<UserId>,
}

impl fmt::Debug for CreateShare {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateShare")
            .field("node_ids", &self.node_ids)
            .field("password", &redacted(self.password.as_ref()))
            .field("expires_at", &self.expires_at)
            .field("recipients", &self.recipients)
            .finish()
    }
}

/// Body of `PATCH /shares/{id}`. A missing field is left as it is; `null` removes the
/// password or the expiry.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateShare {
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub password: Patch<String>,
    #[serde(
        default,
        skip_serializing_if = "Patch::is_keep",
        with = "crate::patch::rfc3339"
    )]
    pub expires_at: Patch<OffsetDateTime>,
    /// Everyone a share with users is for from now on; never empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipients: Option<Vec<UserId>>,
}

impl fmt::Debug for UpdateShare {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let password = match self.password {
            Patch::Keep => Patch::Keep,
            Patch::Clear => Patch::Clear,
            Patch::Set(_) => Patch::Set(Redacted),
        };
        f.debug_struct("UpdateShare")
            .field("password", &password)
            .field("expires_at", &self.expires_at)
            .field("recipients", &self.recipients)
            .finish()
    }
}

/// Query of `GET /shares`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharesQuery {
    /// Only the shares that include this node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<NodeId>,
}

/// What a share lets its recipients do with the shared items.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "server",
    derive(sqlx::Type),
    sqlx(type_name = "text", rename_all = "lowercase")
)]
#[serde(rename_all = "lowercase")]
pub enum SharePermission {
    /// See and download them.
    #[default]
    Read,
}

/// A user a share is for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareRecipient {
    pub user: UserRef,
    #[serde(default)]
    pub permission: SharePermission,
    /// When the user was added to the share.
    #[serde(with = "time::serde::rfc3339")]
    pub added_at: OffsetDateTime,
}

/// A share, as its owner sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    pub id: Uuid,
    /// The secret part of the link, see [`paths::web_share`](crate::paths::web_share); `None`
    /// for a share with users.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// The shared items that are not in the recycle bin.
    pub items: Vec<Node>,
    pub has_password: bool,
    #[serde(default)]
    pub permission: SharePermission,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Who a share with users is for, by username; empty for a link.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recipients: Vec<ShareRecipient>,
}

impl Share {
    /// Whether anyone with a link reads it, rather than named users.
    pub fn is_link(&self) -> bool {
        self.token.is_some()
    }
}

/// A file or folder seen through a share link: only what a visitor needs to see.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedNode {
    pub id: NodeId,
    pub kind: NodeKind,
    pub name: String,
    pub size: i64,
    pub mime: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// `GET /public/shares/{token}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicShare {
    /// The username of who shared it.
    pub owner: String,
    /// How many items are shared, known even before a password is given.
    pub item_count: u32,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// The link has a password and the request did not carry a valid access token.
    pub needs_password: bool,
    /// The shared items; `None` while a password is needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<SharedNode>>,
}

/// A share another user made for the caller: `GET /shared`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceivedShare {
    pub id: Uuid,
    /// Who shared it.
    pub owner: UserRef,
    /// The shared items that are not in the owner's recycle bin.
    pub items: Vec<SharedNode>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// When the caller was given the share.
    #[serde(with = "time::serde::rfc3339")]
    pub shared_at: OffsetDateTime,
}

/// Body of `POST /shared/{id}/copy`: puts copies of shared items into the caller's files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyShared {
    /// Shared items, or items inside shared folders; folders are copied with everything inside.
    pub node_ids: Vec<NodeId>,
    /// The caller's folder to copy into; their root when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<NodeId>,
}

/// Body of `POST /public/shares/{token}/unlock`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnlockShare {
    pub password: String,
}

impl fmt::Debug for UnlockShare {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnlockShare")
            .field("password", &Redacted)
            .finish()
    }
}

/// Answer of `POST /public/shares/{token}/unlock`: proof that the password was given. It stops
/// working when it expires or the owner changes the password.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareAccess {
    pub access: String,
    /// Seconds until it expires.
    pub expires_in: i64,
}

/// Query of `GET /public/shares/{token}/nodes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedFolderQuery {
    /// A shared folder, or a folder inside one.
    pub parent_id: NodeId,
}

/// Query of `GET /public/shares/{token}/archive`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareArchiveQuery {
    /// A folder inside the share to zip; the whole share when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<NodeId>,
}

/// Query naming the access token where the [`SHARE_ACCESS`](crate::paths::SHARE_ACCESS) header
/// cannot be sent, as in a download link.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareAccessQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_update_tells_a_missing_expiry_from_a_removed_one() {
        let missing: UpdateShare = serde_json::from_str("{}").expect("json");
        assert_eq!(missing.expires_at, Patch::Keep);
        let removed: UpdateShare = serde_json::from_str(r#"{"expires_at":null}"#).expect("json");
        assert_eq!(removed.expires_at, Patch::Clear);
    }

    #[test]
    fn an_update_reads_a_new_expiry() {
        let update: UpdateShare =
            serde_json::from_str(r#"{"expires_at":"2026-11-19T14:30:00Z"}"#).expect("json");
        let expected = OffsetDateTime::from_unix_timestamp(1_795_098_600).expect("time");
        assert_eq!(update.expires_at, Patch::Set(expected));
    }

    #[test]
    fn an_update_round_trips() {
        let update = UpdateShare {
            password: Patch::Clear,
            expires_at: Patch::Clear,
            recipients: None,
        };
        let json = serde_json::to_string(&update).expect("json");
        assert_eq!(json, r#"{"password":null,"expires_at":null}"#);
        assert_eq!(
            serde_json::from_str::<UpdateShare>(&json).expect("back"),
            update
        );
    }
}
