use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, header::AUTHORIZATION, request::Parts},
};
use serde::Deserialize;
use uuid::Uuid;

use db::UserId;

use crate::{ApiError, AppState};

const BEARER_PREFIX: &str = "Bearer ";

/// The authenticated user, taken from a valid `Authorization: Bearer` access token.
#[derive(Clone, Copy, Debug)]
pub struct AuthUser {
    pub id: UserId,
    /// The session the token belongs to.
    pub session_id: Uuid,
    /// The device of that session, when the client named one at sign-in.
    pub device_id: Option<Uuid>,
    /// Unix timestamp at which the access token expires.
    pub token_exp: i64,
}

/// Query carrying a short-lived ticket for requests that cannot send headers.
#[derive(Debug, Deserialize)]
pub struct TicketQuery {
    pub ticket: Option<String>,
}

impl AuthUser {
    /// Authenticates from the `Authorization: Bearer` header.
    pub async fn from_headers(headers: &HeaderMap, state: &AppState) -> Result<Self, ApiError> {
        let token = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix(BEARER_PREFIX))
            .ok_or(ApiError::Unauthorized)?;
        let access = state
            .auth
            .verify_access(token)
            .await
            .map_err(|_| ApiError::Unauthorized)?;
        Ok(Self {
            id: access.claims.sub,
            session_id: access.claims.sid,
            device_id: access.device_id,
            token_exp: access.claims.exp,
        })
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Self::from_headers(&parts.headers, state).await
    }
}

/// An authenticated administrator; anyone else is refused with 403.
///
/// Checked against the database on every request, so revoking the role takes effect at once.
#[derive(Clone, Copy, Debug)]
pub struct AdminUser;

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
            .await
            .map_err(|err| ApiError::Internal(err.to_string()))?
            .ok_or(ApiError::Unauthorized)?;
        if found.is_admin {
            Ok(Self)
        } else {
            Err(ApiError::Forbidden)
        }
    }
}
