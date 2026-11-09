//! The sidebar: the places, the sync status and the account. It shrinks to its icons when the
//! window leaves too little room next to it.

use iced::{
    Alignment, Element, Length, Padding,
    alignment::Horizontal,
    widget::{button, column, container, row, space, text},
};

use super::{Message, Page, State, anchor::Anchor, context_menu::Placement, syncing, toolbar::tip};
use client_core::labels::LOST_AND_FOUND;

use crate::{
    ellipsis::ellipsized,
    icons::{ICON_SIZE, Icon, Tint},
    theme,
};

const WIDTH: f32 = 232.0;
const COMPACT_WIDTH: f32 = 60.0;
/// Below this window width the sidebar shows only its icons.
const COMPACT_BELOW: f32 = 760.0;
const DIVIDER: f32 = 1.0;
const PADDING: Padding = Padding {
    top: 16.0,
    right: 12.0,
    bottom: 12.0,
    left: 12.0,
};
const NAV_SPACING: f32 = 4.0;
const NAV_GAP: f32 = 10.0;
const NAV_PADDING: Padding = Padding {
    top: 8.0,
    right: 10.0,
    bottom: 8.0,
    left: 10.0,
};
const FOOT_SPACING: f32 = 8.0;
const LOGO_SIZE: f32 = 28.0;
const LOGO_ICON: f32 = 16.0;
const LOGO_PADDING: Padding = Padding {
    top: 4.0,
    right: 8.0,
    bottom: 16.0,
    left: 8.0,
};
/// Centres the logo in the compact sidebar.
const LOGO_COMPACT_LEFT: f32 = 4.0;
const BRAND: &str = "Home Space";
const BRAND_SIZE: f32 = 18.0;
const COUNT_HEIGHT: f32 = 20.0;
const COUNT_SIZE: f32 = 12.0;
const COUNT_PADDING: Padding = Padding {
    top: 0.0,
    right: 6.0,
    bottom: 0.0,
    left: 6.0,
};
const AVATAR_SIZE: f32 = 28.0;
const AVATAR_TEXT: f32 = 13.0;
const ACCOUNT_PADDING: [f32; 2] = [6.0, 8.0];
const ACCOUNT_COMPACT_PADDING: f32 = 4.0;

/// How much of a window `window_width` wide the sidebar takes.
pub fn width(window_width: f32) -> f32 {
    let sidebar = if is_compact(window_width) {
        COMPACT_WIDTH
    } else {
        WIDTH
    };
    sidebar + DIVIDER
}

fn is_compact(window_width: f32) -> bool {
    window_width < COMPACT_BELOW
}

impl State {
    pub(super) fn sidebar(&self) -> Element<'_, Message> {
        let compact = is_compact(self.window.width);
        let shows_found = self.lost_and_found.has_files();
        let in_found =
            shows_found && matches!(self.page, Page::Folder) && self.within_lost_and_found();
        let files = matches!(self.page, Page::Folder) && !in_found;
        let failed = self.syncing.status().problems.len();
        let mut places = column![
            logo(compact),
            place(
                if files {
                    Icon::Folder
                } else {
                    Icon::FolderOutline
                },
                "Files",
                files,
                0,
                compact,
                Message::OpenFiles
            ),
            place(
                Icon::Sync,
                "Sync queue",
                matches!(self.page, Page::SyncQueue),
                failed,
                compact,
                Message::OpenSyncQueue
            ),
            place(
                Icon::Delete,
                "Recycle bin",
                matches!(self.page, Page::Trash(_)),
                0,
                compact,
                Message::OpenTrash
            ),
        ]
        .spacing(NAV_SPACING);
        if shows_found {
            places = places.push(place(
                Icon::Inventory,
                LOST_AND_FOUND,
                in_found,
                0,
                compact,
                Message::OpenLostAndFound,
            ));
        }
        let places = places.push(place(
            Icon::Devices,
            "Devices",
            matches!(self.page, Page::Devices(_)),
            0,
            compact,
            Message::OpenDevices,
        ));
        let places = places.push(place(
            Icon::SwapVert,
            "Transfers",
            matches!(self.page, Page::Transfers),
            self.transfer_list.waiting(),
            compact,
            Message::OpenTransfers,
        ));
        let foot = column![
            syncing::status_card(
                self.syncing.status(),
                compact,
                matches!(self.page, Page::SyncSettings)
            ),
            account(self.me.username.as_str(), compact)
        ]
        .spacing(FOOT_SPACING);
        let bar = container(column![places, space::vertical(), foot])
            .padding(PADDING)
            .width(width(self.window.width) - DIVIDER)
            .height(Length::Fill)
            .style(theme::sidebar);
        let divider = container(space())
            .width(DIVIDER)
            .height(Length::Fill)
            .style(theme::divider);
        row![bar, divider].into()
    }
}

fn logo<'a>(compact: bool) -> Element<'a, Message> {
    let mark = container(Icon::Home.view(LOGO_ICON, Tint::OnAccent))
        .center(LOGO_SIZE)
        .style(theme::logo);
    let mut logo = row![mark].spacing(NAV_GAP).align_y(Alignment::Center);
    if !compact {
        logo = logo.push(text(BRAND).size(BRAND_SIZE).font(theme::SEMIBOLD));
    }
    let padding = if compact {
        LOGO_PADDING
            .left(LOGO_COMPACT_LEFT)
            .right(LOGO_COMPACT_LEFT)
    } else {
        LOGO_PADDING
    };
    container(logo).padding(padding).into()
}

/// A place: its icon and label, with the count of what `failed` there when anything did.
fn place<'a>(
    icon: Icon,
    label: &'a str,
    active: bool,
    failed: usize,
    compact: bool,
    message: Message,
) -> Element<'a, Message> {
    let tint = match (active, failed) {
        (true, _) => Tint::Accent,
        // With no room for the count, the icon tells that something failed.
        (false, 1..) if compact => Tint::Danger,
        (false, _) => Tint::Muted,
    };
    let mut content = row![icon.view(ICON_SIZE, tint)]
        .spacing(NAV_GAP)
        .align_y(Alignment::Center);
    if !compact {
        content = content.push(text(label).font(theme::MEDIUM).width(Length::Fill));
        if failed > 0 {
            content = content.push(
                container(
                    text(failed.to_string())
                        .size(COUNT_SIZE)
                        .font(theme::SEMIBOLD),
                )
                .padding(COUNT_PADDING)
                .center_y(COUNT_HEIGHT)
                .style(theme::danger_count),
            );
        }
    }
    let item = button(content)
        .padding(NAV_PADDING)
        .width(Length::Fill)
        .style(theme::nav(active))
        .on_press(message);
    if compact {
        tip(item.into(), label, "")
    } else {
        item.into()
    }
}

/// Who is signed in, which opens the menu with Sign out.
fn account(username: &str, compact: bool) -> Element<'_, Message> {
    let initial: String = username
        .chars()
        .next()
        .map(|first| first.to_uppercase().collect())
        .unwrap_or_default();
    let avatar = container(text(initial).size(AVATAR_TEXT).font(theme::BOLD))
        .center(AVATAR_SIZE)
        .style(theme::avatar);
    let mut content = row![avatar].spacing(NAV_GAP).align_y(Alignment::Center);
    if !compact {
        content = content
            .push(ellipsized(username, theme::TEXT_SIZE, Alignment::Start).width(Length::Fill))
            .push(Icon::MoreVert.view(ICON_SIZE, Tint::Muted));
    }
    let button = button(content)
        .padding(if compact {
            Padding::new(ACCOUNT_COMPACT_PADDING)
        } else {
            ACCOUNT_PADDING.into()
        })
        .width(Length::Fill)
        .style(theme::tool)
        // Never sent: the anchor takes the press, and only needs the button to look enabled.
        .on_press(Message::CloseMenu);
    Anchor::new(button)
        .on_press(|bounds| Message::AccountMenu(Placement::hanging(bounds, Horizontal::Left)))
        .into()
}
