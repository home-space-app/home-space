use std::sync::Arc;

use files::FileServices;
use notifications::Hub;
use search::SearchIndex;

use crate::Tenants;

/// Shared services handed to every handler.
#[derive(Clone, Debug)]
pub struct AppState {
    /// Every tenant of the server, with its own auth service. Handlers reach the one a
    /// request was sent to through the [`Tenant`](crate::extract::Tenant) extractor.
    pub tenants: Tenants,
    pub files: FileServices,
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
