//! Shares other users made for the signed-in user (`/shared`): read-only, browsed, downloaded,
//! and copied into the user's own files.

use protocol::{CopyShared, Node, NodeId, ReceivedShare, SharedFolderQuery, SharedNode, paths};
use uuid::Uuid;

use crate::{ApiError, Client};

impl Client {
    /// The shares made for the user that still work, newest first.
    pub async fn shared_with_me(&self) -> Result<Vec<ReceivedShare>, ApiError> {
        self.send_json(|| self.http().get(self.url(paths::SHARED)))
            .await
    }

    /// The children of folder `parent`, a shared folder or one inside it.
    pub async fn shared_children(
        &self,
        share: Uuid,
        parent: NodeId,
    ) -> Result<Vec<SharedNode>, ApiError> {
        let query = SharedFolderQuery { parent_id: parent };
        self.send_json(|| {
            self.http()
                .get(self.url(&paths::shared_nodes(share)))
                .query(&query)
        })
        .await
    }

    /// Saves copies of `node_ids` of the share into the user's folder `parent` (their root when
    /// `None`); answers the copies at the top.
    pub async fn copy_shared(
        &self,
        share: Uuid,
        node_ids: Vec<NodeId>,
        parent: Option<NodeId>,
    ) -> Result<Vec<Node>, ApiError> {
        let request = CopyShared {
            node_ids,
            parent_id: parent,
        };
        self.send_json(|| {
            self.http()
                .post(self.url(&paths::shared_copy(share)))
                .json(&request)
        })
        .await
    }

    /// URL that downloads a shared file when opened, authenticated by `ticket`.
    pub fn received_file_url(&self, share: Uuid, node: NodeId, ticket: &str) -> String {
        format!(
            "{}?{}={ticket}",
            self.url(&paths::shared_file_content(share, node)),
            paths::TICKET_PARAM
        )
    }

    /// URL that downloads a share, or its folder `folder`, as one zip when opened,
    /// authenticated by `ticket`.
    pub fn received_archive_url(
        &self,
        share: Uuid,
        folder: Option<NodeId>,
        ticket: &str,
    ) -> String {
        let base = format!(
            "{}?{}={ticket}",
            self.url(&paths::shared_archive(share)),
            paths::TICKET_PARAM
        );
        match folder {
            Some(folder) => format!("{base}&{}={folder}", paths::SHARE_FOLDER_PARAM),
            None => base,
        }
    }

    /// Starts downloading a shared file.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn download_shared(
        &self,
        share: Uuid,
        node: NodeId,
    ) -> Result<reqwest::Response, ApiError> {
        let response = self
            .send(|| {
                self.http()
                    .get(self.url(&paths::shared_file_content(share, node)))
            })
            .await?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(crate::client::error_from(response).await)
        }
    }
}
