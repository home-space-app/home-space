//! Response headers that limit what a bug elsewhere can do: what a page may load and run,
//! who may frame it, and what browsers and proxies may keep.

use std::{path::Path, sync::Arc};

use axum::{
    extract::Request,
    http::{
        HeaderValue,
        header::{
            CACHE_CONTROL, CONTENT_SECURITY_POLICY, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
            X_FRAME_OPTIONS,
        },
    },
    middleware::Next,
    response::Response,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use protocol::paths::API_PREFIX;
use sha2::{Digest, Sha256};

/// API responses are data, never a page: nothing in them may load or run anything.
const API_POLICY: &str = "default-src 'none'; frame-ancestors 'none'";
/// The web client: everything comes from this server. WebAssembly needs `wasm-unsafe-eval`,
/// and the client sets element styles (the width of a meter), hence `unsafe-inline` for styles
/// only. The scripts written into `index.html` itself are allowed by their hashes, which
/// [`web_policy`] puts in place of `{hashes}`.
const WEB_POLICY: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'{hashes}; \
    style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; object-src 'none'; \
    frame-ancestors 'none'; base-uri 'none'; form-action 'self'";
const HASHES_PLACEHOLDER: &str = "{hashes}";
const SCRIPT_OPEN: &str = "<script";
const SCRIPT_CLOSE: &str = "</script>";
const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

/// The content security policy sent with the web client's files.
#[derive(Clone, Debug)]
pub struct WebPolicy(Arc<HeaderValue>);

impl WebPolicy {
    /// The policy for the web client whose page is `index`: its inline scripts may run, and no
    /// others. A page that cannot be read gets the policy without any inline script.
    pub fn for_index(index: &Path) -> Self {
        let html = std::fs::read_to_string(index).unwrap_or_else(|err| {
            tracing::warn!(path = %index.display(), error = %err, "cannot read the web client's page");
            String::new()
        });
        Self::for_html(&html)
    }

    fn for_html(html: &str) -> Self {
        let policy = web_policy(html);
        let value = HeaderValue::from_str(&policy)
            // Hashes are base64 and the rest is constant, so this cannot fail.
            .unwrap_or_else(|_| HeaderValue::from_static(API_POLICY));
        Self(Arc::new(value))
    }
}

impl Default for WebPolicy {
    fn default() -> Self {
        Self::for_html("")
    }
}

fn web_policy(html: &str) -> String {
    let hashes: String = inline_scripts(html)
        .map(|script| format!(" 'sha256-{}'", STANDARD.encode(Sha256::digest(script))))
        .collect();
    WEB_POLICY.replace(HASHES_PLACEHOLDER, &hashes)
}

/// The text of every script written into the page itself, as opposed to loaded with `src`.
fn inline_scripts(html: &str) -> impl Iterator<Item = &str> {
    let mut rest = html;
    std::iter::from_fn(move || {
        loop {
            let after_open = &rest[rest.find(SCRIPT_OPEN)? + SCRIPT_OPEN.len()..];
            let (attributes, after_tag) = after_open.split_once('>')?;
            let (script, after_close) = after_tag.split_once(SCRIPT_CLOSE)?;
            rest = after_close;
            if !attributes.contains("src=") && !script.is_empty() {
                return Some(script);
            }
        }
    })
}

/// Adds the security headers to every response, keeping any a handler set itself.
pub async fn secure(web: WebPolicy, request: Request, next: Next) -> Response {
    let is_api = request.uri().path().starts_with(API_PREFIX);
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    let policy = if is_api {
        HeaderValue::from_static(API_POLICY)
    } else {
        (*web.0).clone()
    };
    headers.entry(CONTENT_SECURITY_POLICY).or_insert(policy);
    headers
        .entry(X_CONTENT_TYPE_OPTIONS)
        .or_insert(HeaderValue::from_static("nosniff"));
    headers
        .entry(X_FRAME_OPTIONS)
        .or_insert(HeaderValue::from_static("DENY"));
    headers
        .entry(REFERRER_POLICY)
        .or_insert(HeaderValue::from_static("no-referrer"));
    if is_api {
        // Tokens, tickets and the user's files: nothing here belongs in a cache.
        headers.entry(CACHE_CONTROL).or_insert(NO_STORE);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "<html><head><script type=\"module\">import init from '/web.js';</script>\
        <script src=\"/other.js\"></script></head><body><script>reload()</script></body></html>";

    #[test]
    fn finds_the_scripts_written_into_the_page() {
        let scripts: Vec<&str> = inline_scripts(PAGE).collect();
        assert_eq!(scripts, ["import init from '/web.js';", "reload()"]);
        assert_eq!(inline_scripts("<p>no scripts</p><script").count(), 0);
    }

    #[test]
    fn the_web_policy_allows_inline_scripts_by_hash_only() {
        let policy = web_policy(PAGE);
        let hash = STANDARD.encode(Sha256::digest("reload()"));
        assert!(policy.contains(&format!("'sha256-{hash}'")), "{policy}");
        assert_eq!(policy.matches("'sha256-").count(), 2);
        let scripts = policy
            .split("; ")
            .find(|part| part.starts_with("script-src"))
            .expect("script-src");
        assert!(!scripts.contains("unsafe-inline"), "{scripts}");
    }

    #[test]
    fn a_page_without_scripts_allows_none_inline() {
        assert!(!web_policy("").contains("sha256"));
    }
}
