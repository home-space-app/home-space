//! The Transfers page: what other devices sent to this one, and what the user sent.
//!
//! It lives as long as the browser, not only while it is shown, so downloads go on in the
//! background and the sidebar can count what is waiting.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use iced::{
    Alignment, Element, Length, Task,
    futures::{SinkExt, Stream, StreamExt, channel::mpsc},
    widget::{button, column, container, row, scrollable, space, text},
};
use time::OffsetDateTime;
use tokio::{fs, io::AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use api_client::{ApiError, Client};
use client_core::{
    format::sanitize_component,
    transfers::{time_left, transfer_from, transfer_name, transfer_size, transfer_to},
};
use protocol::{ErrorCode, NodeId, NodeKind, TransferState, TransferSummary, UserId};

use super::{
    parts::{
        COLUMN_GAP, DATE_WIDTH, LIST_PADDING, MENU_WIDTH, ROW_SIZE, SECOND_LINE_SIZE, alert_error,
        alert_info, divider, empty, ghost, header_bar, muted_cell, small_outlined,
        small_outlined_icon, small_primary, title,
    },
    transfer_save,
};
use crate::{
    ellipsis::ellipsized,
    icons::{ICON_SIZE, Icon, Tint},
    locale::short_date,
    theme::{self, Tone},
};

const SPACING: f32 = 12.0;
const SECTION_GAP: f32 = 8.0;
const ROW_HEIGHT: f32 = 56.0;
const NAME_GAP: f32 = 12.0;
const ACTION_GAP: f32 = 8.0;
const PILL_HEIGHT: f32 = 22.0;
const PILL_SIZE: f32 = 12.0;
const PILL_PADDING: [f32; 2] = [0.0, 8.0];
const PILL_GAP: f32 = 6.0;
const STOP_BUTTON: f32 = 32.0;
const CHANNEL_SIZE: usize = 64;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// What the downloads folder is called in the user's Downloads.
const DOWNLOAD_FOLDER: &str = "Home Space";
const DOWNLOADS: &str = "Downloads";
/// Where a download is put together, inside the downloads folder, until it is complete.
const STAGING_SUFFIX: &str = ".part";
/// HTTP status of a route the server does not have.
const NOT_FOUND_STATUS: u16 = 404;
const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "heic", "heif", "webp", "bmp", "tif", "tiff", "svg", "avif",
];
const DOCUMENT_EXTENSIONS: &[&str] = &[
    "pdf", "txt", "md", "rtf", "doc", "docx", "odt", "json", "csv", "xls", "xlsx", "ods",
];

#[derive(Debug, Clone)]
pub enum Message {
    Refresh,
    Loaded(Result<Lists, ApiError>),
    /// A minute passed, so the times left are said again.
    Tick,
    /// Asks where on the server to save a transfer.
    Save(Uuid),
    SaveDialog(transfer_save::Message),
    /// Downloads a transfer into the downloads folder.
    Download(Uuid),
    Progress(Uuid, u64),
    /// A download finished, at the path of what it put at the top.
    Downloaded(Uuid, Result<PathBuf, String>),
    StopDownload(Uuid),
    Decline(Uuid),
    /// The sender takes back a transfer that has not been received.
    Cancel(Uuid),
    /// The server took a decline, a cancel or the confirmation of a download.
    Done(Result<(), ApiError>),
    /// Opens the downloads folder.
    ShowDownloads,
    /// Hides the note about the last download.
    Dismiss,
}

/// The transfers waiting for this device and those the user sent.
#[derive(Debug, Clone, Default)]
pub struct Lists {
    incoming: Vec<TransferSummary>,
    sent: Vec<TransferSummary>,
}

/// A transfer being downloaded.
struct Download {
    done: u64,
    total: u64,
    token: CancellationToken,
}

pub struct TransferList {
    me: UserId,
    lost_and_found: NodeId,
    lists: Lists,
    loading: bool,
    error: Option<String>,
    downloads: HashMap<Uuid, Download>,
    /// What the last download put in the downloads folder.
    downloaded: Option<PathBuf>,
    saving: Option<transfer_save::State>,
    now: OffsetDateTime,
}

impl TransferList {
    pub fn new(me: UserId, lost_and_found: NodeId) -> Self {
        Self {
            me,
            lost_and_found,
            lists: Lists::default(),
            loading: false,
            error: None,
            downloads: HashMap::new(),
            downloaded: None,
            saving: None,
            now: OffsetDateTime::now_utc(),
        }
    }

    pub fn load(&mut self, client: &Client) -> Task<Message> {
        self.loading = true;
        let client = client.clone();
        Task::perform(
            async move {
                let incoming = client.incoming_transfers(true).await?;
                let sent = client.outgoing_transfers().await?;
                Ok(Lists { incoming, sent })
            },
            Message::Loaded,
        )
    }

    /// How many transfers wait for the user to save, download or decline them.
    pub fn waiting(&self) -> usize {
        self.lists
            .incoming
            .iter()
            .filter(|transfer| {
                transfer.state == TransferState::Ready && !self.downloads.contains_key(&transfer.id)
            })
            .count()
    }

    /// Whether the save dialog is open, which Escape leaves first.
    pub fn is_asking(&self) -> bool {
        self.saving.is_some()
    }

    pub fn update(&mut self, client: &Client, message: Message) -> Task<Message> {
        match message {
            Message::Refresh => self.load(client),
            Message::Loaded(result) => {
                self.loading = false;
                self.now = OffsetDateTime::now_utc();
                match result {
                    Ok(lists) => {
                        self.lists = lists;
                        self.error = None;
                    }
                    Err(err) => self.error = Some(load_error(&err)),
                }
                Task::none()
            }
            Message::Tick => {
                self.now = OffsetDateTime::now_utc();
                Task::none()
            }
            Message::Save(id) => {
                let Some(transfer) = self.incoming(id).cloned() else {
                    return Task::none();
                };
                let (saving, task) =
                    transfer_save::State::open(client, transfer, self.lost_and_found);
                self.saving = Some(saving);
                task.map(Message::SaveDialog)
            }
            Message::SaveDialog(message) => {
                let Some(saving) = &mut self.saving else {
                    return Task::none();
                };
                match saving.update(client, message) {
                    transfer_save::Outcome::Task(task) => task.map(Message::SaveDialog),
                    transfer_save::Outcome::Closed => {
                        self.saving = None;
                        Task::none()
                    }
                    transfer_save::Outcome::Saved => {
                        self.saving = None;
                        self.load(client)
                    }
                }
            }
            Message::Download(id) => self.download(client, id),
            Message::Progress(id, done) => {
                if let Some(download) = self.downloads.get_mut(&id) {
                    download.done = done;
                }
                Task::none()
            }
            Message::Downloaded(id, result) => {
                self.downloads.remove(&id);
                match result {
                    Ok(path) => {
                        self.downloaded = Some(path);
                        let client = client.clone();
                        Task::perform(
                            async move { client.confirm_transfer(id).await },
                            Message::Done,
                        )
                    }
                    Err(err) => {
                        self.error = Some(err);
                        Task::none()
                    }
                }
            }
            Message::StopDownload(id) => {
                if let Some(download) = self.downloads.remove(&id) {
                    download.token.cancel();
                }
                Task::none()
            }
            Message::Decline(id) => {
                let client = client.clone();
                Task::perform(
                    async move { client.decline_transfer(id).await },
                    Message::Done,
                )
            }
            Message::Cancel(id) => {
                let client = client.clone();
                Task::perform(
                    async move { client.cancel_transfer(id).await },
                    Message::Done,
                )
            }
            Message::Done(result) => {
                if let Err(err) = result {
                    self.error = Some(err.to_string());
                }
                self.load(client)
            }
            Message::ShowDownloads => {
                if let Some(folder) = downloads_folder()
                    && let Err(err) = crate::transfers::open_file(&folder)
                {
                    self.error = Some(err);
                }
                Task::none()
            }
            Message::Dismiss => {
                self.downloaded = None;
                Task::none()
            }
        }
    }

    fn incoming(&self, id: Uuid) -> Option<&TransferSummary> {
        self.lists
            .incoming
            .iter()
            .find(|transfer| transfer.id == id)
    }

    fn download(&mut self, client: &Client, id: Uuid) -> Task<Message> {
        let Some(transfer) = self.incoming(id) else {
            return Task::none();
        };
        if self.downloads.contains_key(&id) {
            return Task::none();
        }
        let Some(root) = downloads_folder() else {
            self.error = Some("Could not find your Downloads folder.".to_owned());
            return Task::none();
        };
        let token = CancellationToken::new();
        self.downloads.insert(
            id,
            Download {
                done: 0,
                total: transfer.total_bytes,
                token: token.clone(),
            },
        );
        self.downloaded = None;
        self.error = None;
        Task::run(download(client.clone(), id, root, token), |message| message)
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

    /// The save dialog, while it is open.
    pub fn dialog(&self) -> Option<Element<'_, Message>> {
        self.saving
            .as_ref()
            .map(|saving| saving.view().map(Message::SaveDialog))
    }

    pub fn view(&self) -> Element<'_, Message> {
        let mut page = column![].spacing(SPACING);
        if let Some(error) = &self.error {
            page = page.push(alert_error(Icon::Error, error.as_str(), None));
        }
        if let Some(path) = &self.downloaded {
            let actions = row![
                small_outlined("Show", Some(Message::ShowDownloads)),
                button(Icon::Close.view(ICON_SIZE, Tint::Muted))
                    .padding(2)
                    .style(theme::tool)
                    .on_press(Message::Dismiss),
            ]
            .spacing(ACTION_GAP)
            .align_y(Alignment::Center);
            page = page.push(alert_info(
                Icon::Download,
                format!("Downloaded to {}", display_download(path)),
                Some(actions.into()),
            ));
        }
        let incoming = column![
            section("Incoming to this device"),
            table(
                header("From", "State", "Expires"),
                &self.lists.incoming,
                "Nothing is waiting for this device.",
                |transfer| self.incoming_row(transfer),
            ),
        ]
        .spacing(SECTION_GAP);
        let sent = column![
            section("Sent"),
            table(
                header("Items", "State", "Sent"),
                &self.lists.sent,
                "You have not sent anything yet.",
                |transfer| self.sent_row(transfer),
            ),
        ]
        .spacing(SECTION_GAP);
        scrollable(page.push(incoming).push(sent))
            .height(Length::Fill)
            .into()
    }

    fn incoming_row<'a>(&'a self, transfer: &'a TransferSummary) -> Element<'a, Message> {
        let id = transfer.id;
        let name = name_cell(
            transfer,
            format!("{} · {}", transfer_name(transfer), transfer_size(transfer)),
            transfer_from(transfer, self.me),
        );
        let (state, stop): (Element<'a, Message>, Option<Message>) =
            match (self.downloads.get(&id), transfer.state) {
                (Some(download), _) => (
                    state_cell(
                        "Downloading",
                        Tone::Accent,
                        Some(format!(
                            "{}% · to {}",
                            percent(download.done, download.total),
                            display_folder()
                        )),
                    ),
                    Some(Message::StopDownload(id)),
                ),
                (None, TransferState::Ready) => (
                    row![
                        small_outlined_icon(Icon::SaveToServer, "Save", Some(Message::Save(id))),
                        small_primary(Icon::Download, "Download", Some(Message::Download(id))),
                        small_outlined("Decline", Some(Message::Decline(id))),
                    ]
                    .spacing(ACTION_GAP)
                    .align_y(Alignment::Center)
                    .width(Length::FillPortion(1))
                    .into(),
                    None,
                ),
                (None, state) => {
                    let (label, tone, note) = state_words(state, transfer, self.now);
                    (state_cell(label, tone, note), None)
                }
            };
        row_of(
            name,
            state,
            muted_cell(
                time_left(transfer.expires_at, self.now),
                DATE_WIDTH,
                Alignment::Start,
            ),
            stop,
        )
    }

    fn sent_row<'a>(&'a self, transfer: &'a TransferSummary) -> Element<'a, Message> {
        let name = name_cell(
            transfer,
            transfer_name(transfer),
            format!(
                "{} · {}",
                transfer_to(transfer, self.me),
                transfer_size(transfer)
            ),
        );
        let (label, tone, note) = state_words(transfer.state, transfer, self.now);
        let open = matches!(
            transfer.state,
            TransferState::Uploading | TransferState::Ready
        );
        row_of(
            name,
            state_cell(label, tone, note),
            muted_cell(
                short_date(transfer.created_at),
                DATE_WIDTH,
                Alignment::Start,
            ),
            open.then_some(Message::Cancel(transfer.id)),
        )
    }
}

/// Where downloads go: a folder of its own in the user's Downloads.
fn downloads_folder() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(DOWNLOADS).join(DOWNLOAD_FOLDER))
}

/// The downloads folder as the user knows it.
fn display_folder() -> String {
    format!("{DOWNLOADS}/{DOWNLOAD_FOLDER}")
}

/// Something the last download put in the downloads folder, as the user knows it.
fn display_download(path: &Path) -> String {
    match path.file_name() {
        Some(name) => format!("{}/{}", display_folder(), name.to_string_lossy()),
        None => display_folder(),
    }
}

fn percent(done: u64, total: u64) -> u64 {
    match total {
        0 => 0,
        total => (done.min(total) * 100) / total,
    }
}

/// What a state is called, its colours and what is worth saying next to it.
fn state_words(
    state: TransferState,
    transfer: &TransferSummary,
    now: OffsetDateTime,
) -> (&'static str, Tone, Option<String>) {
    match state {
        TransferState::Uploading => ("Uploading", Tone::Accent, None),
        TransferState::Ready => (
            "Waiting",
            Tone::Muted,
            Some(format!("Expires {}", time_left(transfer.expires_at, now))),
        ),
        TransferState::Declined => ("Declined", Tone::Danger, None),
        TransferState::Downloaded => ("Downloaded", Tone::Success, None),
        TransferState::Saved => ("Saved", Tone::Success, None),
        TransferState::Cancelled => ("Cancelled", Tone::Muted, None),
        TransferState::Expired => (
            "Expired",
            Tone::Muted,
            Some("Not received in time".to_owned()),
        ),
    }
}

/// The icon of what a transfer holds: a file's kind, or a folder for anything more.
fn transfer_icon(transfer: &TransferSummary) -> (Icon, Tint) {
    let Some(name) = transfer
        .name
        .as_deref()
        .filter(|_| transfer.item_count == 1)
    else {
        return (Icon::Folder, Tint::Accent);
    };
    let extension = Path::new(name)
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let icon = if IMAGE_EXTENSIONS.contains(&extension.as_str()) {
        Icon::Image
    } else if DOCUMENT_EXTENSIONS.contains(&extension.as_str()) {
        Icon::Document
    } else {
        Icon::Other
    };
    (icon, Tint::Text)
}

fn section<'a>(label: &'a str) -> Element<'a, Message> {
    text(label).font(theme::SEMIBOLD).into()
}

fn header<'a>(name: &str, state: &str, time: &str) -> Element<'a, Message> {
    header_bar(
        row![
            title(name, Length::FillPortion(1), Alignment::Start),
            title(state, Length::FillPortion(1), Alignment::Start),
            title(time, DATE_WIDTH, Alignment::Start),
            space().width(MENU_WIDTH),
        ],
        LIST_PADDING,
    )
}

/// A box with `header` over a row for each of `transfers`, or `hint` when there are none.
fn table<'a>(
    header: Element<'a, Message>,
    transfers: &'a [TransferSummary],
    hint: &'a str,
    row: impl Fn(&'a TransferSummary) -> Element<'a, Message>,
) -> Element<'a, Message> {
    let rows: Element<'a, Message> = if transfers.is_empty() {
        empty(hint)
    } else {
        column(transfers.iter().enumerate().flat_map(|(index, transfer)| {
            (index > 0).then(divider).into_iter().chain([row(transfer)])
        }))
        .into()
    };
    container(column![header, rows])
        .padding(1)
        .width(Length::Fill)
        .style(theme::card)
        .into()
}

fn name_cell<'a>(
    transfer: &TransferSummary,
    first: String,
    second: String,
) -> Element<'a, Message> {
    let (icon, tint) = transfer_icon(transfer);
    row![
        icon.view(ICON_SIZE, tint),
        column![
            ellipsized(first, ROW_SIZE, Alignment::Start),
            container(ellipsized(second, SECOND_LINE_SIZE, Alignment::Start)).style(theme::muted),
        ]
        .width(Length::Fill)
    ]
    .spacing(NAME_GAP)
    .align_y(Alignment::Center)
    .width(Length::FillPortion(1))
    .into()
}

/// The state in a pill, with `note` next to it.
fn state_cell<'a>(label: &'a str, tone: Tone, note: Option<String>) -> Element<'a, Message> {
    let pill = container(text(label).size(PILL_SIZE).font(theme::SEMIBOLD))
        .padding(PILL_PADDING)
        .center_y(PILL_HEIGHT)
        .style(theme::tone_pill(tone));
    row![pill]
        .push(note.map(|note| {
            container(ellipsized(note, SECOND_LINE_SIZE, Alignment::Start))
                .style(theme::muted)
                .width(Length::Fill)
        }))
        .spacing(PILL_GAP)
        .align_y(Alignment::Center)
        .width(Length::FillPortion(1))
        .into()
}

/// A row of a table, with an × at its end when `stop` is something to do.
fn row_of<'a>(
    name: Element<'a, Message>,
    state: Element<'a, Message>,
    time: Element<'a, Message>,
    stop: Option<Message>,
) -> Element<'a, Message> {
    let stop: Element<'a, Message> = match stop {
        Some(message) => container(
            button(container(Icon::Close.view(ICON_SIZE, Tint::Muted)).center(STOP_BUTTON))
                .padding(0)
                .style(theme::tool)
                .on_press(message),
        )
        .width(MENU_WIDTH)
        .into(),
        None => space().width(MENU_WIDTH).into(),
    };
    container(
        row![name, state, time, stop]
            .spacing(COLUMN_GAP)
            .align_y(Alignment::Center),
    )
    .padding(LIST_PADDING)
    .center_y(ROW_HEIGHT)
    .width(Length::Fill)
    .into()
}

/// Downloads transfer `id` into `root`, reporting progress, until it is done or `token` is
/// cancelled. Nothing is left behind unless it completes.
fn download(
    client: Client,
    id: Uuid,
    root: PathBuf,
    token: CancellationToken,
) -> impl Stream<Item = Message> {
    iced::stream::channel(
        CHANNEL_SIZE,
        async move |mut output: mpsc::Sender<Message>| {
            let staging = root.join(format!(".{id}{STAGING_SUFFIX}"));
            let fetched = token
                .run_until_cancelled(fetch(&client, id, &root, &staging, output.clone()))
                .await;
            let _ = fs::remove_dir_all(&staging).await;
            if let Some(result) = fetched {
                let _ = output.send(Message::Downloaded(id, result)).await;
            }
        },
    )
}

/// Fetches every file of transfer `id` into `staging`, checks it, and then moves what is at
/// the top into `root` beside what is already there. Answers where the first of those went.
async fn fetch(
    client: &Client,
    id: Uuid,
    root: &Path,
    staging: &Path,
    mut output: mpsc::Sender<Message>,
) -> Result<PathBuf, String> {
    let transfer = client.transfer(id).await.map_err(|err| err.to_string())?;
    let _ = fs::remove_dir_all(staging).await;
    fs::create_dir_all(staging)
        .await
        .map_err(io_error(staging))?;
    let mut done = 0;
    let mut reported: Option<Instant> = None;
    let mut tops: Vec<String> = Vec::new();
    for item in &transfer.items {
        let components = components(&item.path);
        let Some(top) = components.first() else {
            continue;
        };
        if !tops.contains(top) {
            tops.push(top.clone());
        }
        let path = components
            .iter()
            .fold(staging.to_path_buf(), |path, component| {
                path.join(component)
            });
        match item.kind {
            NodeKind::Folder => fs::create_dir_all(&path).await.map_err(io_error(&path))?,
            NodeKind::File => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).await.map_err(io_error(parent))?;
                }
                let response = client
                    .download_transfer_item(id, item.id)
                    .await
                    .map_err(|err| err.to_string())?;
                let mut file = fs::File::create(&path).await.map_err(io_error(&path))?;
                let mut body = response.bytes_stream();
                while let Some(chunk) = body.next().await {
                    let chunk = chunk.map_err(|err| err.to_string())?;
                    file.write_all(&chunk).await.map_err(io_error(&path))?;
                    done += chunk.len() as u64;
                    let now = Instant::now();
                    if reported.is_none_or(|at| now.duration_since(at) >= PROGRESS_INTERVAL) {
                        reported = Some(now);
                        let _ = output.try_send(Message::Progress(id, done));
                    }
                }
                file.flush().await.map_err(io_error(&path))?;
                drop(file);
                if let Some(expected) = &item.sha256 {
                    check(&path, expected, &item.path).await?;
                }
            }
        }
    }
    let _ = output.send(Message::Progress(id, done)).await;
    let mut first = None;
    for top in tops {
        let target = free_path(root, &top).await;
        let from = staging.join(&top);
        fs::rename(&from, &target)
            .await
            .map_err(io_error(&target))?;
        first.get_or_insert(target);
    }
    Ok(first.unwrap_or_else(|| root.to_path_buf()))
}

/// The folders and name of a transfer path, each made safe for the file system.
fn components(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|component| !component.is_empty())
        .map(sanitize_component)
        .collect()
}

/// Whether the file at `path` hashes to `expected`.
async fn check(path: &Path, expected: &str, shown: &str) -> Result<(), String> {
    let file = path.to_path_buf();
    let actual =
        tokio::task::spawn_blocking(move || sync_core::hash_file(&file, &AtomicBool::new(false)))
            .await
            .map_err(|err| err.to_string())?
            .map_err(io_error(path))?;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!("{shown} arrived damaged; download it again."))
    }
}

/// `root/name`, or `root/name (2).ext` and so on when that is taken.
async fn free_path(root: &Path, name: &str) -> PathBuf {
    let first = root.join(name);
    if !fs::try_exists(&first).await.unwrap_or(false) {
        return first;
    }
    let path = Path::new(name);
    let (stem, extension) = match (path.file_stem(), path.extension()) {
        (Some(stem), Some(extension)) if !stem.is_empty() => (
            stem.to_string_lossy().into_owned(),
            format!(".{}", extension.to_string_lossy()),
        ),
        _ => (name.to_owned(), String::new()),
    };
    for number in 2.. {
        let candidate = root.join(format!("{stem} ({number}){extension}"));
        if !fs::try_exists(&candidate).await.unwrap_or(false) {
            return candidate;
        }
    }
    unreachable!("the numbers ran out")
}

/// What to say when the lists could not be loaded; a server from before transfers has no such routes.
fn load_error(err: &ApiError) -> String {
    let missing = match err {
        ApiError::Api { code, .. } => *code == ErrorCode::NotFound,
        ApiError::Status { status, .. } => *status == NOT_FOUND_STATUS,
        _ => false,
    };
    if missing {
        "This server does not support transfers yet. Update it to send files between devices."
            .to_owned()
    } else {
        err.to_string()
    }
}

fn io_error(path: &Path) -> impl Fn(io::Error) -> String + '_ {
    move |err| format!("{}: {err}", path.display())
}

#[cfg(test)]
mod tests {
    use iced_test::simulator;
    use protocol::UserRef;
    use time::Duration as TimeDuration;

    use super::*;

    const ME: u128 = 1;
    const DANA: u128 = 2;

    fn user(raw: u128, name: &str) -> UserRef {
        UserRef {
            id: UserId(Uuid::from_u128(raw)),
            username: name.to_owned(),
        }
    }

    fn summary(raw: u128, state: TransferState, name: Option<&str>, items: u32) -> TransferSummary {
        let now = OffsetDateTime::now_utc();
        TransferSummary {
            id: Uuid::from_u128(raw),
            sender: user(DANA, "dana"),
            sender_device: None,
            sender_device_name: Some("MacBook".to_owned()),
            recipient: user(ME, "sam"),
            recipient_device: Uuid::nil(),
            recipient_device_name: "Laptop".to_owned(),
            state,
            total_bytes: 3 * 1024 * 1024,
            item_count: items,
            file_count: items,
            name: name.map(str::to_owned),
            message: None,
            created_at: now,
            expires_at: now + TimeDuration::hours(5) + TimeDuration::minutes(1),
        }
    }

    fn client() -> Client {
        Client::new("http://localhost")
    }

    fn loaded() -> TransferList {
        let mut list = TransferList::new(UserId(Uuid::from_u128(ME)), NodeId(Uuid::nil()));
        let lists = Lists {
            incoming: vec![
                summary(1, TransferState::Ready, None, 3),
                summary(2, TransferState::Declined, Some("IMG_2041.heic"), 1),
            ],
            sent: vec![TransferSummary {
                sender: user(ME, "sam"),
                recipient: user(DANA, "dana"),
                recipient_device_name: "Phone".to_owned(),
                ..summary(3, TransferState::Downloaded, Some("report.pdf"), 1)
            }],
        };
        let _ = list.update(&client(), Message::Loaded(Ok(lists)));
        list
    }

    #[test]
    fn incoming_transfers_offer_what_can_be_done_with_them() {
        let list = loaded();
        let mut ui = simulator(list.view());

        assert!(ui.find("3 files · 3.0 MB").is_ok());
        assert!(ui.find("From dana · MacBook").is_ok());
        assert!(ui.find("Save").is_ok());
        assert!(ui.find("Download").is_ok());
        assert!(ui.find("Decline").is_ok());
        assert!(ui.find("in 5 h").is_ok());
        assert!(ui.find("Declined").is_ok());
    }

    #[test]
    fn sent_transfers_say_where_they_went_and_how_it_went() {
        let list = loaded();
        let mut ui = simulator(list.view());

        assert!(ui.find("report.pdf").is_ok());
        assert!(ui.find("To dana · Phone · 3.0 MB").is_ok());
        assert!(ui.find("Downloaded").is_ok());
    }

    #[test]
    fn only_ready_transfers_count_as_waiting() {
        let mut list = loaded();
        assert_eq!(list.waiting(), 1);

        list.downloads.insert(
            Uuid::from_u128(1),
            Download {
                done: 0,
                total: 1,
                token: CancellationToken::new(),
            },
        );
        assert_eq!(list.waiting(), 0);
    }

    #[test]
    fn a_download_in_progress_shows_how_far_it_got() {
        let mut list = loaded();
        list.downloads.insert(
            Uuid::from_u128(1),
            Download {
                done: 62,
                total: 100,
                token: CancellationToken::new(),
            },
        );
        let mut ui = simulator(list.view());

        assert!(ui.find("Downloading").is_ok());
        assert!(ui.find("62% · to Downloads/Home Space").is_ok());
    }

    #[test]
    fn stopping_a_download_cancels_it() {
        let mut list = loaded();
        let token = CancellationToken::new();
        list.downloads.insert(
            Uuid::from_u128(1),
            Download {
                done: 0,
                total: 1,
                token: token.clone(),
            },
        );

        let _ = list.update(&client(), Message::StopDownload(Uuid::from_u128(1)));

        assert!(token.is_cancelled());
        assert!(list.downloads.is_empty());
    }

    #[test]
    fn a_finished_download_is_confirmed_to_the_server() {
        let mut list = loaded();

        let task = list.update(
            &client(),
            Message::Downloaded(Uuid::from_u128(1), Ok(PathBuf::from("/d/Home Space/a.pdf"))),
        );

        assert!(task.units() > 0);
        assert!(list.downloaded.is_some());
    }

    #[test]
    fn a_transfer_shows_the_icon_of_what_it_holds() {
        let photo = summary(1, TransferState::Ready, Some("IMG.HEIC"), 1);
        let folder = summary(1, TransferState::Ready, Some("Trip"), 5);
        assert_eq!(transfer_icon(&photo).0, Icon::Image);
        assert_eq!(transfer_icon(&folder).0, Icon::Folder);
    }

    #[test]
    fn transfer_paths_cannot_climb_out_of_the_download() {
        assert_eq!(components("Trip/day 1/a.jpg"), ["Trip", "day 1", "a.jpg"]);
        assert!(components("../a").iter().all(|component| component != ".."));
    }

    #[tokio::test]
    async fn a_taken_name_gets_a_number() {
        let root = std::env::temp_dir().join(format!("home-space-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).await.unwrap();
        fs::write(root.join("a.pdf"), b"x").await.unwrap();

        assert_eq!(free_path(&root, "b.pdf").await, root.join("b.pdf"));
        assert_eq!(free_path(&root, "a.pdf").await, root.join("a (2).pdf"));

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[test]
    fn a_server_without_transfers_is_named() {
        let missing = ApiError::Api {
            code: ErrorCode::NotFound,
            message: "not found".to_owned(),
        };
        assert!(load_error(&missing).contains("does not support transfers"));
        let other = ApiError::Transport("refused".to_owned());
        assert_eq!(load_error(&other), other.to_string());
    }
}
