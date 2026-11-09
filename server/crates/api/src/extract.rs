use std::{
    convert::Infallible,
    net::{IpAddr, SocketAddr},
};

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{HeaderMap, header::AUTHORIZATION, request::Parts},
};
use serde::Deserialize;
use uuid::Uuid;

use db::UserId;

use crate::{ApiError, AppState};

const BEARER_PREFIX: &str = "Bearer ";
const FORWARDED_FOR: &str = "x-forwarded-for";

/// The address a request came from, when it is known: the peer of the connection or, behind
/// a trusted reverse proxy, the client that proxy names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientIp(pub Option<IpAddr>);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(address)| address.ip());
        let forwarded = state
            .trust_forwarded_for
            .then(|| forwarded_for(&parts.headers))
            .flatten();
        Ok(Self(forwarded.or(peer)))
    }
}

/// The client the nearest proxy saw: the last address in `X-Forwarded-For`. A proxy appends
/// the address it accepted the connection from, so everything before it is the client's own
/// claim and must not be believed.
fn forwarded_for(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get_all(FORWARDED_FOR)
        .iter()
        .next_back()?
        .to_str()
        .ok()?
        .rsplit(',')
        .next()?
        .trim()
        .parse()
        .ok()
}

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

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(values: &[&'static str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(FORWARDED_FOR, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn the_address_the_proxy_appended_is_the_client() {
        let client: IpAddr = "203.0.113.9".parse().expect("address");
        assert_eq!(forwarded_for(&headers(&["203.0.113.9"])), Some(client));
        // What the client claimed for itself comes first and is ignored.
        assert_eq!(
            forwarded_for(&headers(&["10.0.0.1, 203.0.113.9"])),
            Some(client)
        );
        assert_eq!(
            forwarded_for(&headers(&["10.0.0.1", "10.0.0.2, 203.0.113.9"])),
            Some(client)
        );
    }

    #[test]
    fn a_missing_or_broken_header_names_nobody() {
        assert_eq!(forwarded_for(&headers(&[])), None);
        assert_eq!(forwarded_for(&headers(&["unknown"])), None);
    }
}
