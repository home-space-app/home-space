//! Wrapping one 32-byte key under another with XChaCha20-Poly1305.
//!
//! Format, 73 bytes: `0x01 || nonce (24) || ciphertext (32) || tag (16)`. The associated data
//! names what is wrapped and what it belongs to (the user for a vault key, the folder for a
//! folder secret), so the server cannot hand one wrapped value out as another.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use crate::{Error, Internal, Result};

const VERSION: u8 = 1;
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;
const TAG_LEN: usize = 16;
/// Length of a wrapped key.
pub const WRAPPED_LEN: usize = 1 + NONCE_LEN + KEY_LEN + TAG_LEN;
/// Starts the associated data of every wrapped value.
const AAD_DOMAIN: &[u8] = b"home-space wrap v1\0";

/// What a wrapped value is, bound into its associated data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    VaultKey,
    FolderSecret,
}

impl Purpose {
    fn label(self) -> &'static [u8] {
        match self {
            Purpose::VaultKey => b"vault key",
            Purpose::FolderSecret => b"folder secret",
        }
    }
}

fn aad(purpose: Purpose, owner: &uuid::Uuid) -> Vec<u8> {
    let mut aad = AAD_DOMAIN.to_vec();
    aad.extend_from_slice(purpose.label());
    aad.push(0);
    aad.extend_from_slice(owner.as_bytes());
    aad
}

pub(crate) fn wrap(
    key: &[u8; KEY_LEN],
    secret: &[u8; KEY_LEN],
    purpose: Purpose,
    owner: &uuid::Uuid,
) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce = [0u8; NONCE_LEN];
    rand::fill(&mut nonce);
    let aad = aad(purpose, owner);
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: secret,
                aad: &aad,
            },
        )
        .map_err(|_| Error::Internal(Internal::KeyWrap))?;
    let mut out = Vec::with_capacity(WRAPPED_LEN);
    out.push(VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

pub(crate) fn unwrap(
    key: &[u8; KEY_LEN],
    wrapped: &[u8],
    purpose: Purpose,
    owner: &uuid::Uuid,
) -> Result<[u8; KEY_LEN]> {
    if wrapped.len() != WRAPPED_LEN {
        return Err(Error::WrapFormat);
    }
    let Some((&VERSION, body)) = wrapped.split_first() else {
        return Err(Error::WrapFormat);
    };
    let Some((nonce, ciphertext)) = body.split_first_chunk::<NONCE_LEN>() else {
        return Err(Error::WrapFormat);
    };
    let cipher = XChaCha20Poly1305::new(key.into());
    let aad = aad(purpose, owner);
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                &XNonce::from(*nonce),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| Error::Unwrap)?,
    );
    <[u8; KEY_LEN]>::try_from(plain.as_slice()).map_err(|_| Error::WrapFormat)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: uuid::Uuid = uuid::Uuid::from_bytes([7; 16]);

    #[test]
    fn round_trip() {
        let wrapped = wrap(&[1; 32], &[2; 32], Purpose::VaultKey, &OWNER).unwrap();
        assert_eq!(wrapped.len(), WRAPPED_LEN);
        assert_eq!(
            unwrap(&[1; 32], &wrapped, Purpose::VaultKey, &OWNER).unwrap(),
            [2; 32]
        );
    }

    #[test]
    fn wrong_key_purpose_or_owner_fails() {
        let wrapped = wrap(&[1; 32], &[2; 32], Purpose::VaultKey, &OWNER).unwrap();
        assert!(matches!(
            unwrap(&[9; 32], &wrapped, Purpose::VaultKey, &OWNER),
            Err(Error::Unwrap)
        ));
        assert!(matches!(
            unwrap(&[1; 32], &wrapped, Purpose::FolderSecret, &OWNER),
            Err(Error::Unwrap)
        ));
        assert!(matches!(
            unwrap(&[1; 32], &wrapped, Purpose::VaultKey, &uuid::Uuid::nil()),
            Err(Error::Unwrap)
        ));
    }

    #[test]
    fn tampering_and_bad_format_fail() {
        let mut wrapped = wrap(&[1; 32], &[2; 32], Purpose::VaultKey, &OWNER).unwrap();
        wrapped[40] ^= 1;
        assert!(matches!(
            unwrap(&[1; 32], &wrapped, Purpose::VaultKey, &OWNER),
            Err(Error::Unwrap)
        ));
        wrapped[0] = 2;
        assert!(matches!(
            unwrap(&[1; 32], &wrapped, Purpose::VaultKey, &OWNER),
            Err(Error::WrapFormat)
        ));
        assert!(matches!(
            unwrap(&[1; 32], &wrapped[..10], Purpose::VaultKey, &OWNER),
            Err(Error::WrapFormat)
        ));
    }
}
