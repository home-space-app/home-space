//! One group of identical files: a summary row that expands into a row per copy.

use std::collections::HashSet;

use leptos::prelude::*;

use client_core::{
    duplicates::{KEEP_ONLY_THIS, copies, group_summary, others},
    format::human_size,
    labels::location_label,
};
use protocol::{DuplicateFile, DuplicateGroup, Node};

use super::file::{FileLocation, FileName, OpenLinks};
use crate::{
    components::{Menu, MenuItem},
    icon::{Icon, node_icon},
    locale::short_date,
};

#[component]
pub fn GroupRows(
    group: DuplicateGroup,
    expanded: RwSignal<HashSet<String>>,
    deleting: RwSignal<Option<Vec<Node>>>,
) -> impl IntoView {
    let sha = StoredValue::new(group.sha256.clone());
    let open = Memo::new(move |_| sha.with_value(|sha| expanded.read().contains(sha)));
    let toggle = move |_| {
        let sha = sha.get_value();
        expanded.update(|open| {
            if !open.remove(&sha) {
                open.insert(sha);
            }
        });
    };
    let first = &group.files[0];
    let icon = node_icon(&first.node);
    let name = first.node.name.clone();
    let count = copies(group.files.len());
    let summary = group_summary(&group);
    let size = human_size(group.size);
    let meta = format!("{count} \u{b7} {summary}");
    let group = StoredValue::new(group);

    view! {
        <tr class="node-table__row duplicates__group">
            <td class="node-table__cell">
                <button
                    class="node-table__name duplicates__toggle"
                    aria-expanded=move || open.get().to_string()
                    on:click=toggle
                >
                    <span class="duplicates__chevron" class:duplicates__chevron--open=open>
                        <Icon name="chevron_right" />
                    </span>
                    <Icon name=icon />
                    <span class="node-table__text">
                        {name}
                        <span class="node-table__meta">{meta}</span>
                    </span>
                </button>
            </td>
            <td class="node-table__cell node-table__cell--meta">
                <span class="pill">{count}</span>
                " "
                <span class="duplicates__summary">{summary}</span>
            </td>
            <td class="node-table__cell node-table__cell--size">{size}</td>
            <td class="node-table__cell node-table__cell--modified"></td>
            <td class="node-table__cell node-table__cell--actions"></td>
        </tr>
        {move || {
            open.get()
                .then(|| {
                    group
                        .read_value()
                        .files
                        .iter()
                        .cloned()
                        .map(|file| view! { <CopyRow file group deleting /> })
                        .collect_view()
                })
        }}
    }
}

#[component]
fn CopyRow(
    file: DuplicateFile,
    group: StoredValue<DuplicateGroup>,
    deleting: RwSignal<Option<Vec<Node>>>,
) -> impl IntoView {
    let node = file.node.clone();
    let id = node.id;
    let size = human_size(node.size);
    let modified = short_date(node.updated_at);
    let meta = location_label(&file.location);
    let keep = Callback::new(move |()| {
        deleting.set(Some(group.with_value(|group| others(group, id))));
    });
    let delete = {
        let node = node.clone();
        Callback::new(move |()| deleting.set(Some(vec![node.clone()])))
    };

    view! {
        <tr class="node-table__row duplicates__copy">
            <td class="node-table__cell duplicates__copy-name">
                <FileName node=node.clone() meta />
            </td>
            <td class="node-table__cell node-table__cell--meta">
                <FileLocation file />
            </td>
            <td class="node-table__cell node-table__cell--size">{size}</td>
            <td class="node-table__cell node-table__cell--modified">{modified}</td>
            <td class="node-table__cell node-table__cell--actions">
                <span class="node-table__actions">
                    <Menu id=format!("duplicate-menu-{id}") label="File actions">
                        <OpenLinks node=node.clone() />
                        <MenuItem icon="check_circle" label=KEEP_ONLY_THIS on_select=keep />
                        <MenuItem icon="delete" label="Delete" danger=true on_select=delete />
                    </Menu>
                </span>
            </td>
        </tr>
    }
}
