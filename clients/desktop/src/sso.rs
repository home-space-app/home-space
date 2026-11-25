//! Signing in in the system browser (RFC 8252), through the server's identity provider or the
//! server's own web page: opens the browser at the server's sign-in URL and waits for the
//! server to send the browser back to a port on this machine.

use std::time::Duration;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};

use api_client::{ApiError, Client, Pkce, VERIFIER_BYTES};
use client_core::sso::{SsoReply, sso_reply};
use protocol::Me;

use crate::session;

const CALLBACK_PATH: &str = "/callback";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REQUEST_BYTES: usize = 8 * 1024;

const ACCESS_DENIED_MESSAGE: &str = "You are not allowed to sign in with this account.";
const CANCELLED_MESSAGE: &str = "Signing in was cancelled in the browser.";
const FAILED_MESSAGE: &str = "Signing in failed. Please try again.";
const EXPIRED_MESSAGE: &str = "The sign-in expired. Please try again.";
const TIMED_OUT_MESSAGE: &str = "Timed out waiting for the browser sign-in.";
const BAD_ADDRESS_MESSAGE: &str = "The server address must start with http:// or https://";

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Home Space</title>\
    <p style=\"font-family:system-ui;margin:3rem auto;max-width:28rem\">\
    You can close this tab and return to Home Space.</p>";
/// [`DONE_PAGE`] once signed in, which also closes its tab after 4 seconds where the browser
/// lets it (a tab that is one page deep, as the server's sign-in leaves it).
const SIGNED_IN_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Home Space</title>\
    <p style=\"font-family:system-ui;margin:3rem auto;max-width:28rem\">\
    You are signed in. You can close this tab and return to Home Space.</p>\
    <script>setTimeout(() => window.close(), 4000)</script>";

/// Where the browser signs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// The server's identity provider (single sign-on).
    Provider,
    /// The server's own web page, which signs in however the server allows.
    WebPage,
}

/// How the server sent the browser back.
#[derive(Debug, PartialEq, Eq)]
enum Callback {
    Code(String),
    Error(String),
}

/// Signs in `via` the provider or the web page, using the system browser.
pub async fn sign_in(server: String, via: Via) -> Result<(Client, Me), String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|err| format!("Could not listen for the sign-in: {err}"))?;
    let port = listener
        .local_addr()
        .map_err(|err| format!("Could not listen for the sign-in: {err}"))?
        .port();
    let mut bytes = [0u8; VERIFIER_BYTES];
    rand::fill(&mut bytes);
    let pkce = Pkce::from_random(bytes);

    let client = session::client(&server);
    let redirect = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
    let url = match via {
        Via::Provider => client.oidc_start_url(&redirect, &pkce),
        Via::WebPage => client.browser_login_start_url(&redirect, &pkce),
    }
    .map_err(|err| err.to_string())?;
    // `open` would launch anything, so only ever hand it a web address.
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(BAD_ADDRESS_MESSAGE.to_owned());
    }
    open::that_detached(&url).map_err(|err| format!("Could not open the browser: {err}"))?;

    let callback = timeout(SIGN_IN_TIMEOUT, wait_for_callback(&listener))
        .await
        .map_err(|_| TIMED_OUT_MESSAGE.to_owned())??;
    match callback {
        Callback::Code(code) => {
            let me = match via {
                Via::Provider => client.oidc_exchange(&code, &pkce).await,
                Via::WebPage => client.browser_login_exchange(&code, &pkce).await,
            }
            .map_err(|err| exchange_error(&err))?;
            Ok((client, me))
        }
        Callback::Error(reason) => Err(refusal(via, &reason).to_owned()),
    }
}

/// A failed trade of the code for a session. Servers say why; an older one's bare `401` means
/// the code expired or was already used.
fn exchange_error(err: &ApiError) -> String {
    match err {
        ApiError::Unauthorized => EXPIRED_MESSAGE.to_owned(),
        other => other.to_string(),
    }
}

/// What the user reads when the browser came back with `reason` instead of a code.
fn refusal(via: Via, reason: &str) -> &'static str {
    match (via, reason) {
        // On the server's own page, the only refusal is the user pressing Cancel.
        (Via::WebPage, "access_denied") => CANCELLED_MESSAGE,
        (Via::Provider, "access_denied") => ACCESS_DENIED_MESSAGE,
        _ => FAILED_MESSAGE,
    }
}

/// Serves requests until one is the server's redirect; others (a browser asking for the
/// favicon, say) get a 404.
async fn wait_for_callback(listener: &TcpListener) -> Result<Callback, String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|err| format!("Could not receive the sign-in: {err}"))?;
        let callback = timeout(READ_TIMEOUT, read_callback(&mut stream))
            .await
            .ok()
            .flatten();
        respond(&mut stream, callback.as_ref()).await;
        if let Some(callback) = callback {
            return Ok(callback);
        }
    }
}

async fn read_callback(stream: &mut TcpStream) -> Option<Callback> {
    let mut request = Vec::new();
    let mut chunk = [0u8; 1024];
    while !request.contains(&b'\n') && request.len() < MAX_REQUEST_BYTES {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
    }
    let text = String::from_utf8_lossy(&request);
    parse_request_line(text.lines().next()?)
}

async fn respond(stream: &mut TcpStream, callback: Option<&Callback>) {
    let (status, body) = match callback {
        Some(Callback::Code(_)) => ("200 OK", SIGNED_IN_PAGE),
        Some(Callback::Error(_)) => ("200 OK", DONE_PAGE),
        None => ("404 Not Found", ""),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\n\
         cache-control: no-store\r\nreferrer-policy: no-referrer\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Reads `GET /callback?code=...` or `GET /callback?error=...`.
fn parse_request_line(line: &str) -> Option<Callback> {
    let mut parts = line.split_whitespace();
    if parts.next()? != "GET" {
        return None;
    }
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{}", parts.next()?)).ok()?;
    if url.path() != CALLBACK_PATH {
        return None;
    }
    Some(match sso_reply(&url)? {
        SsoReply::Code(code) => Callback::Code(code),
        SsoReply::Error(error) => Callback::Error(error),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_on_the_web_page_is_a_cancel() {
        assert_eq!(refusal(Via::WebPage, "access_denied"), CANCELLED_MESSAGE);
        assert_eq!(
            refusal(Via::Provider, "access_denied"),
            ACCESS_DENIED_MESSAGE
        );
        assert_eq!(refusal(Via::WebPage, "server_error"), FAILED_MESSAGE);
    }

    #[test]
    fn reads_a_code() {
        assert_eq!(
            parse_request_line("GET /callback?code=a%20b HTTP/1.1"),
            Some(Callback::Code("a b".to_owned()))
        );
    }

    #[test]
    fn reads_an_error() {
        assert_eq!(
            parse_request_line("GET /callback?error=access_denied HTTP/1.1"),
            Some(Callback::Error("access_denied".to_owned()))
        );
    }

    #[test]
    fn ignores_everything_else() {
        for line in [
            "GET /favicon.ico HTTP/1.1",
            "GET /callback HTTP/1.1",
            "GET /callback?code= HTTP/1.1",
            "GET /callback/extra?code=x HTTP/1.1",
            "POST /callback?code=x HTTP/1.1",
            "GET",
            "",
        ] {
            assert_eq!(parse_request_line(line), None, "{line}");
        }
    }

    async fn get(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        stream
            .write_all(format!("GET {target} HTTP/1.1\r\nhost: localhost\r\n\r\n").as_bytes())
            .await
            .expect("send");
        let mut response = String::new();
        stream.read_to_string(&mut response).await.expect("read");
        response
    }

    #[tokio::test]
    async fn waits_past_unrelated_requests_for_the_redirect() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let browser = tokio::spawn(async move {
            let favicon = get(port, "/favicon.ico").await;
            let callback = get(port, "/callback?code=login-code").await;
            (favicon, callback)
        });

        let callback = wait_for_callback(&listener).await.expect("callback");
        let (favicon, page) = browser.await.expect("browser");

        assert_eq!(callback, Callback::Code("login-code".to_owned()));
        assert!(favicon.starts_with("HTTP/1.1 404"));
        assert!(page.starts_with("HTTP/1.1 200"));
        assert!(page.contains("return to Home Space"));
    }

    #[test]
    fn a_failed_exchange_says_why() {
        assert_eq!(exchange_error(&ApiError::Unauthorized), EXPIRED_MESSAGE);
        let refused = ApiError::Api {
            code: protocol::ErrorCode::Forbidden,
            message: "This account is disabled.".to_owned(),
        };
        assert_eq!(exchange_error(&refused), "This account is disabled.");
    }
}
