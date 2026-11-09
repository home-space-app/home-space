use std::{
    collections::{HashMap, VecDeque},
    convert::identity,
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use iced::{
    Alignment, Element, Length, Padding, Task,
    futures::{SinkExt, Stream, StreamExt, channel::mpsc, stream},
    widget::{
        button, column, container, progress_bar, row, scrollable, space, text, text::Wrapping,
        tooltip,
    },
};
use tokio::{fs, io::AsyncWriteExt};
use tokio_util::{io::ReaderStream, sync::CancellationToken};

use api_client::{ApiError, Client};
use client_core::format::sanitize_component;
use client_core::listing::{Sort, sort_nodes};
use protocol::{CreateUpload, ErrorCode, Node, NodeId, NodeKind};

use crate::{
    ellipsis::ellipsized,
    icons::{ICON_SIZE, Icon, Tint},
    sync::{part_path, read_range},
    theme,
};

const CHANNEL_SIZE: usize = 64;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const MAX_DEPTH: usize = 64;
const TOO_DEEP: &str = "folder tree is too deep";
const ALREADY_EXISTS: &str = "already exists";
const MISSING_FOLDER: &str = "parent folder could not be created";
const PANEL_WIDTH: f32 = 380.0;
const LIST_HEIGHT: f32 = 320.0;
const HEADER_PADDING: Padding = Padding {
    top: 8.0,
    right: 8.0,
    bottom: 8.0,
    left: 16.0,
};
const HEADER_GAP: f32 = 4.0;
const ICON_BUTTON_PADDING: f32 = 6.0;
const OVERALL_PADDING: Padding = Padding {
    top: 0.0,
    right: 16.0,
    bottom: 14.0,
    left: 16.0,
};
const ITEM_PADDING: [f32; 2] = [10.0, 16.0];
const ITEM_GAP: f32 = 6.0;
const LINE_GAP: f32 = 10.0;
const DIVIDER: f32 = 1.0;
const NAME_SIZE: f32 = 14.0;
const STATE_SIZE: f32 = 12.0;
const BAR_HEIGHT: f32 = 6.0;
const TIP_SIZE: f32 = 12.0;
const TIP_PADDING: [f32; 2] = [4.0, 8.0];
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "heic", "bmp", "svg"];
const DOCUMENT_EXTENSIONS: &[&str] = &["pdf", "txt", "md", "rtf", "doc", "docx", "odt", "json"];

type TransferId = u64;

/// Which way bytes flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TransferState {
    Queued,
    Active,
    Done,
    Failed(String),
    /// Held by Pause; Continue queues it again and it starts over.
    Paused,
    /// Dropped by Stop.
    Cancelled,
}

#[derive(Debug, Clone)]
struct Transfer {
    id: TransferId,
    name: String,
    direction: Direction,
    done: u64,
    total: u64,
    state: TransferState,
}

impl Transfer {
    fn is_pending(&self) -> bool {
        matches!(self.state, TransferState::Queued | TransferState::Active)
    }

    /// Whether the transfer is over, one way or another.
    fn is_finished(&self) -> bool {
        matches!(self.state, TransferState::Done | TransferState::Cancelled)
    }

    fn fraction(&self) -> f32 {
        match (&self.state, self.total) {
            (TransferState::Done, _) => 1.0,
            (_, 0) => 0.0,
            (_, total) => (self.done as f64 / total as f64).min(1.0) as f32,
        }
    }
}

/// A local file to upload, with the folder path it goes into relative to the target.
#[derive(Debug, Clone)]
pub struct UploadFile {
    path: PathBuf,
    folder: Vec<String>,
    size: u64,
}

/// Folders to create (parents first) and files to upload.
#[derive(Debug, Clone, Default)]
pub struct UploadPlan {
    folders: Vec<Vec<String>>,
    files: Vec<UploadFile>,
}

impl UploadPlan {
    /// Whether the plan has nothing to do.
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty() && self.files.is_empty()
    }
}

/// What to do when a download target already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Existing {
    Replace,
    Fail,
}

/// What happens to a file once it is downloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Then {
    Keep,
    /// Opens it with the system's default app.
    Open,
}

/// A remote file and where to save it.
#[derive(Debug, Clone)]
pub struct DownloadFile {
    id: NodeId,
    name: String,
    target: PathBuf,
    size: u64,
    existing: Existing,
    then: Then,
}

impl DownloadFile {
    /// Downloads `node` to `target`, replacing it; the user already confirmed the overwrite.
    pub fn replacing(node: &Node, target: PathBuf) -> Self {
        Self {
            id: node.id,
            name: node.name.clone(),
            target,
            size: byte_size(node.size),
            existing: Existing::Replace,
            then: Then::Keep,
        }
    }

    /// Downloads `node` to `target`, replacing it, and then opens it with the default app.
    pub fn opening(node: &Node, target: PathBuf) -> Self {
        Self {
            then: Then::Open,
            ..Self::replacing(node, target)
        }
    }
}

/// A unit of work run as one sequential task.
#[derive(Debug, Clone)]
pub enum Batch {
    Upload {
        parent: Option<NodeId>,
        plan: UploadPlan,
    },
    Download(Vec<DownloadFile>),
}

impl Batch {
    fn direction(&self) -> Direction {
        match self {
            Self::Upload { .. } => Direction::Upload,
            Self::Download(_) => Direction::Download,
        }
    }

    /// What is left of the batch once only the transfers in `ids` that `keep` accepts remain;
    /// `None` when nothing is left. An upload keeps all its folders, which are created again
    /// without harm.
    fn remaining(
        self,
        ids: &[TransferId],
        keep: impl Fn(TransferId) -> bool,
    ) -> Option<(Self, Vec<TransferId>)> {
        let had_files = !ids.is_empty();
        match self {
            Self::Upload { parent, plan } => {
                let (files, kept): (Vec<UploadFile>, Vec<TransferId>) = plan
                    .files
                    .into_iter()
                    .zip(ids.iter().copied())
                    .filter(|(_, id)| keep(*id))
                    .unzip();
                (!files.is_empty() || !had_files).then(|| {
                    let plan = UploadPlan {
                        folders: plan.folders,
                        files,
                    };
                    (Self::Upload { parent, plan }, kept)
                })
            }
            Self::Download(files) => {
                let (files, kept): (Vec<DownloadFile>, Vec<TransferId>) = files
                    .into_iter()
                    .zip(ids.iter().copied())
                    .filter(|(_, id)| keep(*id))
                    .unzip();
                (!files.is_empty()).then_some((Self::Download(files), kept))
            }
        }
    }

    fn entries(&self) -> Vec<(String, u64)> {
        match self {
            Self::Upload { plan, .. } => plan
                .files
                .iter()
                .map(|file| (display_name(&file.path), file.size))
                .collect(),
            Self::Download(files) => files
                .iter()
                .map(|file| (file.name.clone(), file.size))
                .collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Started(TransferId),
    Progress(TransferId, u64),
    Finished(TransferId, Result<(), String>),
    BatchDone(Direction),
    ToggleExpanded,
    ClearCompleted,
    Close,
    /// Holds every unfinished transfer; the one under way starts over on Continue.
    Pause,
    /// Continues after Pause.
    Resume,
    /// Cancels every unfinished transfer.
    Stop,
}

/// The batch under way, kept so that Pause can queue what is left of it again.
#[derive(Debug)]
struct Running {
    batch: Batch,
    ids: Vec<TransferId>,
    token: CancellationToken,
}

/// The transfer queue: one batch runs at a time, the rest wait.
#[derive(Debug, Default)]
pub struct Transfers {
    items: Vec<Transfer>,
    pending: VecDeque<(Batch, Vec<TransferId>)>,
    current: Option<Running>,
    paused: bool,
    next_id: TransferId,
    collapsed: bool,
}

impl Transfers {
    /// Queues `batch` and starts it when nothing else runs.
    pub fn enqueue(&mut self, client: &Client, batch: Batch) -> Task<Message> {
        let direction = batch.direction();
        let ids: Vec<TransferId> = batch
            .entries()
            .into_iter()
            .map(|(name, total)| {
                let id = self.next_id;
                self.next_id += 1;
                self.items.push(Transfer {
                    id,
                    name,
                    direction,
                    done: 0,
                    total,
                    state: TransferState::Queued,
                });
                id
            })
            .collect();
        self.pending.push_back((batch, ids));
        self.start_next(client)
    }

    /// Applies a progress or panel message.
    pub fn update(&mut self, client: &Client, message: Message) -> Task<Message> {
        match message {
            Message::Started(id) => {
                self.change(id, |item| {
                    if item.state == TransferState::Queued {
                        item.state = TransferState::Active;
                    }
                });
                Task::none()
            }
            Message::Progress(id, done) => {
                self.change(id, |item| {
                    if item.state == TransferState::Active {
                        item.done = done;
                    }
                });
                Task::none()
            }
            Message::Finished(id, result) => {
                self.change(id, |item| match result {
                    Ok(()) => {
                        item.done = item.total;
                        item.state = TransferState::Done;
                    }
                    Err(message) => item.state = TransferState::Failed(message),
                });
                Task::none()
            }
            Message::BatchDone(_) => {
                if let Some(running) = self.current.take()
                    && running.token.is_cancelled()
                    && self.paused
                {
                    let rest = running.batch.remaining(&running.ids, |id| {
                        self.items
                            .iter()
                            .any(|item| item.id == id && item.state == TransferState::Paused)
                    });
                    if let Some(rest) = rest {
                        self.pending.push_front(rest);
                    }
                }
                self.start_next(client)
            }
            Message::Pause => {
                self.cancel_current();
                self.paused = true;
                self.set_states(Transfer::is_pending, TransferState::Paused);
                Task::none()
            }
            Message::Resume => {
                self.paused = false;
                for item in &mut self.items {
                    if item.state == TransferState::Paused {
                        item.state = TransferState::Queued;
                        item.done = 0;
                    }
                }
                self.start_next(client)
            }
            Message::Stop => {
                self.cancel_current();
                self.paused = false;
                self.pending.clear();
                self.set_states(
                    |item| item.is_pending() || item.state == TransferState::Paused,
                    TransferState::Cancelled,
                );
                Task::none()
            }
            Message::ToggleExpanded => {
                self.collapsed = !self.collapsed;
                Task::none()
            }
            Message::ClearCompleted => {
                self.items.retain(|item| !item.is_finished());
                Task::none()
            }
            Message::Close => {
                if !self.is_busy() {
                    self.items.clear();
                    self.pending.clear();
                    self.paused = false;
                }
                Task::none()
            }
        }
    }

    /// Whether the panel has anything to show.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn is_busy(&self) -> bool {
        self.current.is_some() || self.items.iter().any(Transfer::is_pending)
    }

    /// Whether Pause holds transfers that Continue would pick up again.
    fn is_paused(&self) -> bool {
        self.paused
            && self
                .items
                .iter()
                .any(|item| item.state == TransferState::Paused)
    }

    fn cancel_current(&self) {
        if let Some(running) = &self.current {
            running.token.cancel();
        }
    }

    fn set_states(&mut self, which: impl Fn(&Transfer) -> bool, state: TransferState) {
        for item in &mut self.items {
            if which(item) {
                item.state = state.clone();
            }
        }
    }

    fn change(&mut self, id: TransferId, change: impl FnOnce(&mut Transfer)) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            change(item);
        }
    }

    fn start_next(&mut self, client: &Client) -> Task<Message> {
        if self.current.is_some() || self.paused {
            Task::none()
        } else if let Some((batch, ids)) = self.pending.pop_front() {
            let token = CancellationToken::new();
            self.current = Some(Running {
                batch: batch.clone(),
                ids: ids.clone(),
                token: token.clone(),
            });
            Task::run(run_batch(client.clone(), batch, ids, token), identity)
        } else {
            Task::none()
        }
    }

    /// The floating progress panel.
    pub fn view(&self) -> Element<'_, Message> {
        let has_done = self.items.iter().any(Transfer::is_finished);
        let controls: Option<Element<'_, Message>> = if self.is_paused() {
            Some(
                row![
                    control(Icon::Resume, "Continue", Message::Resume),
                    control(Icon::Stop, "Cancel", Message::Stop),
                ]
                .spacing(HEADER_GAP)
                .into(),
            )
        } else if self.items.iter().any(Transfer::is_pending) {
            Some(
                row![
                    control(Icon::Pause, "Pause", Message::Pause),
                    control(Icon::Stop, "Stop", Message::Stop),
                ]
                .spacing(HEADER_GAP)
                .into(),
            )
        } else {
            None
        };
        let fold = if self.collapsed {
            Icon::ExpandLess
        } else {
            Icon::ExpandMore
        };
        let close = (!self.is_busy()).then(|| {
            button(Icon::Close.view(ICON_SIZE, Tint::Muted))
                .padding(ICON_BUTTON_PADDING)
                .style(theme::tool)
                .on_press(Message::Close)
        });
        let header = container(
            row![
                // One line, clipped rather than wrapped, so the header keeps its height.
                container(
                    text(summarize(&self.items, self.is_paused()))
                        .font(theme::MEDIUM)
                        .wrapping(Wrapping::None),
                )
                .width(Length::Fill)
                .clip(true),
            ]
            .push(
                has_done
                    .then(|| control(Icon::ClearAll, "Clear completed", Message::ClearCompleted)),
            )
            .push(controls)
            .push(
                button(fold.view(ICON_SIZE, Tint::Muted))
                    .padding(ICON_BUTTON_PADDING)
                    .style(theme::tool)
                    .on_press(Message::ToggleExpanded),
            )
            .push(close)
            .spacing(HEADER_GAP)
            .align_y(Alignment::Center),
        )
        .padding(HEADER_PADDING);
        let body: Element<'_, Message> = if self.collapsed {
            container(
                progress_bar(0.0..=1.0, overall_progress(&self.items))
                    .girth(BAR_HEIGHT)
                    .style(theme::progress),
            )
            .padding(OVERALL_PADDING)
            .into()
        } else {
            let rows = self.items.iter().flat_map(|item| {
                [
                    container(space())
                        .width(Length::Fill)
                        .height(DIVIDER)
                        .style(theme::divider)
                        .into(),
                    item_view(item),
                ]
            });
            container(scrollable(column(rows).width(Length::Fill)).width(Length::Fill))
                .max_height(LIST_HEIGHT)
                .into()
        };
        container(column![header, body])
            .width(PANEL_WIDTH)
            .style(theme::panel)
            .into()
    }
}

/// An icon button of the header, named in a tooltip.
fn control(icon: Icon, label: &'static str, message: Message) -> Element<'static, Message> {
    tooltip(
        button(icon.view(ICON_SIZE, Tint::Muted))
            .padding(ICON_BUTTON_PADDING)
            .style(theme::tool)
            .on_press(message),
        container(text(label).size(TIP_SIZE))
            .padding(TIP_PADDING)
            .style(theme::menu),
        tooltip::Position::Top,
    )
    .into()
}

fn item_view(item: &Transfer) -> Element<'_, Message> {
    let (icon, tint) = match item.state {
        TransferState::Failed(_) => (Icon::Error, Tint::Danger),
        _ => (file_icon(&item.name), Tint::Text),
    };
    let status: Option<Element<'_, Message>> = match item.state {
        TransferState::Queued => Some(
            text("Waiting")
                .size(STATE_SIZE)
                .style(theme::muted_text)
                .into(),
        ),
        TransferState::Active => Some(
            text(format!("{:.0}%", item.fraction() * 100.0))
                .size(STATE_SIZE)
                .style(theme::muted_text)
                .into(),
        ),
        TransferState::Done => Some(
            text("Done")
                .size(STATE_SIZE)
                .style(theme::success_text)
                .into(),
        ),
        TransferState::Paused => Some(
            text("Paused")
                .size(STATE_SIZE)
                .style(theme::muted_text)
                .into(),
        ),
        TransferState::Cancelled => Some(
            text("Cancelled")
                .size(STATE_SIZE)
                .style(theme::muted_text)
                .into(),
        ),
        TransferState::Failed(_) => None,
    };
    let mut lines = column![
        row![
            icon.view(ICON_SIZE, tint),
            ellipsized(item.name.as_str(), NAME_SIZE, Alignment::Start).width(Length::Fill),
        ]
        .push(status)
        .spacing(LINE_GAP)
        .align_y(Alignment::Center)
    ]
    .spacing(ITEM_GAP);
    match &item.state {
        TransferState::Active => {
            lines = lines.push(
                progress_bar(0.0..=1.0, item.fraction())
                    .girth(BAR_HEIGHT)
                    .style(theme::progress),
            );
        }
        TransferState::Failed(message) => {
            lines = lines.push(
                text(format!("Failed: {message}"))
                    .size(STATE_SIZE)
                    .style(theme::danger_text),
            );
        }
        TransferState::Queued
        | TransferState::Done
        | TransferState::Paused
        | TransferState::Cancelled => {}
    }
    container(lines).padding(ITEM_PADDING).into()
}

/// The icon of a file known only by its name.
fn file_icon(name: &str) -> Icon {
    let extension = Path::new(name)
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase());
    match extension.as_deref() {
        Some(extension) if IMAGE_EXTENSIONS.contains(&extension) => Icon::Image,
        Some(extension) if DOCUMENT_EXTENSIONS.contains(&extension) => Icon::Document,
        _ => Icon::Other,
    }
}

fn summarize(items: &[Transfer], paused: bool) -> String {
    let total = items.len();
    let count =
        |state: fn(&TransferState) -> bool| items.iter().filter(|item| state(&item.state)).count();
    let failed = count(|state| matches!(state, TransferState::Failed(_)));
    let cancelled = count(|state| *state == TransferState::Cancelled);
    let held = count(|state| *state == TransferState::Paused);
    let pending: Vec<&Transfer> = items.iter().filter(|item| item.is_pending()).collect();
    match pending.first() {
        _ if paused => format!("Paused · {} of {total} left", held + pending.len()),
        Some(next) => {
            let verb = match next.direction {
                Direction::Upload => "Uploading",
                Direction::Download => "Downloading",
            };
            format!("{verb} {} of {total}", total - pending.len() + 1)
        }
        None => {
            let completed = format!("{} completed", total - failed - cancelled);
            [
                (failed > 0).then(|| format!("{failed} failed")),
                Some(completed),
                (cancelled > 0).then(|| format!("{cancelled} cancelled")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ")
        }
    }
}

fn overall_progress(items: &[Transfer]) -> f32 {
    if items.is_empty() {
        0.0
    } else {
        items.iter().map(Transfer::fraction).sum::<f32>() / items.len() as f32
    }
}

fn byte_size(size: i64) -> u64 {
    u64::try_from(size).unwrap_or_default()
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Walks local files and folders into an upload plan, parents before children.
pub async fn plan_upload(paths: Vec<PathBuf>) -> Result<UploadPlan, String> {
    walk_local(paths).await.map_err(|err| err.to_string())
}

async fn walk_local(paths: Vec<PathBuf>) -> io::Result<UploadPlan> {
    let mut plan = UploadPlan::default();
    let mut stack: Vec<(PathBuf, Vec<String>, usize)> = paths
        .into_iter()
        .rev()
        .map(|path| (path, Vec::new(), 0))
        .collect();
    while let Some((path, folder, depth)) = stack.pop() {
        let metadata = fs::metadata(&path).await?;
        if metadata.is_dir() {
            if depth >= MAX_DEPTH {
                return Err(io::Error::other(TOO_DEEP));
            }
            let mut inner = folder;
            inner.push(display_name(&path));
            let mut entries = fs::read_dir(&path).await?;
            let mut children = Vec::new();
            while let Some(entry) = entries.next_entry().await? {
                children.push(entry.path());
            }
            children.sort();
            stack.extend(
                children
                    .into_iter()
                    .rev()
                    .map(|child| (child, inner.clone(), depth + 1)),
            );
            plan.folders.push(inner);
        } else if metadata.is_file() {
            plan.files.push(UploadFile {
                path,
                folder,
                size: metadata.len(),
            });
        }
    }
    Ok(plan)
}

/// Creates local folders for `nodes` under `root` and lists every file to download.
pub async fn plan_download(
    client: Client,
    nodes: Vec<Node>,
    root: PathBuf,
) -> Result<Vec<DownloadFile>, String> {
    let mut files = Vec::new();
    let mut stack: Vec<(Node, PathBuf, usize)> = nodes
        .into_iter()
        .rev()
        .map(|node| {
            let target = root.join(sanitize_component(&node.name));
            (node, target, 0)
        })
        .collect();
    while let Some((node, target, depth)) = stack.pop() {
        match node.kind {
            NodeKind::File => files.push(DownloadFile {
                id: node.id,
                name: node.name,
                target,
                size: byte_size(node.size),
                existing: Existing::Fail,
                then: Then::Keep,
            }),
            NodeKind::Folder if depth >= MAX_DEPTH => return Err(TOO_DEEP.to_owned()),
            NodeKind::Folder => {
                fs::create_dir_all(&target)
                    .await
                    .map_err(|err| format!("{}: {err}", target.display()))?;
                let mut children = client
                    .list(Some(node.id))
                    .await
                    .map_err(|err| err.to_string())?;
                sort_nodes(&mut children, Sort::default());
                stack.extend(children.into_iter().rev().map(|child| {
                    let path = target.join(sanitize_component(&child.name));
                    (child, path, depth + 1)
                }));
            }
        }
    }
    Ok(files)
}

/// Runs `batch` until it is done or `token` is cancelled. A transfer cut short by the token
/// reports nothing, and the batch ends with [`Message::BatchDone`] either way.
fn run_batch(
    client: Client,
    batch: Batch,
    ids: Vec<TransferId>,
    token: CancellationToken,
) -> impl Stream<Item = Message> {
    iced::stream::channel(
        CHANNEL_SIZE,
        async move |mut output: mpsc::Sender<Message>| {
            let direction = batch.direction();
            match batch {
                Batch::Upload { parent, plan } => {
                    upload_all(&client, parent, plan, &ids, &token, &mut output).await;
                }
                Batch::Download(files) => {
                    download_all(&client, files, &ids, &token, &mut output).await;
                }
            }
            let _ = output.send(Message::BatchDone(direction)).await;
        },
    )
}

async fn upload_all(
    client: &Client,
    parent: Option<NodeId>,
    plan: UploadPlan,
    ids: &[TransferId],
    token: &CancellationToken,
    output: &mut mpsc::Sender<Message>,
) {
    let mut folders: HashMap<Vec<String>, Result<Option<NodeId>, String>> =
        HashMap::from([(Vec::new(), Ok(parent))]);
    for path in plan.folders {
        if let Some((name, base)) = path.split_last() {
            let created = match folders.get(base) {
                Some(Ok(base_id)) => {
                    let Some(created) = token
                        .run_until_cancelled(ensure_folder(client, *base_id, name))
                        .await
                    else {
                        return;
                    };
                    created.map(Some).map_err(|err| err.to_string())
                }
                Some(Err(err)) => Err(err.clone()),
                None => Err(MISSING_FOLDER.to_owned()),
            };
            folders.insert(path, created);
        }
    }
    for (file, id) in plan.files.iter().zip(ids) {
        if token.is_cancelled() {
            return;
        }
        let _ = output.send(Message::Started(*id)).await;
        let result = match folders.get(&file.folder) {
            Some(Ok(target)) => {
                let upload = upload_file(client, *target, file, *id, output);
                match token.run_until_cancelled(upload).await {
                    Some(result) => result,
                    None => return,
                }
            }
            Some(Err(err)) => Err(err.clone()),
            None => Err(MISSING_FOLDER.to_owned()),
        };
        let _ = output.send(Message::Finished(*id, result)).await;
    }
}

async fn ensure_folder(
    client: &Client,
    parent: Option<NodeId>,
    name: &str,
) -> Result<NodeId, ApiError> {
    match client.create_folder(parent, name).await {
        Ok(folder) => Ok(folder.id),
        Err(ApiError::Api {
            code: ErrorCode::Conflict,
            message,
        }) => client
            .list(parent)
            .await?
            .into_iter()
            .find(|node| node.kind == NodeKind::Folder && node.name == name)
            .map(|node| node.id)
            .ok_or(ApiError::Api {
                code: ErrorCode::Conflict,
                message,
            }),
        Err(err) => Err(err),
    }
}

async fn upload_file(
    client: &Client,
    parent: Option<NodeId>,
    file: &UploadFile,
    id: TransferId,
    output: &mpsc::Sender<Message>,
) -> Result<(), String> {
    let name = display_name(&file.path);
    let stored = if file.size > client.upload_chunk_bytes() {
        let request = CreateUpload {
            parent_id: parent,
            name,
            size: file.size,
            modified_at: None,
            mime: None,
            node_id: None,
            keep_both: false,
            version_of: None,
            auto_upload: false,
            transfer_item_id: None,
        };
        client
            .upload_resumable(
                &request,
                |offset, len| read_range(&file.path, offset, len),
                |done| {
                    let _ = output.clone().try_send(Message::Progress(id, done));
                },
            )
            .await
    } else {
        client
            .upload(parent, &name, || {
                upload_body(&file.path, id, output.clone())
            })
            .await
    };
    stored.map(drop).map_err(|err| err.to_string())
}

/// Reads `len` bytes of the file at `path` starting at `offset`, for one chunk of a
/// resumable upload.
fn upload_body(path: &Path, id: TransferId, mut output: mpsc::Sender<Message>) -> reqwest::Body {
    match std::fs::File::open(path) {
        Ok(file) => {
            let mut meter = Meter::new(id);
            let chunks = ReaderStream::new(fs::File::from_std(file)).inspect(move |chunk| {
                if let Ok(bytes) = chunk
                    && let Some(message) = meter.advance(bytes.len())
                {
                    let _ = output.try_send(message);
                }
            });
            reqwest::Body::wrap_stream(chunks)
        }
        Err(err) => {
            reqwest::Body::wrap_stream(stream::once(async move { Err::<Vec<u8>, io::Error>(err) }))
        }
    }
}

async fn download_all(
    client: &Client,
    files: Vec<DownloadFile>,
    ids: &[TransferId],
    token: &CancellationToken,
    output: &mut mpsc::Sender<Message>,
) {
    for (file, id) in files.iter().zip(ids) {
        if token.is_cancelled() {
            return;
        }
        let _ = output.send(Message::Started(*id)).await;
        let Some(result) = download_file(client, file, *id, token, output).await else {
            return;
        };
        let result = result.and_then(|()| match file.then {
            Then::Keep => Ok(()),
            Then::Open => open_file(&file.target),
        });
        let _ = output.send(Message::Finished(*id, result)).await;
    }
}

/// Opens `path` with the system's default app.
pub fn open_file(path: &Path) -> Result<(), String> {
    open::that_detached(path).map_err(|err| format!("could not open {}: {err}", path.display()))
}

/// Downloads `file` next to its target and moves it into place; `None` when `token` cut it
/// short, in which case the partial file is removed.
async fn download_file(
    client: &Client,
    file: &DownloadFile,
    id: TransferId,
    token: &CancellationToken,
    output: &mut mpsc::Sender<Message>,
) -> Option<Result<(), String>> {
    let exists = match fs::try_exists(&file.target).await {
        Ok(exists) => exists,
        Err(err) => return Some(Err(err.to_string())),
    };
    if exists && file.existing == Existing::Fail {
        return Some(Err(ALREADY_EXISTS.to_owned()));
    }
    let response = match token.run_until_cancelled(client.download(file.id)).await? {
        Ok(response) => response,
        Err(err) => return Some(Err(err.to_string())),
    };
    let part = part_path(&file.target);
    let written = token
        .run_until_cancelled(write_part(response, &part, id, output))
        .await;
    let result = match written {
        Some(Ok(())) => fs::rename(&part, &file.target).await,
        Some(Err(err)) => Err(err),
        None => {
            let _ = fs::remove_file(&part).await;
            return None;
        }
    };
    if result.is_err() {
        let _ = fs::remove_file(&part).await;
    }
    Some(result.map_err(|err| err.to_string()))
}

async fn write_part(
    response: reqwest::Response,
    part: &Path,
    id: TransferId,
    output: &mut mpsc::Sender<Message>,
) -> io::Result<()> {
    let mut out = fs::File::create(part).await?;
    let mut body = response.bytes_stream();
    let mut meter = Meter::new(id);
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(io::Error::other)?;
        out.write_all(&chunk).await?;
        if let Some(message) = meter.advance(chunk.len()) {
            let _ = output.try_send(message);
        }
    }
    out.flush().await
}

struct Meter {
    id: TransferId,
    done: u64,
    reported: Option<Instant>,
}

impl Meter {
    fn new(id: TransferId) -> Self {
        Self {
            id,
            done: 0,
            reported: None,
        }
    }

    fn advance(&mut self, bytes: usize) -> Option<Message> {
        self.done = self
            .done
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
        let now = Instant::now();
        let due = self
            .reported
            .is_none_or(|at| now.duration_since(at) >= PROGRESS_INTERVAL);
        if due {
            self.reported = Some(now);
            Some(Message::Progress(self.id, self.done))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transfer(state: TransferState, done: u64, total: u64) -> Transfer {
        Transfer {
            id: 0,
            name: String::new(),
            direction: Direction::Upload,
            done,
            total,
            state,
        }
    }

    fn file_node() -> Node {
        Node {
            id: NodeId(uuid::Uuid::from_u128(1)),
            parent_id: None,
            kind: NodeKind::File,
            name: "a.txt".to_owned(),
            size: 3,
            sha256: None,
            mime: String::new(),
            version: 1,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    fn numbered(id: TransferId, state: TransferState) -> Transfer {
        Transfer {
            id,
            ..transfer(state, 10, 100)
        }
    }

    fn client() -> Client {
        Client::new("http://localhost")
    }

    /// A download of three files, the first done, the second under way and the third waiting.
    fn running_download() -> Transfers {
        let file = |name: &str| DownloadFile::replacing(&file_node(), PathBuf::from(name));
        Transfers {
            items: vec![
                numbered(0, TransferState::Done),
                numbered(1, TransferState::Active),
                numbered(2, TransferState::Queued),
            ],
            current: Some(Running {
                batch: Batch::Download(vec![file("/tmp/a"), file("/tmp/b"), file("/tmp/c")]),
                ids: vec![0, 1, 2],
                token: CancellationToken::new(),
            }),
            next_id: 3,
            ..Transfers::default()
        }
    }

    fn states(transfers: &Transfers) -> Vec<TransferState> {
        transfers
            .items
            .iter()
            .map(|item| item.state.clone())
            .collect()
    }

    #[test]
    fn pause_holds_every_unfinished_transfer() {
        let mut transfers = running_download();
        let token = transfers
            .current
            .as_ref()
            .map(|running| running.token.clone());

        let _ = transfers.update(&client(), Message::Pause);

        assert!(token.is_some_and(|token| token.is_cancelled()));
        assert_eq!(
            states(&transfers),
            [
                TransferState::Done,
                TransferState::Paused,
                TransferState::Paused
            ]
        );
        let mut ui = iced_test::simulator(transfers.view());
        assert!(ui.find("Paused · 2 of 3 left").is_ok());
    }

    #[test]
    fn a_paused_batch_queues_what_is_left_once_it_ends() {
        let mut transfers = running_download();
        let _ = transfers.update(&client(), Message::Pause);

        let _ = transfers.update(&client(), Message::BatchDone(Direction::Download));

        assert!(transfers.current.is_none());
        let ids: Vec<Vec<TransferId>> = transfers
            .pending
            .iter()
            .map(|(_, ids)| ids.clone())
            .collect();
        assert_eq!(ids, [vec![1, 2]]);
    }

    #[test]
    fn continue_starts_the_held_transfers_over() {
        let mut transfers = running_download();
        let _ = transfers.update(&client(), Message::Pause);
        let _ = transfers.update(&client(), Message::BatchDone(Direction::Download));

        let _ = transfers.update(&client(), Message::Resume);

        assert!(!transfers.paused);
        assert_eq!(
            transfers
                .current
                .as_ref()
                .map(|running| running.ids.clone()),
            Some(vec![1, 2])
        );
        assert_eq!(
            states(&transfers),
            [
                TransferState::Done,
                TransferState::Queued,
                TransferState::Queued
            ]
        );
        assert_eq!(transfers.items[1].done, 0);
    }

    #[test]
    fn a_held_transfer_ignores_late_progress() {
        let mut transfers = running_download();
        let _ = transfers.update(&client(), Message::Pause);

        let _ = transfers.update(&client(), Message::Progress(1, 99));
        let _ = transfers.update(&client(), Message::Started(2));

        assert_eq!(transfers.items[1].done, 10);
        assert_eq!(transfers.items[2].state, TransferState::Paused);
    }

    #[test]
    fn stop_cancels_everything_left() {
        let mut transfers = running_download();
        transfers
            .pending
            .push_back((Batch::Download(Vec::new()), Vec::new()));

        let _ = transfers.update(&client(), Message::Stop);
        let _ = transfers.update(&client(), Message::BatchDone(Direction::Download));

        assert!(transfers.pending.is_empty());
        assert!(transfers.current.is_none());
        assert_eq!(
            states(&transfers),
            [
                TransferState::Done,
                TransferState::Cancelled,
                TransferState::Cancelled
            ]
        );
        assert_eq!(
            summarize(&transfers.items, false),
            "1 completed, 2 cancelled"
        );
    }

    #[test]
    fn clear_completed_drops_cancelled_rows_too() {
        let mut transfers = running_download();
        let _ = transfers.update(&client(), Message::Stop);

        let _ = transfers.update(&client(), Message::ClearCompleted);

        assert!(transfers.is_empty());
    }

    #[test]
    fn an_opening_download_replaces_and_then_opens() {
        let file = DownloadFile::opening(&file_node(), PathBuf::from("/tmp/a.txt"));
        assert_eq!(file.existing, Existing::Replace);
        assert_eq!(file.then, Then::Open);
    }

    #[test]
    fn a_replacing_download_is_kept() {
        let file = DownloadFile::replacing(&file_node(), PathBuf::from("/tmp/a.txt"));
        assert_eq!(file.then, Then::Keep);
    }

    #[test]
    fn a_file_icon_goes_by_the_extension() {
        assert_eq!(file_icon("IMG_1.JPG"), Icon::Image);
        assert_eq!(file_icon("notes.txt"), Icon::Document);
        assert_eq!(file_icon("budget.ods"), Icon::Other);
        assert_eq!(file_icon("README"), Icon::Other);
    }

    #[test]
    fn each_row_says_how_its_transfer_goes() {
        let transfers = Transfers {
            items: vec![
                transfer(TransferState::Active, 64, 100),
                transfer(TransferState::Queued, 0, 100),
                transfer(TransferState::Done, 100, 100),
                transfer(TransferState::Failed("too large".to_owned()), 0, 100),
            ],
            ..Transfers::default()
        };
        let mut ui = iced_test::simulator(transfers.view());

        assert!(ui.find("64%").is_ok());
        assert!(ui.find("Waiting").is_ok());
        assert!(ui.find("Done").is_ok());
        assert!(ui.find("Failed: too large").is_ok());
    }

    #[test]
    fn a_collapsed_panel_only_has_the_summary() {
        let transfers = Transfers {
            items: vec![transfer(TransferState::Active, 64, 100)],
            collapsed: true,
            ..Transfers::default()
        };
        let mut ui = iced_test::simulator(transfers.view());

        assert!(ui.find("Uploading 1 of 1").is_ok());
        assert!(ui.find("64%").is_err());
    }

    #[test]
    fn summary_counts_the_current_item() {
        let items = [
            transfer(TransferState::Done, 1, 1),
            transfer(TransferState::Active, 0, 1),
            transfer(TransferState::Queued, 0, 1),
        ];
        assert_eq!(summarize(&items, false), "Uploading 2 of 3");
    }

    #[test]
    fn summary_reports_failures() {
        let items = [
            transfer(TransferState::Done, 1, 1),
            transfer(TransferState::Failed("boom".to_owned()), 0, 1),
        ];
        assert_eq!(summarize(&items, false), "1 failed, 1 completed");
    }

    #[test]
    fn overall_progress_averages_fractions() {
        let items = [
            transfer(TransferState::Done, 0, 0),
            transfer(TransferState::Active, 25, 100),
        ];
        assert!((overall_progress(&items) - 0.625).abs() < f32::EPSILON);
    }

    #[test]
    fn part_path_appends_the_suffix() {
        assert_eq!(
            part_path(Path::new("/tmp/a.pdf")),
            PathBuf::from("/tmp/a.pdf.part")
        );
    }

    #[tokio::test]
    async fn local_plan_lists_folders_before_their_files() {
        let root = std::env::temp_dir().join(format!("home-space-plan-{}", std::process::id()));
        let inner = root.join("inner");
        fs::create_dir_all(&inner).await.expect("create dirs");
        fs::write(inner.join("a.txt"), b"hello")
            .await
            .expect("write file");

        let plan = plan_upload(vec![root.clone()]).await.expect("plan");
        fs::remove_dir_all(&root).await.expect("cleanup");

        let top = display_name(&root);
        assert_eq!(
            plan.folders,
            [vec![top.clone()], vec![top.clone(), "inner".to_owned()]]
        );
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].folder, [top, "inner".to_owned()]);
        assert_eq!(plan.files[0].size, 5);
    }
}
