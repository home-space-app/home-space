//! Shares as the database stores them, and the values that create or change one.

use crate::{SharePermission, UserId};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

/// A share reached through a link, with the name of its owner.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct ShareLink {
    pub id: Uuid,
    pub owner_id: UserId,
    pub owner_name: String,
    pub token: String,
    pub password_hash: Option<String>,
    pub password_version: i32,
    pub permission: SharePermission,
    pub expires_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    /// Shared items that are not in the recycle bin.
    pub item_count: i64,
}

impl ShareLink {
    /// Whether the link stopped working at `now`.
    pub fn is_expired(&self, now: OffsetDateTime) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }

    /// What the link reaches.
    pub fn root(&self) -> SharedRoot {
        SharedRoot {
            id: self.id,
            owner_id: self.owner_id,
        }
    }
}

/// A share as far as reaching its nodes goes: which share, and whose tree it is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SharedRoot {
    pub id: Uuid,
    pub owner_id: UserId,
}

/// A share another user made for the reader, with the name of its owner.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct ReceivedShare {
    pub id: Uuid,
    pub owner_id: UserId,
    pub owner_name: String,
    pub expires_at: Option<OffsetDateTime>,
    /// When the reader was added to the share.
    pub shared_at: OffsetDateTime,
}

impl ReceivedShare {
    /// Whether the share stopped working at `now`.
    pub fn is_expired(&self, now: OffsetDateTime) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }

    /// What the share reaches.
    pub fn root(&self) -> SharedRoot {
        SharedRoot {
            id: self.id,
            owner_id: self.owner_id,
        }
    }
}

/// A share as its owner sees it: through a link, or with named users.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct OwnedShare {
    pub id: Uuid,
    /// `None` for a share with named users.
    pub token: Option<String>,
    pub has_password: bool,
    pub permission: SharePermission,
    pub expires_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

impl From<ShareLink> for OwnedShare {
    fn from(link: ShareLink) -> Self {
        Self {
            id: link.id,
            has_password: link.password_hash.is_some(),
            token: Some(link.token),
            permission: link.permission,
            expires_at: link.expires_at,
            created_at: link.created_at,
        }
    }
}

/// A user a share is for.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct ShareRecipient {
    pub share_id: Uuid,
    pub user_id: UserId,
    pub username: String,
    pub permission: SharePermission,
    /// When the user was added to the share.
    pub created_at: OffsetDateTime,
}

/// Values for a share with named users to insert.
#[derive(Clone, Copy, Debug)]
pub struct NewUserShare {
    pub id: Uuid,
    pub expires_at: Option<OffsetDateTime>,
}

/// Values for a share link to insert.
#[derive(Clone, Copy, Debug)]
pub struct NewShareLink<'a> {
    pub id: Uuid,
    pub token: &'a str,
    pub password_hash: Option<&'a str>,
    pub permission: SharePermission,
    pub expires_at: Option<OffsetDateTime>,
}

/// Changes to a share; `None` leaves a value as it is.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShareUpdate<'a> {
    /// The password of a link; `Some(None)` removes it. Ignored for a share without a link.
    pub password_hash: Option<Option<&'a str>>,
    /// `Some(None)` makes the share never expire.
    pub expires_at: Option<Option<OffsetDateTime>>,
    /// Everyone the share is for from now on; users already in it keep when they were added.
    pub recipients: Option<&'a [UserId]>,
}
