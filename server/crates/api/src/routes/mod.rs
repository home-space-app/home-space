mod admin;
mod archive;
mod auth;
mod capabilities;
mod changes;
mod content;
mod devices;
mod nodes;
mod search;
mod shares;
mod storage;
mod transfers;
mod trash;
mod uploads;
mod users;
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
        .merge(transfers::routes())
        .merge(uploads::routes())
        .merge(users::routes())
        .merge(search::routes())
        .merge(shares::routes())
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
