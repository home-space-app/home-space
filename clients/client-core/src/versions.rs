//! What every client shows about how it fits its server: the colour state, its title, the
//! hint, and the version chip. The state itself comes from [`protocol::compat::check`].

use protocol::{Compatibility, Device, Incompatibility, Newer, PLATFORM_WEB, features};

/// How the version of a server from before 0.13.0, which reports none, is shown.
pub const UNKNOWN_VERSION: &str = "unknown";

/// A device whose client has not reported its version since 0.13.0.
pub const DEVICE_VERSION_UNKNOWN: &str = "Unknown";

/// The last client version `device` reported, for the device list's Version column, e.g.
/// `0.13.0`. `None` for a web browser: it is served by the server, so it runs the server's
/// version.
pub fn device_version(device: &Device) -> Option<&str> {
    (device.platform != PLATFORM_WEB)
        .then(|| device.version.as_deref().unwrap_or(DEVICE_VERSION_UNKNOWN))
}

/// The colour `device`'s version is shown in, when it has a Version and the server knows how
/// it fits.
pub fn device_tone(device: &Device) -> Option<Tone> {
    device_version(device)
        .and(device.compatibility.as_ref())
        .map(tone)
}

/// The colour a [`Compatibility`] is shown in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Same version: green.
    Same,
    /// Different but compatible: yellow.
    Warning,
    /// Cannot fully work: red.
    Error,
}

/// The colour of `compatibility`.
pub fn tone(compatibility: &Compatibility) -> Tone {
    match compatibility {
        Compatibility::Same => Tone::Same,
        Compatibility::Warning { .. } => Tone::Warning,
        Compatibility::Error { .. } => Tone::Error,
    }
}

/// The short title of `compatibility`, e.g. `Up to date`.
pub fn title(compatibility: &Compatibility) -> &'static str {
    match compatibility {
        Compatibility::Same => "Up to date",
        Compatibility::Warning { .. } => "Versions differ",
        Compatibility::Error {
            reason: Incompatibility::MissingFeatures(_),
        } => "Server too old for this app",
        Compatibility::Error {
            reason: Incompatibility::MajorVersion,
        } => "Different major versions",
    }
}

/// The chip in the sidebar or account page, e.g. `App 0.13.0 · Server 0.13.0`.
pub fn chip(client_version: &str, server_version: Option<&str>) -> String {
    format!(
        "App {client_version} · Server {}",
        server_version.unwrap_or(UNKNOWN_VERSION)
    )
}

/// What the user can do about `compatibility`, naming both versions.
pub fn hint(
    compatibility: &Compatibility,
    client_version: &str,
    server_version: Option<&str>,
) -> String {
    let server = server_version.unwrap_or(UNKNOWN_VERSION);
    match compatibility {
        Compatibility::Same => format!("App and server are both {client_version}."),
        Compatibility::Warning { .. } if server_version.is_none() => format!(
            "The server does not report its version, so it is older than 0.13.0. \
             Everything this app ({client_version}) needs is supported. \
             Ask the administrator to update the server."
        ),
        Compatibility::Warning {
            newer: Newer::Server,
        } => format!(
            "Server is {server}, this app is {client_version}. Everything works; \
             update the app to get the newest features."
        ),
        Compatibility::Warning {
            newer: Newer::Client,
        } => format!(
            "Server is {server}, this app is {client_version}. Everything this app needs is \
             supported. Ask the administrator to update the server."
        ),
        Compatibility::Warning {
            newer: Newer::Unknown,
        } => format!(
            "Server is {server}, this app is {client_version}. Everything this app needs is \
             supported."
        ),
        Compatibility::Error {
            reason: Incompatibility::MissingFeatures(missing),
        } => format!(
            "The server ({server}) is too old for this app. These won't work until it is \
             updated: {}.",
            missing_features(missing)
        ),
        Compatibility::Error {
            reason: Incompatibility::MajorVersion,
        } => format!(
            "Server {server} and this app {client_version} are different major versions. \
             Many features may fail. {}",
            major_advice(client_version, server_version)
        ),
    }
}

/// The features a server lacks, as the user reads them, e.g. `Remove devices, Streaming`.
pub fn missing_features(missing: &[String]) -> String {
    missing
        .iter()
        .map(|feature| features::display_name(feature))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Which side to update after a major version mismatch: the older one.
fn major_advice(client_version: &str, server_version: Option<&str>) -> &'static str {
    let client = protocol::Version::parse(client_version);
    let server = server_version.and_then(protocol::Version::parse);
    match (client, server) {
        (Some(client), Some(server)) if client > server => {
            "Ask the administrator to update the server."
        }
        _ => "Update the app.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_names_the_one_version() {
        assert_eq!(
            hint(&Compatibility::Same, "0.13.0", Some("0.13.0")),
            "App and server are both 0.13.0."
        );
    }

    #[test]
    fn a_newer_server_asks_to_update_the_app() {
        let warning = Compatibility::Warning {
            newer: Newer::Server,
        };
        assert!(hint(&warning, "0.13.0", Some("0.14.0")).contains("update the app"));
        assert_eq!(tone(&warning), Tone::Warning);
    }

    #[test]
    fn a_server_without_a_version_says_it_is_older() {
        let warning = Compatibility::Warning {
            newer: Newer::Unknown,
        };
        assert!(hint(&warning, "0.13.0", None).contains("older than 0.13.0"));
    }

    #[test]
    fn missing_features_are_listed() {
        let error = Compatibility::Error {
            reason: Incompatibility::MissingFeatures(vec!["a".to_owned(), "b".to_owned()]),
        };
        assert_eq!(
            hint(&error, "0.14.0", Some("0.13.0")),
            "The server (0.13.0) is too old for this app. These won't work until it is \
             updated: a, b."
        );
        assert_eq!(title(&error), "Server too old for this app");
    }

    #[test]
    fn a_major_mismatch_asks_to_update_the_older_side() {
        let error = Compatibility::Error {
            reason: Incompatibility::MajorVersion,
        };
        assert!(hint(&error, "1.4.0", Some("2.0.0")).ends_with("Update the app."));
        assert!(hint(&error, "2.0.0", Some("1.4.0")).ends_with("update the server."));
    }

    #[test]
    fn the_chip_shows_an_unknown_server_version() {
        assert_eq!(chip("0.13.0", None), "App 0.13.0 · Server unknown");
    }

    fn device(platform: &str, version: Option<&str>) -> Device {
        Device {
            id: uuid::Uuid::nil(),
            name: "Phone".to_owned(),
            platform: platform.to_owned(),
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            last_seen_at: time::OffsetDateTime::UNIX_EPOCH,
            signed_in: true,
            online: false,
            current: false,
            uploads: 0,
            visibility: Default::default(),
            visible_to: Vec::new(),
            version: version.map(str::to_owned),
            compatibility: Some(Compatibility::Same),
        }
    }

    #[test]
    fn a_device_shows_the_last_version_it_reported() {
        let phone = device(protocol::PLATFORM_ANDROID, Some("0.13.0"));
        assert_eq!(device_version(&phone), Some("0.13.0"));
        assert_eq!(device_tone(&phone), Some(Tone::Same));
    }

    #[test]
    fn a_device_that_never_reported_a_version_shows_unknown() {
        let phone = device(protocol::PLATFORM_ANDROID, None);
        assert_eq!(device_version(&phone), Some(DEVICE_VERSION_UNKNOWN));
    }

    #[test]
    fn a_web_browser_shows_no_version() {
        let browser = device(PLATFORM_WEB, Some("0.13.0"));
        assert_eq!(device_version(&browser), None);
        assert_eq!(device_tone(&browser), None);
    }
}
