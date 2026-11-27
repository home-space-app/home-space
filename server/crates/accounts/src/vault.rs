//! The vault and protected folders of a user (feature `protected_folders`).
//!
//! For security-key folders the server checks shapes (lengths, the recipient's form) and
//! consistency, never keys: everything it keeps is wrapped by a security key it never sees.
//! Password folders are the server's own: it keeps their key sealed with its secrets key and
//! opens it to sessions that gave the password.
//!
//! Changes to what unlocks a folder need a recent sign-in, so a stolen access token cannot add
//! an attacker's unlocker or take the user's away.

use time::Duration;

use auth::AuthService;
use db::Db;
use protocol::{WRAPPED_LEN, encoding::Base64Url, rules::password_problem};

use crate::AccountError;

mod convert;
mod names;
mod password;
mod protect;
mod unlockers;

pub use convert::ConvertingFolder;
pub use names::Shown;

/// How long a password folder stays unlocked for a session that stopped using it.
pub const FOLDER_UNLOCK_TTL: Duration = Duration::minutes(15);

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

/// Refuses a folder password that breaks the password rules.
fn check_password(password: &str) -> Result<(), AccountError> {
    match password_problem(password) {
        Some(message) => Err(AccountError::invalid("/password", message)),
        None => Ok(()),
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
