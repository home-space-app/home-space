//! The rows and inputs of the vault and protected folders.

use std::fmt;

use protocol::{FolderLock, ProtectionState, UnlockerKind, redact::Redacted};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DbError, NodeId, NodeKind};

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
    /// Security keys: the folder secret wrapped with the vault key of `epoch`.
    pub wrapped_secret: Option<Vec<u8>>,
    pub epoch: Option<i32>,
    /// The age recipient content is encrypted to.
    pub recipient: String,
    pub format: i32,
    pub state: ProtectionState,
    pub created_at: OffsetDateTime,
}

/// The lock of a folder protected with a security key: its secret wrapped with the vault key
/// of `epoch`, and the age recipient clients encrypt to.
#[derive(Clone, Copy, Debug)]
pub struct KeyLock<'a> {
    pub wrapped_secret: &'a [u8],
    pub epoch: i32,
    pub recipient: &'a str,
}

/// The lock of a folder the server encrypts: the hash its password is checked against, its
/// secret sealed with the server's key, and the recipient derived from that secret.
#[derive(Clone, Copy)]
pub struct PasswordLock<'a> {
    pub password_hash: &'a str,
    /// The hash of the folder's recovery key.
    pub recovery_hash: &'a str,
    pub sealed_key: &'a [u8],
    pub recipient: &'a str,
}

impl fmt::Debug for PasswordLock<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PasswordLock")
            .field("password_hash", &Redacted)
            .field("recovery_hash", &Redacted)
            .field("sealed_key", &Redacted)
            .field("recipient", &self.recipient)
            .finish()
    }
}

/// How a folder is protected.
#[derive(Clone, Copy, Debug)]
pub enum NewProtection<'a> {
    SecurityKey(KeyLock<'a>),
    Password(PasswordLock<'a>),
}

/// A password folder's secrets, to check a password and open its key.
#[derive(Clone, PartialEq, Eq, FromRow)]
pub struct PasswordSecrets {
    pub password_hash: String,
    pub recovery_hash: String,
    pub sealed_key: Vec<u8>,
}

impl fmt::Debug for PasswordSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PasswordSecrets")
            .field("password_hash", &Redacted)
            .field("recovery_hash", &Redacted)
            .field("sealed_key", &Redacted)
            .finish()
    }
}

/// The password folder that holds a node.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct EnclosingPasswordFolder {
    pub node_id: NodeId,
    pub recipient: String,
    pub state: ProtectionState,
}

/// A stored file to convert: its node and the ids of its older versions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderFile {
    pub node_id: NodeId,
    pub versions: Vec<Uuid>,
}

/// A node inside a folder being protected, as [`Db::folder_nodes`](crate::Db::folder_nodes) lists it.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct FolderNode {
    pub id: NodeId,
    pub parent_id: Option<NodeId>,
    pub kind: NodeKind,
    pub name: String,
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
