//! Authentication for home-space: password hashing, JWT access tokens and
//! rotating refresh tokens.

mod error;
mod oidc;
mod password;
mod service;
mod token;

pub use error::AuthError;
pub use oidc::{OidcConfig, OidcService};
pub use protocol::TokenPair;
pub use service::{Access, AuthConfig, AuthService, Grant, LOGIN_ATTEMPTS_MEMORY, device_name};
pub use token::Claims;
