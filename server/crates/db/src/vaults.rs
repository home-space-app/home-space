//! The vault of a user's security-key folders, its unlockers, and the protected folders.
//!
//! Everything stored here is opaque to the server: wrapped keys, PRF salts, Argon2id parameters
//! as JSON, and age recipients. The rules this module keeps are about consistency: every
//! unlocker and security-key folder is wrapped under the vault's current epoch, the last
//! unlocker of a vault with folders is never removed, and a folder is protected only where no
//! other protection overlaps it.

mod password_folders;
mod protections;
mod types;
mod unlockers;
mod unlocks;

pub use types::{
    EnclosingPasswordFolder, FolderFile, FolderKey, FolderNode, KeyLock, NewProtection,
    NewUnlocker, PasswordLock, PasswordSecrets, ProtectError, ProtectRefusal, Rewrapped,
    StoredProtection, StoredUnlocker, StoredVault, UnlockerSource,
};

#[cfg(test)]
mod tests;
