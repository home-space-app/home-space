//! The built-in viewers of the web client: the shared ones, plus the image types browsers show.

use std::sync::LazyLock;

use client_core::viewers::{ViewerKind, Viewers};

/// Image types every current browser renders in an `<img>`, beyond the shared ones.
const BROWSER_IMAGE_TYPES: &[&str] = &[
    "image/svg+xml",
    "image/avif",
    "image/x-icon",
    "image/vnd.microsoft.icon",
];

static VIEWERS: LazyLock<Viewers> = LazyLock::new(|| {
    Viewers::shared().with_types(ViewerKind::Image, BROWSER_IMAGE_TYPES.iter().copied())
});

/// The viewers of this client.
pub fn viewers() -> &'static Viewers {
    &VIEWERS
}
