//! How a client fits the server it is signed in to: the same version, a different but
//! compatible one, or one that cannot fully work. Every client and the server's device list
//! use [`check`], so they agree.

use serde::{Deserialize, Serialize};

use crate::version::Version;

/// The result of [`check`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Compatibility {
    /// Client and server run the same version.
    Same,
    /// The versions differ, but the server has everything the client requires.
    Warning { newer: Newer },
    /// The client cannot fully work with this server.
    Error { reason: Incompatibility },
}

/// Which side of a [`Compatibility::Warning`] is newer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Newer {
    Server,
    Client,
    /// One version is missing or is not a release version, so they cannot be ordered.
    Unknown,
}

/// Why a client and server are a [`Compatibility::Error`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "features", rename_all = "snake_case")]
pub enum Incompatibility {
    /// Their major versions differ.
    MajorVersion,
    /// The client requires these features and the server does not have them.
    MissingFeatures(Vec<String>),
}

/// Compares a client with its server.
///
/// `server_version` is `None` for a server from before 0.13.0, which reports none; its
/// `server_features` are then empty. A version that does not parse never makes the result
/// an error by itself.
pub fn check<F, R>(
    server_version: Option<&str>,
    server_features: &[F],
    client_version: &str,
    client_requires: &[R],
) -> Compatibility
where
    F: AsRef<str>,
    R: AsRef<str>,
{
    if server_version == Some(client_version) {
        return Compatibility::Same;
    }
    let server = server_version.and_then(Version::parse);
    let client = Version::parse(client_version);
    let missing: Vec<String> = client_requires
        .iter()
        .map(AsRef::as_ref)
        .filter(|required| {
            !server_features
                .iter()
                .any(|feature| feature.as_ref() == *required)
        })
        .map(str::to_owned)
        .collect();
    match (&server, &client) {
        (Some(server), Some(client)) if server.major != client.major => Compatibility::Error {
            reason: Incompatibility::MajorVersion,
        },
        _ if !missing.is_empty() => Compatibility::Error {
            reason: Incompatibility::MissingFeatures(missing),
        },
        (Some(server), Some(client)) if server > client => Compatibility::Warning {
            newer: Newer::Server,
        },
        (Some(server), Some(client)) if server < client => Compatibility::Warning {
            newer: Newer::Client,
        },
        _ => Compatibility::Warning {
            newer: Newer::Unknown,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: &[&str] = &[];

    #[test]
    fn the_same_version_is_same() {
        assert_eq!(
            check(Some("0.13.0"), NONE, "0.13.0", NONE),
            Compatibility::Same
        );
    }

    #[test]
    fn a_newer_server_is_a_warning() {
        assert_eq!(
            check(Some("0.14.0"), NONE, "0.13.0", NONE),
            Compatibility::Warning {
                newer: Newer::Server
            }
        );
    }

    #[test]
    fn a_newer_client_with_everything_it_needs_is_a_warning() {
        assert_eq!(
            check(
                Some("0.13.0"),
                &["device_remove"],
                "0.13.2",
                &["device_remove"]
            ),
            Compatibility::Warning {
                newer: Newer::Client
            }
        );
    }

    #[test]
    fn a_missing_feature_is_an_error() {
        assert_eq!(
            check(Some("0.13.0"), &["a"], "0.14.0", &["a", "b"]),
            Compatibility::Error {
                reason: Incompatibility::MissingFeatures(vec!["b".to_owned()])
            }
        );
    }

    #[test]
    fn a_different_major_is_an_error_even_when_features_match() {
        assert_eq!(
            check(Some("2.0.0"), &["a"], "1.4.0", &["a"]),
            Compatibility::Error {
                reason: Incompatibility::MajorVersion
            }
        );
    }

    #[test]
    fn a_different_major_wins_over_missing_features() {
        assert_eq!(
            check(Some("2.0.0"), NONE, "1.4.0", &["a"]),
            Compatibility::Error {
                reason: Incompatibility::MajorVersion
            }
        );
    }

    #[test]
    fn a_server_without_a_version_is_an_unknown_warning() {
        assert_eq!(
            check(None, NONE, "0.13.0", NONE),
            Compatibility::Warning {
                newer: Newer::Unknown
            }
        );
    }

    #[test]
    fn a_server_without_a_version_is_an_error_once_the_client_requires_anything() {
        assert_eq!(
            check(None, NONE, "0.14.0", &["a"]),
            Compatibility::Error {
                reason: Incompatibility::MissingFeatures(vec!["a".to_owned()])
            }
        );
    }

    #[test]
    fn a_version_that_does_not_parse_is_at_worst_a_warning() {
        assert_eq!(
            check(Some("0.13.0"), NONE, "dev-1234", NONE),
            Compatibility::Warning {
                newer: Newer::Unknown
            }
        );
    }

    #[test]
    fn a_pre_release_of_the_same_numbers_is_older() {
        assert_eq!(
            check(Some("0.13.0"), NONE, "0.13.0-rc.1", NONE),
            Compatibility::Warning {
                newer: Newer::Server
            }
        );
    }

    #[test]
    fn serializes_with_a_state_tag() {
        let json = serde_json::to_value(Compatibility::Error {
            reason: Incompatibility::MissingFeatures(vec!["a".to_owned()]),
        })
        .expect("json");
        assert_eq!(
            json,
            serde_json::json!({"state": "error", "reason": {"kind": "missing_features", "features": ["a"]}})
        );
    }
}
