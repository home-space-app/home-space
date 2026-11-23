//! Checks on configured URLs.

/// Whether `url` is https, or plain http to this machine for local development.
pub fn is_https_or_loopback(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    url.strip_prefix("http://").is_some_and(|rest| {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host = if authority.starts_with('[') {
            authority.split_inclusive(']').next().unwrap_or_default()
        } else {
            authority.split(':').next().unwrap_or_default()
        };
        matches!(host, "localhost" | "127.0.0.1" | "[::1]")
    })
}

#[cfg(test)]
mod tests {
    use super::is_https_or_loopback;

    #[test]
    fn https_and_loopback_http_are_accepted() {
        for url in [
            "https://idp.example.com",
            "http://localhost:8081/realms/x",
            "http://127.0.0.1:8080",
            "http://[::1]:8080/",
        ] {
            assert!(is_https_or_loopback(url), "{url}");
        }
    }

    #[test]
    fn other_http_is_rejected() {
        for url in [
            "http://idp.example.com",
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com/",
            "http://user@evil.com",
            "ftp://localhost",
            "",
        ] {
            assert!(!is_https_or_loopback(url), "{url}");
        }
    }
}
