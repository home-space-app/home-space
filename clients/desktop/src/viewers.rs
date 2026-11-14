//! The built-in viewers of the desktop client: the shared ones, plus the image types iced's
//! image decoder reads. Types that depend on what the system has installed, such as camera RAW
//! formats, would be probed and added here.

use std::sync::LazyLock;

use client_core::viewers::{ViewerKind, Viewers};

/// Image types the `image` crate behind iced decodes, beyond the shared ones.
const DECODED_IMAGE_TYPES: &[&str] = &["image/tiff", "image/x-icon", "image/vnd.microsoft.icon"];

static VIEWERS: LazyLock<Viewers> = LazyLock::new(|| {
    Viewers::shared().with_types(ViewerKind::Image, DECODED_IMAGE_TYPES.iter().copied())
});

/// The viewers of this client.
pub fn viewers() -> &'static Viewers {
    &VIEWERS
}
