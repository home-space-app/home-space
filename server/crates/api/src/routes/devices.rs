//! The devices a user signs in from.

use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, patch, put},
};
use uuid::Uuid;

use protocol::{Device, RenameDevice, SetDeviceVisibility, TransferTarget};

use crate::extract::{ApiJson, ApiPath};
use crate::{
    ApiError, AppState,
    extract::{AuthUser, Tenant},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/devices", get(list))
        .route("/devices/targets", get(targets))
        .route("/devices/{id}", patch(rename).delete(sign_out))
        .route("/devices/{id}/visibility", put(set_visibility))
}

async fn list(tenant: Tenant, user: AuthUser) -> Result<Json<Vec<Device>>, ApiError> {
    Ok(Json(tenant.devices.list(user.id, user.device_id).await?))
}

/// The devices the caller can send files to, grouped by user: their own first.
async fn targets(tenant: Tenant, user: AuthUser) -> Result<Json<Vec<TransferTarget>>, ApiError> {
    Ok(Json(tenant.devices.targets(user.id, user.device_id).await?))
}

/// Sets who can see a device and send files to it.
async fn set_visibility(
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<SetDeviceVisibility>,
) -> Result<StatusCode, ApiError> {
    tenant.devices.set_visibility(user.id, id, &request).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn rename(
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<RenameDevice>,
) -> Result<StatusCode, ApiError> {
    tenant.devices.rename(user.id, id, &request).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Ends the device's sessions, closing their websockets, and cancels the uploads it left
/// unfinished.
async fn sign_out(
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, ApiError> {
    tenant.devices.sign_out(user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
