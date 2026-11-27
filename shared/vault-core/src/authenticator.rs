//! Security keys: the platform-specific part, behind one trait.

use crate::Result;
use crate::secret::secret32;

#[cfg(any(test, feature = "testing"))]
pub use soft::{SoftAuthenticator, UnknownCredential};

secret32! {
    /// The 32 bytes a security key returns for a credential and a PRF input.
    pub struct PrfOutput;
}

impl PrfOutput {
    /// What a platform's WebAuthn PRF gave, such as the browser's `prf.results.first`, for
    /// clients that cannot answer [`Authenticator::prf`] synchronously. The caller should wipe
    /// its own copy.
    pub fn new(bytes: [u8; 32]) -> Self {
        Self::from_bytes(bytes)
    }
}

/// A WebAuthn credential on a security key, as `GET /me/webauthn` lists it.
#[derive(Clone, Copy, Debug)]
pub struct CredentialRef<'a> {
    /// The credential id the key gave at registration.
    pub credential_id: &'a [u8],
    /// The RP id it was registered under (`user_webauthn.rp_id`), passed as is.
    pub rp_id: &'a str,
}

/// A security key, reached by the platform: CTAP2 over HID on Linux and macOS, `webauthn.dll` on
/// Windows, yubikit on Android and iOS, `navigator.credentials` in the browser.
///
/// Implementations must always require user verification, and must give the same value as a
/// browser's WebAuthn PRF: `hmac-secret` over [`prf_salt_input`](crate::kdf::prf_salt_input) of
/// `input`. They may keep the key's PIN token between calls (until [`Authenticator::forget`]),
/// never a PRF output. They report their own failures as [`Error::Authenticator`](crate::Error::Authenticator).
pub trait Authenticator {
    /// Asks the key for the PRF value of `credential` and `input` (the unlocker's `prf_salt`).
    /// Prompts for a touch, and for the PIN when there is no PIN token.
    fn prf(&mut self, credential: CredentialRef<'_>, input: &[u8; 32]) -> Result<PrfOutput>;

    /// Drops whatever the authenticator kept between calls (the PIN token). Called on Lock.
    fn forget(&mut self) {}
}

#[cfg(any(test, feature = "testing"))]
mod soft {
    use std::collections::HashMap;

    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    use zeroize::Zeroizing;

    use super::{Authenticator, CredentialRef, PrfOutput};
    use crate::kdf::prf_salt_input;
    use crate::{Error, Result};

    /// Length of the credential ids [`SoftAuthenticator::register`] makes.
    const CREDENTIAL_ID_LEN: usize = 16;

    /// What [`SoftAuthenticator`] answers for a credential it did not register, or registered
    /// under another RP id.
    #[derive(Debug, thiserror::Error)]
    #[error("unknown credential")]
    pub struct UnknownCredential;

    /// A credential's RP id and its `CredRandom`, the per-credential `hmac-secret` key.
    struct Credential {
        rp_id: String,
        cred_random: Zeroizing<[u8; 32]>,
    }

    /// A software security key for tests: `hmac-secret` as a key computes it, from a random
    /// per-credential secret.
    #[derive(Default)]
    pub struct SoftAuthenticator {
        credentials: HashMap<Vec<u8>, Credential>,
        /// How many times [`Authenticator::prf`] was asked, to test "one touch per action".
        pub touches: usize,
    }

    impl SoftAuthenticator {
        /// A key with no credentials.
        pub fn new() -> Self {
            Self::default()
        }

        /// Creates a credential with `hmac-secret` for `rp_id`; returns its id.
        pub fn register(&mut self, rp_id: &str) -> Vec<u8> {
            let mut id = vec![0u8; CREDENTIAL_ID_LEN];
            rand::fill(id.as_mut_slice());
            let mut cred_random = Zeroizing::new([0u8; 32]);
            rand::fill(cred_random.as_mut());
            self.credentials.insert(
                id.clone(),
                Credential {
                    rp_id: rp_id.to_owned(),
                    cred_random,
                },
            );
            id
        }
    }

    impl Authenticator for SoftAuthenticator {
        fn prf(&mut self, credential: CredentialRef<'_>, input: &[u8; 32]) -> Result<PrfOutput> {
            self.touches += 1;
            let found = self
                .credentials
                .get(credential.credential_id)
                .filter(|found| found.rp_id == credential.rp_id)
                .ok_or_else(|| Error::Authenticator(Box::new(UnknownCredential)))?;
            let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(found.cred_random.as_slice())
                .map_err(|err| Error::Authenticator(Box::new(err)))?;
            mac.update(&prf_salt_input(input));
            Ok(PrfOutput::from_bytes(mac.finalize().into_bytes().into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_credential_and_input_give_the_same_secret() {
        let mut key = SoftAuthenticator::new();
        let id = key.register("files.example.org");
        let cred = CredentialRef {
            credential_id: &id,
            rp_id: "files.example.org",
        };
        let a = key.prf(cred, &[1; 32]).unwrap();
        let b = key.prf(cred, &[1; 32]).unwrap();
        let c = key.prf(cred, &[2; 32]).unwrap();
        assert_eq!(a.expose(), b.expose());
        assert_ne!(a.expose(), c.expose());
        assert_eq!(key.touches, 3);
        let other_rp = CredentialRef {
            credential_id: &id,
            rp_id: "other.example.org",
        };
        let err = key.prf(other_rp, &[1; 32]).unwrap_err();
        assert!(matches!(
            &err,
            crate::Error::Authenticator(source) if source.is::<UnknownCredential>()
        ));
        assert_eq!(err.to_string(), "security key: unknown credential");
    }
}
