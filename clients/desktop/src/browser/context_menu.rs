use iced::{
    Alignment, Element, Length, Point, Rectangle, Size, Vector,
    alignment::Horizontal,
    widget::{button, container, float, pin, row, space, text},
};

use protocol::{Node, NodeId, NodeKind};
use uuid::Uuid;

use super::{
    Message, devices,
    fit_column::fit_column,
    parts::divider,
    toolbar::{actions, menu_item, menu_row},
};
use crate::{
    icons::{ICON_SIZE, Icon, Tint},
    theme,
};

pub(super) const ITEM_HEIGHT: f32 = 34.0;
const MIN_CONTENT_WIDTH: f32 = 224.0;
const MENU_PADDING: f32 = 4.0;
const SEPARATOR_PADDING: [f32; 2] = [4.0, 0.0];
const EDGE_MARGIN: f32 = 8.0;
const LABEL_SPACING: f32 = 12.0;

/// An extra entry of the context menu, below the actions.
pub struct Entry {
    pub icon: Icon,
    pub label: &'static str,
    pub message: Message,
}

/// The entry opening the selected folder in the web client: only a single folder has one.
pub fn web_entry(selected: &[&Node]) -> Option<Entry> {
    match selected {
        [node] if node.kind == NodeKind::Folder => Some(Entry {
            icon: Icon::Web,
            label: "Open in web",
            message: Message::OpenInWeb(Some(node.id)),
        }),
        _ => None,
    }
}

/// The context menu for `selected`: the actions with the web entry after Open, the sync `entries`,
/// and last, set apart, moving to the bin.
pub fn view<'a>(
    selected: &[&Node],
    lost_and_found: NodeId,
    entries: Vec<Entry>,
) -> Element<'a, Message> {
    let mut actions = actions(selected, lost_and_found);
    let last = actions.pop();
    let mut items: Vec<_> = actions
        .into_iter()
        .map(|(action, enabled)| menu_item(action, enabled))
        .collect();
    // Only a single folder has a web entry, and Open is then the first item.
    if let Some(web) = web_entry(selected) {
        items.insert(1, entry(web));
    }
    items.extend(entries.into_iter().map(entry));
    if let Some((action, enabled)) = last {
        items.push(separator());
        items.push(menu_item(action, enabled));
    }
    panel(items)
}

fn separator<'a>() -> Element<'a, Message> {
    container(divider()).padding(SEPARATOR_PADDING).into()
}

fn entry<'a>(entry: Entry) -> Element<'a, Message> {
    menu_row(
        entry.icon,
        Tint::Muted,
        entry.label,
        "",
        theme::tool,
        Some(entry.message),
    )
}

/// The menu of the account in the sidebar.
pub fn account<'a>() -> Element<'a, Message> {
    panel([menu_row(
        Icon::SignOut,
        Tint::Muted,
        "Sign out",
        "",
        theme::tool,
        Some(Message::SignOut),
    )])
}

/// The menu of a device: renaming it and signing it out, which `can_sign_out` allows.
pub fn device<'a>(id: Uuid, can_sign_out: bool) -> Element<'a, Message> {
    let rename = Message::Devices(devices::Message::Rename(id));
    let sign_out = Message::Devices(devices::Message::SignOut(id));
    let visibility = Message::Devices(devices::Message::ChooseVisibility(id));
    panel([
        menu_row(
            Icon::Rename,
            Tint::Muted,
            "Rename",
            "",
            theme::tool,
            Some(rename),
        ),
        menu_row(
            Icon::Visibility,
            Tint::Muted,
            "Visibility…",
            "",
            theme::tool,
            Some(visibility),
        ),
        separator(),
        menu_row(
            Icon::SignOut,
            if can_sign_out {
                Tint::Danger
            } else {
                Tint::Disabled
            },
            "Sign out",
            "",
            theme::danger_tool,
            can_sign_out.then_some(sign_out),
        ),
    ])
}

/// A menu of `folders` that opens the chosen one.
pub fn folders(folders: &[Node]) -> Element<'_, Message> {
    let items = folders.iter().map(|folder| {
        button(
            row![
                Icon::Folder.view(ICON_SIZE, Tint::Accent),
                text(folder.name.as_str())
            ]
            .spacing(LABEL_SPACING)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .height(ITEM_HEIGHT)
        .style(theme::tool)
        .on_press(Message::Jump(Some(folder.id)))
        .into()
    });
    panel(items)
}

fn panel<'a>(items: impl IntoIterator<Item = Element<'a, Message>>) -> Element<'a, Message> {
    let floor = space().width(MIN_CONTENT_WIDTH).height(0.0);
    container(fit_column(items.into_iter().chain([floor.into()])))
        .padding(MENU_PADDING)
        .style(theme::menu)
        .into()
}

/// What a menu hangs from: the pointer, or a widget whose `edge` the menu lines up with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    target: Rectangle,
    edge: Horizontal,
}

impl Placement {
    /// A menu opening under the pointer at `cursor`.
    pub fn cursor(cursor: Point) -> Self {
        Self {
            target: Rectangle::new(cursor, Size::ZERO),
            edge: Horizontal::Left,
        }
    }

    /// A menu hanging from `target`: below it, or above when there is no room, with its `edge`
    /// lined up with the same edge of `target`.
    pub fn hanging(target: Rectangle, edge: Horizontal) -> Self {
        Self { target, edge }
    }
}

/// `menu` placed by `placement`. It is moved once laid out, as only then is its size known.
pub fn place<'a>(menu: Element<'a, Message>, placement: Placement) -> Element<'a, Message> {
    pin(float(menu).translate(move |bounds, viewport| {
        let at = position(placement, viewport.size(), bounds.size());
        Vector::new(at.x - bounds.x, at.y - bounds.y)
    }))
    .into()
}

/// Where to put a `menu` sized menu so it stays inside `window`.
fn position(placement: Placement, window: Size, menu: Size) -> Point {
    let Placement { target, edge } = placement;
    let max_x = (window.width - menu.width - EDGE_MARGIN).max(0.0);
    let max_y = (window.height - menu.height - EDGE_MARGIN).max(0.0);
    let x = match edge {
        Horizontal::Right => target.x + target.width - menu.width,
        _ => target.x,
    };
    let below = target.y + target.height;
    let y = if below <= max_y {
        below
    } else {
        target.y - menu.height
    };
    Point::new(x.clamp(0.0, max_x), y.clamp(0.0, max_y))
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    const MENU: Size = Size::new(300.0, 152.0);

    fn node(kind: NodeKind) -> Node {
        Node {
            id: NodeId(Uuid::from_u128(1)),
            parent_id: None,
            kind,
            name: "node".to_owned(),
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
    fn only_a_single_folder_opens_in_web() {
        let (folder, file) = (node(NodeKind::Folder), node(NodeKind::File));
        assert!(web_entry(&[&folder]).is_some());
        assert!(web_entry(&[&file]).is_none());
        assert!(web_entry(&[&folder, &folder]).is_none());
        assert!(web_entry(&[]).is_none());
    }

    #[test]
    fn moving_to_the_bin_comes_last_in_the_menu() {
        let folder = node(NodeKind::Folder);
        let sync = Entry {
            icon: Icon::Sync,
            label: "Sync to local folder…",
            message: Message::CloseMenu,
        };
        let mut ui = iced_test::simulator(view(&[&folder], NodeId(Uuid::from_u128(9)), vec![sync]));

        assert!(ui.find("Move…").is_ok());
        assert!(ui.find("Move to bin").is_ok());
        let sync = ui.find("Sync to local folder…").unwrap();
        let bin = ui.find("Move to bin").unwrap();
        assert!(sync.bounds().y < bin.bounds().y);
    }

    #[test]
    fn menu_opens_at_the_cursor_when_it_fits() {
        let placement = Placement::cursor(Point::new(100.0, 100.0));
        let point = position(placement, Size::new(1000.0, 800.0), MENU);
        assert_eq!(point, Point::new(100.0, 100.0));
    }

    #[test]
    fn menu_is_pushed_back_inside_the_window() {
        let placement = Placement::cursor(Point::new(990.0, 790.0));
        let point = position(placement, Size::new(1000.0, 800.0), MENU);
        assert!(point.x + MENU.width <= 1000.0);
        assert!(point.y + MENU.height <= 800.0);
    }

    #[test]
    fn menu_hangs_below_the_target_with_the_same_edge() {
        let target = Rectangle::new(Point::new(400.0, 100.0), Size::new(24.0, 20.0));
        let window = Size::new(1000.0, 800.0);

        let left = position(Placement::hanging(target, Horizontal::Left), window, MENU);
        let right = position(Placement::hanging(target, Horizontal::Right), window, MENU);

        assert_eq!(left, Point::new(400.0, 120.0));
        assert_eq!(right, Point::new(424.0 - MENU.width, 120.0));
    }

    #[test]
    fn menu_hangs_above_the_target_when_there_is_no_room_below() {
        let target = Rectangle::new(Point::new(400.0, 700.0), Size::new(24.0, 20.0));
        let point = position(
            Placement::hanging(target, Horizontal::Right),
            Size::new(1000.0, 800.0),
            MENU,
        );
        assert_eq!(point.y, 700.0 - MENU.height);
    }
}
