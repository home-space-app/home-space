use std::{ops::Deref, sync::Arc};

use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, header::HOST, request::Parts},
};

use db::UserId;

use super::{AuthUser, last_entry, last_header};
use crate::{ApiError, AppState, Resolved, TenantContext};

const FORWARDED_HOST: &str = "x-forwarded-host";

/// The tenant a request was sent to, by its `Host`. A host no tenant answers on is refused
/// with 421 and a tenant that is not serving with 503, both without naming any tenant.
#[derive(Clone, Debug)]
pub struct Tenant(pub Arc<TenantContext>);

impl Deref for Tenant {
    type Target = TenantContext;

    fn deref(&self) -> &TenantContext {
        &self.0
    }
}

impl Tenant {
    /// Who is asking: the user of a ticket for `request`, or of the request's own sign-in.
    pub async fn caller(
        &self,
        headers: &HeaderMap,
        ticket: Option<&str>,
        request: &protocol::TicketRequest,
    ) -> Result<UserId, ApiError> {
        Ok(self.caller_session(headers, ticket, request).await?.0)
    }

    /// Who is asking, as [`Self::caller`], and from which session.
    pub async fn caller_session(
        &self,
        headers: &HeaderMap,
        ticket: Option<&str>,
        request: &protocol::TicketRequest,
    ) -> Result<(UserId, uuid::Uuid), ApiError> {
        match ticket {
            Some(ticket) => {
                let grant = self.auth.verify_ticket(ticket, request).await?;
                Ok((grant.user_id, grant.session_id))
            }
            None => {
                let user = AuthUser::from_headers(headers, self).await?;
                Ok((user.id, user.session_id))
            }
        }
    }
}

impl FromRequestParts<AppState> for Tenant {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Several extractors of one request need it; the host is looked up once.
        if let Some(tenant) = parts.extensions.get::<Self>() {
            return Ok(tenant.clone());
        }
        let host = request_host(parts, state.trust_forwarded_for);
        let tenant = match state.tenants.resolve(host) {
            Resolved::Ready(tenant) => Self(tenant),
            Resolved::Unavailable(id) => {
                tracing::debug!(tenant = %id, "request for a tenant that is not serving");
                return Err(ApiError::TenantUnavailable);
            }
            Resolved::Unknown => {
                tracing::debug!(host, "request for a host no tenant answers on");
                return Err(ApiError::UnknownHost);
            }
        };
        parts.extensions.insert(tenant.clone());
        Ok(tenant)
    }
}

/// The host a request was sent to: what the nearest proxy names in `X-Forwarded-Host` when
/// proxies are trusted, else `Host`, else the authority of an HTTP/2 request.
fn request_host(parts: &Parts, trust_forwarded: bool) -> Option<&str> {
    if trust_forwarded && let Some(hosts) = last_header(&parts.headers, FORWARDED_HOST) {
        return Some(last_entry(hosts));
    }
    if let Some(host) = last_header(&parts.headers, HOST.as_str()) {
        return Some(host);
    }
    parts.uri.authority().map(|authority| authority.as_str())
}

#[cfg(test)]
mod tests {
    use axum::http::Request;

    use super::*;

    fn parts(uri: &str, headers: &[(&str, &str)]) -> Parts {
        let mut request = Request::builder().uri(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.body(()).expect("request").into_parts().0
    }

    #[test]
    fn a_trusted_proxy_names_the_host_it_was_asked_for() {
        let parts = parts(
            "/",
            &[
                (FORWARDED_HOST, "client.example, files.example"),
                ("host", "internal:8080"),
            ],
        );
        assert_eq!(request_host(&parts, true), Some("files.example"));
        assert_eq!(request_host(&parts, false), Some("internal:8080"));
    }

    #[test]
    fn without_a_host_header_the_authority_names_the_host() {
        let parts = parts("https://files.example/api", &[]);
        assert_eq!(request_host(&parts, true), Some("files.example"));
    }
}
