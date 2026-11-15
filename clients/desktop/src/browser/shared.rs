//! "Shared with me": what other users shared with the user, read-only. Folders open in place;
//! files and folders download into the downloads folder, or are saved as copies into the
//! user's own files.

use std::path::{Path, PathBuf};

use iced::{
    Alignment, Element, Length, Task,
    futures::StreamExt,
    widget::{button, column, container, row, scrollable, space, text, tooltip},
};
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use api_client::{ApiError, Client};
use client_core::{
    format::{human_size, sanitize_component},
    received::{
        ACCESS, EXPIRES, NEVER, NOTHING_SHARED_WITH_ME, SAVE_TO_MY_FILES, SHARED_BY,
        SHARED_WITH_ME, SharedItem, VIEW_ONLY, find_share, shared_items, view_only_note,
    },
    shared_link::{DOWNLOAD, EMPTY_FOLDER, initial, sort_shared},
};
use protocol::{NodeKind, ReceivedShare, SharedNode};

use super::{
    parts::{
        COLUMN_GAP, DATE_WIDTH, LIST_PADDING_WIDE, ROW_HEIGHT, ROW_SIZE, SIZE_WIDTH, alert_error,
        alert_info, divider, empty, ghost, header_bar, list_box, muted_cell, small_outlined,
        small_primary, title,
    },
    transfer_list::{display_download, downloads_folder, free_path, io_error},
    transfer_save::{self, SaveSource, TransferSave},
};
use crate::{
    ellipsis::ellipsized,
    icons::{ICON_SIZE, Icon, Tint, kind_icon},
    locale::short_date,
    theme,
};

const SPACING: f32 = 12.0;
const NAME_GAP: f32 = 12.0;
const ACTION_GAP: f32 = 4.0;
const OWNER_WIDTH: f32 = 180.0;
const ACCESS_WIDTH: f32 = 110.0;
const ACTIONS_WIDTH: f32 = 72.0;
const AVATAR: f32 = 22.0;
const AVATAR_TEXT: f32 = 11.0;
const TIP_PADDING: [f32; 2] = [4.0, 8.0];
const TIP_SIZE: f32 = 12.0;
const NO_SIZE: &str = "—";
const STAGING_SUFFIX: &str = ".part";
/// Below this content width the access column, the same for every share for now, goes.
const ACCESS_BELOW: f32 = 760.0;
/// The most files and folders one download walks, so a huge share cannot run forever.
const MAX_DOWNLOAD_ENTRIES: usize = 10_000;

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<ReceivedShare>, ApiError>),
    Refresh,
    /// Opens a folder of a share, below the folders already open in it.
    Open(Uuid, SharedNode),
    /// Goes back to the folder at this depth of the trail; 0 is everything shared.
    Crumb(usize),
    /// What a folder holds, for the load that asked.
    Listed(u64, Result<Vec<SharedNode>, ApiError>),
    Download(Uuid, SharedNode),
    Downloaded(Result<PathBuf, String>),
    OpenDownloaded(PathBuf),
    Dismiss,
    Save(Uuid, SharedNode),
    SaveDialog(transfer_save::Message),
}

/// A share opened at one of its folders.
struct Opened {
    share: ReceivedShare,
    /// The folders opened from the shared items down to the one shown.
    trail: Vec<SharedNode>,
    items: Option<Vec<SharedNode>>,
}

pub struct State {
    lost_and_found: protocol::NodeId,
    shares: Vec<ReceivedShare>,
    loading: bool,
    error: Option<String>,
    opened: Option<Opened>,
    /// Counts the folder loads, so an answer overtaken by a later one is dropped.
    loads: u64,
    downloading: usize,
    downloaded: Option<PathBuf>,
    saving: Option<TransferSave>,
    saved: bool,
}

impl State {
    pub fn open(lost_and_found: protocol::NodeId) -> Self {
        Self {
            lost_and_found,
            shares: Vec::new(),
            loading: true,
            error: None,
            opened: None,
            loads: 0,
            downloading: 0,
            downloaded: None,
            saving: None,
            saved: false,
        }
    }

    pub fn load(&mut self, client: &Client) -> Task<Message> {
        self.loading = true;
        let client = client.clone();
        Task::perform(
            async move { client.shared_with_me().await },
            Message::Loaded,
        )
    }

    fn list(&mut self, client: &Client) -> Task<Message> {
        let Some(opened) = &mut self.opened else {
            return Task::none();
        };
        let Some(folder) = opened.trail.last() else {
            return Task::none();
        };
        opened.items = None;
        self.loads += 1;
        let (client, load, share, folder) =
            (client.clone(), self.loads, opened.share.id, folder.id);
        Task::perform(
            async move {
                let mut items = client.shared_children(share, folder).await?;
                sort_shared(&mut items);
                Ok(items)
            },
            move |result| Message::Listed(load, result),
        )
    }

    pub fn update(&mut self, client: &Client, message: Message) -> Task<Message> {
        match message {
            Message::Loaded(result) => {
                self.loading = false;
                match result {
                    Ok(shares) => {
                        if let Some(opened) = &mut self.opened {
                            match find_share(&shares, opened.share.id) {
                                Some(share) => opened.share = share.clone(),
                                None => self.opened = None,
                            }
                        }
                        self.shares = shares;
                        self.error = None;
                    }
                    Err(err) => self.error = Some(err.to_string()),
                }
                Task::none()
            }
            Message::Refresh => {
                let listing = self.list(client);
                Task::batch([self.load(client), listing])
            }
            Message::Open(share, folder) => {
                match &mut self.opened {
                    Some(opened) if opened.share.id == share => opened.trail.push(folder),
                    _ => {
                        let Some(found) = find_share(&self.shares, share) else {
                            return Task::none();
                        };
                        self.opened = Some(Opened {
                            share: found.clone(),
                            trail: vec![folder],
                            items: None,
                        });
                    }
                }
                self.error = None;
                self.list(client)
            }
            Message::Crumb(0) => {
                self.opened = None;
                self.error = None;
                Task::none()
            }
            Message::Crumb(depth) => {
                if let Some(opened) = &mut self.opened {
                    opened.trail.truncate(depth);
                }
                self.list(client)
            }
            Message::Listed(load, result) if load == self.loads => {
                match result {
                    Ok(items) => {
                        if let Some(opened) = &mut self.opened {
                            opened.items = Some(items);
                        }
                    }
                    Err(err) => self.error = Some(err.to_string()),
                }
                Task::none()
            }
            Message::Listed(..) => Task::none(),
            Message::Download(share, item) => {
                let Some(root) = downloads_folder() else {
                    self.error = Some("Could not find your Downloads folder.".to_owned());
                    return Task::none();
                };
                self.downloading += 1;
                self.downloaded = None;
                self.error = None;
                let client = client.clone();
                Task::perform(
                    async move { fetch(&client, share, &item, &root).await },
                    Message::Downloaded,
                )
            }
            Message::Downloaded(result) => {
                self.downloading = self.downloading.saturating_sub(1);
                match result {
                    Ok(path) => self.downloaded = Some(path),
                    Err(err) => self.error = Some(err),
                }
                Task::none()
            }
            Message::OpenDownloaded(path) => {
                if let Err(err) = crate::transfers::open_file(&path) {
                    self.error = Some(err);
                }
                Task::none()
            }
            Message::Dismiss => {
                self.downloaded = None;
                self.saved = false;
                Task::none()
            }
            Message::Save(share, item) => {
                let owner = find_share(&self.shares, share)
                    .map(|share| share.owner.username.clone())
                    .unwrap_or_default();
                let source = SaveSource::Shared {
                    share,
                    items: vec![item],
                    owner,
                };
                let (dialog, task) = TransferSave::open_for(client, source, self.lost_and_found);
                self.saving = Some(dialog);
                self.saved = false;
                task.map(Message::SaveDialog)
            }
            Message::SaveDialog(message) => {
                let Some(dialog) = &mut self.saving else {
                    return Task::none();
                };
                match dialog.update(client, message) {
                    transfer_save::Outcome::Task(task) => task.map(Message::SaveDialog),
                    transfer_save::Outcome::Closed => {
                        self.saving = None;
                        Task::none()
                    }
                    transfer_save::Outcome::Saved => {
                        self.saving = None;
                        self.saved = true;
                        Task::none()
                    }
                }
            }
        }
    }

    /// Whether the save dialog is open.
    pub fn is_asking(&self) -> bool {
        self.saving.is_some()
    }

    /// The save dialog, while it is open.
    pub fn dialog(&self) -> Option<Element<'_, Message>> {
        self.saving
            .as_ref()
            .map(|saving| saving.view().map(Message::SaveDialog))
    }

    /// What the page's top line offers.
    pub fn action(&self) -> Element<'_, Message> {
        let busy = if self.loading {
            "Loading…".to_owned()
        } else if self.downloading > 0 {
            "Downloading…".to_owned()
        } else {
            String::new()
        };
        row![
            text(busy).style(theme::muted_text),
            ghost(Some(Icon::Refresh), "Refresh", Some(Message::Refresh))
        ]
        .spacing(SPACING)
        .align_y(Alignment::Center)
        .into()
    }

    pub fn view(&self, width: f32) -> Element<'_, Message> {
        let mut page = column![].spacing(SPACING);
        if let Some(opened) = &self.opened {
            page = page.push(crumbs(&opened.trail)).push(banner(&opened.share));
        }
        if let Some(error) = &self.error {
            page = page.push(alert_error(Icon::Error, error.as_str(), None));
        }
        if let Some(path) = &self.downloaded {
            let actions = row![
                small_primary(
                    Icon::Web,
                    "Open",
                    Some(Message::OpenDownloaded(path.clone()))
                ),
                close_button(),
            ]
            .spacing(ACTION_GAP)
            .align_y(Alignment::Center);
            page = page.push(alert_info(
                Icon::Download,
                format!("Downloaded to {}", display_download(path)),
                Some(actions.into()),
            ));
        }
        if self.saved {
            page = page.push(alert_info(
                Icon::Check,
                "Saved a copy to your files.",
                Some(close_button()),
            ));
        }
        let list = match &self.opened {
            Some(opened) => folder_list(opened),
            None => top_list(&self.shares, self.loading, width >= ACCESS_BELOW),
        };
        page.push(list_box(list)).into()
    }
}

fn close_button<'a>() -> Element<'a, Message> {
    button(Icon::Close.view(ICON_SIZE, Tint::Muted))
        .padding(2)
        .style(theme::tool)
        .on_press(Message::Dismiss)
        .into()
}

/// "Shared with me › Holiday 2026 › Day 1", each a way back.
fn crumbs(trail: &[SharedNode]) -> Element<'_, Message> {
    let mut line = row![small_outlined(SHARED_WITH_ME, Some(Message::Crumb(0)))]
        .spacing(ACTION_GAP)
        .align_y(Alignment::Center);
    for (index, folder) in trail.iter().enumerate() {
        let depth = index + 1;
        let current = depth == trail.len();
        line = line.push(Icon::Chevron.view(ICON_SIZE, Tint::Muted));
        line = line.push(small_outlined(
            folder.name.as_str(),
            (!current).then_some(Message::Crumb(depth)),
        ));
    }
    line.into()
}

/// Who shared the open share, and that it can only be read.
fn banner(share: &ReceivedShare) -> Element<'_, Message> {
    let until = share.expires_at.map(short_date);
    let note = view_only_note(&share.owner.username, until.as_deref());
    container(
        row![
            avatar(&share.owner.username),
            text(note).size(ROW_SIZE).width(Length::Fill),
            view_only(),
        ]
        .spacing(SPACING)
        .align_y(Alignment::Center),
    )
    .padding([8.0, 12.0])
    .width(Length::Fill)
    .style(theme::card)
    .into()
}

fn top_list(shares: &[ReceivedShare], loading: bool, with_access: bool) -> Element<'_, Message> {
    let mut titles = row![
        title("Name", Length::Fill, Alignment::Start),
        title(SHARED_BY, OWNER_WIDTH, Alignment::Start),
    ]
    .spacing(COLUMN_GAP);
    if with_access {
        titles = titles.push(title(ACCESS, ACCESS_WIDTH, Alignment::Start));
    }
    titles = titles
        .push(title(EXPIRES, DATE_WIDTH, Alignment::Start))
        .push(space().width(ACTIONS_WIDTH));
    let rows = shared_items(shares);
    let body: Element<'_, Message> = if rows.is_empty() {
        if loading {
            space().into()
        } else {
            empty(NOTHING_SHARED_WITH_ME)
        }
    } else {
        let rows = rows
            .into_iter()
            .flat_map(|row| [top_row(row, with_access), divider()]);
        scrollable(column(rows)).height(Length::Fill).into()
    };
    column![header_bar(titles, LIST_PADDING_WIDE), body].into()
}

fn top_row<'a>(row_item: SharedItem, with_access: bool) -> Element<'a, Message> {
    let SharedItem {
        share_id,
        owner,
        item,
        expires_at,
    } = row_item;
    let expires = expires_at.map_or_else(|| NEVER.to_owned(), short_date);
    let mut cells = row![
        name(&item),
        row![
            avatar(&owner.username),
            ellipsized(owner.username, ROW_SIZE, Alignment::Start).width(Length::Fill)
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(OWNER_WIDTH),
    ]
    .spacing(COLUMN_GAP)
    .height(Length::Fill)
    .align_y(Alignment::Center);
    if with_access {
        cells = cells.push(container(view_only()).width(ACCESS_WIDTH));
    }
    cells = cells
        .push(muted_cell(expires, DATE_WIDTH, Alignment::Start))
        .push(actions(share_id, &item));
    item_button(cells, share_id, item)
}

fn folder_list(opened: &Opened) -> Element<'_, Message> {
    let titles = row![
        title("Name", Length::Fill, Alignment::Start),
        title("Size", SIZE_WIDTH, Alignment::End),
        title("Modified", DATE_WIDTH, Alignment::End),
        space().width(ACTIONS_WIDTH),
    ]
    .spacing(COLUMN_GAP);
    let body: Element<'_, Message> = match &opened.items {
        None => space().into(),
        Some(items) if items.is_empty() => empty(EMPTY_FOLDER),
        Some(items) => {
            let share = opened.share.id;
            let rows = items
                .iter()
                .flat_map(|item| [folder_row(share, item.clone()), divider()]);
            scrollable(column(rows)).height(Length::Fill).into()
        }
    };
    column![header_bar(titles, LIST_PADDING_WIDE), body].into()
}

fn folder_row<'a>(share: Uuid, item: SharedNode) -> Element<'a, Message> {
    let size = match item.kind {
        NodeKind::Folder => NO_SIZE.to_owned(),
        NodeKind::File => human_size(item.size),
    };
    let cells = row![
        name(&item),
        muted_cell(size, SIZE_WIDTH, Alignment::End),
        muted_cell(short_date(item.updated_at), DATE_WIDTH, Alignment::End),
        actions(share, &item),
    ]
    .spacing(COLUMN_GAP)
    .height(Length::Fill)
    .align_y(Alignment::Center);
    item_button(cells, share, item)
}

/// A row; a folder's opens it.
fn item_button<'a>(
    cells: iced::widget::Row<'a, Message>,
    share: Uuid,
    item: SharedNode,
) -> Element<'a, Message> {
    let is_folder = item.kind == NodeKind::Folder;
    button(cells)
        .width(Length::Fill)
        .height(ROW_HEIGHT)
        .padding(LIST_PADDING_WIDE)
        .style(theme::row(false))
        .on_press_maybe(is_folder.then_some(Message::Open(share, item)))
        .into()
}

fn name<'a>(item: &SharedNode) -> Element<'a, Message> {
    let icon = kind_icon(item.kind, &item.mime);
    let tint = match item.kind {
        NodeKind::Folder => Tint::Accent,
        NodeKind::File => Tint::Text,
    };
    row![
        icon.view(ICON_SIZE, tint),
        ellipsized(item.name.clone(), ROW_SIZE, Alignment::Start).width(Length::Fill)
    ]
    .spacing(NAME_GAP)
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .into()
}

/// Download, and Save to my files.
fn actions<'a>(share: Uuid, item: &SharedNode) -> Element<'a, Message> {
    row![
        action(
            Icon::Download,
            DOWNLOAD,
            Message::Download(share, item.clone())
        ),
        action(
            Icon::SaveToServer,
            SAVE_TO_MY_FILES,
            Message::Save(share, item.clone())
        ),
    ]
    .spacing(ACTION_GAP)
    .width(ACTIONS_WIDTH)
    .into()
}

fn action<'a>(icon: Icon, label: &'a str, message: Message) -> Element<'a, Message> {
    tooltip(
        button(icon.view(ICON_SIZE, Tint::Muted))
            .padding(4)
            .style(theme::tool)
            .on_press(message),
        container(text(label).size(TIP_SIZE))
            .padding(TIP_PADDING)
            .style(theme::menu),
        tooltip::Position::Bottom,
    )
    .into()
}

fn avatar<'a>(name: &str) -> Element<'a, Message> {
    container(text(initial(name)).size(AVATAR_TEXT).font(theme::BOLD))
        .center(AVATAR)
        .style(theme::avatar)
        .into()
}

fn view_only<'a>() -> Element<'a, Message> {
    container(
        row![
            Icon::Visibility.view(14.0, Tint::Muted),
            text(VIEW_ONLY).size(12)
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .padding([2.0, 8.0])
    .style(theme::pill(false))
    .into()
}

/// Downloads `item` of `share` into `root`: a file as it is, a folder with everything inside.
/// Everything is put together beside `root` first and moved in once complete. Answers where
/// it went.
async fn fetch(
    client: &Client,
    share: Uuid,
    item: &SharedNode,
    root: &Path,
) -> Result<PathBuf, String> {
    fs::create_dir_all(root).await.map_err(io_error(root))?;
    let staging = root.join(format!(".{}{STAGING_SUFFIX}", item.id));
    let _ = fs::remove_dir_all(&staging).await;
    fs::create_dir_all(&staging)
        .await
        .map_err(io_error(&staging))?;
    let top = sanitize_component(&item.name);
    let fetched = fetch_into(client, share, item, &staging.join(&top)).await;
    let moved = match fetched {
        Ok(()) => {
            let target = free_path(root, &top).await;
            fs::rename(staging.join(&top), &target)
                .await
                .map(|()| target.clone())
                .map_err(io_error(&target))
        }
        Err(err) => Err(err),
    };
    let _ = fs::remove_dir_all(&staging).await;
    moved
}

async fn fetch_into(
    client: &Client,
    share: Uuid,
    item: &SharedNode,
    path: &Path,
) -> Result<(), String> {
    let mut pending = vec![(item.clone(), path.to_path_buf())];
    let mut walked = 0;
    while let Some((node, path)) = pending.pop() {
        walked += 1;
        if walked > MAX_DOWNLOAD_ENTRIES {
            return Err(format!(
                "{} holds more than {MAX_DOWNLOAD_ENTRIES} items; download it from the web instead.",
                item.name
            ));
        }
        match node.kind {
            NodeKind::Folder => {
                fs::create_dir_all(&path).await.map_err(io_error(&path))?;
                let children = client
                    .shared_children(share, node.id)
                    .await
                    .map_err(|err| err.to_string())?;
                pending.extend(children.into_iter().map(|child| {
                    let child_path = path.join(sanitize_component(&child.name));
                    (child, child_path)
                }));
            }
            NodeKind::File => {
                let response = client
                    .download_shared(share, node.id)
                    .await
                    .map_err(|err| err.to_string())?;
                let mut file = fs::File::create(&path).await.map_err(io_error(&path))?;
                let mut body = response.bytes_stream();
                while let Some(chunk) = body.next().await {
                    let chunk = chunk.map_err(|err| err.to_string())?;
                    file.write_all(&chunk).await.map_err(io_error(&path))?;
                }
                file.flush().await.map_err(io_error(&path))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use iced_test::simulator;
    use time::OffsetDateTime;

    use protocol::{NodeId, UserId, UserRef};

    use super::*;

    fn node(raw: u128, kind: NodeKind, name: &str) -> SharedNode {
        SharedNode {
            id: NodeId(Uuid::from_u128(raw)),
            kind,
            name: name.to_owned(),
            size: 2048,
            mime: String::new(),
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn share() -> ReceivedShare {
        ReceivedShare {
            id: Uuid::from_u128(7),
            owner: UserRef {
                id: UserId(Uuid::from_u128(8)),
                username: "alex".to_owned(),
            },
            items: vec![
                node(1, NodeKind::File, "tax.pdf"),
                node(2, NodeKind::Folder, "Holiday"),
            ],
            expires_at: None,
            shared_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn client() -> Client {
        Client::new("http://localhost")
    }

    fn loaded() -> State {
        let mut state = State::open(NodeId(Uuid::from_u128(99)));
        let _ = state.update(&client(), Message::Loaded(Ok(vec![share()])));
        state
    }

    #[test]
    fn the_list_shows_the_items_and_who_shared_them() {
        let state = loaded();
        let mut ui = simulator(state.view(1000.0));

        assert!(ui.find("Holiday").is_ok());
        assert!(ui.find("tax.pdf").is_ok());
        assert!(ui.find("alex").is_ok());
        assert!(ui.find(VIEW_ONLY).is_ok());
        assert!(ui.find(NEVER).is_ok());
    }

    #[test]
    fn nothing_shared_says_so() {
        let mut state = State::open(NodeId(Uuid::from_u128(99)));
        let _ = state.update(&client(), Message::Loaded(Ok(Vec::new())));
        let mut ui = simulator(state.view(1000.0));

        assert!(ui.find(NOTHING_SHARED_WITH_ME).is_ok());
    }

    #[test]
    fn a_folder_opens_with_who_shared_it_and_a_way_back() {
        let mut state = loaded();
        let _ = state.update(
            &client(),
            Message::Open(share().id, node(2, NodeKind::Folder, "Holiday")),
        );
        let load = state.loads;
        let _ = state.update(
            &client(),
            Message::Listed(load, Ok(vec![node(3, NodeKind::File, "tram.txt")])),
        );
        let mut ui = simulator(state.view(1000.0));

        assert!(ui.find("tram.txt").is_ok());
        assert!(ui.find(view_only_note("alex", None)).is_ok());
        ui.click(SHARED_WITH_ME).expect("way back");
        let messages: Vec<_> = ui.into_messages().collect();
        assert!(matches!(messages.as_slice(), [Message::Crumb(0)]));
    }

    #[test]
    fn an_answer_for_an_older_folder_is_dropped() {
        let mut state = loaded();
        let _ = state.update(
            &client(),
            Message::Open(share().id, node(2, NodeKind::Folder, "Holiday")),
        );
        let stale = state.loads;
        let _ = state.update(&client(), Message::Crumb(1));

        let _ = state.update(
            &client(),
            Message::Listed(stale, Ok(vec![node(3, NodeKind::File, "old.txt")])),
        );

        assert!(
            state
                .opened
                .as_ref()
                .is_some_and(|opened| opened.items.is_none())
        );
    }

    #[test]
    fn saving_opens_the_folder_picker_for_the_item() {
        let mut state = loaded();

        let _ = state.update(
            &client(),
            Message::Save(share().id, node(1, NodeKind::File, "tax.pdf")),
        );

        let dialog = state.dialog().expect("save dialog");
        let mut ui = simulator(dialog);
        assert!(ui.find("Save “tax.pdf” to my files").is_ok());
        assert!(ui.find("Shared by alex").is_ok());
    }
}
