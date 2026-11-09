use api_client::{ApiError, Client};
use client_core::{format::human_size, labels::edited_on};
use iced::widget::text::Wrapping;
use iced::{
    Alignment, Element, Length,
    widget::{button, column, container, row, scrollable, text},
};
use protocol::{Device, FileVersion, Node, NodeId, NodeKind};

use super::{
    Message as BrowserMessage,
    parts::{alert_error, divider, small_danger, small_outlined, small_outlined_icon},
};
use crate::{
    icons::{ICON_SIZE, Icon, Tint, node_icon},
    locale::short_date,
    theme,
};

const WIDTH: f32 = 320.0;
const PADDING: f32 = 16.0;
const SPACING: f32 = 12.0;
const ROW_SPACING: f32 = 8.0;
const FACT_GAP: f32 = 12.0;
const TITLE_SIZE: f32 = 18.0;
const META_SIZE: f32 = 12.0;
const LABEL_WIDTH: f32 = 110.0;
const NAME_ICON: f32 = 24.0;
const CLOSE_PADDING: f32 = 4.0;
const VERSIONS_PADDING: [f32; 2] = [12.0, 16.0];
const VERSIONS_GAP: f32 = 4.0;
const VERSION_PADDING: [f32; 2] = [8.0, 0.0];
const VERSION_ICON: f32 = 16.0;
const VERSION_BUTTON_PADDING: f32 = 6.0;
const FOLDER_LABEL: &str = "Folder";

#[derive(Debug, Clone)]
pub enum Message {
    /// Opens the node; the browser knows how.
    Open,
    /// Downloads the node's current content; the browser does it.
    DownloadNode,
    Loaded(NodeId, Result<Box<Loaded>, ApiError>),
    Download(i32),
    Downloaded(Result<(), String>),
    AskDelete(i32),
    CancelDelete,
    Delete(i32),
    Deleted(NodeId, Result<(), ApiError>),
    Close,
}

/// A node with its previous versions and the devices that may have written them.
#[derive(Debug, Clone)]
pub struct Loaded {
    node: Node,
    versions: Vec<FileVersion>,
    devices: Vec<Device>,
    /// The device that auto-uploaded the file.
    source_device: Option<String>,
}

/// The details panel of one node.
pub struct State {
    node: Node,
    versions: Vec<FileVersion>,
    devices: Vec<Device>,
    source_device: Option<String>,
    confirming: Option<i32>,
    error: Option<String>,
}

impl State {
    /// Opens the panel for `node` and loads its versions.
    pub fn open(client: &Client, node: Node) -> (Self, Task) {
        let task = load(client, node.id);
        let state = Self {
            node,
            versions: Vec::new(),
            devices: Vec::new(),
            source_device: None,
            confirming: None,
            error: None,
        };
        (state, task)
    }

    pub fn node(&self) -> &Node {
        &self.node
    }

    pub fn node_id(&self) -> NodeId {
        self.node.id
    }

    pub fn refresh(&self, client: &Client) -> Task {
        load(client, self.node.id)
    }

    /// Applies `message`; `None` means the panel should close.
    pub fn update(&mut self, client: &Client, message: Message) -> Option<Task> {
        match message {
            Message::Open | Message::DownloadNode => Some(Task::none()),
            Message::Loaded(id, result) if id == self.node.id => {
                match result {
                    Ok(loaded) => {
                        self.node = loaded.node;
                        self.versions = loaded.versions;
                        self.devices = loaded.devices;
                        self.source_device = loaded.source_device;
                        self.error = None;
                    }
                    Err(err) => self.error = Some(err.to_string()),
                }
                Some(Task::none())
            }
            Message::Loaded(..) => Some(Task::none()),
            Message::Download(version) => {
                Some(download(client.clone(), self.node.clone(), version))
            }
            Message::Downloaded(result) => {
                self.error = result.err();
                Some(Task::none())
            }
            Message::AskDelete(version) => {
                self.confirming = Some(version);
                Some(Task::none())
            }
            Message::CancelDelete => {
                self.confirming = None;
                Some(Task::none())
            }
            Message::Delete(version) => {
                self.confirming = None;
                let (client, id) = (client.clone(), self.node.id);
                Some(Task::perform(
                    async move { client.delete_version(id, version).await },
                    move |result| Message::Deleted(id, result),
                ))
            }
            Message::Deleted(id, result) if id == self.node.id => {
                self.error = result.err().map(|err| err.to_string());
                Some(load(client, id))
            }
            Message::Deleted(..) => Some(Task::none()),
            Message::Close => None,
        }
    }

    /// The panel; `synced_to` is where the node syncs on disk, if it does.
    pub fn view(&self, synced_to: Option<String>) -> Element<'_, BrowserMessage> {
        let node = &self.node;
        let is_file = node.kind == NodeKind::File;
        let tint = if is_file { Tint::Text } else { Tint::Accent };
        let name = row![
            node_icon(node).view(NAME_ICON, tint),
            text(node.name.as_str())
                .size(TITLE_SIZE)
                .font(theme::SEMIBOLD)
                .wrapping(Wrapping::WordOrGlyph)
                .width(Length::Fill),
            button(Icon::Close.view(ICON_SIZE, Tint::Muted))
                .padding(CLOSE_PADDING)
                .style(theme::tool)
                .on_press(Message::Close),
        ]
        .spacing(ROW_SPACING);
        let buttons = row![
            small_outlined_icon(Icon::Web, "Open", Some(Message::Open)),
            small_outlined_icon(Icon::Download, "Download", Some(Message::DownloadNode)),
        ]
        .spacing(ROW_SPACING);
        let mut head = column![name, buttons].spacing(SPACING);
        if let Some(error) = &self.error {
            head = head.push(alert_error(Icon::Error, error.as_str(), None));
        }

        let mut facts = column![fact("Type", kind_label(node))].spacing(ROW_SPACING);
        if is_file {
            facts = facts
                .push(fact("Size", human_size(node.size)))
                .push(fact("Version", node.version.to_string()));
        }
        facts = facts
            .push(fact("Created", short_date(node.created_at)))
            .push(fact("Modified", short_date(node.updated_at)));
        if let Some(device) = edited_on(node.edited_on, &self.devices) {
            let label = if is_file { "Edited on" } else { "Created on" };
            facts = facts.push(fact(label, device));
        }
        if let Some(device) = &self.source_device {
            facts = facts.push(fact("Uploaded from", device.clone()));
        }
        if let Some(local) = synced_to {
            facts = facts.push(fact("Synced to", local));
        }

        let mut body = column![container(facts).padding(PADDING)];
        if is_file {
            body = body.push(divider()).push(
                container(
                    column![
                        text("Previous versions").font(theme::MEDIUM),
                        self.version_list()
                    ]
                    .spacing(VERSIONS_GAP),
                )
                .padding(VERSIONS_PADDING),
            );
        }
        let panel: Element<'_, Message> = container(column![
            container(head).padding(PADDING),
            divider(),
            scrollable(body).height(Length::Fill),
        ])
        .width(WIDTH)
        .height(Length::Fill)
        .style(theme::card)
        .into();
        panel.map(BrowserMessage::Details)
    }

    fn version_list(&self) -> Element<'_, Message> {
        if self.versions.is_empty() {
            return text("No previous versions.")
                .size(META_SIZE)
                .style(theme::muted_text)
                .into();
        }
        let rows = self
            .versions
            .iter()
            .enumerate()
            .flat_map(|(index, version)| {
                let number = version.version;
                let mut meta = format!(
                    "{} · {}",
                    short_date(version.modified_at),
                    human_size(version.size)
                );
                if let Some(device) = edited_on(version.edited_on, &self.devices) {
                    meta = format!("{meta} · {device}");
                }
                let info = column![
                    text(format!("Version {number}")),
                    text(meta).size(META_SIZE).style(theme::muted_text)
                ]
                .width(Length::Fill);
                let line: Element<'_, Message> = if self.confirming == Some(number) {
                    column![
                        info,
                        row![
                            small_danger("Delete", Some(Message::Delete(number))),
                            small_outlined("Cancel", Some(Message::CancelDelete)),
                        ]
                        .spacing(ROW_SPACING),
                    ]
                    .spacing(ROW_SPACING)
                    .into()
                } else {
                    row![
                        info,
                        version_button(Icon::Download, Message::Download(number)),
                        version_button(Icon::Delete, Message::AskDelete(number)),
                    ]
                    .align_y(Alignment::Center)
                    .into()
                };
                let line = container(line).padding(VERSION_PADDING).into();
                (index > 0)
                    .then(divider)
                    .into_iter()
                    .chain(std::iter::once(line))
            });
        column(rows).into()
    }
}

type Task = iced::Task<Message>;

fn fact(label: &str, value: String) -> Element<'_, Message> {
    row![
        text(label).width(LABEL_WIDTH).style(theme::muted_text),
        text(value)
            .wrapping(Wrapping::WordOrGlyph)
            .width(Length::Fill),
    ]
    .spacing(FACT_GAP)
    .into()
}

fn version_button<'a>(icon: Icon, message: Message) -> Element<'a, Message> {
    button(icon.view(VERSION_ICON, Tint::Muted))
        .padding(VERSION_BUTTON_PADDING)
        .style(theme::tool)
        .on_press(message)
        .into()
}

/// What a node is, in words where its MIME type has them.
fn kind_label(node: &Node) -> String {
    if node.kind == NodeKind::Folder {
        return FOLDER_LABEL.to_owned();
    }
    let mime = node.mime.as_str();
    match mime.split_once('/') {
        _ if mime == "application/pdf" => "PDF document".to_owned(),
        Some(("text", "plain")) => "Text document".to_owned(),
        Some(("image", kind)) => format!("{} image", kind.to_uppercase()),
        Some(("video", kind)) => format!("{} video", kind.to_uppercase()),
        Some(("audio", kind)) => format!("{} audio", kind.to_uppercase()),
        _ => mime.to_owned(),
    }
}

fn load(client: &Client, id: NodeId) -> Task {
    let client = client.clone();
    Task::perform(
        async move {
            let node = client.get(id).await?;
            let versions = match node.kind {
                NodeKind::File => client.versions(id).await?,
                NodeKind::Folder => Vec::new(),
            };
            let devices = client.devices().await?;
            // Only a label, and an older server has none to give.
            let properties = client.node_properties(id).await.unwrap_or_default();
            Ok(Box::new(Loaded {
                node,
                versions,
                devices,
                source_device: properties.source_device,
            }))
        },
        move |result| Message::Loaded(id, result),
    )
}

fn download(client: Client, node: Node, version: i32) -> Task {
    Task::perform(
        async move {
            let Some(handle) = rfd::AsyncFileDialog::new()
                .set_file_name(node.name.as_str())
                .save_file()
                .await
            else {
                return Ok(());
            };
            save(&client, node.id, version, handle.path())
                .await
                .map_err(|err| err.to_string())
        },
        Message::Downloaded,
    )
}

async fn save(
    client: &Client,
    id: NodeId,
    version: i32,
    target: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::io::AsyncWriteExt;

    let mut response = client.download_version(id, version).await?;
    let mut out = tokio::fs::File::create(target).await?;
    while let Some(chunk) = response.chunk().await? {
        out.write_all(&chunk).await?;
    }
    out.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use iced_test::simulator;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use protocol::PLATFORM_LINUX;

    use super::*;

    const LAPTOP: u128 = 7;

    fn file() -> Node {
        Node {
            id: NodeId(Uuid::from_u128(1)),
            parent_id: None,
            kind: NodeKind::File,
            name: "report.pdf".to_owned(),
            size: 2048,
            sha256: None,
            mime: "application/pdf".to_owned(),
            version: 2,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: Some(Uuid::from_u128(LAPTOP)),
        }
    }

    fn laptop() -> Device {
        Device {
            id: Uuid::from_u128(LAPTOP),
            name: "Laptop".to_owned(),
            platform: PLATFORM_LINUX.to_owned(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            last_seen_at: OffsetDateTime::UNIX_EPOCH,
            signed_in: true,
            online: false,
            current: false,
            uploads: 0,
            visibility: Default::default(),
            visible_to: Vec::new(),
        }
    }

    #[test]
    fn the_panel_names_the_device_a_file_was_edited_on() {
        let client = Client::new("http://localhost");
        let node = file();
        let (mut state, _task) = State::open(&client, node.clone());
        let loaded = Loaded {
            node,
            versions: Vec::new(),
            devices: vec![laptop()],
            source_device: Some("Phone".to_owned()),
        };
        let _ = state.update(
            &client,
            Message::Loaded(state.node_id(), Ok(Box::new(loaded))),
        );

        let mut ui = simulator(state.view(None));

        assert!(ui.find("Edited on").is_ok());
        assert!(ui.find("Laptop").is_ok());
        assert!(ui.find("Uploaded from").is_ok());
        assert!(ui.find("Phone").is_ok());
    }

    #[test]
    fn a_kind_is_in_words_where_it_has_any() {
        let with = |mime: &str| Node {
            mime: mime.to_owned(),
            ..file()
        };
        assert_eq!(kind_label(&file()), "PDF document");
        assert_eq!(kind_label(&with("image/jpeg")), "JPEG image");
        assert_eq!(kind_label(&with("application/zip")), "application/zip");
        let folder = Node {
            kind: NodeKind::Folder,
            ..file()
        };
        assert_eq!(kind_label(&folder), "Folder");
    }
}
