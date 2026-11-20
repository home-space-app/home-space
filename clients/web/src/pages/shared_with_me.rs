//! "Shared with me", `/shared`: what other users shared with the user, read-only. Folders
//! open at `/shared/<share>?path=…` and files in their viewer with `&view=…`; files and
//! folders download, or are saved as copies into the user's own files.

use leptos::{prelude::*, task::spawn_local};
use leptos_router::{
    components::A,
    hooks::{use_params_map, use_query_map},
};
use uuid::Uuid;

use api_client::ApiError;
use client_core::{
    format::human_size,
    received::{
        ACCESS, EXPIRES, NEVER, NOTHING_SHARED_WITH_ME, SAVE_TO_MY_FILES, SHARED_BY,
        SHARED_WITH_ME, SharedItem, VIEW_ONLY, find_share, save_copy_title, shared_items,
        shared_viewer,
    },
    shared_link::{DOWNLOAD, EMPTY_FOLDER, child_folder, initial, shared_by, sort_shared},
};
use protocol::{NodeId, NodeKind, ReceivedShare, SharedNode, TicketRequest};

use super::{SaveDialog, SaveSource, viewer::SharedViewer};
use crate::{
    components::{ErrorAlert, Menu, MenuItem},
    icon::{Icon, kind_icon},
    load,
    locale::{medium_date, short_date},
    route,
    session::{self, Auth},
    viewers::viewers,
};

const SHARE_PARAM: &str = "share";
const NO_SIZE: &str = "—";
const META_SEPARATOR: &str = " · ";
const SHARE_GONE: &str = "This share is no longer available.";
const FILE_GONE: &str = "This file is no longer in the share.";
const LOADING: &str = "Loading…";

/// Items picked to be saved into the user's files, and the share they come from.
#[derive(Clone)]
struct Saving {
    share: Uuid,
    items: Vec<SharedNode>,
}

/// What the page shows and how it reaches the server.
#[derive(Clone, Copy)]
struct Shared {
    auth: Auth,
    error: RwSignal<Option<String>>,
    saving: RwSignal<Option<Saving>>,
}

impl Shared {
    /// Downloads a file, or a folder as one zip, through a short-lived ticket.
    fn download(self, share: Uuid, item: SharedNode) {
        spawn_local(async move {
            let client = session::client();
            let url = match item.kind {
                NodeKind::File => client
                    .ticket(&TicketRequest::SharedFile {
                        share_id: share,
                        node_id: item.id,
                    })
                    .await
                    .map(|ticket| client.received_file_url(share, item.id, &ticket.ticket)),
                NodeKind::Folder => client
                    .ticket(&TicketRequest::SharedArchive { share_id: share })
                    .await
                    .map(|ticket| {
                        client.received_archive_url(share, Some(item.id), &ticket.ticket)
                    }),
            };
            match url {
                Ok(url) => {
                    if let Some(window) = web_sys::window() {
                        let _ = window.location().set_href(&url);
                    }
                }
                Err(err) => self.fail(&err),
            }
        });
    }

    fn save(self, share: Uuid, item: SharedNode) {
        self.saving.set(Some(Saving {
            share,
            items: vec![item],
        }));
    }

    fn fail(self, err: &ApiError) {
        self.error.set(Some(self.auth.fail(err)));
    }
}

#[component]
pub fn SharedWithMePage(auth: Auth) -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let share_id = Memo::new(move |_| {
        params
            .read()
            .get(SHARE_PARAM)
            .and_then(|id| id.parse::<Uuid>().ok())
    });
    let path = Memo::new(move |_| route::share_trail(&query.read()));
    let view = Memo::new(move |_| {
        query
            .read()
            .get(route::SHARED_VIEW_PARAM)
            .and_then(|id| route::parse_id(&id))
    });
    let viewing = Memo::new(move |_| share_id.read().is_some() && view.read().is_some());
    let shares = RwSignal::new(Vec::<ReceivedShare>::new());
    let loaded = RwSignal::new(false);
    let page = Shared {
        auth,
        error: RwSignal::new(None),
        saving: RwSignal::new(None),
    };
    load::load(
        || async { session::client().shared_with_me().await },
        move |result| {
            loaded.set(true);
            match result {
                Ok(listing) => {
                    shares.set(listing);
                    page.error.set(None);
                }
                Err(err) => page.fail(&err),
            }
        },
    );
    let share = Memo::new(move |_| {
        share_id
            .get()
            .and_then(|id| find_share(&shares.read(), id).cloned())
    });
    let saved = Callback::new(move |_: bool| page.saving.set(None));

    view! {
        <main class="explorer" class:viewer=move || viewing.get()>
            {move || match (share_id.get(), share.get()) {
                (None, _) => view! { <TopLevel page shares loaded /> }.into_any(),
                (Some(_), Some(share)) => view! { <Folder page share path view /> }.into_any(),
                (Some(_), None) => {
                    view! {
                        <div class="explorer__header">
                            <Crumbs share=None trail=Vec::new() />
                        </div>
                        <ErrorAlert message=page.error />
                        <Show when=move || loaded.get() && page.error.read().is_none()>
                            <p class="explorer__empty">{SHARE_GONE}</p>
                        </Show>
                    }
                        .into_any()
                }
            }}
        </main>
        {move || {
            page.saving
                .get()
                .map(|Saving { share, items }| {
                    let names: Vec<&str> = items.iter().map(|item| item.name.as_str()).collect();
                    let title = save_copy_title(&names);
                    let subtitle = find_share(&shares.read_untracked(), share)
                        .map(|share| shared_by(&share.owner.username))
                        .unwrap_or_default();
                    let source = SaveSource::Shared {
                        share,
                        nodes: items.iter().map(|item| item.id).collect(),
                    };
                    view! {
                        <SaveDialog
                            title
                            subtitle
                            source
                            auth
                            on_close=saved
                        />
                    }
                })
        }}
    }
}

/// Every item shared with the user, with who shared it.
#[component]
fn TopLevel(
    page: Shared,
    shares: RwSignal<Vec<ReceivedShare>>,
    loaded: RwSignal<bool>,
) -> impl IntoView {
    let rows = Memo::new(move |_| shared_items(&shares.read()));
    view! {
        <div class="explorer__header">
            <h1 class="page-title">{SHARED_WITH_ME}</h1>
        </div>
        <ErrorAlert message=page.error />
        <div class="explorer__scroll">
            <Show when=move || !rows.read().is_empty()>
                <table class="node-table">
                    <thead class="node-table__head">
                        <tr>
                            <th class="node-table__header-cell">"Name"</th>
                            <th class="node-table__header-cell node-table__header-cell--modified">
                                {SHARED_BY}
                            </th>
                            <th class="node-table__header-cell node-table__header-cell--modified shared__extra">
                                {ACCESS}
                            </th>
                            <th class="node-table__header-cell node-table__header-cell--modified">
                                {EXPIRES}
                            </th>
                            <th class="node-table__header-cell node-table__cell--actions">
                                <span class="visually-hidden">"Actions"</span>
                            </th>
                        </tr>
                    </thead>
                    <tbody class="node-table__body">
                        <For
                            each=move || rows.get()
                            key=|row| (row.share_id, row.item.id)
                            children=move |row| view! { <TopRow page row /> }
                        />
                    </tbody>
                </table>
            </Show>
            <Show when=move || {
                loaded.get() && rows.read().is_empty() && page.error.read().is_none()
            }>
                <p class="explorer__empty">{NOTHING_SHARED_WITH_ME}</p>
            </Show>
        </div>
    }
}

#[component]
fn TopRow(page: Shared, row: SharedItem) -> impl IntoView {
    let SharedItem {
        share_id,
        owner,
        item,
        expires_at,
    } = row;
    let expires = expires_at.map_or_else(|| NEVER.to_owned(), medium_date);
    let meta = format!("{}{META_SEPARATOR}{expires}", owner.username);
    view! {
        <tr class="node-table__row">
            <td class="node-table__cell shared__name-cell">
                <ItemName page share=share_id trail=Vec::new() item=item.clone() meta />
            </td>
            <td class="node-table__cell node-table__cell--modified">
                <span class="shared__owner">
                    <span class="avatar avatar--small" aria-hidden="true">
                        {initial(&owner.username)}
                    </span>
                    {owner.username.clone()}
                </span>
            </td>
            <td class="node-table__cell node-table__cell--modified shared__extra">
                <ViewOnly />
            </td>
            <td class="node-table__cell node-table__cell--modified">{expires}</td>
            <td class="node-table__cell node-table__cell--actions">
                <ItemMenu page share=share_id item />
            </td>
        </tr>
    }
}

/// A folder inside a share: where it is, who shared it, and what it holds.
#[component]
fn Folder(
    page: Shared,
    share: ReceivedShare,
    path: Memo<Vec<NodeId>>,
    view: Memo<Option<NodeId>>,
) -> impl IntoView {
    let share_id = share.id;
    let top = share.items.clone();
    let trail = RwSignal::new(Vec::<SharedNode>::new());
    let items = RwSignal::new(Vec::<SharedNode>::new());
    let loaded = RwSignal::new(false);
    let loads = StoredValue::new(0_u64);
    Effect::new(move |_| {
        let path = path.get();
        let top = top.clone();
        loads.update_value(|loads| *loads += 1);
        let load = loads.get_value();
        spawn_local(async move {
            let opened = open_path(share_id, top, &path).await;
            if loads.get_value() != load {
                return;
            }
            loaded.set(true);
            match opened {
                Ok((opened_trail, opened_items)) => {
                    trail.set(opened_trail);
                    items.set(opened_items);
                }
                Err(err) => page.fail(&err),
            }
        });
    });
    let trail_ids = Memo::new(move |_| {
        trail
            .read()
            .iter()
            .map(|folder| folder.id)
            .collect::<Vec<NodeId>>()
    });
    let opened_file = Memo::new(move |_| {
        view.get().and_then(|id| {
            items
                .read()
                .iter()
                .find(|item| item.id == id && item.kind == NodeKind::File)
                .cloned()
        })
    });

    move || match (view.get(), opened_file.get()) {
        (None, _) => view! { <FolderListing page share_id trail items loaded /> }.into_any(),
        (Some(_), Some(item)) => {
            let download = {
                let item = item.clone();
                Callback::new(move |()| page.download(share_id, item.clone()))
            };
            view! {
                <SharedViewer
                    auth=page.auth
                    share=share_id
                    trail=trail_ids.get_untracked()
                    listing=items.get_untracked()
                    item
                    on_download=download
                />
            }
            .into_any()
        }
        (Some(_), None) => view! {
            <div class="explorer__header">
                {move || view! { <Crumbs share=Some(share_id) trail=trail.get() /> }}
            </div>
            <ErrorAlert message=page.error />
            <p class="explorer__empty">
                {move || if loaded.get() { FILE_GONE } else { LOADING }}
            </p>
        }
        .into_any(),
    }
}

/// What a folder of a share holds, below the way back.
#[component]
fn FolderListing(
    page: Shared,
    share_id: Uuid,
    trail: RwSignal<Vec<SharedNode>>,
    items: RwSignal<Vec<SharedNode>>,
    loaded: RwSignal<bool>,
) -> impl IntoView {
    view! {
        <div class="explorer__header">
            {move || view! { <Crumbs share=Some(share_id) trail=trail.get() /> }}
        </div>
        <ErrorAlert message=page.error />
        <div class="explorer__scroll">
            <Show when=move || !items.read().is_empty()>
                <table class="node-table">
                    <thead class="node-table__head">
                        <tr>
                            <th class="node-table__header-cell">"Name"</th>
                            <th class="node-table__header-cell node-table__header-cell--size">
                                "Size"
                            </th>
                            <th class="node-table__header-cell node-table__header-cell--modified">
                                "Modified"
                            </th>
                            <th class="node-table__header-cell node-table__cell--actions">
                                <span class="visually-hidden">"Actions"</span>
                            </th>
                        </tr>
                    </thead>
                    <tbody class="node-table__body">
                        {move || {
                            let trail: Vec<NodeId> = trail
                                .read()
                                .iter()
                                .map(|folder| folder.id)
                                .collect();
                            items
                                .get()
                                .into_iter()
                                .map(|item| {
                                    view! { <FolderRow page share=share_id trail=trail.clone() item /> }
                                })
                                .collect_view()
                        }}
                    </tbody>
                </table>
            </Show>
            <Show when=move || {
                loaded.get() && items.read().is_empty() && page.error.read().is_none()
            }>
                <p class="explorer__empty">{EMPTY_FOLDER}</p>
            </Show>
        </div>
    }
}

#[component]
fn FolderRow(page: Shared, share: Uuid, trail: Vec<NodeId>, item: SharedNode) -> impl IntoView {
    let is_folder = item.kind == NodeKind::Folder;
    let modified = short_date(item.updated_at);
    let size = (!is_folder).then(|| human_size(item.size));
    let meta = size.clone().map_or_else(
        || modified.clone(),
        |size| format!("{size}{META_SEPARATOR}{modified}"),
    );
    view! {
        <tr class="node-table__row">
            <td class="node-table__cell">
                <ItemName page share trail item=item.clone() meta />
            </td>
            <td class="node-table__cell node-table__cell--size">
                {size.unwrap_or_else(|| NO_SIZE.to_owned())}
            </td>
            <td class="node-table__cell node-table__cell--modified">{modified}</td>
            <td class="node-table__cell node-table__cell--actions">
                <ItemMenu page share item />
            </td>
        </tr>
    }
}

/// The name of an item: a folder opens, a file opens in its viewer or else downloads.
#[component]
fn ItemName(
    page: Shared,
    share: Uuid,
    trail: Vec<NodeId>,
    item: SharedNode,
    meta: String,
) -> impl IntoView {
    let label = view! {
        <Icon name=kind_icon(item.kind, &item.mime) />
        <span class="node-table__text">
            {item.name.clone()}
            <span class="node-table__meta">{meta}</span>
        </span>
    };
    match item.kind {
        NodeKind::Folder => {
            let mut path = trail;
            path.push(item.id);
            view! {
                <A
                    href=route::received_folder(share, &path)
                    attr:class="node-table__name node-table__name--link"
                >
                    {label}
                </A>
            }
            .into_any()
        }
        NodeKind::File if shared_viewer(viewers(), &item).is_some() => view! {
            <A
                href=route::received_file(share, &trail, item.id)
                attr:class="node-table__name node-table__name--link"
            >
                {label}
            </A>
        }
        .into_any(),
        NodeKind::File => view! {
            <button
                class="node-table__name node-table__name--link shared__file"
                on:click=move |_| page.download(share, item.clone())
            >
                {label}
            </button>
        }
        .into_any(),
    }
}

#[component]
fn ItemMenu(page: Shared, share: Uuid, item: SharedNode) -> impl IntoView {
    let id = item.id;
    let download = {
        let item = item.clone();
        Callback::new(move |()| page.download(share, item.clone()))
    };
    let save = Callback::new(move |()| page.save(share, item.clone()));
    view! {
        <span class="node-table__actions">
            <Menu id=format!("shared-menu-{share}-{id}") label="Item actions">
                <MenuItem icon="download" label=DOWNLOAD on_select=download />
                <MenuItem icon="drive_file_move" label=SAVE_TO_MY_FILES on_select=save />
            </Menu>
        </span>
    }
}

#[component]
fn ViewOnly() -> impl IntoView {
    view! {
        <span class="pill shared__access">
            <Icon name="visibility" />
            {VIEW_ONLY}
        </span>
    }
}

/// "Shared with me" and the folders opened below it, each a way back.
#[component]
fn Crumbs(share: Option<Uuid>, trail: Vec<SharedNode>) -> impl IntoView {
    let last = trail.len();
    let ids: Vec<NodeId> = trail.iter().map(|folder| folder.id).collect();
    view! {
        <nav class="breadcrumbs" aria-label="Folder path">
            <A href=route::SHARED_PATH attr:class="breadcrumbs__link" exact=true>
                {SHARED_WITH_ME}
            </A>
            {share
                .map(|share| {
                    trail
                        .into_iter()
                        .enumerate()
                        .map(|(index, folder)| {
                            let depth = index + 1;
                            view! {
                                <span class="breadcrumbs__separator" aria-hidden="true">
                                    <Icon name="chevron_right" />
                                </span>
                                <A
                                    href=route::received_folder(share, &ids[..depth])
                                    attr:class="breadcrumbs__link"
                                    attr:aria-current=(depth == last).then_some("page")
                                >
                                    {folder.name}
                                </A>
                            }
                        })
                        .collect_view()
                })}
        </nav>
    }
}

/// Opens the folders of `path` one by one from the shared items `top`; an id that is not a
/// folder of the one before it ends the walk there. Answers the opened folders and what the
/// last one holds.
async fn open_path(
    share: Uuid,
    top: Vec<SharedNode>,
    path: &[NodeId],
) -> Result<(Vec<SharedNode>, Vec<SharedNode>), ApiError> {
    let client = session::client();
    let mut trail = Vec::new();
    let mut items = top;
    for id in path {
        let Some(folder) = child_folder(&items, *id) else {
            break;
        };
        items = client.shared_children(share, folder.id).await?;
        trail.push(folder);
    }
    sort_shared(&mut items);
    Ok((trail, items))
}
