//! The PDF viewer: the pages one under another, each fitted whole in the view, and zoomable.
//!
//! The pages are rendered by PDF workers (`clients/pdf-worker`), which Trunk builds apart from
//! this client: the browser fetches the worker when the first PDF is opened, so the client does
//! not carry the renderer. A few workers start while the PDF downloads, each with its own copy
//! of the document, and share the pages between them so neighbouring pages render at once.
//! A page is rendered when it scrolls near the view, at the size it is shown, and again when
//! the zoom changes. A rendered page keeps its pixels when it scrolls away, until the pages
//! drawn hold more than [`DRAWN_BUDGET`]: then the ones farthest from the view are cleared.

use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    rc::Rc,
};

use js_sys::{Array, ArrayBuffer, Uint8Array, Uint8ClampedArray};
use leptos::{html, prelude::*, task::spawn_local};
use wasm_bindgen::{JsCast, JsValue, prelude::Closure};
use web_sys::{
    CanvasRenderingContext2d, Element, HtmlCanvasElement, ImageData, IntersectionObserver,
    IntersectionObserverEntry, IntersectionObserverInit, MessageEvent, Worker,
};

use client_core::shared_link::DOWNLOAD;
use pdf_core::{
    PageSize,
    messages::{Request, Response},
};

use super::{
    ViewerHeader,
    source::{Source, too_large},
};
use crate::{icon::Icon, session::Auth};

/// The worker's loader, which Trunk writes next to `index.html` under this fixed name. From
/// the root: the viewer's own URL is a few folders down.
const WORKER_URL: &str = "/pdf-worker_loader.js";
const LOADING: &str = "Loading…";
const NOT_STARTED: &str = "The PDF viewer could not be started";
const PAGE_ATTRIBUTE: &str = "data-page";
/// How far beyond the view a page starts rendering, so it is ready when it scrolls in.
const PRELOAD_MARGIN: &str = "150% 0px";
/// The most pixels per CSS pixel a page is rendered at: sharp on a high-density screen, while
/// a phone's 3× screen renders 2.25 times fewer pixels than its full density would ask for.
const MAX_DENSITY: f64 = 2.0;
/// The most workers rendering at once; each holds a copy of the document.
const MAX_WORKERS: usize = 3;
/// The most bytes of pixels the drawn pages keep, about a dozen pages on a large screen.
const DRAWN_BUDGET: u64 = 192 * 1024 * 1024;
/// Zoom levels, as a share of the size that fits the whole page in the view.
const ZOOMS: &[f32] = &[0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0];
const FIT: f32 = 1.0;

/// The observer of the pages, with the callback it calls.
type Observer = (IntersectionObserver, Closure<dyn FnMut(Array)>);

/// The listeners of a worker: its messages and its failure to start.
type Handlers = (
    Closure<dyn FnMut(MessageEvent)>,
    Closure<dyn FnMut(web_sys::Event)>,
);

/// The workers and the observer of the pages, which live as long as the viewer.
struct Session {
    workers: Vec<Worker>,
    feeds: Vec<Rc<RefCell<Feed>>>,
    _handlers: Vec<Handlers>,
    observer: Option<Observer>,
}

impl Session {
    /// Hands the PDF in `bytes` to every worker, now or once it listens.
    fn open(&self, bytes: Vec<u8>) -> Result<(), JsValue> {
        let bytes = Rc::new(bytes);
        for (worker, feed) in self.workers.iter().zip(&self.feeds) {
            let mut feed = feed.borrow_mut();
            feed.bytes = Some(Rc::clone(&bytes));
            feed.send(worker)?;
        }
        Ok(())
    }

    /// The worker that renders `page`: neighbouring pages go to different workers.
    fn worker_for(&self, page: u32) -> &Worker {
        let index = usize::try_from(page).unwrap_or_default() % self.workers.len();
        &self.workers[index]
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        for worker in &self.workers {
            worker.terminate();
        }
        if let Some((observer, _)) = &self.observer {
            observer.disconnect();
        }
    }
}

/// The PDF on its way to one worker, which takes it once it listens and the bytes are in.
#[derive(Default)]
struct Feed {
    ready: bool,
    bytes: Option<Rc<Vec<u8>>>,
}

impl Feed {
    fn send(&mut self, worker: &Worker) -> Result<(), JsValue> {
        if !self.ready {
            return Ok(());
        }
        let Some(bytes) = self.bytes.take() else {
            return Ok(());
        };
        let buffer = Uint8Array::from(bytes.as_slice()).buffer();
        post(worker, &Request::Open, Some(buffer))
    }
}

/// A page drawn on its canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drawn {
    zoom: f32,
    /// The bytes its pixels take.
    bytes: u64,
}

/// Which pages are near the view, which renders they wait for and which are drawn.
#[derive(Default)]
struct Pages {
    next_request: u32,
    /// The latest render asked for each page; answers to earlier ones are dropped.
    latest: HashMap<u32, u32>,
    /// The zoom each page waiting for a render was asked for at.
    asked_at: HashMap<u32, f32>,
    drawn: HashMap<u32, Drawn>,
    /// The pages in or near the view.
    visible: BTreeSet<u32>,
}

impl Pages {
    /// Whether `page` needs a render at `zoom`: it is neither drawn nor asked for at it.
    fn wants(&self, page: u32, zoom: f32) -> bool {
        self.drawn.get(&page).is_none_or(|drawn| drawn.zoom != zoom)
            && self.asked_at.get(&page) != Some(&zoom)
    }

    /// Forgets the drawn pages farthest from the view until the rest fit [`DRAWN_BUDGET`],
    /// and gives them back for their canvases to be cleared. The pages near the view stay.
    fn over_budget(&mut self) -> Vec<u32> {
        let mut total: u64 = self.drawn.values().map(|drawn| drawn.bytes).sum();
        if total <= DRAWN_BUDGET {
            return Vec::new();
        }
        let near = self
            .visible
            .first()
            .copied()
            .zip(self.visible.last().copied());
        let distance = |page: u32| match near {
            Some((first, _)) if page < first => first - page,
            Some((_, last)) => page.saturating_sub(last),
            None => 0,
        };
        let mut far: Vec<u32> = self
            .drawn
            .keys()
            .copied()
            .filter(|page| !self.visible.contains(page))
            .collect();
        far.sort_by_key(|page| std::cmp::Reverse(distance(*page)));
        let mut cleared = Vec::new();
        for page in far {
            if total <= DRAWN_BUDGET {
                break;
            }
            if let Some(drawn) = self.drawn.remove(&page) {
                total -= drawn.bytes;
                cleared.push(page);
            }
        }
        cleared
    }
}

#[component]
pub fn PdfViewer(
    auth: Auth,
    source: Source,
    /// The file name, for messages.
    name: String,
    max_bytes: u64,
    header: ViewerHeader,
    /// Offers Download in the header, for a file shared with the user.
    #[prop(optional)]
    on_download: Option<Callback<()>>,
) -> impl IntoView {
    let sizes = RwSignal::new(None::<Vec<PageSize>>);
    let error = RwSignal::new(None::<String>);
    let zoom = RwSignal::new(FIT);
    let current = RwSignal::new(1_usize);
    let container = NodeRef::<html::Div>::new();
    let session = StoredValue::new_local(None::<Session>);
    let pages = StoredValue::new(Pages::default());

    let request = move |page: u32| {
        let (Some(element), Some(size)) = (
            container.get_untracked(),
            sizes.with_untracked(|sizes| {
                sizes
                    .as_ref()
                    .and_then(|sizes| sizes.get(usize::try_from(page).ok()?).copied())
            }),
        ) else {
            return;
        };
        let Some(canvas) = page_canvas(&element, page) else {
            return;
        };
        let ratio = web_sys::window()
            .map_or(1.0, |window| window.device_pixel_ratio())
            .min(MAX_DENSITY);
        #[allow(clippy::cast_possible_truncation)]
        let width = (canvas.get_bounding_client_rect().width() * ratio) as f32;
        let scale = size.scale_for_width(width);
        let Some(request_id) = pages.try_update_value(|pages| {
            pages.next_request = pages.next_request.wrapping_add(1);
            pages.latest.insert(page, pages.next_request);
            pages.asked_at.insert(page, zoom.get_untracked());
            pages.next_request
        }) else {
            return;
        };
        session.with_value(|session| {
            if let Some(session) = session {
                let render = Request::Render {
                    request_id,
                    page,
                    scale,
                };
                if let Err(err) = post(session.worker_for(page), &render, None) {
                    error.set(Some(describe(&err)));
                }
            }
        });
    };

    let on_response = move |response: Response, buffer: JsValue| match response {
        Response::Ready => {}
        // Every worker opens the document; the first to answer lays the pages out.
        Response::Opened { pages } => {
            if sizes.with_untracked(Option::is_none) {
                sizes.set(Some(pages));
            }
        }
        Response::Failed { message } => error.set(Some(message)),
        Response::Rendered {
            request_id,
            page,
            width,
            height,
        } => {
            let asked_at = pages.with_value(|pages| {
                (pages.latest.get(&page) == Some(&request_id))
                    .then(|| pages.asked_at.get(&page).copied())
                    .flatten()
            });
            let (Some(zoom), Some(element), Ok(buffer)) = (
                asked_at,
                container.get_untracked(),
                buffer.dyn_into::<ArrayBuffer>(),
            ) else {
                return;
            };
            if let Err(err) = draw(&element, page, width, height, &buffer) {
                error.set(Some(describe(&err)));
                return;
            }
            let cleared = pages.try_update_value(|pages| {
                pages.latest.remove(&page);
                pages.asked_at.remove(&page);
                let bytes = u64::from(width) * u64::from(height) * 4;
                pages.drawn.insert(page, Drawn { zoom, bytes });
                pages.over_budget()
            });
            for page in cleared.unwrap_or_default() {
                if let Some(canvas) = page_canvas(&element, page) {
                    canvas.set_width(0);
                    canvas.set_height(0);
                }
            }
        }
        Response::RenderFailed { message, .. } => error.set(Some(message)),
    };

    // The workers start, and load, while the PDF downloads.
    match start(
        Rc::new(on_response),
        Rc::new(move |message| {
            error.try_set(Some(message));
        }),
    ) {
        Ok(started) => session.set_value(Some(started)),
        Err(err) => error.set(Some(format!("{NOT_STARTED}: {}", describe(&err)))),
    }
    spawn_local(async move {
        let outcome = match source.bytes(auth).await {
            Ok(bytes) if u64::try_from(bytes.len()).is_ok_and(|len| len <= max_bytes) => session
                .try_with_value(|session| session.as_ref().map(|session| session.open(bytes))),
            Ok(_) => {
                error.try_set(Some(too_large(&name)));
                None
            }
            Err(err) => {
                error.try_set(Some(err));
                None
            }
        };
        match outcome {
            Some(Some(Ok(()))) => {}
            Some(Some(Err(err))) => {
                error.set(Some(describe(&err)));
            }
            // Nothing to show: the workers stop. The viewer may be gone already.
            _ => {
                let _ = session.try_update_value(|session| *session = None);
            }
        }
    });

    // Once the pages are laid out, render each one as it comes near the view.
    Effect::new(move |_| {
        let (Some(element), true) = (container.get(), sizes.with(Option::is_some)) else {
            return;
        };
        let observed = observe(&element, move |page, visible| {
            if !visible {
                // A page that left keeps what is drawn, but is not rendered if it is still
                // waiting in its worker.
                pages.update_value(|pages| {
                    pages.visible.remove(&page);
                    pages.asked_at.remove(&page);
                    pages.latest.remove(&page);
                });
                session.with_value(|session| {
                    if let Some(session) = session {
                        let _ = post(session.worker_for(page), &Request::Cancel { page }, None);
                    }
                });
                return;
            }
            pages.update_value(|pages| {
                pages.visible.insert(page);
            });
            let stale = pages.with_value(|pages| pages.wants(page, zoom.get_untracked()));
            if stale {
                request(page);
            }
        });
        match observed {
            Ok(observed) => session.update_value(|session| {
                if let Some(session) = session
                    && let Some((old, _)) = session.observer.replace(observed)
                {
                    old.disconnect();
                }
            }),
            Err(err) => error.set(Some(describe(&err))),
        }
    });

    // A new zoom renders the pages in view again; the others follow as they scroll in. Until
    // then each shows what was drawn at the old zoom, stretched.
    Effect::new(move |previous: Option<f32>| {
        let level = zoom.get();
        if previous.is_some_and(|previous| previous != level) {
            // After the next layout, so the pages are measured at their new size.
            request_animation_frame(move || {
                for page in pages.with_value(|pages| pages.visible.clone()) {
                    request(page);
                }
            });
        }
        level
    });

    let step = move |by: isize| {
        let at = ZOOMS
            .iter()
            .position(|level| *level >= zoom.get_untracked())
            .unwrap_or(ZOOMS.len() - 1);
        let next = at.saturating_add_signed(by).min(ZOOMS.len() - 1);
        zoom.set(ZOOMS[next]);
    };
    let total = move || sizes.with(|sizes| sizes.as_ref().map_or(0, Vec::len));
    let actions = move || {
        let counter = (total() > 0).then(|| {
            view! {
                <span class="viewer__counter" aria-live="polite">
                    {move || format!("{} / {}", current.get(), total())}
                </span>
            }
        });
        view! {
            {counter}
            <button
                class="button button--ghost"
                aria-label="Zoom out"
                title="Zoom out"
                disabled=move || { zoom.get() <= ZOOMS[0] }
                on:click=move |_| step(-1)
            >
                <Icon name="zoom_out" />
            </button>
            <button
                class="button button--ghost"
                title="Fit the page"
                disabled=move || { (zoom.get() - FIT).abs() < f32::EPSILON }
                on:click=move |_| zoom.set(FIT)
            >
                {move || format!("{:.0}%", zoom.get() * 100.0)}
            </button>
            <button
                class="button button--ghost"
                aria-label="Zoom in"
                title="Zoom in"
                disabled=move || { zoom.get() >= ZOOMS[ZOOMS.len() - 1] }
                on:click=move |_| step(1)
            >
                <Icon name="zoom_in" />
            </button>
            {on_download.map(|download| view! {
                <button class="button" on:click=move |_| download.run(())>
                    <Icon name="download" />
                    <span class="label">{DOWNLOAD}</span>
                </button>
            })}
        }
    };

    let page_views = move || {
        sizes.get().map(|sizes| {
            sizes
                .into_iter()
                .enumerate()
                .map(|(index, size)| {
                    // At 100% the whole page fits the view: as wide as the view, or as wide as
                    // its height allows. `cq` units are the view's, which is a size container.
                    let style = move || {
                        format!(
                            "width: calc({} * min(100cqw, 100cqh * {} / {})); aspect-ratio: {} / {};",
                            zoom.get(),
                            size.width,
                            size.height,
                            size.width,
                            size.height
                        )
                    };
                    view! {
                        <canvas
                            class="pdf-viewer__page"
                            data-page=index
                            style=style
                            aria-label=format!("Page {}", index + 1)
                        ></canvas>
                    }
                })
                .collect_view()
        })
    };
    // The page whose bottom is below the top of the view is the one being read.
    let on_scroll = move |_| {
        if let Some(element) = container.get_untracked() {
            current.set(page_at_top(&element));
        }
    };
    let loading = move || {
        (sizes.with(Option::is_none) && error.read().is_none())
            .then(|| view! { <p class="explorer__empty">{LOADING}</p> })
    };

    on_cleanup(move || session.update_value(|session| *session = None));

    view! {
        {header.view(Some(actions.into_any()))}
        {move || error.get().map(|message| view! { <p class="alert alert--error" role="alert">{message}</p> })}
        <div class="viewer__body">
            {loading}
            <div
                class="pdf-viewer"
                node_ref=container
                hidden=move || sizes.with(Option::is_none)
                on:scroll=on_scroll
            >
                {page_views}
            </div>
        </div>
    }
}

/// Starts the workers, which wait for the PDF until [`Session::open`] hands it over.
fn start(
    on_response: Rc<dyn Fn(Response, JsValue)>,
    on_failure: Rc<dyn Fn(String)>,
) -> Result<Session, JsValue> {
    let mut session = Session {
        workers: Vec::new(),
        feeds: Vec::new(),
        _handlers: Vec::new(),
        observer: None,
    };
    for _ in 0..worker_count() {
        let worker = Worker::new(WORKER_URL)?;
        let feed = Rc::new(RefCell::new(Feed::default()));
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new({
            let (worker, feed, on_response) =
                (worker.clone(), Rc::clone(&feed), Rc::clone(&on_response));
            move |event: MessageEvent| {
                let parts = Array::from(&event.data());
                let response = parts
                    .get(0)
                    .as_string()
                    .ok_or_else(|| "the PDF worker sent no answer".to_owned())
                    .and_then(|json| Response::from_json(&json));
                let response = match response {
                    Ok(Response::Ready) => {
                        let mut feed = feed.borrow_mut();
                        feed.ready = true;
                        match feed.send(&worker) {
                            Ok(()) => Response::Ready,
                            Err(err) => Response::Failed {
                                message: describe(&err),
                            },
                        }
                    }
                    Ok(response) => response,
                    Err(message) => Response::Failed { message },
                };
                on_response(response, parts.get(1));
            }
        });
        worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        // A worker whose script does not load, or that fails while starting, says so here.
        let on_error = Closure::<dyn FnMut(web_sys::Event)>::new({
            let on_failure = Rc::clone(&on_failure);
            move |event: web_sys::Event| {
                let reason = event
                    .dyn_ref::<web_sys::ErrorEvent>()
                    .map(web_sys::ErrorEvent::message)
                    .filter(|message| !message.is_empty());
                let message = match reason {
                    Some(reason) => format!("{NOT_STARTED}: {reason}"),
                    None => format!("{NOT_STARTED}."),
                };
                on_failure(message);
            }
        });
        worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        session.workers.push(worker);
        session.feeds.push(feed);
        session._handlers.push((on_message, on_error));
    }
    Ok(session)
}

/// How many workers render: half the cores, so the page stays responsive, within
/// [`MAX_WORKERS`].
fn worker_count() -> usize {
    let cores = web_sys::window().map_or(1.0, |window| window.navigator().hardware_concurrency());
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let half = (cores / 2.0) as usize;
    half.clamp(1, MAX_WORKERS)
}

fn post(worker: &Worker, request: &Request, buffer: Option<ArrayBuffer>) -> Result<(), JsValue> {
    let message = Array::of2(
        &JsValue::from_str(&request.to_json()),
        buffer
            .as_ref()
            .map_or(&JsValue::UNDEFINED, |buffer| buffer.as_ref()),
    );
    match buffer {
        Some(buffer) => worker.post_message_with_transfer(&message, &Array::of1(&buffer)),
        None => worker.post_message(&message),
    }
}

/// Watches every page of `container`, telling `on_change` when one comes near the view or
/// leaves it.
fn observe(
    container: &Element,
    on_change: impl Fn(u32, bool) + 'static,
) -> Result<Observer, JsValue> {
    let callback = Closure::<dyn FnMut(Array)>::new(move |entries: Array| {
        for entry in entries.iter() {
            let entry: IntersectionObserverEntry = entry.unchecked_into();
            if let Some(page) = entry
                .target()
                .get_attribute(PAGE_ATTRIBUTE)
                .and_then(|page| page.parse().ok())
            {
                on_change(page, entry.is_intersecting());
            }
        }
    });
    let options = IntersectionObserverInit::new();
    options.set_root(Some(container));
    options.set_root_margin(PRELOAD_MARGIN);
    let observer =
        IntersectionObserver::new_with_options(callback.as_ref().unchecked_ref(), &options)?;
    let canvases = container.query_selector_all(&format!("canvas[{PAGE_ATTRIBUTE}]"))?;
    for index in 0..canvases.length() {
        if let Some(canvas) = canvases.item(index).and_then(|node| node.dyn_into().ok()) {
            observer.observe(&canvas);
        }
    }
    Ok((observer, callback))
}

/// The page, from 1, at the top of `container`'s view.
fn page_at_top(container: &Element) -> usize {
    let top = container.get_bounding_client_rect().top();
    let Ok(canvases) = container.query_selector_all(&format!("canvas[{PAGE_ATTRIBUTE}]")) else {
        return 1;
    };
    (0..canvases.length())
        .filter_map(|index| canvases.item(index)?.dyn_into::<Element>().ok())
        .position(|canvas| canvas.get_bounding_client_rect().bottom() > top + 1.0)
        .map_or(1, |index| index + 1)
}

fn page_canvas(container: &Element, page: u32) -> Option<HtmlCanvasElement> {
    container
        .query_selector(&format!("canvas[{PAGE_ATTRIBUTE}=\"{page}\"]"))
        .ok()
        .flatten()
        .and_then(|canvas| canvas.dyn_into().ok())
}

/// Paints the RGBA pixels in `buffer` onto the canvas of `page`. The pixels stay in `buffer`,
/// outside this client's memory, which never shrinks once grown.
fn draw(
    container: &Element,
    page: u32,
    width: u32,
    height: u32,
    buffer: &ArrayBuffer,
) -> Result<(), JsValue> {
    let Some(canvas) = page_canvas(container, page) else {
        return Ok(());
    };
    let image = ImageData::new_with_js_u8_clamped_array_and_sh(
        &Uint8ClampedArray::new(buffer),
        width,
        height,
    )?;
    canvas.set_width(width);
    canvas.set_height(height);
    let context: CanvasRenderingContext2d = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("the page cannot be drawn"))?
        .unchecked_into();
    context.put_image_data(&image, 0.0, 0.0)
}

fn describe(err: &JsValue) -> String {
    err.as_string()
        .or_else(|| {
            err.dyn_ref::<js_sys::Error>()
                .map(|err| String::from(err.message()))
        })
        .unwrap_or_else(|| format!("{err:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE_BYTES: u64 = 16 * 1024 * 1024;

    fn drawn(pages: &mut Pages, range: std::ops::Range<u32>) {
        for page in range {
            pages.drawn.insert(
                page,
                Drawn {
                    zoom: FIT,
                    bytes: PAGE_BYTES,
                },
            );
        }
    }

    #[test]
    fn a_drawn_page_is_not_rendered_again_at_its_zoom() {
        let mut pages = Pages::default();
        drawn(&mut pages, 0..1);
        assert!(!pages.wants(0, FIT));
        assert!(pages.wants(0, 2.0));
        assert!(pages.wants(1, FIT));
    }

    #[test]
    fn a_page_asked_for_is_not_asked_for_again() {
        let mut pages = Pages::default();
        pages.asked_at.insert(3, FIT);
        assert!(!pages.wants(3, FIT));
        assert!(pages.wants(3, 0.5));
    }

    #[test]
    fn drawn_pages_stay_within_the_budget() {
        let mut pages = Pages::default();
        drawn(&mut pages, 0..8);
        assert!(pages.over_budget().is_empty());
    }

    #[test]
    fn the_pages_farthest_from_the_view_are_cleared_first() {
        let mut pages = Pages::default();
        drawn(&mut pages, 0..20);
        pages.visible.extend([15, 16]);
        let cleared = pages.over_budget();
        let kept: u64 = pages.drawn.values().map(|drawn| drawn.bytes).sum();
        assert!(kept <= DRAWN_BUDGET);
        assert_eq!(cleared, [0, 1, 2, 3, 4, 5, 6, 7]);
        assert!(pages.drawn.contains_key(&15) && pages.drawn.contains_key(&19));
    }
}
