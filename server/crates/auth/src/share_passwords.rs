//! The passwords of share links, and the proof a visitor gets for giving one.

use std::net::IpAddr;

use time::Duration;

use db::{ShareLink, TenantId};
use protocol::ShareAccess;

use crate::{
    AuthError,
    hasher::PasswordHasher,
    password::MAX_PASSWORD_LEN,
    throttle::{Counter, LoginThrottle, attempt_source},
    token::{Keys, ShareGrant, decode_share_access, encode_share_access},
};

/// How long a share link stays unlocked after its password was given.
const SHARE_ACCESS_TTL: Duration = Duration::hours(12);
/// Wrong passwords of a share link are counted under this prefix and the link's id.
const SHARE_ATTEMPTS_PREFIX: &str = "share:";

/// Hashes link passwords and checks them, with one tenant's keys.
#[derive(Clone, Debug)]
pub struct SharePasswords {
    pub(crate) tenant: TenantId,
    pub(crate) keys: Keys,
    pub(crate) hasher: PasswordHasher,
    pub(crate) throttle: LoginThrottle,
}

impl SharePasswords {
    /// Hashes the password of a share link, checked like an account password.
    pub async fn hash(&self, password: &str) -> Result<String, AuthError> {
        self.hasher.hash_new(password).await
    }

    /// Unlocks `share` with `password`, returning proof of it for the following requests.
    ///
    /// Wrong passwords are throttled like wrong sign-ins, counted per link and per source
    /// `address`, so the password of one link cannot be guessed by brute force.
    pub async fn unlock(
        &self,
        share: &ShareLink,
        password: &str,
        address: Option<IpAddr>,
    ) -> Result<ShareAccess, AuthError> {
        if let Some(hash) = share.password_hash.clone() {
            if password.len() > MAX_PASSWORD_LEN {
                return Err(AuthError::InvalidCredentials);
            }
            let source = address.map(attempt_source).unwrap_or_default();
            let key = format!("{SHARE_ATTEMPTS_PREFIX}{}", share.id);
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
                .await?;
        }
        let grant = ShareGrant {
            share_id: share.id,
            password_version: share.password_version,
        };
        Ok(ShareAccess {
            access: encode_share_access(&self.keys, grant, SHARE_ACCESS_TTL)?,
            expires_in: SHARE_ACCESS_TTL.whole_seconds(),
        })
    }

    /// Whether `access` unlocks `share` with its current password.
    pub fn verifies(&self, share: &ShareLink, access: &str) -> bool {
        decode_share_access(&self.keys, access).is_ok_and(|grant| {
            grant.share_id == share.id && grant.password_version == share.password_version
        })
    }
}
