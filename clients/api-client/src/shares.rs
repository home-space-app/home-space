//! The signed-in user's share links (`/shares`).

use protocol::{CreateShare, NodeId, Share, SharesQuery, UpdateShare, paths};
use uuid::Uuid;

use crate::{ApiError, Client, client::expect_success};

impl Client {
    /// Makes a read-only link to the user's `request.node_ids`.
    pub async fn create_share(&self, request: &CreateShare) -> Result<Share, ApiError> {
        self.send_json(|| self.http().post(self.url(paths::SHARES)).json(request))
            .await
    }

    /// The user's share links, newest first; only those including `node_id` when given.
    pub async fn shares(&self, node_id: Option<NodeId>) -> Result<Vec<Share>, ApiError> {
        let query = SharesQuery { node_id };
        self.send_json(|| self.http().get(self.url(paths::SHARES)).query(&query))
            .await
    }

    /// Changes the password or the expiry of a share link.
    pub async fn update_share(&self, id: Uuid, request: &UpdateShare) -> Result<Share, ApiError> {
        self.send_json(|| self.http().patch(self.url(&paths::share(id))).json(request))
            .await
    }

    /// Stops sharing: the link stops working at once.
    pub async fn delete_share(&self, id: Uuid) -> Result<(), ApiError> {
        let response = self
            .send(|| self.http().delete(self.url(&paths::share(id))))
            .await?;
        expect_success(response).await
    }

    /// The web page that opens the share link with `token`.
    pub fn share_url(&self, token: &str) -> String {
        self.url(&paths::web_share(token))
    }
}
