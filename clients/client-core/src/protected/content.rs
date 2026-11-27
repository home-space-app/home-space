//! What the client tells the browser or the OS a decrypted file is. The server only stores
//! `application/octet-stream` for the files of a protected folder, so the type comes from the
//! opened name.

/// What the server stores as the type of every file of a protected folder.
pub const ENCRYPTED_MIME: &str = "application/octet-stream";

/// Types a decrypted file may be shown as, by extension. Anything that could run script on this
/// origin, such as HTML or SVG, is shown as plain text instead.
const SHOWN_TYPES: &[(&str, &str)] = &[
    ("avif", "image/avif"),
    ("bmp", "image/bmp"),
    ("csv", "text/plain"),
    ("flac", "audio/flac"),
    ("gif", "image/gif"),
    ("htm", "text/plain"),
    ("html", "text/plain"),
    ("jpeg", "image/jpeg"),
    ("jpg", "image/jpeg"),
    ("json", "text/plain"),
    ("log", "text/plain"),
    ("m4a", "audio/mp4"),
    ("md", "text/plain"),
    ("mov", "video/quicktime"),
    ("mp3", "audio/mpeg"),
    ("mp4", "video/mp4"),
    ("ogg", "audio/ogg"),
    ("pdf", "application/pdf"),
    ("png", "image/png"),
    ("svg", "text/plain"),
    ("txt", "text/plain"),
    ("wav", "audio/wav"),
    ("webm", "video/webm"),
    ("webp", "image/webp"),
];

/// The type to show the decrypted file `name` as; [`ENCRYPTED_MIME`] when it is not one the
/// client shows, which makes a browser save it instead.
pub fn shown_mime(name: &str) -> &'static str {
    name.rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .and_then(|extension| {
            SHOWN_TYPES
                .iter()
                .find(|(known, _)| *known == extension)
                .map(|(_, mime)| *mime)
        })
        .unwrap_or(ENCRYPTED_MIME)
}
