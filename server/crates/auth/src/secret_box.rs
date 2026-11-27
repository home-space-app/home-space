//! Sealing the secrets the server has to read back later, such as authenticator app secrets,
//! with the tenant's `HOMESPACE_SECRETS_KEY`, so that a database dump does not give them away.
//!
//! AES-256-GCM with a key derived by HKDF-SHA256 from the configured secret, and a random
//! nonce stored in front of the ciphertext. The tenant and user are the associated data, so a
//! sealed secret cannot be moved to another row.
//!
//! There is one key at a time: `homespace-ctl secrets rotate` seals everything again with a
//! new one, so an old key opens nothing once it is replaced.

use std::fmt;

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use hkdf::Hkdf;
use sha2::Sha256;

use db::{TenantId, UserId};

use crate::AuthError;

const NONCE_BYTES: usize = 12;
/// Names what the derived key is for, so the same secret could key something else apart.
const KEY_INFO: &[u8] = b"homespace secret box v1";
/// What the check of a tenant's key seals: any value works, as only the key opens it.
const CHECK_PLAIN: &[u8] = b"homespace secrets key check v1";

/// The tenant's secrets key. Not `Clone`, so the key is not copied around: share it in an
/// `Arc`.
pub struct SecretBox {
    cipher: Aes256Gcm,
}

impl SecretBox {
    /// The key from the configured secret, which
    /// [`check_secret`](crate::secrets::check_secret) has accepted.
    pub fn new(secret: &str) -> Self {
        Self {
            cipher: cipher(secret),
        }
    }

    /// A check for this key, to store beside what it seals: only this key opens it, so it
    /// tells whether a configured key is the one the tenant's secrets were sealed with.
    ///
    /// # Errors
    ///
    /// [`AuthError::Sealing`], as for [`seal`](Self::seal).
    pub fn seal_check(&self, tenant: TenantId) -> Result<Vec<u8>, AuthError> {
        self.seal_with(&check_data(tenant), CHECK_PLAIN)
    }

    /// Whether `sealed` is a check [`seal_check`](Self::seal_check) made with this key.
    pub fn opens_check(&self, tenant: TenantId, sealed: &[u8]) -> bool {
        self.open_with(&check_data(tenant), sealed)
            .is_ok_and(|plain| plain == CHECK_PLAIN)
    }

    /// `plain`, sealed for `user` of `tenant` with the current key.
    ///
    /// # Errors
    ///
    /// [`AuthError::Sealing`] when it cannot be encrypted, which AES-GCM allows only for a
    /// message far longer than any secret.
    pub(crate) fn seal(
        &self,
        tenant: TenantId,
        user: UserId,
        plain: &[u8],
    ) -> Result<Vec<u8>, AuthError> {
        self.seal_with(&associated_data(tenant, user), plain)
    }

    /// What `seal` sealed for `user` of `tenant` with this key.
    ///
    /// # Errors
    ///
    /// [`AuthError::Unsealing`] when it does not open: it was sealed with another key, or
    /// for another user.
    pub(crate) fn open(
        &self,
        tenant: TenantId,
        user: UserId,
        sealed: &[u8],
    ) -> Result<Vec<u8>, AuthError> {
        self.open_with(&associated_data(tenant, user), sealed)
    }

    fn seal_with(&self, aad: &[u8], plain: &[u8]) -> Result<Vec<u8>, AuthError> {
        let mut nonce = [0u8; NONCE_BYTES];
        rand::fill(&mut nonce);
        let sealed = self
            .cipher
            .encrypt(&Nonce::from(nonce), Payload { msg: plain, aad })
            .map_err(|_| AuthError::Sealing)?;
        Ok([nonce.as_slice(), &sealed].concat())
    }

    fn open_with(&self, aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, AuthError> {
        let (nonce, ciphertext) = sealed
            .split_at_checked(NONCE_BYTES)
            .ok_or(AuthError::Unsealing)?;
        let nonce = Nonce::try_from(nonce).map_err(|_| AuthError::Unsealing)?;
        self.cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| AuthError::Unsealing)
    }
}

impl fmt::Debug for SecretBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretBox").finish_non_exhaustive()
    }
}

fn cipher(secret: &str) -> Aes256Gcm {
    let mut key = [0u8; 32];
    // 32 bytes is far below what HKDF-SHA256 can expand to, so this cannot fail.
    let _ = Hkdf::<Sha256>::new(None, secret.as_bytes()).expand(KEY_INFO, &mut key);
    Aes256Gcm::new(&key.into())
}

fn associated_data(tenant: TenantId, user: UserId) -> Vec<u8> {
    format!("{tenant}:{user}").into_bytes()
}

/// The associated data of a tenant's check, which no user's id can be.
fn check_data(tenant: TenantId) -> Vec<u8> {
    format!("{tenant}:secrets-key-check").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "kq3ZcV0m8Tn1yWfLr7uGxP2aHs9EoJd4BbNiC6tM5QzK";
    const OTHER_KEY: &str = "Xw7pR2nLq9TzVb4KsYc1MdHg6JfAe3Uo8NiZ5tQrWmEx";

    #[test]
    fn a_sealed_secret_opens_only_for_its_tenant_and_user() {
        let keys = SecretBox::new(KEY);
        let user = UserId::new();
        let sealed = keys.seal(TenantId::DEFAULT, user, b"secret").expect("seal");
        assert_ne!(&sealed[NONCE_BYTES..], b"secret");
        let opened = keys.open(TenantId::DEFAULT, user, &sealed).expect("open");
        assert_eq!(opened, b"secret");
        assert!(
            keys.open(TenantId::DEFAULT, UserId::new(), &sealed)
                .is_err()
        );
        assert!(keys.open(TenantId(2), user, &sealed).is_err());
        assert!(keys.open(TenantId::DEFAULT, user, &sealed[..5]).is_err());
        assert_ne!(
            sealed,
            keys.seal(TenantId::DEFAULT, user, b"secret").expect("seal")
        );
    }

    #[test]
    fn another_key_opens_nothing() {
        let user = UserId::new();
        let sealed = SecretBox::new(OTHER_KEY)
            .seal(TenantId::DEFAULT, user, b"secret")
            .expect("seal");
        assert!(
            SecretBox::new(KEY)
                .open(TenantId::DEFAULT, user, &sealed)
                .is_err()
        );
    }

    #[test]
    fn a_check_opens_only_with_its_key_and_tenant() {
        let keys = SecretBox::new(KEY);
        let check = keys.seal_check(TenantId::DEFAULT).expect("seal");
        assert!(keys.opens_check(TenantId::DEFAULT, &check));
        assert!(!keys.opens_check(TenantId(2), &check));
        assert!(!SecretBox::new(OTHER_KEY).opens_check(TenantId::DEFAULT, &check));
        // A user's sealed secret is no check, even with the right key.
        let sealed = keys
            .seal(TenantId::DEFAULT, UserId::new(), CHECK_PLAIN)
            .expect("seal");
        assert!(!keys.opens_check(TenantId::DEFAULT, &sealed));
    }
}
