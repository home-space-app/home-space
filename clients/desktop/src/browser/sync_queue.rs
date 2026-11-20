//! The sync queue: what the sync still has to do, with the progress of the transfers.

use std::path::Path;

use iced::{
    Alignment, Element, Length, Padding,
    widget::{Column, column, container, progress_bar, row, scrollable, space, text, tooltip},
};

use client_core::format::human_size;

use super::{
    Message,
    parts::{ROW_SIZE, SECOND_LINE_SIZE, divider, empty, list_box, outlined, small_outlined},
    syncing::{self, display_path},
};
use crate::{
    ellipsis::ellipsized,
    icons::{ICON_SIZE, Icon, Tint},
    locale::short_date,
    sync::{Activity, Fault, Item, ItemState, Operation, Status, Summary},
    theme,
};

/// Rows shown before the rest is only counted; a queue can hold thousands of files.
const MAX_ROWS: usize = 500;
const SPACING: f32 = 12.0;
const ROW_PADDING: [f32; 2] = [10.0, 16.0];
const SECTION_PADDING: [f32; 2] = [10.0, 16.0];
const SECTION_GAP: f32 = 8.0;
const SECTION_ICON: f32 = 16.0;
const MORE_PADDING: Padding = Padding {
    top: 8.0,
    right: 16.0,
    bottom: 8.0,
    left: 16.0,
};
const ICON_CELL: f32 = 24.0;
const NAME_PORTION: u16 = 3;
const MIDDLE_PORTION: u16 = 1;
const STATE_WIDTH: f32 = 130.0;
const PROGRESS_GIRTH: f32 = 6.0;
const TIP_PADDING: f32 = 6.0;
const TIP_SIZE: f32 = 12.0;
const RETRY: &str = "Retry";
pub const RETRY_ALL: &str = "Retry all";

/// A titled group of the queue and which states belong to it.
struct Section {
    title: &'static str,
    icon: Icon,
    tint: Tint,
    wanted: fn(&ItemState) -> bool,
}

/// What the page's top line offers: trying everything that failed again.
pub fn action(status: &Status) -> Element<'_, Message> {
    if status.summary().failed > 0 {
        outlined(Some(Icon::Refresh), RETRY_ALL, Some(retry(None)))
    } else {
        space().into()
    }
}

fn retry(paths: Option<Vec<std::path::PathBuf>>) -> Message {
    Message::Sync(syncing::Message::Retry(paths))
}

pub fn view(status: &Status) -> Element<'_, Message> {
    let summary = status.summary();
    let busy = match status.activity {
        Activity::Checking => Some("Checking server…"),
        Activity::Scanning => Some("Scanning…"),
        Activity::Idle | Activity::Syncing => None,
    };
    let mut page = column![].spacing(SPACING);
    let line = summary_line(summary);
    if let Some(line) = line.as_deref().or(busy) {
        page = page.push(text(line.to_owned()).style(theme::muted_text));
    }
    if status.queue.is_empty() {
        return page
            .push(list_box(empty("Everything is up to date.")))
            .into();
    }

    let mut shown = 0;
    let mut rows = Column::new();
    let sections = [
        Section {
            title: "Could not sync",
            icon: Icon::Error,
            tint: Tint::Danger,
            wanted: |state| matches!(state, ItemState::Failed(_)),
        },
        Section {
            title: "Syncing now",
            icon: Icon::Sync,
            tint: Tint::Accent,
            wanted: |state| *state == ItemState::Active,
        },
        Section {
            title: "Waiting",
            icon: Icon::Schedule,
            tint: Tint::Muted,
            wanted: |state| *state == ItemState::Queued,
        },
    ];
    for section in sections {
        let items: Vec<&Item> = status
            .queue
            .iter()
            .filter(|item| (section.wanted)(&item.state))
            .collect();
        if items.is_empty() {
            continue;
        }
        let first = shown == 0;
        rows = rows.push(section_heading(&section, items.len(), first));
        for item in items.into_iter().take(MAX_ROWS.saturating_sub(shown)) {
            shown += 1;
            rows = rows
                .push(match &item.state {
                    ItemState::Failed(fault) => problem_row(&item.path, fault),
                    ItemState::Queued | ItemState::Active => item_row(item),
                })
                .push(divider());
        }
    }
    let hidden = status.queue.len().saturating_sub(shown);
    if hidden > 0 {
        rows = rows.push(
            container(text(format!("and {hidden} more")).style(theme::muted_text))
                .padding(MORE_PADDING),
        );
    }
    page.push(list_box(scrollable(rows).height(Length::Fill)))
        .into()
}

/// The heading of a group, which follows the box's corners when it comes `first`.
fn section_heading<'a>(section: &Section, count: usize, first: bool) -> Element<'a, Message> {
    let heading = container(
        row![
            section.icon.view(SECTION_ICON, section.tint),
            text(format!("{} ({count})", section.title)).font(theme::MEDIUM)
        ]
        .spacing(SECTION_GAP)
        .align_y(Alignment::Center),
    )
    .padding(SECTION_PADDING)
    .width(Length::Fill)
    .style(if first {
        theme::first_section
    } else {
        theme::section
    });
    column![heading, divider()].into()
}

/// "Uploading 12 · Downloading 3 · 4.0 MB left", or `None` with nothing to do.
fn summary_line(summary: Summary) -> Option<String> {
    let counts = [
        ("Uploading", summary.uploads),
        ("Downloading", summary.downloads),
        ("Other changes", summary.others),
    ]
    .into_iter()
    .filter(|(_, count)| *count > 0)
    .map(|(label, count)| format!("{label} {count}"));
    let mut parts: Vec<String> = counts.collect();
    if summary.bytes_left > 0 {
        let left = i64::try_from(summary.bytes_left).unwrap_or(i64::MAX);
        parts.push(format!("{} left", human_size(left)));
    }
    if summary.failed > 0 {
        parts.push(format!("{} failed", summary.failed));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The name of a queued path over where it is.
fn name_cell<'a>(name: impl Into<String>, path: &Path) -> Element<'a, Message> {
    column![
        ellipsized(name, ROW_SIZE, Alignment::Start).width(Length::Fill),
        container(
            ellipsized(display_path(path), SECOND_LINE_SIZE, Alignment::Start).width(Length::Fill)
        )
        .style(theme::muted),
    ]
    .width(Length::FillPortion(NAME_PORTION))
    .into()
}

/// A path that could not be synced: what went wrong, with the whole story in a tooltip, and a
/// button that tries it again.
fn problem_row<'a>(path: &'a Path, fault: &'a Fault) -> Element<'a, Message> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let problem = tooltip(
        container(
            ellipsized(fault_label(fault), SECOND_LINE_SIZE, Alignment::Start).width(Length::Fill),
        )
        .style(theme::error),
        container(text(fault_hint(fault)).size(TIP_SIZE))
            .padding(TIP_PADDING)
            .style(theme::menu),
        tooltip::Position::Top,
    );
    let again = small_outlined(RETRY, Some(retry(Some(vec![path.to_path_buf()]))));
    let cells = row![
        container(Icon::Error.view(ICON_SIZE, Tint::Danger)).width(ICON_CELL),
        name_cell(name, path),
        container(problem).width(Length::FillPortion(MIDDLE_PORTION)),
        container(again).align_right(STATE_WIDTH),
    ]
    .spacing(SPACING)
    .align_y(Alignment::Center);
    container(cells)
        .padding(ROW_PADDING)
        .width(Length::Fill)
        .into()
}

/// "Error 502 · 3 Nov 2026, 03:05"; a code that is not a number says nothing, so the reason
/// takes its place.
fn fault_label(fault: &Fault) -> String {
    let what = if fault.code.chars().all(|c| c.is_ascii_digit()) {
        format!("Error {}", fault.code)
    } else {
        fault.reason.clone()
    };
    format!("{what} · {}", short_date(fault.at))
}

/// "Error 502 · 2026-11-03 03:05" over the full reason.
fn fault_hint(fault: &Fault) -> String {
    format!(
        "Error {} · {}\n{}",
        fault.code,
        short_date(fault.at),
        fault.reason
    )
}

fn item_row(item: &Item) -> Element<'_, Message> {
    let active = item.state == ItemState::Active;
    let tint = if active { Tint::Accent } else { Tint::Muted };
    let (icon, tint) = match item.operation {
        Operation::Upload => (Icon::Upload, tint),
        Operation::Download => (Icon::Download, tint),
        Operation::CreateRemoteFolder | Operation::CreateLocalFolder => (Icon::NewFolder, tint),
        Operation::Move => (Icon::Move, tint),
        Operation::Delete => (Icon::Delete, tint),
        Operation::Blocked => (Icon::Error, Tint::Danger),
    };
    let bar: Element<'_, Message> = if active && item.size > 0 {
        progress_bar(0.0..=item.size as f32, item.done as f32)
            .length(Length::Fill)
            .girth(PROGRESS_GIRTH)
            .style(theme::progress)
            .into()
    } else {
        space().into()
    };
    let cells = row![
        container(icon.view(ICON_SIZE, tint)).width(ICON_CELL),
        name_cell(item.name.as_str(), &item.path),
        container(bar).width(Length::FillPortion(MIDDLE_PORTION)),
        container(
            ellipsized(state_label(item), SECOND_LINE_SIZE, Alignment::End).width(STATE_WIDTH)
        )
        .style(theme::muted),
    ]
    .spacing(SPACING)
    .align_y(Alignment::Center);
    container(cells)
        .padding(ROW_PADDING)
        .width(Length::Fill)
        .into()
}

/// "Uploading · 72%" while it moves, otherwise that it waits.
fn state_label(item: &Item) -> String {
    if item.state != ItemState::Active {
        return "Waiting".to_owned();
    }
    let doing = match item.operation {
        Operation::Upload => "Uploading",
        Operation::Download => "Downloading",
        _ => "Syncing",
    };
    match item.done.saturating_mul(100).checked_div(item.size) {
        Some(percent) => format!("{doing} · {percent}%"),
        None => doing.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use iced_test::simulator;

    use super::*;
    use crate::sync::Operation;

    fn item(name: &str, operation: Operation, size: u64, done: u64, state: ItemState) -> Item {
        Item::test(
            PathBuf::from("/sync").join(name),
            operation,
            size,
            done,
            state,
        )
    }

    fn status(queue: Vec<Item>) -> Status {
        Status {
            queue,
            ..Status::default()
        }
    }

    #[test]
    fn an_empty_queue_says_everything_is_up_to_date() {
        let status = status(Vec::new());
        let mut ui = simulator(view(&status));
        assert!(ui.find("Everything is up to date.").is_ok());
    }

    #[test]
    fn the_queue_is_grouped_by_state_with_progress_on_the_active_ones() {
        let status = status(vec![
            item(
                "big.mov",
                Operation::Upload,
                80 << 20,
                12 << 20,
                ItemState::Active,
            ),
            item("a.txt", Operation::Download, 2048, 0, ItemState::Queued),
            item(
                "bad.txt",
                Operation::Upload,
                1,
                0,
                ItemState::Failed(Fault::now("502", "boom")),
            ),
        ]);
        let mut ui = simulator(view(&status));
        assert!(ui.find("Syncing now (1)").is_ok());
        assert!(ui.find("Waiting (1)").is_ok());
        assert!(ui.find("Could not sync (1)").is_ok());
        assert!(ui.find("Uploading · 15%").is_ok());
    }

    #[test]
    fn a_failed_path_can_be_retried_alone_or_with_all_the_others() {
        let status = status(vec![item(
            "bad.txt",
            Operation::Upload,
            1,
            0,
            ItemState::Failed(Fault::now("502", "boom")),
        )]);
        let mut ui = simulator(view(&status));
        ui.click(RETRY).expect("retry is shown");
        let messages: Vec<Message> = ui.into_messages().collect();
        assert!(matches!(
            messages.as_slice(),
            [Message::Sync(syncing::Message::Retry(Some(_)))]
        ));

        let mut ui = simulator(action(&status));
        ui.click(RETRY_ALL).expect("retry all is shown");
        let messages: Vec<Message> = ui.into_messages().collect();
        assert!(matches!(
            messages.as_slice(),
            [Message::Sync(syncing::Message::Retry(None))]
        ));
    }

    #[test]
    fn a_failure_shows_its_number_or_else_its_reason() {
        let date = short_date(time::OffsetDateTime::now_utc());
        assert!(fault_label(&Fault::now("502", "bad gateway")).starts_with("Error 502 · "));
        assert_eq!(
            fault_label(&Fault {
                at: time::OffsetDateTime::now_utc(),
                ..Fault::now("local", "disk full")
            })
            .split(" · ")
            .next(),
            Some("disk full")
        );
        assert!(!date.is_empty());
    }

    #[test]
    fn with_nothing_failed_there_is_nothing_to_retry() {
        let status = status(vec![item(
            "a.txt",
            Operation::Upload,
            1,
            0,
            ItemState::Queued,
        )]);
        let mut ui = simulator(action(&status));
        assert!(ui.find(RETRY_ALL).is_err());
    }

    #[test]
    fn the_hint_names_the_code_and_the_reason() {
        let hint = fault_hint(&Fault::now("502", "server unreachable (502 Bad Gateway)"));
        assert!(hint.starts_with("Error 502 · "));
        assert!(hint.ends_with("\nserver unreachable (502 Bad Gateway)"));
    }

    #[test]
    fn a_long_queue_is_cut_off_and_counted() {
        let queue = (0..MAX_ROWS + 20)
            .map(|n| {
                item(
                    &format!("f{n}.txt"),
                    Operation::Upload,
                    1,
                    0,
                    ItemState::Queued,
                )
            })
            .collect();
        let status = status(queue);
        let mut ui = simulator(view(&status));
        assert!(ui.find("and 20 more").is_ok());
    }

    #[test]
    fn the_summary_names_what_is_ahead() {
        let summary = Summary {
            uploads: 12,
            downloads: 3,
            failed: 1,
            bytes_left: 4096,
            ..Summary::default()
        };
        assert_eq!(
            summary_line(summary).as_deref(),
            Some("Uploading 12 · Downloading 3 · 4.0 KB left · 1 failed")
        );
        assert_eq!(summary_line(Summary::default()), None);
    }
}
