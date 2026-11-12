//! Sending files to a device (`/transfers`), and who can see a device.

use protocol::{
    CreateTransfer, CreatedTransfer, ErrorCode, IncomingQuery, Node, NodeId, NodeKind,
    SaveTransfer, SetDeviceVisibility, Transfer, TransferSummary, TransferTarget, UserRef, paths,
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

    /// The bytes of a transfer that is one file, read into memory, for opening it rather than
    /// saving it. Refused with `PayloadTooLarge` when the file is, or turns out to be, over
    /// `max` bytes, and with `BadRequest` when the transfer isn't a single file.
    pub async fn transfer_file_bytes(&self, id: Uuid, max: u64) -> Result<Vec<u8>, ApiError> {
        let transfer = self.transfer(id).await?;
        let [item] = transfer.items.as_slice() else {
            return Err(not_one_file());
        };
        if item.kind != NodeKind::File {
            return Err(not_one_file());
        }
        if item.size > max {
            return Err(too_large(max));
        }
        let response = self
            .send(|| {
                self.http()
                    .get(self.url(&paths::transfer_item_content(id, item.id)))
            })
            .await?;
        if !response.status().is_success() {
            return Err(crate::client::error_from(response).await);
        }
        if response.content_length().is_some_and(|length| length > max) {
            return Err(too_large(max));
        }
        read_capped(response, max).await
    }

    async fn post_empty(&self, path: &str) -> Result<(), ApiError> {
        let response = self.send(|| self.http().post(self.url(path))).await?;
        expect_success(response).await
    }
}

/// Reads a body, stopping as soon as it is over `max` bytes.
#[cfg(not(target_arch = "wasm32"))]
async fn read_capped(mut response: reqwest::Response, max: u64) -> Result<Vec<u8>, ApiError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() as u64 > max {
            return Err(too_large(max));
        }
    }
    Ok(body)
}

/// The browser hands the body over whole; the size was checked against the item and the
/// `Content-Length` before this.
#[cfg(target_arch = "wasm32")]
async fn read_capped(response: reqwest::Response, max: u64) -> Result<Vec<u8>, ApiError> {
    let body = response.bytes().await?;
    if body.len() as u64 > max {
        return Err(too_large(max));
    }
    Ok(body.to_vec())
}

fn not_one_file() -> ApiError {
    ApiError::Api {
        code: ErrorCode::BadRequest,
        message: "This transfer isn't a single file.".to_owned(),
    }
}

fn too_large(max: u64) -> ApiError {
    ApiError::Api {
        code: ErrorCode::PayloadTooLarge,
        message: format!(
            "The file is over {} MB, too large to open here.",
            max / (1024 * 1024)
        ),
    }
}
