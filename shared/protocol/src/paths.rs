//! Route paths relative to the server origin.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

use crate::NodeId;

pub const API_PREFIX: &str = "/api/v1";
pub const HEALTH: &str = "/api/v1/health";
/// Everything about signing in lives below this path; the refresh cookie is limited to it.
pub const AUTH: &str = "/api/v1/auth";
pub const LOGIN: &str = "/api/v1/auth/login";
/// The second step of a sign-in that needs a second factor.
pub const LOGIN_VERIFY: &str = "/api/v1/auth/login/verify";
pub const REFRESH: &str = "/api/v1/auth/refresh";
pub const LOGOUT: &str = "/api/v1/auth/logout";
pub const DEVICE_SESSIONS: &str = "/api/v1/auth/device-sessions";
pub const PROVIDERS: &str = "/api/v1/auth/providers";
pub const OIDC_START: &str = "/api/v1/auth/oidc/start";
pub const OIDC_CALLBACK: &str = "/api/v1/auth/oidc/callback";
pub const OIDC_EXCHANGE: &str = "/api/v1/auth/oidc/exchange";
/// Where single sign-on sends the browser back to the mobile apps (RFC 8252 private-use scheme).
/// Not a route on the server: the apps claim this address on the device.
pub const MOBILE_OIDC_REDIRECT: &str = "app.homespace://oidc/callback";
/// Browser sign-in for apps: sends the system browser to the web client's [`WEB_AUTHORIZE`] page.
pub const BROWSER_LOGIN_START: &str = "/api/v1/auth/browser/start";
pub const BROWSER_LOGIN_APPROVE: &str = "/api/v1/auth/browser/approve";
pub const BROWSER_LOGIN_DENY: &str = "/api/v1/auth/browser/deny";
pub const BROWSER_LOGIN_EXCHANGE: &str = "/api/v1/auth/browser/exchange";
/// The web client page where a signed-in user lets an app sign in; takes `flow`.
pub const WEB_AUTHORIZE: &str = "/authorize";
pub const ME: &str = "/api/v1/me";
pub const ME_PASSWORD: &str = "/api/v1/me/password";
pub const ME_REAUTHENTICATE: &str = "/api/v1/me/reauthenticate";
/// `POST` starts adding an authenticator app, `DELETE` removes it.
pub const ME_TOTP: &str = "/api/v1/me/totp";
pub const ME_TOTP_CONFIRM: &str = "/api/v1/me/totp/confirm";
/// `POST` makes a new set of recovery codes.
pub const ME_RECOVERY_CODES: &str = "/api/v1/me/recovery-codes";
/// `GET` lists the user's passkeys and security keys.
pub const ME_WEBAUTHN: &str = "/api/v1/me/webauthn";
pub const ME_WEBAUTHN_START: &str = "/api/v1/me/webauthn/start";
pub const ME_WEBAUTHN_FINISH: &str = "/api/v1/me/webauthn/finish";
/// The challenge a passkey or security key answers to confirm it is the user.
pub const ME_REAUTHENTICATE_WEBAUTHN: &str = "/api/v1/me/reauthenticate/webauthn";
/// The challenge for a passkey or security key as the second step of a sign-in.
pub const LOGIN_WEBAUTHN: &str = "/api/v1/auth/login/webauthn";
pub const PASSKEY_START: &str = "/api/v1/auth/passkey/start";
pub const PASSKEY_FINISH: &str = "/api/v1/auth/passkey/finish";
/// `GET` the caller's vault of protected folders.
pub const VAULT: &str = "/api/v1/vault";
/// `POST` enables a folder key or sets the recovery key.
pub const VAULT_UNLOCKERS: &str = "/api/v1/vault/unlockers";
/// `POST` replaces the vault key and rewraps everything under it.
pub const VAULT_ROTATE: &str = "/api/v1/vault/rotate";
/// `GET` lists the caller's protected folders, `POST` protects one.
pub const PROTECTED: &str = "/api/v1/protected";
pub const NODES: &str = "/api/v1/nodes";
pub const NODES_MOVE: &str = "/api/v1/nodes/move";
pub const FOLDERS: &str = "/api/v1/folders";
pub const FILES: &str = "/api/v1/files";
pub const TRASH: &str = "/api/v1/trash";
pub const TRASH_RESTORE: &str = "/api/v1/trash/restore";
pub const SEARCH: &str = "/api/v1/search";
/// Live files sharing their content with other files.
pub const DUPLICATES: &str = "/api/v1/duplicates";
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
pub const ADMIN_USERS: &str = "/api/v1/admin/users";
pub const SHARES: &str = "/api/v1/shares";
pub const PUBLIC_SHARES: &str = "/api/v1/public/shares";
/// Shares other users made for the caller.
pub const SHARED: &str = "/api/v1/shared";
/// Header carrying the access token of an unlocked share link.
pub const SHARE_ACCESS: &str = "Share-Access";
/// Query parameter carrying it where headers cannot be sent.
pub const SHARE_ACCESS_PARAM: &str = "access";
/// Query parameter naming the folder of a share to zip.
pub const SHARE_FOLDER_PARAM: &str = "folder_id";
/// Where the web client opens a share link.
pub const WEB_SHARE: &str = "/s";
/// Header of `PATCH /uploads/{id}` naming where the chunk starts, and of upload responses
/// naming how many bytes the server has.
pub const UPLOAD_OFFSET: &str = "Upload-Offset";

/// What the web client shows about the browser sign-in `flow` before the user lets it through.
pub fn browser_login_flow(flow: &str) -> String {
    format!("/api/v1/auth/browser/flows/{flow}")
}

pub fn upload(id: uuid::Uuid) -> String {
    format!("{UPLOADS}/{id}")
}

pub fn admin_user(id: crate::UserId) -> String {
    format!("{ADMIN_USERS}/{id}")
}

pub fn admin_user_password(id: crate::UserId) -> String {
    format!("{ADMIN_USERS}/{id}/password")
}

/// `DELETE` removes an account's second factors, so it signs in with the password alone.
/// `PATCH` renames, `DELETE` removes one of the user's passkeys and security keys.
pub fn me_webauthn_key(id: uuid::Uuid) -> String {
    format!("{ME_WEBAUTHN}/{id}")
}

/// `DELETE` removes one of the vault's unlockers.
pub fn vault_unlocker(id: uuid::Uuid) -> String {
    format!("{VAULT_UNLOCKERS}/{id}")
}

/// `DELETE` forgets the protection of a folder that is being protected or unprotected.
pub fn protected_folder(id: NodeId) -> String {
    format!("{PROTECTED}/{id}")
}

/// `POST` marks a folder whose content the client has encrypted as protected.
pub fn protected_finish(id: NodeId) -> String {
    format!("{PROTECTED}/{id}/finish")
}

/// `POST` starts replacing a protected folder's content with its decrypted form.
pub fn protected_unprotect(id: NodeId) -> String {
    format!("{PROTECTED}/{id}/unprotect")
}

/// `POST` stores a password folder's secret wrapped for a new password.
pub fn protected_password(id: NodeId) -> String {
    format!("{PROTECTED}/{id}/password")
}

pub fn admin_user_second_factors(id: crate::UserId) -> String {
    format!("{ADMIN_USERS}/{id}/second-factors")
}

pub fn admin_user_devices(id: crate::UserId) -> String {
    format!("{ADMIN_USERS}/{id}/devices")
}

/// `DELETE` takes back a user's pending deletion.
pub fn admin_user_deletion(id: crate::UserId) -> String {
    format!("{ADMIN_USERS}/{id}/deletion")
}

/// `DELETE` signs the device out and forgets it.
pub fn device(id: uuid::Uuid) -> String {
    format!("{DEVICES}/{id}")
}

pub fn device_visibility(id: uuid::Uuid) -> String {
    format!("{DEVICES}/{id}/visibility")
}

/// `DELETE` signs the device out and keeps it listed.
pub fn device_sessions(id: uuid::Uuid) -> String {
    format!("{DEVICES}/{id}/sessions")
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

pub fn share(id: uuid::Uuid) -> String {
    format!("{SHARES}/{id}")
}

pub fn public_share(token: &str) -> String {
    format!("{PUBLIC_SHARES}/{token}")
}

pub fn public_share_unlock(token: &str) -> String {
    format!("{PUBLIC_SHARES}/{token}/unlock")
}

pub fn public_share_nodes(token: &str) -> String {
    format!("{PUBLIC_SHARES}/{token}/nodes")
}

pub fn public_share_file_content(token: &str, id: NodeId) -> String {
    format!("{PUBLIC_SHARES}/{token}/files/{id}/content")
}

pub fn public_share_archive(token: &str) -> String {
    format!("{PUBLIC_SHARES}/{token}/archive")
}

pub fn shared_nodes(id: uuid::Uuid) -> String {
    format!("{SHARED}/{id}/nodes")
}

pub fn shared_file_content(id: uuid::Uuid, node: NodeId) -> String {
    format!("{SHARED}/{id}/files/{node}/content")
}

pub fn shared_archive(id: uuid::Uuid) -> String {
    format!("{SHARED}/{id}/archive")
}

pub fn shared_copy(id: uuid::Uuid) -> String {
    format!("{SHARED}/{id}/copy")
}

/// The web client page of a share link, relative to the server origin.
pub fn web_share(token: &str) -> String {
    format!("{WEB_SHARE}/{token}")
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

/// An id as it appears in web client URLs: 22-char base64url instead of hyphenated hex.
pub fn short_id(id: NodeId) -> String {
    URL_SAFE_NO_PAD.encode(id.0.as_bytes())
}

/// The web client page of a folder, or of Home for `None`.
pub fn web_folder(id: Option<NodeId>) -> String {
    id.map_or_else(|| "/".to_owned(), |id| format!("/folder/{}", short_id(id)))
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
