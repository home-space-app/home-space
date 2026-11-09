use std::collections::HashSet;

use leptos::{prelude::*, task::spawn_local};
use leptos_router::components::A;

use client_core::{
    format::{conflicts_title, human_size},
    labels::{LOST_AND_FOUND, files},
};
use protocol::{Conflict, Event, NodeId, OnConflict, TrashItem};

use super::conflict_dialog::ConflictDialog;
use crate::{
    components::{ErrorAlert, RowCheckbox, SelectAll, select_on_click},
    events,
    icon::{Icon, node_icon},
    load,
    locale::short_date,
    route,
    session::{self, Auth},
};

/// What a finished restore reports: how many files came back and where to.
#[derive(Clone)]
struct Restored {
    count: usize,
    folder: Option<NodeId>,
}

#[component]
pub fn TrashPage(auth: Auth) -> impl IntoView {
    let items = RwSignal::new(Vec::<TrashItem>::new());
    let selected = RwSignal::new(HashSet::<NodeId>::new());
    let error = RwSignal::new(None::<String>);
    let restored = RwSignal::new(None::<Restored>);
    let conflicts = RwSignal::new(None::<(String, Vec<Conflict>)>);
    let emptying = RwSignal::new(None::<usize>);

    let trash = load::load(
        || async { session::client().trash().await },
        move |result| match result {
            Ok(listing) => {
                items.set(listing);
                error.set(None);
            }
            Err(err) => error.set(Some(auth.fail(&err))),
        },
    );

    events::use_events(move |event| {
        if matches!(event, Event::TrashChanged | Event::Resync) {
            trash.refetch();
        }
    });

    let restore = Callback::new(move |mode: OnConflict| {
        let ids: Vec<NodeId> = items
            .read_untracked()
            .iter()
            .map(|item| item.node.id)
            .filter(|id| selected.read_untracked().contains(id))
            .collect();
        if ids.is_empty() {
            return;
        }
        spawn_local(async move {
            match session::client().restore_trashed(&ids, mode).await {
                Ok(response) if !response.conflicts.is_empty() => {
                    let title = conflicts_title(response.conflicts.len(), LOST_AND_FOUND);
                    conflicts.set(Some((title, response.conflicts)));
                }
                Ok(response) => {
                    error.set(None);
                    restored.set(Some(Restored {
                        count: response.moved.len(),
                        folder: response.target.map(|folder| folder.id),
                    }));
                    selected.update(HashSet::clear);
                    trash.refetch();
                }
                Err(err) => {
                    error.set(Some(auth.fail(&err)));
                    trash.refetch();
                }
            }
        });
    });
    let choose = Callback::new(move |mode: Option<OnConflict>| {
        conflicts.set(None);
        if let Some(mode) = mode {
            restore.run(mode);
        }
    });
    let confirm_empty = move |_| {
        emptying.set(None);
        spawn_local(async move {
            match session::client().empty_trash().await {
                Ok(()) => {
                    restored.set(None);
                    selected.update(HashSet::clear);
                }
                Err(err) => error.set(Some(auth.fail(&err))),
            }
            trash.refetch();
        });
    };

    let selected_count = move || {
        let ids = selected.read();
        items
            .read()
            .iter()
            .filter(|item| ids.contains(&item.node.id))
            .count()
    };
    let is_empty = move || items.read().is_empty();

    view! {
        <main class="explorer">
            <div class="explorer__header">
                <h1 class="page-title">"Recycle bin"</h1>
                <div class="explorer__toolbar">
                    <button
                        class="button button--primary"
                        disabled=move || selected_count() == 0
                        on:click=move |_| {
                            restored.set(None);
                            restore.run(OnConflict::Abort)
                        }
                    >
                        <Icon name="restore_from_trash" />
                        "Restore"
                    </button>
                    <button
                        class="button button--danger"
                        disabled=is_empty
                        on:click=move |_| emptying.set(Some(items.read_untracked().len()))
                    >
                        <Icon name="delete_forever" />
                        "Empty recycle bin"
                    </button>
                </div>
            </div>
            <ErrorAlert message=error />
            {move || {
                restored
                    .get()
                    .map(|done| {
                        let message = format!("Restored {} to ", files(done.count));
                        let place = match done.folder {
                            Some(id) => {
                                view! { <A href=route::folder(id)>{LOST_AND_FOUND}</A> }.into_any()
                            }
                            None => LOST_AND_FOUND.into_any(),
                        };
                        view! {
                            <p class="alert alert--success" role="status">
                                {message} {place}
                            </p>
                        }
                    })
            }}
            <div class="explorer__scroll">
                <table class="node-table">
                    <thead class="node-table__head">
                        <tr>
                            <th class="node-table__header-cell node-table__header-cell--select">
                                <SelectAll
                                    ids=Signal::derive(move || {
                                        items.read().iter().map(|item| item.node.id).collect()
                                    })
                                    selected
                                />
                            </th>
                            <th class="node-table__header-cell">"Name"</th>
                            <th class="node-table__header-cell node-table__header-cell--modified">
                                "Original path"
                            </th>
                            <th class="node-table__header-cell node-table__header-cell--modified">
                                "Deleted"
                            </th>
                            <th class="node-table__header-cell node-table__header-cell--size">
                                "Size"
                            </th>
                        </tr>
                    </thead>
                    <tbody class="node-table__body">
                        <For
                            each=move || items.get()
                            key=|item| item.node.id
                            children=move |item| view! { <TrashRow item selected /> }
                        />
                    </tbody>
                </table>
                <Show when=move || is_empty() && error.read().is_none()>
                    <p class="explorer__empty">"The recycle bin is empty."</p>
                </Show>
            </div>
        </main>
        {move || {
            conflicts
                .get()
                .map(|(title, conflicts)| {
                    view! { <ConflictDialog title conflicts on_choose=choose /> }
                })
        }}
        {move || {
            emptying
                .get()
                .map(|count| {
                    view! {
                        <div class="modal" on:click=move |_| emptying.set(None)>
                            <div
                                class="modal__panel"
                                role="alertdialog"
                                aria-modal="true"
                                on:click=|event| event.stop_propagation()
                            >
                                <header class="modal__header">
                                    <h2 class="modal__title">
                                        {format!("Permanently delete {}?", files(count))}
                                    </h2>
                                    <p class="modal__subtitle">"This cannot be undone."</p>
                                </header>
                                <div class="modal__actions">
                                    <button class="button" on:click=move |_| emptying.set(None)>
                                        "Cancel"
                                    </button>
                                    <button class="button button--danger" on:click=confirm_empty>
                                        "Empty recycle bin"
                                    </button>
                                </div>
                            </div>
                        </div>
                    }
                })
        }}
    }
}

#[component]
fn TrashRow(item: TrashItem, selected: RwSignal<HashSet<NodeId>>) -> impl IntoView {
    let node_id = item.node.id;
    let icon = node_icon(&item.node);
    let meta = format!(
        "{} \u{b7} {} \u{b7} {}",
        item.original_path,
        short_date(item.deleted_at),
        human_size(item.node.size)
    );
    view! {
        <tr
            class="node-table__row"
            class:node-table__row--selected=move || selected.read().contains(&node_id)
            on:click=select_on_click(node_id, selected)
        >
            <td class="node-table__cell node-table__cell--select">
                <RowCheckbox id=node_id name=item.node.name.clone() selected />
            </td>
            <td class="node-table__cell">
                <span class="node-table__name">
                    <Icon name=icon />
                    <span class="node-table__text">
                        {item.node.name.clone()}
                        <span class="node-table__meta">{meta}</span>
                    </span>
                </span>
            </td>
            <td class="node-table__cell node-table__cell--modified">{item.original_path}</td>
            <td class="node-table__cell node-table__cell--modified">
                {short_date(item.deleted_at)}
            </td>
            <td class="node-table__cell node-table__cell--size">{human_size(item.node.size)}</td>
        </tr>
    }
}
