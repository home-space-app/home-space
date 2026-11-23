//! The "New version available" toast: whether to offer the latest release, and its wording.
//! Fetching the release is `api_client::latest_release`; each client keeps the skipped
//! version and the "check for new versions" setting in its own device storage.

use protocol::Version;

/// The latest release that is not a draft or pre-release, from GitHub's API.
pub const RELEASES_API_URL: &str =
    "https://api.github.com/repos/home-space-app/home-space/releases/latest";

/// Where the web client sends administrators to update the server.
pub const UPDATE_GUIDE_URL: &str = "https://home-space.app/docs/#update";

/// The toast's title.
pub const TITLE: &str = "New version available";
/// Hides the toast until a newer release is out.
pub const SKIP: &str = "Skip";
/// Opens the release page (native clients).
pub const OPEN_WEB: &str = "Open web";
/// Opens the update guide (web client).
pub const HOW_TO_UPDATE: &str = "How to update";
/// The setting that turns the check off.
pub const CHECK_SETTING: &str = "Check for new versions";
/// What the setting does.
pub const CHECK_SETTING_HINT: &str =
    "When the app starts, ask GitHub for the latest release. Nothing else is sent.";
/// The iOS app has no download on the release page.
pub const IOS_NOTE: &str = "Update from the source with Xcode.";

/// Whether to show the toast: `latest` is newer than `installed`, and newer than the version
/// the user skipped, if any. A version that does not parse is never offered.
pub fn should_offer(installed: &str, latest: &str, skipped: Option<&str>) -> bool {
    let (Some(installed), Some(latest)) = (Version::parse(installed), Version::parse(latest))
    else {
        return false;
    };
    let past_skipped = skipped
        .and_then(Version::parse)
        .is_none_or(|skipped| latest > skipped);
    latest > installed && past_skipped
}

/// The toast's text in an app, e.g. `Home Space 0.14.0 is out. You have 0.13.0.`
pub fn app_message(latest: &str, installed: &str) -> String {
    format!("Home Space {latest} is out. You have {installed}.")
}

/// The toast's text in the web client, which runs the server's version.
pub fn server_message(latest: &str, installed: &str) -> String {
    format!("Home Space {latest} is out. This server runs {installed}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_release_is_offered() {
        assert!(should_offer("0.13.0", "0.14.0", None));
    }

    #[test]
    fn the_installed_release_is_not_offered() {
        assert!(!should_offer("0.14.0", "0.14.0", None));
    }

    #[test]
    fn an_older_release_is_not_offered() {
        assert!(!should_offer("0.15.0-dev", "0.14.0", None));
    }

    #[test]
    fn a_dev_build_is_offered_its_release() {
        assert!(should_offer("0.14.0-dev", "0.14.0", None));
    }

    #[test]
    fn a_skipped_release_is_not_offered() {
        assert!(!should_offer("0.13.0", "0.14.0", Some("0.14.0")));
    }

    #[test]
    fn a_release_newer_than_the_skipped_one_is_offered() {
        assert!(should_offer("0.13.0", "0.14.1", Some("0.14.0")));
    }

    #[test]
    fn a_tag_that_is_not_a_version_is_not_offered() {
        assert!(!should_offer("0.13.0", "nightly", None));
    }

    #[test]
    fn the_message_names_both_versions() {
        assert_eq!(
            app_message("0.14.0", "0.13.0"),
            "Home Space 0.14.0 is out. You have 0.13.0."
        );
    }
}
