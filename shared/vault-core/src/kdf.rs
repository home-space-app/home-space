//! Wrapping keys (KEKs): from a security key's PRF secret or a recovery key.

use hkdf::Hkdf;
use sha2::{Digest, Sha256};

use crate::authenticator::PrfOutput;
use crate::recovery::RecoveryKey;
use crate::secret::secret32;
use crate::{Error, Internal, Result};

/// What browsers prefix a WebAuthn PRF input with, before a zero byte.
const WEBAUTHN_PRF_CONTEXT: &[u8] = b"WebAuthn PRF";
const VAULT_KEK_INFO: &[u8] = b"home-space vault kek v1";
const RECOVERY_KEK_INFO: &[u8] = b"home-space recovery kek v1";

secret32! {
    /// A key-encryption key: what an unlocker derives to unwrap the vault key.
    pub struct Kek;
}

/// The salt transform browsers apply to a WebAuthn PRF input before handing it to the key's
/// `hmac-secret`: `SHA-256("WebAuthn PRF" || 0x00 || input)`. Native authenticators must apply it
/// too, or a key gives the desktop a different secret than Chrome.
pub fn prf_salt_input(input: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(WEBAUTHN_PRF_CONTEXT);
    hash.update([0u8]);
    hash.update(input);
    hash.finalize().into()
}

/// `HKDF-SHA256(ikm, info)` into all of `out`.
pub(crate) fn hkdf_into(ikm: &[u8], info: &[u8], out: &mut [u8]) -> Result<()> {
    Hkdf::<Sha256>::new(None, ikm)
        .expand(info, out)
        .map_err(|_| Error::Internal(Internal::KeyDerivation))
}

/// `HKDF-SHA256(ikm, info)` to 32 bytes.
pub(crate) fn hkdf32(ikm: &[u8], info: &[u8]) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    hkdf_into(ikm, info, &mut out)?;
    Ok(out)
}

impl Kek {
    /// The KEK of a security key: `HKDF-SHA256(prf secret, "home-space vault kek v1")`.
    pub fn from_prf(prf: &PrfOutput) -> Result<Self> {
        hkdf32(prf.expose(), VAULT_KEK_INFO).map(Self::from_bytes)
    }

    /// The KEK of a recovery key.
    pub fn from_recovery(recovery: &RecoveryKey) -> Result<Self> {
        hkdf32(recovery.expose(), RECOVERY_KEK_INFO).map(Self::from_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prf_salt_transform_vector() {
        // SHA-256("WebAuthn PRF" || 0x00 || ""), the value every client must agree on.
        let expected = {
            let mut h = Sha256::new();
            h.update(b"WebAuthn PRF\0");
            h.finalize()
        };
        assert_eq!(prf_salt_input(b"").as_slice(), expected.as_slice());
        assert_ne!(prf_salt_input(b"a"), prf_salt_input(b"b"));
    }

    #[test]
    fn prf_kek_is_stable_and_distinct() {
        let a = Kek::from_prf(&PrfOutput::from_bytes([1; 32])).unwrap();
        let b = Kek::from_prf(&PrfOutput::from_bytes([1; 32])).unwrap();
        let c = Kek::from_prf(&PrfOutput::from_bytes([2; 32])).unwrap();
        assert_eq!(a.expose(), b.expose());
        assert_ne!(a.expose(), c.expose());
        assert_ne!(a.expose(), &[1; 32]);
    }
}
