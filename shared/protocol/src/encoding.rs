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

/// Bytes that travel as unpadded base64url text, such as wrapped keys and sealed names.
#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct Base64Url(Vec<u8>);

impl Base64Url {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<u8>> for Base64Url {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl std::fmt::Debug for Base64Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Base64Url({})", self)
    }
}

impl std::fmt::Display for Base64Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        f.write_str(&URL_SAFE_NO_PAD.encode(&self.0))
    }
}

impl serde::Serialize for Base64Url {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Base64Url {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        URL_SAFE_NO_PAD
            .decode(text.trim_end_matches('='))
            .map(Self)
            .map_err(|_| serde::de::Error::custom("expected base64url"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_round_trips_and_takes_padding() {
        let bytes = Base64Url::from(vec![0xfb, 0xff, 0x01]);
        let text = serde_json::to_string(&bytes).expect("encode");
        assert_eq!(text, "\"-_8B\"");
        let back: Base64Url = serde_json::from_str(&text).expect("decode");
        assert_eq!(back, bytes);
        let padded: Base64Url = serde_json::from_str("\"-_8=\"").expect("padded");
        assert_eq!(padded.as_bytes(), [0xfb, 0xff]);
        assert!(serde_json::from_str::<Base64Url>("\"not base64!\"").is_err());
    }

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
