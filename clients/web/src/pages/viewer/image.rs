//! The image viewer: the picture, fitted to the page, with the previous and next pictures of
//! its folder a button or a key away.

use leptos::{ev, prelude::*, task::spawn_local};
use leptos_router::{NavigateOptions, hooks::use_navigate};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Blob, BlobPropertyBag, HtmlElement, KeyboardEvent, Url};

use client_core::{gallery::Gallery, listing::Sort, viewers::Viewer};
use protocol::{Event, Node, NodeId};

use super::{ViewerHeader, fetch_content};
use crate::{
    events,
    icon::Icon,
    route,
    session::{self, Auth},
    viewers::viewers,
};

const LOADING: &str = "Loading…";
const KEY_SPACE: &str = " ";
const KEY_RIGHT: &str = "ArrowRight";
const KEY_LEFT: &str = "ArrowLeft";
const TAG_INPUT: &str = "INPUT";
const TAG_TEXTAREA: &str = "TEXTAREA";
const TAG_SELECT: &str = "SELECT";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Previous,
    Next,
}

#[component]
pub fn ImageViewer(
    auth: Auth,
    node: RwSignal<Option<Node>>,
    viewer: Viewer,
    header: ViewerHeader,
) -> impl IntoView {
    let url = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);
    let gallery = RwSignal::new(None::<Gallery>);
    // The folder the gallery was listed from.
    let listed = StoredValue::new(None::<Option<NodeId>>);
    // The content shown, as the file and the hash of the version it came from.
    let shown = StoredValue::new(None::<(NodeId, Option<String>)>);

    let show = move |file: Node| {
        spawn_local(async move {
            let loaded = fetch_content(auth, file.id).await.and_then(|bytes| {
                if u64::try_from(bytes.len()).is_ok_and(|len| len <= viewer.max_bytes) {
                    object_url(&bytes, &file.mime)
                } else {
                    Err(format!(
                        "{} is too large to show here. Download it to open it.",
                        file.name
                    ))
                }
            });
            let current = node.with_untracked(|shown| shown.as_ref().map(|shown| shown.id));
            if current != Some(file.id) {
                if let Ok(stale) = loaded {
                    revoke(&stale);
                }
                return;
            }
            match loaded {
                Ok(new) => {
                    if let Some(old) = url.get_untracked() {
                        revoke(&old);
                    }
                    url.set(Some(new));
                    error.set(None);
                }
                Err(err) => {
                    url.set(None);
                    error.set(Some(err));
                }
            }
        });
    };

    let list = move |file: Node| {
        listed.set_value(Some(file.parent_id));
        let lost_and_found = auth.user.get_untracked().map(|user| user.lost_and_found_id);
        spawn_local(async move {
            let client = session::client();
            let listing = match (file.parent_id, lost_and_found) {
                (None, Some(found)) => client.list_home(found).await,
                (parent, _) => client.list(parent).await,
            };
            match listing {
                Ok(listing) => {
                    gallery.set(Gallery::new(viewers(), &listing, &file, Sort::default()));
                }
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        });
    };

    Effect::new(move |_| {
        let Some(file) = node.get() else {
            return;
        };
        let version = Some((file.id, file.sha256.clone()));
        if shown.get_value() != version {
            shown.set_value(version);
            show(file.clone());
        }
        if listed.get_value() == Some(file.parent_id) {
            gallery.update(|gallery| {
                if let Some(gallery) = gallery {
                    gallery.move_to(file.id);
                }
            });
        } else {
            list(file);
        }
    });

    events::use_events(move |event| match event {
        Event::Resync => {
            if let Some(file) = node.get_untracked() {
                list(file);
            }
        }
        event => gallery.update(|gallery| {
            if let Some(gallery) = gallery {
                gallery.apply_event(viewers(), &event);
            }
        }),
    });

    on_cleanup(move || {
        if let Some(current) = url.get_untracked() {
            revoke(&current);
        }
    });

    let navigate = use_navigate();
    let step = Callback::new(move |step: Step| {
        let target = gallery.with_untracked(|gallery| {
            gallery.as_ref().and_then(|gallery| match step {
                Step::Previous => gallery.previous().map(|node| node.id),
                Step::Next => gallery.next().map(|node| node.id),
            })
        });
        if let Some(id) = target {
            navigate(
                &route::file_view(id),
                NavigateOptions {
                    replace: true,
                    ..NavigateOptions::default()
                },
            );
        }
    });

    let keys = window_event_listener(ev::keydown, move |event: KeyboardEvent| {
        if has_modifier(&event) || is_typing(&event) {
            return;
        }
        let target = match event.key().as_str() {
            KEY_SPACE | KEY_RIGHT => Some(Step::Next),
            KEY_LEFT => Some(Step::Previous),
            _ => None,
        };
        if let Some(target) = target {
            event.prevent_default();
            step.run(target);
        }
    });
    on_cleanup(move || keys.remove());

    let position = move || gallery.with(|gallery| gallery.as_ref().and_then(Gallery::position));
    let has = move |step: Step| {
        gallery.with(|gallery| {
            gallery.as_ref().is_some_and(|gallery| match step {
                Step::Previous => gallery.previous().is_some(),
                Step::Next => gallery.next().is_some(),
            })
        })
    };
    let actions = move || {
        position()
            .filter(|(_, total)| *total > 1)
            .map(|(index, total)| {
                view! {
                    <button
                        class="button button--ghost"
                        aria-label="Previous image"
                        title="Previous (←)"
                        disabled=move || !has(Step::Previous)
                        on:click=move |_| step.run(Step::Previous)
                    >
                        <span class="viewer__previous-icon">
                            <Icon name="chevron_right" />
                        </span>
                    </button>
                    <span class="viewer__counter" aria-live="polite">
                        {format!("{index} / {total}")}
                    </span>
                    <button
                        class="button button--ghost"
                        aria-label="Next image"
                        title="Next (Space or →)"
                        disabled=move || !has(Step::Next)
                        on:click=move |_| step.run(Step::Next)
                    >
                        <Icon name="chevron_right" />
                    </button>
                }
            })
    };

    let name = move || {
        node.read()
            .as_ref()
            .map(|file| file.name.clone())
            .unwrap_or_default()
    };
    let content = move || match url.get() {
        Some(src) => view! {
            <div class="image-viewer">
                <img class="image-viewer__img" src=src alt=name />
            </div>
        }
        .into_any(),
        None if error.read().is_some() => ().into_any(),
        None => view! { <p class="explorer__empty">{LOADING}</p> }.into_any(),
    };

    view! {
        {header.view(Some(actions.into_any()))}
        {move || error.get().map(|message| view! { <p class="alert alert--error" role="alert">{message}</p> })}
        <div class="viewer__body">{content}</div>
    }
}

/// A `blob:` URL holding `bytes` as content of type `mime`.
fn object_url(bytes: &[u8], mime: &str) -> Result<String, String> {
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let options = BlobPropertyBag::new();
    options.set_type(mime);
    Blob::new_with_u8_array_sequence_and_options(&parts, &options)
        .and_then(|blob| Url::create_object_url_with_blob(&blob))
        .map_err(|err| format!("the image could not be shown: {}", describe(&err)))
}

fn revoke(url: &str) {
    // A URL that is already gone needs no revoking.
    let _ = Url::revoke_object_url(url);
}

fn describe(err: &JsValue) -> String {
    err.as_string().unwrap_or_else(|| format!("{err:?}"))
}

fn has_modifier(event: &KeyboardEvent) -> bool {
    event.ctrl_key() || event.alt_key() || event.meta_key() || event.shift_key()
}

/// Whether the key went to a field the user types in.
fn is_typing(event: &KeyboardEvent) -> bool {
    event
        .target()
        .and_then(|target| target.dyn_into::<HtmlElement>().ok())
        .is_some_and(|element| {
            matches!(
                element.tag_name().as_str(),
                TAG_INPUT | TAG_TEXTAREA | TAG_SELECT
            ) || element.is_content_editable()
        })
}
