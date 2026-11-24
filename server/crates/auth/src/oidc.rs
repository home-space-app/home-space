//! OpenID Connect relying party: sends users to an identity provider (Auth0, Keycloak, ...)
//! and verifies who comes back. Sessions are still issued by [`crate::AuthService`].

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use openidconnect::{
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet,
    EndpointNotSet, EndpointSet, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenResponse,
    core::{CoreClient, CoreProviderMetadata, CoreResponseType},
    reqwest,
    url::Url,
};
use protocol::{BrowserLoginApp, paths};
use sha2::{Digest, Sha256};

use crate::AuthError;

const CALLBACK_PATH: &str = "/api/v1/auth/oidc/callback";
const WEB_REDIRECT_PATH: &str = "/login/callback";
const LOOPBACK_REDIRECT_PATH: &str = "/callback";
const CHALLENGE_LEN: usize = 43;
const MIN_VERIFIER_LEN: usize = 43;
const MAX_VERIFIER_LEN: usize = 128;

type Client = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// Settings for the identity provider.
#[derive(Clone, Debug)]
pub struct OidcConfig {
    /// Issuer URL; `/.well-known/openid-configuration` is read from it.
    pub issuer: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    /// Externally reachable origin of this server, e.g. `https://files.example.com`.
    pub public_url: String,
    /// When set, only members of this group may sign in.
    pub required_group: Option<String>,
    /// Name of the ID token claim that lists the user's groups.
    pub groups_claim: String,
    /// Name shown on the sign-in button.
    pub display_name: String,
}

/// Who the provider says has signed in.
#[derive(Clone, Debug)]
pub struct Identity {
    pub issuer: String,
    pub subject: String,
    pub username_hint: Option<String>,
    pub email: Option<String>,
}

/// A request to the provider's authorization endpoint.
#[derive(Clone, Debug)]
pub struct Authorization {
    pub url: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

/// Talks to one OpenID Connect provider.
#[derive(Clone)]
pub struct OidcService {
    client: Client,
    http: reqwest::Client,
    config: OidcConfig,
}

impl std::fmt::Debug for OidcService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcService")
            .field("issuer", &self.config.issuer)
            .finish_non_exhaustive()
    }
}

impl OidcService {
    /// Reads the provider's discovery document; fails if it is unreachable or invalid.
    pub async fn discover(config: OidcConfig) -> Result<Self, AuthError> {
        let http = reqwest::ClientBuilder::new()
            // Following redirects would open the discovery and token requests to SSRF.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(oidc_error)?;
        let issuer = IssuerUrl::new(config.issuer.clone()).map_err(oidc_error)?;
        let metadata = CoreProviderMetadata::discover_async(issuer, &http)
            .await
            .map_err(oidc_error)?;
        let redirect = RedirectUrl::new(callback_url(&config.public_url)).map_err(oidc_error)?;
        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(config.client_id.clone()),
            config.client_secret.clone().map(ClientSecret::new),
        )
        .set_redirect_uri(redirect);
        Ok(Self {
            client,
            http,
            config,
        })
    }

    pub fn display_name(&self) -> &str {
        &self.config.display_name
    }

    pub fn issuer(&self) -> &str {
        &self.config.issuer
    }

    /// Whether `redirect` may receive the login code. See [`is_allowed_redirect`].
    pub fn allows_redirect(&self, redirect: &str) -> bool {
        is_allowed_redirect(&self.config.public_url, redirect)
    }

    /// This server's own web client callback, where a browser goes when the sign-in it came
    /// back from is no longer known.
    pub fn web_redirect(&self) -> String {
        web_redirect_url(&self.config.public_url)
    }

    /// Builds the URL that sends the user to the provider, tagged with `state`.
    pub fn authorize(&self, state: &str) -> Authorization {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let state = state.to_owned();
        let mut request = self
            .client
            .authorize_url(
                AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
                move || CsrfToken::new(state),
                Nonce::new_random,
            )
            .set_pkce_challenge(challenge);
        // The library always requests `openid`; asking twice is sloppy and some providers object.
        for scope in self.config.scopes.iter().filter(|scope| *scope != "openid") {
            request = request.add_scope(Scope::new(scope.clone()));
        }
        let (url, _state, nonce) = request.url();
        Authorization {
            url: url.to_string(),
            nonce: nonce.secret().clone(),
            pkce_verifier: verifier.secret().clone(),
        }
    }

    /// Redeems the authorization `code` and verifies the ID token (signature, issuer,
    /// audience, expiry and `nonce`), then applies the group restriction.
    pub async fn complete(
        &self,
        code: &str,
        nonce: &str,
        pkce_verifier: &str,
    ) -> Result<Identity, AuthError> {
        let response = self
            .client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .map_err(oidc_error)?
            .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.to_owned()))
            .request_async(&self.http)
            .await
            .map_err(oidc_error)?;
        let id_token = response
            .id_token()
            .ok_or_else(|| AuthError::Oidc("the provider returned no ID token".to_owned()))?;
        let claims = id_token
            .claims(
                &self.client.id_token_verifier(),
                &Nonce::new(nonce.to_owned()),
            )
            .map_err(oidc_error)?;
        // The claims were verified above, so the raw payload can be read for the groups, which
        // are a provider specific claim.
        if let Some(required) = &self.config.required_group {
            let groups = claim_values(&id_token.to_string(), &self.config.groups_claim);
            if !groups.iter().any(|group| same_group(group, required)) {
                return Err(AuthError::OidcDenied);
            }
        }
        Ok(Identity {
            issuer: self.config.issuer.clone(),
            subject: claims.subject().to_string(),
            username_hint: claims.preferred_username().map(|name| name.to_string()),
            email: claims.email().map(|email| email.to_string()),
        })
    }
}

fn oidc_error(err: impl std::error::Error) -> AuthError {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    AuthError::Oidc(message)
}

fn callback_url(public_url: &str) -> String {
    format!("{}{CALLBACK_PATH}", public_url.trim_end_matches('/'))
}

fn web_redirect_url(public_url: &str) -> String {
    format!("{}{WEB_REDIRECT_PATH}", public_url.trim_end_matches('/'))
}

/// Whether a client may be sent the login code at `redirect`: this server's own web client, the
/// mobile apps' fixed address, or a loopback address for desktop apps (RFC 8252). Anything else
/// would let a crafted link send the code to an attacker. Another app on a phone could claim the
/// mobile address, as it could a loopback port on a computer, but the code is useless to it
/// without the PKCE verifier.
pub fn is_allowed_redirect(public_url: &str, redirect: &str) -> bool {
    redirect == web_redirect_url(public_url) || native_app(redirect).is_some()
}

/// The kind of app `redirect` belongs to, when it is an app's address: the mobile apps' fixed
/// address or a desktop app's loopback address (RFC 8252). `None` for anything else, the web
/// client's own address included.
pub(crate) fn native_app(redirect: &str) -> Option<BrowserLoginApp> {
    if redirect == paths::MOBILE_OIDC_REDIRECT {
        return Some(BrowserLoginApp::Mobile);
    }
    let url = Url::parse(redirect).ok()?;
    let loopback = url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1" | "[::1]"))
        && url.path() == LOOPBACK_REDIRECT_PATH
        && url.query().is_none()
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none();
    loopback.then_some(BrowserLoginApp::Desktop)
}

/// Whether `challenge` looks like an unpadded base64url SHA-256 digest.
pub fn is_valid_challenge(challenge: &str) -> bool {
    challenge.len() == CHALLENGE_LEN && challenge.bytes().all(is_base64url)
}

/// Whether `verifier` matches the S256 `challenge` (RFC 7636).
pub fn verifier_matches(verifier: &str, challenge: &str) -> bool {
    (MIN_VERIFIER_LEN..=MAX_VERIFIER_LEN).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|byte| is_base64url(byte) || matches!(byte, b'.' | b'~'))
        && URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) == challenge
}

fn is_base64url(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

/// String values of `claim` in a JWT's payload; the claim may be one string or a list.
fn claim_values(jwt: &str, claim: &str) -> Vec<String> {
    let payload = jwt
        .split('.')
        .nth(1)
        .and_then(|part| URL_SAFE_NO_PAD.decode(part).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    match payload.as_ref().and_then(|payload| payload.get(claim)) {
        Some(serde_json::Value::String(value)) => vec![value.clone()],
        Some(serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// Keycloak reports group paths such as `/family`; ignore the leading slash.
fn same_group(actual: &str, required: &str) -> bool {
    actual.trim_start_matches('/') == required.trim_start_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC: &str = "https://files.example.com";

    #[test]
    fn allows_the_web_callback_and_loopback_callbacks() {
        assert!(is_allowed_redirect(
            PUBLIC,
            "https://files.example.com/login/callback"
        ));
        assert!(is_allowed_redirect(
            PUBLIC,
            "http://127.0.0.1:41234/callback"
        ));
        assert!(is_allowed_redirect(PUBLIC, "http://[::1]:41234/callback"));
    }

    #[test]
    fn allows_the_mobile_redirect() {
        assert!(is_allowed_redirect(PUBLIC, "app.homespace://oidc/callback"));
    }

    #[test]
    fn a_trailing_slash_on_the_public_url_is_ignored() {
        assert!(is_allowed_redirect(
            "https://files.example.com/",
            "https://files.example.com/login/callback"
        ));
    }

    #[test]
    fn rejects_other_redirects() {
        for redirect in [
            "https://evil.example.com/login/callback",
            "https://files.example.com/other",
            "https://files.example.com/login/callback?x=1",
            "https://files.example.com.evil.com/login/callback",
            "http://127.0.0.1:1/other",
            "http://127.0.0.1:1/callback?x=1",
            "http://127.0.0.1:1/callback#frag",
            "http://user@127.0.0.1:1/callback",
            "http://localhost:1/callback",
            "http://127.0.0.1.evil.com/callback",
            "https://127.0.0.1:1/callback",
            "app.homespace://oidc/other",
            "app.homespace://oidc/callback?x=1",
            "app.homespace://oidc/callback#frag",
            "app.homespace://evil/callback",
            "app.homespace.evil://oidc/callback",
            "javascript:alert(1)",
            "not a url",
            "",
        ] {
            assert!(!is_allowed_redirect(PUBLIC, redirect), "{redirect}");
        }
    }

    #[test]
    fn only_app_redirects_are_native() {
        assert_eq!(
            native_app("app.homespace://oidc/callback"),
            Some(BrowserLoginApp::Mobile)
        );
        assert_eq!(
            native_app("http://127.0.0.1:41234/callback"),
            Some(BrowserLoginApp::Desktop)
        );
        assert_eq!(
            native_app("http://[::1]:41234/callback"),
            Some(BrowserLoginApp::Desktop)
        );
        assert_eq!(native_app("https://files.example.com/login/callback"), None);
        assert_eq!(native_app("http://localhost:41234/callback"), None);
        assert_eq!(native_app("http://127.0.0.1:41234/callback?x=1"), None);
    }

    #[test]
    fn validates_challenges() {
        let verifier = "a".repeat(43);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert!(is_valid_challenge(&challenge));
        assert!(!is_valid_challenge("short"));
        assert!(!is_valid_challenge(&"+".repeat(43)));
    }

    #[test]
    fn verifier_must_match_the_challenge() {
        let verifier = "b".repeat(50);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert!(verifier_matches(&verifier, &challenge));
        assert!(!verifier_matches(&"c".repeat(50), &challenge));
        // Too short to be a valid verifier, even though it hashes to its own challenge.
        let short = "d".repeat(10);
        let short_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(short.as_bytes()));
        assert!(!verifier_matches(&short, &short_challenge));
    }

    fn jwt_with(payload: &serde_json::Value) -> String {
        let body = URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("e30.{body}.sig")
    }

    #[test]
    fn reads_group_lists_and_single_values() {
        let list = jwt_with(&serde_json::json!({ "groups": ["a", "/b"] }));
        assert_eq!(claim_values(&list, "groups"), ["a", "/b"]);
        let single = jwt_with(&serde_json::json!({ "groups": "a" }));
        assert_eq!(claim_values(&single, "groups"), ["a"]);
        assert!(claim_values(&single, "missing").is_empty());
        assert!(claim_values("garbage", "groups").is_empty());
    }

    #[test]
    fn groups_match_with_or_without_a_leading_slash() {
        assert!(same_group("/family", "family"));
        assert!(same_group("family", "/family"));
        assert!(!same_group("family-extra", "family"));
    }
}
