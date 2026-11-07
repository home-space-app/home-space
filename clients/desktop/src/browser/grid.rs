use std::{collections::HashSet, path::Path};

use client_core::{
    format::human_size,
    listing::{Sort, SortKey},
};
use iced::{
    Alignment, Element, Length,
    alignment::Horizontal,
    widget::{button, column, container, responsive, row, scrollable, space, stack, text},
};
use protocol::{Node, NodeId, NodeKind};

use super::{
    Message,
    anchor::Anchor,
    context_menu::Placement,
    parts::{
        CHECK_LIST_PADDING, COLUMN_GAP, DATE_WIDTH, DIVIDER, HEADER_SIZE, MENU_WIDTH, ROW_HEIGHT,
        ROW_SIZE, SECOND_LINE_SIZE, SIZE_WIDTH, divider, empty, header_bar,
        menu_button as row_menu, muted_cell,
    },
    selection::{Coverage, Selection},
    syncing::{badge_icon, display_path},
};
use crate::{
    ellipsis::{ellipsized, text_width},
    icons::{ICON_SIZE, Icon, Tint, node_icon},
    locale::short_date,
    sync::{Badge, Item, Status},
    theme,
};

/// The scrollable holding the rows.
pub(super) const ROWS: &str = "rows";
pub(super) const CHECK_WIDTH: f32 = 36.0;
const SORT_ARROW: f32 = 14.0;
const SORT_ARROW_GAP: f32 = 4.0;
const NAME_GAP: f32 = 8.0;
const WAITING_GAP: f32 = 6.0;
const CHECKBOX: f32 = 16.0;
const CHECK_MARK: f32 = 14.0;
const DASH: (f32, f32) = (8.0, 2.0);
const CHECK_HEIGHT: f32 = 32.0;
/// The icon sits in a 24px cell and the badge hangs over its lower right corner.
const ICON_CELL: f32 = 24.0;
const BADGE_RIGHT: f32 = 4.0;
const BADGE_BOTTOM: f32 = 3.0;
const BADGE_DISC: f32 = 14.0;
const BADGE_SIZE: f32 = 13.0;
const NOT_SYNCED: &str = "Not synced";

/// Column titles with the select-all checkbox, aligned with [`rows`].
pub fn header<'a>(coverage: Coverage, sort: Sort) -> Element<'a, Message> {
    let cells = row![
        check(coverage, Message::ToggleAll),
        sort_button("Name", SortKey::Name, sort, Length::Fill, Alignment::Start),
        sort_button(
            "Size",
            SortKey::Size,
            sort,
            SIZE_WIDTH.into(),
            Alignment::End
        ),
        sort_button(
            "Modified",
            SortKey::Modified,
            sort,
            DATE_WIDTH.into(),
            Alignment::End
        ),
        space().width(MENU_WIDTH),
    ];
    header_bar(cells, CHECK_LIST_PADDING)
}

/// A column title that sorts by `key` when pressed, with an arrow while it is active.
fn sort_button(
    title: &str,
    key: SortKey,
    sort: Sort,
    width: Length,
    align: Alignment,
) -> Element<'_, Message> {
    let arrow = match (sort.key == key, sort.descending) {
        (false, _) => None,
        (true, false) => Some(Icon::Up),
        (true, true) => Some(Icon::Down),
    };
    let label = row![
        text(title.to_uppercase())
            .size(HEADER_SIZE)
            .font(theme::SEMIBOLD),
        arrow.map(|arrow| arrow.view(SORT_ARROW, Tint::Muted))
    ]
    .spacing(SORT_ARROW_GAP)
    .align_y(Alignment::Center);
    button(container(label).width(Length::Fill).align_x(align))
        .padding(0)
        .width(width)
        .style(theme::column_title)
        .on_press(Message::SortBy(key))
        .into()
}

/// The scrolling list of rows, or a hint when the folder is empty. After the listed `nodes`
/// come the new files of the `folder` that the sync has yet to upload, which are not on the
/// server to be listed.
pub fn rows<'a>(
    nodes: &'a [Node],
    folder: Option<NodeId>,
    selection: &Selection,
    sync: &'a Status,
) -> Element<'a, Message> {
    let listed: HashSet<&str> = nodes.iter().map(|node| node.name.as_str()).collect();
    // A file that just landed is listed before the sync reports it done.
    let mut waiting: Vec<&Item> = sync
        .new_uploads_in(folder)
        .filter(|item| !listed.contains(item.name.as_str()))
        .collect();
    waiting.sort_by_cached_key(|item| item.name.to_lowercase());
    if nodes.is_empty() && waiting.is_empty() {
        empty("Nothing here yet. Drop files anywhere to upload.")
    } else {
        let rows = nodes
            .iter()
            .map(|node| {
                let badge = sync.badge(Some(node.id));
                let line = second_line(
                    node,
                    badge,
                    sync.location(Some(node.id)),
                    sync.location(folder),
                );
                node_row(node, selection.contains(node.id), badge, line)
            })
            .chain(waiting.into_iter().map(pending_row));
        // The scrollbar floats over the padding right of the menu buttons.
        scrollable(column(rows.flat_map(|row| [row, divider()])))
            .id(ROWS)
            .height(Length::Fill)
            .on_scroll(Message::GridScrolled)
            .into()
    }
}

/// The offset to scroll to for row `index` to be in full view, given the current `offset` and the
/// `height` of the viewport; `None` when it already is.
pub(super) fn reveal_offset(index: usize, offset: f32, height: f32) -> Option<f32> {
    let top = index as f32 * (ROW_HEIGHT + DIVIDER);
    let bottom = top + ROW_HEIGHT;
    if top < offset {
        Some(top)
    } else if bottom > offset + height {
        Some(bottom - height)
    } else {
        None
    }
}

/// What a row says under the name, when it differs from what the folder's chip says: where it
/// syncs when that is not inside the folder's local folder, or that it is left out.
fn second_line(
    node: &Node,
    badge: Option<Badge>,
    location: Option<&Path>,
    folder_location: Option<&Path>,
) -> Option<String> {
    if badge == Some(Badge::Disabled) {
        return Some(NOT_SYNCED.to_owned());
    }
    let location = location?;
    let inside = folder_location.is_some_and(|folder| folder.join(&node.name) == location);
    (!inside).then(|| format!("Syncs to {}", display_path(location)))
}

fn node_row(
    node: &Node,
    selected: bool,
    badge: Option<Badge>,
    second_line: Option<String>,
) -> Element<'_, Message> {
    let icon = match (node_icon(node), badge) {
        (Icon::Folder, Some(Badge::Disabled)) => Icon::FolderOutline,
        (icon, _) => icon,
    };
    let tint = match icon {
        Icon::Folder => Tint::Accent,
        Icon::FolderOutline => Tint::AccentFaded,
        _ => Tint::Text,
    };
    let size = match node.kind {
        NodeKind::Folder => String::new(),
        NodeKind::File => human_size(node.size),
    };
    let coverage = if selected {
        Coverage::All
    } else {
        Coverage::None
    };
    let name: Element<'_, Message> = match node.kind {
        NodeKind::Folder => folder_link(node, icon, tint, badge, second_line),
        NodeKind::File => row![
            badged(icon.view(ICON_SIZE, tint), badge),
            name_cell(node.name.as_str(), second_line)
        ]
        .spacing(NAME_GAP)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into(),
    };
    let cells = row![
        check(coverage, Message::Toggle(node.id)),
        name,
        muted_cell(size, SIZE_WIDTH, Alignment::End),
        muted_cell(short_date(node.updated_at), DATE_WIDTH, Alignment::End),
        menu_button(node.id),
    ]
    .spacing(COLUMN_GAP)
    .height(Length::Fill)
    .align_y(Alignment::Center);
    let row_button = button(cells)
        .width(Length::Fill)
        .height(ROW_HEIGHT)
        .padding(CHECK_LIST_PADDING)
        .style(theme::row(selected))
        .on_press(Message::Click(node.id));
    let id = node.id;
    Anchor::new(row_button)
        .on_right_press(move |cursor| Message::ContextMenu(id, Placement::cursor(cursor)))
        .into()
}

/// The icon and name of a folder as a link into it, only as wide as they are so that the rest of
/// the row still selects.
fn folder_link<'a>(
    node: &'a Node,
    icon: Icon,
    tint: Tint,
    badge: Option<Badge>,
    second_line: Option<String>,
) -> Element<'a, Message> {
    let id = node.id;
    let name = node.name.as_str();
    responsive(move |bounds| {
        let room = bounds.width - ICON_CELL - BADGE_RIGHT - NAME_GAP;
        let second_width = second_line
            .as_deref()
            .map_or(0.0, |second| text_width(second, SECOND_LINE_SIZE));
        let width = text_width(name, ROW_SIZE)
            .max(second_width)
            .ceil()
            .min(room)
            .max(0.0);
        let content = row![
            badged(icon.view(ICON_SIZE, tint), badge),
            container(name_cell(name, second_line.clone())).width(width)
        ]
        .spacing(NAME_GAP)
        .align_y(Alignment::Center);
        button(content)
            .padding(0)
            .style(theme::name_link)
            .on_press(Message::OpenFolderLink(id))
            .into()
    })
    .height(Length::Shrink)
    .into()
}

/// A new file or folder waiting to be uploaded: grayed out and inert, with no menu to act on
/// what is not on the server yet.
fn pending_row(item: &Item) -> Element<'_, Message> {
    let icon = if item.is_folder() {
        Icon::Folder
    } else {
        Icon::Other
    };
    let size = if item.is_folder() {
        String::new()
    } else {
        human_size(i64::try_from(item.size).unwrap_or(i64::MAX))
    };
    let waiting = row![
        Icon::Update.view(ICON_SIZE, Tint::Muted),
        text("pending sync").size(ROW_SIZE)
    ]
    .spacing(WAITING_GAP)
    .align_y(Alignment::Center);
    let cells = row![
        space().width(CHECK_WIDTH),
        row![
            badged(icon.view(ICON_SIZE, Tint::Disabled), None),
            ellipsized(item.name.as_str(), ROW_SIZE, Alignment::Start).width(Length::Fill)
        ]
        .spacing(NAME_GAP)
        .align_y(Alignment::Center)
        .width(Length::Fill),
        ellipsized(size, ROW_SIZE, Alignment::End).width(SIZE_WIDTH),
        container(waiting).align_right(DATE_WIDTH + COLUMN_GAP + MENU_WIDTH),
    ]
    .spacing(COLUMN_GAP)
    .height(Length::Fill)
    .align_y(Alignment::Center);
    container(cells)
        .width(Length::Fill)
        .height(ROW_HEIGHT)
        .padding(CHECK_LIST_PADDING)
        .style(theme::disabled)
        .into()
}

/// The three dots that open the context menu for the row, like right-clicking it.
fn menu_button<'a>(id: NodeId) -> Element<'a, Message> {
    row_menu(Message::CloseMenu, move |bounds| {
        Message::ContextMenu(id, Placement::hanging(bounds, Horizontal::Right))
    })
}

/// The name, with `second` underneath when there is something to add.
fn name_cell(name: &str, second: Option<String>) -> Element<'_, Message> {
    let name = ellipsized(name, ROW_SIZE, Alignment::Start).width(Length::Fill);
    match second {
        Some(second) => column![
            name,
            container(ellipsized(second, SECOND_LINE_SIZE, Alignment::Start).width(Length::Fill))
                .style(theme::muted)
        ]
        .width(Length::Fill)
        .into(),
        None => name.into(),
    }
}

/// `icon` with the sync `badge` hanging over its lower right corner.
fn badged<'a>(icon: Element<'a, Message>, badge: Option<Badge>) -> Element<'a, Message> {
    let width = ICON_CELL + BADGE_RIGHT;
    let height = ICON_CELL + BADGE_BOTTOM;
    let mut layers = vec![
        container(icon)
            .center_x(ICON_CELL)
            .center_y(ICON_CELL)
            .into(),
    ];
    if let Some(badge) = badge {
        let (badge, tint) = badge_icon(badge);
        let disc = container(badge.view(BADGE_SIZE, tint))
            .center(BADGE_DISC)
            .style(theme::badge);
        layers.push(
            container(disc)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_right(Length::Fill)
                .align_bottom(Length::Fill)
                .into(),
        );
    }
    stack(layers).width(width).height(height).into()
}

/// A checkbox for what `coverage` says is checked of what it stands for.
pub(super) fn check<'a, M: Clone + 'a>(coverage: Coverage, message: M) -> Element<'a, M> {
    let mark: Element<'a, M> = match coverage {
        Coverage::None => space().into(),
        Coverage::Some => container(space())
            .width(DASH.0)
            .height(DASH.1)
            .style(theme::checkbox_dash)
            .into(),
        Coverage::All => Icon::Check.view(CHECK_MARK, Tint::OnAccent),
    };
    let checkbox = container(mark)
        .center(CHECKBOX)
        .style(theme::checkbox(coverage != Coverage::None));
    button(container(checkbox).center_y(CHECK_HEIGHT))
        .padding(0)
        .width(CHECK_WIDTH)
        .style(button::text)
        .on_press(message)
        .into()
}

#[cfg(test)]
mod tests {
    use protocol::NodeId;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    fn folder(name: &str) -> Node {
        Node {
            id: NodeId(Uuid::nil()),
            parent_id: None,
            kind: NodeKind::Folder,
            name: name.to_owned(),
            size: 0,
            sha256: None,
            mime: String::new(),
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    #[test]
    fn a_row_in_view_needs_no_scrolling() {
        let step = ROW_HEIGHT + DIVIDER;
        assert_eq!(reveal_offset(2, step, step * 3.0), None);
    }

    #[test]
    fn a_row_above_the_view_scrolls_to_its_top() {
        let step = ROW_HEIGHT + DIVIDER;
        assert_eq!(reveal_offset(1, step * 3.0, step * 3.0), Some(step));
    }

    #[test]
    fn a_row_below_the_view_scrolls_to_its_bottom() {
        let step = ROW_HEIGHT + DIVIDER;
        let height = step * 3.0;
        assert_eq!(
            reveal_offset(4, 0.0, height),
            Some(step * 4.0 + ROW_HEIGHT - height)
        );
    }

    #[test]
    fn a_folder_syncing_inside_the_folders_location_says_nothing_more() {
        let line = second_line(
            &folder("Books"),
            Some(Badge::Synced),
            Some(Path::new("/sync/Books")),
            Some(Path::new("/sync")),
        );
        assert_eq!(line, None);
    }

    #[test]
    fn a_folder_syncing_elsewhere_says_where() {
        let line = second_line(
            &folder("Images"),
            Some(Badge::Synced),
            Some(Path::new("/photos")),
            Some(Path::new("/sync")),
        );
        assert_eq!(line.as_deref(), Some("Syncs to /photos"));
    }

    #[test]
    fn a_synced_folder_in_an_unsynced_folder_says_where() {
        let line = second_line(
            &folder("Images"),
            Some(Badge::Synced),
            Some(Path::new("/photos")),
            None,
        );
        assert_eq!(line.as_deref(), Some("Syncs to /photos"));
    }

    #[test]
    fn a_folder_left_out_says_so() {
        let line = second_line(
            &folder("Skip"),
            Some(Badge::Disabled),
            None,
            Some(Path::new("/sync")),
        );
        assert_eq!(line.as_deref(), Some(NOT_SYNCED));
    }

    #[test]
    fn a_file_says_nothing_more() {
        assert_eq!(second_line(&folder("a"), None, None, None), None);
    }
}
