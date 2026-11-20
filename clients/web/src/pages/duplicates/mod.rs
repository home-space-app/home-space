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

/// Which list the page shows; identical files first.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Copies,
    Outdated,
}

impl View {
    const ALL: [Self; 2] = [Self::Copies, Self::Outdated];

    fn label(self) -> &'static str {
        match self {
            Self::Copies => COPIES_TITLE,
            Self::Outdated => OUTDATED_TITLE,
        }
    }

    fn tab_id(self) -> &'static str {
        match self {
            Self::Copies => "duplicates-copies",
            Self::Outdated => "duplicates-outdated",
        }
    }
}

#[component]
pub fn DuplicatesPage(auth: Auth) -> impl IntoView {
    let report = RwSignal::new(DuplicatesReport::default());
    let loaded = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let expanded = RwSignal::new(HashSet::<String>::new());
    let deleting = RwSignal::new(None::<Vec<Node>>);
    let showing = RwSignal::new(View::Copies);
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
                <div class="segmented duplicates__switch" role="tablist" aria-label="Show">
                    {View::ALL
                        .into_iter()
                        .map(|choice| {
                            let chosen = move || showing.get() == choice;
                            let count = move || match choice {
                                View::Copies => report.read().groups.len(),
                                View::Outdated => report.read().outdated.len(),
                            };
                            view! {
                                <button
                                    type="button"
                                    role="tab"
                                    id=choice.tab_id()
                                    aria-controls="duplicates-panel"
                                    class="segmented__option duplicates__tab"
                                    class:segmented__option--active=chosen
                                    aria-selected=move || chosen().to_string()
                                    on:click=move |_| showing.set(choice)
                                >
                                    {choice.label()}
                                    <Show when=move || loaded.get()>
                                        <span class="duplicates__count">{count}</span>
                                    </Show>
                                </button>
                            }
                        })
                        .collect_view()}
                </div>
                <section
                    class="duplicates__section"
                    id="duplicates-panel"
                    role="tabpanel"
                    aria-labelledby=move || showing.get().tab_id()
                >
                    <Show when=move || showing.get() == View::Copies>
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
                    </Show>
                    <Show when=move || showing.get() == View::Outdated>
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
