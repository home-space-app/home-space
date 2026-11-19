use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, header::AUTHORIZATION, request::Parts},
};
use uuid::Uuid;

use db::{TenantId, UserId};

use super::Tenant;
use crate::{ApiError, AppState};

const BEARER_PREFIX: &str = "Bearer ";

/// The authenticated user, taken from a valid `Authorization: Bearer` access token.
#[derive(Clone, Copy, Debug)]
pub struct AuthUser {
    pub id: UserId,
    /// The tenant of the user, which is the tenant the request was sent to.
    pub tenant: TenantId,
    /// The session the token belongs to.
    pub session_id: Uuid,
    /// The device of that session, when the client named one at sign-in.
    pub device_id: Option<Uuid>,
    /// Unix timestamp at which the access token expires.
    pub token_exp: i64,
}

impl AuthUser {
    /// Authenticates from the `Authorization: Bearer` header, with a token `tenant` issued
    /// to one of its own users.
    pub async fn from_headers(headers: &HeaderMap, tenant: &Tenant) -> Result<Self, ApiError> {
        let token = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix(BEARER_PREFIX))
            .ok_or(ApiError::Unauthorized)?;
        let access = tenant
            .auth
            .verify_access(token)
            .await
            .map_err(|_| ApiError::Unauthorized)?;
        Ok(Self {
            id: access.claims.sub,
            tenant: tenant.id,
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
        let tenant = Tenant::from_request_parts(parts, state).await?;
        Self::from_headers(&parts.headers, &tenant).await
    }
}
