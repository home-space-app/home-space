//! Signing keys of one tenant, and which of them signed a token.

use super::*;

/// The signing key of one tenant. There is one at a time: a secret replaced by
/// `homespace-ctl secrets rotate` verifies nothing, so a leaked one is useless once rotated.
///
/// Every token names the tenant in its `tid` claim, and one naming another tenant is refused
/// even when its signature is good, so a secret two tenants share by mistake still keeps
/// their tokens apart.
#[derive(Clone)]
pub struct Keys {
    pub(super) tenant: TenantId,
    pub(super) current: Vec<u8>,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keys")
            .field("tenant", &self.tenant)
            .finish_non_exhaustive()
    }
}

impl Keys {
    pub fn new(tenant: TenantId, current: Vec<u8>) -> Self {
        Self { tenant, current }
    }

    /// Whether a token's `tid` claim names the tenant of these keys.
    pub(super) fn is_own(&self, tid: i32) -> bool {
        tid == self.tenant.0
    }

    /// Identifier stored in the token header: a truncated hash, never the secret itself.
    fn key_id(secret: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(&Sha256::digest(secret)[..KEY_ID_BYTES])
    }

    pub(super) fn header(&self) -> Header {
        Header {
            kid: Some(Self::key_id(&self.current)),
            ..Header::default()
        }
    }

    /// The secret to verify a token with. A token without a key id predates key ids and is
    /// checked against the secret; one naming another key, such as a rotated-out one, is
    /// rejected.
    pub(super) fn secret_for(&self, token: &str) -> Result<&[u8], AuthError> {
        let kid = decode_header(token)
            .map_err(|_| AuthError::InvalidCredentials)?
            .kid;
        match kid {
            Some(kid) if kid != Self::key_id(&self.current) => Err(AuthError::InvalidCredentials),
            _ => Ok(&self.current),
        }
    }
}
