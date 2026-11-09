//! Administration: the server's status. Administrators get no access to other users' files.

use axum::{Json, Router, extract::State, routing::get};

use protocol::{DatabaseStatus, DiskStatus, ServerStatus, UserStorage};

use super::storage::{disk_space, usage};
use crate::{ApiError, AppState, extract::AdminUser};

pub fn routes() -> Router<AppState> {
    Router::new().route("/admin/status", get(status))
}

async fn status(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<ServerStatus>, ApiError> {
    let internal = |err: db::DbError| ApiError::Internal(err.to_string());
    let users = state.db.storage_by_user().await.map_err(internal)?;
    let database = state.db.database_info().await.map_err(internal)?;
    Ok(Json(ServerStatus {
        version: state.info.version.clone(),
        started_at: state.info.started_at,
        disk: DiskStatus {
            path: state.files.store().root().display().to_string(),
            space: disk_space(&state).await,
        },
        users: users
            .into_iter()
            .map(|user| UserStorage {
                id: user.id,
                username: user.username,
                is_admin: user.is_admin,
                external: user.external,
                usage: usage(user.usage),
            })
            .collect(),
        config: state.info.config.clone(),
        database: DatabaseStatus {
            version: database.version,
            migrations: database.migrations,
        },
    }))
}
