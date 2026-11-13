//! Client-side route paths. Ids appear in URLs as 22-char base64url instead
//! of hyphenated hex; the API still uses the hex form.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use protocol::NodeId;
use uuid::Uuid;

pub use protocol::paths::short_id;

const HOME_PATH: &str = "/";
const FOLDER_PREFIX: &str = "/folder/";

pub fn folder(id: NodeId) -> String {
    protocol::paths::web_folder(Some(id))
}

/// Whether `path` is Home or a folder's page.
pub fn is_folder_path(path: &str) -> bool {
    path == HOME_PATH || path.starts_with(FOLDER_PREFIX)
}

/// The built-in viewer of file `id`.
pub fn file_view(id: NodeId) -> String {
    format!("/files/{}/view", short_id(id))
}

pub const TRASH_PATH: &str = "/trash";

/// The account settings: profile and storage.
pub const SETTINGS_PATH: &str = "/settings";
/// The settings tab listing the user's devices.
pub const DEVICES_PATH: &str = "/settings/devices";

/// Files sent to the user's devices.
pub const TRANSFERS_PATH: &str = "/transfers";
/// Files the user sent.
pub const TRANSFERS_SENT_PATH: &str = "/transfers/sent";

/// The administration pages, for administrators only.
pub const ADMIN_PATH: &str = "/admin";
/// The administration tab listing the accounts.
pub const ADMIN_USERS_PATH: &str = "/admin/users";

pub const SEARCH_PATH: &str = "/search";
pub const SEARCH_PARAM: &str = "q";

/// The search page for `query`, with the query percent-encoded.
pub fn search(query: &str) -> String {
    let encoded = query
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect::<String>();
    format!("{SEARCH_PATH}?{SEARCH_PARAM}={encoded}")
}

/// Parses a route id, accepting the short form and the legacy hex form.
pub fn parse_id(value: &str) -> Option<NodeId> {
    URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| Uuid::from_slice(&bytes).ok())
        .map(NodeId::from)
        .or_else(|| value.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> NodeId {
        NodeId::from(Uuid::from_u128(0x0192_f3a4_7b1c_7d2e_9f00_1a2b_3c4d_5e6f))
    }

    #[test]
    fn folder_path_uses_the_short_form() {
        let path = folder(id());
        assert_eq!(path.len(), "/folder/".len() + 22);
        assert_eq!(parse_id(path.strip_prefix("/folder/").unwrap()), Some(id()));
    }

    #[test]
    fn file_view_path_uses_the_short_form() {
        let path = file_view(id());
        let short = path
            .strip_prefix("/files/")
            .and_then(|rest| rest.strip_suffix("/view"));
        assert_eq!(short.and_then(parse_id), Some(id()));
    }

    #[test]
    fn only_home_and_folders_are_folder_paths() {
        assert!(is_folder_path("/"));
        assert!(is_folder_path(&folder(id())));
        assert!(!is_folder_path(&file_view(id())));
    }

    #[test]
    fn legacy_hex_form_still_parses() {
        assert_eq!(parse_id(&id().to_string()), Some(id()));
    }

    #[test]
    fn search_path_percent_encodes_the_query() {
        assert_eq!(search("a b&c/ü"), "/search?q=a%20b%26c%2F%C3%BC");
    }

    #[test]
    fn garbage_is_rejected() {
        assert_eq!(parse_id("nope"), None);
        assert_eq!(parse_id(""), None);
    }
}
