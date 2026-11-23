//! The "New version available" choices kept on this computer: whether to check at all, and
//! the release the user skipped. Kept in `update.json` next to the session, never synced.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::session::data_dir;

const FILE: &str = "update.json";
/// Overrides where the latest release is read, for tests against a stub.
const URL_VAR: &str = "HOMESPACE_RELEASES_URL";

/// What the user chose about new versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdatePrefs {
    /// Whether to ask GitHub for the latest release when the app starts.
    #[serde(default = "checks_by_default")]
    pub check: bool,
    /// The release the user chose to skip; only a newer one is offered.
    #[serde(default)]
    pub skipped_version: Option<String>,
}

fn checks_by_default() -> bool {
    true
}

impl Default for UpdatePrefs {
    fn default() -> Self {
        Self {
            check: checks_by_default(),
            skipped_version: None,
        }
    }
}

impl UpdatePrefs {
    /// The saved choices; the defaults when there are none or they cannot be read.
    pub fn load() -> Self {
        path().map_or_else(Self::default, |path| load_from(&path))
    }

    /// Saves the choices.
    pub fn save(&self) -> Result<(), String> {
        let path = path().ok_or("no home folder to keep settings in")?;
        save_to(self, &path)
    }
}

/// Where the latest release is read: GitHub, or the stub [`URL_VAR`] names.
pub fn releases_url() -> String {
    std::env::var(URL_VAR).unwrap_or_else(|_| client_core::update::RELEASES_API_URL.to_owned())
}

fn path() -> Option<PathBuf> {
    Some(data_dir()?.join(FILE))
}

fn load_from(path: &Path) -> UpdatePrefs {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_to(prefs: &UpdatePrefs, path: &Path) -> Result<(), String> {
    let text = serde_json::to_string_pretty(prefs).map_err(|err| err.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    }
    std::fs::write(path, text).map_err(|err| format!("could not save {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("home-space-update-{name}-{}", std::process::id()))
            .join(FILE)
    }

    #[test]
    fn checks_until_told_otherwise() {
        assert!(load_from(&temp_file("missing")).check);
    }

    #[test]
    fn the_skipped_version_is_kept() {
        let path = temp_file("kept");
        let prefs = UpdatePrefs {
            check: false,
            skipped_version: Some("0.14.0".to_owned()),
        };

        save_to(&prefs, &path).expect("save");

        assert_eq!(load_from(&path), prefs);
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }
}
