//! Words for device visibility and transfers, shared by the clients.

use protocol::{Device, DeviceVisibility, TransferState, TransferSummary, UserId};
use time::{Duration, OffsetDateTime};

use crate::{format::human_size, labels::files, uri_list::is_uri_list};

/// The visibilities in the order the clients offer them.
pub const VISIBILITIES: [DeviceVisibility; 4] = [
    DeviceVisibility::Owner,
    DeviceVisibility::Hidden,
    DeviceVisibility::Users,
    DeviceVisibility::Everyone,
];

/// The name of a visibility, as a choice.
pub fn visibility_title(visibility: DeviceVisibility) -> &'static str {
    match visibility {
        DeviceVisibility::Owner => "Only you",
        DeviceVisibility::Hidden => "Hidden",
        DeviceVisibility::Users => "Visible to specific users",
        DeviceVisibility::Everyone => "Visible to everyone",
    }
}

/// What a visibility means, under its name.
pub fn visibility_hint(visibility: DeviceVisibility) -> &'static str {
    match visibility {
        DeviceVisibility::Owner => "Only you can send files to it. New devices start here.",
        DeviceVisibility::Hidden => "Nobody can send files to it, you included.",
        DeviceVisibility::Users => "Only these people can see it and send files to it.",
        DeviceVisibility::Everyone => "Everyone with an account on this server.",
    }
}

/// Who can send to a device, for a list: `Only you`, `Hidden`, `Everyone` or the names.
pub fn visible_to(device: &Device) -> String {
    match device.visibility {
        DeviceVisibility::Owner => "Only you".to_owned(),
        DeviceVisibility::Hidden => "Hidden".to_owned(),
        DeviceVisibility::Everyone => "Everyone".to_owned(),
        DeviceVisibility::Users if device.visible_to.is_empty() => "Only you".to_owned(),
        DeviceVisibility::Users => device
            .visible_to
            .iter()
            .map(|user| user.username.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// What a transfer holds: `report.pdf`, `Photos — 124 files`, `3 files` or `Link`.
pub fn transfer_name(transfer: &TransferSummary) -> String {
    if is_uri_list(transfer) {
        return "Link".to_owned();
    }
    match &transfer.name {
        Some(name) if transfer.item_count == 1 => name.clone(),
        Some(name) => format!("{name} — {}", files(transfer.file_count as usize)),
        None => files(transfer.file_count as usize),
    }
}

/// The size of a transfer, e.g. `12.0 MB`.
pub fn transfer_size(transfer: &TransferSummary) -> String {
    human_size(i64::try_from(transfer.total_bytes).unwrap_or(i64::MAX))
}

/// Where an incoming transfer comes from: `From Pixel 9`, or `From dana · MacBook` when another
/// user sent it.
pub fn transfer_from(transfer: &TransferSummary, me: UserId) -> String {
    let device = transfer.sender_device_name.as_deref();
    match (transfer.sender.id == me, device) {
        (true, Some(device)) => format!("From {device}"),
        (true, None) => "From you".to_owned(),
        (false, Some(device)) => format!("From {} · {device}", transfer.sender.username),
        (false, None) => format!("From {}", transfer.sender.username),
    }
}

/// Where an outgoing transfer goes: `To Pixel 9`, or `To dana · Phone`.
pub fn transfer_to(transfer: &TransferSummary, me: UserId) -> String {
    let device = &transfer.recipient_device_name;
    if transfer.recipient.id == me {
        format!("To {device}")
    } else {
        format!("To {} · {device}", transfer.recipient.username)
    }
}

/// How long until `at`, e.g. `in 5 h`, `in 40 min`, `in 2 days`.
pub fn time_left(at: OffsetDateTime, now: OffsetDateTime) -> String {
    let left = at - now;
    if left <= Duration::ZERO {
        "now".to_owned()
    } else if left < Duration::HOUR {
        format!("in {} min", left.whole_minutes().max(1))
    } else if left < Duration::DAY * 2 {
        format!("in {} h", left.whole_hours())
    } else {
        format!("in {} days", left.whole_days())
    }
}

/// The headline of a transfer that just arrived: `dana’s MacBook sent you 3 files`.
pub fn received_title(transfer: &TransferSummary, me: UserId) -> String {
    let what = if is_uri_list(transfer) {
        "a link".to_owned()
    } else {
        transfer_name(transfer)
    };
    let device = transfer.sender_device_name.as_deref();
    match (transfer.sender.id == me, device) {
        (true, Some(device)) => format!("{device} sent you {what}"),
        (true, None) => format!("You were sent {what}"),
        (false, Some(device)) => {
            format!("{}’s {device} sent you {what}", transfer.sender.username)
        }
        (false, None) => format!("{} sent you {what}", transfer.sender.username),
    }
}

/// The line under [`received_title`]: `12.0 MB · downloads as one zip · expires in 23 h`.
pub fn received_detail(transfer: &TransferSummary, now: OffsetDateTime) -> String {
    let size = transfer_size(transfer);
    let left = time_left(transfer.expires_at, now);
    if is_uri_list(transfer) {
        format!("Open it here · expires {left}")
    } else if is_single_file(transfer) {
        format!("{size} · expires {left}")
    } else {
        format!("{size} · downloads as one zip · expires {left}")
    }
}

/// Whether a transfer is one file, downloaded as is; anything else comes as a zip.
pub fn is_single_file(transfer: &TransferSummary) -> bool {
    transfer.item_count == 1 && transfer.file_count == 1
}

/// The file name a browser saves a transfer under: its one file, or the zip the server makes.
pub fn download_name(transfer: &TransferSummary) -> String {
    let base = match &transfer.name {
        Some(name) => name.rsplit('/').next().unwrap_or(name).to_owned(),
        None => format!("From {}", transfer.sender.username),
    };
    if is_single_file(transfer) {
        base
    } else {
        format!("{base}.zip")
    }
}

/// The state of a transfer, e.g. `Waiting · 23 h left` or `Downloaded`.
pub fn transfer_state(
    state: TransferState,
    expires_at: OffsetDateTime,
    now: OffsetDateTime,
) -> String {
    match state {
        TransferState::Uploading => "Uploading".to_owned(),
        TransferState::Ready => {
            let left = time_left(expires_at, now);
            match left.strip_prefix("in ") {
                Some(left) => format!("Waiting · {left} left"),
                None => "Waiting".to_owned(),
            }
        }
        TransferState::Declined => "Declined".to_owned(),
        TransferState::Downloaded => "Downloaded".to_owned(),
        TransferState::Saved => "Saved".to_owned(),
        TransferState::Cancelled => "Cancelled".to_owned(),
        TransferState::Expired => "Expired".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use protocol::{PLATFORM_LINUX, UserRef};
    use time::macros::datetime;
    use uuid::Uuid;

    use super::*;

    fn user(id: u128, name: &str) -> UserRef {
        UserRef {
            id: UserId(Uuid::from_u128(id)),
            username: name.to_owned(),
        }
    }

    fn summary(name: Option<&str>, items: u32, files: u32) -> TransferSummary {
        TransferSummary {
            id: Uuid::nil(),
            sender: user(1, "sam"),
            sender_device: None,
            sender_device_name: Some("Laptop".to_owned()),
            recipient: user(2, "dana"),
            recipient_device: Uuid::nil(),
            recipient_device_name: "Phone".to_owned(),
            state: TransferState::Ready,
            total_bytes: 1536,
            item_count: items,
            file_count: files,
            name: name.map(str::to_owned),
            mime: None,
            message: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            expires_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn a_transfer_is_named_after_what_is_at_its_top() {
        assert_eq!(transfer_name(&summary(Some("a.pdf"), 1, 1)), "a.pdf");
        assert_eq!(
            transfer_name(&summary(Some("Photos"), 125, 124)),
            "Photos — 124 files"
        );
        assert_eq!(transfer_name(&summary(None, 3, 3)), "3 files");
        let link = TransferSummary {
            mime: Some("text/uri-list".to_owned()),
            ..summary(Some("Pixel_2026-11-12.uris"), 1, 1)
        };
        assert_eq!(transfer_name(&link), "Link");
        assert!(received_title(&link, link.sender.id).ends_with("sent you a link"));
        assert_eq!(transfer_size(&summary(None, 3, 3)), "1.5 KB");
    }

    #[test]
    fn the_other_user_is_named_on_either_side() {
        let transfer = summary(None, 1, 1);
        assert_eq!(
            transfer_from(&transfer, UserId(Uuid::from_u128(1))),
            "From Laptop"
        );
        assert_eq!(
            transfer_from(&transfer, UserId(Uuid::from_u128(2))),
            "From sam · Laptop"
        );
        assert_eq!(
            transfer_to(&transfer, UserId(Uuid::from_u128(1))),
            "To dana · Phone"
        );
        assert_eq!(
            transfer_to(&transfer, UserId(Uuid::from_u128(2))),
            "To Phone"
        );
    }

    #[test]
    fn an_arrival_says_who_sent_what_and_how_it_downloads() {
        let now = datetime!(2026-11-09 10:00 UTC);
        let mut transfer = summary(None, 3, 3);
        transfer.expires_at = now + Duration::hours(23);
        assert_eq!(
            received_title(&transfer, UserId(Uuid::from_u128(2))),
            "sam’s Laptop sent you 3 files"
        );
        assert_eq!(
            received_title(&transfer, UserId(Uuid::from_u128(1))),
            "Laptop sent you 3 files"
        );
        assert_eq!(
            received_detail(&transfer, now),
            "1.5 KB · downloads as one zip · expires in 23 h"
        );
        assert_eq!(download_name(&transfer), "From sam.zip");

        let mut file = summary(Some("a.pdf"), 1, 1);
        file.expires_at = now + Duration::hours(23);
        assert_eq!(received_detail(&file, now), "1.5 KB · expires in 23 h");
        assert_eq!(download_name(&file), "a.pdf");
        assert_eq!(download_name(&summary(Some("Photos"), 4, 3)), "Photos.zip");
    }

    #[test]
    fn time_left_rounds_down_to_a_readable_unit() {
        let now = datetime!(2026-11-09 10:00 UTC);
        assert_eq!(time_left(now + Duration::minutes(40), now), "in 40 min");
        assert_eq!(time_left(now + Duration::hours(5), now), "in 5 h");
        assert_eq!(time_left(now + Duration::days(3), now), "in 3 days");
        assert_eq!(time_left(now - Duration::minutes(1), now), "now");
        assert_eq!(
            transfer_state(TransferState::Ready, now + Duration::hours(23), now),
            "Waiting · 23 h left"
        );
    }

    #[test]
    fn a_device_lists_who_can_send_to_it() {
        let mut device = Device {
            id: Uuid::nil(),
            name: "Laptop".to_owned(),
            platform: PLATFORM_LINUX.to_owned(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            last_seen_at: OffsetDateTime::UNIX_EPOCH,
            signed_in: true,
            online: false,
            current: false,
            uploads: 0,
            visibility: DeviceVisibility::Owner,
            visible_to: Vec::new(),
        };
        assert_eq!(visible_to(&device), "Only you");
        device.visibility = DeviceVisibility::Users;
        device.visible_to = vec![user(2, "dana"), user(3, "alex")];
        assert_eq!(visible_to(&device), "dana, alex");
    }
}
