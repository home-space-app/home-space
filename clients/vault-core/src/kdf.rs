//! Wrapping keys (KEKs): from a security key's PRF secret, a recovery key, or a folder password.

use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::authenticator::PrfOutput;
use crate::recovery::RecoveryKey;
use crate::secret::secret32;
use crate::{Error, Result};

secret32! {
    /// A key-encryption key: what an unlocker derives to unwrap the vault key, or a password
    /// folder's secret.
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

    /// The KEK of a password folder: Argon2id with the folder's parameters, then HKDF for domain
    /// separation. Takes about a second with the default parameters.
    pub fn from_password(password: &str, kdf: &PasswordKdf) -> Result<Self> {
        let params = argon2::Params::new(kdf.memory_kib, kdf.passes, kdf.lanes, Some(32))
            .map_err(|e| Error::Kdf(e.to_string()))?;
        let argon =
            argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
        let mut stretched = Zeroizing::new([0u8; 32]);
        argon
            .hash_password_into(password.as_bytes(), &kdf.salt, stretched.as_mut())
            .map_err(|e| Error::Kdf(e.to_string()))?;
        Ok(Self::from_bytes(hkdf32(
            stretched.as_ref(),
            b"home-space password kek v1",
        )))
    }
}

/// Argon2id parameters of a password folder, stored in clear on the server (`kdf`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasswordKdf {
    pub salt: [u8; 16],
    pub memory_kib: u32,
    pub passes: u32,
    pub lanes: u32,
}

impl PasswordKdf {
    /// Shortest folder password accepted, in characters.
    pub const MIN_PASSWORD_CHARS: usize = 12;

    /// New parameters with a random salt: 64 MiB, 3 passes, 1 lane, which the web client and
    /// phones can run in about a second.
    pub fn generate() -> Self {
        let mut salt = [0u8; 16];
        rand::fill(&mut salt);
        Self {
            salt,
            memory_kib: 64 * 1024,
            passes: 3,
            lanes: 1,
        }
    }

    /// Checks a new folder password. Only the length: the clients show a strength meter too.
    pub fn check_new_password(password: &str) -> Result<()> {
        if password.chars().count() < Self::MIN_PASSWORD_CHARS {
            return Err(Error::PasswordTooShort {
                min: Self::MIN_PASSWORD_CHARS,
            });
        }
        Ok(())
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

    #[test]
    fn password_kek_depends_on_password_and_salt() {
        // Small parameters to keep the test fast; the real ones only change the cost.
        let kdf = PasswordKdf {
            salt: [3; 16],
            memory_kib: 64,
            passes: 1,
            lanes: 1,
        };
        let a = Kek::from_password("correct horse battery", &kdf).unwrap();
        let b = Kek::from_password("correct horse battery", &kdf).unwrap();
        let c = Kek::from_password("correct horse battery!", &kdf).unwrap();
        let d = Kek::from_password(
            "correct horse battery",
            &PasswordKdf {
                salt: [4; 16],
                ..kdf.clone()
            },
        )
        .unwrap();
        assert_eq!(a.expose(), b.expose());
        assert_ne!(a.expose(), c.expose());
        assert_ne!(a.expose(), d.expose());
    }

    #[test]
    fn short_passwords_are_refused() {
        assert!(PasswordKdf::check_new_password("short").is_err());
        assert!(PasswordKdf::check_new_password("twelve chars").is_ok());
        // Characters, not bytes.
        assert!(PasswordKdf::check_new_password("ééééééééééé").is_err());
    }

    #[test]
    fn generated_params_are_valid() {
        let kdf = PasswordKdf::generate();
        assert!(argon2::Params::new(kdf.memory_kib, kdf.passes, kdf.lanes, Some(32)).is_ok());
        assert_ne!(kdf.salt, PasswordKdf::generate().salt);
    }
}
