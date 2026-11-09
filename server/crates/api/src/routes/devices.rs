//! The devices a user signs in from.

use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch, put},
};
use uuid::Uuid;

use db::DbError;
use protocol::{
    Device, DeviceVisibility, Event, RenameDevice, SetDeviceVisibility, TargetDevice,
    TransferTarget, UserRef,
};

use crate::{ApiError, AppState, extract::AuthUser};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/devices", get(list))
        .route("/devices/targets", get(targets))
        .route("/devices/{id}", patch(rename).delete(sign_out))
        .route("/devices/{id}/visibility", put(set_visibility))
}

fn db_error(err: DbError) -> ApiError {
    match err {
        DbError::NotFound => ApiError::NotFound,
        DbError::Invalid => ApiError::BadRequest("unknown user, or the device's owner".to_owned()),
        other => ApiError::Internal(other.to_string()),
    }
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<Device>>, ApiError> {
    let devices = state.db.devices(user.id).await.map_err(db_error)?;
    let mut viewers: HashMap<Uuid, Vec<UserRef>> = HashMap::new();
    for (device, viewer) in state.db.device_viewers(user.id).await.map_err(db_error)? {
        viewers.entry(device).or_default().push(viewer);
    }
    Ok(Json(
        devices
            .into_iter()
            .map(|device| Device {
                visible_to: viewers.remove(&device.id).unwrap_or_default(),
                visibility: device.visibility,
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

/// The devices the caller can send files to, grouped by user: their own first.
async fn targets(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<TransferTarget>>, ApiError> {
    let devices = state.db.transfer_targets(user.id).await.map_err(db_error)?;
    let mut targets: Vec<TransferTarget> = Vec::new();
    for device in devices {
        let entry = TargetDevice {
            current: device.owner_id == user.id && user.device_id == Some(device.id),
            online: state.hub.is_online(device.owner_id, device.id),
            id: device.id,
            name: device.name,
            platform: device.platform,
            signed_in: device.signed_in,
        };
        match targets.last_mut() {
            Some(target) if target.user.id == device.owner_id => target.devices.push(entry),
            _ => targets.push(TransferTarget {
                own: device.owner_id == user.id,
                user: UserRef {
                    id: device.owner_id,
                    username: device.owner_name,
                },
                devices: vec![entry],
            }),
        }
    }
    Ok(Json(targets))
}

/// Sets who can see a device and send files to it.
async fn set_visibility(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(request): Json<SetDeviceVisibility>,
) -> Result<StatusCode, ApiError> {
    let viewers = if request.visibility == DeviceVisibility::Users {
        request.users.as_slice()
    } else {
        &[]
    };
    let affected = state
        .db
        .set_device_visibility(user.id, id, request.visibility, viewers)
        .await
        .map_err(db_error)?;
    state.hub.publish(user.id, Event::DevicesChanged);
    for viewer in affected {
        state.hub.publish(viewer, Event::DevicesChanged);
    }
    Ok(StatusCode::NO_CONTENT)
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
