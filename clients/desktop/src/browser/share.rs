//! Sharing files and folders through a read-only link.

use iced::{
    Alignment, Element, Length, Task,
    widget::{button, column, container, row, space, text, text_input},
};
use time::{Duration, OffsetDateTime, UtcOffset};

use api_client::{ApiError, Client};
use client_core::share::{
    BAD_DATE, CHANGE_PASSWORD, COPY_LINK, DATE_HINT, EXPIRES, Expiry, HIDDEN_PASSWORD, LINK_COPIED,
    PASSWORD_PLACEHOLDER, PICK_A_DATE, REQUIRE_PASSWORD, STOP_SHARING, STOP_SHARING_NOTE,
    ShareDraft, format_expiry_date, matching_share, parse_expiry_date, share_subtitle, share_title,
    stop_sharing_question,
};
use protocol::{Node, NodeId, Share, UpdateShare};

use super::parts::{self, SECOND_LINE_SIZE};
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
const CHIP_CHECK: f32 = 16.0;
const SWITCH_WIDTH: f32 = 36.0;
const SWITCH_HEIGHT: f32 = 20.0;
const SWITCH_INSET: f32 = 2.0;
const THUMB: f32 = 16.0;
const DATE_WIDTH: f32 = 200.0;
const PASSWORD_INPUT: &str = "share-password";
const NO_LINK_YET: &str = "Save to make the link";
const SAVING: &str = "Saving…";
const CANCEL: &str = "Cancel";
const CLOSE: &str = "Close";
const SAVE: &str = "Save";
/// How far ahead the date field starts when the owner first picks a date.
const PICKED_DATE_AHEAD: Duration = Duration::days(7);

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<Share>, ApiError>),
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

/// What the browser should do after an update.
pub enum Outcome {
    Task(Task<Message>),
    Closed,
}

/// The dialog that makes a link to some files and folders, or edits the one that shares them.
pub struct ShareDialog {
    nodes: Vec<Node>,
    share: Option<Share>,
    draft: ShareDraft,
    loading: bool,
    saving: bool,
    error: Option<String>,
    copied: bool,
    reveal: bool,
    confirm_stop: bool,
    /// The picked date as typed, which may not read as a date yet.
    date: String,
    offset: UtcOffset,
}

impl ShareDialog {
    pub fn open(client: &Client, nodes: Vec<Node>) -> (Self, Task<Message>) {
        let first = nodes.first().map(|node| node.id);
        let dialog = Self {
            nodes,
            share: None,
            draft: ShareDraft::new(),
            loading: true,
            saving: false,
            error: None,
            copied: false,
            reveal: false,
            confirm_stop: false,
            date: String::new(),
            offset: local_offset(),
        };
        let client = client.clone();
        let task = Task::perform(async move { client.shares(first).await }, Message::Loaded);
        (dialog, task)
    }

    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.iter().map(|node| node.id).collect()
    }

    /// Takes `share` as the saved link the dialog edits.
    fn show(&mut self, share: Share) {
        self.draft = ShareDraft::of(&share);
        self.date = self.date_text();
        self.share = Some(share);
    }

    fn date_text(&self) -> String {
        match self.draft.expiry {
            Expiry::On(at) => format_expiry_date(at, self.offset),
            _ => String::new(),
        }
    }

    fn date_is_bad(&self) -> bool {
        matches!(self.draft.expiry, Expiry::On(_))
            && parse_expiry_date(&self.date, self.offset).is_none()
    }

    pub fn update(&mut self, client: &Client, message: Message) -> Outcome {
        let now = OffsetDateTime::now_utc();
        match message {
            Message::Loaded(Ok(shares)) => {
                self.loading = false;
                if let Some(found) = matching_share(&shares, &self.node_ids()).cloned() {
                    self.show(found);
                }
            }
            Message::Loaded(Err(err)) => {
                self.loading = false;
                self.error = Some(err.to_string());
            }
            Message::TogglePassword => {
                let on = !self.draft.require_password;
                self.draft.set_require_password(on);
                if self.draft.editing_password() {
                    return Outcome::Task(iced::widget::operation::focus(PASSWORD_INPUT));
                }
            }
            Message::PasswordChanged(password) => self.draft.password = password,
            Message::ChangePassword => {
                self.draft.change_password();
                return Outcome::Task(iced::widget::operation::focus(PASSWORD_INPUT));
            }
            Message::Reveal => self.reveal = !self.reveal,
            Message::Expiry(expiry) => self.draft.expiry = expiry,
            Message::PickDate => {
                if !matches!(self.draft.expiry, Expiry::On(_)) {
                    let at = self
                        .draft
                        .expires_at(now)
                        .unwrap_or(now + PICKED_DATE_AHEAD);
                    self.draft.expiry = Expiry::On(at);
                    self.date = self.date_text();
                }
            }
            Message::DateChanged(date) => {
                if let Some(at) = parse_expiry_date(&date, self.offset) {
                    self.draft.expiry = Expiry::On(at);
                }
                self.date = date;
            }
            Message::Copy => {
                if let Some(share) = &self.share {
                    self.copied = true;
                    return Outcome::Task(iced::clipboard::write(client.share_url(&share.token)));
                }
            }
            Message::Submit if !self.saving && !self.loading => return self.save(client, now),
            Message::Submit => {}
            Message::Created(Ok(share)) => {
                self.saving = false;
                self.show(share);
            }
            Message::Updated(Ok(_)) | Message::Stopped(Ok(())) => return Outcome::Closed,
            Message::Created(Err(err)) | Message::Updated(Err(err)) => {
                self.saving = false;
                self.error = Some(err.to_string());
            }
            Message::AskStop => self.confirm_stop = true,
            Message::KeepSharing => self.confirm_stop = false,
            Message::Stop => {
                if let Some(share) = &self.share {
                    self.saving = true;
                    let client = client.clone();
                    let id = share.id;
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
        let task = match &self.share {
            Some(share) => match self.draft.update_request(now) {
                Ok(request) if request == UpdateShare::default() => return Outcome::Closed,
                Ok(request) => {
                    let id = share.id;
                    Task::perform(
                        async move { client.update_share(id, &request).await },
                        Message::Updated,
                    )
                }
                Err(problem) => {
                    self.error = Some(problem.message().to_owned());
                    return Outcome::Task(Task::none());
                }
            },
            None => match self.draft.create_request(self.node_ids(), now) {
                Ok(request) => Task::perform(
                    async move { client.create_share(&request).await },
                    Message::Created,
                ),
                Err(problem) => {
                    self.error = Some(problem.message().to_owned());
                    return Outcome::Task(Task::none());
                }
            },
        };
        self.saving = true;
        self.error = None;
        Outcome::Task(task)
    }

    pub fn view<'a>(&'a self, client: &Client) -> Element<'a, Message> {
        if self.confirm_stop {
            return self.confirm_view();
        }
        let link = self
            .share
            .as_ref()
            .map(|share| client.share_url(&share.token));
        let mut lines = column![
            parts::dialog_head(share_title(&self.nodes), share_subtitle(&self.nodes)),
            self.link_row(link),
            self.password_section(),
            self.expiry_section(),
        ];
        if let Some(error) = &self.error {
            lines = lines.push(parts::alert_error(Icon::Error, error.as_str(), None));
        }
        let aside = self.share.as_ref().map(|_| {
            parts::ghost_danger(
                Icon::LinkOff,
                STOP_SHARING,
                (!self.saving).then_some(Message::AskStop),
            )
        });
        let close = if self.share.is_some() { CLOSE } else { CANCEL };
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
                STOP_SHARING_NOTE
            )],
            [
                parts::outlined(None, CANCEL, Some(Message::KeepSharing)),
                parts::danger(STOP_SHARING, (!self.saving).then_some(Message::Stop)),
            ],
        )
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
                self.share.as_ref().map(|_| Message::Copy)
            ),
        ]
        .spacing(ROW_GAP)
        .align_y(Alignment::Center)
        .into()
    }

    fn password_section(&self) -> Element<'_, Message> {
        let head = row![
            Icon::Lock.view(ICON_SIZE, Tint::Text),
            text(REQUIRE_PASSWORD).font(theme::MEDIUM),
            space::horizontal(),
            switch(self.draft.require_password, Message::TogglePassword),
        ]
        .spacing(ROW_GAP)
        .align_y(Alignment::Center);
        let mut section = column![head].spacing(SECTION_GAP);
        if self.draft.editing_password() {
            let reveal_icon = if self.reveal {
                Icon::VisibilityOff
            } else {
                Icon::Visibility
            };
            section = section.push(
                row![
                    text_input(PASSWORD_PLACEHOLDER, &self.draft.password)
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
        } else if self.draft.offers_change_password() {
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
        let resolved = self
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
        let picked = matches!(self.draft.expiry, Expiry::On(_));
        let chips = row(Expiry::PRESETS.into_iter().map(|preset| {
            chip(
                preset.label(),
                self.draft.expiry.same_chip(preset),
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
                    text_input(DATE_HINT, &self.date)
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

fn chip<'a>(label: &'a str, chosen: bool, message: Message) -> Element<'a, Message> {
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
            token: "token".to_owned(),
            items,
            has_password: false,
            permission: SharePermission::Read,
            expires_at: None,
            created_at: OffsetDateTime::now_utc(),
        }
    }

    fn dialog(nodes: Vec<Node>) -> ShareDialog {
        ShareDialog::open(&client(), nodes).0
    }

    #[test]
    fn the_link_of_exactly_the_items_is_edited() {
        let (a, b) = (node(1), node(2));
        let mut dialog = dialog(vec![a.clone(), b.clone()]);
        let shares = vec![share(vec![a.clone()]), share(vec![b, a])];
        let _ = dialog.update(&client(), Message::Loaded(Ok(shares)));
        assert_eq!(dialog.share.map(|share| share.items.len()), Some(2));
    }

    #[test]
    fn a_date_that_does_not_read_is_not_saved() {
        let mut dialog = dialog(vec![node(1)]);
        let _ = dialog.update(&client(), Message::Loaded(Ok(Vec::new())));
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
