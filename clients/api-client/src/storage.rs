//! Storage usage (`/storage`) and the administration status (`/admin/status`).

use protocol::{ServerStatus, Storage, paths};

use crate::{ApiError, Client};

impl Client {
    /// What the signed-in user stores and the room left on the server.
    pub async fn storage(&self) -> Result<Storage, ApiError> {
        self.send_json(|| self.http().get(self.url(paths::STORAGE)))
            .await
    }

    /// The server at a glance; administrators only.
    pub async fn server_status(&self) -> Result<ServerStatus, ApiError> {
        self.send_json(|| self.http().get(self.url(paths::ADMIN_STATUS)))
            .await
    }
}
