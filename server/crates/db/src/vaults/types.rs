//! The rows and inputs of the vault and protected folders.

use protocol::{FolderLock, ProtectionState, UnlockerKind};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DbError, NodeId};

#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct StoredUnlocker {
    pub id: Uuid,
    pub kind: UnlockerKind,
    pub name: Option<String>,
    pub credential_id: Option<Vec<u8>>,
    pub rp_id: Option<String>,
    pub prf_salt: Option<Vec<u8>>,
    pub wrapped_vault_key: Vec<u8>,
    pub created_at: OffsetDateTime,
}

/// A vault and its unlockers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredVault {
    pub epoch: i32,
    pub unlockers: Vec<StoredUnlocker>,
}

/// A folder key to add: a credential the client made for the vault alone.
#[derive(Clone, Copy, Debug)]
pub struct FolderKey<'a> {
    pub name: &'a str,
    pub credential_id: &'a [u8],
    pub rp_id: &'a str,
    pub prf_salt: &'a [u8],
}

/// What a new unlocker is.
#[derive(Clone, Copy, Debug)]
pub enum UnlockerSource<'a> {
    SecurityKey(FolderKey<'a>),
    /// The recovery key, which replaces the one before.
    Recovery,
}

/// An unlocker to add.
#[derive(Clone, Copy, Debug)]
pub struct NewUnlocker<'a> {
    pub id: Uuid,
    pub source: UnlockerSource<'a>,
    pub wrapped_vault_key: &'a [u8],
    /// The epoch the key is wrapped under: the vault's, or 1 to create it.
    pub epoch: i32,
}

/// An unlocker or folder secret wrapped with a rotated vault key.
#[derive(Clone, Debug)]
pub struct Rewrapped {
    pub id: Uuid,
    pub prf_salt: Option<Vec<u8>>,
    pub wrapped: Vec<u8>,
}

/// A protected folder as stored.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct StoredProtection {
    pub node_id: NodeId,
    pub lock: FolderLock,
    /// The folder secret, wrapped with the vault key of `epoch` (security key) or the
    /// password's key (password).
    pub wrapped_secret: Vec<u8>,
    pub epoch: Option<i32>,
    /// Passwords: the folder secret wrapped with the folder's recovery key.
    pub recovery_wrapped_secret: Option<Vec<u8>>,
    pub kdf_salt: Option<Vec<u8>>,
    pub kdf_memory_kib: Option<i32>,
    pub kdf_iterations: Option<i32>,
    pub kdf_parallelism: Option<i32>,
    /// The age recipient content is encrypted to.
    pub recipient: String,
    pub format: i32,
    pub state: ProtectionState,
    pub created_at: OffsetDateTime,
}

impl StoredProtection {
    /// A password folder's Argon2id parameters; `None` for a security-key folder.
    pub fn kdf(&self) -> Option<StoredKdf> {
        Some(StoredKdf {
            salt: self.kdf_salt.clone()?,
            memory_kib: self.kdf_memory_kib?,
            iterations: self.kdf_iterations?,
            parallelism: self.kdf_parallelism?,
        })
    }
}

/// A password folder's Argon2id parameters, as the client made them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredKdf {
    pub salt: Vec<u8>,
    pub memory_kib: i32,
    pub iterations: i32,
    pub parallelism: i32,
}

/// The lock of a folder protected with a security key: its secret wrapped with the vault key
/// of `epoch`, and the age recipient clients encrypt to.
#[derive(Clone, Copy, Debug)]
pub struct KeyLock<'a> {
    pub wrapped_secret: &'a [u8],
    pub epoch: i32,
    pub recipient: &'a str,
}

/// The lock of a folder protected with a password: its secret wrapped with the key the client
/// derived from the password and with the folder's recovery key, the password's Argon2id
/// parameters, and the age recipient clients encrypt to.
#[derive(Clone, Copy, Debug)]
pub struct PasswordLock<'a> {
    pub wrapped_secret: &'a [u8],
    pub recovery_wrapped_secret: &'a [u8],
    pub kdf: &'a StoredKdf,
    pub recipient: &'a str,
}

/// A password folder's secret wrapped for a new password, and for a new recovery key when
/// the old one was used.
#[derive(Clone, Copy, Debug)]
pub struct NewPassword<'a> {
    pub wrapped_secret: &'a [u8],
    pub kdf: &'a StoredKdf,
    pub recovery_wrapped_secret: Option<&'a [u8]>,
}

/// How a folder is protected.
#[derive(Clone, Copy, Debug)]
pub enum NewProtection<'a> {
    SecurityKey(KeyLock<'a>),
    Password(PasswordLock<'a>),
}

/// Why a folder cannot be protected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProtectRefusal {
    /// Not a live folder of the user.
    #[error("not a folder of the user")]
    NotFound,
    /// The recycle bin or Lost & Found, or a folder holding one.
    #[error("a special folder or one holding it")]
    SpecialFolder,
    /// Already protected, inside a protected folder, or holding one.
    #[error("overlaps another protection")]
    Overlaps,
    /// A security-key lock whose epoch is not the vault's, or with no vault.
    #[error("the vault changed")]
    VaultChanged,
}

/// Why [`Db::protect_folder`](crate::Db::protect_folder) did not protect a folder.
#[derive(Debug, thiserror::Error)]
pub enum ProtectError {
    #[error("folder not protected: {0}")]
    Refused(#[from] ProtectRefusal),
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<sqlx::Error> for ProtectError {
    fn from(err: sqlx::Error) -> Self {
        Self::Db(err.into())
    }
}
