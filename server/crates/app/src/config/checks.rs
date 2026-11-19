//! Checks on configuration values.

use std::collections::HashSet;

use anyhow::Result;

const MIN_SECRET_LEN: usize = 32;
/// Fewer different characters than this in a secret means it was typed, not generated.
const MIN_SECRET_DISTINCT_CHARS: usize = 10;
/// What the example configuration used to ship as the secret, and so known to everyone.
const PLACEHOLDER_SECRET_MARK: &str = "change-me";
const GENERATE_SECRET: &str = "`openssl rand -base64 48`";

/// Refuses a signing secret that is short, a placeholder left in place, or made of a few
/// repeated characters: whoever can guess it can sign tokens.
pub(super) fn check_secret(name: &str, secret: &str) -> Result<()> {
    let distinct = secret.chars().collect::<HashSet<_>>().len();
    if secret.len() < MIN_SECRET_LEN {
        anyhow::bail!("{name} must be at least {MIN_SECRET_LEN} characters");
    } else if secret
        .to_ascii_lowercase()
        .contains(PLACEHOLDER_SECRET_MARK)
    {
        anyhow::bail!("{name} is still a placeholder; generate one with {GENERATE_SECRET}");
    } else if distinct < MIN_SECRET_DISTINCT_CHARS {
        anyhow::bail!("{name} is too easy to guess; generate one with {GENERATE_SECRET}");
    }
    Ok(())
}

/// The value of an optional setting, treating an empty one (as Compose passes) as unset.
pub(super) fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

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
    use super::{check_secret, is_https_or_loopback};

    #[test]
    fn a_generated_secret_is_accepted() {
        check_secret("secret", "kq3ZcV0m8Tn1yWfLr7uGxP2aHs9EoJd4BbNiC6tM5QzK").expect("accepted");
    }

    #[test]
    fn short_placeholder_and_repetitive_secrets_are_refused() {
        for secret in [
            "kq3ZcV0m8Tn1yWfLr7uGxP2aHs9Eo",
            "change-me-to-a-long-random-string",
            "Change-Me-kq3ZcV0m8Tn1yWfLr7uGxP2aHs9EoJd4",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "abcabcabcabcabcabcabcabcabcabcabcabcabc",
        ] {
            assert!(check_secret("secret", secret).is_err(), "{secret}");
        }
    }

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
