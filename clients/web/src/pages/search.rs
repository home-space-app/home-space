use leptos::prelude::*;
use leptos_router::{components::A, hooks::use_query_map};

use client_core::{format::human_size, labels::location_label, search::destination};
use protocol::{NodeKind, SearchHit};

use super::search_box::SearchBox;
use crate::{
    components::ErrorAlert,
    icon::{Icon, node_icon},
    load,
    locale::short_date,
    route,
    session::{self, Auth},
    viewers::viewers,
};

const HOME_PATH: &str = "/";

#[derive(Clone, PartialEq)]
enum Status {
    Idle,
    Loading,
    Done,
    Failed(String),
}

/// Where a hit leads: a file with a built-in viewer opens in it, anything else in its folder.
fn target(hit: &SearchHit) -> String {
    if viewers().viewer_for_node(&hit.node).is_some() {
        route::file_view(hit.node.id)
    } else {
        destination(hit).map_or_else(|| HOME_PATH.to_owned(), route::folder)
    }
}

#[component]
pub fn SearchPage(auth: Auth) -> impl IntoView {
    let params = use_query_map();
    let term = Memo::new(move |_| {
        params
            .read()
            .get(route::SEARCH_PARAM)
            .unwrap_or_default()
            .trim()
            .to_owned()
    });
    let hits = RwSignal::new(Vec::<SearchHit>::new());
    let status = RwSignal::new(Status::Idle);

    Effect::new(move |_| {
        if term.read().is_empty() {
            hits.set(Vec::new());
            status.set(Status::Idle);
        } else {
            status.set(Status::Loading);
        }
    });
    load::load(
        move || {
            let wanted = term.get();
            async move {
                if wanted.is_empty() {
                    None
                } else {
                    Some(session::client().search(&wanted).await)
                }
            }
        },
        move |outcome| match outcome {
            Some(Ok(found)) => {
                hits.set(found);
                status.set(Status::Done);
            }
            Some(Err(err)) => status.set(Status::Failed(auth.fail(&err))),
            None => {}
        },
    );
    let failure = Signal::derive(move || match status.get() {
        Status::Failed(message) => Some(message),
        _ => None,
    });

    view! {
        <main class="explorer">
            <div class="explorer__header">
                <h1 class="page-title">
                    {move || {
                        let term = term.get();
                        if term.is_empty() {
                            "Search".to_owned()
                        } else {
                            format!("Results for \u{201c}{term}\u{201d}")
                        }
                    }}
                </h1>
                <SearchBox />
            </div>
            <ErrorAlert message=failure />
            <div class="explorer__scroll">
                <Show when=move || !hits.read().is_empty()>
                    <table class="node-table">
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
                            </tr>
                        </thead>
                        <tbody class="node-table__body">
                            <For
                                each=move || hits.get()
                                key=|hit| (hit.node.id, hit.node.updated_at.unix_timestamp_nanos())
                                children=|hit| view! { <HitRow hit /> }
                            />
                        </tbody>
                    </table>
                </Show>
                <Show when=move || status.get() == Status::Loading && hits.read().is_empty()>
                    <p class="explorer__empty">"Searching\u{2026}"</p>
                </Show>
                <Show when=move || status.get() == Status::Done && hits.read().is_empty()>
                    <p class="explorer__empty">"Nothing matches your search."</p>
                </Show>
                <Show when=move || status.get() == Status::Idle>
                    <p class="explorer__empty">"Type a file or folder name to search."</p>
                </Show>
            </div>
        </main>
    }
}

#[component]
fn HitRow(hit: SearchHit) -> impl IntoView {
    let href = target(&hit);
    let icon = node_icon(&hit.node);
    let is_folder = hit.node.kind == NodeKind::Folder;
    let size = if is_folder {
        String::new()
    } else {
        human_size(hit.node.size)
    };
    let modified = short_date(hit.node.updated_at);
    let location = location_label(&hit.location);
    let meta = if is_folder {
        location.clone()
    } else {
        format!("{location} \u{b7} {size}")
    };
    let name = hit.node.name;

    view! {
        <tr class="node-table__row">
            <td class="node-table__cell">
                <A href=href attr:class="node-table__name node-table__name--link">
                    <Icon name=icon />
                    <span class="node-table__text">
                        {name}
                        <span class="node-table__meta">{meta}</span>
                    </span>
                </A>
            </td>
            <td class="node-table__cell node-table__cell--meta search-results__location">
                {location}
            </td>
            <td class="node-table__cell node-table__cell--size">{size}</td>
            <td class="node-table__cell node-table__cell--modified">{modified}</td>
        </tr>
    }
}
