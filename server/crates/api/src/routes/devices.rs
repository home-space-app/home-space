//! The devices a user signs in from.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch},
};
use uuid::Uuid;

use db::DbError;
use protocol::{Device, RenameDevice};

use crate::{ApiError, AppState, extract::AuthUser};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/devices", get(list))
        .route("/devices/{id}", patch(rename).delete(sign_out))
}

fn db_error(err: DbError) -> ApiError {
    match err {
        DbError::NotFound => ApiError::NotFound,
        other => ApiError::Internal(other.to_string()),
    }
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<Device>>, ApiError> {
    let devices = state.db.devices(user.id).await.map_err(db_error)?;
    Ok(Json(
        devices
            .into_iter()
            .map(|device| Device {
                current: user.device_id == Some(device.id),
                id: device.id,
                name: device.name,
                platform: device.platform,
                created_at: device.created_at,
                last_seen_at: device.last_seen_at,
                signed_in: device.signed_in,
                online: state.hub.is_online(user.id, device.id),
                uploads: device.uploads,
            })
            .collect(),
    ))
}

async fn rename(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(request): Json<RenameDevice>,
) -> Result<StatusCode, ApiError> {
    let name = auth::device_name(&request.name)
        .ok_or_else(|| ApiError::BadRequest("device name must be 1-100 characters".to_owned()))?;
    state
        .db
        .rename_device(user.id, id, name)
        .await
        .map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Ends the device's sessions, closing their websockets, and cancels the uploads it left unfinished. The device stays in
/// the list, signed out, until it has been gone for as long as a session lasts unrefreshed.
async fn sign_out(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state
        .db
        .sign_out_device(user.id, id)
        .await
        .map_err(db_error)?;
    state.auth.sessions_ended(user.id);
    state.files.cancel_device_uploads(user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
