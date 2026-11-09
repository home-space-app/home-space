//! Route paths relative to the server origin.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

use crate::NodeId;

pub const API_PREFIX: &str = "/api/v1";
pub const HEALTH: &str = "/api/v1/health";
pub const LOGIN: &str = "/api/v1/auth/login";
pub const REFRESH: &str = "/api/v1/auth/refresh";
pub const LOGOUT: &str = "/api/v1/auth/logout";
pub const PROVIDERS: &str = "/api/v1/auth/providers";
pub const OIDC_START: &str = "/api/v1/auth/oidc/start";
pub const OIDC_CALLBACK: &str = "/api/v1/auth/oidc/callback";
pub const OIDC_EXCHANGE: &str = "/api/v1/auth/oidc/exchange";
/// Where single sign-on sends the browser back to the mobile apps (RFC 8252 private-use scheme).
/// Not a route on the server: the apps claim this address on the device.
pub const MOBILE_OIDC_REDIRECT: &str = "app.homespace://oidc/callback";
pub const ME: &str = "/api/v1/me";
pub const ME_PASSWORD: &str = "/api/v1/me/password";
pub const NODES: &str = "/api/v1/nodes";
pub const NODES_MOVE: &str = "/api/v1/nodes/move";
pub const FOLDERS: &str = "/api/v1/folders";
pub const FILES: &str = "/api/v1/files";
pub const TRASH: &str = "/api/v1/trash";
pub const TRASH_RESTORE: &str = "/api/v1/trash/restore";
pub const SEARCH: &str = "/api/v1/search";
pub const CHANGES: &str = "/api/v1/changes";
pub const TICKETS: &str = "/api/v1/tickets";
pub const WS: &str = "/api/v1/ws";
pub const TICKET_PARAM: &str = "ticket";
pub const UPLOADS: &str = "/api/v1/uploads";
pub const DEVICES: &str = "/api/v1/devices";
pub const DEVICE_TARGETS: &str = "/api/v1/devices/targets";
pub const USERS: &str = "/api/v1/users";
pub const TRANSFERS: &str = "/api/v1/transfers";
pub const TRANSFERS_INCOMING: &str = "/api/v1/transfers/incoming";
pub const TRANSFERS_OUTGOING: &str = "/api/v1/transfers/outgoing";
pub const CAPABILITIES: &str = "/api/v1/capabilities";
pub const STORAGE: &str = "/api/v1/storage";
pub const ADMIN_STATUS: &str = "/api/v1/admin/status";
/// Header of `PATCH /uploads/{id}` naming where the chunk starts, and of upload responses
/// naming how many bytes the server has.
pub const UPLOAD_OFFSET: &str = "Upload-Offset";

pub fn upload(id: uuid::Uuid) -> String {
    format!("{UPLOADS}/{id}")
}

pub fn device(id: uuid::Uuid) -> String {
    format!("{DEVICES}/{id}")
}

pub fn device_visibility(id: uuid::Uuid) -> String {
    format!("{DEVICES}/{id}/visibility")
}

pub fn transfer(id: uuid::Uuid) -> String {
    format!("{TRANSFERS}/{id}")
}

pub fn transfer_item_content(id: uuid::Uuid, item: uuid::Uuid) -> String {
    format!("{TRANSFERS}/{id}/items/{item}/content")
}

pub fn transfer_archive(id: uuid::Uuid) -> String {
    format!("{TRANSFERS}/{id}/archive")
}

pub fn transfer_decline(id: uuid::Uuid) -> String {
    format!("{TRANSFERS}/{id}/decline")
}

pub fn transfer_received(id: uuid::Uuid) -> String {
    format!("{TRANSFERS}/{id}/received")
}

pub fn transfer_save(id: uuid::Uuid) -> String {
    format!("{TRANSFERS}/{id}/save")
}

pub fn node(id: NodeId) -> String {
    format!("{NODES}/{id}")
}

pub fn trash_item(id: NodeId) -> String {
    format!("{TRASH}/{id}")
}

pub fn ancestors(id: NodeId) -> String {
    format!("{NODES}/{id}/ancestors")
}

pub fn properties(id: NodeId) -> String {
    format!("{NODES}/{id}/properties")
}

pub fn versions(id: NodeId) -> String {
    format!("{NODES}/{id}/versions")
}

pub fn version(id: NodeId, version: i32) -> String {
    format!("{NODES}/{id}/versions/{version}")
}

pub fn version_content(id: NodeId, version: i32) -> String {
    format!("{FILES}/{id}/versions/{version}/content")
}

pub fn file_content(id: NodeId) -> String {
    format!("{FILES}/{id}/content")
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn node_paths_embed_the_id() {
        let id = NodeId(Uuid::nil());
        assert_eq!(
            file_content(id),
            "/api/v1/files/00000000-0000-0000-0000-000000000000/content"
        );
        assert!(ancestors(id).ends_with("/ancestors"));
    }
}

/// An id as it appears in web client URLs: 22-char base64url instead of hyphenated hex.
pub fn short_id(id: NodeId) -> String {
    URL_SAFE_NO_PAD.encode(id.0.as_bytes())
}

/// The web client page of a folder, or of Home for `None`.
pub fn web_folder(id: Option<NodeId>) -> String {
    id.map_or_else(|| "/".to_owned(), |id| format!("/folder/{}", short_id(id)))
}
