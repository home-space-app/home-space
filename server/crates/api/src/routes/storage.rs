//! How much a user stores and the room left on the server.

use axum::{Json, Router, extract::State, routing::get};

use protocol::{Storage, StorageUsage};

use crate::{ApiError, AppState, extract::AuthUser};

pub fn routes() -> Router<AppState> {
    Router::new().route("/storage", get(storage))
}

/// Converts the database's sums, which are never negative.
pub(crate) fn usage(usage: db::StorageUsage) -> StorageUsage {
    StorageUsage {
        files: usage.files.max(0).unsigned_abs(),
        versions: usage.versions.max(0).unsigned_abs(),
        trash: usage.trash.max(0).unsigned_abs(),
    }
}

async fn storage(State(state): State<AppState>, user: AuthUser) -> Result<Json<Storage>, ApiError> {
    let found = state
        .db
        .storage_usage(user.id)
        .await
        .map_err(|err| ApiError::Internal(err.to_string()))?;
    Ok(Json(Storage {
        usage: usage(found),
        disk: disk_space(&state).await,
    }))
}

/// The disk of the storage root; a failure to read it is logged and reported as unknown.
pub(crate) async fn disk_space(state: &AppState) -> Option<protocol::DiskSpace> {
    state
        .files
        .store()
        .disk_space()
        .await
        .inspect_err(|err| tracing::warn!(error = %err, "reading disk space failed"))
        .ok()
        .flatten()
}
