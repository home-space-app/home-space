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
/// The settings tab listing the user's share links.
pub const SHARES_PATH: &str = "/settings/shares";

/// What other users shared with the user.
pub const SHARED_PATH: &str = "/shared";

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

/// The query parameter of a share link page naming the folders opened inside it.
pub const SHARE_PATH_PARAM: &str = "path";
const SHARE_PATH_SEPARATOR: char = '.';

/// The page of share link `token` opened at the folder reached through `trail`.
pub fn shared_folder(token: &str, trail: &[NodeId]) -> String {
    let page = protocol::paths::web_share(token);
    if trail.is_empty() {
        page
    } else {
        format!("{page}?{SHARE_PATH_PARAM}={}", share_path(trail))
    }
}

/// The query parameter of a "Shared with me" page naming the file open in its viewer.
pub const SHARED_VIEW_PARAM: &str = "view";

/// The page of share `share` made for the user, opened at the folder reached through `trail`.
pub fn received_folder(share: uuid::Uuid, trail: &[NodeId]) -> String {
    let page = format!("{SHARED_PATH}/{share}");
    if trail.is_empty() {
        page
    } else {
        format!("{page}?{SHARE_PATH_PARAM}={}", share_path(trail))
    }
}

/// File `file` of share `share`, in the folder reached through `trail`, open in its viewer.
pub fn received_file(share: uuid::Uuid, trail: &[NodeId], file: NodeId) -> String {
    let view = format!("{SHARED_VIEW_PARAM}={}", short_id(file));
    if trail.is_empty() {
        format!("{SHARED_PATH}/{share}?{view}")
    } else {
        format!("{}&{view}", received_folder(share, trail))
    }
}

fn share_path(trail: &[NodeId]) -> String {
    trail
        .iter()
        .map(|id| short_id(*id))
        .collect::<Vec<_>>()
        .join(&SHARE_PATH_SEPARATOR.to_string())
}

/// The folders named by a share link page's path, up to the first that is not an id.
pub fn parse_share_path(value: &str) -> Vec<NodeId> {
    value
        .split(SHARE_PATH_SEPARATOR)
        .map_while(parse_id)
        .collect()
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
    fn a_share_page_at_its_top_has_no_path() {
        assert_eq!(shared_folder("tok", &[]), "/s/tok");
    }

    #[test]
    fn a_share_page_path_round_trips() {
        let other = NodeId::from(Uuid::from_u128(7));
        let url = shared_folder("tok", &[id(), other]);
        let path = url.strip_prefix("/s/tok?path=").unwrap_or_default();
        assert_eq!(parse_share_path(path), vec![id(), other]);
    }

    #[test]
    fn a_share_page_path_stops_at_a_bad_id() {
        let path = format!("{}.nope.{}", short_id(id()), short_id(id()));
        assert_eq!(parse_share_path(&path), vec![id()]);
    }

    #[test]
    fn an_empty_share_page_path_names_no_folder() {
        assert_eq!(parse_share_path(""), Vec::<NodeId>::new());
    }

    #[test]
    fn a_received_file_names_its_folder_and_itself() {
        let share = Uuid::from_u128(3);
        let file = NodeId::from(Uuid::from_u128(7));
        assert_eq!(
            received_file(share, &[], file),
            format!("/shared/{share}?view={}", short_id(file))
        );
        assert_eq!(
            received_file(share, &[id()], file),
            format!(
                "/shared/{share}?path={}&view={}",
                short_id(id()),
                short_id(file)
            )
        );
    }

    #[test]
    fn garbage_is_rejected() {
        assert_eq!(parse_id("nope"), None);
        assert_eq!(parse_id(""), None);
    }
}
