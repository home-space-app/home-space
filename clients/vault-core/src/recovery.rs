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

const GROUP: usize = 5;

fn checksum(bytes: &[u8; 32]) -> [u8; 2] {
    let hash = Sha256::digest([b"home-space recovery key v1\0".as_slice(), bytes].concat());
    [hash[0], hash[1]]
}

impl RecoveryKey {
    pub fn generate() -> Self {
        Self::random()
    }

    /// The text to print, e.g. `ABCDE-FGHIJ-…` (55 characters in 11 groups).
    pub fn to_display(&self) -> Zeroizing<String> {
        let mut raw = Zeroizing::new(Vec::with_capacity(34));
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
        if raw.len() != 34 {
            return Err(Error::RecoveryKey);
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&raw[..32]);
        let key = Self::from_bytes(bytes);
        zeroize::Zeroize::zeroize(&mut bytes);
        if checksum(key.expose()) != raw[32..] {
            return Err(Error::RecoveryKey);
        }
        Ok(key)
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
