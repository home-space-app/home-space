//! The text viewer: read-only until Edit, then a plain editor with Cancel and Save.

use leptos::{prelude::*, task::spawn_local};
use leptos_router::{NavigateOptions, hooks::use_navigate};

use client_core::{text::TextDoc, viewers::Viewer};
use protocol::Node;

use super::{ViewerHeader, fetch_content};
use crate::{icon::Icon, leave_guard, session::Auth, upload};

const LOADING: &str = "Loading…";
const NOT_UTF8: &str = "This file is not UTF-8 text, so it can only be viewed.";
const DISCARD_TITLE: &str = "Discard changes?";
const CHANGED_ELSEWHERE: &str = "This file was changed elsewhere. Reload drops your edits and \
                                 shows the latest version; Save keeps your edits and the other \
                                 change as a previous version.";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Read,
    Edit,
}

#[component]
pub fn TextViewer(
    auth: Auth,
    node: RwSignal<Option<Node>>,
    viewer: Viewer,
    header: ViewerHeader,
) -> impl IntoView {
    let doc = RwSignal::new(None::<TextDoc>);
    let mode = RwSignal::new(Mode::Read);
    let draft = RwSignal::new(String::new());
    let saving = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let changed_elsewhere = RwSignal::new(false);
    // The content shown, as the hash of the version it came from.
    let shown = StoredValue::new(None::<Option<String>>);

    let guard = leave_guard::use_leave_guard();
    let navigate = use_navigate();
    Effect::new(move |_| {
        let unsaved = mode.get() == Mode::Edit
            && doc
                .read()
                .as_ref()
                .is_some_and(|current| draft.with(|text| *text != current.text));
        guard.set_unsaved(unsaved);
    });
    on_cleanup(move || guard.set_unsaved(false));
    let discard = Callback::new(move |()| {
        if let Some(path) = guard.take() {
            mode.set(Mode::Read);
            navigate(&path, NavigateOptions::default());
        }
    });
    let file_name = move || {
        node.read()
            .as_ref()
            .map(|file| file.name.clone())
            .unwrap_or_default()
    };

    let fetch = move |file: Node| {
        doc.set(None);
        spawn_local(async move {
            match fetch_content(auth, file.id).await {
                Ok(bytes)
                    if u64::try_from(bytes.len()).is_ok_and(|len| len <= viewer.max_bytes) =>
                {
                    doc.set(Some(TextDoc::decode(&bytes)));
                    changed_elsewhere.set(false);
                    error.set(None);
                }
                Ok(_) => error.set(Some(format!(
                    "{} is too large to show here. Download it to open it.",
                    file.name
                ))),
                Err(err) => error.set(Some(err)),
            }
        });
    };

    Effect::new(move |_| {
        let Some(file) = node.get() else {
            return;
        };
        let version = Some(file.sha256.clone());
        if saving.get_untracked() || shown.get_value() == version {
            return;
        }
        if mode.get_untracked() == Mode::Edit {
            changed_elsewhere.set(true);
        } else {
            shown.set_value(version);
            fetch(file);
        }
    });

    let edit = move |_| {
        if let Some(current) = doc.get_untracked() {
            draft.set(current.text);
            error.set(None);
            mode.set(Mode::Edit);
        }
    };
    let cancel = move |_| {
        mode.set(Mode::Read);
        error.set(None);
        if changed_elsewhere.get_untracked()
            && let Some(file) = node.get_untracked()
        {
            shown.set_value(Some(file.sha256.clone()));
            fetch(file);
        }
    };
    let reload = move |_| {
        mode.set(Mode::Read);
        error.set(None);
        if let Some(file) = node.get_untracked() {
            shown.set_value(Some(file.sha256.clone()));
            fetch(file);
        }
    };
    let save = move |_| {
        let (Some(current), Some(file)) = (doc.get_untracked(), node.get_untracked()) else {
            return;
        };
        let text = draft.get_untracked();
        let bytes = current.encode(&text);
        saving.set(true);
        error.set(None);
        spawn_local(async move {
            let client = crate::session::client();
            match upload::replace_content(&client, file.id, &file.name, &file.mime, &bytes).await {
                Ok(saved) => {
                    shown.set_value(Some(saved.sha256.clone()));
                    doc.set(Some(current.with_text(&text)));
                    changed_elsewhere.set(false);
                    mode.set(Mode::Read);
                    saving.set(false);
                    node.set(Some(saved));
                }
                Err(err) => {
                    saving.set(false);
                    error.set(Some(auth.fail(&err)));
                }
            }
        });
    };

    let editable =
        move || viewer.is_editable() && doc.read().as_ref().is_some_and(TextDoc::is_editable);
    let actions = move || match mode.get() {
        Mode::Read => editable().then(|| {
            view! {
                <button class="button" on:click=edit>
                    <Icon name="edit" />
                    <span class="label">"Edit"</span>
                </button>
            }
            .into_any()
        }),
        Mode::Edit => Some(
            view! {
                <button class="button button--ghost" disabled=move || saving.get() on:click=cancel>
                    "Cancel"
                </button>
                <button
                    class="button button--primary"
                    disabled=move || saving.get()
                    on:click=save
                >
                    <Icon name="check" />
                    <span class="label">
                        {move || if saving.get() { "Saving…" } else { "Save" }}
                    </span>
                </button>
            }
            .into_any(),
        ),
    };

    let content = move || match (doc.get(), mode.get()) {
        (None, _) => view! { <p class="explorer__empty">{LOADING}</p> }.into_any(),
        (Some(current), Mode::Read) => {
            view! { <pre class="text-viewer">{current.text}</pre> }.into_any()
        }
        (Some(_), Mode::Edit) => view! {
            <textarea
                class="text-viewer text-viewer--editing"
                spellcheck="false"
                aria-label="File content"
                prop:value=move || draft.get_untracked()
                prop:disabled=move || saving.get()
                on:input:target=move |event| draft.set(event.target().value())
            ></textarea>
        }
        .into_any(),
    };

    view! {
        {header.view(Some(actions.into_any()))}
        {move || error.get().map(|message| view! { <p class="alert alert--error" role="alert">{message}</p> })}
        <Show when=move || changed_elsewhere.get()>
            <div class="alert alert--info alert--action" role="status">
                <span>{CHANGED_ELSEWHERE}</span>
                <button
                    class="button"
                    disabled=move || saving.get()
                    on:click=reload
                >
                    <Icon name="refresh" />
                    <span class="label">"Reload"</span>
                </button>
            </div>
        </Show>
        <Show when=move || doc.read().as_ref().is_some_and(|current| !current.is_editable())>
            <p class="alert alert--info" role="status">{NOT_UTF8}</p>
        </Show>
        <div class="viewer__body">{content}</div>
        <Show when=move || guard.pending().read().is_some()>
            <div class="modal" on:click=move |_| guard.stay()>
                <div
                    class="modal__panel"
                    role="alertdialog"
                    aria-modal="true"
                    on:click=|event| event.stop_propagation()
                >
                    <header class="modal__header">
                        <h2 class="modal__title">{DISCARD_TITLE}</h2>
                        <p class="modal__subtitle">
                            {move || format!("Your edits to “{}” will be lost.", file_name())}
                        </p>
                    </header>
                    <div class="modal__actions">
                        <button class="button" on:click=move |_| guard.stay()>
                            "Keep editing"
                        </button>
                        <button class="button button--danger" on:click=move |_| discard.run(())>
                            "Discard"
                        </button>
                    </div>
                </div>
            </div>
        </Show>
    }
}
