//! Saving a received transfer into a folder of the user's tree.

use api_client::{ApiError, Client};
use client_core::{
    labels::{HOME, files},
    listing::{Sort, sort_nodes},
    transfers::transfer_size,
};
use iced::widget::Row;
use iced::{
    Alignment, Element, Length, Task,
    widget::{button, column, container, operation, row, scrollable, text, text_input},
};
use protocol::{Node, NodeId, NodeKind, TransferSummary};

use super::parts::{self, ROW_SIZE, SECOND_LINE_SIZE, divider};
use crate::{
    ellipsis::ellipsized,
    icons::{ICON_SIZE, Icon, Tint},
    theme,
};

const WIDTH: f32 = 520.0;
const LIST_HEIGHT: f32 = 300.0;
const ROW_HEIGHT: f32 = 48.0;
const ROW_PADDING: [f32; 2] = [0.0, 14.0];
const ROW_GAP: f32 = 12.0;
const TRAIL_GAP: f32 = 4.0;
const TRAIL_PADDING: [f32; 2] = [2.0, 4.0];
const CHEVRON: f32 = 18.0;
const NOTE_PADDING: f32 = 16.0;
const NAME_INPUT: &str = "save-folder-name";
const SAVING: &str = "Saving…";

#[derive(Debug, Clone)]
pub enum Message {
    /// Picks a folder of the list as the destination, or unpicks it.
    Pick(Node),
    /// Opens a folder of the list.
    Enter(Node),
    /// Goes back to the folder at this depth of the trail, Home being 0.
    Crumb(usize),
    Up,
    Folders(Option<NodeId>, Result<Vec<Node>, ApiError>),
    NewFolder,
    NameChanged(String),
    CreateFolder,
    Created(Result<Node, ApiError>),
    Submit,
    Saved(Result<Vec<Node>, ApiError>),
    Cancel,
}

/// What the transfers page should do after an update.
pub enum Outcome {
    Task(Task<Message>),
    Closed,
    Saved,
}

/// The dialog picking where on the server a transfer goes.
pub struct TransferSave {
    transfer: TransferSummary,
    lost_and_found: NodeId,
    /// The folders opened so far, the last one being where the list is.
    trail: Vec<Node>,
    /// The folders of the open one, once listed.
    folders: Option<Vec<Node>>,
    /// A folder of the list picked as the destination, instead of the open one.
    picked: Option<Node>,
    /// The name of a new folder being typed.
    new_folder_name: Option<String>,
    saving: bool,
    error: Option<String>,
}

impl TransferSave {
    pub fn open(
        client: &Client,
        transfer: TransferSummary,
        lost_and_found: NodeId,
    ) -> (Self, Task<Message>) {
        let state = Self {
            transfer,
            lost_and_found,
            trail: Vec::new(),
            folders: None,
            picked: None,
            new_folder_name: None,
            saving: false,
            error: None,
        };
        let task = state.list(client);
        (state, task)
    }

    fn open_folder(&self) -> Option<NodeId> {
        self.trail.last().map(|folder| folder.id)
    }

    fn list(&self, client: &Client) -> Task<Message> {
        let client = client.clone();
        let (parent, found) = (self.open_folder(), self.lost_and_found);
        Task::perform(
            async move {
                let mut folders: Vec<Node> = client
                    .list(parent)
                    .await?
                    .into_iter()
                    .filter(|node| node.kind == NodeKind::Folder && node.id != found)
                    .collect();
                sort_nodes(&mut folders, Sort::default());
                Ok(folders)
            },
            move |result| Message::Folders(parent, result),
        )
    }

    /// Moves the list to another folder.
    fn navigate(&mut self, client: &Client, change: impl FnOnce(&mut Vec<Node>)) -> Outcome {
        change(&mut self.trail);
        self.picked = None;
        self.new_folder_name = None;
        self.folders = None;
        self.error = None;
        Outcome::Task(self.list(client))
    }

    /// Where the files go: the open folder, or the folder picked in it.
    fn destination(&self) -> Vec<&Node> {
        self.trail.iter().chain(self.picked.as_ref()).collect()
    }

    pub fn update(&mut self, client: &Client, message: Message) -> Outcome {
        match message {
            Message::Pick(folder) => {
                self.picked = match &self.picked {
                    Some(picked) if picked.id == folder.id => None,
                    _ => Some(folder),
                };
            }
            Message::Enter(folder) => return self.navigate(client, |trail| trail.push(folder)),
            Message::Crumb(depth) => return self.navigate(client, |trail| trail.truncate(depth)),
            Message::Up => {
                return self.navigate(client, |trail| {
                    trail.pop();
                });
            }
            Message::Folders(parent, result) if parent == self.open_folder() => match result {
                Ok(folders) => self.folders = Some(folders),
                Err(err) => self.error = Some(err.to_string()),
            },
            Message::Folders(..) => {}
            Message::NewFolder => {
                self.new_folder_name = Some(String::new());
                return Outcome::Task(operation::focus(NAME_INPUT));
            }
            Message::NameChanged(name) => {
                if let Some(naming) = &mut self.new_folder_name {
                    *naming = name;
                }
            }
            Message::CreateFolder => {
                let name = self
                    .new_folder_name
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                if name.is_empty() {
                    self.new_folder_name = None;
                } else {
                    let (client, parent) = (client.clone(), self.open_folder());
                    return Outcome::Task(Task::perform(
                        async move { client.create_folder(parent, &name).await },
                        Message::Created,
                    ));
                }
            }
            Message::Created(Ok(folder)) => {
                self.new_folder_name = None;
                self.picked = Some(folder);
                return Outcome::Task(self.list(client));
            }
            Message::Created(Err(err)) => self.error = Some(err.to_string()),
            Message::Submit if !self.saving && self.new_folder_name.is_none() => {
                self.saving = true;
                self.error = None;
                let target = self.destination().last().map(|folder| folder.id);
                let (client, id) = (client.clone(), self.transfer.id);
                return Outcome::Task(Task::perform(
                    async move { client.save_transfer(id, target).await },
                    Message::Saved,
                ));
            }
            Message::Submit => {}
            Message::Saved(Ok(_)) => return Outcome::Saved,
            Message::Saved(Err(err)) => {
                self.saving = false;
                self.error = Some(err.to_string());
            }
            Message::Cancel if self.new_folder_name.is_some() => self.new_folder_name = None,
            Message::Cancel => return Outcome::Closed,
        }
        Outcome::Task(Task::none())
    }

    /// The dialog panel, without the backdrop.
    pub fn view(&self) -> Element<'_, Message> {
        let title = format!(
            "Save {} on the server",
            files(self.transfer.file_count as usize)
        );
        let size = transfer_size(&self.transfer);
        let subtitle = match &self.transfer.sender_device_name {
            Some(device) => format!("From {device} · {size}"),
            None => size,
        };
        let mut lines = column![
            parts::dialog_head(title, subtitle),
            self.trail_view(),
            container(scrollable(self.rows()).height(Length::Fill))
                .padding(1)
                .height(LIST_HEIGHT)
                .width(Length::Fill)
                .style(theme::card),
            text(format!(
                "Files go to {} and sync like any other files.",
                location(&self.destination())
            ))
            .size(SECOND_LINE_SIZE)
            .style(theme::muted_text),
        ];
        if let Some(error) = &self.error {
            lines = lines.push(parts::alert_error(Icon::Error, error.as_str(), None));
        }
        let can_save = !self.saving && self.new_folder_name.is_none();
        parts::dialog_with_aside(
            WIDTH,
            lines,
            Some(parts::ghost(
                Some(Icon::NewFolder),
                "New folder",
                self.new_folder_name.is_none().then_some(Message::NewFolder),
            )),
            [
                parts::outlined(None, "Cancel", Some(Message::Cancel)),
                parts::primary(
                    Some(Icon::Check),
                    if self.saving { SAVING } else { "Save" },
                    can_save.then_some(Message::Submit),
                ),
            ],
        )
    }

    /// The way back up: an arrow, then Home and the folders opened.
    fn trail_view(&self) -> Element<'_, Message> {
        let back = button(Icon::Back.view(CHEVRON, Tint::Muted))
            .padding(TRAIL_PADDING)
            .style(theme::tool)
            .on_press_maybe((!self.trail.is_empty()).then_some(Message::Up));
        let last = self.trail.len();
        let crumbs = std::iter::once(HOME)
            .chain(self.trail.iter().map(|folder| folder.name.as_str()))
            .enumerate()
            .flat_map(|(depth, name)| {
                let crumb: Element<'_, Message> = if depth == last {
                    container(text(name).size(SECOND_LINE_SIZE + 1.0).font(theme::MEDIUM))
                        .padding(TRAIL_PADDING)
                        .into()
                } else {
                    button(text(name).size(SECOND_LINE_SIZE + 1.0))
                        .padding(TRAIL_PADDING)
                        .style(theme::crumb)
                        .on_press(Message::Crumb(depth))
                        .into()
                };
                (depth > 0)
                    .then(|| Icon::Chevron.view(CHEVRON - 2.0, Tint::Muted))
                    .into_iter()
                    .chain([crumb])
            });
        let mut row = Row::new();
        if !self.trail.is_empty() {
            row = row.push(back);
        }
        row = row.extend(crumbs);
        row = row.spacing(TRAIL_GAP);
        row = row.align_y(Alignment::Center);
        row.into()
    }

    fn rows(&self) -> Element<'_, Message> {
        let mut rows = column![];
        if let Some(name) = &self.new_folder_name {
            let input = text_input("Folder name", name)
                .id(NAME_INPUT)
                .style(theme::input)
                .padding(parts::FIELD_PADDING)
                .on_input(Message::NameChanged)
                .on_submit(Message::CreateFolder);
            rows = rows.push(
                container(
                    row![
                        Icon::NewFolder.view(ICON_SIZE, Tint::Accent),
                        input,
                        parts::small_primary(Icon::Check, "Create", Some(Message::CreateFolder)),
                        parts::small_outlined_icon(Icon::Close, "Cancel", Some(Message::Cancel)),
                    ]
                    .spacing(ROW_GAP)
                    .align_y(Alignment::Center),
                )
                .padding(ROW_PADDING)
                .center_y(ROW_HEIGHT),
            );
        }
        match self.folders.as_deref() {
            None => rows = rows.push(note("Loading…")),
            Some([]) if self.new_folder_name.is_none() => rows = rows.push(note("No folders here")),
            Some(folders) => {
                for (index, folder) in folders.iter().enumerate() {
                    if index > 0 || self.new_folder_name.is_some() {
                        rows = rows.push(divider());
                    }
                    rows = rows.push(self.folder_row(folder));
                }
            }
        }
        rows.width(Length::Fill).into()
    }

    fn folder_row<'a>(&'a self, folder: &'a Node) -> Element<'a, Message> {
        let picked = self
            .picked
            .as_ref()
            .is_some_and(|picked| picked.id == folder.id);
        let end: Element<'a, Message> = if picked {
            Icon::Check.view(CHEVRON, Tint::Accent)
        } else {
            button(Icon::Chevron.view(CHEVRON, Tint::Muted))
                .padding(TRAIL_PADDING)
                .style(theme::tool)
                .on_press(Message::Enter(folder.clone()))
                .into()
        };
        let name = ellipsized(folder.name.as_str(), ROW_SIZE, Alignment::Start);
        let name: Element<'a, Message> = if picked {
            container(name)
                .style(theme::accent)
                .width(Length::Fill)
                .into()
        } else {
            name.width(Length::Fill).into()
        };
        button(
            row![Icon::FolderOutline.view(ICON_SIZE, Tint::Accent), name, end]
                .spacing(ROW_GAP)
                .align_y(Alignment::Center)
                .height(Length::Fill),
        )
        .padding(ROW_PADDING)
        .height(ROW_HEIGHT)
        .width(Length::Fill)
        .style(theme::row(picked))
        .on_press(Message::Pick(folder.clone()))
        .into()
    }
}

fn note(hint: &str) -> Element<'_, Message> {
    container(text(hint).style(theme::muted_text))
        .padding(NOTE_PADDING)
        .into()
}

/// `Documents / Receipts`, or `Home` at the top.
fn location(path: &[&Node]) -> String {
    if path.is_empty() {
        HOME.to_owned()
    } else {
        path.iter()
            .map(|folder| folder.name.as_str())
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

#[cfg(test)]
mod tests {
    use iced_test::simulator;
    use protocol::{TransferState, UserId, UserRef};
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    fn folder(raw: u128, name: &str) -> Node {
        Node {
            id: NodeId(Uuid::from_u128(raw)),
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

    fn transfer() -> TransferSummary {
        let user = UserRef {
            id: UserId(Uuid::nil()),
            username: "dana".to_owned(),
        };
        TransferSummary {
            id: Uuid::from_u128(7),
            sender: user.clone(),
            sender_device: None,
            sender_device_name: Some("MacBook".to_owned()),
            recipient: user,
            recipient_device: Uuid::nil(),
            recipient_device_name: "Laptop".to_owned(),
            state: TransferState::Ready,
            total_bytes: 12 * 1024 * 1024,
            item_count: 3,
            file_count: 3,
            name: None,
            message: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            expires_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn client() -> Client {
        Client::new("http://localhost")
    }

    fn listed() -> TransferSave {
        let (mut state, _) = TransferSave::open(&client(), transfer(), NodeId(Uuid::from_u128(99)));
        let folders = vec![folder(1, "Documents"), folder(2, "Photos")];
        let _ = state.update(&client(), Message::Folders(None, Ok(folders)));
        state
    }

    #[test]
    fn the_dialog_says_what_it_saves_and_where() {
        let state = listed();
        let mut ui = simulator(state.view());

        assert!(ui.find("Save 3 files on the server").is_ok());
        assert!(ui.find("From MacBook · 12.0 MB").is_ok());
        assert!(ui.find("Documents").is_ok());
        assert!(
            ui.find("Files go to Home and sync like any other files.")
                .is_ok()
        );
    }

    #[test]
    fn picking_a_folder_makes_it_the_destination_until_picked_again() {
        let mut state = listed();
        let documents = folder(1, "Documents");

        let _ = state.update(&client(), Message::Pick(documents.clone()));
        assert_eq!(location(&state.destination()), "Documents");

        let _ = state.update(&client(), Message::Pick(documents));
        assert_eq!(location(&state.destination()), "Home");
    }

    #[test]
    fn opening_a_folder_lists_it_and_forgets_the_pick() {
        let mut state = listed();
        let _ = state.update(&client(), Message::Pick(folder(2, "Photos")));

        let Outcome::Task(task) = state.update(&client(), Message::Enter(folder(1, "Documents")))
        else {
            panic!("entering a folder closed the dialog");
        };

        assert!(task.units() > 0);
        assert!(state.picked.is_none());
        assert!(state.folders.is_none());
        assert_eq!(location(&state.destination()), "Documents");
    }

    #[test]
    fn a_late_listing_of_another_folder_is_ignored() {
        let mut state = listed();
        let _ = state.update(&client(), Message::Enter(folder(1, "Documents")));

        let _ = state.update(
            &client(),
            Message::Folders(None, Ok(vec![folder(3, "Old")])),
        );

        assert!(state.folders.is_none());
    }

    #[test]
    fn escape_leaves_the_new_folder_before_the_dialog() {
        let mut state = listed();
        let _ = state.update(&client(), Message::NewFolder);

        assert!(matches!(
            state.update(&client(), Message::Cancel),
            Outcome::Task(_)
        ));
        assert!(state.new_folder_name.is_none());
        assert!(matches!(
            state.update(&client(), Message::Cancel),
            Outcome::Closed
        ));
    }
}
