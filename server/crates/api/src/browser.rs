//! What only a browser does: it names the page a request comes from in `Origin`, and it keeps
//! the refresh token in a cookie that the page's scripts cannot read.
//!
//! A cookie is sent along by the browser whatever page asks, so every request that relies on
//! it, and the websocket, must come from this server's own pages.

use std::net::IpAddr;

use axum::http::{
    HeaderMap, HeaderValue,
    header::{COOKIE, HOST, ORIGIN},
};
use protocol::paths;
use time::Duration;

use crate::{ApiError, AppState};

/// Name of the cookie that holds a browser's refresh token.
pub const REFRESH_COOKIE: &str = "homespace_refresh";
const HTTPS: &str = "https://";
const LOCALHOST: &str = "localhost";

/// A browser on one of this server's own pages.
#[derive(Clone, Copy, Debug)]
pub struct Browser {
    /// The page was loaded over https, so the cookie must never travel in clear text.
    secure: bool,
}

impl Browser {
    /// The browser behind a request that uses the cookie; any other page is refused.
    pub fn of(headers: &HeaderMap, state: &AppState) -> Result<Self, ApiError> {
        let origin = origin(headers).ok_or(ApiError::Forbidden)?;
        if is_own_origin(origin, headers, state) {
            Ok(Self {
                secure: origin.starts_with(HTTPS),
            })
        } else {
            Err(ApiError::Forbidden)
        }
    }

    /// The header that stores `token` in the browser for `lifetime`.
    pub fn keep(self, token: &str, lifetime: Duration) -> HeaderValue {
        self.cookie(token, lifetime.whole_seconds())
    }

    /// The header that makes the browser forget its token.
    pub fn forget(self) -> HeaderValue {
        self.cookie("", 0)
    }

    /// Only this server's sign-in routes get the cookie (`Path`), no script can read it
    /// (`HttpOnly`) and no other site can have it sent (`SameSite=Strict`).
    fn cookie(self, token: &str, max_age: i64) -> HeaderValue {
        let secure = if self.secure { "; Secure" } else { "" };
        let cookie = format!(
            "{REFRESH_COOKIE}={token}; Path={}; Max-Age={max_age}; HttpOnly; SameSite=Strict{secure}",
            paths::AUTH
        );
        // Refresh tokens are base64url, which is always a valid header value.
        HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static(""))
    }
}

/// Refuses a request that a page of another site made. A request without `Origin` does not
/// come from a page at all, but from one of the apps.
pub fn refuse_other_sites(headers: &HeaderMap, state: &AppState) -> Result<(), ApiError> {
    match origin(headers) {
        Some(origin) if !is_own_origin(origin, headers, state) => Err(ApiError::Forbidden),
        _ => Ok(()),
    }
}

/// The refresh token in the request's cookies.
pub fn refresh_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .find(|(name, token)| *name == REFRESH_COOKIE && !token.is_empty())
        .map(|(_, token)| token)
}

fn origin(headers: &HeaderMap) -> Option<&str> {
    headers.get(ORIGIN)?.to_str().ok()
}

/// Whether `origin` is this server: the address the request was sent to (`Host`), or the
/// configured public URL, which covers a proxy that does not pass `Host` on unchanged. Two
/// loopback addresses also count as one, for a development server on another port.
fn is_own_origin(origin: &str, headers: &HeaderMap, state: &AppState) -> bool {
    let Some((_, authority)) = origin.split_once("://") else {
        return false;
    };
    let host = headers.get(HOST).and_then(|value| value.to_str().ok());
    let public_url = state.info.config.public_url.as_deref();
    host.is_some_and(|host| host.eq_ignore_ascii_case(authority))
        || public_url.is_some_and(|url| url.trim_end_matches('/').eq_ignore_ascii_case(origin))
        || host.is_some_and(|host| is_loopback(host) && is_loopback(authority))
}

/// Whether `authority` (a host with an optional port) names this machine.
fn is_loopback(authority: &str) -> bool {
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    host.eq_ignore_ascii_case(LOCALHOST)
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookies(values: &[&'static str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(COOKIE, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn finds_the_refresh_token_among_other_cookies() {
        assert_eq!(
            refresh_cookie(&cookies(&["theme=dark; homespace_refresh=abc_-1; x=y"])),
            Some("abc_-1")
        );
        assert_eq!(
            refresh_cookie(&cookies(&["theme=dark", "homespace_refresh=abc"])),
            Some("abc")
        );
        assert_eq!(refresh_cookie(&cookies(&["theme=dark"])), None);
        assert_eq!(refresh_cookie(&cookies(&["homespace_refresh="])), None);
        assert_eq!(refresh_cookie(&cookies(&["xhomespace_refresh=abc"])), None);
    }

    #[test]
    fn the_cookie_is_closed_to_scripts_other_sites_and_other_paths() {
        let cookie = Browser { secure: true }.keep("abc", Duration::days(30));
        assert_eq!(
            cookie.to_str().expect("text"),
            "homespace_refresh=abc; Path=/api/v1/auth; Max-Age=2592000; HttpOnly; \
             SameSite=Strict; Secure"
        );
        let plain = Browser { secure: false }.forget();
        assert_eq!(
            plain.to_str().expect("text"),
            "homespace_refresh=; Path=/api/v1/auth; Max-Age=0; HttpOnly; SameSite=Strict"
        );
    }

    #[test]
    fn loopback_is_told_from_other_hosts() {
        for authority in [
            "localhost:8081",
            "127.0.0.1:8080",
            "[::1]:8080",
            "LOCALHOST",
        ] {
            assert!(is_loopback(authority), "{authority}");
        }
        for authority in [
            "example.com",
            "localhost.example.com",
            "192.168.1.5:8080",
            "",
        ] {
            assert!(!is_loopback(authority), "{authority}");
        }
    }
}
