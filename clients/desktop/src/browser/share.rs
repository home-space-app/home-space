//! Sharing files and folders, read-only, with people on the server or through a link.

use iced::{
    Alignment, Element, Length, Task,
    alignment::Horizontal,
    widget::{button, column, container, row, space, text, text_input},
};
use time::{Duration, OffsetDateTime, UtcOffset};

use api_client::{ApiError, Client};
use client_core::{
    share::{
        ADD_PEOPLE_PLACEHOLDER, Audience, BAD_DATE, CAN_VIEW, CHANGE_PASSWORD, COPY_LINK,
        DATE_HINT, EXPIRES, Expiry, HIDDEN_PASSWORD, LINK_COPIED, OWNER, PASSWORD_PLACEHOLDER,
        PEOPLE_NOTE, PICK_A_DATE, REMOVE_PERSON, REQUIRE_PASSWORD, STOP_SHARING, ShareDraft,
        format_expiry_date, matching_share, parse_expiry_date, share_subtitle, share_title,
        stop_sharing_note, stop_sharing_question, you,
    },
    shared_link::initial,
};
use protocol::{Node, NodeId, Share, UpdateShare, UserId, UserRef};

use super::{
    anchor::Anchor,
    context_menu::Placement,
    parts::{self, SECOND_LINE_SIZE, divider},
    people_picker::{self, Picker},
};
use crate::{
    icons::{ICON_SIZE, Icon, Tint},
    locale::local_offset,
    theme,
};

const WIDTH: f32 = 560.0;
const SECTION_GAP: f32 = 8.0;
const ROW_GAP: f32 = 8.0;
const CHIP_GAP: f32 = 8.0;
const CHIP_PADDING: [f32; 2] = [5.0, 12.0];
const TAB_PADDING: [f32; 2] = [6.0, 14.0];
const CHIP_CHECK: f32 = 16.0;
const SWITCH_WIDTH: f32 = 36.0;
const SWITCH_HEIGHT: f32 = 20.0;
const SWITCH_INSET: f32 = 2.0;
const THUMB: f32 = 16.0;
const DATE_WIDTH: f32 = 200.0;
const AVATAR: f32 = 32.0;
const AVATAR_TEXT: f32 = 13.0;
const PERSON_PADDING: [f32; 2] = [8.0, 12.0];
const REMOVE_ICON: f32 = 18.0;
const PASSWORD_INPUT: &str = "share-password";
const SEARCH_INPUT: &str = "share-people-search";
const NO_LINK_YET: &str = "Save to make the link";
const ADDED: &str = "Added";
const SAVING: &str = "Saving…";
const CANCEL: &str = "Cancel";
const CLOSE: &str = "Close";
const SAVE: &str = "Save";
/// How far ahead the date field starts when the owner first picks a date.
const PICKED_DATE_AHEAD: Duration = Duration::days(7);

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<Share>, ApiError>),
    UsersLoaded(Result<Vec<UserRef>, ApiError>),
    Tab(Audience),
    /// Opens the people picker hanging from the field that adds people; the browser shows it.
    Pick(Placement),
    /// The field the people picker hangs from has moved; the browser moves the picker with it.
    Moved(Placement),
    Query(String),
    /// Adds a person to those the share is for, or takes them away.
    Toggle(UserRef),
    RemovePerson(UserId),
    TogglePassword,
    PasswordChanged(String),
    ChangePassword,
    Reveal,
    Expiry(Expiry),
    PickDate,
    DateChanged(String),
    Copy,
    Submit,
    Created(Result<Share, ApiError>),
    Updated(Result<Share, ApiError>),
    AskStop,
    KeepSharing,
    Stop,
    Stopped(Result<(), ApiError>),
    Cancel,
}

impl Message {
    /// Whether the message comes from the people picker, which stays open for more.
    pub fn keeps_picker_open(&self) -> bool {
        matches!(self, Self::Query(_) | Self::Toggle(_) | Self::Moved(_))
    }
}

/// What the browser should do after an update.
pub enum Outcome {
    Task(Task<Message>),
    Closed,
}

/// One tab of the dialog: what is drafted there, and the share it edits once there is one.
struct Tab {
    share: Option<Share>,
    draft: ShareDraft,
    /// The picked date as typed, which may not read as a date yet.
    date: String,
}

impl Tab {
    fn new(audience: Audience) -> Self {
        Self {
            share: None,
            draft: ShareDraft::for_audience(audience),
            date: String::new(),
        }
    }

    /// Takes `share` as the saved share the tab edits.
    fn show(&mut self, share: Share, offset: UtcOffset) {
        self.draft = ShareDraft::of(&share);
        self.date = date_text(&self.draft, offset);
        self.share = Some(share);
    }
}

fn date_text(draft: &ShareDraft, offset: UtcOffset) -> String {
    match draft.expiry {
        Expiry::On(at) => format_expiry_date(at, offset),
        _ => String::new(),
    }
}

/// The dialog that shares some files and folders with people or through a link, or edits the
/// shares of exactly them.
pub struct ShareDialog {
    nodes: Vec<Node>,
    /// The signed-in user, who owns what is shared.
    owner: String,
    audience: Audience,
    people: Tab,
    link: Tab,
    /// The other users of the server, once listed.
    users: Vec<UserRef>,
    /// What the people picker is searching for.
    query: String,
    /// How wide the people picker opens: as wide as the field it hangs from.
    picker_width: f32,
    loading: bool,
    saving: bool,
    error: Option<String>,
    copied: bool,
    reveal: bool,
    confirm_stop: bool,
    offset: UtcOffset,
}

impl ShareDialog {
    pub fn open(client: &Client, nodes: Vec<Node>, owner: String) -> (Self, Task<Message>) {
        let first = nodes.first().map(|node| node.id);
        let dialog = Self {
            nodes,
            owner,
            audience: Audience::People,
            people: Tab::new(Audience::People),
            link: Tab::new(Audience::Link),
            users: Vec::new(),
            query: String::new(),
            picker_width: people_picker::MIN_WIDTH,
            loading: true,
            saving: false,
            error: None,
            copied: false,
            reveal: false,
            confirm_stop: false,
            offset: local_offset(),
        };
        let shares = {
            let client = client.clone();
            Task::perform(async move { client.shares(first).await }, Message::Loaded)
        };
        let users = {
            let client = client.clone();
            Task::perform(async move { client.users().await }, Message::UsersLoaded)
        };
        (dialog, Task::batch([shares, users]))
    }

    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.iter().map(|node| node.id).collect()
    }

    fn tab(&self) -> &Tab {
        match self.audience {
            Audience::People => &self.people,
            Audience::Link => &self.link,
        }
    }

    fn tab_mut(&mut self) -> &mut Tab {
        match self.audience {
            Audience::People => &mut self.people,
            Audience::Link => &mut self.link,
        }
    }

    fn date_is_bad(&self) -> bool {
        let tab = self.tab();
        matches!(tab.draft.expiry, Expiry::On(_))
            && parse_expiry_date(&tab.date, self.offset).is_none()
    }

    pub fn update(&mut self, client: &Client, message: Message) -> Outcome {
        let now = OffsetDateTime::now_utc();
        match message {
            Message::Loaded(Ok(shares)) => {
                self.loading = false;
                let ids = self.node_ids();
                let with_people = matching_share(&shares, &ids, Audience::People).cloned();
                let with_link = matching_share(&shares, &ids, Audience::Link).cloned();
                if with_people.is_none() && with_link.is_some() {
                    self.audience = Audience::Link;
                }
                if let Some(found) = with_people {
                    self.people.show(found, self.offset);
                }
                if let Some(found) = with_link {
                    self.link.show(found, self.offset);
                }
            }
            Message::Loaded(Err(err)) | Message::UsersLoaded(Err(err)) => {
                self.loading = false;
                self.error = Some(err.to_string());
            }
            Message::UsersLoaded(Ok(users)) => self.users = users,
            Message::Tab(audience) => {
                self.audience = audience;
                self.error = None;
            }
            Message::Pick(placement) => {
                self.query.clear();
                self.picker_width = placement.width();
                return Outcome::Task(iced::widget::operation::focus(SEARCH_INPUT));
            }
            Message::Moved(placement) => self.picker_width = placement.width(),
            Message::Query(query) => self.query = query,
            Message::Toggle(user) => {
                let draft = &mut self.people.draft;
                if draft.has_recipient(user.id) {
                    draft.remove_recipient(user.id);
                } else {
                    draft.add_recipient(user);
                }
            }
            Message::RemovePerson(id) => self.people.draft.remove_recipient(id),
            Message::TogglePassword => {
                let draft = &mut self.link.draft;
                draft.set_require_password(!draft.require_password);
                if draft.editing_password() {
                    return Outcome::Task(iced::widget::operation::focus(PASSWORD_INPUT));
                }
            }
            Message::PasswordChanged(password) => self.link.draft.password = password,
            Message::ChangePassword => {
                self.link.draft.change_password();
                return Outcome::Task(iced::widget::operation::focus(PASSWORD_INPUT));
            }
            Message::Reveal => self.reveal = !self.reveal,
            Message::Expiry(expiry) => self.tab_mut().draft.expiry = expiry,
            Message::PickDate => {
                let offset = self.offset;
                let tab = self.tab_mut();
                if !matches!(tab.draft.expiry, Expiry::On(_)) {
                    let at = tab.draft.expires_at(now).unwrap_or(now + PICKED_DATE_AHEAD);
                    tab.draft.expiry = Expiry::On(at);
                    tab.date = date_text(&tab.draft, offset);
                }
            }
            Message::DateChanged(date) => {
                let offset = self.offset;
                let tab = self.tab_mut();
                if let Some(at) = parse_expiry_date(&date, offset) {
                    tab.draft.expiry = Expiry::On(at);
                }
                tab.date = date;
            }
            Message::Copy => {
                if let Some(token) = self
                    .link
                    .share
                    .as_ref()
                    .and_then(|share| share.token.as_deref())
                {
                    self.copied = true;
                    return Outcome::Task(iced::clipboard::write(client.share_url(token)));
                }
            }
            Message::Submit if !self.saving && !self.loading => return self.save(client, now),
            Message::Submit => {}
            Message::Created(Ok(share)) if share.is_link() => {
                self.saving = false;
                self.link.show(share, self.offset);
            }
            Message::Created(Ok(_)) | Message::Updated(Ok(_)) | Message::Stopped(Ok(())) => {
                return Outcome::Closed;
            }
            Message::Created(Err(err)) | Message::Updated(Err(err)) => {
                self.saving = false;
                self.error = Some(err.to_string());
            }
            Message::AskStop => self.confirm_stop = true,
            Message::KeepSharing => self.confirm_stop = false,
            Message::Stop => {
                if let Some(id) = self.tab().share.as_ref().map(|share| share.id) {
                    self.saving = true;
                    let client = client.clone();
                    return Outcome::Task(Task::perform(
                        async move { client.delete_share(id).await },
                        Message::Stopped,
                    ));
                }
            }
            Message::Stopped(Err(err)) => {
                self.saving = false;
                self.confirm_stop = false;
                self.error = Some(err.to_string());
            }
            Message::Cancel if self.confirm_stop => self.confirm_stop = false,
            Message::Cancel => return Outcome::Closed,
        }
        Outcome::Task(Task::none())
    }

    fn save(&mut self, client: &Client, now: OffsetDateTime) -> Outcome {
        if self.date_is_bad() {
            self.error = Some(BAD_DATE.to_owned());
            return Outcome::Task(Task::none());
        }
        let client = client.clone();
        let node_ids = self.node_ids();
        let tab = self.tab();
        let task = match tab.share.as_ref().map(|share| share.id) {
            Some(id) => tab.draft.update_request(now).map(|request| {
                (request != UpdateShare::default()).then(|| {
                    Task::perform(
                        async move { client.update_share(id, &request).await },
                        Message::Updated,
                    )
                })
            }),
            None => tab.draft.create_request(node_ids, now).map(|request| {
                Some(Task::perform(
                    async move { client.create_share(&request).await },
                    Message::Created,
                ))
            }),
        };
        let task = match task {
            Ok(Some(task)) => task,
            Ok(None) => return Outcome::Closed,
            Err(problem) => {
                self.error = Some(problem.message().to_owned());
                return Outcome::Task(Task::none());
            }
        };
        self.saving = true;
        self.error = None;
        Outcome::Task(task)
    }

    pub fn view(&self, client: &Client) -> Element<'_, Message> {
        if self.confirm_stop {
            return self.confirm_view();
        }
        let mut lines = column![
            parts::dialog_head(share_title(&self.nodes), share_subtitle(&self.nodes)),
            self.tabs(),
        ];
        lines = match self.audience {
            Audience::People => lines.push(self.people_section()),
            Audience::Link => {
                let link = self
                    .link
                    .share
                    .as_ref()
                    .and_then(|share| share.token.as_deref())
                    .map(|token| client.share_url(token));
                lines
                    .push(self.link_row(link))
                    .push(self.password_section())
            }
        };
        lines = lines.push(self.expiry_section());
        if self.audience == Audience::People {
            lines = lines.push(parts::alert_info(Icon::Info, PEOPLE_NOTE, None));
        }
        if let Some(error) = &self.error {
            lines = lines.push(parts::alert_error(Icon::Error, error.as_str(), None));
        }
        let aside = self.tab().share.as_ref().map(|_| {
            parts::ghost_danger(
                stop_icon(self.audience),
                STOP_SHARING,
                (!self.saving).then_some(Message::AskStop),
            )
        });
        let close = if self.tab().share.is_some() {
            CLOSE
        } else {
            CANCEL
        };
        let save = if self.saving { SAVING } else { SAVE };
        parts::dialog_with_aside(
            WIDTH,
            lines,
            aside,
            [
                parts::outlined(None, close, Some(Message::Cancel)),
                parts::primary(
                    Some(Icon::Check),
                    save,
                    (!self.saving && !self.loading).then_some(Message::Submit),
                ),
            ],
        )
    }

    fn confirm_view(&self) -> Element<'_, Message> {
        parts::dialog(
            WIDTH,
            column![parts::dialog_head(
                stop_sharing_question(&self.nodes),
                stop_sharing_note(self.audience)
            )],
            [
                parts::outlined(None, CANCEL, Some(Message::KeepSharing)),
                parts::danger(STOP_SHARING, (!self.saving).then_some(Message::Stop)),
            ],
        )
    }

    /// The people picker: a search box over the other users, each with a checkbox.
    pub fn picker(&self) -> Element<'_, Message> {
        Picker {
            users: &self.users,
            query: &self.query,
            search_id: SEARCH_INPUT,
            width: self.picker_width,
            on_query: Message::Query,
            on_toggle: Message::Toggle,
        }
        .view(|id| self.people.draft.has_recipient(id))
    }

    fn tabs(&self) -> Element<'_, Message> {
        row(Audience::ALL.into_iter().map(|audience| {
            let chosen = self.audience == audience;
            let icon = match audience {
                Audience::People => Icon::Group,
                Audience::Link => Icon::Link,
            };
            button(
                row![
                    icon.view(CHIP_CHECK, if chosen { Tint::Accent } else { Tint::Text }),
                    text(audience.label())
                ]
                .spacing(ROW_GAP / 2.0)
                .align_y(Alignment::Center),
            )
            .padding(TAB_PADDING)
            .style(theme::filter_chip(chosen))
            .on_press(Message::Tab(audience))
            .into()
        }))
        .spacing(CHIP_GAP / 2.0)
        .into()
    }

    fn people_section(&self) -> Element<'_, Message> {
        let field = button(
            row![
                Icon::PersonAdd.view(ICON_SIZE, Tint::Muted),
                text(ADD_PEOPLE_PLACEHOLDER).width(Length::Fill),
                Icon::ExpandMore.view(ICON_SIZE, Tint::Muted),
            ]
            .spacing(ROW_GAP)
            .align_y(Alignment::Center),
        )
        .padding(parts::FIELD_PADDING)
        .width(Length::Fill)
        .style(theme::combo)
        // Never sent: the anchor takes the press, and only needs the button to look enabled.
        .on_press(Message::Cancel);
        let field = Anchor::new(field)
            .on_press(|bounds| Message::Pick(Placement::hanging(bounds, Horizontal::Left)))
            .on_move(|bounds| Message::Moved(Placement::hanging(bounds, Horizontal::Left)));
        let permission = container(text(CAN_VIEW).style(theme::muted_text))
            .padding(parts::FIELD_PADDING)
            .style(theme::field);
        let section = column![
            row![field, permission]
                .spacing(ROW_GAP)
                .align_y(Alignment::Center)
        ]
        .spacing(SECTION_GAP);
        let saved = self.people.share.as_ref();
        let mut people = column![person_row(
            avatar(&self.owner, AVATAR, AVATAR_TEXT),
            you(&self.owner),
            None,
            text(OWNER).style(theme::muted_text).into(),
            None,
        )];
        for user in self.people.draft.recipients() {
            let added = saved.and_then(|share| {
                share
                    .recipients
                    .iter()
                    .find(|recipient| recipient.user.id == user.id)
                    .map(|recipient| {
                        format!(
                            "{ADDED} {}",
                            format_expiry_date(recipient.added_at, self.offset)
                        )
                    })
            });
            let remove = button(Icon::Close.view(REMOVE_ICON, Tint::Muted))
                .padding(4)
                .style(theme::tool)
                .on_press(Message::RemovePerson(user.id));
            people = people.push(divider()).push(person_row(
                avatar(&user.username, AVATAR, AVATAR_TEXT),
                user.username.clone(),
                added,
                text(CAN_VIEW).style(theme::muted_text).into(),
                Some(
                    iced::widget::tooltip(
                        remove,
                        text(format!("{REMOVE_PERSON} {}", user.username)),
                        iced::widget::tooltip::Position::Top,
                    )
                    .into(),
                ),
            ));
        }
        section
            .push(
                container(people)
                    .padding(1)
                    .width(Length::Fill)
                    .style(theme::card),
            )
            .into()
    }

    fn link_row(&self, link: Option<String>) -> Element<'_, Message> {
        let shown = match link {
            Some(link) => text(link),
            None => text(NO_LINK_YET).style(theme::muted_text),
        };
        let field = container(
            row![
                Icon::Link.view(ICON_SIZE, Tint::Muted),
                shown.wrapping(text::Wrapping::None)
            ]
            .spacing(ROW_GAP)
            .align_y(Alignment::Center),
        )
        .padding(parts::FIELD_PADDING)
        .width(Length::Fill)
        .clip(true)
        .style(theme::field);
        let copy = if self.copied { LINK_COPIED } else { COPY_LINK };
        row![
            field,
            parts::outlined(
                Some(Icon::Copy),
                copy,
                self.link.share.as_ref().map(|_| Message::Copy)
            ),
        ]
        .spacing(ROW_GAP)
        .align_y(Alignment::Center)
        .into()
    }

    fn password_section(&self) -> Element<'_, Message> {
        let draft = &self.link.draft;
        let head = row![
            Icon::Lock.view(ICON_SIZE, Tint::Text),
            text(REQUIRE_PASSWORD).font(theme::MEDIUM),
            space::horizontal(),
            switch(draft.require_password, Message::TogglePassword),
        ]
        .spacing(ROW_GAP)
        .align_y(Alignment::Center);
        let mut section = column![head].spacing(SECTION_GAP);
        if draft.editing_password() {
            let reveal_icon = if self.reveal {
                Icon::VisibilityOff
            } else {
                Icon::Visibility
            };
            section = section.push(
                row![
                    text_input(PASSWORD_PLACEHOLDER, &draft.password)
                        .id(PASSWORD_INPUT)
                        .secure(!self.reveal)
                        .style(theme::input)
                        .padding(parts::FIELD_PADDING)
                        .on_input(Message::PasswordChanged)
                        .on_submit(Message::Submit),
                    parts::ghost(Some(reveal_icon), "", Some(Message::Reveal)),
                ]
                .spacing(ROW_GAP)
                .align_y(Alignment::Center),
            );
        } else if draft.offers_change_password() {
            section = section.push(
                row![
                    container(text(HIDDEN_PASSWORD))
                        .padding(parts::FIELD_PADDING)
                        .width(Length::Fill)
                        .style(theme::field),
                    parts::outlined(
                        Some(Icon::Key),
                        CHANGE_PASSWORD,
                        Some(Message::ChangePassword)
                    ),
                ]
                .spacing(ROW_GAP)
                .align_y(Alignment::Center),
            );
        }
        section.into()
    }

    fn expiry_section(&self) -> Element<'_, Message> {
        let now = OffsetDateTime::now_utc();
        let tab = self.tab();
        let resolved = tab
            .draft
            .expires_at(now)
            .map(|at| format!("· {}", format_expiry_date(at, self.offset)));
        let head = row![
            Icon::Event.view(ICON_SIZE, Tint::Text),
            text(EXPIRES).font(theme::MEDIUM),
        ]
        .push(resolved.map(|line| text(line).style(theme::muted_text)))
        .spacing(ROW_GAP)
        .align_y(Alignment::Center);
        let picked = matches!(tab.draft.expiry, Expiry::On(_));
        let chips = row(Expiry::PRESETS.into_iter().map(|preset| {
            chip(
                preset.label(),
                tab.draft.expiry.same_chip(preset),
                Message::Expiry(preset),
            )
        }))
        .push(chip(PICK_A_DATE, picked, Message::PickDate))
        .spacing(CHIP_GAP)
        .wrap();
        let mut section = column![head, chips].spacing(SECTION_GAP);
        if picked {
            let hint = format!("{DATE_HINT}, UTC{}", offset_label(self.offset));
            section = section.push(
                column![
                    text_input(DATE_HINT, &tab.date)
                        .style(theme::input)
                        .padding(parts::FIELD_PADDING)
                        .width(DATE_WIDTH)
                        .on_input(Message::DateChanged)
                        .on_submit(Message::Submit),
                    text(hint).size(SECOND_LINE_SIZE).style(theme::muted_text),
                ]
                .spacing(ROW_GAP / 2.0),
            );
        }
        section.into()
    }
}

fn stop_icon(audience: Audience) -> Icon {
    match audience {
        Audience::People => Icon::PersonRemove,
        Audience::Link => Icon::LinkOff,
    }
}

fn avatar<'a>(name: &str, size: f32, text_size: f32) -> Element<'a, Message> {
    container(text(initial(name)).size(text_size).font(theme::BOLD))
        .center(size)
        .style(theme::avatar)
        .into()
}

/// One of the people with access: who, when they were added, what they may do.
fn person_row<'a>(
    avatar: Element<'a, Message>,
    name: String,
    added: Option<String>,
    role: Element<'a, Message>,
    remove: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let names = column![text(name).font(theme::MEDIUM)]
        .push(added.map(|added| text(added).size(SECOND_LINE_SIZE).style(theme::muted_text)));
    container(
        row![avatar, names.width(Length::Fill), role]
            .push(remove)
            .spacing(ROW_GAP + 4.0)
            .align_y(Alignment::Center),
    )
    .padding(PERSON_PADDING)
    .width(Length::Fill)
    .into()
}

/// `+02:00`, or nothing for UTC itself.
fn offset_label(offset: UtcOffset) -> String {
    if offset == UtcOffset::UTC {
        String::new()
    } else {
        let (hours, minutes, _) = offset.as_hms();
        let sign = if offset.is_negative() { '-' } else { '+' };
        format!("{sign}{:02}:{:02}", hours.abs(), minutes.abs())
    }
}

fn chip(label: &str, chosen: bool, message: Message) -> Element<'_, Message> {
    let check = chosen.then(|| Icon::Check.view(CHIP_CHECK, Tint::Accent));
    button(
        row![]
            .push(check)
            .push(text(label))
            .spacing(ROW_GAP / 2.0)
            .align_y(Alignment::Center),
    )
    .padding(CHIP_PADDING)
    .style(theme::filter_chip(chosen))
    .on_press(message)
    .into()
}

/// An on/off switch.
fn switch<'a>(on: bool, message: Message) -> Element<'a, Message> {
    let thumb = container(space())
        .width(THUMB)
        .height(THUMB)
        .style(theme::switch_thumb);
    let track = if on {
        row![space::horizontal(), thumb]
    } else {
        row![thumb, space::horizontal()]
    };
    button(track.width(Length::Fill))
        .padding(SWITCH_INSET)
        .width(SWITCH_WIDTH)
        .height(SWITCH_HEIGHT)
        .style(theme::switch(on))
        .on_press(message)
        .into()
}

#[cfg(test)]
mod tests {
    use protocol::{NodeKind, SharePermission};
    use uuid::Uuid;

    use super::*;

    fn client() -> Client {
        Client::new("http://localhost")
    }

    fn node(raw: u128) -> Node {
        let now = OffsetDateTime::now_utc();
        Node {
            id: NodeId(Uuid::from_u128(raw)),
            parent_id: None,
            kind: NodeKind::File,
            name: format!("file-{raw}"),
            size: 0,
            sha256: None,
            mime: String::new(),
            version: 1,
            created_at: now,
            updated_at: now,
            edited_on: None,
        }
    }

    fn share(items: Vec<Node>) -> Share {
        Share {
            id: Uuid::from_u128(9),
            token: Some("token".to_owned()),
            items,
            has_password: false,
            permission: SharePermission::Read,
            expires_at: None,
            created_at: OffsetDateTime::now_utc(),
            recipients: Vec::new(),
        }
    }

    fn user(raw: u128, name: &str) -> UserRef {
        UserRef {
            id: UserId(Uuid::from_u128(raw)),
            username: name.to_owned(),
        }
    }

    fn dialog(nodes: Vec<Node>) -> ShareDialog {
        ShareDialog::open(&client(), nodes, "sam".to_owned()).0
    }

    #[test]
    fn the_link_of_exactly_the_items_is_edited() {
        let (a, b) = (node(1), node(2));
        let mut dialog = dialog(vec![a.clone(), b.clone()]);
        let shares = vec![share(vec![a.clone()]), share(vec![b, a])];
        let _ = dialog.update(&client(), Message::Loaded(Ok(shares)));
        assert_eq!(dialog.audience, Audience::Link);
        assert_eq!(dialog.link.share.map(|share| share.items.len()), Some(2));
    }

    #[test]
    fn people_come_first_when_nothing_is_shared() {
        let mut dialog = dialog(vec![node(1)]);
        let _ = dialog.update(&client(), Message::Loaded(Ok(Vec::new())));
        assert_eq!(dialog.audience, Audience::People);
    }

    #[test]
    fn ticking_a_person_adds_them_and_ticking_again_takes_them_away() {
        let mut dialog = dialog(vec![node(1)]);
        let users = vec![user(1, "dana"), user(2, "daniel")];
        let _ = dialog.update(&client(), Message::UsersLoaded(Ok(users)));
        let _ = dialog.update(&client(), Message::Toggle(user(1, "dana")));
        let names: Vec<&str> = dialog
            .people
            .draft
            .recipients()
            .iter()
            .map(|user| user.username.as_str())
            .collect();
        assert_eq!(names, ["dana"]);
        let _ = dialog.update(&client(), Message::Toggle(user(1, "dana")));
        assert!(dialog.people.draft.recipients().is_empty());
    }

    #[test]
    fn the_picker_opens_as_wide_as_the_field_with_a_fresh_search() {
        let mut dialog = dialog(vec![node(1)]);
        let _ = dialog.update(&client(), Message::Query("da".to_owned()));
        let field = iced::Rectangle::new(iced::Point::ORIGIN, iced::Size::new(420.0, 36.0));
        let _ = dialog.update(
            &client(),
            Message::Pick(Placement::hanging(field, Horizontal::Left)),
        );
        assert!(dialog.query.is_empty());
        assert_eq!(dialog.picker_width, 420.0);
    }

    #[test]
    fn the_picker_keeps_open_while_searching_and_ticking() {
        assert!(Message::Query(String::new()).keeps_picker_open());
        assert!(Message::Toggle(user(1, "dana")).keeps_picker_open());
        assert!(!Message::Cancel.keeps_picker_open());
    }

    #[test]
    fn saving_people_without_anyone_is_refused() {
        let mut dialog = dialog(vec![node(1)]);
        let _ = dialog.update(&client(), Message::Loaded(Ok(Vec::new())));
        let _ = dialog.update(&client(), Message::Submit);
        assert_eq!(
            dialog.error.as_deref(),
            Some(client_core::share::NO_RECIPIENTS)
        );
        assert!(!dialog.saving);
    }

    #[test]
    fn a_date_that_does_not_read_is_not_saved() {
        let mut dialog = dialog(vec![node(1)]);
        let _ = dialog.update(&client(), Message::Loaded(Ok(Vec::new())));
        let _ = dialog.update(&client(), Message::Tab(Audience::Link));
        let _ = dialog.update(&client(), Message::PickDate);
        let _ = dialog.update(&client(), Message::DateChanged("next friday".to_owned()));
        let _ = dialog.update(&client(), Message::Submit);
        assert_eq!(dialog.error.as_deref(), Some(BAD_DATE));
        assert!(!dialog.saving);
    }

    #[test]
    fn cancelling_the_question_keeps_the_dialog() {
        let mut dialog = dialog(vec![node(1)]);
        let _ = dialog.update(&client(), Message::Loaded(Ok(vec![share(vec![node(1)])])));
        let _ = dialog.update(&client(), Message::AskStop);
        assert!(matches!(
            dialog.update(&client(), Message::Cancel),
            Outcome::Task(_)
        ));
        assert!(!dialog.confirm_stop);
    }

    fn offset(hours: i8, minutes: i8) -> UtcOffset {
        UtcOffset::from_hms(hours, minutes, 0).expect("offset")
    }

    #[test]
    fn offsets_are_labelled_with_their_sign() {
        assert_eq!(offset_label(offset(2, 0)), "+02:00");
        assert_eq!(offset_label(offset(-5, -30)), "-05:30");
        assert_eq!(offset_label(UtcOffset::UTC), "");
    }
}
