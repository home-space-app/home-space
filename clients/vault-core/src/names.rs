//! Sealed names: deterministic AES-SIV, so the server can still refuse two equal names in one
//! folder without reading them.
//!
//! `sealed = AES-SIV-256(name key, ad = [parent id, kind], pad(NFC(name)))`. The padding is
//! `0x80` and zeros up to a multiple of 32 bytes, so the server learns a name's length only to
//! that granularity.

use aes_siv::aead::KeyInit;
use aes_siv::siv::Aes256Siv;
use unicode_normalization::{UnicodeNormalization, is_nfc};
use zeroize::Zeroizing;

use crate::{Error, Result};

/// Longest name, in characters, as the server's `nodes.name` check.
pub const MAX_NAME_CHARS: usize = 255;
/// Names are padded to a multiple of this many bytes.
pub const PAD_TO: usize = 32;
/// Longest sealed name in bytes: 255 four-byte characters, padded, plus the 16-byte tag.
pub const MAX_SEALED_LEN: usize = (MAX_NAME_CHARS * 4 / PAD_TO + 1) * PAD_TO + 16;

/// File or folder; bound into the sealed name like the server's unique key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Folder,
    File,
}

impl NodeKind {
    /// The server's spelling (`nodes.kind`).
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Folder => "folder",
            NodeKind::File => "file",
        }
    }
}

fn cipher(key: &[u8; 64]) -> Aes256Siv {
    Aes256Siv::new(key.into())
}

pub(crate) fn seal(
    key: &[u8; 64],
    parent: &uuid::Uuid,
    kind: NodeKind,
    name: &str,
) -> Result<Vec<u8>> {
    let name: Zeroizing<String> = Zeroizing::new(name.nfc().collect());
    if name.is_empty() {
        return Err(Error::Name("empty"));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(Error::Name("longer than 255 characters"));
    }
    let mut padded = Zeroizing::new(Vec::with_capacity(name.len() + PAD_TO));
    padded.extend_from_slice(name.as_bytes());
    padded.push(0x80);
    while padded.len() % PAD_TO != 0 {
        padded.push(0);
    }
    let sealed = cipher(key)
        .encrypt(
            [parent.as_bytes().as_slice(), kind.as_str().as_bytes()],
            &padded,
        )
        .map_err(|_| Error::Name("cannot be sealed"))?;
    debug_assert!(sealed.len() <= MAX_SEALED_LEN);
    Ok(sealed)
}

pub(crate) fn open(
    key: &[u8; 64],
    parent: &uuid::Uuid,
    kind: NodeKind,
    sealed: &[u8],
) -> Result<String> {
    if sealed.len() > MAX_SEALED_LEN {
        return Err(Error::SealedName);
    }
    let padded = Zeroizing::new(
        cipher(key)
            .decrypt(
                [parent.as_bytes().as_slice(), kind.as_str().as_bytes()],
                sealed,
            )
            .map_err(|_| Error::SealedName)?,
    );
    let end = padded
        .iter()
        .rposition(|&b| b != 0)
        .ok_or(Error::SealedName)?;
    if padded[end] != 0x80 || padded.len() % PAD_TO != 0 {
        return Err(Error::SealedName);
    }
    let name = std::str::from_utf8(&padded[..end]).map_err(|_| Error::SealedName)?;
    // Another client wrote it; still refuse what this one would never seal.
    if name.is_empty() || !is_nfc(name) || name.chars().count() > MAX_NAME_CHARS {
        return Err(Error::SealedName);
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 64] = [9; 64];
    const PARENT: uuid::Uuid = uuid::Uuid::from_bytes([1; 16]);
    const OTHER: uuid::Uuid = uuid::Uuid::from_bytes([2; 16]);

    #[test]
    fn round_trip_and_padding() {
        let sealed = seal(&KEY, &PARENT, NodeKind::File, "tax 2025.pdf").unwrap();
        assert_eq!(sealed.len(), PAD_TO + 16);
        assert_eq!(
            open(&KEY, &PARENT, NodeKind::File, &sealed).unwrap(),
            "tax 2025.pdf"
        );
        // 31 bytes still fit one block with the 0x80; 32 need two.
        assert_eq!(
            seal(&KEY, &PARENT, NodeKind::File, &"a".repeat(31))
                .unwrap()
                .len(),
            PAD_TO + 16
        );
        assert_eq!(
            seal(&KEY, &PARENT, NodeKind::File, &"a".repeat(32))
                .unwrap()
                .len(),
            2 * PAD_TO + 16
        );
    }

    #[test]
    fn deterministic_per_folder_and_kind() {
        let a = seal(&KEY, &PARENT, NodeKind::File, "a").unwrap();
        assert_eq!(a, seal(&KEY, &PARENT, NodeKind::File, "a").unwrap());
        assert_ne!(a, seal(&KEY, &OTHER, NodeKind::File, "a").unwrap());
        assert_ne!(a, seal(&KEY, &PARENT, NodeKind::Folder, "a").unwrap());
        assert!(open(&KEY, &OTHER, NodeKind::File, &a).is_err());
        assert!(open(&KEY, &PARENT, NodeKind::Folder, &a).is_err());
        assert!(open(&[8; 64], &PARENT, NodeKind::File, &a).is_err());
    }

    #[test]
    fn nfd_and_nfc_seal_the_same() {
        let nfc = seal(&KEY, &PARENT, NodeKind::File, "\u{e9}t\u{e9}").unwrap();
        let nfd = seal(&KEY, &PARENT, NodeKind::File, "e\u{301}te\u{301}").unwrap();
        assert_eq!(nfc, nfd);
        assert_eq!(
            open(&KEY, &PARENT, NodeKind::File, &nfd).unwrap(),
            "\u{e9}t\u{e9}"
        );
    }

    #[test]
    fn limits() {
        assert!(seal(&KEY, &PARENT, NodeKind::File, "").is_err());
        let longest = "😀".repeat(MAX_NAME_CHARS);
        let sealed = seal(&KEY, &PARENT, NodeKind::File, &longest).unwrap();
        assert_eq!(sealed.len(), MAX_SEALED_LEN);
        assert_eq!(
            open(&KEY, &PARENT, NodeKind::File, &sealed).unwrap(),
            longest
        );
        assert!(
            seal(
                &KEY,
                &PARENT,
                NodeKind::File,
                &"a".repeat(MAX_NAME_CHARS + 1)
            )
            .is_err()
        );
    }

    #[test]
    fn tampering_fails() {
        let mut sealed = seal(&KEY, &PARENT, NodeKind::File, "a").unwrap();
        sealed[0] ^= 1;
        assert!(open(&KEY, &PARENT, NodeKind::File, &sealed).is_err());
        assert!(open(&KEY, &PARENT, NodeKind::File, &[0; 4]).is_err());
    }
}
