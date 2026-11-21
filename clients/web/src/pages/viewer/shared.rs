//! A file shared with the user, open in its built-in viewer over its folder: read-only, with the other
//! pictures of its folder a button or a key away. PDFs and videos open in the same viewers as
//! the user's own files.

use leptos::{ev, prelude::*, task::spawn_local};
use leptos_router::{NavigateOptions, hooks::use_navigate};
use uuid::Uuid;
use web_sys::KeyboardEvent;

use client_core::{
    received::{SharedGallery, shared_viewer},
    shared_link::DOWNLOAD,
    text::TextDoc,
    viewers::{Viewer, ViewerKind},
};
use protocol::{NodeId, SharedNode};

use super::{
    NO_VIEWER, ViewerHeader,
    image::{has_modifier, is_typing, object_url, revoke},
    pdf::PdfViewer,
    source::{Source, too_large},
    video::VideoViewer,
};
use crate::{icon::Icon, route, session::Auth, viewers::viewers};

const LOADING: &str = "Loading…";
const KEY_SPACE: &str = " ";
const KEY_RIGHT: &str = "ArrowRight";
const KEY_LEFT: &str = "ArrowLeft";

/// What the viewer shows once the content is in.
#[derive(Clone)]
enum Shown {
    Text(String),
    /// A `blob:` URL of the picture.
    Image(String),
}

#[component]
pub fn SharedViewer(
    auth: Auth,
    share: Uuid,
    /// The folders opened from the shared items down to the one holding the file.
    trail: Vec<NodeId>,
    /// What that folder holds, the file among it.
    listing: Vec<SharedNode>,
    item: SharedNode,
    /// Where closing the viewer goes.
    close: Memo<String>,
    on_download: Callback<()>,
) -> impl IntoView {
    let viewer = shared_viewer(viewers(), &item);
    let gallery = SharedGallery::new(viewers(), &listing, &item);
    let header = ViewerHeader {
        close,
        name: Memo::new({
            let name = item.name.clone();
            move |_| name.clone()
        }),
    };
    let source = Source::Shared {
        share,
        node: item.id,
    };
    match viewer {
        Some(
            viewer @ Viewer {
                kind: ViewerKind::Pdf,
                ..
            },
        ) => {
            return view! {
                <PdfViewer
                    auth
                    source
                    name=item.name.clone()
                    max_bytes=viewer.max_bytes
                    header
                    on_download
                />
            }
            .into_any();
        }
        Some(Viewer {
            kind: ViewerKind::Video,
            ..
        }) => return view! { <VideoViewer auth source header on_download /> }.into_any(),
        _ => {}
    }
    let shown = RwSignal::new(None::<Shown>);
    let error = RwSignal::new(None::<String>);

    if let Some(viewer) = viewer {
        load(auth, source, item.clone(), viewer, shown, error);
    }
    on_cleanup(move || {
        if let Some(Shown::Image(url)) = shown.get_untracked() {
            revoke(&url);
        }
    });

    let navigate = use_navigate();
    let step_to = {
        let trail = trail.clone();
        move |target: Option<&SharedNode>| {
            if let Some(target) = target {
                navigate(
                    &route::received_file(share, &trail, target.id),
                    NavigateOptions {
                        replace: true,
                        scroll: false,
                        ..NavigateOptions::default()
                    },
                );
            }
        }
    };
    let previous = gallery.as_ref().and_then(SharedGallery::previous).cloned();
    let next = gallery.as_ref().and_then(SharedGallery::next).cloned();
    let go_previous = {
        let (step_to, previous) = (step_to.clone(), previous.clone());
        Callback::new(move |()| step_to(previous.as_ref()))
    };
    let go_next = {
        let next = next.clone();
        Callback::new(move |()| step_to(next.as_ref()))
    };
    let keys = window_event_listener(ev::keydown, move |event: KeyboardEvent| {
        if has_modifier(&event) || is_typing(&event) {
            return;
        }
        let step = match event.key().as_str() {
            KEY_SPACE | KEY_RIGHT => Some(go_next),
            KEY_LEFT => Some(go_previous),
            _ => None,
        };
        if let Some(step) = step {
            event.prevent_default();
            step.run(());
        }
    });
    on_cleanup(move || keys.remove());

    let steps = gallery
        .as_ref()
        .and_then(SharedGallery::position)
        .filter(|(_, total)| *total > 1)
        .map(|(index, total)| {
            view! {
                <button
                    class="button button--ghost"
                    aria-label="Previous image"
                    title="Previous (←)"
                    disabled=previous.is_none()
                    on:click=move |_| go_previous.run(())
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
                    disabled=next.is_none()
                    on:click=move |_| go_next.run(())
                >
                    <Icon name="chevron_right" />
                </button>
            }
        });
    let actions = view! {
        {steps}
        <button class="button" on:click=move |_| on_download.run(())>
            <Icon name="download" />
            <span class="label">{DOWNLOAD}</span>
        </button>
    };

    let name = item.name.clone();
    let content = move || match shown.get() {
        Some(Shown::Text(text)) => view! { <pre class="text-viewer">{text}</pre> }.into_any(),
        Some(Shown::Image(src)) => view! {
            <div class="image-viewer">
                <img class="image-viewer__img" src=src alt=name.clone() />
            </div>
        }
        .into_any(),
        None if viewer.is_none() => view! { <p class="explorer__empty">{NO_VIEWER}</p> }.into_any(),
        None if error.read().is_some() => ().into_any(),
        None => view! { <p class="explorer__empty">{LOADING}</p> }.into_any(),
    };

    view! {
        {header.view(Some(actions.into_any()))}
        {move || error.get().map(|message| view! { <p class="alert alert--error" role="alert">{message}</p> })}
        <div class="viewer__body">{content}</div>
    }
    .into_any()
}

/// Fetches `item` through a ticket for the share and hands it to the text or image viewer.
fn load(
    auth: Auth,
    source: Source,
    item: SharedNode,
    viewer: Viewer,
    shown: RwSignal<Option<Shown>>,
    error: RwSignal<Option<String>>,
) {
    spawn_local(async move {
        let result = source
            .bytes(auth)
            .await
            .and_then(|bytes| {
                if u64::try_from(bytes.len()).is_ok_and(|len| len <= viewer.max_bytes) {
                    Ok(bytes)
                } else {
                    Err(too_large(&item.name))
                }
            })
            .and_then(|bytes| match viewer.kind {
                ViewerKind::Text => Ok(Shown::Text(TextDoc::decode(&bytes).text)),
                ViewerKind::Image => object_url(&bytes, &item.mime).map(Shown::Image),
                // Shown by their own viewers, above.
                ViewerKind::Pdf | ViewerKind::Video => Err(NO_VIEWER.to_owned()),
            });
        match result {
            Ok(content) => {
                if let Some(Some(Shown::Image(stale))) = shown.try_set(Some(content)) {
                    revoke(&stale);
                }
            }
            Err(err) => error.set(Some(err)),
        }
    });
}
