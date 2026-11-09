use std::sync::Arc;

use auth::AuthService;
use db::Db;
use files::FileService;
use notifications::Hub;
use search::SearchIndex;

/// Shared services handed to every handler.
#[derive(Clone, Debug)]
pub struct AppState {
    pub db: Db,
    pub auth: AuthService,
    pub files: FileService,
    pub hub: Hub,
    pub search: SearchIndex,
    /// What the administration status reports about this server.
    pub info: Arc<ServerInfo>,
    /// Whether a reverse proxy in front of the server names the client in `X-Forwarded-For`.
    /// Off, the header is ignored, since any client could put anything in it.
    pub trust_forwarded_for: bool,
}

/// Facts about the running server that only change with a restart.
#[derive(Clone, Debug)]
pub struct ServerInfo {
    pub version: String,
    pub started_at: time::OffsetDateTime,
    pub config: protocol::ServerConfig,
}

impl AppState {
    /// Indexes a node's name; a failure is logged because the node change itself succeeded and
    /// the next catch-up pass repairs the index.
    pub(crate) async fn index_name(&self, node: &db::Node) {
        if let Err(err) = self.search.index_name(node).await {
            tracing::warn!(node_id = %node.id, error = %err, "failed to index node name");
        }
    }
}
