//! Codes of authenticator apps (TOTP, RFC 6238): SHA-1, six digits, 30-second steps, which
//! every app handles.

use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use time::OffsetDateTime;

const SECRET_BYTES: usize = 20;
const STEP_SECONDS: i64 = 30;
const DIGITS: usize = 6;
const MODULUS: u32 = 1_000_000;
/// Steps on each side of the current one whose codes still count, for clocks that are off.
const DRIFT_STEPS: i64 = 1;

/// A new random secret of 160 bits.
pub(crate) fn new_secret() -> Vec<u8> {
    let mut secret = vec![0u8; SECRET_BYTES];
    rand::fill(secret.as_mut_slice());
    secret
}

/// The secret as people type it into an app: base32 without padding.
pub(crate) fn encode_secret(secret: &[u8]) -> String {
    BASE32_NOPAD.encode(secret)
}

/// The `otpauth://` address a QR code carries, labelled `issuer: account`.
pub(crate) fn otpauth_uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    let issuer = percent_encode(issuer);
    format!(
        "otpauth://totp/{issuer}:{}?secret={}&issuer={issuer}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECONDS}",
        percent_encode(account),
        encode_secret(secret),
    )
}

/// The time step `at` falls in.
pub(crate) fn step_at(at: OffsetDateTime) -> i64 {
    at.unix_timestamp().div_euclid(STEP_SECONDS)
}

/// The code of `secret` for `step` (HOTP, RFC 4226); `None` for a secret HMAC does not
/// take, which no secret of ours is.
fn code(secret: &[u8], step: i64) -> Option<u32> {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).ok()?;
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest.last()? & 0x0f);
    let bytes: [u8; 4] = digest.get(offset..offset + 4)?.try_into().ok()?;
    Some((u32::from_be_bytes(bytes) & 0x7fff_ffff) % MODULUS)
}

/// The step whose code `given` is, among the current step at `now` and one on each side;
/// `None` if it is none of them. Spaces in `given` are ignored, as apps show the code in two
/// groups. Every step is compared, so the time taken does not tell which one matched.
pub(crate) fn matching_step(secret: &[u8], given: &str, now: OffsetDateTime) -> Option<i64> {
    let digits: String = given.chars().filter(|c| !c.is_whitespace()).collect();
    if digits.len() != DIGITS || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let given: u32 = digits.parse().ok()?;
    let current = step_at(now);
    let mut found = None;
    for step in current - DRIFT_STEPS..=current + DRIFT_STEPS {
        if code(secret, step) == Some(given) {
            found = Some(step);
        }
    }
    found
}

/// Percent-encodes everything but unreserved characters (RFC 3986).
fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SHA-1 secret of RFC 6238's test vectors.
    const RFC_SECRET: &[u8] = b"12345678901234567890";

    fn at(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(seconds).expect("time")
    }

    #[test]
    fn codes_match_the_rfc_vectors() {
        // RFC 6238, appendix B, cut to six digits.
        for (seconds, expected) in [
            (59, 287_082),
            (1_111_111_109, 81_804),
            (1_111_111_111, 50_471),
            (1_234_567_890, 5_924),
            (2_000_000_000, 279_037),
        ] {
            assert_eq!(
                code(RFC_SECRET, step_at(at(seconds))),
                Some(expected),
                "{seconds}"
            );
        }
    }

    #[test]
    fn a_code_counts_one_step_either_side_and_with_spaces() {
        let now = at(1_111_111_111);
        let step = step_at(now);
        let code_of = |step| format!("{:06}", code(RFC_SECRET, step).expect("code"));
        assert_eq!(matching_step(RFC_SECRET, &code_of(step), now), Some(step));
        assert_eq!(
            matching_step(RFC_SECRET, &code_of(step - 1), now),
            Some(step - 1)
        );
        assert_eq!(
            matching_step(RFC_SECRET, &code_of(step + 1), now),
            Some(step + 1)
        );
        assert_eq!(matching_step(RFC_SECRET, &code_of(step - 2), now), None);
        let spaced = format!("{} {}", &code_of(step)[..3], &code_of(step)[3..]);
        assert_eq!(matching_step(RFC_SECRET, &spaced, now), Some(step));
        assert_eq!(matching_step(RFC_SECRET, "12345", now), None);
        assert_eq!(matching_step(RFC_SECRET, "abcdef", now), None);
    }

    #[test]
    fn the_address_names_the_account_and_carries_the_secret() {
        let uri = otpauth_uri("Home Space", "alice@files.example", RFC_SECRET);
        assert_eq!(
            uri,
            "otpauth://totp/Home%20Space:alice%40files.example\
             ?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Home%20Space\
             &algorithm=SHA1&digits=6&period=30"
        );
        assert_eq!(new_secret().len(), SECRET_BYTES);
    }
}
