use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    DeviceInfo, NodeId, PublicKeyCredential, UserId, WebAuthnAnswer,
    redact::{Redacted, redacted},
};

/// A way for the user to prove it is them: their password, a code from their authenticator app
/// (`totp`), a passkey or security key (`webauthn`), or one of their recovery codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    Password,
    Totp,
    Webauthn,
    Recovery,
}

impl AuthMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Totp => "totp",
            Self::Webauthn => "webauthn",
            Self::Recovery => "recovery",
        }
    }
}

/// The methods of a list this client knows, leaving out those a newer server may name.
pub(crate) fn known_methods<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<AuthMethod>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Named {
        Known(AuthMethod),
        Other(serde::de::IgnoredAny),
    }
    let named = Vec::<Named>::deserialize(deserializer)?;
    Ok(named
        .into_iter()
        .filter_map(|method| match method {
            Named::Known(method) => Some(method),
            Named::Other(_) => None,
        })
        .collect())
}

/// Credentials submitted to log in.
#[derive(Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// The device signing in; older clients leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceInfo>,
    /// Set by a browser: the server then keeps the refresh token in a cookie that scripts
    /// cannot read, and leaves it out of the response.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cookie: bool,
    /// Set by a client that can ask for a second factor: when the account needs one, the
    /// `second-factor-required` problem then carries a `login_token` for
    /// [`LoginVerifyRequest`]. Without it the problem only explains why the sign-in failed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub two_step: bool,
}

impl fmt::Debug for LoginRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginRequest")
            .field("username", &self.username)
            .field("password", &Redacted)
            .field("device", &self.device)
            .field("cookie", &self.cookie)
            .field("two_step", &self.two_step)
            .finish()
    }
}

/// `POST /auth/login/verify`: the second step of a sign-in, with one of `code`,
/// `recovery_code` and `webauthn`.
#[derive(Clone, Serialize, Deserialize)]
pub struct LoginVerifyRequest {
    /// From the `second-factor-required` problem of the first step.
    pub login_token: String,
    /// The code the authenticator app shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// One of the account's recovery codes, used up by this sign-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_code: Option<String>,
    /// The browser's answer to the challenge of `POST /auth/login/webauthn`, from a passkey or
    /// security key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webauthn: Option<PublicKeyCredential>,
    /// The device signing in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceInfo>,
    /// As in [`LoginRequest::cookie`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cookie: bool,
}

impl fmt::Debug for LoginVerifyRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginVerifyRequest")
            .field("login_token", &Redacted)
            .field("code", &redacted(self.code.as_ref()))
            .field("recovery_code", &redacted(self.recovery_code.as_ref()))
            .field("webauthn", &self.webauthn)
            .field("device", &self.device)
            .field("cookie", &self.cookie)
            .finish()
    }
}

/// `POST /me/totp`: a new authenticator app secret, saved only once a code from it is
/// confirmed with [`TotpConfirmRequest`].
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TotpEnrolment {
    /// The secret in base32, for typing into the app.
    pub secret: String,
    /// The `otpauth://` address, for the QR code.
    pub uri: String,
}

impl fmt::Debug for TotpEnrolment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TotpEnrolment")
            .field("secret", &Redacted)
            .field("uri", &Redacted)
            .finish()
    }
}

/// `POST /me/totp/confirm`: the first code from the new authenticator app.
#[derive(Clone, Serialize, Deserialize)]
pub struct TotpConfirmRequest {
    pub code: String,
}

impl fmt::Debug for TotpConfirmRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TotpConfirmRequest")
            .field("code", &Redacted)
            .finish()
    }
}

/// A new set of recovery codes, each usable once instead of a second factor. They are shown
/// only now; a new set replaces the old one. Empty after confirming an authenticator app when
/// the account already had another second factor, and so its codes.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryCodes {
    pub codes: Vec<String>,
}

impl fmt::Debug for RecoveryCodes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveryCodes")
            .field("codes", &vec![Redacted; self.codes.len()])
            .finish()
    }
}

/// The second factors of an account, in [`Me`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecondFactors {
    /// An authenticator app is set up, so signing in takes one of its codes.
    pub totp: bool,
    /// Passkeys and security keys; older servers leave it out.
    #[serde(default)]
    pub webauthn: u32,
    /// Recovery codes not used yet.
    pub recovery_codes_left: u32,
}

impl SecondFactors {
    /// Whether signing in takes a second factor.
    pub fn enabled(&self) -> bool {
        self.totp || self.webauthn > 0
    }
}

/// `POST /me/password`: a signed-in user replaces their own password.
#[derive(Clone, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

impl fmt::Debug for ChangePasswordRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChangePasswordRequest")
            .field("current_password", &Redacted)
            .field("new_password", &Redacted)
            .finish()
    }
}

/// `POST /me/reauthenticate`: a signed-in user confirms it is them before a sensitive change,
/// answering `reauthentication-required`, with one of the `methods` it named: `password`,
/// `totp` as `code`, or `webauthn` with the answer to `POST /me/reauthenticate/webauthn`.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ReauthenticateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// The code the authenticator app shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webauthn: Option<WebAuthnAnswer>,
}

impl fmt::Debug for ReauthenticateRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReauthenticateRequest")
            .field("password", &redacted(self.password.as_ref()))
            .field("code", &redacted(self.code.as_ref()))
            .field("webauthn", &self.webauthn)
            .finish()
    }
}

/// Refresh token submitted to rotate a session or log out.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RefreshRequest {
    /// Left out by a browser whose token is in the cookie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Set by a browser: the next refresh token goes into the cookie, not the response.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cookie: bool,
}

/// `POST /auth/device-sessions`: the app's session starts a second session on its device, for
/// one of the app's extensions, which keeps its own refresh token.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceSessionRequest {
    /// Which extension it is for: one of [`DEVICE_SESSION_LABELS`].
    pub label: String,
}

/// The labels of the extra sessions a device may hold.
pub const DEVICE_SESSION_LABELS: [&str; 2] = ["files", "share"];

/// Access and refresh token returned on login and refresh.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenPair {
    pub access_token: String,
    /// Empty when the server keeps it in a cookie instead (see [`LoginRequest::cookie`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub refresh_token: String,
    pub expires_in: i64,
}

/// The authenticated user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Me {
    pub id: UserId,
    pub username: String,
    /// The Lost & Found folder, which cannot be deleted.
    pub lost_and_found_id: NodeId,
    /// May open the administration pages; older servers leave it out.
    #[serde(default)]
    pub is_admin: bool,
    /// Linked to the identity provider, which signs it in and keeps its second factors and
    /// passkeys: it has no password, authenticator app, recovery codes or sign-in keys here
    /// (`409 external-account`), only folder keys. Older servers leave it out.
    #[serde(default)]
    pub external: bool,
    /// The account's second factors; older servers leave it out.
    #[serde(default)]
    pub second_factors: SecondFactors,
}

/// How this server lets users sign in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Providers {
    /// Username and password login is enabled.
    pub password: bool,
    /// An OpenID Connect provider (Auth0, Keycloak, ...) is configured.
    pub oidc: Option<OidcProvider>,
    /// Apps can sign in through the server's web page ([`BrowserLoginStartQuery`]); older
    /// servers leave it out.
    #[serde(default)]
    pub browser_login: bool,
    /// Users can sign in with a passkey ([`crate::PasskeyFinish`]) on this address; older
    /// servers leave it out.
    #[serde(default)]
    pub passkeys: bool,
}

impl Providers {
    /// The identity provider the tenant has, leaving out the stand-in for older apps (see
    /// [`OidcProvider::browser`]).
    pub fn identity_provider(&self) -> Option<&OidcProvider> {
        self.oidc.as_ref().filter(|oidc| !oidc.browser)
    }
}

/// A configured OpenID Connect provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OidcProvider {
    /// Name shown on the sign-in button.
    pub name: String,
    /// Not a provider but the server's own web page, announced where there is no provider so
    /// that apps from before [`Providers::browser_login`] get a button that signs in there:
    /// `/auth/oidc/start` and `/auth/oidc/exchange` then run the browser sign-in. Clients that
    /// know `browser_login` leave it out. Older servers leave the field out.
    #[serde(default)]
    pub browser: bool,
}

/// Query of `GET /auth/oidc/start`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OidcStartQuery {
    /// Where to send the user after sign-in: the web client's callback page, a loopback URL or
    /// [`MOBILE_OIDC_REDIRECT`](crate::paths::MOBILE_OIDC_REDIRECT).
    pub redirect_uri: String,
    /// Base64url (unpadded) SHA-256 of the `code_verifier` presented later (PKCE `S256`).
    pub code_challenge: String,
}

/// Query the provider sends to `GET /auth/oidc/callback`.
#[derive(Clone, Serialize, Deserialize)]
pub struct OidcCallbackQuery {
    /// Left out by some providers when they report an error.
    pub state: Option<String>,
    pub code: Option<String>,
    pub error: Option<String>,
}

impl fmt::Debug for OidcCallbackQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OidcCallbackQuery")
            .field("state", &self.state)
            .field("code", &redacted(self.code.as_ref()))
            .field("error", &self.error)
            .finish()
    }
}

/// Exchanges the one-time code from the redirect for a session.
#[derive(Clone, Serialize, Deserialize)]
pub struct OidcExchangeRequest {
    pub code: String,
    pub code_verifier: String,
    /// The device signing in; older clients leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceInfo>,
    /// Set by a browser: the server then keeps the refresh token in a cookie that scripts
    /// cannot read, and leaves it out of the response.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cookie: bool,
}

impl fmt::Debug for OidcExchangeRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OidcExchangeRequest")
            .field("code", &Redacted)
            .field("code_verifier", &Redacted)
            .field("device", &self.device)
            .field("cookie", &self.cookie)
            .finish()
    }
}

/// Query of `GET /auth/browser/start`, which an app opens in the system browser to sign in
/// through the server's web page.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrowserLoginStartQuery {
    /// Where the browser goes with the one-time code: a loopback URL or
    /// [`MOBILE_OIDC_REDIRECT`](crate::paths::MOBILE_OIDC_REDIRECT).
    pub redirect_uri: String,
    /// Base64url (unpadded) SHA-256 of the `code_verifier` presented later (PKCE `S256`).
    pub code_challenge: String,
    /// The device's name, shown to the user before they let the app sign in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
}

/// What kind of app asks to sign in, told by its redirect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserLoginApp {
    Desktop,
    Mobile,
}

/// `GET /auth/browser/flows/{flow}`: the app waiting for the signed-in user to let it in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserLoginFlow {
    pub app: BrowserLoginApp,
    /// The name the app gave for its device, if any. The app chose it, so it is no proof.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
}

/// `POST /auth/browser/approve` or `/deny`: the signed-in user's answer to a browser sign-in.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrowserLoginDecision {
    pub flow: String,
}

/// Where the web client sends the browser after the answer: the app's redirect with a `code`
/// or an `error`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserLoginRedirect {
    pub redirect: String,
}

/// `POST /auth/browser/exchange`: the app trades the one-time code for a session. Apps only,
/// so the refresh token is always in the response.
#[derive(Clone, Serialize, Deserialize)]
pub struct BrowserLoginExchangeRequest {
    pub code: String,
    pub code_verifier: String,
    /// The device signing in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceInfo>,
}

impl fmt::Debug for BrowserLoginExchangeRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrowserLoginExchangeRequest")
            .field("code", &Redacted)
            .field("code_verifier", &Redacted)
            .field("device", &self.device)
            .finish()
    }
}
