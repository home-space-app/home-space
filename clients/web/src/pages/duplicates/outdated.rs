//! A live file holding the content an other file had before it changed.

use leptos::prelude::*;
use leptos_router::components::A;

use client_core::{duplicates::version_of, labels::location_label};
use protocol::{Node, OutdatedMatch};

use super::file::{FileLocation, FileName, OpenLinks, folder_href, view_href};
use crate::{
    components::{Menu, MenuItem, MenuLink},
    icon::Icon,
    locale::short_date,
};

#[component]
pub fn OutdatedRow(found: OutdatedMatch, deleting: RwSignal<Option<Vec<Node>>>) -> impl IntoView {
    let OutdatedMatch {
        file,
        source,
        version,
        ..
    } = found;
    let node = file.node.clone();
    let id = node.id;
    let modified = short_date(node.updated_at);
    let matches = version_of(version, &source.node);
    let meta = format!("{} \u{b7} {matches}", location_label(&file.location));
    let source_href = view_href(&source.node).unwrap_or_else(|| folder_href(&source.node));
    let source_where = location_label(&source.location);
    let open_source = StoredValue::new(format!("Open {}", source.node.name));
    let delete = {
        let node = node.clone();
        Callback::new(move |()| deleting.set(Some(vec![node.clone()])))
    };

    view! {
        <tr class="node-table__row">
            <td class="node-table__cell">
                <FileName node=node.clone() meta />
            </td>
            <td class="node-table__cell node-table__cell--meta">
                <FileLocation file />
            </td>
            <td class="node-table__cell node-table__cell--meta">
                <A href=source_href.clone() attr:class="duplicates__location">
                    {matches}
                </A>
                <span class="duplicates__summary">{source_where}</span>
            </td>
            <td class="node-table__cell node-table__cell--modified">{modified}</td>
            <td class="node-table__cell node-table__cell--actions">
                <span class="node-table__actions">
                    <Menu id=format!("outdated-menu-{id}-{}", source.node.id) label="File actions">
                        <OpenLinks node=node.clone() />
                        <MenuLink href=source_href.clone()>
                            <Icon name="difference" />
                            {open_source.get_value()}
                        </MenuLink>
                        <MenuItem icon="delete" label="Delete" danger=true on_select=delete />
                    </Menu>
                </span>
            </td>
        </tr>
    }
}
