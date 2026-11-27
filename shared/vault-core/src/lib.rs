//! Client-side cryptography of protected folders (`docs/todo/protected-folders.md`).
//!
//! No I/O and no UI: the desktop, web and mobile clients fetch the wrapped keys from the server,
//! ask an [`Authenticator`] (a security key) or the user (a password or a recovery key) for the
//! wrapping key, and call into this crate to unwrap, seal names and encrypt content. Folders of
//! both locks are end to end: the server never has a key.
//!
//! The vault key lives only inside one call ([`vault::with_vault_key`]). A folder's
//! [`FolderKeys`] are what an unlocked folder is: the client holds them until it locks the
//! folder. Every key type wipes itself on drop and implements neither `Clone` nor a `Debug`
//! that shows its bytes.

#![warn(missing_docs)]

pub mod authenticator;
pub mod content;
mod error;
pub mod folder;
pub mod kdf;
pub mod names;
pub mod password;
pub mod recovery;
mod secret;
pub mod vault;
#[cfg(test)]
mod vectors;
pub mod wrap;

/// The public key a protected folder's files are encrypted to.
pub use age::x25519::Recipient;
#[cfg(any(test, feature = "testing"))]
pub use authenticator::SoftAuthenticator;
pub use authenticator::{Authenticator, CredentialRef, PrfOutput};
pub use error::{ContentError, Error, Internal, NameProblem};
pub use folder::{FolderKeys, FolderSecret};
pub use kdf::Kek;
pub use names::NodeKind;
pub use password::{PasswordKdf, PasswordUnlock};
pub use recovery::RecoveryKey;
pub use vault::VaultKey;

/// This crate's result, with [`Error`] by default.
pub type Result<T, E = Error> = std::result::Result<T, E>;
