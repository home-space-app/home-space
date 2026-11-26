//! Client-side cryptography of protected folders (`docs/todo/protected-folders.md`).
//!
//! No I/O and no UI: the desktop, web and mobile clients fetch the wrapped keys from the server,
//! ask an [`Authenticator`] (a security key) or the user (the recovery key) for the wrapping key,
//! and call into this crate to unwrap, seal names and encrypt content. This is for folders locked
//! with a security key, which are end to end; folders locked with a password are encrypted by
//! the server.
//!
//! Keys are short-lived by design. For security-key folders the client keeps no decryption key
//! between actions, so the API hands keys to closures ([`vault::with_vault_key`],
//! [`vault::with_folder_keys`]) instead of returning them. Every key type wipes itself on drop
//! and implements neither `Clone` nor a `Debug` that shows its bytes.

pub mod authenticator;
pub mod content;
mod error;
pub mod folder;
pub mod kdf;
pub mod names;
pub mod recovery;
mod secret;
pub mod vault;
#[cfg(test)]
mod vectors;
pub mod wrap;

pub use authenticator::{Authenticator, CredentialRef, PrfOutput, SoftAuthenticator};
pub use error::Error;
pub use folder::{FolderKeys, FolderSecret};
pub use kdf::Kek;
pub use names::NodeKind;
pub use recovery::RecoveryKey;
pub use vault::VaultKey;

pub type Result<T, E = Error> = std::result::Result<T, E>;
