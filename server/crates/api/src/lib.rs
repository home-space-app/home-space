//! HTTP and websocket API for home-space.

mod error;
mod extract;
mod routes;
mod state;

pub use error::ApiError;
pub use state::{AppState, ServerInfo};

use std::path::Path;

use axum::{Router, extract::Request};
use protocol::paths::API_PREFIX;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing::info_span;

const INDEX_FILE: &str = "index.html";

/// Builds the application router with all routes mounted under `/api/v1`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .nest(
            API_PREFIX,
            routes::routes().fallback(|| async { ApiError::NotFound }),
        )
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request| {
                info_span!(
                    "request",
                    method = %request.method(),
                    path = request.uri().path(),
                )
            }),
        )
        .with_state(state)
}

/// Like [`router`], but also serves the single-page web client from `web_root`.
///
/// Unknown paths outside the API fall back to `index.html` for client-side routing.
pub fn router_with_web(state: AppState, web_root: &Path) -> Router {
    let files = ServeDir::new(web_root).fallback(ServeFile::new(web_root.join(INDEX_FILE)));
    router(state).fallback_service(files)
}
