//! The passwords and keys of password folders, which the server encrypts itself: hashing and
//! checking a folder's password, throttled like a sign-in, and keeping the folder's key sealed
//! with the tenant's secrets key.

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
const FOLDER_ATTEMPTS_PREFIX: &str = "folder:";

impl AuthService {
    /// Whether this server can keep the keys of password folders: it has a secrets key.
    pub fn keeps_folder_keys(&self) -> bool {
        self.secret_box.is_some()
    }

    /// Hashes a new folder password, like an account password.
    pub async fn hash_folder_password(&self, password: &str) -> Result<String, AuthError> {
        self.hasher.hash_new(password).await
    }

    /// Checks `password` against the hash of `folder`. Wrong passwords are throttled like wrong
    /// sign-ins, per folder and per source `address`, so a folder's password cannot be guessed
    /// through the API.
    pub async fn check_folder_password(
        &self,
        folder: NodeId,
        hash: String,
        password: &str,
        address: Option<IpAddr>,
    ) -> Result<(), AuthError> {
        if password.len() > MAX_PASSWORD_LEN {
            return Err(AuthError::InvalidCredentials);
        }
        let source = address.map(attempt_source).unwrap_or_default();
        let key = format!("{FOLDER_ATTEMPTS_PREFIX}{folder}");
        let attempt = self
            .throttle
            .admit(vec![Counter::key(self.tenant, key, source)])
            .await?;
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
        let opened = self.secret_box()?.open(self.tenant, user, sealed)?;
        Ok(Zeroizing::new(opened.plain))
    }
}
