//! The web client's PDF worker: opens a PDF and renders its pages with hayro, off the page's
//! thread. Trunk builds it apart from the client, and the browser only fetches it when the PDF
//! viewer starts the worker, so the client does not carry the renderer.
//!
//! It speaks [`pdf_core::messages`]: it says [`Response::Ready`] once it listens, then answers
//! each request in turn. Renders wait in a queue and run one per task, so the messages that
//! arrive meanwhile are read first: a newer render of a page replaces the waiting one, and a
//! page that left the view is cancelled, instead of rendering pages nobody will see.

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
};

use js_sys::{Array, ArrayBuffer, Uint8Array};
use pdf_core::{
    Document,
    messages::{Request, Response},
};
use wasm_bindgen::{JsCast, JsValue, prelude::Closure};
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

const NO_DOCUMENT: &str = "No PDF is open.";
const NO_PAGE: &str = "The PDF has no such page.";

/// A render waiting for its turn.
#[derive(Clone, Copy)]
struct Job {
    request_id: u32,
    scale: f32,
}

thread_local! {
    static DOCUMENT: RefCell<Option<Document>> = const { RefCell::new(None) };
    /// The render waiting for each page.
    static QUEUE: RefCell<BTreeMap<u32, Job>> = const { RefCell::new(BTreeMap::new()) };
    /// Whether the next turn is already scheduled.
    static SCHEDULED: Cell<bool> = const { Cell::new(false) };
}

fn main() {
    console_error_panic_hook::set_once();
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let listener = Closure::<dyn Fn(MessageEvent)>::new({
        let scope = scope.clone();
        move |event: MessageEvent| handle(&scope, &event)
    });
    scope.set_onmessage(Some(listener.as_ref().unchecked_ref()));
    // The worker lives as long as its listener.
    listener.forget();
    post(&scope, &Response::Ready, None);
}

fn handle(scope: &DedicatedWorkerGlobalScope, event: &MessageEvent) {
    let parts = Array::from(&event.data());
    let request = parts
        .get(0)
        .as_string()
        .ok_or_else(|| "the message is not a request".to_owned())
        .and_then(|json| Request::from_json(&json));
    match request {
        Ok(Request::Open) => {
            let bytes = parts
                .get(1)
                .dyn_into::<ArrayBuffer>()
                .map(|buffer| Uint8Array::new(&buffer).to_vec())
                .unwrap_or_default();
            QUEUE.with_borrow_mut(BTreeMap::clear);
            let response = match Document::open(bytes) {
                Ok(document) => {
                    let pages = document.page_sizes();
                    DOCUMENT.with_borrow_mut(|open| *open = Some(document));
                    Response::Opened { pages }
                }
                Err(message) => Response::Failed { message },
            };
            post(scope, &response, None);
        }
        Ok(Request::Render {
            request_id,
            page,
            scale,
        }) => {
            QUEUE.with_borrow_mut(|queue| queue.insert(page, Job { request_id, scale }));
            schedule(scope);
        }
        Ok(Request::Cancel { page }) => {
            QUEUE.with_borrow_mut(|queue| queue.remove(&page));
        }
        Err(message) => post(scope, &Response::Failed { message }, None),
    }
}

/// Runs the next render in a task of its own, after the messages already waiting.
fn schedule(scope: &DedicatedWorkerGlobalScope) {
    if SCHEDULED.replace(true) {
        return;
    }
    let turn = Closure::once_into_js({
        let scope = scope.clone();
        move || {
            SCHEDULED.set(false);
            render_next(&scope);
        }
    });
    if scope
        .set_timeout_with_callback_and_timeout_and_arguments_0(turn.unchecked_ref(), 0)
        .is_err()
    {
        // Without timers, render now rather than never.
        SCHEDULED.set(false);
        render_next(scope);
    }
}

/// Renders the job asked for first, then schedules the rest.
fn render_next(scope: &DedicatedWorkerGlobalScope) {
    let next = QUEUE.with_borrow_mut(|queue| {
        let page = queue
            .iter()
            .min_by_key(|(_, job)| job.request_id)
            .map(|(page, _)| *page)?;
        queue.remove(&page).map(|job| (page, job))
    });
    let Some((page, Job { request_id, scale })) = next else {
        return;
    };
    let rendered = DOCUMENT.with_borrow(|open| match open {
        None => Err(NO_DOCUMENT.to_owned()),
        Some(document) => usize::try_from(page)
            .ok()
            .and_then(|index| document.render(index, scale))
            .ok_or_else(|| NO_PAGE.to_owned()),
    });
    match rendered {
        Ok(rendered) => {
            // The one copy, out of the worker's memory; the buffer is then transferred.
            let buffer = Uint8Array::from(rendered.rgba.as_slice()).buffer();
            let response = Response::Rendered {
                request_id,
                page,
                width: rendered.width,
                height: rendered.height,
            };
            drop(rendered);
            post(scope, &response, Some(buffer));
        }
        Err(message) => post(
            scope,
            &Response::RenderFailed {
                request_id,
                message,
            },
            None,
        ),
    }
    if QUEUE.with_borrow(|queue| !queue.is_empty()) {
        schedule(scope);
    }
}

/// Posts `response` with `buffer`, which is handed over rather than copied.
fn post(scope: &DedicatedWorkerGlobalScope, response: &Response, buffer: Option<ArrayBuffer>) {
    let message = Array::of2(
        &JsValue::from_str(&response.to_json()),
        buffer
            .as_ref()
            .map_or(&JsValue::UNDEFINED, |buffer| buffer.as_ref()),
    );
    let result = match buffer {
        Some(buffer) => scope.post_message_with_transfer(&message, &Array::of1(&buffer)),
        None => scope.post_message(&message),
    };
    // The page that started the worker is gone when posting fails; nobody is left to tell.
    let _ = result;
}
