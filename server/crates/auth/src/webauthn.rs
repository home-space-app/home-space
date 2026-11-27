//! The WebAuthn relying party a request speaks to, and the stored form of its credentials.
//!
//! The relying party is the page that asked: its origin must be one of the tenant's sites (the
//! caller checks that: its configured names where it has them, else the host the request was
//! sent to), https or `localhost`, and its host becomes the RP id. So a tenant reached under two
//! of its names has passkeys for each, and one reached by IP address has none, since WebAuthn
//! does not allow IP addresses.

use webauthn_rs::prelude::{Credential, Passkey, SecurityKey, Url, Webauthn, WebauthnBuilder};

use crate::{AuthError, urls::is_https_or_loopback};

/// What authenticators show as the site's name.
const RP_NAME: &str = "Home Space";

/// The site a WebAuthn ceremony is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelyingParty {
    id: String,
    origin: Url,
}

impl RelyingParty {
    /// The relying party of a page at `origin`, such as `https://files.example.com`.
    pub fn new(origin: &str) -> Result<Self, AuthError> {
        if !is_https_or_loopback(origin) {
            return Err(AuthError::WebAuthnUnavailable(
                "Passkeys need the server's https address (or localhost).",
            ));
        }
        let url = Url::parse(origin)
            .ok()
            .filter(|url| url.path() == "/" && url.query().is_none() && url.username().is_empty())
            .ok_or(AuthError::WebAuthnUnavailable(
                "The page's address is not valid.",
            ))?;
        let id = url
            .domain()
            .ok_or(AuthError::WebAuthnUnavailable(
                "Passkeys don't work on an IP address. Use the server's name instead.",
            ))?
            .to_ascii_lowercase();
        Ok(Self { id, origin: url })
    }

    /// The host credentials are made for.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The page's origin, as it was given, without the trailing `/`.
    pub fn origin(&self) -> &str {
        self.origin.as_str().trim_end_matches('/')
    }

    pub(crate) fn webauthn(&self) -> Result<Webauthn, AuthError> {
        WebauthnBuilder::new(&self.id, &self.origin)
            .and_then(|builder| builder.rp_name(RP_NAME).build())
            .map_err(|err| {
                tracing::warn!(error = %err, origin = %self.origin, "no WebAuthn for this origin");
                AuthError::WebAuthnUnavailable("Passkeys can't be used on this address.")
            })
    }
}

/// A stored credential, as webauthn-rs gave it at registration.
pub(crate) fn parse_credential(json: &str) -> Result<Credential, AuthError> {
    serde_json::from_str(json).map_err(|err| {
        tracing::error!(error = %err, "a stored WebAuthn credential can't be read");
        AuthError::WebAuthnFailed
    })
}

pub(crate) fn credential_json(credential: &Credential) -> Result<String, AuthError> {
    serde_json::to_string(credential).map_err(AuthError::WebAuthnEncoding)
}

/// The stored credential as a passkey, which can update its counter and backup flags.
pub(crate) fn passkey(credential: Credential) -> Passkey {
    credential.into()
}

pub(crate) fn security_key(credential: Credential) -> SecurityKey {
    credential.into()
}

/// A ceremony's state, kept in the database between its two requests.
pub(crate) fn state_json<T: serde::Serialize>(state: &T) -> Result<String, AuthError> {
    serde_json::to_string(state).map_err(AuthError::WebAuthnEncoding)
}

pub(crate) fn parse_state<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, AuthError> {
    serde_json::from_str(json).map_err(|err| {
        tracing::error!(error = %err, "a stored WebAuthn ceremony state can't be read");
        AuthError::WebAuthnFailed
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_of_an_https_or_localhost_page_is_the_rp_id() {
        let party = RelyingParty::new("https://Files.Example.com").expect("https");
        assert_eq!(party.id(), "files.example.com");
        assert_eq!(party.origin(), "https://files.example.com");
        let local = RelyingParty::new("http://localhost:8080").expect("localhost");
        assert_eq!(local.id(), "localhost");
        assert_eq!(local.origin(), "http://localhost:8080");
        assert!(local.webauthn().is_ok());
    }

    #[test]
    fn plain_http_ip_addresses_and_paths_are_refused() {
        for origin in [
            "http://files.example.com",
            "https://192.168.1.4",
            "http://127.0.0.1:8080",
            "https://[::1]",
            "https://files.example.com/app",
            "https://user@files.example.com",
            "null",
            "",
        ] {
            assert!(
                matches!(
                    RelyingParty::new(origin),
                    Err(AuthError::WebAuthnUnavailable(_))
                ),
                "{origin}"
            );
        }
    }
}
