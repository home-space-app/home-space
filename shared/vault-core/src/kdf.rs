//! Wrapping keys (KEKs): from a security key's PRF secret or a recovery key.

use hkdf::Hkdf;
use sha2::{Digest, Sha256};

use crate::authenticator::PrfOutput;
use crate::recovery::RecoveryKey;
use crate::secret::secret32;

secret32! {
    /// A key-encryption key: what an unlocker derives to unwrap the vault key.
    pub struct Kek;
}

/// The salt transform browsers apply to a WebAuthn PRF input before handing it to the key's
/// `hmac-secret`: `SHA-256("WebAuthn PRF" || 0x00 || input)`. Native authenticators must apply it
/// too, or a key gives the desktop a different secret than Chrome.
pub fn prf_salt_input(input: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"WebAuthn PRF");
    hash.update([0u8]);
    hash.update(input);
    hash.finalize().into()
}

/// `HKDF-SHA256(ikm, info)` to 32 bytes.
pub(crate) fn hkdf32(ikm: &[u8], info: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    Hkdf::<Sha256>::new(None, ikm)
        .expand(info, &mut out)
        .expect("32 bytes is a valid HKDF-SHA256 length");
    out
}

impl Kek {
    /// The KEK of a security key: `HKDF-SHA256(prf secret, "home-space vault kek v1")`.
    pub fn from_prf(prf: &PrfOutput) -> Self {
        Self::from_bytes(hkdf32(prf.expose(), b"home-space vault kek v1"))
    }

    /// The KEK of a recovery key.
    pub fn from_recovery(recovery: &RecoveryKey) -> Self {
        Self::from_bytes(hkdf32(recovery.expose(), b"home-space recovery kek v1"))
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
        let a = Kek::from_prf(&PrfOutput::from_bytes([1; 32]));
        let b = Kek::from_prf(&PrfOutput::from_bytes([1; 32]));
        let c = Kek::from_prf(&PrfOutput::from_bytes([2; 32]));
        assert_eq!(a.expose(), b.expose());
        assert_ne!(a.expose(), c.expose());
        assert_ne!(a.expose(), &[1; 32]);
    }
}
