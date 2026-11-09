//! The other users of the server, for pickers.

use axum::{Json, Router, extract::State, routing::get};

use protocol::UserRef;

use crate::{ApiError, AppState, extract::AuthUser};

pub fn routes() -> Router<AppState> {
    Router::new().route("/users", get(list))
}

/// Every user but the caller, by username.
async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<UserRef>>, ApiError> {
    let users = state
        .db
        .list_users()
        .await
        .map_err(|err| ApiError::Internal(err.to_string()))?;
    Ok(Json(
        users
            .into_iter()
            .filter(|other| other.id != user.id)
            .map(|other| UserRef {
                id: other.id,
                username: other.username,
            })
            .collect(),
    ))
}
