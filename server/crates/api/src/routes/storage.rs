//! How much a user stores and the room left on the server.

use axum::{Json, Router, extract::State, routing::get};

use protocol::Storage;

use crate::{
    ApiError, AppState,
    extract::AuthUser,
    support::{convert::usage, disk::disk_space},
};

pub fn routes() -> Router<AppState> {
    Router::new().route("/storage", get(storage))
}

async fn storage(State(state): State<AppState>, user: AuthUser) -> Result<Json<Storage>, ApiError> {
    let found = state.db.storage_usage(user.id).await?;
    Ok(Json(Storage {
        usage: usage(found),
        disk: disk_space(&state).await,
    }))
}
