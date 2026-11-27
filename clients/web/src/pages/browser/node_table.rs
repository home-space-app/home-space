use std::collections::HashSet;

use leptos::{html::Input, prelude::*};
use leptos_router::components::A;
use web_sys::KeyboardEvent;

use client_core::{
    format::human_size,
    listing::{Sort, SortKey},
};
use protocol::{FolderLock, Node, NodeId, NodeKind, ProtectionState};

use crate::components::{Menu, MenuItem};
use crate::{
    components::{RowCheckbox, SelectAll, select_on_click},
    icon::{Icon, node_icon},
    locale::short_date,
    pages::viewer::use_view_link,
    protected::{Vault, use_vault},
    route,
    viewers::viewers,
};

const KEY_ENTER: &str = "Enter";
const KEY_ESCAPE: &str = "Escape";

const ICON_FOLDER: &str = "folder";
const ICON_LOCKED: &str = "lock";

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
    pub share: Callback<Vec<Node>>,
    pub details: Callback<Node>,
    pub delete: Callback<Vec<Node>>,
    pub lost_and_found: Signal<Option<NodeId>>,
    /// The listing is inside a protected folder: files open decrypted, and nothing is shared,
    /// sent or moved out.
    pub inside_protected: Signal<bool>,
    /// The listing is inside a folder being unprotected: nothing is renamed or moved.
    pub frozen: Signal<bool>,
    pub protect: Callback<Node>,
    pub unprotect: Callback<Node>,
    pub unlock: Callback<Node>,
    pub lock: Callback<NodeId>,
    pub change_password: Callback<Node>,
    /// Goes on protecting or unprotecting a folder that stopped halfway.
    pub finish: Callback<NodeId>,
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
        share,
        details,
        delete,
        lost_and_found,
        inside_protected,
        frozen,
        protect,
        unprotect,
        unlock,
        lock,
        change_password,
        finish,
        ..
    } = actions;
    let is_folder = node.kind == NodeKind::Folder;
    let vault = use_vault();
    let node_id = node.id;
    let protection = Memo::new(move |_| {
        if is_folder {
            vault.protection_of(node_id)
        } else {
            None
        }
    });
    let is_protected = move || protection.read().is_some();
    let unlocked = move || is_protected() && vault.is_unlocked(node_id);
    let is_password = move || {
        protection
            .read()
            .as_ref()
            .is_some_and(|folder| folder.lock == FolderLock::Password)
    };
    let state = move || protection.read().as_ref().map(|folder| folder.state);
    let finish_label = move || match state() {
        Some(ProtectionState::Unprotecting) => "Finish unprotecting",
        _ => "Finish protecting",
    };
    let plain_icon = node_icon(&node);
    let icon = move || {
        if is_protected() {
            ICON_LOCKED
        } else {
            plain_icon
        }
    };
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
    // Inside a protected folder the listing gives a file the type its opened name says, and
    // the viewer decrypts it.
    let viewable = viewers().viewer_for_node(&node).is_some();
    let href = if is_folder {
        route::folder(node.id)
    } else {
        use_view_link().href(node.id)
    };
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
                            <Icon name=icon() />
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
                        <A
                            href=href
                            scroll=is_folder
                            attr:class="node-table__name node-table__name--link"
                        >
                            <Icon name=icon() />
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
                            <Icon name=icon() />
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
                            <Show when=move || is_protected() && !unlocked()>
                                <MenuItem
                                    icon="lock"
                                    label=if is_password() { "Unlock…" } else { "Open in the desktop app" }
                                    on_select=Callback::new(move |()| unlock.run(target.get_value()))
                                />
                            </Show>
                            <Show when=unlocked>
                                <MenuItem
                                    icon="lock"
                                    label="Lock"
                                    on_select=Callback::new(move |()| lock.run(node_id))
                                />
                            </Show>
                            <Show when=move || unlocked() && state() != Some(ProtectionState::Protected)>
                                <MenuItem
                                    icon="lock"
                                    label=finish_label()
                                    on_select=Callback::new(move |()| finish.run(node_id))
                                />
                            </Show>
                            <Show when=move || unlocked() && state() == Some(ProtectionState::Protected)>
                                <MenuItem
                                    icon="lock"
                                    label="Unprotect…"
                                    on_select=Callback::new(move |()| unprotect.run(target.get_value()))
                                />
                            </Show>
                            <Show when=is_password>
                                <MenuItem
                                    icon="edit"
                                    label="Change password…"
                                    on_select=Callback::new(move |()| change_password.run(target.get_value()))
                                />
                            </Show>
                            <Show when=move || {
                                is_folder && !is_protected() && !inside_protected.get()
                                    && lost_and_found.get() != Some(node_id) && Vault::available()
                            }>
                                <MenuItem
                                    icon="lock"
                                    label="Protect with password…"
                                    on_select=Callback::new(move |()| protect.run(target.get_value()))
                                />
                            </Show>
                            <Show when=move || !is_protected() && !inside_protected.get()>
                                <MenuItem
                                    icon="share"
                                    label="Share…"
                                    on_select=Callback::new(move |()| share.run(vec![target.get_value()]))
                                />
                                <MenuItem
                                    icon="send"
                                    label="Send to device…"
                                    on_select=Callback::new(move |()| send.run(vec![target.get_value()]))
                                />
                            </Show>
                            <Show when=move || !inside_protected.get()>
                                <MenuItem
                                    icon="info"
                                    label="Details"
                                    on_select=Callback::new(move |()| details.run(target.get_value()))
                                />
                            </Show>
                            <Show when=move || !frozen.get()>
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
                            </Show>
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
