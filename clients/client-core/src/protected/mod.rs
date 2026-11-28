//! Protected folders on the client (`docs/todo/protected-folders.md`): which folder a node is
//! in, the keys of the folders the user unlocked, sealed names in listings, and the password
//! lock's requests. The cryptography is `vault-core`'s; this module only joins it to the API.

mod content;
mod names;
mod password;
mod security_key;
mod unlocked;

use protocol::{NodeId, ProtectedFolder};

pub use content::{ENCRYPTED_MIME, shown_mime};
pub use names::{UNREADABLE_NAME, open_listing, open_name, open_path, seal_name};
pub use password::{
    NewFolderPassword, NewPasswordFolder, PasswordProof, change_folder_password,
    protect_with_password, unlock_password_folder,
};
pub use security_key::{
    FolderKey, Rotation, RotationProof, VaultProof, enable_folder_key, folder_keys,
    new_vault_recovery_key, open_with_vault, protect_with_vault, recovery_unlocker, rotate_vault,
    with_vault,
};
pub use unlocked::{AUTO_LOCK_AFTER, UnlockedFolders};

/// What can go wrong with a protected folder on the client. The texts are for the user.
#[derive(Debug, thiserror::Error)]
pub enum ProtectedError {
    /// The password does not open the folder.
    #[error("Wrong password.")]
    WrongPassword,
    /// The recovery key does not open the folder, or was mistyped.
    #[error("This recovery key does not open the folder.")]
    WrongRecoveryKey,
    /// A password action on a security-key folder, or a folder listed without its password
    /// parameters.
    #[error("This folder is not protected with a password.")]
    NotPassword,
    /// A security-key action on a password folder.
    #[error("This folder is not protected with a security key.")]
    NotSecurityKey,
    /// The security key answered, but what it gave does not open the vault.
    #[error("This security key does not open your folders.")]
    KeyDoesNotOpen,
    /// The vault has no recovery key to open it with.
    #[error("There is no recovery key for your folder keys.")]
    NoRecoveryKey,
    /// The folder's secret is wrapped under another vault key than the one listed.
    #[error("Your folder keys changed meanwhile. Try again.")]
    VaultChanged,
    /// The security key could not be asked; the text is the platform's, for the user.
    #[error("{0}")]
    SecurityKey(String),
    /// The server lists password parameters this app would not have made.
    #[error("The server lists password settings for this folder that this app does not accept.")]
    UnacceptedParameters,
    /// The folder's public key on the server is not the one its secret derives: someone changed
    /// it, so nothing is encrypted to it.
    #[error(
        "The server lists a different key for this folder than its password opens. Nothing was uploaded to it."
    )]
    RecipientMismatch,
    /// A name too long, or empty, to seal.
    #[error("{0}")]
    Name(String),
    /// Anything else `vault-core` refused.
    #[error("The folder could not be opened: {0}")]
    Vault(vault_core::Error),
}

impl From<vault_core::Error> for ProtectedError {
    fn from(error: vault_core::Error) -> Self {
        match error {
            vault_core::Error::PasswordParams => Self::UnacceptedParameters,
            vault_core::Error::RecipientMismatch => Self::RecipientMismatch,
            vault_core::Error::Name(vault_core::NameProblem::TooLong) => Self::Name(format!(
                "The name is too long: at most {} characters.",
                vault_core::names::MAX_NAME_CHARS
            )),
            vault_core::Error::Name(vault_core::NameProblem::Empty) => {
                Self::Name("The name is empty.".to_owned())
            }
            vault_core::Error::Authenticator(source) => Self::SecurityKey(source.to_string()),
            other => Self::Vault(other),
        }
    }
}

/// The protected folder that holds `path`: a folder followed by its ancestors, in any order.
/// Protected folders never overlap, so there is at most one.
pub fn enclosing(
    folders: &[ProtectedFolder],
    path: impl IntoIterator<Item = NodeId>,
) -> Option<&ProtectedFolder> {
    path.into_iter()
        .find_map(|id| folders.iter().find(|folder| folder.node_id == id))
}

#[cfg(test)]
mod tests;
