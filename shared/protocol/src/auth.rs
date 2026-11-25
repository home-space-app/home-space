use serde::{Deserialize, Serialize};

use crate::{DeviceInfo, NodeId, PublicKeyCredential, UserId, WebAuthnAnswer};

/// Credentials submitted to log in.
#[derive(Clone, Debug, Serialize, Deserialize)]
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

/// `POST /auth/login/verify`: the second step of a sign-in, with one of `code`,
/// `recovery_code` and `webauthn`.
#[derive(Clone, Debug, Serialize, Deserialize)]
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

/// `POST /me/totp`: a new authenticator app secret, saved only once a code from it is
/// confirmed with [`TotpConfirmRequest`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TotpEnrolment {
    /// The secret in base32, for typing into the app.
    pub secret: String,
    /// The `otpauth://` address, for the QR code.
    pub uri: String,
}

/// `POST /me/totp/confirm`: the first code from the new authenticator app.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TotpConfirmRequest {
    pub code: String,
}

/// A new set of recovery codes, each usable once instead of a second factor. They are shown
/// only now; a new set replaces the old one. Empty after confirming an authenticator app when
/// the account already had another second factor, and so its codes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryCodes {
    pub codes: Vec<String>,
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// `POST /me/reauthenticate`: a signed-in user confirms it is them before a sensitive change,
/// answering `reauthentication-required`, with one of the `methods` it named: `password`,
/// `totp` as `code`, or `webauthn` with the answer to `POST /me/reauthenticate/webauthn`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ReauthenticateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// The code the authenticator app shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webauthn: Option<WebAuthnAnswer>,
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
    /// Signs in through the identity provider rather than with a password; older servers leave
    /// it out.
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OidcCallbackQuery {
    /// Left out by some providers when they report an error.
    pub state: Option<String>,
    pub code: Option<String>,
    pub error: Option<String>,
}

/// Exchanges the one-time code from the redirect for a session.
#[derive(Clone, Debug, Serialize, Deserialize)]
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrowserLoginExchangeRequest {
    pub code: String,
    pub code_verifier: String,
    /// The device signing in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceInfo>,
}
