//! Authentication for home-space: password hashing, JWT access tokens and
//! rotating refresh tokens.

mod error;
mod hasher;
pub mod offline;
mod oidc;
mod oidc_login;
mod password;
pub mod secrets;
mod service;
mod sessions;
mod share_passwords;
mod throttle;
mod tickets;
mod token;
mod urls;

pub use error::AuthError;
pub use oidc::{OidcConfig, OidcService};
pub use protocol::TokenPair;
pub use service::{Access, AuthConfig, AuthService, Grant};
pub use share_passwords::SharePasswords;
pub use throttle::LOGIN_ATTEMPTS_MEMORY;
pub use token::{Claims, new_share_token};
pub use urls::is_https_or_loopback;
