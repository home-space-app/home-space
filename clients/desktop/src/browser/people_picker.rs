//! The panel that picks people on the server: a search box over everyone, each with a
//! checkbox. The browser floats it over the dialog it opens from.

use iced::{
    Alignment, Element, Length,
    widget::{button, column, container, row, scrollable, space, text, text_input},
};

use client_core::shared_link::initial;
use protocol::{UserId, UserRef};

use super::parts::{self, SECOND_LINE_SIZE, divider};
use crate::{
    icons::{Icon, Tint},
    theme,
};

/// How wide the panel is unless what it opens from is wider.
pub const MIN_WIDTH: f32 = 300.0;
const LIST_HEIGHT: f32 = 180.0;
const PADDING: f32 = 6.0;
const GAP: f32 = 4.0;
const ROW_GAP: f32 = 12.0;
const ROW_HEIGHT: f32 = 36.0;
const ROW_PADDING: [f32; 2] = [0.0, 10.0];
const NOTE_PADDING: [f32; 2] = [8.0, 10.0];
const CHECKBOX: f32 = 18.0;
const CHECK_MARK: f32 = 14.0;
const AVATAR: f32 = 18.0;
const AVATAR_TEXT: f32 = 11.0;
const SEARCH_PEOPLE: &str = "Search people";
const NOBODY_MATCHES: &str = "Nobody else matches.";
const SERVER_ONLY: &str = "Only accounts on this server are listed.";

/// What the picker lists and how it answers.
pub struct Picker<'a, M> {
    pub users: &'a [UserRef],
    pub query: &'a str,
    /// The id of the search box, to focus it when the picker opens.
    pub search_id: &'static str,
    pub width: f32,
    pub on_query: fn(String) -> M,
    /// Picks a user, or takes back one picked.
    pub on_toggle: fn(UserRef) -> M,
}

impl<'a, M: Clone + 'a> Picker<'a, M> {
    /// The panel, with the users `picked` says are picked checked.
    pub fn view(self, picked: impl Fn(UserId) -> bool) -> Element<'a, M> {
        let shown = matching(self.users, self.query);
        let rows: Element<'a, M> = if shown.is_empty() {
            container(text(NOBODY_MATCHES).style(theme::muted_text))
                .padding(NOTE_PADDING)
                .into()
        } else {
            let rows = shown.into_iter().map(|user| {
                button(
                    row![
                        checkbox(picked(user.id)),
                        avatar(&user.username),
                        text(user.username.as_str())
                    ]
                    .spacing(ROW_GAP)
                    .align_y(Alignment::Center),
                )
                .padding(ROW_PADDING)
                .width(Length::Fill)
                .height(ROW_HEIGHT)
                .style(theme::tool)
                .on_press((self.on_toggle)(user.clone()))
                .into()
            });
            scrollable(column(rows)).into()
        };
        container(
            column![
                text_input(SEARCH_PEOPLE, self.query)
                    .id(self.search_id)
                    .style(theme::input)
                    .padding(parts::FIELD_PADDING)
                    .on_input(self.on_query),
                container(rows).max_height(LIST_HEIGHT),
                divider(),
                container(
                    text(SERVER_ONLY)
                        .size(SECOND_LINE_SIZE)
                        .style(theme::muted_text)
                )
                .padding(NOTE_PADDING),
            ]
            .spacing(GAP),
        )
        .padding(PADDING)
        .width(self.width.max(MIN_WIDTH))
        .style(theme::menu)
        .into()
    }
}

fn avatar<'a, M: 'a>(name: &str) -> Element<'a, M> {
    container(text(initial(name)).size(AVATAR_TEXT).font(theme::BOLD))
        .center(AVATAR)
        .style(theme::avatar)
        .into()
}

fn checkbox<'a, M: 'a>(checked: bool) -> Element<'a, M> {
    let mark: Element<'a, M> = if checked {
        Icon::Check.view(CHECK_MARK, Tint::OnAccent)
    } else {
        space().into()
    };
    container(mark)
        .center(CHECKBOX)
        .style(theme::checkbox(checked))
        .into()
}

/// The users whose name contains `query`, ignoring case.
fn matching<'a>(users: &'a [UserRef], query: &str) -> Vec<&'a UserRef> {
    let query = query.trim().to_lowercase();
    users
        .iter()
        .filter(|user| query.is_empty() || user.username.to_lowercase().contains(&query))
        .collect()
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn user(raw: u128, name: &str) -> UserRef {
        UserRef {
            id: UserId(Uuid::from_u128(raw)),
            username: name.to_owned(),
        }
    }

    #[test]
    fn the_picker_filters_by_name_ignoring_case() {
        let users = [user(1, "dana"), user(2, "Alex"), user(3, "maria")];
        let names = |query| {
            matching(&users, query)
                .into_iter()
                .map(|user| user.username.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(""), ["dana", "Alex", "maria"]);
        assert_eq!(names(" AL "), ["Alex"]);
        assert!(names("zed").is_empty());
    }
}
