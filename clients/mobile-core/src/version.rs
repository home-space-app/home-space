//! How this app fits its server, and whether a newer release is out: what the account page's
//! version row, its sheet, the red banner and the "New version available" toast show.

use client_core::{
    update,
    versions::{self, Tone},
};
use protocol::{Compatibility, Incompatibility};

use crate::{CoreClient, off_ui_thread};

/// This app's version, which `scripts/release.sh` keeps equal to the Android `versionName`
/// and, without a pre-release suffix, the iOS `MARKETING_VERSION`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The server features (`protocol::features`) the Android and iOS apps cannot work without.
/// Add one only when the apps rely on it with no fallback for older servers; see
/// `docs/versioning.md`.
pub const REQUIRES: &[&str] = &[];

/// The colour of a version state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum VersionTone {
    /// Same version: green.
    Same,
    /// Different but compatible: yellow.
    Warning,
    /// Cannot fully work: red.
    Error,
}

impl From<Tone> for VersionTone {
    fn from(tone: Tone) -> Self {
        match tone {
            Tone::Same => Self::Same,
            Tone::Warning => Self::Warning,
            Tone::Error => Self::Error,
        }
    }
}

/// How this app fits the server it is signed in to.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct VersionCheck {
    pub app_version: String,
    /// `None` for a server from before 0.13.0, which does not report it.
    pub server_version: Option<String>,
    pub tone: VersionTone,
    /// E.g. `Up to date`.
    pub title: String,
    /// What the user can do about it, naming both versions.
    pub hint: String,
    /// E.g. `App 0.13.0 · Server 0.13.0`.
    pub chip: String,
    /// The features the server lacks, as the user reads them; empty unless red for them.
    pub missing_features: Vec<String>,
    /// The one line of the red banner; `None` unless red.
    pub banner: Option<String>,
}

/// A newer release to offer in the toast.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct UpdateOffer {
    /// E.g. `0.14.0`.
    pub version: String,
    /// The release page, where the Android app is attached.
    pub url: String,
    /// E.g. `Home Space 0.14.0 is out. You have 0.13.0.`
    pub message: String,
}

/// The fixed words of the toast and its setting.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct UpdateLabels {
    pub title: String,
    pub skip: String,
    pub open_web: String,
    pub check_setting: String,
    pub check_setting_hint: String,
    /// The iOS app has no download on the release page.
    pub ios_note: String,
}

/// This app's version.
#[uniffi::export]
pub fn app_version() -> String {
    VERSION.to_owned()
}

/// The fixed words of the "New version available" toast and its setting.
#[uniffi::export]
pub fn update_labels() -> UpdateLabels {
    UpdateLabels {
        title: update::TITLE.to_owned(),
        skip: update::SKIP.to_owned(),
        open_web: update::OPEN_WEB.to_owned(),
        check_setting: update::CHECK_SETTING.to_owned(),
        check_setting_hint: update::CHECK_SETTING_HINT.to_owned(),
        ios_note: update::IOS_NOTE.to_owned(),
    }
}

/// The latest release when it is newer than this app and than `skipped`, the version the
/// user skipped. Reads GitHub, or `releases_url` when given (tests). Any failure, such as
/// GitHub's rate limit, is `None`: nothing is shown, and the next start tries again.
#[uniffi::export]
pub async fn new_version(
    skipped: Option<String>,
    releases_url: Option<String>,
) -> Option<UpdateOffer> {
    let url = releases_url.unwrap_or_else(|| update::RELEASES_API_URL.to_owned());
    let release = off_ui_thread(async move { api_client::latest_release(&url).await }).await?;
    offer(release, skipped.as_deref())
}

fn offer(release: api_client::Release, skipped: Option<&str>) -> Option<UpdateOffer> {
    update::should_offer(VERSION, &release.version, skipped).then(|| UpdateOffer {
        message: update::app_message(&release.version, VERSION),
        version: release.version,
        url: release.url,
    })
}

#[uniffi::export]
impl CoreClient {
    /// How this app fits its server, from what the server reported at sign-in; `None` when
    /// that could not be read, so nothing is shown rather than a guess.
    pub fn version_check(&self) -> Option<VersionCheck> {
        let compatibility = self.inner.compatibility(VERSION, REQUIRES)?;
        Some(version_check(
            &compatibility,
            self.inner.server_version().as_deref(),
        ))
    }
}

fn version_check(compatibility: &Compatibility, server: Option<&str>) -> VersionCheck {
    let missing = match compatibility {
        Compatibility::Error {
            reason: Incompatibility::MissingFeatures(missing),
        } => missing
            .iter()
            .map(|feature| protocol::features::display_name(feature).to_owned())
            .collect(),
        _ => Vec::new(),
    };
    let banner = match compatibility {
        Compatibility::Error {
            reason: Incompatibility::MissingFeatures(missing),
        } => Some(format!(
            "{} won't work until the server is updated.",
            versions::missing_features(missing)
        )),
        Compatibility::Error { .. } => Some(versions::hint(compatibility, VERSION, server)),
        _ => None,
    };
    VersionCheck {
        app_version: VERSION.to_owned(),
        server_version: server.map(str::to_owned),
        tone: versions::tone(compatibility).into(),
        title: versions::title(compatibility).to_owned(),
        hint: versions::hint(compatibility, VERSION, server),
        chip: versions::chip(VERSION, server),
        missing_features: missing,
        banner,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str) -> api_client::Release {
        api_client::Release {
            version: version.to_owned(),
            url: "https://example.invalid/release".to_owned(),
        }
    }

    #[test]
    fn a_newer_release_is_offered_with_both_versions() {
        let offer = offer(release("999.0.0"), None).expect("offer");
        assert_eq!(
            offer.message,
            format!("Home Space 999.0.0 is out. You have {VERSION}.")
        );
    }

    #[test]
    fn a_skipped_release_is_not_offered() {
        assert_eq!(offer(release("999.0.0"), Some("999.0.0")), None);
    }

    #[test]
    fn red_for_missing_features_names_them_and_has_a_banner() {
        let check = version_check(
            &Compatibility::Error {
                reason: Incompatibility::MissingFeatures(vec!["device_remove".to_owned()]),
            },
            Some("0.12.3"),
        );
        assert_eq!(check.tone, VersionTone::Error);
        assert_eq!(check.missing_features, ["device_remove"]);
        assert!(check.banner.is_some());
    }

    #[test]
    fn same_has_no_banner() {
        let check = version_check(&Compatibility::Same, Some(VERSION));
        assert_eq!(check.tone, VersionTone::Same);
        assert_eq!(check.banner, None);
    }
}
