use std::{
    convert::Infallible,
    net::{IpAddr, SocketAddr},
};

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{HeaderMap, request::Parts},
};

use super::{last_entry, last_header};
use crate::AppState;

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
        let forwarded = if state.trust_forwarded_for {
            forwarded_for(&parts.headers)
        } else {
            None
        };
        Ok(Self(forwarded.or(peer)))
    }
}

/// The client the nearest proxy saw: the last address in `X-Forwarded-For`. A proxy appends
/// the address it accepted the connection from, so everything before it is the client's own
/// claim and must not be believed.
fn forwarded_for(headers: &HeaderMap) -> Option<IpAddr> {
    let list = last_header(headers, FORWARDED_FOR)?;
    last_entry(list).parse().ok()
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
