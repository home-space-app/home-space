//! What this server supports, so clients need no fixed limits of their own.

use axum::{Json, Router, extract::State, routing::get};

use protocol::Capabilities;

use crate::{AppState, extract::AuthUser};

pub fn routes() -> Router<AppState> {
    Router::new().route("/capabilities", get(capabilities))
}

async fn capabilities(State(state): State<AppState>, _user: AuthUser) -> Json<Capabilities> {
    Json(Capabilities {
        upload_chunk_bytes: state.files.uploads.chunk_bytes(),
    })
}
