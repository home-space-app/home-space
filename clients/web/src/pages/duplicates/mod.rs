//! The settings tab listing files that share their content: identical live copies, and live
//! files holding an old version of another file.

mod file;
mod groups;
mod outdated;

use std::collections::HashSet;

use leptos::{prelude::*, task::spawn_local};

use client_core::duplicates::{
    COPIES_TITLE, NO_COPIES, NO_OUTDATED, OUTDATED_HINT, OUTDATED_TITLE,
};
use protocol::{DuplicatesReport, Event, Node};

use super::{delete_dialog::DeleteDialog, settings::SettingsHeader};
use crate::{
    components::ErrorAlert,
    events, load,
    session::{self, Auth},
};
use groups::GroupRows;
use outdated::OutdatedRow;

#[component]
pub fn DuplicatesPage(auth: Auth) -> impl IntoView {
    let report = RwSignal::new(DuplicatesReport::default());
    let loaded = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let expanded = RwSignal::new(HashSet::<String>::new());
    let deleting = RwSignal::new(None::<Vec<Node>>);
    let list = load::load(
        || async { session::client().duplicates().await },
        move |result| {
            loaded.set(true);
            match result {
                Ok(found) => {
                    report.set(found);
                    error.set(None);
                }
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        },
    );
    events::use_events(move |event| {
        if matches!(
            event,
            Event::NodeCreated { .. }
                | Event::NodeUpdated { .. }
                | Event::NodeDeleted { .. }
                | Event::TrashChanged
                | Event::Resync
        ) {
            list.refetch();
        }
    });
    let confirm_delete = Callback::new(move |targets: Vec<Node>| {
        spawn_local(async move {
            let client = session::client();
            for node in targets {
                if let Err(err) = client.delete(node.id).await {
                    error.set(Some(auth.fail(&err)));
                    break;
                }
            }
            list.refetch();
        });
    });
    let no_groups = move || loaded.get() && report.read().groups.is_empty();
    let no_outdated = move || loaded.get() && report.read().outdated.is_empty();

    view! {
        <main class="explorer explorer--tabbed">
            <div class="explorer__header explorer__header--stacked">
                <SettingsHeader />
            </div>
            <ErrorAlert message=error />
            <div class="explorer__scroll duplicates">
                <section class="duplicates__section" aria-labelledby="duplicates-copies">
                    <h2 class="duplicates__title" id="duplicates-copies">{COPIES_TITLE}</h2>
                    <Show when=move || !report.read().groups.is_empty()>
                        <table class="node-table duplicates__table">
                            <thead class="node-table__head">
                                <tr>
                                    <th class="node-table__header-cell">"Name"</th>
                                    <th class="node-table__header-cell">"Location"</th>
                                    <th class="node-table__header-cell node-table__header-cell--size">
                                        "Size"
                                    </th>
                                    <th class="node-table__header-cell node-table__header-cell--modified">
                                        "Modified"
                                    </th>
                                    <th class="node-table__header-cell node-table__cell--actions"></th>
                                </tr>
                            </thead>
                            <tbody class="node-table__body">
                                <For
                                    each=move || report.get().groups
                                    key=|group| {
                                        (
                                            group.sha256.clone(),
                                            group
                                                .files
                                                .iter()
                                                .map(|file| (file.node.id, file.location.clone()))
                                                .collect::<Vec<_>>(),
                                        )
                                    }
                                    children=move |group| {
                                        view! { <GroupRows group expanded deleting /> }
                                    }
                                />
                            </tbody>
                        </table>
                    </Show>
                    <Show when=no_groups>
                        <p class="explorer__empty">{NO_COPIES}</p>
                    </Show>
                </section>
                <section class="duplicates__section" aria-labelledby="duplicates-outdated">
                    <h2 class="duplicates__title" id="duplicates-outdated">{OUTDATED_TITLE}</h2>
                    <p class="duplicates__hint">{OUTDATED_HINT}</p>
                    <Show when=move || !report.read().outdated.is_empty()>
                        <table class="node-table duplicates__table">
                            <thead class="node-table__head">
                                <tr>
                                    <th class="node-table__header-cell">"Name"</th>
                                    <th class="node-table__header-cell">"Location"</th>
                                    <th class="node-table__header-cell">"Matches"</th>
                                    <th class="node-table__header-cell node-table__header-cell--modified">
                                        "Modified"
                                    </th>
                                    <th class="node-table__header-cell node-table__cell--actions"></th>
                                </tr>
                            </thead>
                            <tbody class="node-table__body">
                                <For
                                    each=move || report.get().outdated
                                    key=|found| (found.file.node.id, found.source.node.id, found.version)
                                    children=move |found| view! { <OutdatedRow found deleting /> }
                                />
                            </tbody>
                        </table>
                    </Show>
                    <Show when=no_outdated>
                        <p class="explorer__empty">{NO_OUTDATED}</p>
                    </Show>
                </section>
            </div>
        </main>
        {move || {
            deleting
                .get()
                .map(|nodes| view! { <DeleteDialog nodes deleting on_confirm=confirm_delete /> })
        }}
    }
}
