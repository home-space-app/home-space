use iced::{
    Alignment, Element, Length, Task,
    alignment::Horizontal,
    widget::{button, column, container, operation, row, scrollable, space, text, text_input},
};
use uuid::Uuid;

use api_client::{ApiError, Client};
use client_core::{
    labels::{THIS_DEVICE, device_state, platform, sign_out_question},
    transfers::visible_to,
};
use protocol::{
    Device, DeviceVisibility, PLATFORM_ANDROID, PLATFORM_IOS, PLATFORM_LINUX, PLATFORM_MACOS,
    PLATFORM_WEB, PLATFORM_WINDOWS,
};

use super::{
    context_menu::Placement,
    parts::{
        COLUMN_GAP, DATE_WIDTH, LIST_PADDING, MENU_WIDTH, ROW_SIZE, SECOND_LINE_SIZE, alert_error,
        divider, empty, ghost, header_bar, list_box, menu_button, muted_cell, title,
    },
    visibility::{self, visibility_icon},
};
use crate::{
    ellipsis::ellipsized,
    icons::{ICON_SIZE, Icon, Tint},
    locale::short_date,
    theme,
};

const SPACING: f32 = 12.0;
const NAME_GAP: f32 = 12.0;
const DEVICE_ROW: f32 = 60.0;
const NAME_PORTION: u16 = 3;
const STATE_PORTION: u16 = 2;
const VISIBLE_PORTION: u16 = 2;
const VISIBLE_GAP: f32 = 6.0;
const VISIBLE_ICON: f32 = 16.0;
const PILL_HEIGHT: f32 = 24.0;
const PILL_SIZE: f32 = 12.0;
const PILL_PADDING: [f32; 2] = [0.0, 8.0];
const PILL_GAP: f32 = 6.0;
/// Between the state and what the device left unfinished, in [`device_state`].
const STATE_SEPARATOR: &str = " · ";
const NAME_INPUT: &str = "device-name";
const SIGN_OUT: &str = "Sign out";

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<Device>, ApiError>),
    Refresh,
    /// Opens the menu of a device, which the browser shows.
    Menu(Uuid, Placement),
    /// Starts renaming a device.
    Rename(Uuid),
    NameChanged(String),
    Save,
    Renamed(Result<(), ApiError>),
    /// Asks before signing a device out.
    SignOut(Uuid),
    ConfirmSignOut,
    SignedOut(Result<(), ApiError>),
    /// Asks who can see a device.
    ChooseVisibility(Uuid),
    Visibility(visibility::Message),
    /// Leaves the rename, the sign-out question or the visibility dialog.
    Cancel,
}

impl Message {
    /// Whether the message comes from the people picker, which stays open for more.
    pub fn keeps_picker_open(&self) -> bool {
        matches!(self, Self::Visibility(message) if message.keeps_picker_open())
    }
}

/// A device being renamed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Renaming {
    id: Uuid,
    name: String,
    saving: bool,
}

/// The user's devices, with renaming and signing out.
#[derive(Default)]
pub struct State {
    devices: Vec<Device>,
    loading: bool,
    error: Option<String>,
    renaming: Option<Renaming>,
    /// The device waiting for the user to confirm its sign-out.
    confirming: Option<Uuid>,
    /// The dialog choosing who can see a device.
    choosing: Option<visibility::State>,
}

impl State {
    /// A device list that is still waiting for its first listing.
    pub fn open() -> Self {
        Self {
            loading: true,
            ..Self::default()
        }
    }

    pub fn load(&mut self, client: &Client) -> Task<Message> {
        self.loading = true;
        let client = client.clone();
        Task::perform(async move { client.devices().await }, Message::Loaded)
    }

    /// Whether a rename, a sign-out question or a dialog is open, which Escape leaves first.
    pub fn is_asking(&self) -> bool {
        self.renaming.is_some() || self.confirming.is_some() || self.choosing.is_some()
    }

    /// The visibility dialog, while it is open.
    pub fn dialog(&self) -> Option<Element<'_, Message>> {
        self.choosing
            .as_ref()
            .map(|choosing| choosing.view().map(Message::Visibility))
    }

    /// The people picker of the visibility dialog.
    pub fn picker(&self) -> Option<Element<'_, Message>> {
        self.choosing
            .as_ref()
            .map(|choosing| choosing.picker().map(Message::Visibility))
    }

    pub fn update(&mut self, client: &Client, message: Message) -> Task<Message> {
        match message {
            Message::Loaded(result) => {
                self.loading = false;
                match result {
                    Ok(devices) => {
                        self.devices = devices;
                        self.error = None;
                    }
                    Err(err) => self.error = Some(err.to_string()),
                }
                Task::none()
            }
            Message::Refresh => self.load(client),
            // The browser takes this one.
            Message::Menu(..) => Task::none(),
            Message::Rename(id) => {
                let Some(name) = self.device(id).map(|device| device.name.clone()) else {
                    return Task::none();
                };
                self.confirming = None;
                self.renaming = Some(Renaming {
                    id,
                    name,
                    saving: false,
                });
                Task::batch([
                    operation::focus(NAME_INPUT),
                    operation::select_all(NAME_INPUT),
                ])
            }
            Message::NameChanged(name) => {
                if let Some(renaming) = &mut self.renaming {
                    renaming.name = name;
                }
                Task::none()
            }
            Message::Save => self.save(client),
            Message::Renamed(result) => {
                self.renaming = None;
                self.done(client, result)
            }
            Message::SignOut(id) => {
                self.renaming = None;
                self.confirming = Some(id);
                Task::none()
            }
            Message::ConfirmSignOut => {
                let Some(id) = self.confirming.take() else {
                    return Task::none();
                };
                let client = client.clone();
                Task::perform(
                    async move { client.sign_out_device(id).await },
                    Message::SignedOut,
                )
            }
            Message::SignedOut(result) => self.done(client, result),
            Message::ChooseVisibility(id) => {
                let Some(device) = self.device(id).cloned() else {
                    return Task::none();
                };
                self.renaming = None;
                self.confirming = None;
                let (choosing, task) = visibility::State::open(client, device);
                self.choosing = Some(choosing);
                task.map(Message::Visibility)
            }
            Message::Visibility(message) => {
                let Some(choosing) = &mut self.choosing else {
                    return Task::none();
                };
                match choosing.update(client, message) {
                    visibility::Outcome::Task(task) => task.map(Message::Visibility),
                    visibility::Outcome::Closed => {
                        self.choosing = None;
                        Task::none()
                    }
                    visibility::Outcome::Saved => {
                        self.choosing = None;
                        self.load(client)
                    }
                }
            }
            Message::Cancel => {
                self.renaming = None;
                self.confirming = None;
                self.choosing = None;
                Task::none()
            }
        }
    }

    fn device(&self, id: Uuid) -> Option<&Device> {
        self.devices.iter().find(|device| device.id == id)
    }

    /// Sends the new name, unless it is blank or unchanged.
    fn save(&mut self, client: &Client) -> Task<Message> {
        let Some(renaming) = &mut self.renaming else {
            return Task::none();
        };
        let name = renaming.name.trim().to_owned();
        let unchanged = self
            .devices
            .iter()
            .any(|device| device.id == renaming.id && device.name == name);
        if renaming.saving || name.is_empty() {
            Task::none()
        } else if unchanged {
            self.renaming = None;
            Task::none()
        } else {
            renaming.saving = true;
            let (client, id) = (client.clone(), renaming.id);
            Task::perform(
                async move { client.rename_device(id, &name).await },
                Message::Renamed,
            )
        }
    }

    /// Shows what went wrong, if anything, and reloads the list either way.
    fn done(&mut self, client: &Client, result: Result<(), ApiError>) -> Task<Message> {
        if let Err(err) = result {
            self.error = Some(err.to_string());
        }
        self.load(client)
    }

    /// Whether `id` can be signed out from here. The current device signs out with the account
    /// menu; one that is signed out and left nothing behind has nothing to end.
    pub fn can_sign_out(&self, id: Uuid) -> bool {
        self.device(id)
            .is_some_and(|device| !device.current && (device.signed_in || device.uploads > 0))
    }

    /// What the page's top line offers.
    pub fn action(&self) -> Element<'_, Message> {
        row![
            text(if self.loading { "Loading…" } else { "" }).style(theme::muted_text),
            ghost(Some(Icon::Refresh), "Refresh", Some(Message::Refresh))
        ]
        .spacing(SPACING)
        .align_y(Alignment::Center)
        .into()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let mut page = column![].spacing(SPACING);
        if let Some(error) = &self.error {
            page = page.push(alert_error(Icon::Error, error.as_str(), None));
        }
        page.push(list_box(column![header(), self.rows()])).into()
    }

    fn rows(&self) -> Element<'_, Message> {
        if self.devices.is_empty() {
            return empty(if self.loading {
                ""
            } else {
                "No devices have signed in yet."
            });
        }
        let rows = self
            .devices
            .iter()
            .flat_map(|device| [self.device_row(device), divider()]);
        scrollable(column(rows)).height(Length::Fill).into()
    }

    fn device_row<'a>(&'a self, device: &'a Device) -> Element<'a, Message> {
        let cells = match (&self.renaming, self.confirming) {
            (Some(renaming), _) if renaming.id == device.id => rename_cells(renaming),
            (_, Some(id)) if id == device.id => confirm_cells(device),
            _ => device_cells(device),
        };
        container(cells.spacing(COLUMN_GAP).align_y(Alignment::Center))
            .padding(LIST_PADDING)
            .center_y(DEVICE_ROW)
            .width(Length::Fill)
            .into()
    }
}

fn header<'a>() -> Element<'a, Message> {
    header_bar(
        row![
            title("Name", Length::FillPortion(NAME_PORTION), Alignment::Start),
            title(
                "State",
                Length::FillPortion(STATE_PORTION),
                Alignment::Start
            ),
            title(
                "Visible to",
                Length::FillPortion(VISIBLE_PORTION),
                Alignment::Start
            ),
            title("Last seen", DATE_WIDTH, Alignment::Start),
            space().width(MENU_WIDTH),
        ],
        LIST_PADDING,
    )
}

/// The icon of a device of `platform`.
pub(super) fn platform_icon(platform: &str) -> Icon {
    match platform {
        PLATFORM_LINUX | PLATFORM_MACOS | PLATFORM_WINDOWS => Icon::Laptop,
        PLATFORM_WEB => Icon::Language,
        PLATFORM_ANDROID | PLATFORM_IOS => Icon::Smartphone,
        _ => Icon::Devices,
    }
}

fn device_cells(device: &Device) -> iced::widget::Row<'_, Message> {
    let kind = if device.current {
        format!("{} · {THIS_DEVICE}", platform(&device.platform))
    } else {
        platform(&device.platform).to_owned()
    };
    let active = device.online || device.signed_in;
    let (tint, name_style): (Tint, fn(&iced::Theme) -> container::Style) = if active {
        (Tint::Text, theme::plain)
    } else {
        (Tint::Muted, theme::muted)
    };
    let name = row![
        platform_icon(&device.platform).view(ICON_SIZE, tint),
        column![
            container(ellipsized(device.name.as_str(), ROW_SIZE, Alignment::Start))
                .style(name_style),
            container(text(kind).size(SECOND_LINE_SIZE)).style(theme::muted),
        ]
        .width(Length::Fill)
    ]
    .spacing(NAME_GAP)
    .align_y(Alignment::Center)
    .width(Length::FillPortion(NAME_PORTION));
    let id = device.id;
    row![
        name,
        state_cell(device, active),
        visible_cell(device),
        muted_cell(
            short_date(device.last_seen_at),
            DATE_WIDTH,
            Alignment::Start
        ),
        menu_button(Message::Cancel, move |bounds| {
            Message::Menu(id, Placement::hanging(bounds, Horizontal::Right))
        }),
    ]
}

/// The state in a pill, with what the device left unfinished next to it.
fn state_cell<'a>(device: &Device, active: bool) -> Element<'a, Message> {
    let label = device_state(device);
    let (state, note) = match label.split_once(STATE_SEPARATOR) {
        Some((state, note)) => (state.to_owned(), Some(note.to_owned())),
        None => (label, None),
    };
    let pill = container(text(state).size(PILL_SIZE).font(theme::SEMIBOLD))
        .padding(PILL_PADDING)
        .center_y(PILL_HEIGHT)
        .style(theme::pill(active));
    row![pill]
        .push(note.map(|note| text(note).size(SECOND_LINE_SIZE).style(theme::muted_text)))
        .spacing(PILL_GAP)
        .align_y(Alignment::Center)
        .width(Length::FillPortion(STATE_PORTION))
        .into()
}

/// Who can send to the device, after the icon of its visibility.
fn visible_cell<'a>(device: &Device) -> Element<'a, Message> {
    let hidden = device.visibility == DeviceVisibility::Hidden;
    row![
        visibility_icon(device.visibility).view(VISIBLE_ICON, Tint::Muted),
        container(ellipsized(visible_to(device), ROW_SIZE, Alignment::Start))
            .style(if hidden { theme::muted } else { theme::plain })
            .width(Length::Fill),
    ]
    .spacing(VISIBLE_GAP)
    .align_y(Alignment::Center)
    .width(Length::FillPortion(VISIBLE_PORTION))
    .into()
}

fn rename_cells(renaming: &Renaming) -> iced::widget::Row<'_, Message> {
    let can_save = !renaming.saving && !renaming.name.trim().is_empty();
    row![
        Icon::Rename.view(ICON_SIZE, Tint::Muted),
        text_input("Device name", &renaming.name)
            .style(theme::input)
            .id(NAME_INPUT)
            .on_input_maybe((!renaming.saving).then_some(Message::NameChanged))
            .on_submit(Message::Save)
            .width(Length::Fill),
        button(text("Save"))
            .style(theme::primary)
            .on_press_maybe(can_save.then_some(Message::Save)),
        button(text("Cancel"))
            .style(theme::secondary)
            .on_press(Message::Cancel),
    ]
}

fn confirm_cells(device: &Device) -> iced::widget::Row<'_, Message> {
    row![
        Icon::SignOut.view(ICON_SIZE, Tint::Danger),
        text(sign_out_question(device)).width(Length::Fill),
        button(text("Cancel"))
            .style(theme::secondary)
            .on_press(Message::Cancel),
        button(text(SIGN_OUT))
            .style(theme::danger)
            .on_press(Message::ConfirmSignOut),
    ]
}

#[cfg(test)]
mod tests {
    use iced_test::simulator;
    use time::OffsetDateTime;

    use super::*;

    const LAPTOP: u128 = 1;
    const BROWSER: u128 = 2;

    fn device(raw: u128, name: &str, platform: &str, current: bool) -> Device {
        Device {
            id: Uuid::from_u128(raw),
            name: name.to_owned(),
            platform: platform.to_owned(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            last_seen_at: OffsetDateTime::UNIX_EPOCH,
            signed_in: true,
            online: false,
            current,
            uploads: 0,
            visibility: Default::default(),
            visible_to: Vec::new(),
        }
    }

    fn client() -> Client {
        Client::new("http://localhost")
    }

    fn loaded() -> State {
        let mut state = State::open();
        let devices = vec![
            device(LAPTOP, "Laptop", PLATFORM_LINUX, true),
            Device {
                uploads: 2,
                ..device(BROWSER, "Firefox", PLATFORM_WEB, false)
            },
        ];
        let _ = state.update(&client(), Message::Loaded(Ok(devices)));
        state
    }

    #[test]
    fn the_list_names_each_device_and_marks_this_one() {
        let state = loaded();
        let mut ui = simulator(state.view());

        assert!(ui.find("Laptop").is_ok());
        assert!(ui.find("Linux · This device").is_ok());
        assert!(ui.find("Web browser").is_ok());
        assert!(ui.find("Signed in").is_ok());
        assert!(ui.find("2 unfinished uploads").is_ok());
    }

    #[test]
    fn this_device_cannot_be_signed_out_from_the_list() {
        let state = loaded();

        assert!(!state.can_sign_out(Uuid::from_u128(LAPTOP)));
        assert!(state.can_sign_out(Uuid::from_u128(BROWSER)));
    }

    #[test]
    fn a_signed_out_device_with_nothing_left_has_nothing_to_end() {
        let state = State {
            devices: vec![Device {
                signed_in: false,
                ..device(BROWSER, "Firefox", PLATFORM_WEB, false)
            }],
            ..State::default()
        };

        assert!(!state.can_sign_out(Uuid::from_u128(BROWSER)));
    }

    #[test]
    fn each_platform_has_its_icon() {
        assert_eq!(platform_icon(PLATFORM_LINUX), Icon::Laptop);
        assert_eq!(platform_icon(PLATFORM_WEB), Icon::Language);
        assert_eq!(platform_icon(PLATFORM_ANDROID), Icon::Smartphone);
        assert_eq!(platform_icon("toaster"), Icon::Devices);
    }

    #[test]
    fn signing_out_asks_first_and_names_what_it_cancels() {
        let mut state = loaded();

        let _ = state.update(&client(), Message::SignOut(Uuid::from_u128(BROWSER)));
        let mut ui = simulator(state.view());

        assert!(
            ui.find("Sign out “Firefox” and cancel its 2 unfinished uploads?")
                .is_ok()
        );
    }

    #[test]
    fn confirming_sends_the_sign_out_and_closes_the_question() {
        let mut state = loaded();
        let _ = state.update(&client(), Message::SignOut(Uuid::from_u128(BROWSER)));

        let task = state.update(&client(), Message::ConfirmSignOut);

        assert!(task.units() > 0);
        assert!(!state.is_asking());
    }

    #[test]
    fn renaming_starts_from_the_current_name() {
        let mut state = loaded();

        let _ = state.update(&client(), Message::Rename(Uuid::from_u128(LAPTOP)));

        assert_eq!(
            state
                .renaming
                .as_ref()
                .map(|renaming| renaming.name.as_str()),
            Some("Laptop")
        );
    }

    #[test]
    fn a_blank_name_is_not_saved() {
        let mut state = loaded();
        let _ = state.update(&client(), Message::Rename(Uuid::from_u128(LAPTOP)));
        let _ = state.update(&client(), Message::NameChanged("  ".to_owned()));

        let task = state.update(&client(), Message::Save);

        assert_eq!(task.units(), 0);
        assert!(state.renaming.as_ref().is_some_and(|r| !r.saving));
    }

    #[test]
    fn a_new_name_is_sent() {
        let mut state = loaded();
        let _ = state.update(&client(), Message::Rename(Uuid::from_u128(LAPTOP)));
        let _ = state.update(&client(), Message::NameChanged(" Work laptop ".to_owned()));

        let task = state.update(&client(), Message::Save);

        assert!(task.units() > 0);
        assert!(state.renaming.as_ref().is_some_and(|r| r.saving));
    }

    #[test]
    fn a_failed_sign_out_is_shown() {
        let mut state = loaded();

        let _ = state.update(&client(), Message::SignedOut(Err(ApiError::Unauthorized)));

        assert!(state.error.is_some());
    }
}
