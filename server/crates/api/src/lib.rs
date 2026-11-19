//! HTTP and websocket API for home-space.

mod browser;
mod error;
mod extract;
mod headers;
mod routes;
mod state;
mod support;
mod tenants;

pub use error::ApiError;
use headers::WebPolicy;
pub use state::{AppState, ServerInfo};
pub use tenants::{Resolved, TenantContext, TenantMode, Tenants, normalize_host};

use std::path::Path;

use axum::{
    Router,
    extract::Request,
    middleware::{Next, from_fn},
};
use protocol::paths::API_PREFIX;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing::info_span;

const INDEX_FILE: &str = "index.html";

/// Tells the sender and the recipient of a transfer that it expired.
pub fn publish_transfer_expired(hub: &notifications::Hub, transfer: &db::ExpiredTransfer) {
    support::publish::publish_changed(
        hub,
        transfer.id,
        transfer.sender_id,
        transfer.recipient_id,
        protocol::TransferState::Expired,
    );
}

/// Builds the application router with all routes mounted under `/api/v1`.
pub fn router(state: AppState) -> Router {
    finish(api(state), WebPolicy::default())
}

fn api(state: AppState) -> Router {
    Router::new()
        .nest(
            API_PREFIX,
            routes::routes()
                .fallback(|| async { ApiError::NotFound })
                .method_not_allowed_fallback(|| async { ApiError::MethodNotAllowed }),
        )
        .with_state(state)
}

/// Wraps every route of `router` in what all responses get: the security headers and a trace
/// span. It comes last, because a layer only covers the routes that exist when it is added.
fn finish(router: Router, web: WebPolicy) -> Router {
    router
        .layer(from_fn(error::with_instance))
        .layer(from_fn(move |request: Request, next: Next| {
            headers::secure(web.clone(), request, next)
        }))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request| {
                info_span!(
                    "request",
                    method = %request.method(),
                    path = request.uri().path(),
                )
            }),
        )
}

/// Like [`router`], but also serves the single-page web client from `web_root`.
///
/// Unknown paths outside the API fall back to `index.html` for client-side routing.
pub fn router_with_web(state: AppState, web_root: &Path) -> Router {
    let index = web_root.join(INDEX_FILE);
    let web = WebPolicy::for_index(&index);
    let files = ServeDir::new(web_root).fallback(ServeFile::new(index));
    finish(api(state).fallback_service(files), web)
}
