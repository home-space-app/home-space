//! Sharing files and folders through a read-only link.

use leptos::{prelude::*, task::spawn_local};
use time::Duration;

use client_core::share::{
    CHANGE_PASSWORD, COPY_LINK, EXPIRES, Expiry, HIDDEN_PASSWORD, LINK_COPIED,
    PASSWORD_PLACEHOLDER, PICK_A_DATE, REQUIRE_PASSWORD, STOP_SHARING, STOP_SHARING_NOTE,
    ShareDraft, format_expiry_date, matching_share, parse_expiry_date, share_subtitle, share_title,
    stop_sharing_question,
};
use protocol::{Node, NodeId, Share, UpdateShare};

use crate::{
    components::ErrorAlert,
    icon::Icon,
    load,
    locale::{now, offset_at, short_date},
    session::{self, Auth},
};

const TITLE_ID: &str = "share-title";
const NO_LINK_YET: &str = "Save to make the link";
/// How far ahead the date field starts when the owner first picks a date.
const PICKED_DATE_AHEAD: Duration = Duration::days(7);

/// Shares `nodes` through a link: makes one, or edits the link that shares exactly them.
#[component]
pub fn ShareDialog(nodes: Vec<Node>, auth: Auth, on_close: Callback<()>) -> impl IntoView {
    let node_ids: Vec<NodeId> = nodes.iter().map(|node| node.id).collect();
    let first = node_ids.first().copied();
    let share = RwSignal::new(None::<Share>);
    let draft = RwSignal::new(ShareDraft::new());
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let copied = RwSignal::new(false);
    let reveal = RwSignal::new(false);
    let confirm_stop = RwSignal::new(false);

    let wanted = StoredValue::new(node_ids);
    load::load(
        move || async move { session::client().shares(first).await },
        move |result| {
            loading.set(false);
            match result {
                Ok(shares) => {
                    if let Some(found) =
                        wanted.with_value(|ids| matching_share(&shares, ids).cloned())
                    {
                        draft.set(ShareDraft::of(&found));
                        share.set(Some(found));
                    }
                }
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        },
    );

    let link = Memo::new(move |_| {
        share
            .read()
            .as_ref()
            .map(|share| session::client().share_url(&share.token))
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
        let current = share.get_untracked();
        let request = match &current {
            Some(saved) => draft
                .read_untracked()
                .update_request(now)
                .map(|request| Err((saved.id, request))),
            None => draft
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
                    let made = current.is_none();
                    let result = match request {
                        Ok(create) => client.create_share(&create).await,
                        Err((id, update)) => client.update_share(id, &update).await,
                    };
                    saving.set(false);
                    match result {
                        Ok(saved) if made => {
                            draft.set(ShareDraft::of(&saved));
                            share.set(Some(saved));
                        }
                        Ok(_) => on_close.run(()),
                        Err(err) => error.set(Some(auth.fail(&err))),
                    }
                });
            }
        }
    };

    let stop = move |_| {
        let Some(id) = share.read_untracked().as_ref().map(|share| share.id) else {
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
                                busy=saving
                                on_cancel=Callback::new(move |()| confirm_stop.set(false))
                                on_confirm=Callback::new(stop)
                            />
                        }
                    }
                >
                    <div class="share__link">
                        <div class="share__url">
                            <Icon name="link" />
                            <input
                                class="share__url-text"
                                readonly
                                aria-label="Link"
                                placeholder=NO_LINK_YET
                                prop:value=move || link.get().unwrap_or_default()
                                on:focus:target=|event| event.target().select()
                            />
                        </div>
                        <button class="button" disabled=move || link.read().is_none() on:click=copy>
                            <Icon name="content_copy" />
                            {move || if copied.get() { LINK_COPIED } else { COPY_LINK }}
                        </button>
                    </div>
                    <PasswordSection draft reveal />
                    <ExpirySection draft />
                    <ErrorAlert message=error />
                    <div class="modal__actions share__actions">
                        <Show when=move || share.read().is_some()>
                            <button
                                class="button button--ghost button--danger share__stop"
                                disabled=move || saving.get()
                                on:click=move |event| {
                                    event.stop_propagation();
                                    confirm_stop.set(true);
                                }
                            >
                                <Icon name="link_off" />
                                {STOP_SHARING}
                            </button>
                        </Show>
                        <button class="button" on:click=move |_| on_close.run(())>
                            {move || if share.read().is_some() { "Close" } else { "Cancel" }}
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

/// Asks before the link stops working.
#[component]
fn StopSharing(
    question: String,
    busy: RwSignal<bool>,
    on_cancel: Callback<()>,
    on_confirm: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="share__confirm">
            <p class="share__label">{question}</p>
            <p class="modal__note">{STOP_SHARING_NOTE}</p>
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
                <Icon name="link_off" />
                {STOP_SHARING}
            </button>
        </div>
    }
}
