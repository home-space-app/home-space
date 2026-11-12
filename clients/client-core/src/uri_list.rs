//! Links sent to a device: a `text/uri-list` file (RFC 2483), and opening one of them.
//!
//! A received list is opened only after the user has seen the link: [`first_openable`] skips
//! schemes known to run code or reach local files, and the clients show a [`LinkPreview`] to
//! confirm, for whatever this list misses.

use protocol::TransferSummary;

/// The media type of a list of links.
pub const URI_LIST: &str = "text/uri-list";

/// The extension of a list of links sent as a file.
pub const URI_LIST_EXTENSION: &str = "uris";

/// The largest file a client reads into memory to open it.
pub const OPEN_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// The longest link a client offers to open.
const MAX_URI_CHARS: usize = 8 * 1024;

/// The media types a client can open from a transfer.
const OPENABLE: &[&str] = &[URI_LIST];

/// Schemes never handed to the system: they run script, read local or app-private files, or
/// start programs. Anything starting with `ms-` is refused too, for the Windows handlers
/// (`ms-msdt`, `ms-officecmd`, …) that have been used to run code.
const BLOCKED_SCHEMES: &[&str] = &[
    "javascript",
    "vbscript",
    "data",
    "blob",
    "file",
    "filesystem",
    "about",
    "jar",
    "view-source",
    "content",
    "intent",
    "android-app",
    "chrome",
    "chrome-extension",
    "moz-extension",
    "resource",
    "search",
    "search-ms",
    "shell",
];

/// A link about to be opened, as the confirmation shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkPreview {
    /// The link as it will be opened.
    pub uri: String,
    /// Lowercase, e.g. `https` or `spotify`.
    pub scheme: String,
    /// The host in ASCII, so a look-alike name shows as `xn--…`; `None` when the link has none.
    pub host: Option<String>,
    /// Whether it opens in a browser (`http` or `https`) rather than another app.
    pub web: bool,
    /// How many more links the list holds; only the first one opens.
    pub others: usize,
}

/// The type of a media type without its parameters, lowercase: `text/uri-list; charset=utf-8`
/// gives `text/uri-list`.
pub fn mime_essence(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// Whether a transfer is a list of links.
pub fn is_uri_list(transfer: &TransferSummary) -> bool {
    transfer
        .mime
        .as_deref()
        .is_some_and(|mime| mime_essence(mime) == URI_LIST)
}

/// Whether a client may offer to open a transfer: one file, small enough to read into memory, of
/// a type it can open.
pub fn can_open(transfer: &TransferSummary) -> bool {
    transfer.item_count == 1
        && transfer.file_count == 1
        && transfer.total_bytes <= OPEN_MAX_BYTES
        && transfer
            .mime
            .as_deref()
            .is_some_and(|mime| OPENABLE.contains(&mime_essence(mime).as_str()))
}

/// The links as a `text/uri-list` body.
pub fn encode(uris: &[String]) -> String {
    let mut body = uris.join("\r\n");
    body.push_str("\r\n");
    body
}

/// The links of a text that is nothing but links, one per line; `None` when anything else is in
/// it, such as words around a link, so that nothing of it is lost by sending it as links.
pub fn as_uri_list(text: &str) -> Option<Vec<String>> {
    let uris: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| is_uri(line).then(|| line.to_owned()))
        .collect::<Option<_>>()?;
    (!uris.is_empty()).then_some(uris)
}

/// The links of a `text/uri-list` body, without comments and blank lines.
pub fn parse(body: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(body)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// The first link of a `text/uri-list` body that may be opened, with how many others it holds.
pub fn first_openable(body: &[u8]) -> Option<LinkPreview> {
    let uris = parse(body);
    let others = uris.len().saturating_sub(1);
    uris.into_iter()
        .find(|uri| is_safe_to_open(uri))
        .map(|uri| preview(uri, others))
}

/// Whether a link may be handed to the system: an absolute link of a scheme not blocked, with
/// no spaces or control characters.
pub fn is_safe_to_open(uri: &str) -> bool {
    if uri.chars().count() > MAX_URI_CHARS || !is_uri(uri) {
        return false;
    }
    let scheme = scheme(uri).unwrap_or_default().to_ascii_lowercase();
    !scheme.starts_with("ms-") && !BLOCKED_SCHEMES.contains(&scheme.as_str())
}

/// How a link is shown before it opens.
pub fn preview(uri: String, others: usize) -> LinkPreview {
    let scheme = scheme(&uri).unwrap_or_default().to_ascii_lowercase();
    let host = url::Url::parse(&uri)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .filter(|host| !host.is_empty());
    LinkPreview {
        web: scheme == "http" || scheme == "https",
        scheme,
        host,
        uri,
        others,
    }
}

/// An absolute link: a scheme, a colon and something after it, with no whitespace or control
/// characters anywhere.
fn is_uri(text: &str) -> bool {
    scheme(text).is_some_and(|scheme| text.len() > scheme.len() + 1)
        && !text.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The scheme of a link, as written: a letter, then letters, digits, `+`, `-` or `.`.
fn scheme(text: &str) -> Option<&str> {
    let (scheme, _) = text.split_once(':')?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
    .then_some(scheme)
}

#[cfg(test)]
mod tests {
    use protocol::{TransferState, UserId, UserRef};
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    fn summary(mime: Option<&str>, items: u32, files: u32, bytes: u64) -> TransferSummary {
        let user = UserRef {
            id: UserId(Uuid::nil()),
            username: "sam".to_owned(),
        };
        TransferSummary {
            id: Uuid::nil(),
            sender: user.clone(),
            sender_device: None,
            sender_device_name: None,
            recipient: user,
            recipient_device: Uuid::nil(),
            recipient_device_name: "Laptop".to_owned(),
            state: TransferState::Ready,
            total_bytes: bytes,
            item_count: items,
            file_count: files,
            name: Some("link.uris".to_owned()),
            mime: mime.map(str::to_owned),
            message: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            expires_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn only_a_single_small_link_list_can_be_opened() {
        assert!(can_open(&summary(Some(URI_LIST), 1, 1, 20)));
        assert!(can_open(&summary(
            Some("Text/URI-List; charset=utf-8"),
            1,
            1,
            20
        )));
        assert!(can_open(&summary(Some(URI_LIST), 1, 1, OPEN_MAX_BYTES)));
        assert!(!can_open(&summary(
            Some(URI_LIST),
            1,
            1,
            OPEN_MAX_BYTES + 1
        )));
        assert!(!can_open(&summary(Some(URI_LIST), 2, 2, 20)));
        assert!(!can_open(&summary(Some("text/plain"), 1, 1, 20)));
        assert!(!can_open(&summary(None, 1, 1, 20)));
    }

    #[test]
    fn text_of_links_only_becomes_a_list() {
        assert_eq!(
            as_uri_list(" https://a.example/x \n\nspotify:track:1\r\n"),
            Some(vec![
                "https://a.example/x".to_owned(),
                "spotify:track:1".to_owned()
            ])
        );
        assert_eq!(as_uri_list("look at https://a.example"), None);
        assert_eq!(as_uri_list("https://a.example\nand more"), None);
        assert_eq!(as_uri_list("12:30"), None);
        assert_eq!(as_uri_list("   \n"), None);
    }

    #[test]
    fn a_list_is_written_with_crlf() {
        let uris = vec![
            "https://a.example".to_owned(),
            "https://b.example".to_owned(),
        ];
        assert_eq!(encode(&uris), "https://a.example\r\nhttps://b.example\r\n");
        assert_eq!(parse(encode(&uris).as_bytes()), uris);
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let body = b"# sent from Pixel\r\n\r\nhttps://a.example\nhttps://b.example\n";
        assert_eq!(parse(body), vec!["https://a.example", "https://b.example"]);
        let first = first_openable(body).expect("link");
        assert_eq!(first.uri, "https://a.example");
        assert_eq!(first.others, 1);
    }

    #[test]
    fn dangerous_schemes_are_never_opened() {
        for uri in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "data:text/html,<b>x</b>",
            "file:///etc/passwd",
            "intent://x#Intent;end",
            "content://app.homespace.android.files/offline/x",
            "ms-msdt:/id",
            "MS-OfficeCmd:x",
            "search-ms:query=x",
            "vbscript:x",
        ] {
            assert!(!is_safe_to_open(uri), "{uri}");
        }
        assert!(!is_safe_to_open("https://a.example/with space"));
        assert!(!is_safe_to_open("https://a.example/\u{7}"));
        assert!(!is_safe_to_open(&format!(
            "https://a.example/{}",
            "x".repeat(MAX_URI_CHARS)
        )));
    }

    #[test]
    fn app_links_are_allowed() {
        for uri in [
            "https://a.example",
            "http://192.168.1.2:8080/x",
            "spotify:track:4uLU6hMCjMI75M1A2tKUQC",
            "mailto:sam@example.com",
            "tel:+15551234",
            "geo:42.69,23.32",
            "magnet:?xt=urn:btih:abc",
        ] {
            assert!(is_safe_to_open(uri), "{uri}");
        }
    }

    #[test]
    fn the_first_safe_link_is_offered() {
        let body = b"javascript:alert(1)\r\nspotify:track:1\r\nhttps://a.example\r\n";
        let first = first_openable(body).expect("link");
        assert_eq!(first.uri, "spotify:track:1");
        assert_eq!(first.scheme, "spotify");
        assert_eq!(first.host, None);
        assert!(!first.web);
        assert_eq!(first.others, 2);
        assert_eq!(
            first_openable(b"javascript:alert(1)\r\n# only this\r\n"),
            None
        );
    }

    #[test]
    fn a_look_alike_host_shows_as_punycode() {
        let preview = preview("https://\u{0435}xample.com/login".to_owned(), 0);
        assert!(preview.web);
        assert_eq!(preview.host.as_deref(), Some("xn--xample-2of.com"));
        assert_eq!(
            super::preview("HTTPS://Example.COM/a".to_owned(), 0)
                .host
                .as_deref(),
            Some("example.com")
        );
    }
}
