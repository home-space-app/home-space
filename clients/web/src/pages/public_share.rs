//! The page a share link opens, `/s/<token>`: what was shared, for anyone with the link.

use leptos::{ev::SubmitEvent, prelude::*, task::spawn_local};
use leptos_router::{
    components::A,
    hooks::{use_params_map, use_query_map},
};

use api_client::ApiError;
use client_core::{
    format::human_size,
    shared_link::{
        DOWNLOAD, DOWNLOAD_ALL, EMPTY_FOLDER, LINK_GONE, LINK_GONE_NOTE, OPEN, PASSWORD,
        PASSWORD_PROTECTED, SHARED_WITH_YOU, WRONG_PASSWORD, child_folder, expires_on, initial,
        kept_depth, password_prompt, shared_by, shared_summary, sort_shared,
    },
};
use protocol::{ErrorCode, NodeId, NodeKind, PublicShare, SharedNode};

use crate::{
    icon::{Icon, kind_icon},
    locale::{medium_date, short_date},
    route,
    session::client,
};

const TOKEN_PARAM: &str = "token";
/// Where an unlocked link's access token is kept for the tab, so a reload asks no password.
const ACCESS_KEY_PREFIX: &str = "homespace.share_access.";
const NO_SIZE: &str = "—";
const META_SEPARATOR: &str = " · ";

#[derive(Clone, Debug, PartialEq)]
enum Stage {
    Loading,
    Password,
    Items,
    Gone,
    Failed(String),
}

impl Stage {
    fn of_error(err: &ApiError) -> Self {
        match err {
            ApiError::Api {
                code: ErrorCode::NotFound | ErrorCode::Gone,
                ..
            } => Self::Gone,
            ApiError::Unauthorized => Self::Password,
            _ => Self::Failed(err.to_string()),
        }
    }
}

/// One visit of a share link: where the visitor is and what they see.
#[derive(Clone, Copy)]
struct Visit {
    token: Memo<String>,
    /// The folders the URL opens, from the shared items down.
    path: Memo<Vec<NodeId>>,
    stage: RwSignal<Stage>,
    share: RwSignal<Option<PublicShare>>,
    access: RwSignal<Option<String>>,
    /// The folders opened from the shared items down to the one shown.
    trail: RwSignal<Vec<SharedNode>>,
    items: RwSignal<Vec<SharedNode>>,
    /// Counts the loads, so an answer overtaken by a later one is dropped.
    loads: StoredValue<u64>,
    /// The link whose folders `trail` holds.
    shown_token: StoredValue<String>,
}

impl Visit {
    fn new(token: Memo<String>, path: Memo<Vec<NodeId>>) -> Self {
        Self {
            token,
            path,
            stage: RwSignal::new(Stage::Loading),
            share: RwSignal::new(None),
            access: RwSignal::new(None),
            trail: RwSignal::new(Vec::new()),
            items: RwSignal::new(Vec::new()),
            loads: StoredValue::new(0),
            shown_token: StoredValue::new(String::new()),
        }
    }

    /// Shows the folder the URL opens, looking up only the folders not already open.
    fn show(self) {
        let token = self.token.get_untracked();
        let path = self.path.get_untracked();
        let access = self.access.get_untracked().or_else(|| saved_access(&token));
        self.access.set(access.clone());
        let mut trail = self.trail.get_untracked();
        let same_link = self.shown_token.with_value(|shown| *shown == token);
        let kept = if same_link && self.stage.get_untracked() == Stage::Items {
            kept_depth(&trail, &path)
        } else {
            0
        };
        trail.truncate(kept);
        self.loads.update_value(|loads| *loads += 1);
        let load = self.loads.get_value();
        spawn_local(async move {
            let opened = open_path(&token, access.as_deref(), trail, &path[kept..]).await;
            if self.loads.get_value() != load {
                return;
            }
            match opened {
                Ok(Opened::Locked(share)) => {
                    forget_access(&token);
                    self.access.set(None);
                    self.share.set(Some(share));
                    self.stage.set(Stage::Password);
                }
                Ok(Opened::Folder {
                    share,
                    trail,
                    items,
                }) => {
                    if let Some(share) = share {
                        self.share.set(Some(share));
                    }
                    self.shown_token.set_value(token);
                    self.trail.set(trail);
                    self.items.set(items);
                    self.stage.set(Stage::Items);
                }
                Err(err) => self.fail(&err),
            }
        });
    }

    /// The page of the folder `depth` steps below the shared items, which are depth 0.
    fn href_at(self, depth: usize) -> String {
        let ids = self.trail.with(|trail| {
            trail
                .iter()
                .take(depth)
                .map(|folder| folder.id)
                .collect::<Vec<_>>()
        });
        route::shared_folder(&self.token.get_untracked(), &ids)
    }

    /// The page of `folder`, opened from the folder shown.
    fn href_into(self, folder: NodeId) -> String {
        let ids = self.trail.with(|trail| {
            trail
                .iter()
                .map(|open| open.id)
                .chain(std::iter::once(folder))
                .collect::<Vec<_>>()
        });
        route::shared_folder(&self.token.get_untracked(), &ids)
    }

    fn fail(self, err: &ApiError) {
        if matches!(err, ApiError::Unauthorized) {
            forget_access(&self.token.get_untracked());
            self.access.set(None);
        }
        self.stage.set(Stage::of_error(err));
    }

    fn download_url(self, item: &SharedNode) -> String {
        client().shared_file_url(
            &self.token.get_untracked(),
            item.id,
            self.access.get_untracked().as_deref(),
        )
    }

    /// The zip of the folder shown, or of everything shared at the top.
    fn archive_url(self) -> String {
        client().shared_archive_url(
            &self.token.get_untracked(),
            self.trail
                .with(|trail| trail.last().map(|folder| folder.id)),
            self.access.get_untracked().as_deref(),
        )
    }
}

#[component]
pub fn PublicSharePage() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let token = Memo::new(move |_| params.read().get(TOKEN_PARAM).unwrap_or_default());
    let path = Memo::new(move |_| route::share_trail(&query.read()));
    let visit = Visit::new(token, path);
    Effect::new(move |_| {
        token.track();
        path.track();
        visit.show();
    });

    view! {
        <div class="public">
            <header class="public__bar">
                <span class="public__brand">
                    <span class="sidebar__logo" aria-hidden="true">
                        <Icon name="home" />
                    </span>
                    "Home Space"
                </span>
                <Show when=move || visit.stage.get() == Stage::Items>
                    <Owner share=visit.share />
                </Show>
            </header>
            {move || match visit.stage.get() {
                Stage::Loading => view! { <p class="status">"Loading…"</p> }.into_any(),
                Stage::Password => view! { <PasswordCard visit /> }.into_any(),
                Stage::Items => view! { <Listing visit /> }.into_any(),
                Stage::Gone => view! { <GoneCard /> }.into_any(),
                Stage::Failed(message) => {
                    view! {
                        <main class="public__main public__main--centered">
                            <p class="alert alert--error" role="alert">{message}</p>
                        </main>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

/// Who shared the items and until when, at the end of the top bar.
#[component]
fn Owner(share: RwSignal<Option<PublicShare>>) -> impl IntoView {
    move || {
        share.get().map(|share| {
            let expires = share
                .expires_at
                .map(|at| format!("{META_SEPARATOR}{}", expires_on(&medium_date(at))));
            view! {
                <span class="public__owner">
                    <span class="public__avatar" aria-hidden="true">{initial(&share.owner)}</span>
                    {shared_by(&share.owner)}
                    <span class="public__expiry">{expires}</span>
                </span>
            }
        })
    }
}

#[component]
fn PasswordCard(visit: Visit) -> impl IntoView {
    let password = RwSignal::new(String::new());
    let reveal = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let share = visit.share.get_untracked();
    let prompt = share
        .as_ref()
        .map(|share| password_prompt(&share.owner, share.item_count));
    let expires = share
        .and_then(|share| share.expires_at)
        .map(|at| expires_on(&medium_date(at)));

    let submit = move |event: SubmitEvent| {
        event.prevent_default();
        let token = visit.token.get_untracked();
        let entered = password.get_untracked();
        busy.set(true);
        spawn_local(async move {
            let result = client().unlock_share(&token, &entered).await;
            busy.set(false);
            match result {
                Ok(access) => {
                    remember_access(&token, &access.access);
                    visit.access.set(Some(access.access));
                    visit.show();
                }
                Err(ApiError::Unauthorized) => error.set(Some(WRONG_PASSWORD.to_owned())),
                Err(
                    err @ ApiError::Api {
                        code: ErrorCode::NotFound | ErrorCode::Gone,
                        ..
                    },
                ) => visit.fail(&err),
                Err(err) => error.set(Some(err.to_string())),
            }
        });
    };

    view! {
        <main class="public__main public__main--centered">
            <section class="public__card" aria-labelledby="public-title">
                <span class="public__badge" aria-hidden="true">
                    <Icon name="lock" />
                </span>
                <h1 id="public-title" class="public__title">{PASSWORD_PROTECTED}</h1>
                <p class="public__note">{prompt}</p>
                <form class="public__form" on:submit=submit>
                    <label class="field">
                        <span class="field__label">{PASSWORD}</span>
                        <span class="share__secret" class:public__secret--invalid=move || error.read().is_some()>
                            <input
                                class="share__secret-text"
                                type=move || if reveal.get() { "text" } else { "password" }
                                autocomplete="current-password"
                                required
                                autofocus
                                aria-invalid=move || error.read().is_some().to_string()
                                aria-describedby="public-error"
                                prop:value=move || password.get()
                                on:input:target=move |event| {
                                    password.set(event.target().value());
                                    error.set(None);
                                }
                            />
                            <button
                                type="button"
                                class="button button--ghost button--small"
                                aria-label=move || if reveal.get() { "Hide password" } else { "Show password" }
                                on:click=move |_| reveal.update(|on| *on = !*on)
                            >
                                <Icon name="visibility" />
                            </button>
                        </span>
                    </label>
                    <p id="public-error" class="public__error" role="alert">
                        {move || {
                            error
                                .get()
                                .map(|message| view! { <Icon name="error" /> {message} })
                        }}
                    </p>
                    <button
                        type="submit"
                        class="button button--primary button--block"
                        disabled=move || busy.get() || password.read().is_empty()
                    >
                        <Icon name="lock" />
                        {OPEN}
                    </button>
                </form>
                <p class="public__expires">{expires}</p>
            </section>
        </main>
    }
}

#[component]
fn GoneCard() -> impl IntoView {
    view! {
        <main class="public__main public__main--centered">
            <section class="public__card" aria-labelledby="public-title">
                <span class="public__badge public__badge--muted" aria-hidden="true">
                    <Icon name="link_off" />
                </span>
                <h1 id="public-title" class="public__title">{LINK_GONE}</h1>
                <p class="public__note">{LINK_GONE_NOTE}</p>
            </section>
        </main>
    }
}

#[component]
fn Listing(visit: Visit) -> impl IntoView {
    let summary = move || {
        visit
            .share
            .read()
            .as_ref()
            .filter(|_| visit.trail.read().is_empty())
            .map(|share| shared_summary(&share.owner, share.item_count))
    };
    view! {
        <main class="public__main">
            <section class="public__listing">
                <div class="public__heading">
                    <div class="public__where">
                        <Crumbs visit />
                        <p class="page-header__meta">{summary}</p>
                    </div>
                    <Show when=move || !visit.items.read().is_empty()>
                        <a
                            href=move || visit.archive_url()
                            download
                            class="button button--primary public__download-all"
                        >
                            <Icon name="download" />
                            {DOWNLOAD_ALL}
                        </a>
                    </Show>
                </div>
                <div class="explorer__nodes">
                    {move || {
                        if visit.items.read().is_empty() {
                            view! { <p class="explorer__empty">{EMPTY_FOLDER}</p> }.into_any()
                        } else {
                            view! { <ItemTable visit /> }.into_any()
                        }
                    }}
                </div>
            </section>
        </main>
    }
}

/// "Shared with you" and the folders opened below it, each a way back.
#[component]
fn Crumbs(visit: Visit) -> impl IntoView {
    let at_top = move || visit.trail.read().is_empty();
    view! {
        <nav class="breadcrumbs" aria-label="Folder path">
            <A
                href=move || visit.href_at(0)
                attr:class="breadcrumbs__link"
                attr:aria-current=move || at_top().then_some("page")
            >
                {SHARED_WITH_YOU}
            </A>
            {move || {
                let trail = visit.trail.get();
                let last = trail.len();
                trail
                    .into_iter()
                    .enumerate()
                    .map(|(index, folder)| {
                        let depth = index + 1;
                        view! {
                            <span class="breadcrumbs__separator" aria-hidden="true">
                                "/"
                            </span>
                            <A
                                href=visit.href_at(depth)
                                attr:class="breadcrumbs__link"
                                attr:aria-current=(depth == last).then_some("page")
                            >
                                {folder.name}
                            </A>
                        }
                    })
                    .collect_view()
            }}
        </nav>
    }
}

#[component]
fn ItemTable(visit: Visit) -> impl IntoView {
    view! {
        <table class="node-table">
            <thead class="node-table__head">
                <tr>
                    <th class="node-table__header-cell">"Name"</th>
                    <th class="node-table__header-cell node-table__header-cell--size">"Size"</th>
                    <th class="node-table__header-cell node-table__header-cell--modified">
                        "Modified"
                    </th>
                    <th class="node-table__header-cell">
                        <span class="visually-hidden">"Actions"</span>
                    </th>
                </tr>
            </thead>
            <tbody class="node-table__body">
                {move || {
                    visit
                        .items
                        .get()
                        .into_iter()
                        .map(|item| view! { <ItemRow visit item /> })
                        .collect_view()
                }}
            </tbody>
        </table>
    }
}

#[component]
fn ItemRow(visit: Visit, item: SharedNode) -> impl IntoView {
    let is_folder = item.kind == NodeKind::Folder;
    let icon = kind_icon(item.kind, &item.mime);
    let modified = short_date(item.updated_at);
    let size = (!is_folder).then(|| human_size(item.size));
    let meta = size.clone().map_or_else(
        || modified.clone(),
        |size| format!("{size}{META_SEPARATOR}{modified}"),
    );
    let label = view! {
        <Icon name=icon />
        <span class="node-table__text">
            {item.name.clone()}
            <span class="node-table__meta">{meta}</span>
        </span>
    };
    let (name, action) = if is_folder {
        let name = view! {
            <A href=visit.href_into(item.id) attr:class="node-table__name node-table__name--link">
                {label}
            </A>
        }
        .into_any();
        (name, None)
    } else {
        let url = visit.download_url(&item);
        let name = view! {
            <a href=url.clone() download=item.name.clone() class="node-table__name node-table__name--link">
                {label}
            </a>
        }
        .into_any();
        let action = view! {
            <a
                href=url
                download=item.name.clone()
                class="button button--ghost button--small"
                aria-label=format!("{DOWNLOAD} {}", item.name)
                title=DOWNLOAD
            >
                <Icon name="download" />
            </a>
        };
        (name, Some(action))
    };
    view! {
        <tr class="node-table__row">
            <td class="node-table__cell">{name}</td>
            <td class="node-table__cell node-table__cell--size">
                {size.unwrap_or_else(|| NO_SIZE.to_owned())}
            </td>
            <td class="node-table__cell node-table__cell--modified">{modified}</td>
            <td class="node-table__cell node-table__cell--actions">{action}</td>
        </tr>
    }
}

fn access_key(token: &str) -> String {
    format!("{ACCESS_KEY_PREFIX}{token}")
}

fn tab_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok().flatten()
}

fn saved_access(token: &str) -> Option<String> {
    tab_storage()?.get_item(&access_key(token)).ok().flatten()
}

fn remember_access(token: &str, access: &str) {
    if let Some(storage) = tab_storage() {
        let _ = storage.set_item(&access_key(token), access);
    }
}

fn forget_access(token: &str) {
    if let Some(storage) = tab_storage() {
        let _ = storage.remove_item(&access_key(token));
    }
}

/// What a share link's URL opens to.
enum Opened {
    /// The link needs its password first.
    Locked(PublicShare),
    Folder {
        /// The link itself, when it was asked for again.
        share: Option<PublicShare>,
        trail: Vec<SharedNode>,
        items: Vec<SharedNode>,
    },
}

/// Opens the folders of `path` one by one below `trail`, which is already open; an id that
/// is not a folder of the one before it ends the walk there.
async fn open_path(
    token: &str,
    access: Option<&str>,
    mut trail: Vec<SharedNode>,
    path: &[NodeId],
) -> Result<Opened, ApiError> {
    let api = client();
    let (share, mut items) = match trail.last() {
        Some(folder) => (None, api.shared_folder(token, folder.id, access).await?),
        None => {
            let share = api.public_share(token, access).await?;
            if share.needs_password {
                return Ok(Opened::Locked(share));
            }
            let items = share.items.clone().unwrap_or_default();
            (Some(share), items)
        }
    };
    for id in path {
        let Some(folder) = child_folder(&items, *id) else {
            break;
        };
        items = api.shared_folder(token, folder.id, access).await?;
        trail.push(folder);
    }
    sort_shared(&mut items);
    Ok(Opened::Folder {
        share,
        trail,
        items,
    })
}
