mod admin;
mod auth;
mod capabilities;
mod changes;
mod devices;
mod nodes;
mod search;
mod storage;
mod trash;
mod uploads;
mod ws;

use axum::{Json, Router, routing::get};
use protocol::Health;

use crate::{ApiError, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .merge(admin::routes())
        .merge(auth::routes())
        .merge(capabilities::routes())
        .merge(changes::routes())
        .merge(devices::routes())
        .merge(nodes::routes())
        .merge(trash::routes())
        .merge(uploads::routes())
        .merge(search::routes())
        .merge(storage::routes())
        .merge(ws::routes())
}

async fn health(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<Json<Health>, ApiError> {
    state
        .db
        .ping()
        .await
        .map_err(|err| ApiError::Internal(err.to_string()))?;
    Ok(Json(Health::ok()))
}
