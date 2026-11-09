//! Building a transfer from local files, and the words of the send dialogs.

use std::collections::BTreeSet;

use protocol::{NewTransferItem, NodeKind, TargetDevice, TransferTarget};

use crate::{
    format::human_size,
    labels::{files, platform},
};

/// A file or folder picked on this computer, by its path in the transfer: `a.pdf`, or
/// `Trip 2026/day 1/a.jpg` for a file inside a picked folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalEntry {
    pub path: String,
    pub kind: NodeKind,
    pub size: u64,
}

/// The first segment of `path`: the name shown in the items list.
pub fn top_name(path: &str) -> &str {
    path.split('/').next().unwrap_or(path)
}

/// `name`, or the first of `name (2)`, `name (3)`… that is not in `taken`; the number goes
/// before a file's extension.
pub fn free_name(name: &str, taken: &BTreeSet<String>) -> String {
    if !taken.contains(name) {
        return name.to_owned();
    }
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    (2..)
        .map(|n| format!("{stem} ({n}){extension}"))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| name.to_owned())
}

/// `path` with its first segment replaced by `top`.
pub fn with_top(path: &str, top: &str) -> String {
    match path.split_once('/') {
        Some((_, rest)) => format!("{top}/{rest}"),
        None => top.to_owned(),
    }
}

/// The items of a transfer of `entries`: every folder on the way to a file is listed once,
/// before what it holds, as the server requires.
pub fn manifest(entries: &[LocalEntry]) -> Vec<NewTransferItem> {
    let mut folders = BTreeSet::new();
    for entry in entries {
        let segments: Vec<&str> = entry.path.split('/').collect();
        let parents = match entry.kind {
            NodeKind::Folder => segments.len(),
            NodeKind::File => segments.len() - 1,
        };
        for depth in 1..=parents {
            folders.insert(segments[..depth].join("/"));
        }
    }
    let folder_items = folders.into_iter().map(|path| NewTransferItem {
        path,
        kind: NodeKind::Folder,
        size: 0,
        node_id: None,
        mime: None,
        modified_at: None,
    });
    let file_items = entries
        .iter()
        .filter(|entry| entry.kind == NodeKind::File)
        .map(|entry| NewTransferItem {
            path: entry.path.clone(),
            kind: NodeKind::File,
            size: entry.size,
            node_id: None,
            mime: None,
            modified_at: None,
        });
    // Sorted by path, a folder comes before everything inside it.
    let mut items: Vec<_> = folder_items.chain(file_items).collect();
    items.sort_by(|a, b| a.path.cmp(&b.path));
    items
}

/// What is being sent, counted: `1 folder and 2 files · 121.3 MB`.
pub fn items_summary(folders: usize, file_count: usize, bytes: u64) -> String {
    let size = human_size(i64::try_from(bytes).unwrap_or(i64::MAX));
    let counted = match (folders, file_count) {
        (0, files_only) => files(files_only),
        (1, 0) => "1 folder".to_owned(),
        (folders, 0) => format!("{folders} folders"),
        (1, file_count) => format!("1 folder and {}", files(file_count)),
        (folders, file_count) => format!("{folders} folders and {}", files(file_count)),
    };
    format!("{counted} · {size}")
}

/// The title of the dialog that sends server items: `Send report.pdf to a device`.
pub fn send_title(names: &[String]) -> String {
    match names {
        [only] => format!("Send {only} to a device"),
        _ => format!("Send {} items to a device", names.len()),
    }
}

/// The names of the items being sent, as a subtitle: `report.pdf, receipt-scan.jpg`.
pub fn names_line(names: &[String]) -> String {
    const SHOWN: usize = 3;
    match names.len() {
        0..=SHOWN => names.join(", "),
        more => format!("{} and {} more", names[..SHOWN].join(", "), more - SHOWN),
    }
}

/// How a device is doing: `Android · online`, or `iOS · signed out, gets it on next sign-in`.
pub fn device_status(device: &TargetDevice) -> String {
    let state = if device.online {
        "online"
    } else if device.signed_in {
        "offline"
    } else {
        "signed out, gets it on next sign-in"
    };
    format!("{} · {state}", platform(&device.platform))
}

/// The devices of `targets` the user can pick: everything but the device sending.
pub fn pickable(
    targets: &[TransferTarget],
    own: bool,
) -> Vec<(&TransferTarget, Vec<&TargetDevice>)> {
    targets
        .iter()
        .filter(|target| target.own == own)
        .map(|target| {
            let devices = target
                .devices
                .iter()
                .filter(|device| !device.current)
                .collect();
            (target, devices)
        })
        .filter(|(_, devices): &(_, Vec<_>)| !devices.is_empty())
        .collect()
}

/// The device picked first: the user's own device that is online, else their first one.
pub fn first_choice(targets: &[TransferTarget]) -> Option<(&TransferTarget, &TargetDevice)> {
    let own: Vec<_> = pickable(targets, true)
        .into_iter()
        .flat_map(|(target, devices)| devices.into_iter().map(move |device| (target, device)))
        .collect();
    own.iter()
        .find(|(_, device)| device.online)
        .or_else(|| own.first())
        .copied()
}

/// Who gets it, as the send button says it: `Pixel 9`, or `dana’s Phone`.
pub fn recipient_name(target: &TransferTarget, device: &TargetDevice) -> String {
    if target.own || names_owner(&device.name, &target.user.username) {
        device.name.clone()
    } else {
        format!("{}’s {}", target.user.username, device.name)
    }
}

/// Whether a device name already says whose it is, as in `dana's MacBook`.
fn names_owner(device: &str, owner: &str) -> bool {
    let device = device.to_lowercase();
    let owner = owner.to_lowercase();
    ["'s ", "’s "]
        .iter()
        .any(|suffix| device.starts_with(&format!("{owner}{suffix}")))
}

/// What happens after sending server items.
pub fn send_note(target: &TransferTarget, device: &TargetDevice) -> String {
    if target.own {
        format!(
            "The server keeps a copy until {} downloads it or it expires.",
            device.name
        )
    } else {
        format!(
            "{} gets a notification on {}. The copy is kept until it is downloaded or expires.",
            target.user.username, device.name
        )
    }
}

/// What happens after sending files from this computer.
pub fn upload_note(target: &TransferTarget, device: &TargetDevice) -> String {
    format!(
        "Keep this tab open until the upload finishes. {} is notified when it is complete; the copy is kept until it is downloaded or expires.",
        recipient_name(target, device)
    )
}

#[cfg(test)]
mod tests {
    use protocol::{UserId, UserRef};
    use uuid::Uuid;

    use super::*;

    fn file(path: &str, size: u64) -> LocalEntry {
        LocalEntry {
            path: path.to_owned(),
            kind: NodeKind::File,
            size,
        }
    }

    fn device(name: &str, online: bool, signed_in: bool, current: bool) -> TargetDevice {
        TargetDevice {
            id: Uuid::nil(),
            name: name.to_owned(),
            platform: "android".to_owned(),
            signed_in,
            online,
            current,
        }
    }

    fn target(name: &str, own: bool, devices: Vec<TargetDevice>) -> TransferTarget {
        TransferTarget {
            user: UserRef {
                id: UserId(Uuid::nil()),
                username: name.to_owned(),
            },
            own,
            devices,
        }
    }

    #[test]
    fn a_taken_name_gets_a_number_before_its_extension() {
        let taken = BTreeSet::from([
            "a.pdf".to_owned(),
            "a (2).pdf".to_owned(),
            "Trip".to_owned(),
        ]);
        assert_eq!(free_name("b.pdf", &taken), "b.pdf");
        assert_eq!(free_name("a.pdf", &taken), "a (3).pdf");
        assert_eq!(free_name("Trip", &taken), "Trip (2)");
        assert_eq!(
            free_name(".env", &BTreeSet::from([".env".to_owned()])),
            ".env (2)"
        );
        assert_eq!(with_top("Trip/day/a.jpg", "Trip (2)"), "Trip (2)/day/a.jpg");
        assert_eq!(with_top("a.pdf", "a (2).pdf"), "a (2).pdf");
        assert_eq!(top_name("Trip/day/a.jpg"), "Trip");
    }

    #[test]
    fn the_manifest_lists_each_folder_once_before_its_files() {
        let entries = [
            file("Trip/day 2/b.jpg", 2),
            file("Trip/a.jpg", 1),
            file("Trip/day 2/c.jpg", 3),
            file("notes.txt", 4),
            LocalEntry {
                path: "Trip/empty".to_owned(),
                kind: NodeKind::Folder,
                size: 0,
            },
        ];
        let items = manifest(&entries);
        let listed: Vec<_> = items
            .iter()
            .map(|item| (item.path.as_str(), item.kind))
            .collect();
        assert_eq!(
            listed,
            [
                ("Trip", NodeKind::Folder),
                ("Trip/a.jpg", NodeKind::File),
                ("Trip/day 2", NodeKind::Folder),
                ("Trip/day 2/b.jpg", NodeKind::File),
                ("Trip/day 2/c.jpg", NodeKind::File),
                ("Trip/empty", NodeKind::Folder),
                ("notes.txt", NodeKind::File),
            ]
        );
        assert_eq!(items[1].size, 1);
    }

    #[test]
    fn the_dialog_counts_and_names_what_is_sent() {
        assert_eq!(items_summary(1, 2, 1536), "1 folder and 2 files · 1.5 KB");
        assert_eq!(items_summary(0, 1, 0), "1 file · 0 B");
        assert_eq!(items_summary(2, 0, 0), "2 folders · 0 B");
        let names: Vec<String> = ["a", "b", "c", "d", "e"].map(str::to_owned).into();
        assert_eq!(send_title(&names[..1]), "Send a to a device");
        assert_eq!(send_title(&names[..2]), "Send 2 items to a device");
        assert_eq!(names_line(&names[..2]), "a, b");
        assert_eq!(names_line(&names), "a, b, c and 2 more");
    }

    #[test]
    fn devices_say_whether_they_get_it_now() {
        assert_eq!(
            device_status(&device("P", true, true, false)),
            "Android · online"
        );
        assert_eq!(
            device_status(&device("P", false, true, false)),
            "Android · offline"
        );
        assert_eq!(
            device_status(&device("P", false, false, false)),
            "Android · signed out, gets it on next sign-in"
        );
    }

    #[test]
    fn the_first_choice_is_an_online_device_of_ones_own_but_not_this_one() {
        let targets = vec![
            target(
                "sam",
                true,
                vec![
                    device("This browser", true, true, true),
                    device("iPad", false, false, false),
                    device("Pixel 9", true, true, false),
                ],
            ),
            target("dana", false, vec![device("Phone", true, true, false)]),
        ];
        let (who, picked) = first_choice(&targets).expect("a device");
        assert_eq!(picked.name, "Pixel 9");
        assert_eq!(recipient_name(who, picked), "Pixel 9");
        assert_eq!(pickable(&targets, true)[0].1.len(), 2);
        let (dana, devices) = &pickable(&targets, false)[0];
        assert_eq!(recipient_name(dana, devices[0]), "dana’s Phone");
        assert!(send_note(dana, devices[0]).starts_with("dana gets a notification on Phone."));
        assert!(first_choice(&targets[1..]).is_none());
        let named = target(
            "dana",
            false,
            vec![device("Dana's MacBook", true, true, false)],
        );
        assert_eq!(recipient_name(&named, &named.devices[0]), "Dana's MacBook");
    }
}
