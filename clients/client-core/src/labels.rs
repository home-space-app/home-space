use protocol::{
    Device, Node, PLATFORM_ANDROID, PLATFORM_IOS, PLATFORM_LINUX, PLATFORM_MACOS, PLATFORM_WEB,
    PLATFORM_WINDOWS,
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::format::human_size;

/// The name of the root folder.
pub const HOME: &str = "Home";

/// The name of the folder that receives restored files whose folder is gone.
pub const LOST_AND_FOUND: &str = "Lost & Found";

/// Marks the device the list is shown on.
pub const THIS_DEVICE: &str = "This device";

/// A device with the events websocket open.
pub const ONLINE: &str = "Online";

const LOCATION_SEPARATOR: &str = " / ";
const PLATFORMS: [(&str, &str); 6] = [
    (PLATFORM_LINUX, "Linux"),
    (PLATFORM_MACOS, "macOS"),
    (PLATFORM_WINDOWS, "Windows"),
    (PLATFORM_WEB, "Web browser"),
    (PLATFORM_ANDROID, "Android"),
    (PLATFORM_IOS, "iOS"),
];

/// What a device runs, e.g. `macOS`; a platform this client does not know is shown as is.
pub fn platform(platform: &str) -> &str {
    PLATFORMS
        .iter()
        .find(|(known, _)| *known == platform)
        .map_or(platform, |(_, label)| label)
}

/// Whether a device is online or signed in, and what it left unfinished, e.g.
/// `Signed in · 2 unfinished uploads`.
pub fn device_state(device: &Device) -> String {
    let state = if device.online {
        ONLINE
    } else if device.signed_in {
        "Signed in"
    } else {
        "Signed out"
    };
    match device.uploads {
        0 => state.to_owned(),
        1 => format!("{state} · 1 unfinished upload"),
        count => format!("{state} · {count} unfinished uploads"),
    }
}

/// The device behind `edited_on` (a node's or a version's), by name, or `this device`; `None`
/// when nothing is recorded or the device is not among `devices`.
pub fn edited_on(edited_on: Option<Uuid>, devices: &[Device]) -> Option<String> {
    let device = devices.iter().find(|device| Some(device.id) == edited_on)?;
    Some(if device.current {
        THIS_DEVICE.to_lowercase()
    } else {
        device.name.clone()
    })
}

/// The question asked before signing `device` out, saying what it cancels.
pub fn sign_out_question(device: &Device) -> String {
    let name = device.name.as_str();
    match device.uploads {
        0 => format!("Sign out “{name}”?"),
        1 => format!("Sign out “{name}” and cancel its unfinished upload?"),
        count => format!("Sign out “{name}” and cancel its {count} unfinished uploads?"),
    }
}

/// Where a search hit lives, e.g. `Home / Photos / 2026`.
pub fn location_label(location: &[String]) -> String {
    std::iter::once(HOME)
        .chain(location.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(LOCATION_SEPARATOR)
}

/// `1 file`, `2 files`.
pub fn files(count: usize) -> String {
    match count {
        1 => "1 file".to_owned(),
        _ => format!("{count} files"),
    }
}

/// A node's modification time and size, e.g. `2026-03-04 05:06 · 1.5 KB`; `short_date`
/// formats the time, since each client follows its platform's regional settings.
pub fn describe(node: &Node, short_date: impl Fn(OffsetDateTime) -> String) -> String {
    format!(
        "{} · {}",
        short_date(node.updated_at),
        human_size(node.size)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_start_at_home() {
        assert_eq!(location_label(&[]), "Home");
        assert_eq!(
            location_label(&["a".to_owned(), "b".to_owned()]),
            "Home / a / b"
        );
    }

    fn device(signed_in: bool, uploads: i64) -> Device {
        Device {
            id: Uuid::nil(),
            name: "Laptop".to_owned(),
            platform: PLATFORM_LINUX.to_owned(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            last_seen_at: OffsetDateTime::UNIX_EPOCH,
            signed_in,
            online: false,
            current: false,
            uploads,
            visibility: Default::default(),
            visible_to: Vec::new(),
        }
    }

    #[test]
    fn known_platforms_get_a_name_and_others_are_shown_as_is() {
        assert_eq!(platform(PLATFORM_MACOS), "macOS");
        assert_eq!(platform("desktop"), "desktop");
    }

    #[test]
    fn the_device_state_counts_unfinished_uploads() {
        assert_eq!(device_state(&device(true, 0)), "Signed in");
        assert_eq!(
            device_state(&device(false, 1)),
            "Signed out · 1 unfinished upload"
        );
        assert_eq!(
            device_state(&device(true, 3)),
            "Signed in · 3 unfinished uploads"
        );
    }

    #[test]
    fn an_online_device_says_so_before_its_uploads() {
        let online = Device {
            online: true,
            ..device(true, 1)
        };
        assert_eq!(device_state(&online), "Online · 1 unfinished upload");
    }

    #[test]
    fn an_edit_is_named_after_its_device() {
        let laptop = device(true, 0);
        let this = Device {
            id: Uuid::from_u128(1),
            current: true,
            ..device(true, 0)
        };
        let devices = [laptop, this];

        assert_eq!(
            edited_on(Some(Uuid::nil()), &devices).as_deref(),
            Some("Laptop")
        );
        assert_eq!(
            edited_on(Some(Uuid::from_u128(1)), &devices).as_deref(),
            Some("this device")
        );
        assert_eq!(edited_on(Some(Uuid::from_u128(2)), &devices), None);
        assert_eq!(edited_on(None, &devices), None);
    }

    #[test]
    fn signing_out_says_what_it_cancels() {
        assert_eq!(sign_out_question(&device(true, 0)), "Sign out “Laptop”?");
        assert_eq!(
            sign_out_question(&device(true, 2)),
            "Sign out “Laptop” and cancel its 2 unfinished uploads?"
        );
    }

    #[test]
    fn file_counts_agree_with_the_number() {
        assert_eq!(files(1), "1 file");
        assert_eq!(files(0), "0 files");
        assert_eq!(files(3), "3 files");
    }
}
