//! Authentication for home-space: password hashing, JWT access tokens and
//! rotating refresh tokens.

mod browser_login;
mod error;
mod hasher;
mod login_codes;
pub mod offline;
mod oidc;
mod oidc_login;
mod password;
mod recovery_codes;
mod secret_box;
pub mod secrets;
pub mod secrets_key;
mod service;
mod sessions;
mod share_passwords;
mod throttle;
mod tickets;
mod token;
mod totp;
mod urls;
mod webauthn;

pub use error::AuthError;
pub use oidc::{OidcConfig, OidcService};
pub use protocol::TokenPair;
pub use secret_box::SecretBox;
pub use service::{Access, AuthConfig, AuthService, Grant, ReauthProof, SecondFactorProof};
pub use share_passwords::SharePasswords;
pub use throttle::LOGIN_ATTEMPTS_MEMORY;
pub use token::{Claims, new_share_token};
pub use urls::is_https_or_loopback;
pub use webauthn::RelyingParty;
