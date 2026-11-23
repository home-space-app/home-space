//! This install as one of the user's devices: the server lists it by the id kept here, so it
//! stays the same device across sign-ins.

use std::{
    io::{self, ErrorKind},
    path::Path,
};

use uuid::Uuid;

use protocol::DeviceInfo;

use crate::session::data_dir;

const ID_FILE: &str = "device-id";
const HOSTNAME_FILES: [&str; 2] = ["/proc/sys/kernel/hostname", "/etc/hostname"];
const HOSTNAME_VARS: [&str; 2] = ["COMPUTERNAME", "HOSTNAME"];
const FALLBACK_NAME: &str = "Desktop";

/// This app's version, reported to the server and compared with its own and the latest
/// release's.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The server features (`protocol::features`) this app cannot work without. Add one only
/// when the app relies on it with no fallback for older servers; see `docs/versioning.md`.
pub const REQUIRES: &[&str] = &[];

#[cfg(target_os = "linux")]
const PLATFORM: &str = protocol::PLATFORM_LINUX;
#[cfg(target_os = "macos")]
const PLATFORM: &str = protocol::PLATFORM_MACOS;
#[cfg(target_os = "windows")]
const PLATFORM: &str = protocol::PLATFORM_WINDOWS;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
const PLATFORM: &str = "desktop";

/// This install, named after the computer; `None` when its id cannot be kept, so a new device
/// is not made up on every run.
pub fn info() -> Option<DeviceInfo> {
    let dir = data_dir()?;
    match load_or_create(&dir) {
        Ok(id) => Some(DeviceInfo {
            id,
            name: host_name(),
            platform: PLATFORM.to_owned(),
            version: Some(VERSION.to_owned()),
            requires: REQUIRES
                .iter()
                .map(|feature| (*feature).to_owned())
                .collect(),
        }),
        Err(err) => {
            eprintln!("could not keep the device id: {err}");
            None
        }
    }
}

/// The id saved in `dir`, saving a new one there first if there is none.
fn load_or_create(dir: &Path) -> io::Result<Uuid> {
    let path = dir.join(ID_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => text
            .trim()
            .parse()
            .map_err(|err| io::Error::new(ErrorKind::InvalidData, err)),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            let id = Uuid::new_v4();
            std::fs::create_dir_all(dir)?;
            std::fs::write(&path, id.to_string())?;
            Ok(id)
        }
        Err(err) => Err(err),
    }
}

/// The computer's name, as the system reports it.
fn host_name() -> String {
    let from_files = HOSTNAME_FILES
        .iter()
        .filter_map(|file| std::fs::read_to_string(file).ok());
    let from_vars = HOSTNAME_VARS
        .iter()
        .filter_map(|var| std::env::var(var).ok());
    computer_name()
        .into_iter()
        .chain(from_files)
        .chain(from_vars)
        .map(|name| name.trim().to_owned())
        .find(|name| !name.is_empty())
        .unwrap_or_else(|| FALLBACK_NAME.to_owned())
}

/// The name set in System Settings › General › Sharing, such as "Anna's MacBook": macOS has
/// no `/etc/hostname`, and its shells do not export `HOSTNAME`.
#[cfg(target_os = "macos")]
fn computer_name() -> Option<String> {
    const SCUTIL: &str = "scutil";
    const ARGS: [&str; 2] = ["--get", "ComputerName"];
    let output = std::process::Command::new(SCUTIL)
        .args(ARGS)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Other systems name the computer through the host name files and variables.
#[cfg(not(target_os = "macos"))]
fn computer_name() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_is_made_once_and_then_kept() {
        let dir = std::env::temp_dir().join(format!("home-space-device-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let first = load_or_create(&dir).expect("first");
        let second = load_or_create(&dir).expect("second");

        assert_eq!(first, second);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_id_file_is_an_error_not_a_new_device() {
        let dir =
            std::env::temp_dir().join(format!("home-space-device-damaged-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join(ID_FILE), "not a uuid").expect("write");

        assert!(load_or_create(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
