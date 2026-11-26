//! Protected folders and the vault that unlocks them (feature `protected_folders`).
//!
//! The server keeps only what is useless without the user's security key or password: wrapped
//! keys, PRF salts, Argon2id parameters and the folders' public age recipients. Clients do all
//! the cryptography (`clients/vault-core`); see `docs/todo/protected-folders.md`.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{NodeId, encoding::Base64Url};

/// Length of a wrapped vault key or folder secret: version, nonce, key and tag.
pub const WRAPPED_LEN: usize = 73;
/// Length of the PRF input of a security key unlocker.
pub const PRF_SALT_LEN: usize = 32;
/// Length of the Argon2id salt of a password folder.
pub const KDF_SALT_LEN: usize = 16;
/// The least Argon2id memory the server accepts for a password folder, in KiB (19 MiB, the
/// OWASP minimum). Clients use more (64 MiB).
pub const MIN_KDF_MEMORY_KIB: u32 = 19 * 1024;
/// The most, in KiB (4 GiB), so a folder cannot ask clients for more than they can give.
pub const MAX_KDF_MEMORY_KIB: u32 = 4 * 1024 * 1024;
/// The most Argon2id passes.
pub const MAX_KDF_PASSES: u32 = 16;
/// The most Argon2id lanes.
pub const MAX_KDF_LANES: u32 = 16;

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

/// A folder secret wrapped with a new key, in a [`RotateVault`] or a [`ChangeLock`].
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
    /// The vault, so any of the user's enabled security keys.
    SecurityKey,
    /// A password of this folder; anyone with the server's data can try to guess it.
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

/// The Argon2id salt and parameters of a password folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasswordKdf {
    pub salt: Base64Url,
    pub memory_kib: u32,
    pub passes: u32,
    pub lanes: u32,
}

impl PasswordKdf {
    /// Why the server refuses these parameters; `None` when they are acceptable.
    pub fn problem(&self) -> Option<&'static str> {
        if self.salt.len() != KDF_SALT_LEN {
            Some("The salt must be 16 bytes.")
        } else if !(MIN_KDF_MEMORY_KIB..=MAX_KDF_MEMORY_KIB).contains(&self.memory_kib) {
            Some("The memory must be between 19 MiB and 4 GiB.")
        } else if !(1..=MAX_KDF_PASSES).contains(&self.passes) {
            Some("The passes must be between 1 and 16.")
        } else if !(1..=MAX_KDF_LANES).contains(&self.lanes) {
            Some("The lanes must be between 1 and 16.")
        } else {
            None
        }
    }
}

/// Where a protected folder stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectionState {
    /// A client is encrypting the existing content; nothing else may change it meanwhile.
    Protecting,
    Protected,
    /// A client is decrypting the content back.
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
    pub wrapped_secret: Base64Url,
    /// Security keys: the vault epoch the secret is wrapped under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<i32>,
    /// Passwords: how the key is derived from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf: Option<PasswordKdf>,
    /// The age recipient uploads encrypt to. Clients check it against the one their key
    /// derives before they encrypt anything to it, so a server cannot swap in its own.
    pub recipient: String,
    pub format: i32,
    pub state: ProtectionState,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// `POST /protected`: protects one of the caller's folders.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProtectFolder {
    pub node_id: NodeId,
    pub lock: FolderLock,
    pub wrapped_secret: Base64Url,
    pub recipient: String,
    /// Security keys: the vault's current epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<i32>,
    /// Passwords.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf: Option<PasswordKdf>,
}

/// `PATCH /protected/{id}`: locks the folder with something else (or a new password). The
/// secret and recipient stay, so no file is encrypted again.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangeLock {
    pub lock: FolderLock,
    pub wrapped_secret: Base64Url,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf: Option<PasswordKdf>,
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

    #[test]
    fn weak_or_huge_kdf_parameters_are_refused() {
        let good = PasswordKdf {
            salt: Base64Url(vec![0; KDF_SALT_LEN]),
            memory_kib: 64 * 1024,
            passes: 3,
            lanes: 1,
        };
        assert_eq!(good.problem(), None);
        let weak = PasswordKdf {
            memory_kib: 1024,
            ..good.clone()
        };
        assert!(weak.problem().is_some());
        let short_salt = PasswordKdf {
            salt: Base64Url(vec![0; 8]),
            ..good.clone()
        };
        assert!(short_salt.problem().is_some());
        let no_passes = PasswordKdf { passes: 0, ..good };
        assert!(no_passes.problem().is_some());
    }
}
