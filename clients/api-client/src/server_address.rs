//! Telling a server on the home network from one reached over the internet, where only
//! `https://` keeps the password and the tokens from whoever carries the traffic.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

const CLEARTEXT_SCHEME: &str = "http";
const LOCALHOST: &str = "localhost";
/// Endings of names that only resolve inside a home or private network. `.ts.net` names
/// belong to a Tailscale network, which encrypts the traffic itself.
const LOCAL_SUFFIXES: [&str; 7] = [
    ".localhost",
    ".local",
    ".lan",
    ".home",
    ".home.arpa",
    ".internal",
    ".ts.net",
];
/// 100.64.0.0/10, the shared address space that VPNs such as Tailscale hand out.
const SHARED_SPACE: (u8, u8) = (100, 64);
const SHARED_SPACE_MASK: u8 = 0b1100_0000;
/// fc00::/7, the private addresses of IPv6, and fe80::/10, its link-local ones.
const UNIQUE_LOCAL: (u16, u16) = (0xfc00, 0xfe00);
const LINK_LOCAL: (u16, u16) = (0xfe80, 0xffc0);

/// Whether `server` is an `http://` address outside the home network. Signing in there would
/// send the password and every token unencrypted across the internet, so the apps refuse it.
///
/// Plain `http://` stays allowed for this machine, for private and link-local addresses, and
/// for names that only a local network resolves (`nas`, `nas.local`, `files.lan`).
pub fn is_unprotected(server: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(server.trim()) else {
        return false;
    };
    url.scheme() == CLEARTEXT_SCHEME && !url.host_str().is_some_and(is_local_host)
}

fn is_local_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let address = host.trim_start_matches('[').trim_end_matches(']');
    match address.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => is_local_v4(address),
        Ok(IpAddr::V6(address)) => match address.to_ipv4_mapped() {
            Some(mapped) => is_local_v4(mapped),
            None => is_local_v6(address),
        },
        Err(_) => {
            host == LOCALHOST
                || !host.contains('.')
                || LOCAL_SUFFIXES.iter().any(|suffix| host.ends_with(suffix))
        }
    }
}

fn is_local_v4(address: Ipv4Addr) -> bool {
    let [first, second, ..] = address.octets();
    address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || (first == SHARED_SPACE.0 && second & SHARED_SPACE_MASK == SHARED_SPACE.1)
}

fn is_local_v6(address: Ipv6Addr) -> bool {
    let first = address.segments()[0];
    address.is_loopback()
        || first & UNIQUE_LOCAL.1 == UNIQUE_LOCAL.0
        || first & LINK_LOCAL.1 == LINK_LOCAL.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_is_fine_at_home() {
        for server in [
            "http://127.0.0.1:8080",
            "http://localhost:8080",
            "http://[::1]:8080",
            "http://192.168.1.20:8080",
            "http://10.0.2.2:8080",
            "http://172.16.4.1",
            "http://169.254.10.1",
            "http://100.101.102.103:8080",
            "http://[fd12:3456::1]:8080",
            "http://[fe80::1]",
            "http://[::ffff:192.168.1.20]",
            "http://nas",
            "http://nas:8080",
            "http://nas.local:8080",
            "http://NAS.Local.",
            "http://files.lan",
            "http://files.home.arpa",
            "http://nas.tailnet-name.ts.net",
        ] {
            assert!(!is_unprotected(server), "{server}");
        }
    }

    #[test]
    fn http_across_the_internet_is_unprotected() {
        for server in [
            "http://files.example.com",
            "http://files.example.com:8080",
            "  http://example.com/ ",
            "http://203.0.113.9:8080",
            "http://172.32.0.1",
            "http://100.128.0.1",
            "http://[2001:db8::1]",
            "http://[::ffff:203.0.113.9]",
            "http://local.example.com",
        ] {
            assert!(is_unprotected(server), "{server}");
        }
    }

    #[test]
    fn https_is_protected_anywhere() {
        for server in [
            "https://files.example.com",
            "https://203.0.113.9",
            "nonsense",
            "",
        ] {
            assert!(!is_unprotected(server), "{server}");
        }
    }
}
