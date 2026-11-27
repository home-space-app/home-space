//! What only a browser does: it names the page a request comes from in `Origin`, and it keeps
//! the refresh token in a cookie that the page's scripts cannot read.
//!
//! A cookie is sent along by the browser whatever page asks, so every request that relies on
//! it, and the websocket, must come from this server's own pages, and from the pages of the
//! tenant the request was sent to. Passkeys and security keys are held to the tenant's own
//! names where it has them ([`WebAuthnSites`]).

use std::{borrow::Cow, net::IpAddr};

use axum::http::{
    HeaderMap, HeaderValue,
    header::{COOKIE, HOST, ORIGIN},
};
use protocol::{encoding::authority_host, paths};
use time::Duration;

use crate::{ApiError, TenantContext, tenants::normalize_host};

/// Name of the cookie that holds a browser's refresh token.
pub const REFRESH_COOKIE: &str = "homespace_refresh";
const HTTPS: &str = "https://";
const HTTPS_SCHEME: &str = "https";
const HTTP_SCHEME: &str = "http";
const SCHEME_END: &str = "://";
const LOCALHOST: &str = "localhost";

/// A browser on one of this server's own pages.
#[derive(Clone, Copy, Debug)]
pub struct Browser {
    /// The page was loaded over https, so the cookie must never travel in clear text.
    secure: bool,
}

impl Browser {
    /// The browser behind a request to `tenant` that uses the cookie; any other page is
    /// refused.
    pub fn of(headers: &HeaderMap, tenant: &TenantContext) -> Result<Self, ApiError> {
        let origin = origin(headers).ok_or(ApiError::Forbidden)?;
        if is_own_origin(origin, headers, tenant.public_url.as_deref()) {
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
pub fn refuse_other_sites(headers: &HeaderMap, tenant: &TenantContext) -> Result<(), ApiError> {
    match origin(headers) {
        Some(origin) if !is_own_origin(origin, headers, tenant.public_url.as_deref()) => {
            Err(ApiError::Forbidden)
        }
        _ => Ok(()),
    }
}

/// Where a tenant's passkeys, security keys and folder keys may be used: the pages that may
/// ask for a WebAuthn ceremony, and the RP ids their credentials are made for. The one place
/// that decides it.
///
/// A tenant with names of its own (the hosts of its `public_url` and its domains) takes only
/// pages on one of them, over https or, for a loopback name, http, whatever `Host` says: a proxy
/// that rewrites `Host` changes nothing, and no one can try passkeys under other names. Like
/// WebAuthn, it goes by the host and not the port, and each name is an RP id of its own. Only a
/// tenant without names (development, a server on a bare LAN) goes by the name the request was
/// sent to, and takes a page on another loopback port too.
///
/// One RP id for the whole tenant, with its other names allowed through Related Origin
/// Requests (`/.well-known/webauthn`), would change only this.
#[derive(Debug)]
pub struct WebAuthnSites<'a> {
    headers: &'a HeaderMap,
    /// The hosts of the tenant's names, lower-case and without ports, the `public_url`'s first;
    /// empty when it has none.
    names: Vec<Cow<'a, str>>,
}

impl<'a> WebAuthnSites<'a> {
    /// The sites of `tenant` for a request with `headers`.
    pub fn of(headers: &'a HeaderMap, tenant: &'a TenantContext) -> Self {
        Self::new(headers, tenant.public_url.as_deref(), &tenant.domains)
    }

    /// `domains` are stored normalized already.
    fn new(headers: &'a HeaderMap, public_url: Option<&str>, domains: &'a [String]) -> Self {
        let public = public_url
            .and_then(url_authority)
            .and_then(normalize_host)
            .map(|authority| Cow::Owned(authority_host(&authority).to_owned()));
        let names = public
            .into_iter()
            .chain(
                domains
                    .iter()
                    .map(|domain| Cow::Borrowed(authority_host(domain))),
            )
            .collect();
        Self { headers, names }
    }

    /// The origin of the page the request comes from, when it may run the tenant's WebAuthn
    /// ceremonies; `None` for any other page and for apps, which send no `Origin`.
    pub fn origin(&self) -> Option<&'a str> {
        let origin = origin(self.headers)?;
        let allowed = if self.names.is_empty() {
            is_own_origin(origin, self.headers, None)
        } else {
            self.is_named_origin(origin)
        };
        allowed.then_some(origin)
    }

    /// The RP id of the pages the request was sent from: the name it was sent to when that is
    /// one of the tenant's, else the tenant's first name; without names, the name it was sent
    /// to. `None` for an IP address, which WebAuthn allows no credentials for.
    pub fn rp_id(&self) -> Option<&str> {
        let sent_to = self.headers.get(HOST).and_then(|value| value.to_str().ok());
        let host = if self.names.is_empty() {
            authority_host(sent_to?)
        } else {
            let sent_to = sent_to.and_then(normalize_host);
            let sent_to = sent_to.as_deref().map(authority_host);
            let mut names = self.names.iter().map(AsRef::as_ref);
            names
                .clone()
                .find(|name| sent_to == Some(*name))
                .or_else(|| names.next())?
        };
        Some(host).filter(|host| is_name(host))
    }

    /// Whether a credential made for `rp_id` was made on the tenant's pages.
    pub fn is_rp_id(&self, rp_id: &str) -> bool {
        if self.names.is_empty() {
            self.rp_id()
                .is_some_and(|own| own.eq_ignore_ascii_case(rp_id))
        } else {
            self.names
                .iter()
                .any(|name| name.eq_ignore_ascii_case(rp_id))
        }
    }

    fn is_named_origin(&self, origin: &str) -> bool {
        let Some((scheme, authority)) = origin.split_once(SCHEME_END) else {
            return false;
        };
        let Some(authority) = normalize_host(authority) else {
            return false;
        };
        let host = authority_host(&authority);
        let secure = scheme == HTTPS_SCHEME || (scheme == HTTP_SCHEME && is_loopback(host));
        secure && self.names.iter().any(|name| name.as_ref() == host)
    }
}

/// Whether a WebAuthn credential made for `rp_id` was made on `tenant`'s pages; see
/// [`WebAuthnSites`].
pub fn is_own_rp_id(rp_id: &str, headers: &HeaderMap, tenant: &TenantContext) -> bool {
    WebAuthnSites::of(headers, tenant).is_rp_id(rp_id)
}

/// The host and port of `url`.
fn url_authority(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once(SCHEME_END)?;
    rest.split('/').next()
}

/// Whether `host` is a name rather than an IP address, as WebAuthn needs.
fn is_name(host: &str) -> bool {
    !host.is_empty() && host.trim_matches(['[', ']']).parse::<IpAddr>().is_err()
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
/// tenant's `public_url`, which covers a proxy that does not pass `Host` on unchanged. Two
/// loopback addresses also count as one, for a development server on another port.
fn is_own_origin(origin: &str, headers: &HeaderMap, public_url: Option<&str>) -> bool {
    let Some((_, authority)) = origin.split_once(SCHEME_END) else {
        return false;
    };
    let host = headers.get(HOST).and_then(|value| value.to_str().ok());
    host.is_some_and(|host| host.eq_ignore_ascii_case(authority))
        || public_url.is_some_and(|url| url.trim_end_matches('/').eq_ignore_ascii_case(origin))
        || host.is_some_and(|host| is_loopback(host) && is_loopback(authority))
}

/// Whether `authority` (a host with an optional port) names this machine.
fn is_loopback(authority: &str) -> bool {
    let host = authority_host(authority);
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

    #[test]
    fn only_the_tenants_own_public_url_counts() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("homespace:8080"));
        let ours = Some("https://files.example.com/");
        assert!(is_own_origin("https://files.example.com", &headers, ours));
        assert!(is_own_origin("http://homespace:8080", &headers, ours));
        assert!(!is_own_origin("https://cloud.other.org", &headers, ours));
        assert!(!is_own_origin("https://files.example.com", &headers, None));
    }

    fn page(host: &'static str, origin: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static(host));
        headers.insert(ORIGIN, HeaderValue::from_static(origin));
        headers
    }

    #[test]
    fn a_tenant_with_a_public_url_takes_passkeys_only_there() {
        let ours = Some("https://Files.Example.com/");
        // A proxy that rewrites `Host` changes nothing.
        let proxied = page("homespace:8080", "https://files.example.com");
        let sites = WebAuthnSites::new(&proxied, ours, &[]);
        assert_eq!(sites.origin(), Some("https://files.example.com"));
        assert_eq!(sites.rp_id(), Some("files.example.com"));
        assert!(sites.is_rp_id("files.example.com"));
        assert!(!sites.is_rp_id("homespace"));
        for (host, origin) in [
            ("homespace:8080", "http://homespace:8080"),
            ("other.example", "https://other.example"),
            ("files.example.com", "http://files.example.com"),
            ("localhost:8080", "http://localhost:8081"),
        ] {
            let headers = page(host, origin);
            let sites = WebAuthnSites::new(&headers, ours, &[]);
            assert_eq!(sites.origin(), None, "{origin} on {host}");
            assert_eq!(sites.rp_id(), Some("files.example.com"), "{host}");
        }
    }

    #[test]
    fn each_domain_of_a_tenant_is_a_site_of_its_own() {
        let domains = [
            "files.example.com".to_owned(),
            "files.other.org:8443".to_owned(),
        ];
        let headers = page("files.other.org:8443", "https://files.other.org:8443");
        let sites = WebAuthnSites::new(&headers, None, &domains);
        assert_eq!(sites.origin(), Some("https://files.other.org:8443"));
        assert_eq!(sites.rp_id(), Some("files.other.org"));
        assert!(sites.is_rp_id("files.example.com") && sites.is_rp_id("files.other.org"));
        let elsewhere = page("files.other.org:8443", "https://other.org");
        assert_eq!(
            WebAuthnSites::new(&elsewhere, None, &domains).origin(),
            None
        );
    }

    #[test]
    fn a_loopback_public_url_takes_its_pages_over_http() {
        let ours = Some("http://localhost:8080");
        // The web client's development server, on another port of the same name.
        let dev = page("localhost:8080", "http://localhost:8081");
        assert!(WebAuthnSites::new(&dev, ours, &[]).origin().is_some());
        let other_name = page("localhost:8080", "http://127.0.0.1:8081");
        assert_eq!(WebAuthnSites::new(&other_name, ours, &[]).origin(), None);
    }

    #[test]
    fn without_names_the_host_the_request_was_sent_to_counts() {
        let dev = page("localhost:8080", "http://localhost:8081");
        let sites = WebAuthnSites::new(&dev, None, &[]);
        assert_eq!(sites.origin(), Some("http://localhost:8081"));
        assert_eq!(sites.rp_id(), Some("localhost"));
        assert!(sites.is_rp_id("localhost") && !sites.is_rp_id("files.example.com"));
        let lan = page("192.168.1.5:8080", "http://192.168.1.5:8080");
        assert_eq!(WebAuthnSites::new(&lan, None, &[]).rp_id(), None);
        let by_address = page("192.168.1.5", "https://192.168.1.5");
        let sites = WebAuthnSites::new(&by_address, Some("https://192.168.1.5"), &[]);
        assert_eq!(sites.rp_id(), None);
    }
}
