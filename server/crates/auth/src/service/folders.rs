//! The passwords and keys of password folders, which the server encrypts itself: hashing and
//! checking a folder's password or recovery key, throttled like a sign-in, and keeping the
//! folder's key sealed with the tenant's secrets key.
//!
//! A wrong password counts three times: for the folder from the address it came from, for the
//! folder from every address together, and for the address over everything it guesses at,
//! sign-ins included. Spreading guesses over many addresses therefore gains nothing. Each
//! count's wait is bounded (a day per address, an hour over all of them), so nobody can lock
//! the owner out for good; the recovery key is counted apart from the password, so guessing
//! the password never keeps the owner from recovering with it. Only the owner's own sessions
//! can try either, so ending those (by changing the account password, say) stops a guesser.

use std::net::IpAddr;

use zeroize::Zeroizing;

use db::{NodeId, UserId};

use super::AuthService;
use crate::{
    AuthError,
    password::MAX_PASSWORD_LEN,
    throttle::{Counter, attempt_source},
};

/// Wrong passwords of a folder are counted under this prefix and the folder's id.
const FOLDER_PASSWORD_PREFIX: &str = "folder:";
/// Wrong recovery keys of a folder are counted under this prefix and the folder's id.
const FOLDER_RECOVERY_PREFIX: &str = "folder-recovery:";

/// Which secret of a password folder is checked. Each is counted on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderSecretKind {
    /// The folder's password.
    Password,
    /// The folder's recovery key.
    RecoveryKey,
}

impl FolderSecretKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Password => FOLDER_PASSWORD_PREFIX,
            Self::RecoveryKey => FOLDER_RECOVERY_PREFIX,
        }
    }
}

impl AuthService {
    /// Whether this server can keep the keys of password folders: it has a secrets key.
    pub fn keeps_folder_keys(&self) -> bool {
        self.secret_box.is_some()
    }

    /// Hashes a new folder password, like an account password.
    pub async fn hash_folder_password(&self, password: &str) -> Result<String, AuthError> {
        self.hasher.hash_new(password).await
    }

    /// Checks `password`, the folder's secret of `kind`, against its `hash`. Wrong ones are
    /// throttled like wrong sign-ins (see the module), so a folder's password cannot be guessed
    /// through the API, from one address or many.
    pub async fn check_folder_password(
        &self,
        folder: NodeId,
        kind: FolderSecretKind,
        hash: String,
        password: &str,
        address: Option<IpAddr>,
    ) -> Result<(), AuthError> {
        if password.len() > MAX_PASSWORD_LEN {
            return Err(AuthError::InvalidCredentials);
        }
        let key = format!("{}{folder}", kind.prefix());
        let mut counters = Vec::with_capacity(3);
        let source = match address.map(attempt_source) {
            Some(source) => {
                counters.push(Counter::address(source.clone()));
                source
            }
            None => String::new(),
        };
        counters.push(Counter::key(self.tenant, key.as_str(), source));
        counters.push(Counter::all_sources(self.tenant, key));
        let attempt = self.throttle.admit(counters).await?;
        let checked = match self.hasher.verify(password, hash).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(AuthError::InvalidCredentials),
            Err(err) => Err(err),
        };
        attempt
            .settle(checked, |err| matches!(err, AuthError::InvalidCredentials))
            .await
    }

    /// `key`, sealed for `user` with the tenant's secrets key.
    pub fn seal_folder_key(&self, user: UserId, key: &[u8]) -> Result<Vec<u8>, AuthError> {
        self.secret_box()?.seal(self.tenant, user, key)
    }

    /// What [`Self::seal_folder_key`] sealed for `user`.
    pub fn open_folder_key(
        &self,
        user: UserId,
        sealed: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, AuthError> {
        let plain = self.secret_box()?.open(self.tenant, user, sealed)?;
        Ok(Zeroizing::new(plain))
    }
}
