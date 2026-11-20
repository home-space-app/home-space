//! The web client's PDF worker: opens a PDF and renders its pages with hayro, off the page's
//! thread. Trunk builds it apart from the client, and the browser only fetches it when the PDF
//! viewer starts the worker, so the client does not carry the renderer.
//!
//! It speaks [`pdf_core::messages`]: it says [`Response::Ready`] once it listens, then answers
//! each request in turn.

use std::cell::RefCell;

use js_sys::{Array, ArrayBuffer, Uint8Array};
use pdf_core::{
    Document,
    messages::{Request, Response},
};
use wasm_bindgen::{JsCast, JsValue, prelude::Closure};
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

const NO_DOCUMENT: &str = "No PDF is open.";
const NO_PAGE: &str = "The PDF has no such page.";

thread_local! {
    static DOCUMENT: RefCell<Option<Document>> = const { RefCell::new(None) };
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
            let rendered = DOCUMENT.with_borrow(|open| match open {
                None => Err(NO_DOCUMENT.to_owned()),
                Some(document) => usize::try_from(page)
                    .ok()
                    .and_then(|index| document.render(index, scale))
                    .ok_or_else(|| NO_PAGE.to_owned()),
            });
            match rendered {
                Ok(rendered) => {
                    let buffer = Uint8Array::from(rendered.rgba.as_slice()).buffer();
                    let response = Response::Rendered {
                        request_id,
                        page,
                        width: rendered.width,
                        height: rendered.height,
                    };
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
        }
        Err(message) => post(scope, &Response::Failed { message }, None),
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
