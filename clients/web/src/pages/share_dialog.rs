//! Sharing files and folders, read-only, with people on the server or through a link.

use leptos::{prelude::*, task::spawn_local};
use time::Duration;

use client_core::{
    share::{
        ADD_PEOPLE, ADD_PEOPLE_PLACEHOLDER, Audience, CAN_VIEW, CHANGE_PASSWORD, COPY_LINK,
        EXPIRES, Expiry, HIDDEN_PASSWORD, LINK_COPIED, OWNER, PASSWORD_PLACEHOLDER, PEOPLE_NOTE,
        PICK_A_DATE, REMOVE_PERSON, REQUIRE_PASSWORD, STOP_SHARING, ShareDraft, format_expiry_date,
        matching_share, parse_expiry_date, share_subtitle, share_title, stop_sharing_note,
        stop_sharing_question, you,
    },
    shared_link::initial,
};
use protocol::{Node, NodeId, Share, UpdateShare, UserRef};

use crate::{
    components::{ErrorAlert, Menu, UserPicker},
    icon::Icon,
    load,
    locale::{now, offset_at, short_date},
    session::{self, Auth},
};

const TITLE_ID: &str = "share-title";
const PEOPLE_PICKER_ID: &str = "share-people";
const NO_LINK_YET: &str = "Save to make the link";
const ADDED: &str = "Added";
const ICON_PEOPLE: &str = "group";
const ICON_LINK: &str = "link";
const ICON_STOP_LINK: &str = "link_off";
const ICON_STOP_PEOPLE: &str = "person_remove";
/// How far ahead the date field starts when the owner first picks a date.
const PICKED_DATE_AHEAD: Duration = Duration::days(7);

/// One tab of the dialog: what is drafted there, and the share it edits once there is one.
#[derive(Clone, Copy)]
struct Tab {
    draft: RwSignal<ShareDraft>,
    share: RwSignal<Option<Share>>,
}

impl Tab {
    fn new(audience: Audience, existing: Option<Share>) -> Self {
        let draft = existing
            .as_ref()
            .map_or_else(|| ShareDraft::for_audience(audience), ShareDraft::of);
        Self {
            draft: RwSignal::new(draft),
            share: RwSignal::new(existing),
        }
    }

    fn found(self, share: Share) {
        self.draft.set(ShareDraft::of(&share));
        self.share.set(Some(share));
    }
}

fn audience_icon(audience: Audience) -> &'static str {
    match audience {
        Audience::People => ICON_PEOPLE,
        Audience::Link => ICON_LINK,
    }
}

fn stop_icon(audience: Audience) -> &'static str {
    match audience {
        Audience::People => ICON_STOP_PEOPLE,
        Audience::Link => ICON_STOP_LINK,
    }
}

/// Shares `nodes` with people or through a link, each in its own tab: makes a share, or
/// edits the one that shares exactly them. Given `existing`, it edits only that share, and
/// with `stopping` it first asks to stop sharing it; declining then closes the dialog.
#[component]
pub fn ShareDialog(
    nodes: Vec<Node>,
    auth: Auth,
    on_close: Callback<()>,
    #[prop(optional)] existing: Option<Share>,
    #[prop(optional)] stopping: bool,
) -> impl IntoView {
    let node_ids: Vec<NodeId> = nodes.iter().map(|node| node.id).collect();
    let first = node_ids.first().copied();
    let known = existing.as_ref().map(Audience::of);
    let audience = RwSignal::new(known.unwrap_or_default());
    let people = Tab::new(
        Audience::People,
        existing.clone().filter(|share| !share.is_link()),
    );
    let link_tab = Tab::new(Audience::Link, existing.filter(Share::is_link));
    let tab = move || match audience.get() {
        Audience::People => people,
        Audience::Link => link_tab,
    };
    let current = move || match audience.get_untracked() {
        Audience::People => people,
        Audience::Link => link_tab,
    };
    let users = RwSignal::new(Vec::<UserRef>::new());
    let loading = RwSignal::new(known.is_none());
    let saving = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let copied = RwSignal::new(false);
    let reveal = RwSignal::new(false);
    let confirm_stop = RwSignal::new(stopping && known.is_some());

    let wanted = StoredValue::new(node_ids);
    if known.is_none() {
        load::load(
            move || async move { session::client().shares(first).await },
            move |result| {
                loading.set(false);
                match result {
                    Ok(shares) => {
                        let find = |audience| {
                            wanted.with_value(|ids| matching_share(&shares, ids, audience).cloned())
                        };
                        let (with_people, with_link) =
                            (find(Audience::People), find(Audience::Link));
                        if with_people.is_none() && with_link.is_some() {
                            audience.set(Audience::Link);
                        }
                        if let Some(found) = with_people {
                            people.found(found);
                        }
                        if let Some(found) = with_link {
                            link_tab.found(found);
                        }
                    }
                    Err(err) => error.set(Some(auth.fail(&err))),
                }
            },
        );
    }
    if known != Some(Audience::Link) {
        load::load(
            || async { session::client().users().await },
            move |result| match result {
                Ok(listed) => users.set(listed),
                Err(err) => error.set(Some(auth.fail(&err))),
            },
        );
    }

    let link = Memo::new(move |_| {
        link_tab.share.read().as_ref().and_then(|share| {
            share
                .token
                .as_deref()
                .map(|token| session::client().share_url(token))
        })
    });
    let copy = move |_| {
        let Some(url) = link.get_untracked() else {
            return;
        };
        if let Some(window) = web_sys::window() {
            let _ = window.navigator().clipboard().write_text(&url);
            copied.set(true);
        }
    };

    let save = move |_| {
        let now = now();
        let open = current();
        let saved = open.share.get_untracked();
        let request = match &saved {
            Some(saved) => open
                .draft
                .read_untracked()
                .update_request(now)
                .map(|request| Err((saved.id, request))),
            None => open
                .draft
                .read_untracked()
                .create_request(wanted.get_value(), now)
                .map(Ok),
        };
        match request {
            Err(problem) => error.set(Some(problem.message().to_owned())),
            Ok(Err((_, request))) if request == UpdateShare::default() => on_close.run(()),
            Ok(request) => {
                saving.set(true);
                error.set(None);
                spawn_local(async move {
                    let client = session::client();
                    let made = saved.is_none();
                    let result = match request {
                        Ok(create) => client.create_share(&create).await,
                        Err((id, update)) => client.update_share(id, &update).await,
                    };
                    saving.set(false);
                    match result {
                        Ok(saved) if made && saved.is_link() => open.found(saved),
                        Ok(_) => on_close.run(()),
                        Err(err) => error.set(Some(auth.fail(&err))),
                    }
                });
            }
        }
    };

    let stop = move |_| {
        let Some(id) = current()
            .share
            .read_untracked()
            .as_ref()
            .map(|share| share.id)
        else {
            return;
        };
        saving.set(true);
        spawn_local(async move {
            match session::client().delete_share(id).await {
                Ok(()) => on_close.run(()),
                Err(err) => {
                    saving.set(false);
                    confirm_stop.set(false);
                    error.set(Some(auth.fail(&err)));
                }
            }
        });
    };

    let title = share_title(&nodes);
    let subtitle = share_subtitle(&nodes);
    let question = stop_sharing_question(&nodes);
    let has_share = move || tab().share.read().is_some();
    let owner = Signal::derive(move || auth.user.get().map(|me| me.username).unwrap_or_default());

    view! {
        <div class="modal" on:click=move |_| on_close.run(())>
            <div
                class="modal__panel modal__panel--share"
                role="dialog"
                aria-modal="true"
                aria-labelledby=TITLE_ID
                on:click=|event| event.stop_propagation()
            >
                <header class="modal__header">
                    <h2 id=TITLE_ID class="modal__title">{title}</h2>
                    <p class="modal__subtitle">{subtitle}</p>
                </header>
                <Show
                    when=move || !confirm_stop.get()
                    fallback=move || {
                        view! {
                            <StopSharing
                                question=question.clone()
                                note=stop_sharing_note(audience.get_untracked())
                                icon=stop_icon(audience.get_untracked())
                                busy=saving
                                on_cancel=Callback::new(move |()| {
                                    if stopping {
                                        on_close.run(());
                                    } else {
                                        confirm_stop.set(false);
                                    }
                                })
                                on_confirm=Callback::new(stop)
                            />
                        }
                    }
                >
                    {known
                        .is_none()
                        .then(|| {
                            view! {
                                <div class="segmented share__tabs" role="tablist">
                                    {Audience::ALL
                                        .into_iter()
                                        .map(|choice| {
                                            let chosen = move || audience.get() == choice;
                                            view! {
                                                <button
                                                    type="button"
                                                    role="tab"
                                                    class="segmented__option share__tab"
                                                    class:segmented__option--active=chosen
                                                    aria-selected=move || chosen().to_string()
                                                    on:click=move |_| {
                                                        error.set(None);
                                                        audience.set(choice);
                                                    }
                                                >
                                                    <Icon name=audience_icon(choice) />
                                                    <span class="share__tab-long">{choice.label()}</span>
                                                    <span class="share__tab-short">
                                                        {choice.short_label()}
                                                    </span>
                                                </button>
                                            }
                                        })
                                        .collect_view()}
                                </div>
                            }
                        })}
                    {move || match audience.get() {
                        Audience::People => {
                            view! {
                                <PeopleSection
                                    draft=people.draft
                                    share=people.share
                                    users
                                    owner
                                />
                            }
                                .into_any()
                        }
                        Audience::Link => {
                            view! {
                                <div class="share__link">
                                    <div class="share__url">
                                        <Icon name=ICON_LINK />
                                        <input
                                            class="share__url-text"
                                            readonly
                                            aria-label="Link"
                                            placeholder=NO_LINK_YET
                                            prop:value=move || link.get().unwrap_or_default()
                                            on:focus:target=|event| event.target().select()
                                        />
                                    </div>
                                    <button
                                        class="button"
                                        disabled=move || link.read().is_none()
                                        on:click=copy
                                    >
                                        <Icon name="content_copy" />
                                        {move || if copied.get() { LINK_COPIED } else { COPY_LINK }}
                                    </button>
                                </div>
                                <PasswordSection draft=link_tab.draft reveal />
                            }
                                .into_any()
                        }
                    }}
                    {move || view! { <ExpirySection draft=tab().draft /> }}
                    <Show when=move || audience.get() == Audience::People>
                        <p class="share__info">
                            <Icon name="info" />
                            {PEOPLE_NOTE}
                        </p>
                    </Show>
                    <ErrorAlert message=error />
                    <div class="modal__actions share__actions">
                        <Show when=has_share>
                            <button
                                class="button button--ghost button--danger share__stop"
                                disabled=move || saving.get()
                                on:click=move |event| {
                                    event.stop_propagation();
                                    confirm_stop.set(true);
                                }
                            >
                                {move || view! { <Icon name=stop_icon(audience.get()) /> }}
                                {STOP_SHARING}
                            </button>
                        </Show>
                        <button class="button" on:click=move |_| on_close.run(())>
                            {move || if has_share() { "Close" } else { "Cancel" }}
                        </button>
                        <button
                            class="button button--primary"
                            disabled=move || loading.get() || saving.get()
                            on:click=save
                        >
                            <Icon name="check" />
                            "Save"
                        </button>
                    </div>
                </Show>
            </div>
        </div>
    }
}

/// Who the share is for: a drop-down that picks people, the owner, and everyone added.
#[component]
fn PeopleSection(
    draft: RwSignal<ShareDraft>,
    share: RwSignal<Option<Share>>,
    users: RwSignal<Vec<UserRef>>,
    owner: Signal<String>,
) -> impl IntoView {
    let toggle = Callback::new(move |user: UserRef| {
        draft.update(|draft| {
            if draft.has_recipient(user.id) {
                draft.remove_recipient(user.id);
            } else {
                draft.add_recipient(user);
            }
        });
    });
    let picked = Signal::derive(move || {
        draft
            .read()
            .recipients()
            .iter()
            .map(|user| user.id)
            .collect()
    });
    let added_on = move |user: &UserRef| {
        share.read().as_ref().and_then(|share| {
            share
                .recipients
                .iter()
                .find(|recipient| recipient.user.id == user.id)
                .map(|recipient| format!("{ADDED} {}", short_date(recipient.added_at)))
        })
    };

    view! {
        <div class="share__add">
            <Menu
                id=PEOPLE_PICKER_ID.to_owned()
                label=ADD_PEOPLE
                icon="person_add"
                text=Signal::stored(ADD_PEOPLE_PLACEHOLDER.to_owned())
                align_start=true
                combo=true
            >
                <UserPicker users picked toggle />
            </Menu>
            <select class="field__input share__permission" disabled aria-label="Permission">
                <option>{CAN_VIEW}</option>
            </select>
        </div>
        <ul class="share__people">
            <li class="share__person">
                <span class="avatar" aria-hidden="true">{move || initial(&owner.get())}</span>
                <span class="share__person-name">{move || you(&owner.get())}</span>
                <span class="share__person-role">{OWNER}</span>
            </li>
            <For
                each=move || draft.read().recipients().to_vec()
                key=|user| user.id
                children=move |user| {
                    let id = user.id;
                    let name = user.username.clone();
                    let added = {
                        let user = user.clone();
                        move || added_on(&user)
                    };
                    view! {
                        <li class="share__person">
                            <span class="avatar" aria-hidden="true">{initial(&name)}</span>
                            <span class="share__person-name">
                                {name.clone()}
                                <span class="share__person-added">{added}</span>
                            </span>
                            <span class="share__person-role">{CAN_VIEW}</span>
                            <button
                                class="button button--ghost button--small"
                                aria-label=format!("{REMOVE_PERSON} {name}")
                                on:click=move |_| draft.update(|draft| draft.remove_recipient(id))
                            >
                                <Icon name="close" />
                            </button>
                        </li>
                    }
                }
            />
        </ul>
    }
}

/// The password toggle, and the field for a new password or the saved one.
#[component]
fn PasswordSection(draft: RwSignal<ShareDraft>, reveal: RwSignal<bool>) -> impl IntoView {
    let required = Signal::derive(move || draft.read().require_password);
    view! {
        <div class="share__section">
            <div class="share__row">
                <Icon name="lock" />
                <span class="share__label">{REQUIRE_PASSWORD}</span>
                <button
                    type="button"
                    class="switch"
                    role="switch"
                    aria-label=REQUIRE_PASSWORD
                    aria-checked=move || required.get().to_string()
                    on:click=move |_| {
                        draft.update(|draft| draft.set_require_password(!draft.require_password))
                    }
                >
                    <span class="switch__thumb"></span>
                </button>
            </div>
            <Show when=move || required.get()>
                <div class="share__password">
                    <div class="share__secret">
                        {move || {
                            if draft.read().editing_password() {
                                view! {
                                    <input
                                        class="share__secret-text"
                                        type=move || if reveal.get() { "text" } else { "password" }
                                        autocomplete="new-password"
                                        aria-label=PASSWORD_PLACEHOLDER
                                        placeholder=PASSWORD_PLACEHOLDER
                                        prop:value=move || draft.read().password.clone()
                                        on:input:target=move |event| {
                                            let value = event.target().value();
                                            draft.update(|draft| draft.password = value);
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
                                }
                                    .into_any()
                            } else {
                                view! {
                                    <span class="share__secret-text share__secret-text--saved">
                                        {HIDDEN_PASSWORD}
                                    </span>
                                }
                                    .into_any()
                            }
                        }}
                    </div>
                    <Show when=move || draft.read().offers_change_password()>
                        <button
                            type="button"
                            class="button"
                            on:click=move |_| draft.update(ShareDraft::change_password)
                        >
                            <Icon name="key" />
                            {CHANGE_PASSWORD}
                        </button>
                    </Show>
                </div>
            </Show>
        </div>
    }
}

/// The expiry chips, the date they come to, and the field of a picked date.
#[component]
fn ExpirySection(draft: RwSignal<ShareDraft>) -> impl IntoView {
    let expiry = Signal::derive(move || draft.read().expiry);
    let resolved = move || {
        draft
            .read()
            .expires_at(now())
            .map(|at| format!("· {}", short_date(at)))
    };
    let pick = move |_| {
        if !matches!(expiry.get_untracked(), Expiry::On(_)) {
            let at = draft
                .read_untracked()
                .expires_at(now())
                .unwrap_or_else(|| now() + PICKED_DATE_AHEAD);
            draft.update(|draft| draft.expiry = Expiry::On(at));
        }
    };
    let date_value = move || match expiry.get() {
        Expiry::On(at) => format_expiry_date(at, offset_at(at)).replace(' ', "T"),
        _ => String::new(),
    };
    let set_date = move |text: String| {
        let guess = parse_expiry_date(&text, offset_at(now()));
        if let Some(at) = guess.and_then(|guess| parse_expiry_date(&text, offset_at(guess))) {
            draft.update(|draft| draft.expiry = Expiry::On(at));
        }
    };

    view! {
        <div class="share__section">
            <div class="share__row">
                <Icon name="event" />
                <span class="share__label">{EXPIRES}</span>
                <span class="share__note">{resolved}</span>
            </div>
            <div class="filter-chips" role="radiogroup" aria-label=EXPIRES>
                {Expiry::PRESETS
                    .into_iter()
                    .map(|preset| {
                        let chosen = move || expiry.get().same_chip(preset);
                        view! {
                            <button
                                type="button"
                                class="filter-chip"
                                class:filter-chip--active=chosen
                                role="radio"
                                aria-checked=move || chosen().to_string()
                                on:click=move |_| draft.update(|draft| draft.expiry = preset)
                            >
                                <Show when=chosen>
                                    <Icon name="check" />
                                </Show>
                                {preset.label()}
                            </button>
                        }
                    })
                    .collect_view()}
                <button
                    type="button"
                    class="filter-chip"
                    class:filter-chip--active=move || matches!(expiry.get(), Expiry::On(_))
                    role="radio"
                    aria-checked=move || matches!(expiry.get(), Expiry::On(_)).to_string()
                    on:click=pick
                >
                    {PICK_A_DATE}
                </button>
            </div>
            <Show when=move || matches!(expiry.get(), Expiry::On(_))>
                <input
                    class="field__input share__date"
                    type="datetime-local"
                    aria-label="Expiry date"
                    prop:value=date_value
                    on:change:target=move |event| set_date(event.target().value())
                />
            </Show>
        </div>
    }
}

/// Asks before the share stops working.
#[component]
fn StopSharing(
    question: String,
    note: &'static str,
    icon: &'static str,
    busy: RwSignal<bool>,
    on_cancel: Callback<()>,
    on_confirm: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="share__confirm">
            <p class="share__label">{question}</p>
            <p class="modal__note">{note}</p>
        </div>
        <div class="modal__actions">
            <button
                class="button"
                on:click=move |event| {
                    event.stop_propagation();
                    on_cancel.run(());
                }
            >
                "Cancel"
            </button>
            <button
                class="button button--primary share__confirm-stop"
                disabled=move || busy.get()
                on:click=move |_| on_confirm.run(())
            >
                <Icon name=icon />
                {STOP_SHARING}
            </button>
        </div>
    }
}
