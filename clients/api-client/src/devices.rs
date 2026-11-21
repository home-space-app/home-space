//! The devices of the signed-in user (`/devices`) and their unfinished uploads.

use protocol::{Device, RenameDevice, UploadInfo, UploadsQuery, paths};
use uuid::Uuid;

use crate::{ApiError, Client, client::expect_success};

impl Client {
    /// The user's devices, the most recently seen first.
    pub async fn devices(&self) -> Result<Vec<Device>, ApiError> {
        self.send_json(|| self.http().get(self.url(paths::DEVICES)))
            .await
    }

    /// Renames one of the user's devices.
    pub async fn rename_device(&self, id: Uuid, name: &str) -> Result<(), ApiError> {
        let request = RenameDevice {
            name: name.to_owned(),
        };
        let response = self
            .send(|| {
                self.http()
                    .patch(self.url(&paths::device(id)))
                    .json(&request)
            })
            .await?;
        expect_success(response).await
    }

    /// Signs a device out: ends its sessions and cancels its unfinished uploads.
    pub async fn sign_out_device(&self, id: Uuid) -> Result<(), ApiError> {
        let response = self
            .send(|| self.http().delete(self.url(&paths::device_sessions(id))))
            .await?;
        expect_success(response).await
    }

    /// Removes a device: signs it out as [`Client::sign_out_device`] does, declines the files
    /// sent to it and takes it off the list. It comes back if it signs in again.
    pub async fn remove_device(&self, id: Uuid) -> Result<(), ApiError> {
        let response = self
            .send(|| self.http().delete(self.url(&paths::device(id))))
            .await?;
        expect_success(response).await
    }

    /// The user's unfinished uploads, or only those this device started.
    pub async fn uploads(&self, this_device: bool) -> Result<Vec<UploadInfo>, ApiError> {
        let query = UploadsQuery { this_device };
        self.send_json(|| self.http().get(self.url(paths::UPLOADS)).query(&query))
            .await
    }
}
