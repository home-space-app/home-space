use std::sync::Arc;

use leptos::{prelude::*, task::spawn_local};

use api_client::{ApiError, Client, Pkce, VERIFIER_BYTES};
use protocol::{DeviceInfo, Me, PLATFORM_WEB};
use uuid::Uuid;

/// Where the refresh token used to be kept. The server now keeps it in a cookie that scripts
/// cannot read; a token still found here is handed over once and removed.
const OLD_REFRESH_TOKEN_KEY: &str = "homespace.refresh_token";
/// Remembers that this browser signed in, so that a visit without a session does not ask the
/// server for one. It holds no secret: the session itself is in the cookie.
const SIGNED_IN_KEY: &str = "homespace.signed_in";
const SIGNED_IN: &str = "1";
const DEVICE_ID_KEY: &str = "homespace.device_id";
/// Browsers by a token of their user agent, the more specific first: Edge and Opera also
/// claim to be Chrome, and Chrome claims to be Safari.
const BROWSERS: [(&str, &str); 5] = [
    ("Edg/", "Edge"),
    ("OPR/", "Opera"),
    ("Firefox/", "Firefox"),
    ("Chrome/", "Chrome"),
    ("Safari/", "Safari"),
];
const FALLBACK_BROWSER: &str = "Web browser";
const INVALID_LOGIN_MESSAGE: &str = "Invalid username or password";
const OIDC_VERIFIER_KEY: &str = "homespace.oidc_verifier";
const OIDC_CALLBACK_PATH: &str = "/login/callback";
const OIDC_EXPIRED_MESSAGE: &str = "The sign-in expired. Please try again.";

thread_local! {
    static CLIENT: Client = build_client();
}

/// The API client for this origin; clones share one session.
pub fn client() -> Client {
    CLIENT.with(Client::clone)
}

fn build_client() -> Client {
    let origin = web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_default();
    let client = Client::with_listener(origin, Arc::new(remember_signed_in)).with_cookie_session();
    match device() {
        Some(device) => client.with_device(device),
        None => client,
    }
}

/// This browser as one of the user's devices, its id kept in local storage; `None` when
/// there is no storage to keep it in.
fn device() -> Option<DeviceInfo> {
    let storage = storage()?;
    let saved = storage
        .get_item(DEVICE_ID_KEY)
        .ok()
        .flatten()
        .and_then(|id| id.parse::<Uuid>().ok());
    let id = match saved {
        Some(id) => id,
        None => {
            let id = web_sys::window()?
                .crypto()
                .ok()?
                .random_uuid()
                .parse::<Uuid>()
                .ok()?;
            storage.set_item(DEVICE_ID_KEY, &id.to_string()).ok()?;
            id
        }
    };
    let agent = web_sys::window()
        .and_then(|window| window.navigator().user_agent().ok())
        .unwrap_or_default();
    Some(DeviceInfo {
        id,
        name: browser_name(&agent).to_owned(),
        platform: PLATFORM_WEB.to_owned(),
    })
}

/// The browser's name from its user agent.
fn browser_name(agent: &str) -> &'static str {
    BROWSERS
        .iter()
        .find(|(token, _)| agent.contains(token))
        .map_or(FALLBACK_BROWSER, |(_, name)| name)
}

pub(crate) fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Called when the session starts, continues (`Some`) or ends (`None`).
fn remember_signed_in(session: Option<&str>) {
    if let Some(storage) = storage() {
        let _ = storage.remove_item(OLD_REFRESH_TOKEN_KEY);
        let _ = match session {
            Some(_) => storage.set_item(SIGNED_IN_KEY, SIGNED_IN),
            None => storage.remove_item(SIGNED_IN_KEY),
        };
    }
}

/// Kept only for the tab that started a single sign-on, and only until it comes back.
fn session_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok().flatten()
}

/// A refresh token from before the cookie existed.
fn old_refresh_token() -> Option<String> {
    storage()?.get_item(OLD_REFRESH_TOKEN_KEY).ok().flatten()
}

fn was_signed_in() -> bool {
    storage()
        .and_then(|storage| storage.get_item(SIGNED_IN_KEY).ok().flatten())
        .is_some()
}

/// Reactive sign-in state shared by all pages.
#[derive(Clone, Copy)]
pub struct Auth {
    pub user: RwSignal<Option<Me>>,
    pub checking: RwSignal<bool>,
}

impl Auth {
    /// Creates the state and resumes a saved session in the background.
    pub fn start() -> Self {
        let old_token = old_refresh_token();
        let resume = old_token.is_some() || was_signed_in();
        let auth = Self {
            user: RwSignal::new(None),
            checking: RwSignal::new(resume),
        };
        if resume {
            spawn_local(async move {
                if let Ok(me) = client().resume(old_token.as_deref()).await {
                    auth.user.set(Some(me));
                }
                auth.checking.set(false);
            });
        }
        auth
    }

    /// Signs in; the error is a message fit for display.
    pub async fn login(self, username: &str, password: &str) -> Result<(), String> {
        match client().login(username, password).await {
            Ok(me) => {
                self.user.set(Some(me));
                Ok(())
            }
            Err(ApiError::Unauthorized) => Err(INVALID_LOGIN_MESSAGE.to_owned()),
            Err(err) => Err(err.to_string()),
        }
    }

    /// Sends the browser to the single sign-on provider; it comes back to `/login/callback`.
    pub fn start_oidc() -> Result<(), String> {
        let window = web_sys::window().ok_or("No browser window")?;
        let mut bytes = [0u8; VERIFIER_BYTES];
        window
            .crypto()
            .and_then(|crypto| crypto.get_random_values_with_u8_array(&mut bytes))
            .map_err(|_| "The browser cannot generate secure random numbers")?;
        let pkce = Pkce::from_random(bytes);
        session_storage()
            .and_then(|storage| storage.set_item(OIDC_VERIFIER_KEY, pkce.verifier()).ok())
            .ok_or("The browser cannot store the sign-in state")?;
        let origin = window
            .location()
            .origin()
            .map_err(|_| "No browser origin")?;
        let url = client()
            .oidc_start_url(&format!("{origin}{OIDC_CALLBACK_PATH}"), &pkce)
            .map_err(|err| err.to_string())?;
        window
            .location()
            .set_href(&url)
            .map_err(|_| "Could not open the sign-in page".to_owned())
    }

    /// Completes a single sign-on with the `code` from the redirect.
    pub async fn finish_oidc(self, code: &str) -> Result<(), String> {
        let verifier = take_oidc_verifier().ok_or(OIDC_EXPIRED_MESSAGE)?;
        match client()
            .oidc_exchange(code, &Pkce::from_verifier(verifier))
            .await
        {
            Ok(me) => {
                self.user.set(Some(me));
                Ok(())
            }
            Err(ApiError::Unauthorized) => Err(OIDC_EXPIRED_MESSAGE.to_owned()),
            Err(err) => Err(err.to_string()),
        }
    }

    /// Ends the session.
    pub fn logout(self) {
        spawn_local(async move {
            let _ = client().logout().await;
            self.user.set(None);
        });
    }

    /// Turns a failed call into a message, signing out when the session is gone.
    pub fn fail(self, err: &ApiError) -> String {
        if matches!(err, ApiError::Unauthorized) {
            self.user.set(None);
        }
        err.to_string()
    }
}

/// The verifier saved when the sign-in started; it can be used once.
pub fn take_oidc_verifier() -> Option<String> {
    let storage = session_storage()?;
    let verifier = storage.get_item(OIDC_VERIFIER_KEY).ok().flatten();
    let _ = storage.remove_item(OIDC_VERIFIER_KEY);
    verifier
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browsers_that_claim_to_be_others_are_named_for_themselves() {
        let edge =
            "Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 Chrome/130.0 Safari/537.36 Edg/130.0";
        let chrome =
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/130.0 Safari/537.36";
        let firefox = "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0";
        assert_eq!(browser_name(edge), "Edge");
        assert_eq!(browser_name(chrome), "Chrome");
        assert_eq!(browser_name(firefox), "Firefox");
        assert_eq!(browser_name(""), FALLBACK_BROWSER);
    }
}
