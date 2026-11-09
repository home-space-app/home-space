use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::UserId;

/// Platform of a desktop client on Linux.
pub const PLATFORM_LINUX: &str = "desktop-linux";
/// Platform of a desktop client on macOS.
pub const PLATFORM_MACOS: &str = "desktop-macos";
/// Platform of a desktop client on Windows.
pub const PLATFORM_WINDOWS: &str = "desktop-windows";
/// Platform of the web client.
pub const PLATFORM_WEB: &str = "web";
/// Platform of the Android app.
pub const PLATFORM_ANDROID: &str = "android";
/// Platform of the iOS app.
pub const PLATFORM_IOS: &str = "ios";

/// The device a client signs in from. The client generates `id` once and keeps it, so the
/// server recognises the device across sign-ins.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: Uuid,
    /// The name shown in the device list until the user renames it, such as the host name.
    pub name: String,
    /// One of the `PLATFORM_*` values, or another short name.
    pub platform: String,
}

/// A device of the user, from `GET /devices`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: Uuid,
    pub name: String,
    pub platform: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen_at: OffsetDateTime,
    /// Whether the device has a session that has not ended.
    pub signed_in: bool,
    /// Whether the device has the events websocket open right now.
    #[serde(default)]
    pub online: bool,
    /// Whether this is the device that asked.
    pub current: bool,
    /// How many resumable uploads it has left unfinished.
    pub uploads: i64,
    /// Who can see the device and send files to it.
    #[serde(default)]
    pub visibility: DeviceVisibility,
    /// With [`DeviceVisibility::Users`], the other users who can.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visible_to: Vec<UserRef>,
}

/// Body of `PATCH /devices/{id}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameDevice {
    pub name: String,
}

/// Who can see a device and send files to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "server",
    derive(sqlx::Type),
    sqlx(type_name = "text", rename_all = "lowercase")
)]
#[serde(rename_all = "lowercase")]
pub enum DeviceVisibility {
    /// Only the user the device belongs to.
    #[default]
    Owner,
    /// Nobody, the owner included: the device cannot receive files.
    Hidden,
    /// The owner and every other user.
    Everyone,
    /// The owner and the users named in [`SetDeviceVisibility::users`].
    Users,
}

/// Another user, as shown in pickers.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "server", derive(sqlx::FromRow))]
pub struct UserRef {
    pub id: UserId,
    pub username: String,
}

/// Body of `PUT /devices/{id}/visibility`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetDeviceVisibility {
    pub visibility: DeviceVisibility,
    /// With [`DeviceVisibility::Users`], the other users who may see the device; ignored
    /// otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<UserId>,
}

/// A device files can be sent to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetDevice {
    pub id: Uuid,
    pub name: String,
    pub platform: String,
    /// Whether the device has a session that has not ended.
    pub signed_in: bool,
    /// Whether the device has the events websocket open right now.
    pub online: bool,
    /// Whether this is the device that asked.
    #[serde(default)]
    pub current: bool,
}

/// A user and their devices the caller can send files to, from `GET /devices/targets`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferTarget {
    pub user: UserRef,
    /// Whether these are the caller's own devices.
    pub own: bool,
    pub devices: Vec<TargetDevice>,
}

#[cfg(test)]
mod tests {
    use crate::LoginRequest;

    #[test]
    fn a_login_without_a_device_is_still_accepted() {
        let request: LoginRequest =
            serde_json::from_str(r#"{"username":"alice","password":"pw"}"#).expect("json");
        assert_eq!(request.device, None);
    }
}
