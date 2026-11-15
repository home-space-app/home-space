//! The settings tab listing everything the user shares, with people or through a link.

use leptos::prelude::*;

use client_core::share::{
    COPY_LINK, EDIT_SHARE, EXPIRED, NEVER_EXPIRES, NO_SHARES, OPEN_LINK, PASSWORD_PROTECTED,
    STOP_SHARING, ShareExpiry, recipient_names, share_name, share_subtitle,
};
use protocol::Share;

use super::{settings::SettingsHeader, share_dialog::ShareDialog};
use crate::{
    components::{ErrorAlert, Menu, MenuItem},
    icon::{Icon, node_icon},
    load,
    locale::{now, short_date},
    session::{self, Auth},
};

const ICON_LINK: &str = "link";
const ICON_PEOPLE: &str = "group";
const NEW_TAB: &str = "_blank";

/// A link being edited, or about to stop working.
#[derive(Clone)]
struct Editing {
    share: Share,
    stopping: bool,
}

#[component]
pub fn SharesPage(auth: Auth) -> impl IntoView {
    let shares = RwSignal::new(Vec::<Share>::new());
    let loaded = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let editing = RwSignal::new(None::<Editing>);
    let list = load::load(
        || async { session::client().shares(None).await },
        move |result| {
            loaded.set(true);
            match result {
                Ok(listing) => {
                    shares.set(listing);
                    error.set(None);
                }
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        },
    );
    let close = Callback::new(move |()| {
        editing.set(None);
        list.refetch();
    });

    view! {
        <main class="explorer explorer--tabbed">
            <div class="explorer__header explorer__header--stacked">
                <SettingsHeader />
            </div>
            <ErrorAlert message=error />
            <div class="explorer__scroll">
                <Show when=move || !shares.read().is_empty()>
                    <table class="node-table">
                        <thead class="node-table__head">
                            <tr>
                                <th class="node-table__header-cell">"Name"</th>
                                <th class="node-table__header-cell node-table__header-cell--modified">
                                    "Expires"
                                </th>
                                <th class="node-table__header-cell node-table__header-cell--modified">
                                    "Created"
                                </th>
                                <th class="node-table__header-cell node-table__cell--actions"></th>
                            </tr>
                        </thead>
                        <tbody class="node-table__body">
                            <For
                                each=move || shares.get()
                                key=|share| {
                                    (
                                        share.id,
                                        share.has_password,
                                        share.expires_at,
                                        share
                                            .recipients
                                            .iter()
                                            .map(|recipient| recipient.user.id)
                                            .collect::<Vec<_>>(),
                                        share
                                            .items
                                            .iter()
                                            .map(|item| (item.id, item.name.clone()))
                                            .collect::<Vec<_>>(),
                                    )
                                }
                                children=move |share| view! { <ShareRow share editing /> }
                            />
                        </tbody>
                    </table>
                </Show>
                <Show when=move || {
                    loaded.get() && shares.read().is_empty() && error.read().is_none()
                }>
                    <p class="explorer__empty">{NO_SHARES}</p>
                </Show>
            </div>
        </main>
        {move || {
            editing
                .get()
                .map(|Editing { share, stopping }| {
                    view! {
                        <ShareDialog
                            nodes=share.items.clone()
                            auth
                            on_close=close
                            existing=share
                            stopping
                        />
                    }
                })
        }}
    }
}

#[component]
fn ShareRow(share: Share, editing: RwSignal<Option<Editing>>) -> impl IntoView {
    let id = share.id;
    let url = share
        .token
        .as_deref()
        .map(|token| session::client().share_url(token));
    let people = (!share.is_link()).then(|| recipient_names(&share));
    let icon = match share.items.as_slice() {
        [item] => node_icon(item),
        _ if share.is_link() => ICON_LINK,
        _ => ICON_PEOPLE,
    };
    let name = share_name(&share.items);
    let subtitle = (share.items.len() > 1).then(|| share_subtitle(&share.items));
    let expiry = ShareExpiry::of(&share, now());
    let (expires, ended) = match expiry {
        ShareExpiry::Never => (NEVER_EXPIRES.to_owned(), false),
        ShareExpiry::Until(at) => (short_date(at), false),
        ShareExpiry::Ended(at) => (format!("{EXPIRED} {}", short_date(at)), true),
    };
    let created = short_date(share.created_at);
    let meta = subtitle
        .as_ref()
        .map_or_else(|| expires.clone(), |items| format!("{items} · {expires}"));
    let has_password = share.has_password;

    let link_actions = url.map(|url| {
        let copy = {
            let url = url.clone();
            Callback::new(move |()| {
                if let Some(window) = web_sys::window() {
                    let _ = window.navigator().clipboard().write_text(&url);
                }
            })
        };
        let open = Callback::new(move |()| {
            if let Some(window) = web_sys::window() {
                let _ = window.open_with_url_and_target(&url, NEW_TAB);
            }
        });
        (copy, open)
    });
    let edit = |stopping: bool| {
        let share = share.clone();
        Callback::new(move |()| {
            editing.set(Some(Editing {
                share: share.clone(),
                stopping,
            }));
        })
    };
    let (start_edit, start_stop) = (edit(false), edit(true));
    let stop_icon = if share.is_link() {
        "link_off"
    } else {
        "person_remove"
    };

    view! {
        <tr class="node-table__row">
            <td class="node-table__cell">
                <button class="node-table__name share-row__name" on:click=move |_| start_edit.run(())>
                    <Icon name=icon />
                    <span class="node-table__text">
                        <span class="share-row__title">
                            {name}
                            {has_password
                            .then(|| {
                                view! {
                                    <span class="pill share-row__pill">
                                        <Icon name="lock" />
                                        {PASSWORD_PROTECTED}
                                    </span>
                                }
                            })}
                            {people
                                .map(|names| {
                                    view! {
                                        <span class="pill share-row__pill">
                                            <Icon name=ICON_PEOPLE />
                                            {names}
                                        </span>
                                    }
                                })}
                        </span>
                        {subtitle
                            .map(|items| view! { <span class="share-row__items">{items}</span> })}
                        <span class="node-table__meta">{meta}</span>
                    </span>
                </button>
            </td>
            <td class="node-table__cell node-table__cell--modified">
                <span class:pill=ended class:pill--danger=ended>{expires}</span>
            </td>
            <td class="node-table__cell node-table__cell--modified">{created}</td>
            <td class="node-table__cell node-table__cell--actions">
                <span class="node-table__actions">
                    <Menu id=format!("share-menu-{id}") label="Share actions">
                        {link_actions
                            .map(|(copy, open)| {
                                view! {
                                    <MenuItem icon="content_copy" label=COPY_LINK on_select=copy />
                                    <MenuItem icon="open_in_new" label=OPEN_LINK on_select=open />
                                }
                            })}
                        <MenuItem icon="edit" label=EDIT_SHARE on_select=start_edit />
                        <MenuItem icon=stop_icon label=STOP_SHARING danger=true on_select=start_stop />
                    </Menu>
                </span>
            </td>
        </tr>
    }
}
