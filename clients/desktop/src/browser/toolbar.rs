use iced::{
    Alignment, Element, Length, Padding,
    alignment::Horizontal,
    widget::{button, container, row, space, text, tooltip},
};

use protocol::{Node, NodeId, NodeKind};

use super::{Message, anchor::Anchor, context_menu, context_menu::Placement};
use crate::{
    ellipsis::ellipsized,
    icons::{ICON_SIZE, Icon, Tint},
    theme,
};

const SPACING: f32 = 4.0;
const LABEL_SPACING: f32 = 6.0;
const BUTTON_PADDING: f32 = 10.0;
const BUTTON_PADDING_Y: f32 = 8.0;
const BUTTON_PADDING_Y_X: [f32; 2] = [BUTTON_PADDING_Y, BUTTON_PADDING];
const PRIMARY_PADDING: f32 = 16.0;
const BAR_PADDING: f32 = 4.0;
const KBD_SIZE: f32 = 11.0;
const KBD_PADDING: [f32; 2] = [0.0, 5.0];
const CHIP_HEIGHT: f32 = 28.0;
const CHIP_LEFT: f32 = 8.0;
const CHIP_RIGHT: f32 = 10.0;
const CHIP_ICON: f32 = 16.0;
const CHIP_GAP: f32 = 6.0;
const CHIP_SIZE: f32 = 13.0;
const CHIP_MIN_LABEL: f32 = 120.0;
const CLEAR_SELECTION: &str = "Clear selection";
const CLEAR_SHORTCUT: &str = "Esc";
const CHAR_WIDTH: f32 = 7.5;
const MIN_GAP: f32 = 24.0;
const TIP_PADDING: f32 = 6.0;
const TIP_SIZE: f32 = 12.0;
const MENU_GAP: f32 = 10.0;
const MENU_ROW_PADDING: [f32; 2] = [0.0, 10.0];

#[cfg(target_os = "macos")]
const NEW_FOLDER_SHORTCUT: &str = "Cmd+N";
#[cfg(not(target_os = "macos"))]
const NEW_FOLDER_SHORTCUT: &str = "Ctrl+N";
#[cfg(target_os = "macos")]
const UPLOAD_SHORTCUT: &str = "Cmd+U";
#[cfg(not(target_os = "macos"))]
const UPLOAD_SHORTCUT: &str = "Ctrl+U";
#[cfg(target_os = "macos")]
const DELETE_SHORTCUT: &str = "Delete";
#[cfg(not(target_os = "macos"))]
const DELETE_SHORTCUT: &str = "Del";

/// Something the user can do with the selected nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Open,
    /// Opens a file with the system's default app.
    OpenFile,
    Download,
    /// Sends to a device.
    Send,
    Rename,
    Move,
    Details,
    Delete,
}

impl Action {
    fn icon(self) -> Icon {
        match self {
            Self::Open => Icon::Folder,
            Self::OpenFile => Icon::Web,
            Self::Download => Icon::Download,
            Self::Send => Icon::Send,
            Self::Rename => Icon::Rename,
            Self::Move => Icon::Move,
            Self::Details => Icon::Info,
            Self::Delete => Icon::Delete,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Open | Self::OpenFile => "Open",
            Self::Download => "Download",
            Self::Send => "Send",
            Self::Rename => "Rename",
            Self::Move => "Move",
            Self::Details => "Details",
            Self::Delete => "Delete",
        }
    }

    /// The label in a menu, which spells out what a toolbar's room leaves out.
    fn menu_label(self) -> &'static str {
        match self {
            Self::Move => "Move…",
            Self::Send => "Send to device…",
            Self::Delete => "Move to bin",
            _ => self.label(),
        }
    }

    fn shortcut(self) -> &'static str {
        match self {
            Self::Open | Self::Download => "Enter",
            Self::Rename => "F2",
            Self::OpenFile | Self::Send | Self::Move | Self::Details => "",
            Self::Delete => DELETE_SHORTCUT,
        }
    }
}

/// The actions shown for `selected`, each with whether it is enabled.
/// Lost & Found, identified by `lost_and_found`, cannot be deleted.
pub fn actions(selected: &[&Node], lost_and_found: NodeId) -> Vec<(Action, bool)> {
    let single = selected.len() == 1;
    let any = !selected.is_empty();
    let deletable = any && selected.iter().all(|node| node.id != lost_and_found);
    let open = match selected {
        [node] if node.kind == NodeKind::Folder => Some((Action::Open, true)),
        [_] => Some((Action::OpenFile, true)),
        _ => None,
    };
    open.into_iter()
        .chain([
            (Action::Download, any),
            (Action::Send, any),
            (Action::Move, any),
            (Action::Rename, single),
            (Action::Details, single),
            (Action::Delete, deletable),
        ])
        .collect()
}

/// A toolbar button before it is laid out.
struct Tool<'a> {
    icon: Icon,
    label: &'a str,
    shortcut: &'a str,
    message: Option<Message>,
    look: Look,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Ghost,
    Primary,
}

impl<'a> Tool<'a> {
    fn new(icon: Icon, label: &'a str, shortcut: &'a str, message: Option<Message>) -> Self {
        Self {
            icon,
            label,
            shortcut,
            message,
            look: Look::Ghost,
        }
    }

    fn primary(self) -> Self {
        Self {
            look: Look::Primary,
            ..self
        }
    }

    fn padding(&self) -> f32 {
        match self.look {
            Look::Primary => PRIMARY_PADDING,
            Look::Ghost => BUTTON_PADDING,
        }
    }

    /// Roughly how wide the button is, with or without its label.
    fn width(&self, labelled: bool) -> f32 {
        let label = if labelled {
            LABEL_SPACING + text_width(self.label)
        } else {
            0.0
        };
        self.padding() * 2.0 + ICON_SIZE + label
    }
}

/// The toolbar, `width` wide. Normally it holds the folder commands; once several nodes
/// are selected a tinted bar replaces it, whose count opens the actions on them. When the labels
/// do not fit,
/// the buttons show only their icons and the label moves into the tooltip.
/// `can_add` is false where nothing may be created, such as Lost & Found.
pub fn view<'a>(
    selected: &[&Node],
    can_add: bool,
    folder: Option<NodeId>,
    chip: Option<SyncChip>,
    width: f32,
) -> Element<'a, Message> {
    if client_core::selection::shows_bulk_actions(selected.len()) {
        return selection_bar(selected.len(), width);
    }
    let tools = [
        Tool::new(
            Icon::Upload,
            "Upload",
            UPLOAD_SHORTCUT,
            can_add.then_some(Message::Upload),
        )
        .primary(),
        Tool::new(Icon::Send, "Send…", "", Some(Message::SendLocal)),
        Tool::new(
            Icon::NewFolder,
            "New folder",
            NEW_FOLDER_SHORTCUT,
            can_add.then_some(Message::NewFolder),
        ),
        Tool::new(Icon::Refresh, "Refresh", "F5", Some(Message::Refresh)),
        Tool::new(
            Icon::Web,
            "Open in web",
            "",
            Some(Message::OpenInWeb(folder)),
        ),
    ];
    let labelled = fits(tools.iter(), width);
    let used: f32 = tools.iter().map(|tool| tool.width(true) + SPACING).sum();
    let mut bar = row![]
        .spacing(SPACING)
        .align_y(Alignment::Center)
        .extend(tools.into_iter().map(|tool| tool.view(labelled)))
        .width(Length::Fill);
    if labelled && let Some(chip) = chip.and_then(|chip| chip.view(width - used - MIN_GAP)) {
        bar = bar.push(space::horizontal()).push(chip);
    }
    bar.into()
}

/// The pill above the listing saying what the sync does with the folder shown.
pub struct SyncChip {
    pub icon: Icon,
    pub tint: Tint,
    pub label: String,
}

impl SyncChip {
    /// The chip, its label cut to fit in `room`, or nothing when too little of it would show.
    fn view<'a>(self, room: f32) -> Option<Element<'a, Message>> {
        let chrome = CHIP_LEFT + CHIP_ICON + CHIP_GAP + CHIP_RIGHT;
        let room = room - chrome;
        if room < CHIP_MIN_LABEL {
            return None;
        }
        // A pixel more than measured, as the text would wrap at exactly its own width.
        let width = room.min(text_width_at(&self.label) + 1.0);
        Some(
            container(
                row![
                    self.icon.view(CHIP_ICON, self.tint),
                    ellipsized(self.label, CHIP_SIZE, Alignment::Start).width(width)
                ]
                .spacing(CHIP_GAP)
                .align_y(Alignment::Center),
            )
            .padding(Padding::ZERO.left(CHIP_LEFT).right(CHIP_RIGHT))
            .center_y(CHIP_HEIGHT)
            .style(theme::chip)
            .into(),
        )
    }
}

fn text_width_at(label: &str) -> f32 {
    crate::ellipsis::text_width(label, CHIP_SIZE)
}

/// What replaces the toolbar while several nodes are selected: the count, which opens a menu of
/// the actions on them below it, and the way out. Only the way out folds to an icon, so the
/// count always says what it counts.
fn selection_bar<'a>(count: usize, width: f32) -> Element<'a, Message> {
    let label = count_label(count);
    let labelled = clear_fits(&label, width - BAR_PADDING * 2.0);
    let clear: Element<'a, Message> = if labelled {
        button(
            row![
                text(CLEAR_SELECTION).font(theme::MEDIUM),
                kbd(CLEAR_SHORTCUT)
            ]
            .spacing(LABEL_SPACING)
            .align_y(Alignment::Center),
        )
        .padding(BUTTON_PADDING_Y_X)
        .style(theme::tool)
        .on_press(Message::ClearSelection)
        .into()
    } else {
        let clear = button(Icon::Close.view(ICON_SIZE, Tint::Muted))
            .padding(BUTTON_PADDING_Y_X)
            .style(theme::tool)
            .on_press(Message::ClearSelection);
        tip(clear.into(), CLEAR_SELECTION, CLEAR_SHORTCUT)
    };
    let bar = row![selection_menu(label), space::horizontal(), clear]
        .spacing(SPACING)
        .align_y(Alignment::Center);
    container(bar)
        .padding(BAR_PADDING)
        .width(Length::Fill)
        .style(theme::selection_bar)
        .into()
}

/// The count of the selection as a dropdown, whose menu hangs from its left edge.
fn selection_menu<'a>(label: String) -> Element<'a, Message> {
    let button = button(
        row![
            text(label).font(theme::MEDIUM).style(theme::accent_text),
            Icon::ExpandMore.view(ICON_SIZE, Tint::Accent),
        ]
        .spacing(LABEL_SPACING)
        .align_y(Alignment::Center),
    )
    .padding(BUTTON_PADDING_Y_X)
    .style(theme::tool)
    // Never sent: the anchor takes the press, and only needs the button to look enabled.
    .on_press(Message::CloseMenu);
    Anchor::new(button)
        .on_press(|bounds| Message::SelectionMenu(Placement::hanging(bounds, Horizontal::Left)))
        .into()
}

/// Whether the selection count, `label`, and Clear selection with its key fit in `width`.
fn clear_fits(label: &str, width: f32) -> bool {
    let menu = BUTTON_PADDING * 2.0 + text_width(label) + LABEL_SPACING + ICON_SIZE;
    let clear = BUTTON_PADDING * 2.0
        + text_width(CLEAR_SELECTION)
        + LABEL_SPACING
        + text_width(CLEAR_SHORTCUT);
    menu + clear + SPACING + MIN_GAP <= width
}

/// Whether `tools` fit in `width` with their labels.
fn fits<'t, 'a: 't>(tools: impl Iterator<Item = &'t Tool<'a>>, width: f32) -> bool {
    let (count, buttons) = tools.fold((0, 0.0), |(count, sum), tool| {
        (count + 1, sum + tool.width(true))
    });
    buttons + SPACING * count as f32 + MIN_GAP <= width
}

fn count_label(count: usize) -> String {
    format!("{count} selected")
}

/// An estimate of how wide `label` renders at the default text size.
fn text_width(label: &str) -> f32 {
    label.chars().count() as f32 * CHAR_WIDTH
}

/// A keyboard shortcut drawn as a key.
pub(super) fn kbd<'a>(label: &'a str) -> Element<'a, Message> {
    container(text(label).size(KBD_SIZE).font(theme::SEMIBOLD))
        .padding(KBD_PADDING)
        .style(theme::kbd)
        .into()
}

/// A menu entry for the context menu.
pub fn menu_item<'a>(action: Action, enabled: bool) -> Element<'a, Message> {
    let tint = match action {
        _ if !enabled => Tint::Disabled,
        Action::Delete => Tint::Danger,
        _ => Tint::Muted,
    };
    let style = match action {
        Action::Delete => theme::danger_tool,
        _ => theme::tool,
    };
    menu_row(
        action.icon(),
        tint,
        action.menu_label(),
        action.shortcut(),
        style,
        enabled.then_some(Message::Act(action)),
    )
}

/// One row of the context menu; every entry is laid out by this.
pub fn menu_row<'a>(
    icon: Icon,
    tint: Tint,
    label: &'a str,
    shortcut: &'a str,
    style: fn(&iced::Theme, button::Status) -> button::Style,
    message: Option<Message>,
) -> Element<'a, Message> {
    button(
        row![
            icon.view(ICON_SIZE, tint),
            text(label)
                .width(Length::Fill)
                .wrapping(text::Wrapping::None),
            text(shortcut).size(TIP_SIZE).style(theme::muted_text),
        ]
        .spacing(MENU_GAP)
        .height(Length::Fill)
        .align_y(Alignment::Center),
    )
    .padding(MENU_ROW_PADDING)
    .width(Length::Fill)
    .height(context_menu::ITEM_HEIGHT)
    .style(style)
    .on_press_maybe(message)
    .into()
}

impl<'a> Tool<'a> {
    fn view(self, labelled: bool) -> Element<'a, Message> {
        let padding = self.padding();
        let (style, tint): (fn(&iced::Theme, button::Status) -> button::Style, Tint) =
            match self.look {
                Look::Ghost => (theme::tool, Tint::Text),
                Look::Primary => (theme::primary, Tint::OnAccent),
            };
        let tint = if self.message.is_some() || self.look == Look::Primary {
            tint
        } else {
            Tint::Disabled
        };
        let mut content = row![self.icon.view(ICON_SIZE, tint)]
            .spacing(LABEL_SPACING)
            .align_y(Alignment::Center);
        if labelled {
            content = content.push(text(self.label).font(theme::MEDIUM));
        }
        let tool = button(content)
            .padding([BUTTON_PADDING_Y, padding])
            .style(style)
            .on_press_maybe(self.message);
        match (labelled, self.shortcut.is_empty()) {
            (true, true) => tool.into(),
            (true, false) => tip(tool.into(), "", self.shortcut),
            (false, _) => tip(tool.into(), self.label, self.shortcut),
        }
    }
}

/// `content` with a tooltip below it naming `label` and its `shortcut`.
pub(super) fn tip<'a>(
    content: Element<'a, Message>,
    label: &'a str,
    shortcut: &'a str,
) -> Element<'a, Message> {
    let mut lines = row![].spacing(LABEL_SPACING).align_y(Alignment::Center);
    if !label.is_empty() {
        lines = lines.push(text(label).size(TIP_SIZE));
    }
    if !shortcut.is_empty() {
        lines = lines.push(text(shortcut).size(TIP_SIZE).style(theme::muted_text));
    }
    tooltip(
        content,
        container(lines).padding(TIP_PADDING).style(theme::menu),
        tooltip::Position::Bottom,
    )
    .into()
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    fn file(raw: u128) -> Node {
        Node {
            id: NodeId(Uuid::from_u128(raw)),
            parent_id: None,
            kind: NodeKind::File,
            name: format!("file-{raw}"),
            size: 0,
            sha256: None,
            mime: String::new(),
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    const LOST_AND_FOUND: NodeId = NodeId(Uuid::from_u128(99));

    fn delete_enabled(selected: &[&Node]) -> bool {
        actions(selected, LOST_AND_FOUND)
            .into_iter()
            .any(|(action, enabled)| action == Action::Delete && enabled)
    }

    fn details_enabled(selected: &[&Node]) -> bool {
        actions(selected, LOST_AND_FOUND)
            .into_iter()
            .any(|(action, enabled)| action == Action::Details && enabled)
    }

    #[test]
    fn details_are_available_for_a_single_node() {
        let node = file(1);
        assert!(details_enabled(&[&node]));
    }

    #[test]
    fn details_are_disabled_for_several_nodes() {
        let (first, second) = (file(1), file(2));
        assert!(!details_enabled(&[&first, &second]));
    }

    #[test]
    fn delete_is_available_for_ordinary_nodes() {
        let (first, second) = (file(1), file(2));
        assert!(delete_enabled(&[&first, &second]));
    }

    #[test]
    fn delete_is_disabled_for_lost_and_found() {
        let folder = file(99);
        assert!(!delete_enabled(&[&folder]));
    }

    #[test]
    fn delete_is_disabled_when_the_selection_holds_lost_and_found() {
        let (first, folder) = (file(1), file(99));
        assert!(!delete_enabled(&[&first, &folder]));
    }

    #[test]
    fn rename_and_move_stay_available_for_lost_and_found() {
        let folder = file(99);
        let enabled: Vec<_> = actions(&[&folder], LOST_AND_FOUND)
            .into_iter()
            .filter(|(_, enabled)| *enabled)
            .map(|(action, _)| action)
            .collect();
        assert!(enabled.contains(&Action::Rename) && enabled.contains(&Action::Move));
    }

    fn first_action(selected: &[&Node]) -> Option<Action> {
        actions(selected, LOST_AND_FOUND)
            .first()
            .map(|(action, _)| *action)
    }

    #[test]
    fn a_single_file_opens_first() {
        let node = file(1);
        assert_eq!(first_action(&[&node]), Some(Action::OpenFile));
    }

    #[test]
    fn a_single_folder_opens_first() {
        let node = Node {
            kind: NodeKind::Folder,
            ..file(1)
        };
        assert_eq!(first_action(&[&node]), Some(Action::Open));
    }

    #[test]
    fn several_nodes_cannot_be_opened() {
        let (first, second) = (file(1), file(2));
        assert_eq!(first_action(&[&first, &second]), Some(Action::Download));
    }

    #[test]
    fn clear_selection_keeps_its_label_in_a_wide_window() {
        assert!(clear_fits(&count_label(2), 1000.0));
    }

    #[test]
    fn clear_selection_folds_to_an_icon_in_a_narrow_window() {
        assert!(!clear_fits(&count_label(2), 260.0));
    }

    #[test]
    fn the_count_says_what_it_counts_at_any_width() {
        assert_eq!(count_label(3), "3 selected");
    }
}
