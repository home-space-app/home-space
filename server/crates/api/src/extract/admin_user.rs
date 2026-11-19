use axum::{extract::FromRequestParts, http::request::Parts};

use db::UserId;

use super::AuthUser;
use crate::{ApiError, AppState};

/// An authenticated administrator of the request's tenant; anyone else is refused with 403.
///
/// Checked against the database on every request, so revoking the role takes effect at once.
#[derive(Clone, Copy, Debug)]
pub struct AdminUser {
    /// The administrator's own id.
    pub id: UserId,
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        let found = state
            .db
            .find_user(user.id)
            .await?
            .filter(|found| found.tenant_id == user.tenant)
            .ok_or(ApiError::Unauthorized)?;
        if found.is_admin {
            Ok(Self { id: found.id })
        } else {
            Err(ApiError::Forbidden)
        }
    }
}
