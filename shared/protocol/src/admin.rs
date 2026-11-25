//! Account management by administrators (`/admin/users`).

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{Patch, StorageUsage, UserId};

/// One account, as the administration lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: UserId,
    pub username: String,
    pub is_admin: bool,
    /// Signs in through the identity provider rather than with a password.
    pub external: bool,
    /// Signing in takes a second factor, such as an authenticator app; older servers leave it
    /// out.
    #[serde(default)]
    pub two_factor: bool,
    /// Which second factors: `totp` and `webauthn` (passkeys and security keys); older servers
    /// leave it out.
    #[serde(default)]
    pub factors: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// The account exists but may not sign in. Always so while it is marked for deletion.
    pub disabled: bool,
    /// The most the user should store; `None` for no quota. Not enforced yet.
    pub quota_bytes: Option<u64>,
    /// When the account and all its files will be deleted; `None` unless it is marked for
    /// deletion. A marked account may not sign in.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub delete_at: Option<OffsetDateTime>,
    pub usage: StorageUsage,
    /// Devices the user has signed in from.
    pub devices: u32,
    /// When one of those devices was last seen; `None` if none ever was.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub last_seen_at: Option<OffsetDateTime>,
}

impl Account {
    /// Whether the user may sign in: not disabled, which includes being marked for deletion.
    pub fn active(&self) -> bool {
        !self.disabled
    }
}

/// `POST /admin/users`: a new account that signs in with a password.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateAccount {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub is_admin: bool,
    #[serde(default)]
    pub quota_bytes: Option<u64>,
}

/// `PATCH /admin/users/{id}`: the fields to change; absent ones stay as they are.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateAccount {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_admin: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    /// [`Patch::Clear`] removes the quota; sent as `"quota_bytes": null`.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub quota_bytes: Patch<u64>,
}

/// `PUT /admin/users/{id}/password`: a new password for an account that has one. Ends all of
/// the user's sessions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetPassword {
    pub password: String,
}

/// A device of an account, for the administration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDevice {
    pub id: uuid::Uuid,
    pub name: String,
    pub platform: String,
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen_at: OffsetDateTime,
    pub signed_in: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_null_quota_removes_it_and_a_missing_one_keeps_it() {
        let remove: UpdateAccount = serde_json::from_str(r#"{"quota_bytes":null}"#).expect("json");
        assert_eq!(remove.quota_bytes, Patch::Clear);
        let keep: UpdateAccount = serde_json::from_str(r#"{"disabled":true}"#).expect("json");
        assert_eq!(keep.quota_bytes, Patch::Keep);
        assert_eq!(keep.disabled, Some(true));
    }

    #[test]
    fn removing_a_quota_is_sent_as_null() {
        let update = UpdateAccount {
            quota_bytes: Patch::Clear,
            ..UpdateAccount::default()
        };
        assert_eq!(
            serde_json::to_string(&update).expect("json"),
            r#"{"quota_bytes":null}"#
        );
    }
}
