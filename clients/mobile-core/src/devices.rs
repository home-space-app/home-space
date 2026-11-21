//! The devices of the signed-in user: renaming them, choosing who can see them, signing them
//! out and removing them.

use std::time::SystemTime;

use client_core::{
    labels::{REMOVE_DEVICE_HINT, remove_question, sign_out_question},
    transfers::{VISIBILITIES, visibility_hint, visibility_title, visible_to},
};
use protocol::{Device, DeviceVisibility, SetDeviceVisibility, UserId, UserRef};
use uuid::Uuid;

use crate::{CoreClient, CoreError, MAX_DEVICE_NAME_CHARS, off_ui_thread};

const INVALID_CODE: &str = "invalid";
const EMPTY_NAME: &str = "A device needs a name.";
const NOBODY_PICKED: &str = "Pick at least one user.";

/// Who can see a device and send files to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DeviceAudience {
    /// Only the user the device belongs to.
    Owner,
    /// Nobody, the owner included.
    Hidden,
    /// The owner and the users picked.
    Users,
    /// Everyone with an account on the server.
    Everyone,
}

impl From<DeviceVisibility> for DeviceAudience {
    fn from(visibility: DeviceVisibility) -> Self {
        match visibility {
            DeviceVisibility::Owner => Self::Owner,
            DeviceVisibility::Hidden => Self::Hidden,
            DeviceVisibility::Users => Self::Users,
            DeviceVisibility::Everyone => Self::Everyone,
        }
    }
}

impl From<DeviceAudience> for DeviceVisibility {
    fn from(audience: DeviceAudience) -> Self {
        match audience {
            DeviceAudience::Owner => Self::Owner,
            DeviceAudience::Hidden => Self::Hidden,
            DeviceAudience::Users => Self::Users,
            DeviceAudience::Everyone => Self::Everyone,
        }
    }
}

/// One choice of the visibility dialog, in the order the dialog lists them.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AudienceChoice {
    pub audience: DeviceAudience,
    /// E.g. `Only you`.
    pub title: String,
    /// What it means, under the title.
    pub hint: String,
}

/// Another user of the server.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OtherUser {
    pub id: String,
    pub username: String,
}

impl From<UserRef> for OtherUser {
    fn from(user: UserRef) -> Self {
        Self {
            id: user.id.to_string(),
            username: user.username,
        }
    }
}

/// A device of the signed-in user.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct UserDevice {
    pub id: String,
    pub name: String,
    /// One of `protocol`'s `PLATFORM_*` values, or another short name.
    pub platform: String,
    /// When the server last heard from it.
    pub last_seen_at: SystemTime,
    pub signed_in: bool,
    /// Whether it is connected to the server right now.
    pub online: bool,
    /// Whether this is the device that asked.
    pub current: bool,
    pub audience: DeviceAudience,
    /// With [`DeviceAudience::Users`], the other users who can see it.
    pub visible_to: Vec<OtherUser>,
    /// `Only you`, `Hidden`, `Everyone` or the names of the users.
    pub audience_label: String,
    /// Whether it can be signed out from the list: not the current device, which signs out
    /// with the account, and not one that is signed out with nothing left unfinished.
    pub can_sign_out: bool,
    /// Asked before signing it out, saying what that cancels.
    pub sign_out_question: String,
    /// Asked before removing it, saying what that cancels; see [`remove_device_hint`].
    pub remove_question: String,
}

impl From<Device> for UserDevice {
    fn from(device: Device) -> Self {
        let audience_label = visible_to(&device);
        let can_sign_out = !device.current && (device.signed_in || device.uploads > 0);
        let sign_out_question = sign_out_question(&device);
        let remove_question = remove_question(&device);
        Self {
            id: device.id.to_string(),
            name: device.name,
            platform: device.platform,
            last_seen_at: device.last_seen_at.into(),
            signed_in: device.signed_in,
            online: device.online,
            current: device.current,
            audience: device.visibility.into(),
            visible_to: device.visible_to.into_iter().map(OtherUser::from).collect(),
            audience_label,
            can_sign_out,
            sign_out_question,
            remove_question,
        }
    }
}

/// The choices of the visibility dialog.
#[uniffi::export]
pub fn audience_choices() -> Vec<AudienceChoice> {
    VISIBILITIES
        .into_iter()
        .map(|visibility| AudienceChoice {
            audience: visibility.into(),
            title: visibility_title(visibility).to_owned(),
            hint: visibility_hint(visibility).to_owned(),
        })
        .collect()
}

/// What removing a device does, under its `remove_question`.
#[uniffi::export]
pub fn remove_device_hint() -> String {
    REMOVE_DEVICE_HINT.to_owned()
}

#[uniffi::export]
impl CoreClient {
    /// The user's devices, the most recently seen first.
    pub async fn devices(&self) -> Result<Vec<UserDevice>, CoreError> {
        let inner = self.inner.clone();
        let devices = off_ui_thread(async move { inner.devices().await }).await?;
        Ok(devices.into_iter().map(UserDevice::from).collect())
    }

    /// Renames one of the user's devices. The name is trimmed and cut to the server's limit;
    /// fails without asking the server when nothing is left of it.
    pub async fn rename_device(&self, id: String, name: String) -> Result<(), CoreError> {
        let id = device_id(&id)?;
        let name = device_name(&name)?;
        let inner = self.inner.clone();
        off_ui_thread(async move { inner.rename_device(id, &name).await }).await?;
        Ok(())
    }

    /// The other users of the server, who a device can be made visible to.
    pub async fn other_users(&self) -> Result<Vec<OtherUser>, CoreError> {
        let inner = self.inner.clone();
        let users = off_ui_thread(async move { inner.users().await }).await?;
        Ok(users.into_iter().map(OtherUser::from).collect())
    }

    /// Sets who can see one of the user's devices. `user_ids` count only with
    /// [`DeviceAudience::Users`], which needs at least one.
    pub async fn set_device_audience(
        &self,
        id: String,
        audience: DeviceAudience,
        user_ids: Vec<String>,
    ) -> Result<(), CoreError> {
        let id = device_id(&id)?;
        let request = visibility_request(audience, &user_ids)?;
        let inner = self.inner.clone();
        off_ui_thread(async move { inner.set_device_visibility(id, &request).await }).await?;
        Ok(())
    }

    /// Signs one of the user's devices out: ends its sessions and cancels its unfinished
    /// uploads. It stays on the list, signed out.
    pub async fn sign_out_device(&self, id: String) -> Result<(), CoreError> {
        let id = device_id(&id)?;
        let inner = self.inner.clone();
        off_ui_thread(async move { inner.sign_out_device(id).await }).await?;
        Ok(())
    }

    /// Signs one of the user's devices out and takes it off the list; the files sent to it are
    /// declined. It comes back if it signs in again.
    pub async fn remove_device(&self, id: String) -> Result<(), CoreError> {
        let id = device_id(&id)?;
        let inner = self.inner.clone();
        off_ui_thread(async move { inner.remove_device(id).await }).await?;
        Ok(())
    }
}

fn device_id(id: &str) -> Result<Uuid, CoreError> {
    id.parse()
        .map_err(|_| CoreError::InvalidId { id: id.to_owned() })
}

fn device_name(name: &str) -> Result<String, CoreError> {
    let name: String = name.trim().chars().take(MAX_DEVICE_NAME_CHARS).collect();
    if name.is_empty() {
        Err(invalid(EMPTY_NAME))
    } else {
        Ok(name)
    }
}

fn visibility_request(
    audience: DeviceAudience,
    user_ids: &[String],
) -> Result<SetDeviceVisibility, CoreError> {
    let users = match audience {
        DeviceAudience::Users => user_ids
            .iter()
            .map(|id| {
                id.parse::<UserId>()
                    .map_err(|_| CoreError::InvalidId { id: id.clone() })
            })
            .collect::<Result<Vec<_>, _>>()?,
        DeviceAudience::Owner | DeviceAudience::Hidden | DeviceAudience::Everyone => Vec::new(),
    };
    if audience == DeviceAudience::Users && users.is_empty() {
        Err(invalid(NOBODY_PICKED))
    } else {
        Ok(SetDeviceVisibility {
            visibility: audience.into(),
            users,
        })
    }
}

fn invalid(detail: &str) -> CoreError {
    CoreError::Failed {
        code: INVALID_CODE.to_owned(),
        detail: detail.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use protocol::paths;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, method, path},
    };

    use super::*;
    use crate::tests::signed_in;

    const DEVICE: &str = "00000000-0000-0000-0000-00000000000b";
    const USER: &str = "00000000-0000-0000-0000-0000000000c1";

    fn device_id() -> Uuid {
        DEVICE.parse().unwrap()
    }

    #[tokio::test]
    async fn a_rename_sends_the_trimmed_name() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(paths::device(device_id())))
            .and(body_json(serde_json::json!({ "name": "Laptop" })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let client = signed_in(&server).await;

        client
            .rename_device(DEVICE.into(), "  Laptop  ".into())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_blank_name_is_refused_before_the_server_hears_of_it() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;

        let err = client
            .rename_device(DEVICE.into(), "   ".into())
            .await
            .unwrap_err();

        assert!(matches!(err, CoreError::Failed { code, .. } if code == INVALID_CODE));
    }

    #[tokio::test]
    async fn visible_to_users_sends_the_users_picked() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(paths::device_visibility(device_id())))
            .and(body_json(serde_json::json!({
                "visibility": "users", "users": [USER],
            })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let client = signed_in(&server).await;

        client
            .set_device_audience(DEVICE.into(), DeviceAudience::Users, vec![USER.into()])
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn removing_a_device_asks_the_server_to_forget_it() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(paths::device(device_id())))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let client = signed_in(&server).await;

        client.remove_device(DEVICE.into()).await.unwrap();
    }

    #[tokio::test]
    async fn signing_a_device_out_keeps_it_on_the_server() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(paths::device_sessions(device_id())))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let client = signed_in(&server).await;

        client.sign_out_device(DEVICE.into()).await.unwrap();
    }

    #[test]
    fn the_users_picked_count_only_when_visible_to_users() {
        let request = visibility_request(DeviceAudience::Everyone, &[USER.into()]).unwrap();

        assert_eq!(request.visibility, DeviceVisibility::Everyone);
        assert!(request.users.is_empty());
    }

    #[test]
    fn visible_to_users_with_nobody_picked_is_refused() {
        let err = visibility_request(DeviceAudience::Users, &[]).unwrap_err();

        assert!(matches!(err, CoreError::Failed { code, .. } if code == INVALID_CODE));
    }

    #[tokio::test]
    async fn devices_say_who_can_see_them() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(paths::DEVICES))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "id": DEVICE,
                    "name": "Laptop",
                    "platform": "desktop-linux",
                    "created_at": "2026-10-01T00:00:00Z",
                    "last_seen_at": "2026-11-03T22:10:00Z",
                    "signed_in": true,
                    "online": false,
                    "current": false,
                    "uploads": 0,
                    "visibility": "users",
                    "visible_to": [{ "id": USER, "username": "sam" }],
                }])),
            )
            .mount(&server)
            .await;
        let client = signed_in(&server).await;

        let devices = client.devices().await.unwrap();

        let device = &devices[0];
        assert_eq!(device.audience, DeviceAudience::Users);
        assert_eq!(device.audience_label, "sam");
        assert_eq!(device.visible_to[0].username, "sam");
        assert!(device.can_sign_out);
        assert_eq!(device.remove_question, "Remove “Laptop”?");
    }
}
