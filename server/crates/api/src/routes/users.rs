//! The other users of the server, for pickers.

use axum::{Json, Router, routing::get};

use protocol::UserRef;

use crate::{
    ApiError, AppState,
    extract::{AuthUser, Tenant},
};

pub fn routes() -> Router<AppState> {
    Router::new().route("/users", get(list))
}

/// Every user of the caller's tenant but the caller, by username.
async fn list(tenant: Tenant, user: AuthUser) -> Result<Json<Vec<UserRef>>, ApiError> {
    Ok(Json(tenant.accounts.others(user.id).await?))
}
