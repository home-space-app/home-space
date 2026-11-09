use client_core::icon::NodeIcon;
use leptos::prelude::*;

use protocol::Node;

const ICON_FOLDER: &str = "folder";
const ICON_IMAGE: &str = "image";
const ICON_DOCUMENT: &str = "description";
const ICON_OTHER: &str = "draft";

/// The Material Symbols ligature that represents `node`.
pub fn node_icon(node: &Node) -> &'static str {
    match client_core::icon::node_icon(node) {
        NodeIcon::Folder => ICON_FOLDER,
        NodeIcon::Image => ICON_IMAGE,
        NodeIcon::Document => ICON_DOCUMENT,
        NodeIcon::Other => ICON_OTHER,
    }
}

#[component]
pub fn Icon(name: &'static str) -> impl IntoView {
    view! {
        <span class="icon" class:icon--folder=name == ICON_FOLDER aria-hidden="true">
            {name}
        </span>
    }
}

/// The Material Symbols ligature for a device running `platform`.
pub fn platform_icon(platform: &str) -> &'static str {
    match platform {
        protocol::PLATFORM_WEB => "language",
        protocol::PLATFORM_ANDROID | protocol::PLATFORM_IOS => "smartphone",
        _ => "laptop",
    }
}

/// The Material Symbols ligature for who can see a device.
pub fn visibility_icon(visibility: protocol::DeviceVisibility) -> &'static str {
    match visibility {
        protocol::DeviceVisibility::Owner => "lock",
        protocol::DeviceVisibility::Hidden => "visibility_off",
        protocol::DeviceVisibility::Everyone => "public",
        protocol::DeviceVisibility::Users => "group",
    }
}
