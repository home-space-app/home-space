//! Sending files to a device (`/transfers`), and who can see a device.

use protocol::{
    CreateTransfer, CreatedTransfer, IncomingQuery, Node, NodeId, SaveTransfer,
    SetDeviceVisibility, Transfer, TransferSummary, TransferTarget, UserRef, paths,
};
use uuid::Uuid;

use crate::{ApiError, Client, client::expect_success};

impl Client {
    /// Sets who can see one of the user's devices and send files to it.
    pub async fn set_device_visibility(
        &self,
        id: Uuid,
        request: &SetDeviceVisibility,
    ) -> Result<(), ApiError> {
        let response = self
            .send(|| {
                self.http()
                    .put(self.url(&paths::device_visibility(id)))
                    .json(request)
            })
            .await?;
        expect_success(response).await
    }

    /// The other users of the server, for picking who can see a device.
    pub async fn users(&self) -> Result<Vec<UserRef>, ApiError> {
        self.send_json(|| self.http().get(self.url(paths::USERS)))
            .await
    }

    /// The devices the user can send files to: their own first, then other users'.
    pub async fn transfer_targets(&self) -> Result<Vec<TransferTarget>, ApiError> {
        self.send_json(|| self.http().get(self.url(paths::DEVICE_TARGETS)))
            .await
    }

    /// Creates a transfer; the items that need it are uploaded afterwards.
    pub async fn create_transfer(
        &self,
        request: &CreateTransfer,
    ) -> Result<CreatedTransfer, ApiError> {
        self.send_json(|| self.http().post(self.url(paths::TRANSFERS)).json(request))
            .await
    }

    /// The transfers waiting for the user, or only those sent to this device.
    pub async fn incoming_transfers(
        &self,
        this_device: bool,
    ) -> Result<Vec<TransferSummary>, ApiError> {
        let query = IncomingQuery { this_device };
        self.send_json(|| {
            self.http()
                .get(self.url(paths::TRANSFERS_INCOMING))
                .query(&query)
        })
        .await
    }

    /// The transfers the user sent, the newest first.
    pub async fn outgoing_transfers(&self) -> Result<Vec<TransferSummary>, ApiError> {
        self.send_json(|| self.http().get(self.url(paths::TRANSFERS_OUTGOING)))
            .await
    }

    /// A transfer with its files and folders.
    pub async fn transfer(&self, id: Uuid) -> Result<Transfer, ApiError> {
        self.send_json(|| self.http().get(self.url(&paths::transfer(id))))
            .await
    }

    /// The sender cancels a transfer.
    pub async fn cancel_transfer(&self, id: Uuid) -> Result<(), ApiError> {
        let response = self
            .send(|| self.http().delete(self.url(&paths::transfer(id))))
            .await?;
        expect_success(response).await
    }

    /// The recipient declines a transfer.
    pub async fn decline_transfer(&self, id: Uuid) -> Result<(), ApiError> {
        self.post_empty(&paths::transfer_decline(id)).await
    }

    /// The recipient confirms the download finished; the server removes its copy.
    pub async fn confirm_transfer(&self, id: Uuid) -> Result<(), ApiError> {
        self.post_empty(&paths::transfer_received(id)).await
    }

    /// The recipient saves a transfer into their tree under `parent` (the root when `None`).
    /// Answers the files and folders created at the top.
    pub async fn save_transfer(
        &self,
        id: Uuid,
        parent: Option<NodeId>,
    ) -> Result<Vec<Node>, ApiError> {
        let request = SaveTransfer { parent_id: parent };
        self.send_json(|| {
            self.http()
                .post(self.url(&paths::transfer_save(id)))
                .json(&request)
        })
        .await
    }

    /// URL that downloads one file of a transfer when opened, authenticated by `ticket`.
    pub fn transfer_item_url(&self, id: Uuid, item: Uuid, ticket: &str) -> String {
        format!(
            "{}?{}={ticket}",
            self.url(&paths::transfer_item_content(id, item)),
            paths::TICKET_PARAM
        )
    }

    /// URL that downloads a whole transfer as one zip when opened, authenticated by `ticket`.
    pub fn transfer_archive_url(&self, id: Uuid, ticket: &str) -> String {
        format!(
            "{}?{}={ticket}",
            self.url(&paths::transfer_archive(id)),
            paths::TICKET_PARAM
        )
    }

    /// Starts downloading one file of a transfer.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn download_transfer_item(
        &self,
        id: Uuid,
        item: Uuid,
    ) -> Result<reqwest::Response, ApiError> {
        let response = self
            .send(|| {
                self.http()
                    .get(self.url(&paths::transfer_item_content(id, item)))
            })
            .await?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(crate::client::error_from(response).await)
        }
    }

    async fn post_empty(&self, path: &str) -> Result<(), ApiError> {
        let response = self.send(|| self.http().post(self.url(path))).await?;
        expect_success(response).await
    }
}
