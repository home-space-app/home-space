use std::collections::HashSet;

use leptos::{html::Input, prelude::*};
use leptos_router::components::A;
use web_sys::KeyboardEvent;

use client_core::{
    format::human_size,
    listing::{Sort, SortKey},
};
use protocol::{Node, NodeId, NodeKind};

use crate::components::{Menu, MenuItem};
use crate::{
    components::{RowCheckbox, SelectAll, select_on_click},
    icon::{Icon, node_icon},
    locale::short_date,
    route,
    viewers::viewers,
};

const KEY_ENTER: &str = "Enter";
const KEY_ESCAPE: &str = "Escape";

const ICON_FOLDER: &str = "folder";

#[derive(Clone, Copy)]
pub struct TableActions {
    pub draft: RwSignal<Option<String>>,
    pub renaming: RwSignal<Option<(NodeId, String)>>,
    pub saving: RwSignal<bool>,
    pub moving: RwSignal<Option<Vec<Node>>>,
    pub selected: RwSignal<HashSet<NodeId>>,
    pub create_folder: Callback<()>,
    pub cancel_folder: Callback<()>,
    pub save_rename: Callback<()>,
    pub cancel_rename: Callback<()>,
    pub download: Callback<Node>,
    pub send: Callback<Vec<Node>>,
    pub details: Callback<Node>,
    pub delete: Callback<Vec<Node>>,
    pub lost_and_found: Signal<Option<NodeId>>,
}

#[component]
pub fn NodeTable(
    nodes: Memo<Vec<Node>>,
    sort: RwSignal<Sort>,
    actions: TableActions,
    draft_input: NodeRef<Input>,
) -> impl IntoView {
    view! {
        <table class="node-table">
            <thead class="node-table__head">
                <tr>
                    <th class="node-table__header-cell node-table__header-cell--select">
                        <SelectAll
                            ids=Signal::derive(move || nodes.read().iter().map(|node| node.id).collect())
                            selected=actions.selected
                        />
                    </th>
                    <SortHeader sort key=SortKey::Name label="Name" />
                    <SortHeader
                        sort
                        key=SortKey::Size
                        label="Size"
                        class="node-table__header-cell--size"
                    />
                    <SortHeader
                        sort
                        key=SortKey::Modified
                        label="Modified"
                        class="node-table__header-cell--modified"
                    />
                    <th class="node-table__header-cell">
                        <span class="visually-hidden">"Actions"</span>
                    </th>
                </tr>
            </thead>
            <tbody class="node-table__body">
                <Show when=move || actions.draft.read().is_some()>
                    <DraftRow actions draft_input />
                </Show>
                <For
                    each=move || nodes.get()
                    key=|node| (node.id, node.updated_at.unix_timestamp_nanos())
                    children=move |node| view! { <NodeRow node actions /> }
                />
            </tbody>
        </table>
    }
}

#[component]
fn SortHeader(
    sort: RwSignal<Sort>,
    key: SortKey,
    label: &'static str,
    #[prop(optional)] class: &'static str,
) -> impl IntoView {
    let active = move || sort.get().key == key;
    let aria_sort = move || match (active(), sort.get().descending) {
        (false, _) => "none",
        (true, false) => "ascending",
        (true, true) => "descending",
    };
    let arrow = move || {
        if sort.get().descending {
            "\u{2193}"
        } else {
            "\u{2191}"
        }
    };
    view! {
        <th class=format!("node-table__header-cell {class}") aria-sort=aria_sort>
            <button
                class="node-table__sort"
                class:node-table__sort--active=active
                on:click=move |_| sort.update(|sort| *sort = sort.toggled(key))
            >
                {label}
                <Show when=active>
                    <span class="node-table__sort-arrow">{arrow}</span>
                </Show>
            </button>
        </th>
    }
}

#[component]
fn DraftRow(actions: TableActions, draft_input: NodeRef<Input>) -> impl IntoView {
    let TableActions {
        draft,
        saving,
        create_folder,
        cancel_folder,
        ..
    } = actions;

    view! {
        <tr class="node-table__row node-table__row--draft">
            <td class="node-table__cell node-table__cell--select"></td>
            <td class="node-table__cell">
                <span class="node-table__name">
                    <Icon name=ICON_FOLDER />
                    <input
                        node_ref=draft_input
                        class="field__input node-table__input"
                        type="text"
                        placeholder="Folder name"
                        aria-label="Folder name"
                        prop:value=move || draft.get().unwrap_or_default()
                        disabled=move || saving.get()
                        on:input:target=move |event| draft.set(Some(event.target().value()))
                        on:keydown=move |event: KeyboardEvent| {
                            match event.key().as_str() {
                                KEY_ENTER => create_folder.run(()),
                                KEY_ESCAPE => cancel_folder.run(()),
                                _ => {}
                            }
                        }
                    />
                </span>
            </td>
            <td class="node-table__cell node-table__cell--size"></td>
            <td class="node-table__cell node-table__cell--modified"></td>
            <td class="node-table__cell node-table__cell--actions">
                <div class="node-table__actions">
                    <button
                        class="button button--primary button--small"
                        disabled=move || saving.get()
                        on:click=move |_| create_folder.run(())
                    >
                        "Create"
                    </button>
                    <button
                        class="button button--ghost button--small"
                        disabled=move || saving.get()
                        on:click=move |_| cancel_folder.run(())
                    >
                        "Cancel"
                    </button>
                </div>
            </td>
        </tr>
    }
}

#[component]
fn NodeRow(node: Node, actions: TableActions) -> impl IntoView {
    let TableActions {
        renaming,
        saving,
        moving,
        selected,
        save_rename,
        cancel_rename,
        download,
        send,
        details,
        delete,
        lost_and_found,
        ..
    } = actions;
    let is_folder = node.kind == NodeKind::Folder;
    let icon = node_icon(&node);
    let name = node.name.clone();
    let size = if is_folder {
        String::new()
    } else {
        human_size(node.size)
    };
    let modified = short_date(node.updated_at);
    let meta = if is_folder {
        modified.clone()
    } else {
        format!("{size} \u{b7} {modified}")
    };
    let viewable = viewers().viewer_for_node(&node).is_some();
    let href = if is_folder {
        route::folder(node.id)
    } else {
        route::file_view(node.id)
    };
    let node_id = node.id;
    let editing = Memo::new(move |_| {
        renaming
            .read()
            .as_ref()
            .is_some_and(|(id, _)| *id == node_id)
    });
    let rename_input = NodeRef::<Input>::new();
    Effect::new(move |_| {
        if let Some(input) = rename_input.get() {
            let _ = input.focus();
            input.select();
        }
    });
    view! {
        <tr
            class="node-table__row"
            class:node-table__row--selected=move || selected.read().contains(&node_id)
            on:click=select_on_click(node_id, selected)
        >
            <td class="node-table__cell node-table__cell--select">
                <RowCheckbox id=node_id name=node.name.clone() selected />
            </td>
            <td class="node-table__cell">
                {move || if editing.get() {
                    view! {
                        <span class="node-table__name node-table__name--editing">
                            <Icon name=icon />
                            <input
                                node_ref=rename_input
                                class="field__input node-table__input"
                                type="text"
                                aria-label="New name"
                                prop:value=move || {
                                    renaming
                                        .read()
                                        .as_ref()
                                        .map(|(_, name)| name.clone())
                                        .unwrap_or_default()
                                }
                                disabled=move || saving.get()
                                on:input:target=move |event| {
                                    renaming.set(Some((node_id, event.target().value())))
                                }
                                on:keydown=move |event: KeyboardEvent| {
                                    match event.key().as_str() {
                                        KEY_ENTER => save_rename.run(()),
                                        KEY_ESCAPE => cancel_rename.run(()),
                                        _ => {}
                                    }
                                }
                            />
                        </span>
                    }
                        .into_any()
                } else if is_folder || viewable {
                    let (href, name, meta) = (href.clone(), name.clone(), meta.clone());
                    view! {
                        <A href=href attr:class="node-table__name node-table__name--link">
                            <Icon name=icon />
                            <span class="node-table__text">
                                {name}
                                <span class="node-table__meta">{meta}</span>
                            </span>
                        </A>
                    }
                        .into_any()
                } else {
                    view! {
                        <span class="node-table__name">
                            <Icon name=icon />
                            <span class="node-table__text">
                                {name.clone()}
                                <span class="node-table__meta">{meta.clone()}</span>
                            </span>
                        </span>
                    }
                        .into_any()
                }}
            </td>
            <td class="node-table__cell node-table__cell--size">{size}</td>
            <td class="node-table__cell node-table__cell--modified">{modified}</td>
            <td class="node-table__cell node-table__cell--actions">
                {move || if editing.get() {
                    view! {
                        <div class="node-table__actions">
                            <button
                                class="button button--primary button--small"
                                disabled=move || saving.get()
                                on:click=move |_| save_rename.run(())
                            >
                                "Save"
                            </button>
                            <button
                                class="button button--ghost button--small"
                                disabled=move || saving.get()
                                on:click=move |_| cancel_rename.run(())
                            >
                                "Cancel"
                            </button>
                        </div>
                    }
                        .into_any()
                } else {
                    let target = StoredValue::new(node.clone());
                    view! {
                        <Menu
                            id=format!("node-menu-{}", route::short_id(node_id))
                            label=format!("Actions for {}", node.name)
                        >
                            {(!is_folder)
                                .then(|| {
                                    view! {
                                        <MenuItem
                                            icon="download"
                                            label="Download"
                                            on_select=Callback::new(move |()| {
                                                download.run(target.get_value())
                                            })
                                        />
                                    }
                                })}
                            <MenuItem
                                icon="send"
                                label="Send to device…"
                                on_select=Callback::new(move |()| send.run(vec![target.get_value()]))
                            />
                            <MenuItem
                                icon="info"
                                label="Details"
                                on_select=Callback::new(move |()| details.run(target.get_value()))
                            />
                            <MenuItem
                                icon="edit"
                                label="Rename"
                                on_select=Callback::new(move |()| {
                                    let node = target.get_value();
                                    renaming.set(Some((node.id, node.name)))
                                })
                            />
                            <MenuItem
                                icon="drive_file_move"
                                label="Move"
                                on_select=Callback::new(move |()| {
                                    moving.set(Some(vec![target.get_value()]))
                                })
                            />
                            <Show when=move || lost_and_found.get() != Some(node_id)>
                                <MenuItem
                                    icon="delete"
                                    label="Delete"
                                    danger=true
                                    on_select=Callback::new(move |()| {
                                        delete.run(vec![target.get_value()])
                                    })
                                />
                            </Show>
                        </Menu>
                    }
                        .into_any()
                }}
            </td>
        </tr>
    }
}
