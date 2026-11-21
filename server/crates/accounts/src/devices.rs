use std::collections::HashMap;

use uuid::Uuid;

use auth::AuthService;
use db::{Db, DbError, UserId};
use events::{DomainEvent, Events};
use files::UploadService;
use notifications::Hub;
use protocol::{
    Device, DeviceVisibility, RenameDevice, SetDeviceVisibility, TargetDevice, TransferTarget,
    UserRef,
};

use crate::AccountError;

/// The devices users sign in from: who sees them, and which are online.
#[derive(Clone, Debug)]
pub struct DeviceService {
    db: Db,
    auth: AuthService,
    /// Ends the uploads a signed-out device left unfinished.
    uploads: UploadService,
    /// Knows which devices have their websocket open.
    hub: Hub,
    events: Events,
}

impl DeviceService {
    pub fn new(
        db: Db,
        auth: AuthService,
        uploads: UploadService,
        hub: Hub,
        events: Events,
    ) -> Self {
        Self {
            db,
            auth,
            uploads,
            hub,
            events,
        }
    }

    /// The devices of `user`, marking `current` as the one that asks.
    pub async fn list(
        &self,
        user: UserId,
        current: Option<Uuid>,
    ) -> Result<Vec<Device>, AccountError> {
        let devices = self.db.devices(user).await?;
        let mut viewers: HashMap<Uuid, Vec<UserRef>> = HashMap::new();
        for (device, viewer) in self.db.device_viewers(user).await? {
            viewers.entry(device).or_default().push(viewer);
        }
        Ok(devices
            .into_iter()
            .map(|device| Device {
                visible_to: viewers.remove(&device.id).unwrap_or_default(),
                visibility: device.visibility,
                current: current == Some(device.id),
                online: self.hub.is_online(user, device.id),
                id: device.id,
                name: device.name,
                platform: device.platform,
                created_at: device.created_at,
                last_seen_at: device.last_seen_at,
                signed_in: device.signed_in,
                uploads: device.uploads,
            })
            .collect())
    }

    /// The devices `user` can send files to, grouped by their owner: `user`'s own first.
    pub async fn targets(
        &self,
        user: UserId,
        current: Option<Uuid>,
    ) -> Result<Vec<TransferTarget>, AccountError> {
        let mut targets: Vec<TransferTarget> = Vec::new();
        for device in self.db.transfer_targets(user).await? {
            let entry = TargetDevice {
                current: device.owner_id == user && current == Some(device.id),
                online: self.hub.is_online(device.owner_id, device.id),
                id: device.id,
                name: device.name,
                platform: device.platform,
                signed_in: device.signed_in,
            };
            match targets.last_mut() {
                Some(target) if target.user.id == device.owner_id => target.devices.push(entry),
                _ => targets.push(TransferTarget {
                    own: device.owner_id == user,
                    user: UserRef {
                        id: device.owner_id,
                        username: device.owner_name,
                    },
                    devices: vec![entry],
                }),
            }
        }
        Ok(targets)
    }

    /// Sets who can see a device and send files to it, and tells everyone whose list changed.
    pub async fn set_visibility(
        &self,
        user: UserId,
        id: Uuid,
        request: &SetDeviceVisibility,
    ) -> Result<(), AccountError> {
        let viewers = if request.visibility == DeviceVisibility::Users {
            request.users.as_slice()
        } else {
            &[]
        };
        let affected = self
            .db
            .set_device_visibility(user, id, request.visibility, viewers)
            .await
            .map_err(|err| match err {
                DbError::Invalid => AccountError::InvalidViewer,
                other => other.into(),
            })?;
        let events = std::iter::once(user)
            .chain(affected)
            .map(|user| DomainEvent::DevicesChanged { user })
            .collect();
        self.events.publish(events).await;
        Ok(())
    }

    pub async fn rename(
        &self,
        user: UserId,
        id: Uuid,
        request: &RenameDevice,
    ) -> Result<(), AccountError> {
        // The request was checked; this only takes the blanks off.
        let name = protocol::rules::device_name(&request.name).unwrap_or(&request.name);
        Ok(self.db.rename_device(user, id, name).await?)
    }

    /// Ends the device's sessions, closing their websockets, and cancels the uploads it left
    /// unfinished. The device stays in the list, signed out, until it has been gone for as
    /// long as a session lasts unrefreshed.
    pub async fn sign_out(&self, user: UserId, id: Uuid) -> Result<(), AccountError> {
        self.db.sign_out_device(user, id).await?;
        self.auth.sessions_ended(user);
        self.uploads.cancel_device_uploads(user, id).await?;
        Ok(())
    }

    /// Signs the device out and forgets it at once, telling everyone who could see it. The
    /// transfers sent to it are deleted with it; it comes back if it signs in again.
    pub async fn remove(&self, user: UserId, id: Uuid) -> Result<(), AccountError> {
        let affected = self.db.remove_device(user, id).await?;
        self.auth.sessions_ended(user);
        self.uploads.cancel_device_uploads(user, id).await?;
        let events = std::iter::once(user)
            .chain(affected)
            .map(|user| DomainEvent::DevicesChanged { user })
            .collect();
        self.events.publish(events).await;
        Ok(())
    }

    /// Records that `device` was just seen. A failure is logged: it only shows a slightly
    /// older last-seen time.
    pub async fn seen(&self, user: UserId, device: Uuid) {
        if let Err(err) = self.db.touch_device(user, device).await {
            tracing::warn!(error = %err, "recording when a device was last seen failed");
        }
    }
}
