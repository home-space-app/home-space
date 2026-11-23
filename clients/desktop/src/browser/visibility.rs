//! Choosing who can see one of the user's devices and send files to it.

use iced::{
    Alignment, Element, Length, Task,
    alignment::Horizontal,
    widget::{button, column, container, operation, row, space, text},
};

use api_client::{ApiError, Client};
use client_core::{
    labels::platform,
    transfers::{VISIBILITIES, visibility_hint, visibility_title},
};
use protocol::{Device, DeviceVisibility, SetDeviceVisibility, UserRef};

use super::{
    anchor::Anchor,
    context_menu::Placement,
    parts::{self, SECOND_LINE_SIZE, divider},
    people_picker::{self, Picker},
};
use crate::{
    icons::{Icon, Tint},
    theme,
};

const WIDTH: f32 = 520.0;
const CHOICE_PADDING: [f32; 2] = [12.0, 16.0];
const CHOICE_GAP: f32 = 12.0;
const CHIPS_GAP: f32 = 8.0;
const RADIO: f32 = 18.0;
const CHIP_PADDING: [f32; 2] = [2.0, 8.0];
const CHIP_GAP: f32 = 6.0;
const AVATAR: f32 = 18.0;
const AVATAR_TEXT: f32 = 11.0;
const REMOVE_ICON: f32 = 14.0;
const SEARCH_INPUT: &str = "visibility-search";
const SAVING: &str = "Saving…";

#[derive(Debug, Clone)]
pub enum Message {
    Choose(DeviceVisibility),
    UsersLoaded(Result<Vec<UserRef>, ApiError>),
    /// Opens the people picker hanging from the button that adds users; the browser shows it.
    Pick(Placement),
    /// The field the people picker hangs from has moved; the browser moves the picker with it.
    Moved(Placement),
    Query(String),
    /// Adds a user to those who can see the device, or takes them away.
    Toggle(UserRef),
    Save,
    Saved(Result<(), ApiError>),
    Cancel,
}

impl Message {
    /// Whether the message comes from the people picker, which stays open for more.
    pub fn keeps_picker_open(&self) -> bool {
        matches!(self, Self::Query(_) | Self::Toggle(_) | Self::Moved(_))
    }
}

/// What the devices page should do after an update.
pub enum Outcome {
    Task(Task<Message>),
    Closed,
    Saved,
}

/// The dialog choosing who can see `device`.
pub struct State {
    device: Device,
    choice: DeviceVisibility,
    picked: Vec<UserRef>,
    /// The other users of the server, once listed.
    users: Vec<UserRef>,
    query: String,
    saving: bool,
    error: Option<String>,
}

impl State {
    pub fn open(client: &Client, device: Device) -> (Self, Task<Message>) {
        let state = Self {
            choice: device.visibility,
            picked: device.visible_to.clone(),
            device,
            users: Vec::new(),
            query: String::new(),
            saving: false,
            error: None,
        };
        let client = client.clone();
        let task = Task::perform(async move { client.users().await }, Message::UsersLoaded);
        (state, task)
    }

    pub fn update(&mut self, client: &Client, message: Message) -> Outcome {
        match message {
            Message::Choose(choice) => self.choice = choice,
            Message::UsersLoaded(Ok(users)) => self.users = users,
            Message::UsersLoaded(Err(err)) => self.error = Some(err.to_string()),
            Message::Pick(_) => {
                self.query.clear();
                return Outcome::Task(operation::focus(SEARCH_INPUT));
            }
            Message::Moved(_) => {}
            Message::Query(query) => self.query = query,
            Message::Toggle(user) => {
                match self.picked.iter().position(|picked| picked.id == user.id) {
                    Some(at) => {
                        self.picked.remove(at);
                    }
                    None => self.picked.push(user),
                }
            }
            Message::Save if self.can_save() => {
                self.saving = true;
                self.error = None;
                let request = SetDeviceVisibility {
                    visibility: self.choice,
                    users: match self.choice {
                        DeviceVisibility::Users => self.picked.iter().map(|user| user.id).collect(),
                        _ => Vec::new(),
                    },
                };
                let (client, id) = (client.clone(), self.device.id);
                return Outcome::Task(Task::perform(
                    async move { client.set_device_visibility(id, &request).await },
                    Message::Saved,
                ));
            }
            Message::Save => {}
            Message::Saved(Ok(())) => return Outcome::Saved,
            Message::Saved(Err(err)) => {
                self.saving = false;
                self.error = Some(err.to_string());
            }
            Message::Cancel => return Outcome::Closed,
        }
        Outcome::Task(Task::none())
    }

    /// Visible to specific users with nobody picked would be the same as only you.
    fn can_save(&self) -> bool {
        !self.saving && (self.choice != DeviceVisibility::Users || !self.picked.is_empty())
    }

    /// The dialog panel, without the backdrop.
    pub fn view(&self) -> Element<'_, Message> {
        let choices = VISIBILITIES
            .into_iter()
            .enumerate()
            .flat_map(|(index, visibility)| {
                (index > 0)
                    .then(divider)
                    .into_iter()
                    .chain([self.choice_row(visibility)])
            });
        let mut lines = column![
            parts::dialog_head(
                format!("Who can see {}?", self.device.name),
                format!(
                    "{} · Only affects this device",
                    platform(&self.device.platform)
                ),
            ),
            container(column(choices))
                .padding(1)
                .width(Length::Fill)
                .style(theme::card),
            text(
                "New devices start visible only to you. Your other devices keep their own setting."
            )
            .size(SECOND_LINE_SIZE)
            .style(theme::muted_text),
        ];
        if let Some(error) = &self.error {
            lines = lines.push(parts::alert_error(Icon::Error, error.as_str(), None));
        }
        parts::dialog(
            WIDTH,
            lines,
            [
                parts::outlined(None, "Cancel", Some(Message::Cancel)),
                parts::primary(
                    Some(Icon::Check),
                    if self.saving { SAVING } else { "Save" },
                    self.can_save().then_some(Message::Save),
                ),
            ],
        )
    }

    fn choice_row(&self, visibility: DeviceVisibility) -> Element<'_, Message> {
        let selected = self.choice == visibility;
        let mark = container(space())
            .center(RADIO)
            .style(theme::radio(selected));
        let mut copy = column![
            text(visibility_title(visibility)).font(theme::MEDIUM),
            text(visibility_hint(visibility))
                .size(SECOND_LINE_SIZE)
                .style(theme::muted_text),
        ]
        .width(Length::Fill);
        if selected && visibility == DeviceVisibility::Users {
            copy = copy.push(space().height(CHIPS_GAP)).push(self.chips());
        }
        button(
            row![mark, copy]
                .spacing(CHOICE_GAP)
                .align_y(Alignment::Start),
        )
        .padding(CHOICE_PADDING)
        .width(Length::Fill)
        .style(theme::choice(selected))
        .on_press(Message::Choose(visibility))
        .into()
    }

    /// The users picked so far, each removable, and the button that picks more.
    fn chips(&self) -> Element<'_, Message> {
        let add = button(
            row![
                Icon::PersonAdd.view(REMOVE_ICON, Tint::Accent),
                text("Add user")
                    .size(SECOND_LINE_SIZE)
                    .style(theme::accent_text),
            ]
            .spacing(CHIP_GAP)
            .align_y(Alignment::Center),
        )
        .padding(CHIP_PADDING)
        .style(theme::tool)
        // Never sent: the anchor takes the press, and only needs the button to look enabled.
        .on_press(Message::Cancel);
        let add = Anchor::new(add)
            .on_press(|bounds| Message::Pick(Placement::hanging(bounds, Horizontal::Left)))
            .on_move(|bounds| Message::Moved(Placement::hanging(bounds, Horizontal::Left)));
        row(self.picked.iter().map(chip))
            .push(add)
            .spacing(CHIPS_GAP)
            .align_y(Alignment::Center)
            .wrap()
            .into()
    }

    /// The people picker: a search box over the users, each with a checkbox.
    pub fn picker(&self) -> Element<'_, Message> {
        Picker {
            users: &self.users,
            query: &self.query,
            search_id: SEARCH_INPUT,
            width: people_picker::MIN_WIDTH,
            on_query: Message::Query,
            on_toggle: Message::Toggle,
        }
        .view(|id| self.picked.iter().any(|picked| picked.id == id))
    }
}

/// A picked user, with the button that takes them away.
fn chip(user: &UserRef) -> Element<'_, Message> {
    let remove = button(Icon::Close.view(REMOVE_ICON, Tint::Muted))
        .padding(0)
        .style(theme::tool)
        .on_press(Message::Toggle(user.clone()));
    container(
        row![
            avatar(&user.username),
            text(user.username.as_str()).size(SECOND_LINE_SIZE + 1.0),
            remove
        ]
        .spacing(CHIP_GAP)
        .align_y(Alignment::Center),
    )
    .padding(CHIP_PADDING)
    .style(theme::chip)
    .into()
}

fn avatar<'a>(name: &str) -> Element<'a, Message> {
    let initial: String = name
        .chars()
        .next()
        .map(|first| first.to_uppercase().collect())
        .unwrap_or_default();
    container(text(initial).size(AVATAR_TEXT).font(theme::BOLD))
        .center(AVATAR)
        .style(theme::avatar)
        .into()
}

/// The icon standing for a visibility in the list of devices.
pub fn visibility_icon(visibility: DeviceVisibility) -> Icon {
    match visibility {
        DeviceVisibility::Owner => Icon::Lock,
        DeviceVisibility::Hidden => Icon::VisibilityOff,
        DeviceVisibility::Everyone => Icon::Public,
        DeviceVisibility::Users => Icon::Group,
    }
}

#[cfg(test)]
mod tests {
    use iced_test::simulator;
    use protocol::{PLATFORM_ANDROID, UserId};
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    fn user(raw: u128, name: &str) -> UserRef {
        UserRef {
            id: UserId(Uuid::from_u128(raw)),
            username: name.to_owned(),
        }
    }

    fn device() -> Device {
        Device {
            id: Uuid::from_u128(1),
            name: "Pixel 9".to_owned(),
            platform: PLATFORM_ANDROID.to_owned(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            last_seen_at: OffsetDateTime::UNIX_EPOCH,
            signed_in: true,
            online: false,
            current: false,
            uploads: 0,
            visibility: DeviceVisibility::Owner,
            visible_to: Vec::new(),
            version: None,
            compatibility: None,
        }
    }

    fn client() -> Client {
        Client::new("http://localhost")
    }

    fn opened() -> State {
        let (mut state, _) = State::open(&client(), device());
        let users = vec![user(2, "dana"), user(3, "Alex")];
        let _ = state.update(&client(), Message::UsersLoaded(Ok(users)));
        state
    }

    #[test]
    fn the_dialog_names_the_device_and_offers_every_visibility() {
        let state = opened();
        let mut ui = simulator(state.view());

        assert!(ui.find("Who can see Pixel 9?").is_ok());
        assert!(ui.find("Android · Only affects this device").is_ok());
        for visibility in VISIBILITIES {
            assert!(ui.find(visibility_title(visibility)).is_ok());
        }
    }

    #[test]
    fn specific_users_need_somebody_picked_before_saving() {
        let mut state = opened();
        let _ = state.update(&client(), Message::Choose(DeviceVisibility::Users));
        assert!(!state.can_save());

        let _ = state.update(&client(), Message::Toggle(user(2, "dana")));
        assert!(state.can_save());

        let _ = state.update(&client(), Message::Toggle(user(2, "dana")));
        assert!(!state.can_save());
    }

    #[test]
    fn saving_sends_the_choice_and_waits_for_the_answer() {
        let mut state = opened();
        let _ = state.update(&client(), Message::Choose(DeviceVisibility::Everyone));

        let Outcome::Task(task) = state.update(&client(), Message::Save) else {
            panic!("saving closed the dialog");
        };

        assert!(task.units() > 0);
        assert!(state.saving);
        assert!(matches!(
            state.update(&client(), Message::Saved(Ok(()))),
            Outcome::Saved
        ));
    }

    #[test]
    fn the_picked_users_show_as_chips() {
        let mut state = opened();
        let _ = state.update(&client(), Message::Choose(DeviceVisibility::Users));
        let _ = state.update(&client(), Message::Toggle(user(2, "dana")));
        let mut ui = simulator(state.view());

        assert!(ui.find("dana").is_ok());
        assert!(ui.find("Add user").is_ok());
    }
}
