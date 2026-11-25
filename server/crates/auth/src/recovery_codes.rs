//! Recovery codes: one-time codes for signing in when the authenticator app is lost. They are
//! random and long, so a SHA-256 hash keeps them as well as a password hash would.

use sha2::{Digest, Sha256};

/// How many codes a set has.
pub(crate) const CODES_PER_SET: usize = 10;
/// Letters and digits that can't be mistaken for one another: no `0`, `1`, `i`, `l` or `o`.
const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
const GROUP_LEN: usize = 5;

/// A new set of codes, each `xxxxx-xxxxx`.
pub(crate) fn new_set() -> Vec<String> {
    (0..CODES_PER_SET).map(|_| new_code()).collect()
}

fn new_code() -> String {
    // Bytes from 248 up are dropped, so that every character is as likely.
    let limit = u8::try_from(256 - 256 % ALPHABET.len()).unwrap_or(u8::MAX);
    let mut chars = Vec::with_capacity(2 * GROUP_LEN);
    while chars.len() < 2 * GROUP_LEN {
        let mut bytes = [0u8; 16];
        rand::fill(&mut bytes);
        chars.extend(
            bytes
                .iter()
                .filter(|&&byte| byte < limit)
                .map(|&byte| ALPHABET[usize::from(byte) % ALPHABET.len()])
                .take(2 * GROUP_LEN - chars.len()),
        );
    }
    let (first, second) = chars.split_at(GROUP_LEN);
    format!(
        "{}-{}",
        String::from_utf8_lossy(first),
        String::from_utf8_lossy(second)
    )
}

/// The hash a code is stored under. Case, spaces and dashes don't matter, so a code typed
/// as `ABCDE FGHIJ` is the same as `abcde-fghij`.
pub(crate) fn hash(code: &str) -> Vec<u8> {
    let normal: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(char::to_lowercase)
        .collect();
    Sha256::digest(normal.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn a_set_has_ten_different_codes_in_two_groups() {
        let codes = new_set();
        assert_eq!(codes.len(), CODES_PER_SET);
        assert_eq!(codes.iter().collect::<HashSet<_>>().len(), CODES_PER_SET);
        for code in &codes {
            let (first, second) = code.split_once('-').expect("two groups");
            assert_eq!((first.len(), second.len()), (GROUP_LEN, GROUP_LEN));
            assert!(
                code.bytes()
                    .all(|byte| byte == b'-' || ALPHABET.contains(&byte))
            );
        }
    }

    #[test]
    fn case_spaces_and_dashes_do_not_matter() {
        assert_eq!(hash("abcde-fghjk"), hash(" ABCDE FGHJK "));
        assert_eq!(hash("abcde-fghjk"), hash("abcdefghjk"));
        assert_ne!(hash("abcde-fghjk"), hash("abcde-fghjm"));
    }
}
