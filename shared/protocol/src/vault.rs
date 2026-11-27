//! Protected folders and the vault that unlocks them (feature `protected_folders`).
//!
//! Protected folders are end to end, whatever their lock: the server keeps only what is useless
//! without the user's security key or the folder's password (wrapped keys, PRF salts, Argon2id
//! parameters, the folders' public age recipients) and clients do all the cryptography
//! (`shared/vault-core`). See `docs/todo/protected-folders.md`.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{NodeId, encoding::Base64Url};

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
    /// Any of the user's folder keys opens it through the vault.
    SecurityKey,
    /// The folder's password opens it, or its recovery key.
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
    /// The existing content is being replaced with its encrypted form by the client, which
    /// downloaded it, removed it from the server and uploads it encrypted.
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

/// The Argon2id parameters a client derives a password folder's key with. Clients make them
/// (`vault-core`) and refuse ones outside the bounds below; the server checks the same bounds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasswordKdf {
    /// Random per password, [`PASSWORD_SALT_LEN`] bytes.
    pub salt: Base64Url,
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

/// Length of a password folder's Argon2id salt.
pub const PASSWORD_SALT_LEN: usize = 16;
/// Bounds of [`PasswordKdf::memory_kib`]: 19 MiB to 256 MiB.
pub const PASSWORD_MEMORY_KIB: std::ops::RangeInclusive<u32> = 19 * 1024..=256 * 1024;
/// Bounds of [`PasswordKdf::iterations`].
pub const PASSWORD_ITERATIONS: std::ops::RangeInclusive<u32> = 2..=10;
/// Bounds of [`PasswordKdf::parallelism`].
pub const PASSWORD_PARALLELISM: std::ops::RangeInclusive<u32> = 1..=4;

impl PasswordKdf {
    /// Whether the salt has its length and the costs are within the bounds clients accept.
    pub fn is_valid(&self) -> bool {
        self.salt.len() == PASSWORD_SALT_LEN
            && PASSWORD_MEMORY_KIB.contains(&self.memory_kib)
            && PASSWORD_ITERATIONS.contains(&self.iterations)
            && PASSWORD_PARALLELISM.contains(&self.parallelism)
    }
}

/// A protected folder, from `GET /protected`. Everything here is useless without the user's
/// security key, or the folder's password or recovery key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedFolder {
    pub node_id: NodeId,
    pub lock: FolderLock,
    /// The folder secret: wrapped with the vault key of `epoch` (security key), or with the key
    /// the password derives through `kdf` (password).
    pub wrapped_secret: Base64Url,
    /// Security keys: the vault epoch `wrapped_secret` is wrapped under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<i32>,
    /// Passwords: the Argon2id parameters of the password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf: Option<PasswordKdf>,
    /// Passwords: the folder secret wrapped with the folder's recovery key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_wrapped_secret: Option<Base64Url>,
    /// The age recipient content is encrypted to. Clients check it against the one the secret
    /// derives before they encrypt anything to it, so a server cannot swap in its own.
    pub recipient: String,
    pub format: i32,
    pub state: ProtectionState,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// `POST /protected`: protects one of the caller's folders with a secret the client made and
/// wrapped. A security key needs the vault's `epoch`; a password needs `kdf` and
/// `recovery_wrapped_secret`. An empty folder is protected at once. One with content stays
/// `protecting` while the client replaces it with its encrypted form, then calls
/// `POST /protected/{id}/finish`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProtectFolder {
    pub node_id: NodeId,
    pub lock: FolderLock,
    pub wrapped_secret: Base64Url,
    pub recipient: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf: Option<PasswordKdf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_wrapped_secret: Option<Base64Url>,
}

/// `POST /protected/{id}/password`: a password folder's secret wrapped for a new password,
/// which the client could only do after opening it with the old password or the recovery key.
/// With a new recovery key too when the old one was used. The server cannot check either.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangeFolderPassword {
    pub wrapped_secret: Base64Url,
    pub kdf: PasswordKdf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_wrapped_secret: Option<Base64Url>,
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
