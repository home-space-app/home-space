//! The built-in viewer, picked by MIME type in [`client_core::viewers`], shown in a popup over
//! the page the file was opened from. The page names the file in its query (`?file=…`, see
//! [`route::with_viewer`]); closing the popup drops it again, so the user stays where they were.
//! Each [`ViewerKind`] has its own component here.

mod image;
mod pdf;
mod shared;
mod source;
mod text;
mod video;

use leptos::{ev, html, prelude::*};
use leptos_router::{
    NavigateOptions,
    components::{A, Redirect},
    hooks::{use_location, use_navigate, use_params_map},
};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{KeyboardEvent, Response};

use client_core::viewers::ViewerKind;
use protocol::{Event, Node, NodeId, TicketRequest};

use crate::protected::{self, Vault, use_vault};

pub use self::shared::SharedViewer;
use self::{
    image::{ImageViewer, has_modifier, is_typing},
    pdf::PdfViewer,
    source::Source,
    text::TextViewer,
    video::VideoViewer,
};
use crate::{
    components::ErrorAlert,
    events,
    icon::Icon,
    leave_guard, load, route,
    session::{self, Auth},
    viewers::viewers,
};

const HOME_PATH: &str = "/";
const LOADING: &str = "Loading…";
pub(super) const NO_VIEWER: &str = "This file cannot be shown here. Download it to open it.";
const UNREACHABLE: &str = "could not reach the server";
const DELETED: &str = "This file was deleted.";
const LOCKED: &str = "This file is in a protected folder that is locked. Unlock the folder to see it.";
const CLOSE: &str = "Close";
const KEY_ESCAPE: &str = "Escape";
/// Set on the document while the popup is open, so the page under it does not scroll.
const OPEN_CLASS: &str = "viewer-open";

/// Links that open one of the user's files in the viewer, over the page that is open now.
#[derive(Clone, Copy)]
pub struct ViewLink {
    path: Memo<String>,
    search: Memo<String>,
}

impl ViewLink {
    pub fn href(self, id: NodeId) -> String {
        let path = self.path.get_untracked();
        self.search
            .with_untracked(|search| route::with_viewer(&path, search, Some(id)))
    }
}

pub fn use_view_link() -> ViewLink {
    let location = use_location();
    ViewLink {
        path: location.pathname,
        search: location.search,
    }
}

/// Where the viewer used to be, `/files/<id>/view`: Home, with the file open over it.
#[component]
pub fn LegacyViewer() -> impl IntoView {
    let params = use_params_map();
    let id = params
        .read_untracked()
        .get("id")
        .and_then(|id| route::parse_id(&id));
    view! { <Redirect path=route::with_viewer(HOME_PATH, "", id) /> }
}

/// The user's file named by the page's query, in a popup over the page.
#[component]
pub fn FileViewer(auth: Auth) -> impl IntoView {
    let location = use_location();
    let id = Memo::new(move |_| {
        location
            .query
            .read()
            .get(route::FILE_PARAM)
            .and_then(|id| route::parse_id(&id))
    });
    let close = Memo::new(move |_| {
        route::with_viewer(&location.pathname.read(), &location.search.read(), None)
    });
    view! {
        <Show when=move || id.read().is_some()>
            <ViewerOverlay close>
                <OwnViewer auth id close />
            </ViewerOverlay>
        </Show>
    }
}

/// The popup a viewer is shown in: the page darkened behind it, closed by the header's close
/// button or Escape, which go to `close`.
#[component]
pub fn ViewerOverlay(close: Memo<String>, children: Children) -> impl IntoView {
    let panel = NodeRef::<html::Div>::new();
    let guard = leave_guard::use_leave_guard();
    let navigate = use_navigate();
    let keys = window_event_listener(ev::keydown, move |event: KeyboardEvent| {
        if event.key() != KEY_ESCAPE
            || event.default_prevented()
            || has_modifier(&event)
            || is_typing(&event)
        {
            return;
        }
        let path = close.get_untracked();
        event.prevent_default();
        // Unsaved edits hold the popup open while the viewer asks about them.
        if guard.allows(&path) {
            navigate(&path, keep_scroll());
        }
    });
    on_cleanup(move || keys.remove());

    mark_open(true);
    on_cleanup(|| mark_open(false));
    Effect::new(move |_| {
        if let Some(panel) = panel.get() {
            let _ = panel.focus();
        }
    });

    view! {
        <div class="viewer-overlay">
            <div
                class="viewer-overlay__panel viewer"
                role="dialog"
                aria-modal="true"
                tabindex="-1"
                node_ref=panel
            >
                {children()}
            </div>
        </div>
    }
}

/// Navigation between the popup and the page under it: the page stays scrolled where the user
/// left it, rather than jumping back to the top.
pub(super) fn keep_scroll() -> NavigateOptions {
    NavigateOptions {
        scroll: false,
        ..NavigateOptions::default()
    }
}

/// Marks the document while the popup is open.
fn mark_open(open: bool) {
    let Some(root) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.document_element())
    else {
        return;
    };
    let classes = root.class_list();
    // Without the class the page under the popup only scrolls along.
    let _ = if open {
        classes.add_1(OPEN_CLASS)
    } else {
        classes.remove_1(OPEN_CLASS)
    };
}

/// `file` as the viewer shows it: in the protected folder `inside`, with its name opened and
/// its type taken from the name; `None` while that folder is locked.
fn shown_file(vault: Vault, inside: Option<NodeId>, file: Node) -> Option<Node> {
    match inside {
        None => Some(file),
        Some(folder) => vault
            .opened(folder, vec![file])
            .and_then(|mut shown| shown.pop()),
    }
}

/// The user's file `id`, kept up to date as it changes. A file of a protected folder is
/// decrypted in the page while the folder is unlocked, and closed when it locks.
#[component]
fn OwnViewer(auth: Auth, id: Memo<Option<NodeId>>, close: Memo<String>) -> impl IntoView {
    let node = RwSignal::new(None::<Node>);
    let error = RwSignal::new(None::<String>);
    let deleted = RwSignal::new(false);
    let vault = use_vault();
    // The protected folder the file is in.
    let inside = RwSignal::new(None::<NodeId>);
    let locked = Memo::new(move |_| inside.get().is_some_and(|folder| !vault.is_unlocked(folder)));

    let resource = load::load(
        move || {
            let id = id.get();
            // Loaded again when a folder locks or unlocks.
            vault.track();
            async move {
                let Some(id) = id else {
                    return Ok(None);
                };
                let client = session::client();
                let file = client.get(id).await?;
                let ancestors = if vault.any_protected() {
                    client.ancestors(id).await?
                } else {
                    Vec::new()
                };
                Ok(Some((file, ancestors)))
            }
        },
        move |loaded| match loaded {
            Ok(Some((file, ancestors))) => {
                let folder = vault
                    .enclosing(ancestors.iter().map(|ancestor| ancestor.id))
                    .map(|folder| folder.node_id);
                inside.set(folder);
                node.set(shown_file(vault, folder, file));
                error.set(None);
            }
            Ok(None) => {
                inside.set(None);
                node.set(None);
                error.set(None);
            }
            Err(err) => error.set(Some(auth.fail(&err))),
        },
    );

    events::use_events(move |event| match event {
        Event::NodeUpdated { node: changed } if Some(changed.id) == id.get_untracked() => {
            node.set(shown_file(vault, inside.get_untracked(), changed));
        }
        Event::NodeDeleted { id: gone, .. } if Some(gone) == id.get_untracked() => {
            deleted.set(true);
        }
        Event::Resync => resource.refetch(),
        _ => {}
    });

    let viewer = Memo::new(move |_| {
        node.read()
            .as_ref()
            .and_then(|node| viewers().viewer_for_node(node))
    });
    let loaded = Memo::new(move |_| node.read().is_some());
    let name = Memo::new(move |_| {
        node.read()
            .as_ref()
            .map(|node| node.name.clone())
            .unwrap_or_default()
    });

    // Each version of a file has its own hash; its size is what a decrypting source needs.
    let version = Memo::new(move |_| {
        node.read()
            .as_ref()
            .map(|node| ((node.id, node.size), node.sha256.clone()))
    });

    let header = ViewerHeader { close, name };
    let body = move || {
        if deleted.get() {
            return view! {
                {header.view(None)}
                <p class="alert alert--info" role="status">{DELETED}</p>
            }
            .into_any();
        }
        if locked.get() {
            return view! {
                {header.view(None)}
                <p class="explorer__empty">{LOCKED}</p>
            }
            .into_any();
        }
        let protected_in = inside.get();
        match (viewer.get(), loaded.get()) {
            (Some(viewer), _) => match viewer.kind {
                ViewerKind::Text => {
                    view! { <TextViewer auth node viewer header protected=protected_in /> }
                        .into_any()
                }
                ViewerKind::Image => {
                    view! { <ImageViewer auth node viewer header protected=protected_in /> }
                        .into_any()
                }
                ViewerKind::Pdf | ViewerKind::Video => {
                    // Opened again for each version of the file.
                    (move || {
                        version.get().map(|((id, size), _)| {
                            let source = match protected_in {
                                Some(folder) => Source::Protected {
                                    folder,
                                    node: id,
                                    size,
                                },
                                None => Source::Own(id),
                            };
                            match viewer.kind {
                                ViewerKind::Pdf => view! {
                                    <PdfViewer
                                        auth
                                        source
                                        name=name.get_untracked()
                                        max_bytes=viewer.max_bytes
                                        header
                                    />
                                }
                                .into_any(),
                                _ => view! { <VideoViewer auth source header /> }.into_any(),
                            }
                        })
                    })
                    .into_any()
                }
            },
            (None, true) => view! {
                {header.view(None)}
                <p class="explorer__empty">{NO_VIEWER}</p>
            }
            .into_any(),
            (None, false) => view! {
                {header.view(None)}
                <p class="explorer__empty">{LOADING}</p>
            }
            .into_any(),
        }
    };

    view! {
        <ErrorAlert message=error />
        {body}
    }
}

/// The top of the viewer: the file name, the viewer's buttons and the way out of the popup.
#[derive(Clone, Copy)]
pub(super) struct ViewerHeader {
    close: Memo<String>,
    name: Memo<String>,
}

impl ViewerHeader {
    pub(super) fn view(self, actions: Option<AnyView>) -> impl IntoView {
        let Self { close, name } = self;
        view! {
            <div class="explorer__header viewer__header">
                <h2 class="page-title viewer__title" title=move || name.get()>
                    {move || name.get()}
                </h2>
                <div class="explorer__toolbar">
                    {actions}
                    <A
                        href=move || close.get()
                        scroll=false
                        attr:class="button button--ghost"
                        attr:aria-label=CLOSE
                        attr:title="Close (Esc)"
                    >
                        <Icon name="close" />
                    </A>
                </div>
            </div>
        }
    }
}

/// The content of `file`, decrypted when it is in the protected folder `protected`.
pub(super) async fn file_content(
    auth: Auth,
    file: &Node,
    protected: Option<NodeId>,
) -> Result<Vec<u8>, String> {
    match protected {
        Some(folder) => {
            protected::download_plain(protected::vault(), folder, file.id, file.size).await
        }
        None => fetch_content(auth, file.id).await,
    }
}

/// The content of file `id`, fetched with a download ticket.
pub(super) async fn fetch_content(auth: Auth, id: NodeId) -> Result<Vec<u8>, String> {
    let client = session::client();
    let ticket = client
        .ticket(&TicketRequest::Download { node_id: id })
        .await
        .map_err(|err| auth.fail(&err))?;
    fetch_bytes(&client.download_url(id, &ticket.ticket))
        .await
        .map_err(|err| download_failed(&err))
}

/// "the download failed: …" with the server's answer, or that it could not be reached; the
/// browser's own error text is a stack trace, not a message.
pub(super) fn download_failed(err: &JsValue) -> String {
    let reason = err.as_string().unwrap_or_else(|| UNREACHABLE.to_owned());
    format!("the download failed: {reason}")
}

pub(super) async fn fetch_bytes(url: &str) -> Result<Vec<u8>, JsValue> {
    let window = web_sys::window().ok_or(JsValue::NULL)?;
    let response: Response = JsFuture::from(window.fetch_with_str(url))
        .await?
        .dyn_into()?;
    if !response.ok() {
        return Err(JsValue::from_str(&format!(
            "the server answered {}",
            response.status()
        )));
    }
    let buffer = JsFuture::from(response.array_buffer()?).await?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}
