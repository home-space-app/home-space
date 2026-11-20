//! Small text encodings the server and the clients both need.

use std::fmt::Write;

/// `value` with everything but RFC 3986 unreserved characters percent-encoded.
pub fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// `bytes` as lowercase hex, e.g. a SHA-256 digest as the API sends it.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// The host of `authority` (a host with an optional port), without an IPv6 address's brackets:
/// `example.com` from `example.com:8443`, `::1` from `[::1]:3000`.
pub fn authority_host(authority: &str) -> &str {
    match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unreserved_characters_stay_as_they_are() {
        assert_eq!(percent_encode("a-b.c_d~e"), "a-b.c_d~e");
        assert_eq!(percent_encode("a b/é"), "a%20b%2F%C3%A9");
    }

    #[test]
    fn bytes_turn_into_lowercase_hex() {
        assert_eq!(to_hex(&[0x00, 0xab, 0x7f]), "00ab7f");
        assert_eq!(to_hex(&[]), "");
    }

    #[test]
    fn the_host_drops_the_port_and_brackets() {
        assert_eq!(authority_host("example.com"), "example.com");
        assert_eq!(authority_host("example.com:8443"), "example.com");
        assert_eq!(authority_host("[::1]:3000"), "::1");
        assert_eq!(authority_host("[::1]"), "::1");
        assert_eq!(authority_host(""), "");
    }
}
