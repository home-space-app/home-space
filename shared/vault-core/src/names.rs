//! Sealed names: deterministic AES-SIV, so the server can still refuse two equal names in one
//! folder without reading them.
//!
//! `sealed = base64url(AES-SIV-256(name key, ad = [parent id, kind], pad(NFC(name))))`, without
//! padding characters. The padding inside is `0x80` and zeros up to a multiple of 32 bytes, so
//! the server learns a name's length only to that granularity. The server stores the text as
//! the node's name like any other: it fits its 255-character limit and holds neither `/` nor
//! `\`.

use aes_siv::aead::KeyInit;
use aes_siv::siv::Aes256Siv;
use data_encoding::BASE64URL_NOPAD;
use unicode_normalization::{UnicodeNormalization, is_nfc};
use zeroize::Zeroizing;

use crate::{Error, NameProblem, Result};

/// The server's limit for a node name, in characters, which a sealed name must fit.
pub const SERVER_NAME_CHARS: usize = 255;
/// Names are padded to a multiple of this many bytes.
pub const PAD_TO: usize = 32;
/// Most padded blocks a sealed name holds.
const MAX_BLOCKS: usize = 5;
/// AES-SIV's synthetic IV, which comes first in a sealed name.
const SIV_TAG_LEN: usize = 16;
/// Ends a name inside its padding.
const PAD_MARK: u8 = 0x80;
/// Longest sealed name in bytes: the 16-byte tag and five padded blocks, which in base64url is
/// 235 characters, the most that fits [`SERVER_NAME_CHARS`].
pub const MAX_SEALED_LEN: usize = MAX_BLOCKS * PAD_TO + SIV_TAG_LEN;
/// Longest name in a protected folder, in UTF-8 bytes after NFC: the padded blocks less the
/// `0x80` that ends it. Shorter than elsewhere (255 characters), the price of hiding names.
pub const MAX_NAME_BYTES: usize = MAX_BLOCKS * PAD_TO - 1;

/// File or folder; bound into the sealed name like the server's unique key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// A folder.
    Folder,
    /// A file.
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
) -> Result<String> {
    let name: Zeroizing<String> = Zeroizing::new(name.nfc().collect());
    if name.is_empty() {
        return Err(Error::Name(NameProblem::Empty));
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(Error::Name(NameProblem::TooLong));
    }
    let mut padded = Zeroizing::new(Vec::with_capacity(name.len() + PAD_TO));
    padded.extend_from_slice(name.as_bytes());
    padded.push(PAD_MARK);
    let padded_len = padded.len().next_multiple_of(PAD_TO);
    padded.resize(padded_len, 0);
    let sealed = cipher(key)
        .encrypt(
            [parent.as_bytes().as_slice(), kind.as_str().as_bytes()],
            &padded,
        )
        .map_err(|_| Error::Name(NameProblem::Unsealable))?;
    debug_assert!(sealed.len() <= MAX_SEALED_LEN);
    Ok(BASE64URL_NOPAD.encode(&sealed))
}

pub(crate) fn open(
    key: &[u8; 64],
    parent: &uuid::Uuid,
    kind: NodeKind,
    sealed: &str,
) -> Result<String> {
    let sealed = BASE64URL_NOPAD
        .decode(sealed.as_bytes())
        .map_err(|_| Error::SealedName)?;
    if sealed.len() > MAX_SEALED_LEN {
        return Err(Error::SealedName);
    }
    let padded = Zeroizing::new(
        cipher(key)
            .decrypt(
                [parent.as_bytes().as_slice(), kind.as_str().as_bytes()],
                sealed.as_slice(),
            )
            .map_err(|_| Error::SealedName)?,
    );
    let end = padded
        .iter()
        .rposition(|&b| b != 0)
        .ok_or(Error::SealedName)?;
    if padded[end] != PAD_MARK || padded.len() % PAD_TO != 0 {
        return Err(Error::SealedName);
    }
    let name = std::str::from_utf8(&padded[..end]).map_err(|_| Error::SealedName)?;
    // Another client wrote it; still refuse what this one would never seal.
    if name.is_empty() || !is_nfc(name) || name.len() > MAX_NAME_BYTES {
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

    fn sealed_len(name: &str) -> usize {
        let text = seal(&KEY, &PARENT, NodeKind::File, name).unwrap();
        BASE64URL_NOPAD.decode(text.as_bytes()).unwrap().len()
    }

    #[test]
    fn round_trip_and_padding() {
        let sealed = seal(&KEY, &PARENT, NodeKind::File, "tax 2025.pdf").unwrap();
        assert!(
            sealed
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_eq!(
            open(&KEY, &PARENT, NodeKind::File, &sealed).unwrap(),
            "tax 2025.pdf"
        );
        // 31 bytes still fit one block with the 0x80; 32 need two.
        assert_eq!(sealed_len("tax 2025.pdf"), PAD_TO + 16);
        assert_eq!(sealed_len(&"a".repeat(31)), PAD_TO + 16);
        assert_eq!(sealed_len(&"a".repeat(32)), 2 * PAD_TO + 16);
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
    fn the_longest_name_fits_the_server() {
        assert_eq!(
            seal(&KEY, &PARENT, NodeKind::File, "")
                .unwrap_err()
                .to_string(),
            "invalid name: empty"
        );
        let longest = "a".repeat(MAX_NAME_BYTES);
        let sealed = seal(&KEY, &PARENT, NodeKind::File, &longest).unwrap();
        assert_eq!(sealed.len(), 235);
        assert!(sealed.len() <= SERVER_NAME_CHARS);
        assert_eq!(
            open(&KEY, &PARENT, NodeKind::File, &sealed).unwrap(),
            longest
        );
        // 39 four-byte characters are 156 bytes; 40 are too many.
        assert!(seal(&KEY, &PARENT, NodeKind::File, &"😀".repeat(39)).is_ok());
        assert_eq!(
            seal(&KEY, &PARENT, NodeKind::File, &"😀".repeat(40))
                .unwrap_err()
                .to_string(),
            "invalid name: longer than 159 bytes"
        );
        assert!(
            seal(
                &KEY,
                &PARENT,
                NodeKind::File,
                &"a".repeat(MAX_NAME_BYTES + 1)
            )
            .is_err()
        );
    }

    #[test]
    fn tampering_fails() {
        let sealed = seal(&KEY, &PARENT, NodeKind::File, "a").unwrap();
        let mut bytes = BASE64URL_NOPAD.decode(sealed.as_bytes()).unwrap();
        bytes[0] ^= 1;
        let tampered = BASE64URL_NOPAD.encode(&bytes);
        assert!(open(&KEY, &PARENT, NodeKind::File, &tampered).is_err());
        assert!(open(&KEY, &PARENT, NodeKind::File, "AAAA").is_err());
        assert!(open(&KEY, &PARENT, NodeKind::File, "not base64!").is_err());
        assert!(open(&KEY, &PARENT, NodeKind::File, "tax.pdf").is_err());
    }
}
