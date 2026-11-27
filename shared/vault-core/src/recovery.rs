//! The printed recovery key: 32 random bytes and a 2-byte checksum, shown as 11 groups of five
//! Base32 characters (`ABCDE-FGHIJ-…`).

use data_encoding::BASE32_NOPAD;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::secret::secret32;
use crate::{Error, Result};

secret32! {
    /// A recovery key: another unlocker of the vault, with no PRF.
    pub struct RecoveryKey;
}

/// Characters per group of the printed key.
const GROUP: usize = 5;
const KEY_LEN: usize = 32;
const CHECKSUM_LEN: usize = 2;
const CHECKSUM_DOMAIN: &[u8] = b"home-space recovery key v1\0";

fn checksum(bytes: &[u8; KEY_LEN]) -> [u8; CHECKSUM_LEN] {
    let hash: [u8; 32] = Sha256::new()
        .chain_update(CHECKSUM_DOMAIN)
        .chain_update(bytes)
        .finalize()
        .into();
    let [first, second, ..] = hash;
    [first, second]
}

impl RecoveryKey {
    /// A new random recovery key.
    pub fn generate() -> Self {
        Self::random()
    }

    /// The text to print, e.g. `ABCDE-FGHIJ-…` (55 characters in 11 groups).
    pub fn to_display(&self) -> Zeroizing<String> {
        let mut raw = Zeroizing::new(Vec::with_capacity(KEY_LEN + CHECKSUM_LEN));
        raw.extend_from_slice(self.expose());
        raw.extend_from_slice(&checksum(self.expose()));
        let encoded = Zeroizing::new(BASE32_NOPAD.encode(&raw));
        let mut out = Zeroizing::new(String::with_capacity(encoded.len() + encoded.len() / GROUP));
        for (i, c) in encoded.chars().enumerate() {
            if i > 0 && i % GROUP == 0 {
                out.push('-');
            }
            out.push(c);
        }
        out
    }

    /// Parses what the user typed: case, spaces and dashes do not matter, and the common
    /// look-alikes `0`, `1` and `8` (not in the Base32 alphabet) are read as `O`, `I` and `B`.
    pub fn parse(text: &str) -> Result<Self> {
        let cleaned: Zeroizing<String> = Zeroizing::new(
            text.chars()
                .filter(|c| !c.is_whitespace() && *c != '-')
                .map(|c| match c.to_ascii_uppercase() {
                    '0' => 'O',
                    '1' => 'I',
                    '8' => 'B',
                    c => c,
                })
                .collect(),
        );
        let raw = Zeroizing::new(
            BASE32_NOPAD
                .decode(cleaned.as_bytes())
                .map_err(|_| Error::RecoveryKey)?,
        );
        if raw.len() != KEY_LEN + CHECKSUM_LEN {
            return Err(Error::RecoveryKey);
        }
        let Some((bytes, typed_checksum)) = raw.split_first_chunk::<KEY_LEN>() else {
            return Err(Error::RecoveryKey);
        };
        if typed_checksum != checksum(bytes) {
            return Err(Error::RecoveryKey);
        }
        Ok(Self::from_bytes(*bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_parse_round_trip() {
        let key = RecoveryKey::generate();
        let text = key.to_display();
        assert_eq!(text.len(), 55 + 10);
        assert_eq!(text.split('-').count(), 11);
        let parsed = RecoveryKey::parse(&text).unwrap();
        assert_eq!(parsed.expose(), key.expose());
        let sloppy = text.to_lowercase().replace('-', " ");
        assert_eq!(RecoveryKey::parse(&sloppy).unwrap().expose(), key.expose());
    }

    #[test]
    fn typos_are_detected() {
        let key = RecoveryKey::from_bytes([5; 32]);
        let mut text: Vec<char> = key.to_display().chars().collect();
        text[3] = if text[3] == 'A' { 'C' } else { 'A' };
        let text: String = text.into_iter().collect();
        assert!(matches!(RecoveryKey::parse(&text), Err(Error::RecoveryKey)));
        assert!(matches!(
            RecoveryKey::parse("ABCDE"),
            Err(Error::RecoveryKey)
        ));
        assert!(matches!(
            RecoveryKey::parse("not base32 !!"),
            Err(Error::RecoveryKey)
        ));
    }
}
