use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::Arc,
};

use iced::{
    Alignment, Element, Length, Task,
    widget::{Row, button, column, container, progress_bar, row, scrollable, space, text},
};

use client_core::{format::human_size, labels::HOME};
use protocol::{Node, NodeId, NodeKind};

use super::{
    Message as BrowserMessage,
    context_menu::Entry,
    parts::{self, alert_error, divider, ghost, outlined, small_outlined},
    sync_queue::RETRY_ALL,
    toolbar,
};
use crate::{
    ellipsis::ellipsized,
    icons::{ICON_SIZE, Icon, Tint},
    locale::short_date,
    sync::{self, Activity, Badge, Command, Commands, ItemState, Mapping, Outage, Status},
    theme,
};

const STATUS_PADDING: [f32; 2] = [10.0, 12.0];
const STATUS_COMPACT_PADDING: f32 = 8.0;
const STATUS_GAP: f32 = 8.0;
const STATUS_SPACING: f32 = 6.0;
const STATUS_BAR: f32 = 6.0;
const CONFIRM_WIDTH: f32 = 500.0;
const CONFIRM_GAP: f32 = 8.0;
const SPACING: f32 = 10.0;
const NOTE_SIZE: f32 = 13.0;
const PATH_SIZE: f32 = 13.0;
const CARD_SPACING: f32 = 16.0;
const CARD_PADDING: [f32; 2] = [14.0, 16.0];
const CARD_TITLE_GAP: f32 = 2.0;
const ACTION_GAP: f32 = 4.0;
const SYNC_AGAIN: &str = "Sync again";
const STOP_SYNCING: &str = "Stop syncing";
const SET_TO_NOT_SYNC: &str = "Set to not sync";
const SHOW_ALL: &str = "Show all";
const RETRY_NOW: &str = "Retry now";
const VERIFY: &str = "Verify";
const VERIFYING: &str = "Verifying…";

#[derive(Debug, Clone)]
pub enum Message {
    /// A report of the sync engine.
    Engine(sync::Update),
    /// Asks for the local folder a remote folder (`None` for Home) syncs with.
    Pick(Option<NodeId>),
    Picked(Option<NodeId>, Option<PathBuf>),
    Measured(Confirm),
    /// Whether to go ahead with the picked folder.
    Confirm(bool),
    Set(Mapping),
    Unmap(Option<NodeId>),
    OpenLocal(PathBuf),
    /// Tries the failed paths again now: these ones, or all of them with `None`.
    Retry(Option<Vec<PathBuf>>),
    /// Reads a synced folder (`None` for Home) from scratch on both sides.
    Verify(Option<NodeId>),
}

/// What the browser has to do after a sync update.
pub enum Outcome {
    Task(Task<Message>),
    /// Something to tell the user.
    Error(String),
}

/// What the browser knows of the sync engine.
#[derive(Debug, Default)]
pub struct Syncing {
    commands: Option<Commands>,
    status: Arc<Status>,
    /// A picked folder waiting for the user to confirm it.
    pub confirm: Option<Confirm>,
    /// Why the last mapping was refused.
    pub notice: Option<String>,
}

/// A local folder picked for a remote one, shown before syncing starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    pub remote: Option<NodeId>,
    /// The remote folder's name.
    pub name: String,
    pub local: PathBuf,
    pub contents: Contents,
}

/// What a local folder already holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Contents {
    pub files: u64,
    pub bytes: u64,
}

impl Syncing {
    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Applies `message`; `listed` names a folder (not Home) the listing knows.
    pub fn update(
        &mut self,
        message: Message,
        listed: impl FnOnce(NodeId) -> Option<String>,
    ) -> Outcome {
        match message {
            Message::Engine(update) => match update {
                sync::Update::Ready(commands) => self.commands = Some(commands),
                sync::Update::Status(status) => self.status = status,
                sync::Update::Settled(_) => {}
                sync::Update::Rejected(reason) => {
                    self.notice = Some(reason.clone());
                    return Outcome::Error(format!("Could not sync there: {reason}"));
                }
            },
            Message::Pick(remote) => {
                let ask = crate::file_dialog::over_window(|dialog| {
                    dialog.set_title("Choose the local folder to sync with")
                });
                return Outcome::Task(ask.then(move |dialog| {
                    Task::perform(pick_folder(dialog), move |local| {
                        Message::Picked(remote, local)
                    })
                }));
            }
            Message::Picked(remote, Some(local)) => {
                let name = match remote {
                    None => HOME.to_owned(),
                    Some(id) => listed(id)
                        .or_else(|| self.status.names.get(&id).cloned())
                        .unwrap_or_else(|| "this folder".to_owned()),
                };
                return Outcome::Task(Task::perform(measure(local.clone()), move |contents| {
                    Message::Measured(Confirm {
                        remote,
                        name,
                        local,
                        contents,
                    })
                }));
            }
            Message::Picked(_, None) => {}
            Message::Measured(confirm) => self.confirm = Some(confirm),
            Message::Confirm(true) => self.confirmed(),
            Message::Confirm(false) => self.confirm = None,
            Message::Set(mapping) => self.send(Command::Map(mapping)),
            Message::Unmap(remote) => self.send(Command::Unmap(remote)),
            Message::OpenLocal(path) => {
                if let Err(err) = reveal(&path) {
                    return Outcome::Error(err);
                }
            }
            Message::Retry(paths) => self.send(Command::Retry(paths)),
            Message::Verify(remote) => self.send(Command::Verify(remote)),
        }
        Outcome::Task(Task::none())
    }

    /// Starts syncing what was confirmed.
    fn confirmed(&mut self) {
        if let Some(confirm) = self.confirm.take() {
            self.notice = None;
            self.send(Command::Map(Mapping {
                remote: confirm.remote,
                local: Some(confirm.local),
            }));
        }
    }

    pub fn send(&self, command: Command) {
        if let Some(commands) = &self.commands {
            commands.send(command);
        }
    }

    /// The sync entries of the context menu for `selected`: only a single folder has any.
    pub fn entries(&self, selected: &[&Node]) -> Vec<Entry> {
        let [node] = selected else {
            return Vec::new();
        };
        if node.kind != NodeKind::Folder {
            return Vec::new();
        }
        let id = Some(node.id);
        match self.status.mapping(id).map(|mapping| mapping.local.clone()) {
            Some(Some(local)) => vec![
                Entry {
                    icon: Icon::FolderOpen,
                    label: "Open local folder",
                    message: BrowserMessage::Sync(Message::OpenLocal(local)),
                },
                Entry {
                    icon: Icon::Sync,
                    label: "Change local folder…",
                    message: BrowserMessage::Sync(Message::Pick(id)),
                },
                Entry {
                    icon: Icon::SyncDisabled,
                    label: "Stop syncing",
                    message: BrowserMessage::Sync(Message::Unmap(id)),
                },
            ],
            Some(None) => vec![Entry {
                icon: Icon::Sync,
                label: "Sync again",
                message: BrowserMessage::Sync(Message::Unmap(id)),
            }],
            None => {
                let mut entries = vec![Entry {
                    icon: Icon::Sync,
                    label: "Sync to local folder…",
                    message: BrowserMessage::Sync(Message::Pick(id)),
                }];
                // Only a folder synced through a parent can be left out of it.
                if self.status.badge(id).is_some() {
                    entries.push(Entry {
                        icon: Icon::SyncDisabled,
                        label: "Don't sync",
                        message: BrowserMessage::Sync(Message::Set(Mapping {
                            remote: id,
                            local: None,
                        })),
                    });
                }
                entries
            }
        }
    }
}

/// The icon and tint of `badge`.
pub fn badge_icon(badge: Badge) -> (Icon, Tint) {
    match badge {
        Badge::Synced => (Icon::Synced, Tint::Success),
        Badge::Pending => (Icon::Sync, Tint::Accent),
        Badge::Problem => (Icon::Error, Tint::Danger),
        Badge::Disabled => (Icon::SyncDisabled, Tint::Muted),
    }
}

/// What the sidebar says about the sync.
struct Overview {
    icon: Icon,
    tint: Tint,
    title: &'static str,
    detail: Option<String>,
    /// How much of the work ahead is done, while there is some.
    progress: Option<f32>,
}

fn overview(status: &Status) -> Overview {
    let plain = |icon, tint, title, detail| Overview {
        icon,
        tint,
        title,
        detail,
        progress: None,
    };
    let busy = match status.activity {
        Activity::Checking => Some("Checking server…"),
        Activity::Scanning => Some("Scanning…"),
        Activity::Idle | Activity::Syncing => None,
    };
    match (status.overall(), busy) {
        (_, Some(title)) if status.pending == 0 => plain(Icon::Sync, Tint::Accent, title, None),
        (None, _) => plain(
            Icon::Sync,
            Tint::Muted,
            "Sync",
            Some("Choose a local folder".to_owned()),
        ),
        (Some(Badge::Synced), _) => plain(Icon::Synced, Tint::Success, "Up to date", None),
        (Some(Badge::Pending), _) => Overview {
            progress: progress(status),
            ..plain(Icon::Sync, Tint::Accent, "Syncing", Some(work_left(status)))
        },
        (Some(Badge::Problem | Badge::Disabled), _) => match &status.outage {
            Some(outage) => plain(
                Icon::SyncProblem,
                Tint::Danger,
                "Server unreachable",
                Some(format!("Trying again at {}", short_date(outage.retry_at))),
            ),
            None => plain(
                Icon::SyncProblem,
                Tint::Danger,
                "Sync problem",
                (!status.problems.is_empty())
                    .then(|| format!("{} could not be synced", plural(status.problems.len()))),
            ),
        },
    }
}

/// "3 items · 18.4 MB left".
fn work_left(status: &Status) -> String {
    let items = plural(status.pending);
    match status.summary().bytes_left {
        0 => items,
        bytes => format!(
            "{items} · {} left",
            human_size(i64::try_from(bytes).unwrap_or(i64::MAX))
        ),
    }
}

/// How much of the bytes in the queue have moved.
fn progress(status: &Status) -> Option<f32> {
    let (done, size) = status
        .queue
        .iter()
        .filter(|item| !matches!(item.state, ItemState::Failed(_)))
        .fold((0, 0), |(done, size), item| {
            (done + item.done, size + item.size)
        });
    (size > 0).then(|| done as f32 / size as f32)
}

/// The sidebar's box summing up the sync, which opens its settings.
pub fn status_card(status: &Status, compact: bool, active: bool) -> Element<'_, BrowserMessage> {
    let overview = overview(status);
    let open = BrowserMessage::OpenSyncSettings;
    if compact {
        let card = button(overview.icon.view(ICON_SIZE, overview.tint))
            .padding(STATUS_COMPACT_PADDING)
            .style(theme::status_card(active))
            .on_press(open);
        return toolbar::tip(card.into(), overview.title, "");
    }
    let mut card = column![
        row![
            overview.icon.view(ICON_SIZE, overview.tint),
            text(overview.title).font(theme::MEDIUM)
        ]
        .spacing(STATUS_GAP)
        .align_y(Alignment::Center)
    ]
    .spacing(STATUS_SPACING);
    if let Some(detail) = overview.detail {
        card = card.push(text(detail).size(NOTE_SIZE).style(theme::muted_text));
    }
    if let Some(done) = overview.progress {
        card = card.push(
            progress_bar(0.0..=1.0, done)
                .girth(STATUS_BAR)
                .style(theme::progress),
        );
    }
    button(card)
        .padding(STATUS_PADDING)
        .width(Length::Fill)
        .style(theme::status_card(active))
        .on_press(open)
        .into()
}

/// Asks before syncing a remote folder with the picked local folder, which can still be changed.
pub fn confirm<'a>(confirm: &'a Confirm, status: &Status) -> Element<'a, BrowserMessage> {
    let name = confirm.name.as_str();
    let local = container(
        row![
            Icon::FolderOpen.view(ICON_SIZE, Tint::Muted),
            ellipsized(
                display_path(&confirm.local),
                parts::ROW_SIZE,
                Alignment::Start
            )
            .width(Length::Fill),
        ]
        .spacing(CONFIRM_GAP)
        .align_y(Alignment::Center),
    )
    .padding(parts::FIELD_PADDING)
    .width(Length::Fill)
    .style(theme::field);
    let folder = row![
        local,
        outlined(
            None,
            "Choose folder…",
            Some(BrowserMessage::Sync(Message::Pick(confirm.remote)))
        ),
    ]
    .spacing(CONFIRM_GAP)
    .align_y(Alignment::Center);
    let instead = status
        .location(confirm.remote)
        .map(|current| format!(" instead of {}", display_path(current)))
        .unwrap_or_default();
    parts::dialog(
        CONFIRM_WIDTH,
        column![
            parts::dialog_title(format!("Sync “{name}” with this folder?")),
            parts::field("Local folder", folder, None),
            parts::alert_info(
                Icon::Info,
                format!(
                    "“{name}” and its subfolders sync here{instead}. You can change this later in \
                     Sync settings."
                ),
                None,
            ),
        ],
        [
            outlined(
                None,
                "Cancel",
                Some(BrowserMessage::Sync(Message::Confirm(false))),
            ),
            parts::primary(
                Some(Icon::Sync),
                "Sync",
                Some(BrowserMessage::Sync(Message::Confirm(true))),
            ),
        ],
    )
}

/// Counts the files under `path` and their size.
pub async fn measure(path: PathBuf) -> Contents {
    tokio::task::spawn_blocking(move || {
        let mut contents = Contents::default();
        let mut pending = vec![path];
        while let Some(dir) = pending.pop() {
            let Ok(listing) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in listing.flatten() {
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                if meta.is_dir() {
                    pending.push(entry.path());
                } else if meta.is_file() {
                    contents.files += 1;
                    contents.bytes += meta.len();
                }
            }
        }
        contents
    })
    .await
    .unwrap_or_default()
}

/// What the sync settings' top line offers: reading Home again from scratch, when it syncs.
pub fn settings_action(status: &Status) -> Element<'_, BrowserMessage> {
    match status.mapping(None) {
        Some(Mapping { local: Some(_), .. }) => {
            let verifying = status.is_verifying(None);
            outlined(
                Some(Icon::Synced),
                verify_label(verifying),
                (!verifying).then_some(verify(None)),
            )
        }
        _ => space().into(),
    }
}

/// The sync settings: what went wrong, and what syncs where. In a window too narrow for a row's
/// buttons next to its text, as with `stacked`, they go under it.
pub fn settings<'a>(
    status: &'a Status,
    notice: Option<&'a str>,
    stacked: bool,
) -> Element<'a, BrowserMessage> {
    let mut page = column![].spacing(CARD_SPACING);
    for error in status.error.as_deref().into_iter().chain(notice) {
        page = page.push(alert_error(Icon::SyncProblem, error, None));
    }
    if let Some(outage) = &status.outage {
        let retry = small_outlined(RETRY_NOW, Some(BrowserMessage::Sync(Message::Retry(None))));
        page = page.push(alert_error(
            Icon::SyncProblem,
            outage_text(outage),
            Some(retry),
        ));
    }
    page = page.push(local_folder_card(status, stacked));
    page = page.push(own_settings_card(status, stacked));
    if !status.problems.is_empty() {
        page = page.push(problems_card(status));
    }
    scrollable(page).height(Length::Fill).into()
}

fn card<'a>(
    title: impl text::IntoFragment<'a>,
    note: impl text::IntoFragment<'a>,
    action: Option<Element<'a, BrowserMessage>>,
    rows: Vec<Element<'a, BrowserMessage>>,
) -> Element<'a, BrowserMessage> {
    let head = row![
        column![
            text(title).font(theme::MEDIUM),
            text(note).size(NOTE_SIZE).style(theme::muted_text)
        ]
        .spacing(CARD_TITLE_GAP)
        .width(Length::Fill)
    ]
    .push(action)
    .spacing(SPACING)
    .align_y(Alignment::Center);
    let mut card = column![container(head).padding(CARD_PADDING)];
    for row in rows {
        card = card
            .push(divider())
            .push(container(row).padding(CARD_PADDING));
    }
    container(card)
        .width(Length::Fill)
        .style(theme::card)
        .into()
}

/// A row of a card: an icon and text, then what can be done about it.
fn card_row<'a>(
    icon: Element<'a, BrowserMessage>,
    text: Element<'a, BrowserMessage>,
    actions: Row<'a, BrowserMessage>,
    stacked: bool,
) -> Element<'a, BrowserMessage> {
    let line = row![icon, text].spacing(SPACING).align_y(Alignment::Center);
    let actions = actions.spacing(ACTION_GAP).align_y(Alignment::Center);
    if stacked {
        column![line, actions].spacing(SPACING).into()
    } else {
        row![line.width(Length::Fill), actions]
            .spacing(SPACING)
            .align_y(Alignment::Center)
            .into()
    }
}

/// Where Home syncs on this computer.
fn local_folder_card(status: &Status, stacked: bool) -> Element<'_, BrowserMessage> {
    let open = |message: Message| BrowserMessage::Sync(message);
    let actions = match status.mapping(None).map(|mapping| &mapping.local) {
        None => row![outlined(
            None,
            "Choose folder…",
            Some(open(Message::Pick(None)))
        )],
        Some(None) => row![ghost(None, SYNC_AGAIN, Some(open(Message::Unmap(None))))],
        Some(Some(local)) => row![
            ghost(
                Some(Icon::Web),
                "Open local folder",
                Some(open(Message::OpenLocal(local.clone())))
            ),
            ghost(None, STOP_SYNCING, Some(open(Message::Unmap(None)))),
            outlined(
                None,
                "Change local folder…",
                Some(open(Message::Pick(None)))
            ),
        ],
    };
    let where_it_is: Element<'_, BrowserMessage> =
        match status.mapping(None).map(|mapping| mapping.local.as_deref()) {
            Some(Some(local)) => text(display_path(local))
                .size(PATH_SIZE)
                .font(theme::MONO)
                .width(Length::Fill)
                .into(),
            Some(None) => text(SET_TO_NOT_SYNC)
                .style(theme::muted_text)
                .width(Length::Fill)
                .into(),
            None => text("Not synced")
                .style(theme::muted_text)
                .width(Length::Fill)
                .into(),
        };
    card(
        "Local folder",
        format!("{HOME} syncs into this folder on this computer."),
        None,
        vec![card_row(
            Icon::FolderOpen.view(ICON_SIZE, Tint::Accent),
            where_it_is,
            actions,
            stacked,
        )],
    )
}

/// The folders that sync somewhere else than through Home, or not at all.
fn own_settings_card(status: &Status, stacked: bool) -> Element<'_, BrowserMessage> {
    let mut mappings: Vec<(String, &Mapping)> = status
        .mappings
        .iter()
        .filter_map(|mapping| mapping.remote.map(|id| (folder_name(status, id), mapping)))
        .collect();
    mappings.sort_by(|(a, _), (b, _)| a.cmp(b));
    let rows = mappings
        .into_iter()
        .map(|(name, mapping)| own_setting_row(name, mapping, status, stacked))
        .collect();
    card(
        "Folders with their own setting",
        "Right-click a folder to sync it to a location of its own, or to leave it out.",
        None,
        rows,
    )
}

fn own_setting_row<'a>(
    name: String,
    mapping: &'a Mapping,
    status: &'a Status,
    stacked: bool,
) -> Element<'a, BrowserMessage> {
    let remote = mapping.remote;
    let open = |message: Message| Some(BrowserMessage::Sync(message));
    let (icon, target, actions) = match &mapping.local {
        Some(local) => {
            let verifying = status.is_verifying(remote);
            (
                Icon::Folder.view(ICON_SIZE, Tint::Accent),
                format!("Syncs to {}", display_path(local)),
                row![
                    ghost(None, "Open", open(Message::OpenLocal(local.clone()))),
                    ghost(
                        None,
                        verify_label(verifying),
                        (!verifying).then_some(verify(remote))
                    ),
                    ghost(None, "Change…", open(Message::Pick(remote))),
                    ghost(None, STOP_SYNCING, open(Message::Unmap(remote))),
                ],
            )
        }
        None => (
            Icon::FolderOutline.view(ICON_SIZE, Tint::AccentFaded),
            SET_TO_NOT_SYNC.to_owned(),
            row![ghost(None, SYNC_AGAIN, open(Message::Unmap(remote)))],
        ),
    };
    let text = column![
        text(name),
        text(target).size(NOTE_SIZE).style(theme::muted_text)
    ]
    .width(Length::Fill)
    .into();
    card_row(icon, text, actions, stacked)
}

/// How many paths could not be synced, with the way to the queue that lists them.
fn problems_card(status: &Status) -> Element<'_, BrowserMessage> {
    let actions = row![
        ghost(
            Some(Icon::Refresh),
            RETRY_ALL,
            Some(BrowserMessage::Sync(Message::Retry(None)))
        ),
        outlined(None, SHOW_ALL, Some(BrowserMessage::OpenSyncQueue)),
    ]
    .spacing(ACTION_GAP);
    card(
        format!("{} could not be synced", plural(status.problems.len())),
        "The sync queue lists them with what went wrong.",
        Some(actions.into()),
        Vec::new(),
    )
}

fn verify(remote: Option<NodeId>) -> BrowserMessage {
    BrowserMessage::Sync(Message::Verify(remote))
}

fn verify_label(verifying: bool) -> &'static str {
    if verifying { VERIFYING } else { VERIFY }
}

/// "Cannot reach the server since …: why. Trying again at …".
fn outage_text(outage: &Outage) -> String {
    format!(
        "Cannot reach the server since {}: {}. Trying again at {}.",
        short_date(outage.since),
        outage.reason,
        short_date(outage.retry_at)
    )
}

fn folder_name(status: &Status, id: NodeId) -> String {
    status
        .names
        .get(&id)
        .cloned()
        .unwrap_or_else(|| "A removed folder".to_owned())
}

fn plural(count: usize) -> String {
    if count == 1 {
        "1 item".to_owned()
    } else {
        format!("{count} items")
    }
}

/// `path` for display, with the home folder shortened to `~`.
pub fn display_path(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Opens `path` in the system's file manager.
pub fn reveal(path: &Path) -> Result<(), String> {
    open_external(path.as_os_str(), &path.display().to_string())
}

/// Opens `url` in the default web browser.
pub fn open_url(url: &str) -> Result<(), String> {
    open_external(OsStr::new(url), url)
}

/// Hands `target` to the system to open; `name` is what an error calls it.
fn open_external(target: &OsStr, name: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    const OPENER: &str = "open";
    #[cfg(target_os = "windows")]
    const OPENER: &str = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    const OPENER: &str = "xdg-open";
    std::process::Command::new(OPENER)
        .arg(target)
        .spawn()
        .map(drop)
        .map_err(|err| format!("could not open {name}: {err}"))
}

/// Asks for the local folder to sync with.
pub async fn pick_folder(dialog: rfd::AsyncFileDialog) -> Option<PathBuf> {
    // A single pick reads the panel's URL, which on macOS can be the parent of a folder that
    // was just created in the panel; the list of selected folders is reliable.
    dialog
        .pick_folders()
        .await?
        .into_iter()
        .next()
        .map(|folder| folder.path().to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    fn folder(n: u128) -> Node {
        Node {
            id: NodeId(Uuid::from_u128(n)),
            parent_id: None,
            kind: NodeKind::Folder,
            name: format!("f{n}"),
            size: 0,
            sha256: None,
            mime: "inode/directory".to_owned(),
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    fn syncing(mappings: Vec<Mapping>, badges: Vec<(Node, Badge)>) -> Syncing {
        Syncing {
            status: Arc::new(Status {
                mappings,
                badges: badges
                    .into_iter()
                    .map(|(node, badge)| (Some(node.id), badge))
                    .collect(),
                names: HashMap::new(),
                ..Status::default()
            }),
            ..Syncing::default()
        }
    }

    fn labels(syncing: &Syncing, node: &Node) -> Vec<&'static str> {
        syncing
            .entries(&[node])
            .into_iter()
            .map(|entry| entry.label)
            .collect()
    }

    #[test]
    fn an_unsynced_folder_can_only_be_synced() {
        let node = folder(1);
        assert_eq!(
            labels(&syncing(vec![], vec![]), &node),
            ["Sync to local folder…"]
        );
    }

    #[test]
    fn a_folder_synced_through_its_parent_can_be_left_out() {
        let node = folder(1);
        let syncing = syncing(vec![], vec![(node.clone(), Badge::Synced)]);
        assert_eq!(
            labels(&syncing, &node),
            ["Sync to local folder…", "Don't sync"]
        );
    }

    #[test]
    fn a_mapped_folder_can_be_opened_moved_or_stopped() {
        let node = folder(1);
        let mapping = Mapping {
            remote: Some(node.id),
            local: Some(PathBuf::from("/sync")),
        };
        assert_eq!(
            labels(&syncing(vec![mapping], vec![]), &node),
            ["Open local folder", "Change local folder…", "Stop syncing"]
        );
    }

    #[test]
    fn a_folder_set_to_not_sync_can_sync_again() {
        let node = folder(1);
        let mapping = Mapping {
            remote: Some(node.id),
            local: None,
        };
        assert_eq!(
            labels(&syncing(vec![mapping], vec![]), &node),
            ["Sync again"]
        );
    }

    #[test]
    fn several_selected_nodes_have_no_sync_entries() {
        let (a, b) = (folder(1), folder(2));
        assert!(syncing(vec![], vec![]).entries(&[&a, &b]).is_empty());
    }

    fn page_shows(status: &Status, label: &str) -> bool {
        iced_test::simulator(settings(status, None, false))
            .find(label)
            .is_ok()
    }

    #[test]
    fn an_unsynced_home_offers_to_choose_a_folder() {
        let status = Status::default();
        assert!(page_shows(&status, "Not synced"));
        assert!(page_shows(&status, "Choose folder…"));
    }

    #[test]
    fn a_synced_home_can_be_opened_changed_or_stopped() {
        let status = Status {
            mappings: vec![Mapping {
                remote: None,
                local: Some(PathBuf::from("/sync")),
            }],
            ..Status::default()
        };
        assert!(page_shows(&status, "/sync"));
        assert!(page_shows(&status, "Open local folder"));
        assert!(page_shows(&status, "Change local folder…"));
        assert!(page_shows(&status, STOP_SYNCING));
        assert!(
            iced_test::simulator(settings_action(&status))
                .find(VERIFY)
                .is_ok()
        );
    }

    #[test]
    fn verify_needs_a_synced_home() {
        let status = Status::default();
        assert!(
            iced_test::simulator(settings_action(&status))
                .find(VERIFY)
                .is_err()
        );
    }

    #[test]
    fn a_folder_left_out_can_sync_again_and_a_synced_one_can_stop() {
        let (left_out, own) = (folder(1), folder(2));
        let status = Status {
            mappings: vec![
                Mapping {
                    remote: Some(left_out.id),
                    local: None,
                },
                Mapping {
                    remote: Some(own.id),
                    local: Some(PathBuf::from("/photos")),
                },
            ],
            names: HashMap::from([
                (left_out.id, "Skip".to_owned()),
                (own.id, "Images".to_owned()),
            ]),
            ..Status::default()
        };
        assert!(page_shows(&status, SET_TO_NOT_SYNC));
        assert!(page_shows(&status, SYNC_AGAIN));
        assert!(page_shows(&status, "Syncs to /photos"));
        assert!(page_shows(&status, STOP_SYNCING));
    }

    #[test]
    fn what_failed_leads_to_the_queue() {
        let status = Status {
            problems: vec![(PathBuf::from("/sync/a"), sync::Fault::now("502", "boom"))],
            ..Status::default()
        };
        assert!(page_shows(&status, "1 item could not be synced"));
        let mut ui = iced_test::simulator(settings(&status, None, false));
        ui.click(SHOW_ALL).expect("the button is shown");
        assert!(
            ui.into_messages()
                .any(|message| matches!(message, BrowserMessage::OpenSyncQueue))
        );
    }

    #[test]
    fn the_overall_state_is_the_worst_one() {
        let mapped = vec![Mapping {
            remote: None,
            local: Some(PathBuf::from("/sync")),
        }];
        let mut status = Status::default();
        assert_eq!(status.overall(), None);
        status.mappings = mapped;
        assert_eq!(status.overall(), Some(Badge::Synced));
        status.pending = 2;
        assert_eq!(status.overall(), Some(Badge::Pending));
        status.problems = vec![(PathBuf::from("/sync/a"), sync::Fault::now("local", "boom"))];
        assert_eq!(status.overall(), Some(Badge::Problem));
    }

    #[test]
    fn paths_in_the_home_folder_are_shortened() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        assert_eq!(display_path(&home.join("Sync/Photos")), "~/Sync/Photos");
        assert_eq!(display_path(&home), "~");
        assert_eq!(display_path(Path::new("/Volumes/Ext")), "/Volumes/Ext");
    }
}
