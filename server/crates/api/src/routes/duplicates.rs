//! Files sharing their content with other files.

use axum::{Json, Router, extract::State, routing::get};

use protocol::DuplicatesReport;

use crate::{ApiError, AppState, extract::AuthUser};

pub fn routes() -> Router<AppState> {
    Router::new().route("/duplicates", get(duplicates))
}

async fn duplicates(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<DuplicatesReport>, ApiError> {
    Ok(Json(state.files.duplicates.report(user.id).await?))
}
