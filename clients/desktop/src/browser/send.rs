//! Sending files to one of the user's devices, or to another user's device.

use std::{collections::BTreeSet, path::PathBuf};

use iced::{
    Alignment, Element, Length, Task,
    widget::{button, column, container, row, scrollable, text},
};

use api_client::{ApiError, Client};
use client_core::{
    format::human_size,
    labels::files,
    send::{
        LocalEntry, device_status, first_choice, free_name, items_summary, manifest, names_line,
        pickable, recipient_name, send_note, send_title, top_name, with_top,
    },
};
use protocol::{
    CreateTransfer, CreatedTransfer, NewTransferItem, Node, NodeKind, TargetDevice, TransferTarget,
    UserId,
};
use uuid::Uuid;

use super::{
    devices::platform_icon,
    parts::{self, ROW_SIZE, SECOND_LINE_SIZE, divider},
};
use crate::{
    ellipsis::ellipsized,
    icons::{ICON_SIZE, Icon, Tint, node_icon},
    theme,
    transfers::{self, SendFile, SendPlan},
};

const WIDTH: f32 = 520.0;
const LIST_HEIGHT: f32 = 240.0;
const ROW_HEIGHT: f32 = 48.0;
const ROW_PADDING: [f32; 2] = [0.0, 14.0];
const ROW_GAP: f32 = 12.0;
const SECTION_GAP: f32 = 6.0;
const ADD_GAP: f32 = 4.0;
const CHEVRON: f32 = 18.0;
const NOTE_PADDING: f32 = 16.0;
const SENDING: &str = "Sending…";
const NO_DEVICES: &str = "None of your other devices can receive files. Sign in on one, or check its visibility in Devices.";

/// What the dialog sends.
#[derive(Debug, Clone)]
pub enum Source {
    /// Files and folders already on the server.
    Nodes(Vec<Node>),
    /// Files and folders picked on this computer.
    Local,
}

/// A file or folder picked on this computer, by the name it has at the top of the transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    name: String,
    folder: bool,
    plan: SendPlan,
}

impl Picked {
    fn bytes(&self) -> u64 {
        self.plan.files.iter().map(|file| file.size).sum()
    }

    /// Sends it under `name` instead, with everything inside it.
    fn rename(&mut self, name: String) {
        for folder in &mut self.plan.folders {
            *folder = with_top(folder, &name);
        }
        for file in &mut self.plan.files {
            file.path = with_top(&file.path, &name);
        }
        self.name = name;
    }

    fn entries(&self) -> impl Iterator<Item = LocalEntry> + '_ {
        let folders = self.plan.folders.iter().map(|path| LocalEntry {
            path: path.clone(),
            kind: NodeKind::Folder,
            size: 0,
        });
        let files = self.plan.files.iter().map(|file| LocalEntry {
            path: file.path.clone(),
            kind: NodeKind::File,
            size: file.size,
        });
        folders.chain(files)
    }

    /// The size line of its row: `220 KB`, or `124 files · 118 MB` for a folder.
    fn detail(&self) -> String {
        let size = human_size(i64::try_from(self.bytes()).unwrap_or(i64::MAX));
        if self.folder {
            format!("{} · {size}", files(self.plan.files.len()))
        } else {
            size
        }
    }
}

/// A device that can be picked, by its owner and id.
type Choice = (UserId, Uuid);

#[derive(Debug, Clone)]
pub enum Message {
    Targets(Result<Vec<TransferTarget>, ApiError>),
    Choose(Choice),
    /// Opens the list of devices instead of the one chosen.
    ShowDevices,
    /// Shows other users' devices, or the user's own again.
    Others(bool),
    AddFiles,
    AddFolder,
    Picked(Result<Vec<Picked>, String>),
    Remove(String),
    Submit,
    Created(Result<CreatedTransfer, ApiError>),
    Cancel,
}

/// What the browser should do after an update.
pub enum Outcome {
    Task(Task<Message>),
    Closed,
    /// The transfer is made: the browser uploads `files` into it, and says it went to `label`.
    Sent {
        created: CreatedTransfer,
        files: Vec<SendFile>,
        label: String,
    },
}

/// The dialog asking where to send something, which then sends it.
pub struct SendDialog {
    source: Source,
    targets: Vec<TransferTarget>,
    loaded: bool,
    chosen: Option<Choice>,
    /// Whether the list of devices shows, rather than only the one chosen.
    choosing: bool,
    others: bool,
    picked: Vec<Picked>,
    sending: bool,
    error: Option<String>,
}

impl SendDialog {
    pub fn open(client: &Client, source: Source) -> (Self, Task<Message>) {
        let state = Self {
            source,
            targets: Vec::new(),
            loaded: false,
            chosen: None,
            choosing: false,
            others: false,
            picked: Vec::new(),
            sending: false,
            error: None,
        };
        let client = client.clone();
        let task = Task::perform(
            async move { client.transfer_targets().await },
            Message::Targets,
        );
        (state, task)
    }

    /// Whether it sends files from this computer, so dropped files go into it.
    pub fn is_local(&self) -> bool {
        matches!(self.source, Source::Local)
    }

    /// Adds files or folders dropped on the window.
    pub fn add_paths(&self, paths: Vec<PathBuf>) -> Task<Message> {
        if self.is_local() {
            Task::perform(read_picked(paths), Message::Picked)
        } else {
            Task::none()
        }
    }

    fn current(&self) -> Option<(&TransferTarget, &TargetDevice)> {
        let (user, device) = self.chosen?;
        let target = self.targets.iter().find(|target| target.user.id == user)?;
        let device = target.devices.iter().find(|item| item.id == device)?;
        Some((target, device))
    }

    fn can_send(&self) -> bool {
        !self.sending && self.current().is_some() && !(self.is_local() && self.picked.is_empty())
    }

    fn add(&mut self, new: Vec<Picked>) {
        let mut taken: BTreeSet<String> =
            self.picked.iter().map(|item| item.name.clone()).collect();
        for mut item in new {
            let name = free_name(&item.name, &taken);
            if name != item.name {
                item.rename(name.clone());
            }
            taken.insert(name);
            self.picked.push(item);
        }
    }

    pub fn update(&mut self, client: &Client, message: Message) -> Outcome {
        match message {
            Message::Targets(Ok(targets)) => {
                self.loaded = true;
                if self.chosen.is_none() {
                    self.chosen =
                        first_choice(&targets).map(|(target, device)| (target.user.id, device.id));
                    self.choosing = self.chosen.is_none();
                }
                self.targets = targets;
            }
            Message::Targets(Err(err)) => {
                self.loaded = true;
                self.choosing = true;
                self.error = Some(err.to_string());
            }
            Message::Choose(choice) => {
                self.chosen = Some(choice);
                self.choosing = false;
            }
            Message::ShowDevices => self.choosing = true,
            Message::Others(others) => self.others = others,
            Message::AddFiles => return Outcome::Task(pick(false)),
            Message::AddFolder => return Outcome::Task(pick(true)),
            Message::Picked(Ok(picked)) => self.add(picked),
            Message::Picked(Err(err)) => self.error = Some(err),
            Message::Remove(name) => self.picked.retain(|item| item.name != name),
            Message::Submit if self.can_send() => {
                let Some((target, device)) = self.current() else {
                    return Outcome::Task(Task::none());
                };
                let items = match &self.source {
                    Source::Nodes(nodes) => node_items(nodes),
                    Source::Local => {
                        let entries: Vec<LocalEntry> =
                            self.picked.iter().flat_map(Picked::entries).collect();
                        manifest(&entries)
                    }
                };
                let request = CreateTransfer {
                    recipient_id: target.user.id,
                    recipient_device: device.id,
                    items,
                    message: None,
                };
                self.sending = true;
                self.error = None;
                let client = client.clone();
                return Outcome::Task(Task::perform(
                    async move { client.create_transfer(&request).await },
                    Message::Created,
                ));
            }
            Message::Submit => {}
            Message::Created(Ok(created)) => {
                let files = self
                    .picked
                    .iter()
                    .flat_map(|item| item.plan.files.iter().cloned())
                    .collect();
                return Outcome::Sent {
                    created,
                    files,
                    label: self.label(),
                };
            }
            Message::Created(Err(err)) => {
                self.sending = false;
                self.error = Some(err.to_string());
            }
            Message::Cancel => return Outcome::Closed,
        }
        Outcome::Task(Task::none())
    }

    /// What was sent where, as the transfers panel names it: `report.pdf → Pixel 9`.
    fn label(&self) -> String {
        let what = names_line(&self.names());
        match self.current() {
            Some((target, device)) => format!("{what} → {}", recipient_name(target, device)),
            None => what,
        }
    }

    fn names(&self) -> Vec<String> {
        match &self.source {
            Source::Nodes(nodes) => nodes.iter().map(|node| node.name.clone()).collect(),
            Source::Local => self.picked.iter().map(|item| item.name.clone()).collect(),
        }
    }

    fn head(&self) -> (String, String) {
        match &self.source {
            Source::Nodes(nodes) => (send_title(&self.names()), nodes_subtitle(nodes)),
            Source::Local if self.picked.is_empty() => (
                "Send from this computer".to_owned(),
                "Pick files or folders to send.".to_owned(),
            ),
            Source::Local => {
                let folders = self.picked.iter().filter(|item| item.folder).count();
                let file_count = self.picked.iter().map(|item| item.plan.files.len()).sum();
                let bytes = self.picked.iter().map(Picked::bytes).sum();
                (
                    "Send from this computer".to_owned(),
                    items_summary(folders, file_count, bytes),
                )
            }
        }
    }

    fn note(&self) -> Option<String> {
        let (target, device) = self.current()?;
        Some(if self.is_local() {
            format!(
                "Keep the app open until the upload finishes. {} is notified when it is complete; the copy is kept until it is downloaded or expires.",
                recipient_name(target, device)
            )
        } else {
            send_note(target, device)
        })
    }

    /// The dialog panel, without the backdrop.
    pub fn view(&self) -> Element<'_, Message> {
        let (title, subtitle) = self.head();
        let mut lines = column![parts::dialog_head(title, subtitle)];
        if self.is_local() {
            lines = lines.push(section("Items", list(self.items())));
            lines = lines.push(if self.choosing {
                self.devices()
            } else {
                section("To", self.chosen_row())
            });
        } else {
            lines = lines.push(self.devices());
        }
        if let Some(note) = self.note() {
            lines = lines.push(text(note).size(SECOND_LINE_SIZE).style(theme::muted_text));
        }
        if let Some(error) = &self.error {
            lines = lines.push(parts::alert_error(Icon::Error, error.as_str(), None));
        }
        let button = match self.current() {
            _ if self.sending => SENDING.to_owned(),
            Some((target, device)) => format!("Send to {}", recipient_name(target, device)),
            None => "Send".to_owned(),
        };
        parts::dialog(
            WIDTH,
            lines,
            [
                parts::outlined(None, "Cancel", Some(Message::Cancel)),
                parts::primary(
                    Some(Icon::Send),
                    button,
                    self.can_send().then_some(Message::Submit),
                ),
            ],
        )
    }

    fn items(&self) -> Vec<Element<'_, Message>> {
        let mut rows: Vec<Element<'_, Message>> = self
            .picked
            .iter()
            .map(|item| {
                let icon = if item.folder {
                    Icon::Folder
                } else {
                    transfers_icon(&item.name)
                };
                let remove = button(Icon::Close.view(CHEVRON, Tint::Muted))
                    .padding(4)
                    .style(theme::tool)
                    .on_press(Message::Remove(item.name.clone()));
                container(
                    row![
                        icon.view(ICON_SIZE, Tint::Accent),
                        ellipsized(item.name.as_str(), ROW_SIZE, Alignment::Start)
                            .width(Length::Fill),
                        text(item.detail())
                            .size(SECOND_LINE_SIZE)
                            .style(theme::muted_text),
                        remove,
                    ]
                    .spacing(ROW_GAP)
                    .align_y(Alignment::Center),
                )
                .padding(ROW_PADDING)
                .center_y(ROW_HEIGHT)
                .into()
            })
            .collect();
        rows.push(
            container(
                row![
                    parts::ghost(Some(Icon::Add), "Add files…", Some(Message::AddFiles)),
                    parts::ghost(
                        Some(Icon::NewFolder),
                        "Add a folder…",
                        Some(Message::AddFolder)
                    ),
                    text("or drop them here")
                        .size(SECOND_LINE_SIZE)
                        .style(theme::muted_text),
                ]
                .spacing(ADD_GAP)
                .align_y(Alignment::Center),
            )
            .padding([4.0, 4.0])
            .into(),
        );
        rows
    }

    /// The device chosen, which opens the list of devices when pressed.
    fn chosen_row(&self) -> Element<'_, Message> {
        let content: Element<'_, Message> = match self.current() {
            Some((target, device)) => device_line(
                platform_icon(&device.platform),
                recipient_name(target, device),
                Some(device_status(device)),
                Icon::ExpandMore,
            ),
            None => note("Loading…"),
        };
        container(
            button(content)
                .padding(ROW_PADDING)
                .height(ROW_HEIGHT)
                .width(Length::Fill)
                .style(theme::row(false))
                .on_press(Message::ShowDevices),
        )
        .padding(1)
        .width(Length::Fill)
        .style(theme::card)
        .into()
    }

    /// The devices to send to: the user's own, or, a step further, other users' devices.
    fn devices(&self) -> Element<'_, Message> {
        let title = if self.others {
            "Another user’s device"
        } else {
            "Your devices"
        };
        let mut rows: Vec<Element<'_, Message>> = Vec::new();
        if self.others {
            rows.push(link_row(
                Icon::Back,
                "Back to your devices",
                Message::Others(false),
            ));
            for (target, devices) in pickable(&self.targets, false) {
                rows.push(
                    container(
                        row![
                            Icon::Person.view(ICON_SIZE, Tint::Muted),
                            text(target.user.username.as_str()).font(theme::MEDIUM),
                        ]
                        .spacing(ROW_GAP)
                        .align_y(Alignment::Center),
                    )
                    .padding(ROW_PADDING)
                    .center_y(ROW_HEIGHT)
                    .into(),
                );
                rows.extend(
                    devices
                        .into_iter()
                        .map(|device| self.device_row(target, device)),
                );
            }
        } else {
            let own = pickable(&self.targets, true);
            if !self.loaded {
                rows.push(note("Loading…"));
            } else if own.is_empty() {
                rows.push(note(NO_DEVICES));
            }
            for (target, devices) in own {
                rows.extend(
                    devices
                        .into_iter()
                        .map(|device| self.device_row(target, device)),
                );
            }
            if !pickable(&self.targets, false).is_empty() {
                rows.push(link_row(
                    Icon::Group,
                    "Another user’s device…",
                    Message::Others(true),
                ));
            }
        }
        section(title, list(rows))
    }

    fn device_row<'a>(
        &'a self,
        target: &'a TransferTarget,
        device: &'a TargetDevice,
    ) -> Element<'a, Message> {
        let choice = (target.user.id, device.id);
        let selected = self.chosen == Some(choice);
        let end = if selected { Icon::Check } else { Icon::Chevron };
        button(device_line(
            platform_icon(&device.platform),
            device.name.clone(),
            Some(device_status(device)),
            end,
        ))
        .padding(ROW_PADDING)
        .height(ROW_HEIGHT)
        .width(Length::Fill)
        .style(theme::row(selected))
        .on_press(Message::Choose(choice))
        .into()
    }
}

/// The items of a send of server files and folders, each taken by its id.
fn node_items(nodes: &[Node]) -> Vec<NewTransferItem> {
    nodes
        .iter()
        .map(|node| NewTransferItem {
            path: node.name.clone(),
            kind: node.kind,
            size: u64::try_from(node.size).unwrap_or_default(),
            node_id: Some(node.id),
            mime: None,
            modified_at: None,
        })
        .collect()
}

/// The subtitle of a send of server items: their size, and their names when there are several.
fn nodes_subtitle(nodes: &[Node]) -> String {
    let bytes: i64 = nodes
        .iter()
        .filter(|node| node.kind == NodeKind::File)
        .map(|node| node.size)
        .sum();
    let size = human_size(bytes);
    match nodes {
        [only] if only.kind == NodeKind::Folder => "Folder · everything in it".to_owned(),
        [_] => size,
        _ => {
            let names: Vec<String> = nodes.iter().map(|node| node.name.clone()).collect();
            format!("{} · {size}", names_line(&names))
        }
    }
}

/// The icon of a picked file, known by its name.
fn transfers_icon(name: &str) -> Icon {
    let node = Node {
        id: protocol::NodeId(Uuid::nil()),
        parent_id: None,
        kind: NodeKind::File,
        name: name.to_owned(),
        size: 0,
        sha256: None,
        mime: String::new(),
        version: 1,
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
        edited_on: None,
    };
    node_icon(&node)
}

/// Asks for files, or for a folder, and reads what was picked.
fn pick(folder: bool) -> Task<Message> {
    Task::perform(
        async move {
            let dialog = rfd::AsyncFileDialog::new();
            let paths: Vec<PathBuf> = if folder {
                dialog
                    .pick_folder()
                    .await
                    .map(|handle| handle.path().to_path_buf())
                    .into_iter()
                    .collect()
            } else {
                dialog
                    .pick_files()
                    .await
                    .unwrap_or_default()
                    .iter()
                    .map(|handle| handle.path().to_path_buf())
                    .collect()
            };
            read_picked(paths).await
        },
        Message::Picked,
    )
}

/// Walks each of `paths` into what sending it takes.
async fn read_picked(paths: Vec<PathBuf>) -> Result<Vec<Picked>, String> {
    let mut picked = Vec::new();
    for path in paths {
        let plan = transfers::plan_send(path.clone())
            .await
            .map_err(|err| format!("{}: {err}", path.display()))?;
        // A folder lists itself first; a file has no folders.
        let folder = !plan.folders.is_empty();
        let name = match (plan.folders.first(), plan.files.first()) {
            (Some(top), _) => top.clone(),
            (None, Some(file)) => top_name(&file.path).to_owned(),
            // Neither a file nor a folder, such as a socket.
            (None, None) => continue,
        };
        picked.push(Picked { name, folder, plan });
    }
    Ok(picked)
}

/// A titled part of the dialog.
fn section<'a>(title: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
    column![text(title).font(theme::MEDIUM), content]
        .spacing(SECTION_GAP)
        .into()
}

/// `rows` in a scrolling card, divided by hairlines.
fn list(rows: Vec<Element<'_, Message>>) -> Element<'_, Message> {
    let mut items = column![];
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            items = items.push(divider());
        }
        items = items.push(row);
    }
    container(scrollable(items.width(Length::Fill)))
        .padding(1)
        .max_height(LIST_HEIGHT)
        .width(Length::Fill)
        .style(theme::card)
        .into()
}

/// An icon, a name with a line below it, and `end` at the far side.
fn device_line<'a>(
    icon: Icon,
    name: String,
    detail: Option<String>,
    end: Icon,
) -> Element<'a, Message> {
    let tint = if end == Icon::Check {
        Tint::Accent
    } else {
        Tint::Muted
    };
    row![
        icon.view(ICON_SIZE, Tint::Text),
        column![ellipsized(name, ROW_SIZE, Alignment::Start)]
            .push(
                detail
                    .map(|detail| { text(detail).size(SECOND_LINE_SIZE).style(theme::muted_text) })
            )
            .width(Length::Fill),
        end.view(CHEVRON, tint),
    ]
    .spacing(ROW_GAP)
    .align_y(Alignment::Center)
    .height(Length::Fill)
    .into()
}

/// A row that leads somewhere else in the list.
fn link_row(icon: Icon, label: &str, message: Message) -> Element<'_, Message> {
    button(
        row![
            icon.view(ICON_SIZE, Tint::Muted),
            text(label).width(Length::Fill),
        ]
        .spacing(ROW_GAP)
        .align_y(Alignment::Center)
        .height(Length::Fill),
    )
    .padding(ROW_PADDING)
    .height(ROW_HEIGHT)
    .width(Length::Fill)
    .style(theme::row(false))
    .on_press(message)
    .into()
}

fn note(hint: &str) -> Element<'_, Message> {
    container(text(hint).style(theme::muted_text))
        .padding(NOTE_PADDING)
        .into()
}

#[cfg(test)]
mod tests {
    use iced_test::simulator;
    use protocol::{NodeId, TransferState, UserRef};
    use time::OffsetDateTime;

    use super::*;

    fn node(raw: u128, name: &str, kind: NodeKind, size: i64) -> Node {
        Node {
            id: NodeId(Uuid::from_u128(raw)),
            parent_id: None,
            kind,
            name: name.to_owned(),
            size,
            sha256: None,
            mime: String::new(),
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    fn device(raw: u128, name: &str, online: bool, current: bool) -> TargetDevice {
        TargetDevice {
            id: Uuid::from_u128(raw),
            name: name.to_owned(),
            platform: "android".to_owned(),
            signed_in: true,
            online,
            current,
        }
    }

    fn targets() -> Vec<TransferTarget> {
        vec![
            TransferTarget {
                user: UserRef {
                    id: UserId(Uuid::from_u128(1)),
                    username: "sam".to_owned(),
                },
                own: true,
                devices: vec![
                    device(10, "This Mac", true, true),
                    device(11, "Pixel 9", true, false),
                ],
            },
            TransferTarget {
                user: UserRef {
                    id: UserId(Uuid::from_u128(2)),
                    username: "dana".to_owned(),
                },
                own: false,
                devices: vec![device(20, "Phone", false, false)],
            },
        ]
    }

    fn client() -> Client {
        Client::new("http://localhost")
    }

    fn file(path: &str, size: u64) -> SendFile {
        SendFile {
            path: path.to_owned(),
            local: PathBuf::from("/tmp").join(path),
            size,
        }
    }

    fn picked_folder(name: &str) -> Picked {
        Picked {
            name: name.to_owned(),
            folder: true,
            plan: SendPlan {
                folders: vec![name.to_owned(), format!("{name}/day 1")],
                files: vec![file(&format!("{name}/day 1/a.jpg"), 3)],
            },
        }
    }

    fn opened(source: Source) -> SendDialog {
        let (mut dialog, _) = SendDialog::open(&client(), source);
        let _ = dialog.update(&client(), Message::Targets(Ok(targets())));
        dialog
    }

    #[test]
    fn the_first_choice_is_an_online_device_of_ones_own() {
        let dialog = opened(Source::Nodes(vec![node(
            1,
            "report.pdf",
            NodeKind::File,
            1536,
        )]));
        let mut ui = simulator(dialog.view());

        assert!(ui.find("Send report.pdf to a device").is_ok());
        assert!(ui.find("1.5 KB").is_ok());
        assert!(ui.find("Send to Pixel 9").is_ok());
        assert!(ui.find("This Mac").is_err());
        assert!(ui.find("Another user’s device…").is_ok());
    }

    #[test]
    fn another_users_device_is_named_with_its_owner() {
        let mut dialog = opened(Source::Nodes(vec![node(1, "a.pdf", NodeKind::File, 1)]));
        let _ = dialog.update(&client(), Message::Others(true));
        let _ = dialog.update(
            &client(),
            Message::Choose((UserId(Uuid::from_u128(2)), Uuid::from_u128(20))),
        );
        let mut ui = simulator(dialog.view());

        assert!(ui.find("dana").is_ok());
        assert!(ui.find("Send to dana’s Phone").is_ok());
        assert!(ui.find("Back to your devices").is_ok());
    }

    #[test]
    fn a_local_send_waits_for_something_to_send() {
        let mut dialog = opened(Source::Local);
        assert!(!dialog.can_send());

        let _ = dialog.update(&client(), Message::Picked(Ok(vec![picked_folder("Trip")])));

        assert!(dialog.can_send());
        let mut ui = simulator(dialog.view());
        assert!(ui.find("1 folder and 1 file · 3 B").is_ok());
        assert!(ui.find("1 file · 3 B").is_ok());
    }

    #[test]
    fn a_second_item_of_the_same_name_is_numbered() {
        let mut dialog = opened(Source::Local);
        let _ = dialog.update(&client(), Message::Picked(Ok(vec![picked_folder("Trip")])));

        let _ = dialog.update(&client(), Message::Picked(Ok(vec![picked_folder("Trip")])));

        let second = &dialog.picked[1];
        assert_eq!(second.name, "Trip (2)");
        assert_eq!(second.plan.folders, ["Trip (2)", "Trip (2)/day 1"]);
        assert_eq!(second.plan.files[0].path, "Trip (2)/day 1/a.jpg");
        let _ = dialog.update(&client(), Message::Remove("Trip".to_owned()));
        assert_eq!(dialog.picked.len(), 1);
    }

    #[test]
    fn a_made_transfer_hands_over_the_files_to_upload() {
        let mut dialog = opened(Source::Local);
        let _ = dialog.update(&client(), Message::Picked(Ok(vec![picked_folder("Trip")])));
        let created = CreatedTransfer {
            id: Uuid::from_u128(5),
            state: TransferState::Uploading,
            items: Vec::new(),
        };

        let Outcome::Sent { files, label, .. } =
            dialog.update(&client(), Message::Created(Ok(created)))
        else {
            panic!("the dialog did not send");
        };

        assert_eq!(files, [file("Trip/day 1/a.jpg", 3)]);
        assert_eq!(label, "Trip → Pixel 9");
    }

    #[test]
    fn server_items_are_sent_by_id_under_their_own_name() {
        let nodes = [
            node(1, "report.pdf", NodeKind::File, 1536),
            node(2, "Photos", NodeKind::Folder, 0),
        ];
        let items = node_items(&nodes);
        assert_eq!(items[0].path, "report.pdf");
        assert_eq!(items[0].node_id, Some(nodes[0].id));
        assert_eq!(items[1].kind, NodeKind::Folder);
        assert_eq!(nodes_subtitle(&nodes[1..]), "Folder · everything in it");
        assert_eq!(nodes_subtitle(&nodes), "report.pdf, Photos · 1.5 KB");
    }
}
