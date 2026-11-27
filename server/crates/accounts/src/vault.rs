//! The vault and protected folders of a user (feature `protected_folders`).
//!
//! Folders of both locks are end to end: the server checks shapes (lengths, the recipient's
//! form, the bounds of Argon2id parameters) and consistency, never keys. Everything it keeps is
//! wrapped by a security key, a password or a recovery key it never sees.
//!
//! Changes to what unlocks a folder need a recent sign-in, so a stolen access token cannot add
//! an attacker's unlocker or take the user's away.

use auth::AuthService;
use db::{Db, StoredKdf};
use protocol::{PasswordKdf, WRAPPED_LEN, encoding::Base64Url};

use crate::AccountError;

mod password;
mod protect;
mod unlockers;

/// The vault and protected folders of one tenant's users.
#[derive(Clone, Debug)]
pub struct VaultService {
    db: Db,
    auth: AuthService,
}

impl VaultService {
    pub fn new(db: Db, auth: AuthService) -> Self {
        Self { db, auth }
    }
}

fn check_wrapped(field: &'static str, wrapped: &Base64Url) -> Result<(), AccountError> {
    if wrapped.len() == WRAPPED_LEN {
        Ok(())
    } else {
        Err(AccountError::invalid(
            field,
            "A wrapped key must be 73 bytes.",
        ))
    }
}

/// Refuses Argon2id parameters outside what clients accept, and stores them as the database
/// keeps them.
fn check_kdf(field: &'static str, kdf: &PasswordKdf) -> Result<StoredKdf, AccountError> {
    let invalid = || {
        AccountError::invalid(
            field,
            "The password's parameters are out of bounds: a 16-byte salt, 19 to 256 MiB, 2 to \
             10 passes and 1 to 4 lanes.",
        )
    };
    if !kdf.is_valid() {
        return Err(invalid());
    }
    let count = |value: u32| i32::try_from(value).map_err(|_| invalid());
    Ok(StoredKdf {
        salt: kdf.salt.as_bytes().to_vec(),
        memory_kib: count(kdf.memory_kib)?,
        iterations: count(kdf.iterations)?,
        parallelism: count(kdf.parallelism)?,
    })
}
