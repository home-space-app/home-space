//! The built-in viewers of the web client: the shared ones, plus the image types browsers show
//! and video, which the browser's own player plays.

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
    Viewers::shared()
        .with_types(ViewerKind::Image, BROWSER_IMAGE_TYPES.iter().copied())
        .with_video()
});

/// The viewers of this client.
pub fn viewers() -> &'static Viewers {
    &VIEWERS
}
