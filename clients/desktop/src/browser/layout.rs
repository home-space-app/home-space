use iced::{
    Alignment, Element, Length, Padding, Size,
    alignment::Horizontal,
    widget::{
        Row, button, center, column, container, mouse_area, opaque, responsive, row, space, stack,
        svg, text, text_input,
    },
};

use client_core::{labels::HOME, listing::collapsed_crumbs};
use protocol::Node;

use super::{
    Menu, Message, Page, State, anchor::Anchor, context_menu, context_menu::Placement, devices,
    dialogs, grid, parts, search, send, sidebar, sync_queue, syncing, toolbar, toolbar::SyncChip,
    transfer_list, transfer_save,
};
use crate::{
    icons::{ICON_SIZE, Icon, Tint},
    sync, theme,
};

const MAIN_PADDING: [f32; 2] = [16.0, 24.0];
const OVERLAY_PADDING: f32 = 24.0;
const SPACING: f32 = 12.0;
const GRID_SPACING: f32 = 4.0;
const BACK_PADDING: f32 = 6.0;
const BACK_GAP: f32 = 8.0;
/// Below this content width a row of the sync settings puts its buttons under its text.
const SETTINGS_STACK_BELOW: f32 = 760.0;
const TOP_HEIGHT: f32 = 40.0;
const HEADING_SIZE: f32 = 18.0;
const SEARCH_WIDTH: f32 = 300.0;
/// Below this width the search box leaves out the key that focuses it.
const SEARCH_KBD_WIDTH: f32 = 240.0;
const SEARCH_PADDING: Padding = Padding {
    top: 8.0,
    right: 64.0,
    bottom: 8.0,
    left: 38.0,
};
const SEARCH_OVERLAY_PADDING: [f32; 2] = [0.0, 8.0];
const CLEAR_SEARCH_PADDING: f32 = 2.0;
const CRUMB_PADDING: [f32; 2] = [2.0, 6.0];
const SEARCH_PLACEHOLDER: &str = "Search files and folders";
#[cfg(target_os = "macos")]
const SEARCH_SHORTCUT: &str = "Cmd K";
#[cfg(not(target_os = "macos"))]
const SEARCH_SHORTCUT: &str = "Ctrl K";
pub(super) const SEARCH_INPUT: &str = "search";
const DROP_LABEL_SIZE: f32 = 16.0;
const DROP_LABEL_PADDING: [f32; 2] = [14.0, 24.0];
const DROP_INSET: f32 = 12.0;
const DROP_LINE: f32 = 2.0;
const DROP_RADIUS: f32 = 8.0;
const DROP_DASH: [f32; 2] = [6.0, 4.0];
const TRASH: &str = "Recycle bin";
const SYNC_QUEUE: &str = "Sync queue";
const SYNC_SETTINGS: &str = "Sync settings";
const DEVICES: &str = "Devices";
const TRANSFERS: &str = "Transfers";
const CRUMB_SEPARATOR: &str = "›";
const CRUMB_OVERFLOW: &str = "…";

impl State {
    pub fn view(&self) -> Element<'_, Message> {
        let selected = self.folder.selection.selected(&self.folder.listing.nodes);
        // The folder's listing takes the drop overlay itself; other pages have it over them all.
        let shown = self.page(&selected);
        let shown: Element<'_, Message> = if self.dropping && !self.shows_listing() {
            stack![shown, self.drop_overlay()].into()
        } else {
            shown
        };
        let main = container(shown)
            .padding(MAIN_PADDING)
            .width(Length::Fill)
            .height(Length::Fill);
        let page = row![self.sidebar(), main];

        let mut layers: Vec<Element<'_, Message>> = vec![page.into()];
        // The toast stands above the transfers panel, in the same corner.
        let toast = (!matches!(self.page, Page::Transfers))
            .then(|| self.transfer_list.toast())
            .flatten()
            .map(|toast| toast.map(Message::Transfers));
        let panel =
            (!self.transfers.is_empty()).then(|| self.transfers.view().map(Message::Transfer));
        if toast.is_some() || panel.is_some() {
            layers.push(
                container(
                    column![]
                        .push(toast)
                        .push(panel)
                        .spacing(SPACING)
                        .align_x(Horizontal::Right),
                )
                .padding(OVERLAY_PADDING)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_right(Length::Fill)
                .align_bottom(Length::Fill)
                .into(),
            );
        }
        // The people picker opens from a dialog, so it goes over it below.
        let menu = self.menu.filter(|menu| !matches!(menu, Menu::Users(_)));
        if let Some(menu) = menu {
            layers.extend(self.menu_layers(menu, &selected));
        }
        if let Some(confirm) = &self.syncing.confirm {
            layers.push(modal(
                syncing::confirm(confirm, self.syncing.status()),
                Message::Sync(syncing::Message::Confirm(false)),
            ));
        }
        if let Some(dialog) = &self.dialog {
            layers.push(modal(
                dialog.view().map(Message::Dialog),
                Message::Dialog(dialogs::Message::Cancel),
            ));
        }
        if let Some(send) = &self.send {
            layers.push(modal(
                send.view().map(Message::Send),
                Message::Send(send::Message::Cancel),
            ));
        }
        match &self.page {
            Page::Devices(devices) => {
                if let Some(dialog) = devices.dialog() {
                    layers.push(modal(
                        dialog.map(Message::Devices),
                        Message::Devices(devices::Message::Cancel),
                    ));
                }
                if let (Some(Menu::Users(placement)), Some(picker)) = (self.menu, devices.picker())
                {
                    layers.extend(menu_layers(picker.map(Message::Devices), placement));
                }
            }
            Page::Transfers => {
                if let Some(dialog) = self.transfer_list.dialog() {
                    layers.push(modal(
                        dialog.map(Message::Transfers),
                        Message::Transfers(transfer_list::Message::SaveDialog(
                            transfer_save::Message::Cancel,
                        )),
                    ));
                }
            }
            _ => {}
        }
        // Over every page: Open starts from the toast as well as from the list.
        if let Some(confirm) = self.transfer_list.link_dialog() {
            layers.push(modal(
                confirm.map(Message::Transfers),
                Message::Transfers(transfer_list::Message::CancelOpen),
            ));
        }
        stack(layers).into()
    }

    fn shows_listing(&self) -> bool {
        !self.search.is_active() && matches!(self.page, Page::Folder)
    }

    /// How wide the main column's content is.
    fn content_width(&self) -> f32 {
        self.window.width - sidebar::width(self.window.width) - MAIN_PADDING[1] * 2.0
    }

    /// The top line and whatever the browser shows below it: search results, a page or the
    /// folder.
    fn page<'a>(&'a self, selected: &[&'a Node]) -> Element<'a, Message> {
        if self.search.is_active() {
            let back = button(Icon::Back.view(ICON_SIZE, Tint::Muted))
                .padding(BACK_PADDING)
                .style(theme::tool)
                .on_press(Message::Search(search::Message::Clear));
            let title = row![
                back,
                heading(format!("Results for “{}”", self.search.query().trim()))
            ]
            .spacing(BACK_GAP)
            .align_y(Alignment::Center);
            return column![top(title, self.search_box()), self.search.view()]
                .spacing(SPACING)
                .into();
        }
        let status = self.syncing.status();
        let (title, action, content): (_, Element<'a, Message>, Element<'a, Message>) =
            match &self.page {
                Page::Trash(trash) => (
                    TRASH,
                    trash.action().map(Message::Trash),
                    trash.view().map(Message::Trash),
                ),
                Page::SyncQueue => (
                    SYNC_QUEUE,
                    sync_queue::action(status),
                    sync_queue::view(status),
                ),
                Page::SyncSettings => (
                    SYNC_SETTINGS,
                    syncing::settings_action(status),
                    syncing::settings(
                        status,
                        self.syncing.notice.as_deref(),
                        self.content_width() < SETTINGS_STACK_BELOW,
                    ),
                ),
                Page::Devices(devices) => (
                    DEVICES,
                    devices.action().map(Message::Devices),
                    devices.view().map(Message::Devices),
                ),
                Page::Transfers => (
                    TRANSFERS,
                    self.transfer_list.action().map(Message::Transfers),
                    self.transfer_list.view().map(Message::Transfers),
                ),
                Page::Folder => return self.folder_page(selected),
            };
        column![top(heading(title), action), content]
            .spacing(SPACING)
            .into()
    }

    fn folder_page<'a>(&'a self, selected: &[&'a Node]) -> Element<'a, Message> {
        let mut page = column![
            top(self.breadcrumbs(), self.search_box()),
            toolbar::view(
                selected,
                !self.in_lost_and_found(),
                self.folder.id,
                self.sync_chip(),
                self.content_width(),
            )
        ]
        .spacing(SPACING);
        if let Some(error) = &self.folder.error {
            page = page.push(parts::alert_error(Icon::Error, error.as_str(), None));
        }
        let grid = responsive(move |size| {
            let columns = grid::Columns::fitting(size.width);
            column![
                grid::header(
                    self.folder.selection.coverage(&self.folder.listing.nodes),
                    self.folder.sort,
                    columns,
                ),
                grid::rows(
                    &self.folder.listing.nodes,
                    self.folder.id,
                    &self.folder.selection,
                    self.syncing.status(),
                    columns,
                ),
            ]
            .into()
        });
        let listing: Element<'a, Message> = if self.dropping {
            stack![parts::list_box(grid), self.drop_overlay()].into()
        } else {
            parts::list_box(grid)
        };
        match &self.details {
            Some(details) => page.push(
                row![
                    listing,
                    details.view(
                        self.syncing
                            .status()
                            .location(Some(details.node_id()))
                            .map(syncing::display_path)
                    )
                ]
                .spacing(SPACING),
            ),
            None => page.push(listing),
        }
        .into()
    }

    /// The click-away backdrop and the popup menu.
    fn menu_layers<'a>(&'a self, menu: Menu, selected: &[&'a Node]) -> [Element<'a, Message>; 2] {
        let (placement, panel) = match menu {
            Menu::Actions(placement) => (
                placement,
                context_menu::view(
                    selected,
                    self.me.lost_and_found_id,
                    self.syncing.entries(selected),
                ),
            ),
            Menu::Crumbs(placement) => (
                placement,
                context_menu::folders(self.folder.hidden_crumbs()),
            ),
            Menu::Account(placement) => (placement, context_menu::account()),
            Menu::Device(id, placement) => {
                let can_sign_out = match &self.page {
                    Page::Devices(devices) => devices.can_sign_out(id),
                    _ => false,
                };
                (placement, context_menu::device(id, can_sign_out))
            }
            // Shown over its dialog by the caller.
            Menu::Users(placement) => (placement, space().into()),
        };
        menu_layers(panel, placement)
    }

    /// What the chip above the listing says about the folder shown, if it takes part in the sync.
    fn sync_chip(&self) -> Option<SyncChip> {
        let status = self.syncing.status();
        let folder = self.folder.id;
        if status.badge(folder) == Some(sync::Badge::Disabled) {
            return Some(SyncChip {
                icon: Icon::SyncDisabled,
                tint: Tint::Muted,
                label: "Not synced".to_owned(),
            });
        }
        status.location(folder).map(|location| SyncChip {
            icon: Icon::Sync,
            tint: Tint::Success,
            label: format!("Syncs to {}", syncing::display_path(location)),
        })
    }

    /// Where dropped files go, in a dashed outline over what it covers.
    fn drop_overlay(&self) -> Element<'_, Message> {
        let label = container(
            row![
                Icon::Upload.view(ICON_SIZE, Tint::Accent),
                text(format!("Drop files to upload to {}", self.folder_name()))
                    .size(DROP_LABEL_SIZE)
                    .font(theme::SEMIBOLD),
            ]
            .spacing(SPACING)
            .align_y(Alignment::Center),
        )
        .padding(DROP_LABEL_PADDING)
        .style(theme::menu);
        container(stack![responsive(dashed_outline), center(label)])
            .padding(DROP_INSET)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::drop_zone)
            .into()
    }

    /// The search field, with its icon and the key that focuses it, or what clears it, drawn
    /// over the field's padding.
    fn search_box(&self) -> Element<'_, Message> {
        let width = SEARCH_WIDTH.min(self.content_width() / 2.0);
        let input = text_input(SEARCH_PLACEHOLDER, self.search.query())
            .id(SEARCH_INPUT)
            .style(theme::input)
            .padding(SEARCH_PADDING)
            .on_input(|query| Message::Search(search::Message::Changed(query)));
        let mut overlay = row![
            Icon::Search.view(ICON_SIZE, Tint::Muted),
            space::horizontal()
        ]
        .align_y(Alignment::Center)
        .padding(SEARCH_OVERLAY_PADDING)
        .height(Length::Fill);
        if !self.search.query().is_empty() {
            overlay = overlay.push(
                button(Icon::Close.view(ICON_SIZE, Tint::Muted))
                    .padding(CLEAR_SEARCH_PADDING)
                    .style(theme::tool)
                    .on_press(Message::Search(search::Message::Clear)),
            );
        } else if width >= SEARCH_KBD_WIDTH {
            overlay = overlay.push(toolbar::kbd(SEARCH_SHORTCUT));
        }
        stack![input, overlay].width(width).into()
    }

    /// Where the folder is: the folders above it lead there, and the current one is the title.
    fn breadcrumbs(&self) -> Element<'_, Message> {
        let path = &self.folder.listing.path;
        let hidden = collapsed_crumbs(path.len()).unwrap_or_default();
        let last = path.len().checked_sub(1);
        let crumbs = path
            .iter()
            .enumerate()
            .filter_map(|(index, folder)| {
                if index == hidden.start && !hidden.is_empty() {
                    Some(overflow_crumb())
                } else if hidden.contains(&index) {
                    None
                } else if Some(index) == last {
                    Some([separator(), current_crumb(folder.name.as_str())])
                } else {
                    Some(crumb(folder.name.as_str(), Message::Jump(Some(folder.id))))
                }
            })
            .flatten();
        let home = if path.is_empty() {
            current_crumb(HOME)
        } else {
            crumb_button(HOME, Message::Jump(None))
        };
        let mut trail: Row<'_, Message> = row![home]
            .extend(crumbs)
            .spacing(GRID_SPACING)
            .align_y(Alignment::Center);
        if self.folder.loading {
            trail = trail
                .push(container(text("Loading…").style(theme::muted_text)).padding(CRUMB_PADDING));
        }
        trail.into()
    }
}

/// A dashed rounded outline filling `size`. It is an svg made for that size, as iced draws no
/// dashed borders; it only exists while files are dragged over the window.
fn dashed_outline<'a>(size: Size) -> Element<'a, Message> {
    let (width, height) = (size.width, size.height);
    let inset = DROP_LINE / 2.0;
    let outline = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"><rect x="{inset}" y="{inset}" width="{}" height="{}" rx="{DROP_RADIUS}" fill="none" stroke="black" stroke-width="{DROP_LINE}" stroke-dasharray="{} {}"/></svg>"#,
        width - DROP_LINE,
        height - DROP_LINE,
        DROP_DASH[0],
        DROP_DASH[1],
    );
    svg(svg::Handle::from_memory(outline.into_bytes()))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|theme: &iced::Theme, _status| svg::Style {
            color: Some(theme::tokens(theme).accent),
        })
        .into()
}

/// The click-away backdrop and the popup `panel` placed by `placement`.
fn menu_layers(panel: Element<'_, Message>, placement: Placement) -> [Element<'_, Message>; 2] {
    [
        mouse_area(space().width(Length::Fill).height(Length::Fill))
            .on_press(Message::CloseMenu)
            .on_right_press(Message::CloseMenu)
            .into(),
        context_menu::place(panel, placement),
    ]
}

fn modal<'a>(content: Element<'a, Message>, on_dismiss: Message) -> Element<'a, Message> {
    opaque(mouse_area(center(opaque(content)).style(theme::backdrop)).on_press(on_dismiss))
}

/// `title` on the left, what the page offers on the right.
fn top<'a>(
    title: impl Into<Element<'a, Message>>,
    action: Element<'a, Message>,
) -> Element<'a, Message> {
    row![container(title).width(Length::Fill).clip(true), action]
        .spacing(SPACING)
        .height(TOP_HEIGHT)
        .align_y(Alignment::Center)
        .into()
}

/// The title of a page.
fn heading<'a>(title: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    text(title)
        .size(HEADING_SIZE)
        .font(theme::SEMIBOLD)
        .wrapping(text::Wrapping::None)
        .into()
}

fn separator<'a>() -> Element<'a, Message> {
    text(CRUMB_SEPARATOR)
        .size(HEADING_SIZE)
        .style(theme::crumb_separator)
        .into()
}

/// The crumb of the folder shown, which leads nowhere.
fn current_crumb(label: &str) -> Element<'_, Message> {
    container(heading(label)).padding(CRUMB_PADDING).into()
}

fn crumb_button(label: &str, message: Message) -> Element<'_, Message> {
    button(
        text(label)
            .size(HEADING_SIZE)
            .font(theme::MEDIUM)
            .wrapping(text::Wrapping::None),
    )
    .padding(CRUMB_PADDING)
    .style(theme::crumb)
    .on_press(message)
    .into()
}

/// The crumb standing for the hidden folders, whose menu hangs from its left edge.
fn overflow_crumb<'a>() -> [Element<'a, Message>; 2] {
    let button = button(text(CRUMB_OVERFLOW).size(HEADING_SIZE))
        .padding(CRUMB_PADDING)
        .style(theme::crumb)
        // Never sent: the anchor takes the press, and only needs the button to look enabled.
        .on_press(Message::CloseMenu);
    [
        separator(),
        Anchor::new(button)
            .on_press(|bounds| Message::CrumbsMenu(Placement::hanging(bounds, Horizontal::Left)))
            .into(),
    ]
}

/// A separator followed by a crumb leading to a folder.
fn crumb(label: &str, message: Message) -> [Element<'_, Message>; 2] {
    [separator(), crumb_button(label, message)]
}
