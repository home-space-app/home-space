//! Protected folders and the vault that unlocks them (feature `protected_folders`).
//!
//! A folder locked with a security key is end to end: the server keeps only what is useless
//! without the key (wrapped keys, PRF salts, the folders' public age recipients) and clients do
//! all the cryptography (`shared/vault-core`). A folder locked with a password is encrypted by
//! the server, which keeps its key. See `docs/todo/protected-folders.md`.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{NodeId, encoding::Base64Url};

/// Length of a wrapped vault key or folder secret: version, nonce, key and tag.
pub const WRAPPED_LEN: usize = 73;
/// Length of the PRF input of a security key unlocker.
pub const PRF_SALT_LEN: usize = 32;
/// What opens the vault.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnlockerKind {
    /// One of the user's security keys (a sign-in credential with PRF).
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

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "security_key" => Some(Self::SecurityKey),
            "recovery" => Some(Self::Recovery),
            _ => None,
        }
    }
}

/// One unlocker of the vault, as `GET /vault` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultUnlocker {
    pub id: Uuid,
    pub kind: UnlockerKind,
    /// Security keys only: the sign-in credential (`GET /me/webauthn`), which names the key's
    /// credential id and RP id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<Uuid>,
    /// Security keys only: the authenticator's credential id, base64url, so a client asks the
    /// right key without another request.
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

/// `POST /vault/unlockers`: enables a security key or sets the recovery key. The first creates
/// the vault, at epoch 1; a recovery key replaces the one before.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddUnlocker {
    pub kind: UnlockerKind,
    /// Security keys: the sign-in credential, which must be a hardware key with PRF.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<Uuid>,
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
pub enum FolderLock {
    /// End to end: the client encrypts to the folder's recipient, and any of the user's
    /// enabled security keys opens it through the vault.
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

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "security_key" => Some(Self::SecurityKey),
            "password" => Some(Self::Password),
            _ => None,
        }
    }
}

/// Where a protected folder stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "protecting" => Some(Self::Protecting),
            "protected" => Some(Self::Protected),
            "unprotecting" => Some(Self::Unprotecting),
            _ => None,
        }
    }
}

/// A protected folder, from `GET /protected`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// `POST /protected`: protects one of the caller's folders. A security key needs
/// `wrapped_secret`, `recipient` and the vault's `epoch`; a password needs `password`. An empty
/// folder is protected at once. One with content stays `protecting` until it is replaced with
/// its encrypted form: by the client, which then calls `POST /protected/{id}/finish` (security
/// key), or by the server (password).
#[derive(Clone, Debug, Serialize, Deserialize)]
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

/// `POST /protected/{id}/unlock`: the password of a password folder.
#[derive(Clone, Serialize, Deserialize)]
pub struct UnlockFolder {
    pub password: String,
}

impl std::fmt::Debug for UnlockFolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UnlockFolder { .. }")
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
