use serde::{Deserialize, Serialize};

use crate::{DeviceInfo, NodeId, UserId};

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
}

/// `POST /me/password`: a signed-in user replaces their own password.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
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
}

/// How this server lets users sign in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Providers {
    /// Username and password login is enabled.
    pub password: bool,
    /// An OpenID Connect provider (Auth0, Keycloak, ...) is configured.
    pub oidc: Option<OidcProvider>,
}

/// A configured OpenID Connect provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OidcProvider {
    /// Name shown on the sign-in button.
    pub name: String,
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
