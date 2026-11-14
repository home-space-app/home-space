//! The page showing a file in its built-in viewer, picked by MIME type in
//! [`client_core::viewers`]. Each [`ViewerKind`] has its own component here.

mod image;
mod text;

use leptos::prelude::*;
use leptos_router::{components::A, hooks::use_params_map};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::Response;

use client_core::viewers::ViewerKind;
use protocol::{Event, Node, NodeId, TicketRequest};

use self::{image::ImageViewer, text::TextViewer};
use crate::{
    components::ErrorAlert,
    events,
    icon::Icon,
    load, route,
    session::{self, Auth},
    viewers::viewers,
};

const HOME_PATH: &str = "/";
const LOADING: &str = "Loading…";
const NO_VIEWER: &str = "This file cannot be shown here. Download it to open it.";
const DELETED: &str = "This file was deleted.";

#[component]
pub fn ViewerPage(auth: Auth) -> impl IntoView {
    let params = use_params_map();
    let id = Memo::new(move |_| params.read().get("id").and_then(|id| route::parse_id(&id)));
    let node = RwSignal::new(None::<Node>);
    let error = RwSignal::new(None::<String>);
    let deleted = RwSignal::new(false);

    let resource = load::load(
        move || {
            let id = id.get();
            async move {
                match id {
                    Some(id) => session::client().get(id).await.map(Some),
                    None => Ok(None),
                }
            }
        },
        move |loaded| match loaded {
            Ok(loaded) => {
                node.set(loaded);
                error.set(None);
            }
            Err(err) => error.set(Some(auth.fail(&err))),
        },
    );

    events::use_events(move |event| match event {
        Event::NodeUpdated { node: changed } if Some(changed.id) == id.get_untracked() => {
            node.set(Some(changed));
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
    let back = Memo::new(move |_| {
        node.read()
            .as_ref()
            .and_then(|node| node.parent_id)
            .map_or_else(|| HOME_PATH.to_owned(), route::folder)
    });
    let name = Memo::new(move |_| {
        node.read()
            .as_ref()
            .map(|node| node.name.clone())
            .unwrap_or_default()
    });

    let header = ViewerHeader { back, name };
    let body = move || {
        if deleted.get() {
            return view! {
                {header.view(None)}
                <p class="alert alert--info" role="status">{DELETED}</p>
            }
            .into_any();
        }
        match (viewer.get(), loaded.get()) {
            (Some(viewer), _) => match viewer.kind {
                ViewerKind::Text => view! { <TextViewer auth node viewer header /> }.into_any(),
                ViewerKind::Image => view! { <ImageViewer auth node viewer header /> }.into_any(),
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
        <main class="explorer viewer">
            <ErrorAlert message=error />
            {body}
        </main>
    }
}

/// The top of the viewer page: back to the folder, the file name and the viewer's buttons.
#[derive(Clone, Copy)]
pub(super) struct ViewerHeader {
    back: Memo<String>,
    name: Memo<String>,
}

impl ViewerHeader {
    pub(super) fn view(self, actions: Option<AnyView>) -> impl IntoView {
        let Self { back, name } = self;
        view! {
            <div class="explorer__header viewer__header">
                <A href=move || back.get() attr:class="button button--ghost" attr:aria-label="Back">
                    <Icon name="arrow_back" />
                </A>
                <h1 class="page-title viewer__title">{move || name.get()}</h1>
                <div class="explorer__toolbar">{actions}</div>
            </div>
        }
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
        .map_err(|err| format!("the download failed: {err:?}"))
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, JsValue> {
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
