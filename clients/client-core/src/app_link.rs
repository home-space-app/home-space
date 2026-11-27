//! Links that open the desktop app, `home-space://…`, for what the browser hands over to it: a
//! folder protected with a security key, which only the apps can unlock. The desktop app
//! registers the scheme with the operating system and opens what a link names.

use protocol::NodeId;
use url::Url;
use uuid::Uuid;

/// The scheme the desktop app registers.
pub const SCHEME: &str = "home-space";
const FOLDER: &str = "folder";
const SERVER: &str = "server";

/// What a `home-space://` link asks the app to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppLink {
    /// Folder `folder` of the account on `server`, the address the page was on:
    /// `home-space://folder/<id>?server=<address>`.
    Folder { server: Url, folder: NodeId },
}

impl AppLink {
    /// The link, to hand to the browser.
    pub fn to_url(&self) -> String {
        match self {
            Self::Folder { server, folder } => {
                let mut link = format!("{SCHEME}://{FOLDER}/{}", folder.0);
                link.push('?');
                link.push_str(
                    &url::form_urlencoded::Serializer::new(String::new())
                        .append_pair(SERVER, server.as_str())
                        .finish(),
                );
                link
            }
        }
    }

    /// The link `link` names; `None` for one of another scheme, of a kind this version does not
    /// know, or with a server that is not an `http` or `https` address.
    pub fn parse(link: &str) -> Option<Self> {
        let link = Url::parse(link).ok()?;
        if link.scheme() != SCHEME || link.host_str() != Some(FOLDER) {
            return None;
        }
        let folder = Uuid::parse_str(link.path().trim_start_matches('/')).ok()?;
        let server = link
            .query_pairs()
            .find(|(key, _)| key == SERVER)
            .and_then(|(_, value)| Url::parse(&value).ok())
            .filter(|server| matches!(server.scheme(), "http" | "https"))?;
        Some(Self::Folder {
            server,
            folder: NodeId(folder),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: NodeId = NodeId(Uuid::from_bytes([7; 16]));

    #[test]
    fn a_folder_link_names_the_folder_and_the_server() {
        let link = AppLink::Folder {
            server: Url::parse("https://files.example.com:8443/").expect("url"),
            folder: ID,
        };
        let text = link.to_url();
        assert_eq!(
            text,
            "home-space://folder/07070707-0707-0707-0707-070707070707\
             ?server=https%3A%2F%2Ffiles.example.com%3A8443%2F"
        );
        assert_eq!(AppLink::parse(&text), Some(link));
    }

    #[test]
    fn other_links_are_not_opened() {
        for link in [
            "https://folder/07070707-0707-0707-0707-070707070707?server=https%3A%2F%2Fa",
            "home-space://file/07070707-0707-0707-0707-070707070707?server=https%3A%2F%2Fa",
            "home-space://folder/not-an-id?server=https%3A%2F%2Fa",
            "home-space://folder/07070707-0707-0707-0707-070707070707",
            "home-space://folder/07070707-0707-0707-0707-070707070707?server=file%3A%2F%2F%2Fetc",
        ] {
            assert_eq!(AppLink::parse(link), None, "{link}");
        }
    }
}
