//! Security keys: the platform-specific part, behind one trait.

use std::collections::HashMap;

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::kdf::prf_salt_input;
use crate::secret::secret32;
use crate::{Error, Result};

secret32! {
    /// The 32 bytes a security key returns for a credential and a PRF input.
    pub struct PrfOutput;
}

/// A WebAuthn credential on a security key, as `GET /me/webauthn` lists it.
#[derive(Clone, Copy, Debug)]
pub struct CredentialRef<'a> {
    pub credential_id: &'a [u8],
    /// The RP id it was registered under (`user_webauthn.rp_id`), passed as is.
    pub rp_id: &'a str,
}

/// A security key, reached by the platform: CTAP2 over HID on Linux and macOS, `webauthn.dll` on
/// Windows, yubikit on Android and iOS, `navigator.credentials` in the browser.
///
/// Implementations must always require user verification, and must give the same value as a
/// browser's WebAuthn PRF: `hmac-secret` over [`prf_salt_input`] of `input`. They may keep the
/// key's PIN token between calls (until [`Authenticator::forget`]), never a PRF output.
pub trait Authenticator {
    /// Asks the key for the PRF value of `credential` and `input` (the unlocker's `prf_salt`).
    /// Prompts for a touch, and for the PIN when there is no PIN token.
    fn prf(&mut self, credential: CredentialRef<'_>, input: &[u8; 32]) -> Result<PrfOutput>;

    /// Drops whatever the authenticator kept between calls (the PIN token). Called on Lock.
    fn forget(&mut self) {}
}

/// A software security key for tests: `hmac-secret` as a key computes it, from a random
/// per-credential secret.
#[derive(Default)]
pub struct SoftAuthenticator {
    credentials: HashMap<(Vec<u8>, String), [u8; 32]>,
    /// How many times [`Authenticator::prf`] was asked, to test "one touch per action".
    pub touches: usize,
}

impl SoftAuthenticator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a credential with `hmac-secret` for `rp_id`; returns its id.
    pub fn register(&mut self, rp_id: &str) -> Vec<u8> {
        let mut id = vec![0u8; 16];
        rand::fill(id.as_mut_slice());
        let mut cred_random = [0u8; 32];
        rand::fill(&mut cred_random);
        self.credentials
            .insert((id.clone(), rp_id.to_owned()), cred_random);
        id
    }
}

impl Drop for SoftAuthenticator {
    fn drop(&mut self) {
        for secret in self.credentials.values_mut() {
            zeroize::Zeroize::zeroize(secret);
        }
    }
}

impl Authenticator for SoftAuthenticator {
    fn prf(&mut self, credential: CredentialRef<'_>, input: &[u8; 32]) -> Result<PrfOutput> {
        self.touches += 1;
        let cred_random = self
            .credentials
            .get(&(
                credential.credential_id.to_vec(),
                credential.rp_id.to_owned(),
            ))
            .ok_or_else(|| Error::Authenticator("unknown credential".into()))?;
        let mut mac =
            <Hmac<Sha256> as KeyInit>::new_from_slice(cred_random).expect("HMAC takes any key");
        mac.update(&prf_salt_input(input));
        Ok(PrfOutput::from_bytes(mac.finalize().into_bytes().into()))
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
        assert!(key.prf(other_rp, &[1; 32]).is_err());
    }
}
