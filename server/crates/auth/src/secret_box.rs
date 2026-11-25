//! Sealing the secrets the server has to read back later, such as authenticator app secrets,
//! with the tenant's `HOMESPACE_SECRETS_KEY`, so that a database dump does not give them away.
//!
//! AES-256-GCM with a key derived by HKDF-SHA256 from the configured secret, and a random
//! nonce stored in front of the ciphertext. The tenant and user are the associated data, so a
//! sealed secret cannot be moved to another row.

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

/// The tenant's keys: the current one seals, and earlier ones still open what they sealed.
#[derive(Clone)]
pub struct SecretBox {
    current: Aes256Gcm,
    previous: Vec<Aes256Gcm>,
}

/// A secret as [`SecretBox::open`] read it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Opened {
    pub(crate) plain: Vec<u8>,
    /// It was sealed with an earlier key, so it should be sealed again with the current one.
    pub(crate) stale: bool,
}

impl SecretBox {
    /// Keys from the configured secrets, which [`check_secret`](crate::secrets::check_secret)
    /// has accepted.
    pub fn new(current: &str, previous: &[String]) -> Self {
        Self {
            current: cipher(current),
            previous: previous.iter().map(|secret| cipher(secret)).collect(),
        }
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
        let mut nonce = [0u8; NONCE_BYTES];
        rand::fill(&mut nonce);
        let aad = associated_data(tenant, user);
        let sealed = self
            .current
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: plain,
                    aad: &aad,
                },
            )
            .map_err(|_| AuthError::Sealing)?;
        Ok([nonce.as_slice(), &sealed].concat())
    }

    /// What `seal` sealed for `user` of `tenant`, with whichever key sealed it.
    ///
    /// # Errors
    ///
    /// [`AuthError::Unsealing`] when no key opens it: it was sealed with a key no longer
    /// configured, or for another user.
    pub(crate) fn open(
        &self,
        tenant: TenantId,
        user: UserId,
        sealed: &[u8],
    ) -> Result<Opened, AuthError> {
        let (nonce, ciphertext) = sealed
            .split_at_checked(NONCE_BYTES)
            .ok_or(AuthError::Unsealing)?;
        let nonce = Nonce::try_from(nonce).map_err(|_| AuthError::Unsealing)?;
        let aad = associated_data(tenant, user);
        std::iter::once(&self.current)
            .chain(&self.previous)
            .enumerate()
            .find_map(|(index, cipher)| {
                let payload = Payload {
                    msg: ciphertext,
                    aad: &aad,
                };
                cipher.decrypt(&nonce, payload).ok().map(|plain| Opened {
                    plain,
                    stale: index > 0,
                })
            })
            .ok_or(AuthError::Unsealing)
    }
}

impl fmt::Debug for SecretBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretBox")
            .field("previous", &self.previous.len())
            .finish_non_exhaustive()
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

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "kq3ZcV0m8Tn1yWfLr7uGxP2aHs9EoJd4BbNiC6tM5QzK";
    const OLD_KEY: &str = "Xw7pR2nLq9TzVb4KsYc1MdHg6JfAe3Uo8NiZ5tQrWmEx";

    #[test]
    fn a_sealed_secret_opens_only_for_its_tenant_and_user() {
        let keys = SecretBox::new(KEY, &[]);
        let user = UserId::new();
        let sealed = keys.seal(TenantId::DEFAULT, user, b"secret").expect("seal");
        assert_ne!(&sealed[NONCE_BYTES..], b"secret");
        let opened = keys.open(TenantId::DEFAULT, user, &sealed).expect("open");
        assert_eq!(
            opened,
            Opened {
                plain: b"secret".to_vec(),
                stale: false
            }
        );
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
    fn an_earlier_key_still_opens_and_asks_to_seal_again() {
        let user = UserId::new();
        let sealed = SecretBox::new(OLD_KEY, &[])
            .seal(TenantId::DEFAULT, user, b"secret")
            .expect("seal");
        let rotated = SecretBox::new(KEY, &[OLD_KEY.to_owned()]);
        let opened = rotated
            .open(TenantId::DEFAULT, user, &sealed)
            .expect("open");
        assert!(opened.stale);
        assert!(
            SecretBox::new(KEY, &[])
                .open(TenantId::DEFAULT, user, &sealed)
                .is_err()
        );
    }
}
