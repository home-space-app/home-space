//! Protected folders and the vault that unlocks them (feature `protected_folders`).
//!
//! A folder locked with a security key is end to end: the server keeps only what is useless
//! without the key (wrapped keys, PRF salts, the folders' public age recipients) and clients do
//! all the cryptography (`shared/vault-core`). A folder locked with a password is encrypted by
//! the server, which keeps its key. See `docs/todo/protected-folders.md`.

use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    NodeId,
    encoding::Base64Url,
    redact::{Redacted, redacted},
};

/// Length of a wrapped vault key or folder secret: version, nonce, key and tag.
pub const WRAPPED_LEN: usize = 73;
/// Length of the PRF input of a security key unlocker.
pub const PRF_SALT_LEN: usize = 32;
/// The longest WebAuthn credential id, in bytes, as the WebAuthn specification allows.
pub const MAX_CREDENTIAL_ID_LEN: usize = 1023;

/// What opens the vault.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    feature = "server",
    derive(sqlx::Type),
    sqlx(type_name = "text", rename_all = "snake_case")
)]
pub enum UnlockerKind {
    /// A folder key: a WebAuthn credential with PRF that a client made on a security key for
    /// the vault alone. It never signs in, even on a key that also holds a sign-in credential.
    SecurityKey,
    /// The printed recovery key; at most one.
    Recovery,
}

impl UnlockerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SecurityKey => "security_key",
            Self::Recovery => "recovery",
        }
    }
}

/// One unlocker of the vault, as `GET /vault` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultUnlocker {
    pub id: Uuid,
    pub kind: UnlockerKind,
    /// Security keys only: what the user calls the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Security keys only: the credential id, which a client asks the key for, with `rp_id`
    /// and `prf_salt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<Base64Url>,
    /// Security keys only: the RP id the credential was made for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rp_id: Option<String>,
    /// Security keys only: the PRF input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prf_salt: Option<Base64Url>,
    pub wrapped_vault_key: Base64Url,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// `GET /vault`: the vault of the caller, which exists once they enabled a key or a recovery
/// key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vault {
    /// Bumped by every rotation; what new unlockers and folder secrets must be wrapped under.
    pub epoch: i32,
    pub unlockers: Vec<VaultUnlocker>,
}

/// `POST /vault/unlockers`: enables a folder key or sets the recovery key. The first creates
/// the vault, at epoch 1; a recovery key replaces the one before.
///
/// A folder key needs `name`, `credential_id`, `rp_id` and `prf_salt`; a recovery key none
/// of them. The server cannot check the credential: the client made it and ran the PRF
/// assertion with challenges of its own.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddUnlocker {
    pub kind: UnlockerKind,
    /// What the user calls the key, at most [`MAX_KEY_NAME_LEN`](crate::MAX_KEY_NAME_LEN)
    /// characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The new credential's id, at most [`MAX_CREDENTIAL_ID_LEN`] bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<Base64Url>,
    /// The RP id the credential was made for: a host this server answers on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rp_id: Option<String>,
    /// The PRF input the key answered for the wrapping key, [`PRF_SALT_LEN`] bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prf_salt: Option<Base64Url>,
    pub wrapped_vault_key: Base64Url,
    /// The vault epoch the key is wrapped under: the vault's current one, or 1 for a new vault.
    pub epoch: i32,
}

/// An unlocker with the new vault key, in a [`RotateVault`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RewrappedUnlocker {
    pub id: Uuid,
    /// A new PRF input; the old one stays when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prf_salt: Option<Base64Url>,
    pub wrapped_vault_key: Base64Url,
}

/// A folder secret wrapped with a new vault key, in a [`RotateVault`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RewrappedFolder {
    pub node_id: NodeId,
    pub wrapped_secret: Base64Url,
}

/// `POST /vault/rotate`: replaces the vault key. Every unlocker that stays and every folder
/// locked with a security key must be listed, all wrapped with the new key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RotateVault {
    /// The epoch being replaced, which must still be the vault's.
    pub epoch: i32,
    pub unlockers: Vec<RewrappedUnlocker>,
    #[serde(default)]
    pub folders: Vec<RewrappedFolder>,
    /// Unlockers to remove in the same step, such as a lost key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<Uuid>,
}

/// What a protected folder is locked with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    feature = "server",
    derive(sqlx::Type),
    sqlx(type_name = "text", rename_all = "snake_case")
)]
pub enum FolderLock {
    /// End to end: the client encrypts to the folder's recipient, and any of the user's
    /// folder keys opens it through the vault.
    SecurityKey,
    /// The server encrypts with a key it keeps and opens it to whoever gives the folder's
    /// password. Not end to end.
    Password,
}

impl FolderLock {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SecurityKey => "security_key",
            Self::Password => "password",
        }
    }
}

/// Where a protected folder stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    feature = "server",
    derive(sqlx::Type),
    sqlx(type_name = "text", rename_all = "snake_case")
)]
pub enum ProtectionState {
    /// The existing content is being replaced with its encrypted form: by the client, which
    /// downloaded it, removed it from the server and uploads it encrypted (security key), or
    /// by the server (password).
    Protecting,
    Protected,
    /// The content is being replaced with its decrypted form.
    Unprotecting,
}

impl ProtectionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Protecting => "protecting",
            Self::Protected => "protected",
            Self::Unprotecting => "unprotecting",
        }
    }
}

/// A protected folder, from `GET /protected`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedFolder {
    pub node_id: NodeId,
    pub lock: FolderLock,
    /// Security keys: the folder secret, wrapped with the vault key of `epoch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapped_secret: Option<Base64Url>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<i32>,
    /// The age recipient content is encrypted to. For a security key, clients check it against
    /// the one their key derives before they encrypt anything to it, so a server cannot swap
    /// in its own.
    pub recipient: String,
    pub format: i32,
    pub state: ProtectionState,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Only in the answer to protecting with a password: the folder's recovery key, shown once.
    /// It sets a new password when the old one is forgotten; the server keeps only its hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_key: Option<String>,
}

impl fmt::Debug for ProtectedFolder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProtectedFolder")
            .field("node_id", &self.node_id)
            .field("lock", &self.lock)
            .field("wrapped_secret", &self.wrapped_secret)
            .field("epoch", &self.epoch)
            .field("recipient", &self.recipient)
            .field("format", &self.format)
            .field("state", &self.state)
            .field("created_at", &self.created_at)
            .field("recovery_key", &redacted(self.recovery_key.as_ref()))
            .finish()
    }
}

/// `POST /protected`: protects one of the caller's folders. A security key needs
/// `wrapped_secret`, `recipient` and the vault's `epoch`; a password needs `password`. An empty
/// folder is protected at once. One with content stays `protecting` until it is replaced with
/// its encrypted form: by the client, which then calls `POST /protected/{id}/finish` (security
/// key), or by the server (password).
#[derive(Clone, Serialize, Deserialize)]
pub struct ProtectFolder {
    pub node_id: NodeId,
    pub lock: FolderLock,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapped_secret: Option<Base64Url>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<i32>,
    /// The folder's password, for a password lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

impl fmt::Debug for ProtectFolder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProtectFolder")
            .field("node_id", &self.node_id)
            .field("lock", &self.lock)
            .field("wrapped_secret", &self.wrapped_secret)
            .field("recipient", &self.recipient)
            .field("epoch", &self.epoch)
            .field("password", &redacted(self.password.as_ref()))
            .finish()
    }
}

/// `POST /protected/{id}/unlock`: the password of a password folder.
#[derive(Clone, Serialize, Deserialize)]
pub struct UnlockFolder {
    pub password: String,
}

impl fmt::Debug for UnlockFolder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnlockFolder")
            .field("password", &Redacted)
            .finish()
    }
}

/// `POST /protected/{id}/password`: a new password for a password folder, with exactly one
/// proof: the current password, or the folder's recovery key when it is forgotten. Signing in to
/// the account is not enough. The folder's key stays; other sessions are locked out.
#[derive(Clone, Serialize, Deserialize)]
pub struct ChangeFolderPassword {
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_key: Option<String>,
}

/// The answer to `POST /protected/{id}/password`.
#[derive(Clone, Serialize, Deserialize)]
pub struct FolderPasswordChanged {
    /// A new recovery key, shown once, when the old one was used: it was typed in, so it is
    /// replaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_key: Option<String>,
}

impl fmt::Debug for FolderPasswordChanged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FolderPasswordChanged")
            .field("recovery_key", &redacted(self.recovery_key.as_ref()))
            .finish()
    }
}

impl fmt::Debug for ChangeFolderPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChangeFolderPassword")
            .field("password", &Redacted)
            .field(
                "current_password",
                &redacted(self.current_password.as_ref()),
            )
            .field("recovery_key", &redacted(self.recovery_key.as_ref()))
            .finish()
    }
}

/// The answer to `POST /protected/{id}/unlock`: the session can use the folder until
/// `expires_at`, which moves on while it does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderUnlocked {
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

/// Whether `recipient` looks like an age X25519 recipient: `age1` and 58 bech32 characters.
/// The server cannot check more; clients check it against their key.
pub fn is_age_recipient(recipient: &str) -> bool {
    const CHARSET: &str = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";
    recipient.len() == 62
        && recipient
            .strip_prefix("age1")
            .is_some_and(|data| data.chars().all(|c| CHARSET.contains(c)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipients_are_checked_for_shape() {
        assert!(is_age_recipient(
            "age1fauk43xvke3myu0vg0syaflnwhlslwxd2504jfdjwejkjc50pcysgcf5gf"
        ));
        assert!(!is_age_recipient(
            "age1fauk43xvke3myu0vg0syaflnwhlslwxd2504jfdjwejkjc50pcysgcf5gb"
        ));
        assert!(!is_age_recipient("age1short"));
        assert!(!is_age_recipient(
            "AGE1FAUK43XVKE3MYU0VG0SYAFLNWHLSLWXD2504JFDJWEJKJC50PCYSGCF5GF"
        ));
    }
}
