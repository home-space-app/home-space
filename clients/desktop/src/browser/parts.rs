//! The pieces the browser's pages share: the box a list sits in, its header and buttons.

use iced::{
    Alignment, Element, Length, Padding, Rectangle,
    widget::{Column, Row, button, center, container, row, space, text},
};

use super::anchor::Anchor;
use crate::{
    ellipsis::ellipsized,
    icons::{ICON_SIZE, Icon, Tint},
    theme,
};

pub(super) const LIST_PADDING: Padding = Padding {
    top: 0.0,
    right: 8.0,
    bottom: 0.0,
    left: 16.0,
};
/// A list with checkboxes starts closer to the edge, to fit them.
pub(super) const CHECK_LIST_PADDING: Padding = Padding {
    top: 0.0,
    right: 8.0,
    bottom: 0.0,
    left: 12.0,
};
/// The same with no menu column to keep the last column clear of the scrollbar.
pub(super) const CHECK_LIST_PADDING_WIDE: Padding = Padding {
    right: 20.0,
    ..CHECK_LIST_PADDING
};
/// The same for a list without checkboxes.
pub(super) const LIST_PADDING_WIDE: Padding = Padding {
    right: 20.0,
    ..LIST_PADDING
};
pub(super) const COLUMN_GAP: f32 = 8.0;
pub(super) const ROW_HEIGHT: f32 = 48.0;
pub(super) const ROW_SIZE: f32 = 14.0;
pub(super) const SECOND_LINE_SIZE: f32 = 12.0;
pub(super) const SIZE_WIDTH: f32 = 110.0;
pub(super) const DATE_WIDTH: f32 = 170.0;
pub(super) const MENU_WIDTH: f32 = 44.0;
pub(super) const EMPTY_PADDING: f32 = 32.0;
const MENU_BUTTON: f32 = 32.0;
const HEADER_HEIGHT: f32 = 40.0;
pub(super) const HEADER_SIZE: f32 = 12.0;
pub(super) const DIVIDER: f32 = 1.0;
const BOX_PADDING: f32 = 1.0;
const LABEL_GAP: f32 = 6.0;
const BUTTON_PADDING_Y: f32 = 8.0;
const GHOST_PADDING_X: f32 = 10.0;
const OUTLINED_PADDING_X: f32 = 16.0;
const SMALL_PADDING: [f32; 2] = [5.0, 10.0];
const SMALL_SIZE: f32 = 13.0;
const SMALL_ICON: f32 = 16.0;
const ALERT_PADDING: [f32; 2] = [10.0, 12.0];
const ALERT_GAP: f32 = 10.0;
pub(super) const DIALOG_WIDTH: f32 = 440.0;
const DIALOG_PADDING: f32 = 24.0;
const DIALOG_GAP: f32 = 16.0;
const DIALOG_TITLE_SIZE: f32 = 18.0;
const DIALOG_BUTTON_GAP: f32 = 8.0;
const DIALOG_HEAD_GAP: f32 = 2.0;
const FIELD_GAP: f32 = 4.0;
pub(crate) const FIELD_PADDING: [f32; 2] = [9.0, 10.0];

/// A hairline across the box.
pub(super) fn divider<'a, M: 'a>() -> Element<'a, M> {
    container(space())
        .width(Length::Fill)
        .height(DIVIDER)
        .style(theme::divider)
        .into()
}

/// `content` in the box that rests on the window, reaching its edges inside the border.
pub(super) fn list_box<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    container(content)
        .padding(BOX_PADDING)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::card)
        .into()
}

/// The tinted strip with the column titles, and the hairline under it.
pub(super) fn header_bar<'a, M: 'a>(cells: Row<'a, M>, padding: Padding) -> Element<'a, M> {
    iced::widget::column![
        container(cells.spacing(COLUMN_GAP).align_y(Alignment::Center))
            .padding(padding)
            .center_y(HEADER_HEIGHT)
            .width(Length::Fill)
            .style(theme::header),
        divider()
    ]
    .into()
}

/// A column title.
pub(super) fn title<'a, M: 'a>(
    label: &str,
    width: impl Into<Length>,
    align: Alignment,
) -> Element<'a, M> {
    text(label.to_uppercase())
        .size(HEADER_SIZE)
        .font(theme::SEMIBOLD)
        .width(width)
        .align_x(align)
        .into()
}

/// A cell in the secondary text colour, cut short when it does not fit.
pub(super) fn muted_cell<'a, M: 'a>(
    content: impl Into<String>,
    width: impl Into<Length>,
    align: Alignment,
) -> Element<'a, M> {
    container(ellipsized(content, ROW_SIZE, align).width(Length::Fill))
        .width(width)
        .style(theme::muted)
        .into()
}

/// The three dots of a row, which open its menu hanging from `on_press`'s bounds. `filler` is
/// never sent: the anchor takes the press, and the button only needs a handler to look enabled.
pub(super) fn menu_button<'a, M: Clone + 'a>(
    filler: M,
    on_press: impl Fn(Rectangle) -> M + 'a,
) -> Element<'a, M> {
    let button = button(center(Icon::More.view(ICON_SIZE, Tint::Muted)))
        .padding(0)
        .width(MENU_BUTTON)
        .height(MENU_BUTTON)
        .style(theme::tool)
        .on_press(filler);
    container(Anchor::new(button).on_press(on_press))
        .width(MENU_WIDTH)
        .into()
}

/// The empty state of a list.
pub(super) fn empty<'a, M: 'a>(hint: &'a str) -> Element<'a, M> {
    container(text(hint).style(theme::muted_text))
        .padding(EMPTY_PADDING)
        .center_x(Length::Fill)
        .into()
}

/// A notice over the page saying what went wrong, with what can be done about it.
pub(crate) fn alert_error<'a, M: 'a>(
    icon: Icon,
    message: impl text::IntoFragment<'a>,
    action: Option<Element<'a, M>>,
) -> Element<'a, M> {
    alert(icon, Tint::Danger, message, action, theme::alert_error)
}

/// A notice over the page saying how something went.
pub(super) fn alert_info<'a, M: 'a>(
    icon: Icon,
    message: impl text::IntoFragment<'a>,
    action: Option<Element<'a, M>>,
) -> Element<'a, M> {
    alert(icon, Tint::Accent, message, action, theme::alert_info)
}

fn alert<'a, M: 'a>(
    icon: Icon,
    tint: Tint,
    message: impl text::IntoFragment<'a>,
    action: Option<Element<'a, M>>,
    style: fn(&iced::Theme) -> container::Style,
) -> Element<'a, M> {
    container(
        row![
            icon.view(ICON_SIZE, tint),
            text(message).width(Length::Fill)
        ]
        .push(action)
        .spacing(ALERT_GAP)
        .align_y(Alignment::Center),
    )
    .padding(ALERT_PADDING)
    .width(Length::Fill)
    .style(style)
    .into()
}

type ButtonStyle = fn(&iced::Theme, button::Status) -> button::Style;

fn labelled<'a, M: Clone + 'a>(
    icon: Option<(Icon, Tint)>,
    label: impl text::IntoFragment<'a>,
    size: f32,
    icon_size: f32,
    message: Option<M>,
) -> button::Button<'a, M> {
    let enabled = message.is_some();
    let icon =
        icon.map(|(icon, tint)| icon.view(icon_size, if enabled { tint } else { Tint::Disabled }));
    button(
        row![]
            .push(icon)
            .push(text(label).size(size).font(theme::MEDIUM))
            .spacing(LABEL_GAP)
            .align_y(Alignment::Center),
    )
    .on_press_maybe(message)
}

fn sized<'a, M: Clone + 'a>(
    icon: Option<(Icon, Tint)>,
    label: impl text::IntoFragment<'a>,
    style: ButtonStyle,
    padding_x: f32,
    message: Option<M>,
) -> Element<'a, M> {
    labelled(icon, label, theme::TEXT_SIZE, ICON_SIZE, message)
        .padding([BUTTON_PADDING_Y, padding_x])
        .style(style)
        .into()
}

/// A flat button.
pub(super) fn ghost<'a, M: Clone + 'a>(
    icon: Option<Icon>,
    label: impl text::IntoFragment<'a>,
    message: Option<M>,
) -> Element<'a, M> {
    let icon = icon.map(|icon| (icon, Tint::Text));
    sized(icon, label, theme::tool, GHOST_PADDING_X, message)
}

/// A flat button for something destructive.
pub(super) fn ghost_danger<'a, M: Clone + 'a>(
    icon: Icon,
    label: impl text::IntoFragment<'a>,
    message: Option<M>,
) -> Element<'a, M> {
    sized(
        Some((icon, Tint::Danger)),
        label,
        theme::danger_tool,
        GHOST_PADDING_X,
        message,
    )
}

/// An outlined button.
pub(crate) fn outlined<'a, M: Clone + 'a>(
    icon: Option<Icon>,
    label: impl text::IntoFragment<'a>,
    message: Option<M>,
) -> Element<'a, M> {
    let icon = icon.map(|icon| (icon, Tint::Text));
    sized(icon, label, theme::secondary, OUTLINED_PADDING_X, message)
}

/// A small outlined button, inside a row or a notice.
pub(super) fn small_outlined<'a, M: Clone + 'a>(
    label: impl text::IntoFragment<'a>,
    message: Option<M>,
) -> Element<'a, M> {
    labelled(None, label, SMALL_SIZE, SMALL_ICON, message)
        .padding(SMALL_PADDING)
        .style(theme::secondary)
        .into()
}

/// A small outlined button with an icon.
pub(super) fn small_outlined_icon<'a, M: Clone + 'a>(
    icon: Icon,
    label: impl text::IntoFragment<'a>,
    message: Option<M>,
) -> Element<'a, M> {
    labelled(
        Some((icon, Tint::Text)),
        label,
        SMALL_SIZE,
        SMALL_ICON,
        message,
    )
    .padding(SMALL_PADDING)
    .style(theme::secondary)
    .into()
}

/// A small button for something destructive.
pub(super) fn small_danger<'a, M: Clone + 'a>(
    label: impl text::IntoFragment<'a>,
    message: Option<M>,
) -> Element<'a, M> {
    labelled(None, label, SMALL_SIZE, SMALL_ICON, message)
        .padding(SMALL_PADDING)
        .style(theme::danger)
        .into()
}

/// The button that carries out a dialog.
pub(crate) fn primary<'a, M: Clone + 'a>(
    icon: Option<Icon>,
    label: impl text::IntoFragment<'a>,
    message: Option<M>,
) -> Element<'a, M> {
    let icon = icon.map(|icon| (icon, Tint::OnAccent));
    sized(icon, label, theme::primary, OUTLINED_PADDING_X, message)
}

/// The button that carries out a dialog doing something destructive.
pub(super) fn danger<'a, M: Clone + 'a>(
    label: impl text::IntoFragment<'a>,
    message: Option<M>,
) -> Element<'a, M> {
    sized(None, label, theme::danger, OUTLINED_PADDING_X, message)
}

/// The title of a dialog.
pub(super) fn dialog_title<'a, M: 'a>(title: impl text::IntoFragment<'a>) -> Element<'a, M> {
    text(title)
        .size(DIALOG_TITLE_SIZE)
        .font(theme::SEMIBOLD)
        .into()
}

/// A dialog of `lines`, closed by the `buttons` on its last line.
pub(super) fn dialog<'a, M: 'a>(
    width: f32,
    lines: Column<'a, M>,
    buttons: impl IntoIterator<Item = Element<'a, M>>,
) -> Element<'a, M> {
    dialog_with_aside(width, lines, None, buttons)
}

/// The title of a dialog with a line about it below.
pub(super) fn dialog_head<'a, M: 'a>(
    title: impl text::IntoFragment<'a>,
    subtitle: impl text::IntoFragment<'a>,
) -> Element<'a, M> {
    iced::widget::column![
        dialog_title(title),
        text(subtitle)
            .size(SECOND_LINE_SIZE)
            .style(theme::muted_text),
    ]
    .spacing(DIALOG_HEAD_GAP)
    .into()
}

/// A dialog like [`dialog`] with `aside`, such as a secondary action, at the start of its last
/// line.
pub(super) fn dialog_with_aside<'a, M: 'a>(
    width: f32,
    lines: Column<'a, M>,
    aside: Option<Element<'a, M>>,
    buttons: impl IntoIterator<Item = Element<'a, M>>,
) -> Element<'a, M> {
    let buttons = row![]
        .push(aside)
        .push(space::horizontal())
        .extend(buttons)
        .spacing(DIALOG_BUTTON_GAP)
        .align_y(Alignment::Center);
    container(lines.push(buttons).spacing(DIALOG_GAP))
        .padding(DIALOG_PADDING)
        .width(width)
        .style(theme::panel)
        .into()
}

/// `input` with the `label` above it and, if any, a `hint` below.
pub(crate) fn field<'a, M: 'a>(
    label: &'a str,
    input: impl Into<Element<'a, M>>,
    hint: Option<String>,
) -> Element<'a, M> {
    iced::widget::column![text(label).font(theme::MEDIUM), input.into()]
        .push(hint.map(|hint| text(hint).size(SECOND_LINE_SIZE).style(theme::muted_text)))
        .spacing(FIELD_GAP)
        .into()
}

/// A small primary button.
pub(super) fn small_primary<'a, M: Clone + 'a>(
    icon: Icon,
    label: impl text::IntoFragment<'a>,
    message: Option<M>,
) -> Element<'a, M> {
    labelled(
        Some((icon, Tint::OnAccent)),
        label,
        SMALL_SIZE,
        SMALL_ICON,
        message,
    )
    .padding(SMALL_PADDING)
    .style(theme::primary)
    .into()
}
