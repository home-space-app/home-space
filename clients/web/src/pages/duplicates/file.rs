//! Cells and actions shared by both tables of the duplicates page.

use leptos::prelude::*;
use leptos_router::components::A;

use client_core::{
    duplicates::{OPEN, SHOW_IN_FOLDER},
    labels::location_label,
};
use protocol::{DuplicateFile, Node};

use crate::{
    components::MenuLink,
    icon::{Icon, node_icon},
    pages::viewer::use_view_link,
    route,
    viewers::viewers,
};

const HOME_PATH: &str = "/";

/// The folder holding `node`.
pub fn folder_href(node: &Node) -> String {
    node.parent_id
        .map_or_else(|| HOME_PATH.to_owned(), route::folder)
}

/// `node` open in the built-in viewer over this page, if a viewer shows it.
pub fn view_href(node: &Node) -> Option<String> {
    viewers()
        .viewer_for_node(node)
        .is_some()
        .then(|| use_view_link().href(node.id))
}

/// A file's icon and name, opening its viewer when there is one.
#[component]
pub fn FileName(node: Node, #[prop(optional, into)] meta: Option<String>) -> impl IntoView {
    let icon = node_icon(&node);
    let text = view! {
        <Icon name=icon />
        <span class="node-table__text">
            {node.name.clone()}
            {meta.map(|meta| view! { <span class="node-table__meta">{meta}</span> })}
        </span>
    };
    match view_href(&node) {
        Some(href) => view! {
            <A href=href attr:class="node-table__name node-table__name--link">{text}</A>
        }
        .into_any(),
        None => view! { <span class="node-table__name">{text}</span> }.into_any(),
    }
}

/// Where a file lives, linking to its folder.
#[component]
pub fn FileLocation(file: DuplicateFile) -> impl IntoView {
    let href = folder_href(&file.node);
    let label = location_label(&file.location);
    view! {
        <A href=href attr:class="duplicates__location">{label}</A>
    }
}

/// The menu entries that open a file or its folder.
#[component]
pub fn OpenLinks(node: Node) -> impl IntoView {
    let folder = folder_href(&node);
    view! {
        {view_href(&node)
            .map(|href| {
                view! {
                    <MenuLink href>
                        <Icon name="visibility" />
                        {OPEN}
                    </MenuLink>
                }
            })}
        <MenuLink href=folder>
            <Icon name="folder_open" />
            {SHOW_IN_FOLDER}
        </MenuLink>
    }
}
