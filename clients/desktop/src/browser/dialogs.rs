use iced::{
    Alignment, Element, Length, Task,
    widget::{
        Row, button, column, container, operation, row, scrollable, text, text::Wrapping,
        text_input,
    },
};

use api_client::{ApiError, Client};
use client_core::{
    format::conflicts_title,
    labels::{HOME, LOST_AND_FOUND, describe, files},
    listing::{Sort, already_in, collapsed_crumbs, sort_nodes},
};
use protocol::{Conflict, MoveResponse, Node, NodeId, NodeKind, OnConflict};

use super::parts;
use crate::{
    ellipsis::{ellipsized, fit},
    icons::{ICON_SIZE, Icon, Tint},
    locale::short_date,
    theme,
};

const NAME_INPUT: &str = "dialog-name";
const WORKING: &str = "Working…";
const CANCEL: &str = "Cancel";
const KEEP_EDITING: &str = "Keep editing";
const DISCARD: &str = "Discard";
const DISCARD_TITLE: &str = "Discard changes?";
const MOVE_WIDTH: f32 = 520.0;
/// What the destination's name may take on the move button, which leaves room for Cancel.
const MOVE_ACTION_WIDTH: f32 = 280.0;
const MOVE_HEAD_GAP: f32 = 4.0;
const CONFLICTS_WIDTH: f32 = 560.0;
const CONFLICTS_HEIGHT: f32 = 280.0;
const CONFLICT_PADDING: [f32; 2] = [10.0, 12.0];
const META_SIZE: f32 = 12.0;
const BOX_PADDING: f32 = 1.0;
const PICKER_HEIGHT: f32 = 248.0;
const PICKER_PADDING: f32 = 4.0;
const PICKER_SCROLLBAR_GAP: f32 = 4.0;
const PICKER_CRUMBS_PADDING: iced::Padding = iced::Padding {
    top: 0.0,
    right: 10.0,
    bottom: 0.0,
    left: 10.0,
};
const PICKER_NOTE_PADDING: [f32; 2] = [9.0, 10.0];
const PICKER_ENTRY_PADDING: [f32; 2] = [0.0, 10.0];
const PICKER_ENTRY_HEIGHT: f32 = 40.0;
const PICKER_ENTRY_GAP: f32 = 10.0;
const PICKER_CHEVRON: f32 = 16.0;
const CRUMB_GAP: f32 = 2.0;
const CRUMB_PADDING: [f32; 2] = [2.0, 2.0];
const WARNING_DISC: f32 = 40.0;
const WARNING_GAP: f32 = 14.0;
const WARNING_LINE_GAP: f32 = 6.0;

#[derive(Debug, Clone)]
pub enum Message {
    NameChanged(String),
    Submit,
    Cancel,
    Enter(Node),
    Up,
    /// Goes back to the folder at this depth of the trail, Home being the first.
    Crumb(usize),
    Folders(Option<NodeId>, Result<Vec<Node>, ApiError>),
    Finished(Result<(), ApiError>),
    /// The outcome of a move or restore that may have been held back by clashing files.
    Placed(Result<MoveResponse, ApiError>),
    /// How to resolve the clashes listed by a conflicts dialog.
    Resolve(OnConflict),
}

/// What the browser should do after a dialog update.
pub enum Outcome {
    Task(Task<Message>),
    Closed,
    Done,
    /// A restore went through; the response says what came back and where to.
    Restored(Box<MoveResponse>),
    Failed,
    /// The user chose to drop the viewer's unsaved edits.
    Discarded,
}

#[derive(Clone)]
enum Kind {
    NewFolder {
        name: String,
        /// The name of the folder it goes in.
        parent: String,
    },
    Rename {
        node: Node,
        name: String,
    },
    Move {
        nodes: Vec<Node>,
        lost_and_found: NodeId,
        trail: Vec<Node>,
        folders: Option<Vec<Node>>,
    },
    Delete {
        nodes: Vec<Node>,
    },
    /// Confirms permanently deleting the `count` files in the recycle bin.
    EmptyTrash {
        count: usize,
    },
    /// Asks before leaving the viewer's unsaved edits to file `name`.
    DiscardEdits {
        name: String,
    },
    /// Files that clash at a destination; the user decides how to go on.
    Conflicts {
        title: String,
        conflicts: Vec<Conflict>,
        /// What to send again with the chosen mode.
        retry: Retry,
        /// The dialog that held the request, shown again on cancel; none closes the dialog.
        back: Option<Box<Kind>>,
    },
}

/// A request that was held back by clashes and is sent again with a mode.
#[derive(Clone)]
enum Retry {
    Move {
        ids: Vec<NodeId>,
        destination: Option<NodeId>,
    },
    /// Move binned files back to Lost & Found.
    Restore { ids: Vec<NodeId> },
}

impl Retry {
    fn send(&self, client: &Client, on_conflict: OnConflict) -> Task<Message> {
        let client = client.clone();
        match self.clone() {
            Retry::Move { ids, destination } => Task::perform(
                async move { client.move_nodes(&ids, destination, on_conflict).await },
                Message::Placed,
            ),
            Retry::Restore { ids } => Task::perform(
                async move { client.restore_trashed(&ids, on_conflict).await },
                Message::Placed,
            ),
        }
    }
}

/// A modal dialog over the browser.
pub struct Dialog {
    kind: Kind,
    saving: bool,
    error: Option<String>,
}

impl Dialog {
    fn with(kind: Kind) -> Self {
        Self {
            kind,
            saving: false,
            error: None,
        }
    }

    /// Asks for the name of a new folder inside the folder named `parent`.
    pub fn new_folder(parent: &str) -> (Self, Task<Message>) {
        let dialog = Self::with(Kind::NewFolder {
            name: String::new(),
            parent: parent.to_owned(),
        });
        (dialog, operation::focus(NAME_INPUT))
    }

    /// Asks for a new name for `node`, with the current name selected.
    pub fn rename(node: Node) -> (Self, Task<Message>) {
        let name = node.name.clone();
        let dialog = Self::with(Kind::Rename { node, name });
        let task = Task::batch([
            operation::focus(NAME_INPUT),
            operation::select_all(NAME_INPUT),
        ]);
        (dialog, task)
    }

    /// Lets the user pick a destination folder for `nodes`.
    pub fn move_nodes(
        client: &Client,
        nodes: Vec<Node>,
        lost_and_found: NodeId,
    ) -> (Self, Task<Message>) {
        let task = list_folders(client, None);
        let dialog = Self::with(Kind::Move {
            nodes,
            lost_and_found,
            trail: Vec::new(),
            folders: None,
        });
        (dialog, task)
    }

    /// Confirms deleting `nodes`.
    pub fn delete(nodes: Vec<Node>) -> Self {
        Self::with(Kind::Delete { nodes })
    }

    /// Confirms permanently deleting the `count` files in the recycle bin.
    pub fn empty_trash(count: usize) -> Self {
        Self::with(Kind::EmptyTrash { count })
    }

    /// Asks whether to drop the unsaved edits to file `name`.
    pub fn discard_edits(name: String) -> Self {
        Self::with(Kind::DiscardEdits { name })
    }

    /// Lists the files that a restore of `ids` could not put back, as `response` reports.
    pub fn restore_conflicts(ids: Vec<NodeId>, response: MoveResponse) -> Self {
        let title = conflicts_title(response.conflicts.len(), LOST_AND_FOUND);
        Self::with(Kind::Conflicts {
            title,
            conflicts: response.conflicts,
            retry: Retry::Restore { ids },
            back: None,
        })
    }

    /// Applies `message`; `folder` is the folder the browser shows.
    pub fn update(&mut self, client: &Client, folder: Option<NodeId>, message: Message) -> Outcome {
        match message {
            Message::NameChanged(value) => {
                match &mut self.kind {
                    Kind::NewFolder { name, .. } | Kind::Rename { name, .. } => *name = value,
                    Kind::Move { .. }
                    | Kind::Delete { .. }
                    | Kind::EmptyTrash { .. }
                    | Kind::DiscardEdits { .. }
                    | Kind::Conflicts { .. } => {}
                }
                Outcome::Task(Task::none())
            }
            Message::Cancel => match &self.kind {
                Kind::Conflicts {
                    back: Some(back), ..
                } => {
                    self.kind = (**back).clone();
                    self.saving = false;
                    self.error = None;
                    Outcome::Task(Task::none())
                }
                _ => Outcome::Closed,
            },
            Message::Resolve(_) if self.saving => Outcome::Task(Task::none()),
            Message::Resolve(mode) => match &self.kind {
                Kind::Conflicts { retry, .. } => {
                    let task = retry.send(client, mode);
                    self.saving = true;
                    self.error = None;
                    Outcome::Task(task)
                }
                _ => Outcome::Task(Task::none()),
            },
            Message::Submit if matches!(self.kind, Kind::DiscardEdits { .. }) => Outcome::Discarded,
            Message::Submit if self.saving || self.in_place() => Outcome::Task(Task::none()),
            Message::Submit => self.submit(client, folder),
            Message::Enter(target) => self.navigate(client, |trail| trail.push(target)),
            Message::Up => self.navigate(client, |trail| {
                trail.pop();
            }),
            Message::Crumb(depth) => self.navigate(client, |trail| trail.truncate(depth)),
            Message::Folders(target, result) => {
                if let Kind::Move {
                    nodes,
                    lost_and_found,
                    trail,
                    folders,
                } = &mut self.kind
                    && trail.last().map(|folder| folder.id) == target
                {
                    match result {
                        Ok(listing) => *folders = Some(pickable(listing, nodes, *lost_and_found)),
                        Err(err) => self.error = Some(err.to_string()),
                    }
                }
                Outcome::Task(Task::none())
            }
            Message::Placed(Ok(response)) if response.conflicts.is_empty() => match &self.kind {
                Kind::Conflicts {
                    retry: Retry::Restore { .. },
                    ..
                } => Outcome::Restored(Box::new(response)),
                _ => Outcome::Done,
            },
            Message::Placed(Ok(response)) => {
                self.hold_back(response);
                Outcome::Task(Task::none())
            }
            Message::Placed(Err(err)) => self.fail(&err),
            Message::Finished(Ok(())) => Outcome::Done,
            Message::Finished(Err(err)) => self.fail(&err),
        }
    }

    /// Whether the move dialog points at the folder its nodes are already in.
    fn in_place(&self) -> bool {
        matches!(
            &self.kind,
            Kind::Move { nodes, trail, .. }
                if already_in(nodes, trail.last().map(|folder| folder.id))
        )
    }

    fn fail(&mut self, err: &ApiError) -> Outcome {
        self.saving = false;
        self.error = Some(err.to_string());
        Outcome::Failed
    }

    /// Replaces the move dialog with the list of clashes the server reported.
    fn hold_back(&mut self, response: MoveResponse) {
        let Kind::Move { nodes, trail, .. } = &self.kind else {
            return;
        };
        let retry = Retry::Move {
            ids: ids_of(nodes),
            destination: trail.last().map(|folder| folder.id),
        };
        let name = response
            .target
            .as_ref()
            .map_or(HOME, |folder| folder.name.as_str());
        let title = conflicts_title(response.conflicts.len(), &format!("“{name}”"));
        self.kind = Kind::Conflicts {
            title,
            conflicts: response.conflicts,
            retry,
            back: Some(Box::new(self.kind.clone())),
        };
        self.saving = false;
        self.error = None;
    }

    fn navigate(&mut self, client: &Client, change: impl FnOnce(&mut Vec<Node>)) -> Outcome {
        if let Kind::Move { trail, folders, .. } = &mut self.kind {
            change(trail);
            *folders = None;
            self.error = None;
            Outcome::Task(list_folders(client, trail.last().map(|folder| folder.id)))
        } else {
            Outcome::Task(Task::none())
        }
    }

    fn submit(&mut self, client: &Client, folder: Option<NodeId>) -> Outcome {
        let client = client.clone();
        let operation: Option<Task<Message>> = match &self.kind {
            Kind::NewFolder { name, .. } => {
                let name = name.trim().to_owned();
                (!name.is_empty()).then(|| {
                    perform(async move { client.create_folder(folder, &name).await.map(drop) })
                })
            }
            Kind::Rename { node, name } => {
                let name = name.trim().to_owned();
                let id = node.id;
                (!name.is_empty() && name != node.name)
                    .then(|| perform(async move { client.rename(id, &name).await.map(drop) }))
            }
            Kind::Move { nodes, trail, .. } => {
                let ids = ids_of(nodes);
                let destination = trail.last().map(|folder| folder.id);
                Some(Retry::Move { ids, destination }.send(&client, OnConflict::Abort))
            }
            Kind::Conflicts { .. } | Kind::DiscardEdits { .. } => None,
            Kind::EmptyTrash { .. } => Some(perform(async move { client.empty_trash().await })),
            Kind::Delete { nodes } => {
                let ids = ids_of(nodes);
                Some(perform(async move {
                    for id in ids {
                        client.delete(id).await?;
                    }
                    Ok(())
                }))
            }
        };
        match operation {
            Some(task) => {
                self.saving = true;
                self.error = None;
                Outcome::Task(task)
            }
            None if matches!(self.kind, Kind::Conflicts { .. }) => Outcome::Task(Task::none()),
            None => Outcome::Closed,
        }
    }

    /// The dialog panel, without the backdrop.
    pub fn view(&self) -> Element<'_, Message> {
        let (width, mut lines, action) = match &self.kind {
            Kind::NewFolder { name, parent } => (
                parts::DIALOG_WIDTH,
                column![
                    parts::dialog_title("New folder"),
                    self.name_field(name, Some(format!("In {parent}"))),
                ],
                self.primary(None, "Create"),
            ),
            Kind::Rename { node, name } => (
                parts::DIALOG_WIDTH,
                column![
                    parts::dialog_title(format!("Rename “{}”", node.name)),
                    self.name_field(name, None),
                ],
                self.primary(None, "Rename"),
            ),
            Kind::Move {
                nodes,
                trail,
                folders,
                ..
            } => {
                let destination = trail.last().map_or(HOME, |folder| folder.name.as_str());
                let action = fit(
                    &format!("Move to {destination}"),
                    theme::TEXT_SIZE,
                    MOVE_ACTION_WIDTH,
                );
                let mut head =
                    column![parts::dialog_title(titled("Move", nodes))].spacing(MOVE_HEAD_GAP);
                if let [_, _, ..] = nodes.as_slice() {
                    let names: Vec<&str> = nodes.iter().map(|node| node.name.as_str()).collect();
                    head = head.push(
                        container(
                            ellipsized(names.join(", "), META_SIZE, Alignment::Start)
                                .width(Length::Fill),
                        )
                        .style(theme::muted),
                    );
                }
                (
                    MOVE_WIDTH,
                    column![head, picker(trail, folders.as_deref())],
                    self.primary(Some(Icon::Move), action),
                )
            }
            Kind::Delete { nodes } => (
                parts::DIALOG_WIDTH,
                column![warning(
                    format!("{} to recycle bin?", titled("Move", nodes)),
                    "Files inside folders are moved too. You can restore them from the recycle \
                     bin."
                        .to_owned(),
                )],
                self.danger("Move to bin"),
            ),
            Kind::EmptyTrash { count } => (
                parts::DIALOG_WIDTH,
                column![warning(
                    "Empty recycle bin".to_owned(),
                    format!(
                        "Permanently delete {}? This cannot be undone.",
                        files(*count)
                    ),
                )],
                self.danger("Empty recycle bin"),
            ),
            Kind::DiscardEdits { name } => (
                parts::DIALOG_WIDTH,
                column![warning(
                    DISCARD_TITLE.to_owned(),
                    format!("Your edits to “{name}” will be lost."),
                )],
                self.danger(DISCARD),
            ),
            Kind::Conflicts {
                title, conflicts, ..
            } => return self.conflicts_view(title, conflicts),
        };
        let cancel = if matches!(self.kind, Kind::DiscardEdits { .. }) {
            KEEP_EDITING
        } else {
            CANCEL
        };
        if let Some(error) = &self.error {
            lines = lines.push(parts::alert_error(Icon::Error, error.as_str(), None));
        }
        parts::dialog(
            width,
            lines,
            [parts::outlined(None, cancel, Some(Message::Cancel)), action],
        )
    }

    /// The label of the button carrying out the dialog, and what pressing it sends.
    fn action(&self, label: impl Into<String>) -> (String, Option<Message>) {
        let label = if self.saving {
            WORKING.to_owned()
        } else {
            label.into()
        };
        let enabled = !self.saving && !self.in_place();
        (label, enabled.then_some(Message::Submit))
    }

    fn primary<'a>(&self, icon: Option<Icon>, label: impl Into<String>) -> Element<'a, Message> {
        let (label, message) = self.action(label);
        parts::primary(icon, label, message)
    }

    fn danger<'a>(&self, label: impl Into<String>) -> Element<'a, Message> {
        let (label, message) = self.action(label);
        parts::danger(label, message)
    }

    fn conflicts_view<'a>(
        &'a self,
        title: &'a str,
        conflicts: &'a [Conflict],
    ) -> Element<'a, Message> {
        let rows = conflicts.iter().enumerate().flat_map(|(index, conflict)| {
            let existing = conflict.existing.as_ref().map_or_else(
                || "Also selected".to_owned(),
                |node| describe(node, short_date),
            );
            let row = container(
                column![
                    text(conflict.source.name.as_str()),
                    text(conflict.path.as_str())
                        .size(META_SIZE)
                        .style(theme::muted_text),
                    text(format!(
                        "Incoming: {}",
                        describe(&conflict.source, short_date)
                    ))
                    .size(META_SIZE)
                    .style(theme::muted_text),
                    text(format!("Existing: {existing}"))
                        .size(META_SIZE)
                        .style(theme::muted_text),
                ]
                .width(Length::Fill),
            )
            .padding(CONFLICT_PADDING)
            .into();
            (index > 0)
                .then(parts::divider)
                .into_iter()
                .chain(std::iter::once(row))
        });
        let list = container(
            container(scrollable(column(rows).width(Length::Fill)).width(Length::Fill))
                .max_height(CONFLICTS_HEIGHT),
        )
        .padding(BOX_PADDING)
        .width(Length::Fill)
        .style(theme::card);
        let choose = |mode| (!self.saving).then_some(Message::Resolve(mode));
        let mut lines = column![
            column![
                parts::dialog_title(title),
                text("Override replaces the current content and keeps it as a version.")
                    .size(META_SIZE)
                    .style(theme::muted_text),
            ]
            .spacing(MOVE_HEAD_GAP),
            list,
        ];
        if let Some(error) = &self.error {
            lines = lines.push(parts::alert_error(Icon::Error, error.as_str(), None));
        }
        parts::dialog(
            CONFLICTS_WIDTH,
            lines,
            [
                parts::outlined(None, CANCEL, Some(Message::Cancel)),
                parts::outlined(None, "Skip these files", choose(OnConflict::Skip)),
                parts::danger(
                    if self.saving { WORKING } else { "Override" },
                    choose(OnConflict::Override),
                ),
            ],
        )
    }

    fn name_field<'a>(&self, name: &'a str, hint: Option<String>) -> Element<'a, Message> {
        parts::field(
            "Name",
            text_input("", name)
                .style(theme::input)
                .id(NAME_INPUT)
                .padding(parts::FIELD_PADDING)
                .on_input_maybe((!self.saving).then_some(Message::NameChanged))
                .on_submit(Message::Submit),
            hint,
        )
    }
}

/// The danger icon next to what is about to happen.
fn warning<'a>(title: String, body: String) -> Element<'a, Message> {
    row![
        container(Icon::Delete.view(ICON_SIZE, Tint::Danger))
            .center(WARNING_DISC)
            .style(theme::danger_disc),
        column![
            parts::dialog_title(title),
            text(body).style(theme::muted_text),
        ]
        .spacing(WARNING_LINE_GAP)
        .width(Length::Fill),
    ]
    .spacing(WARNING_GAP)
    .into()
}

/// The box to pick a destination in: where it is, the way up and the folders inside.
fn picker<'a>(trail: &'a [Node], folders: Option<&'a [Node]>) -> Element<'a, Message> {
    let mut entries = column![];
    if let Some(above) = trail.len().checked_sub(1) {
        let parent = match above.checked_sub(1) {
            Some(index) => trail[index].name.as_str(),
            None => HOME,
        };
        entries = entries.push(picker_entry(
            Icon::Up,
            Tint::Muted,
            format!("Up to {parent}"),
            false,
            Message::Up,
        ));
    }
    entries = match folders {
        None => entries.push(picker_note("Loading…")),
        Some([]) => entries.push(picker_note("No subfolders")),
        Some(folders) => entries.extend(folders.iter().map(|folder| {
            picker_entry(
                Icon::Folder,
                Tint::Accent,
                folder.name.clone(),
                true,
                Message::Enter(folder.clone()),
            )
        })),
    };
    let list = container(
        scrollable(entries.width(Length::Fill))
            .height(Length::Fill)
            .width(Length::Fill)
            .spacing(PICKER_SCROLLBAR_GAP),
    )
    .padding(PICKER_PADDING)
    .height(PICKER_HEIGHT);
    container(column![
        parts::header_bar(crumbs(trail), PICKER_CRUMBS_PADDING),
        list
    ])
    .padding(BOX_PADDING)
    .width(Length::Fill)
    .style(theme::card)
    .into()
}

/// Home and the folders down to where the picker is; each one above it leads back there.
fn crumbs(trail: &[Node]) -> Row<'_, Message> {
    let hidden = collapsed_crumbs(trail.len()).unwrap_or_default();
    let last = trail.len().checked_sub(1);
    let home = match last {
        Some(_) => crumb(HOME, 0),
        None => current_crumb(HOME),
    };
    let mut crumbs = row![home].spacing(CRUMB_GAP);
    for (index, folder) in trail.iter().enumerate() {
        if hidden.contains(&index) {
            if index == hidden.start {
                crumbs = crumbs.push(crumb_text("›")).push(crumb_text("…"));
            }
            continue;
        }
        let name = folder.name.as_str();
        crumbs = crumbs.push(crumb_text("›")).push(if Some(index) == last {
            current_crumb(name)
        } else {
            crumb(name, index + 1)
        });
    }
    crumbs
}

fn crumb(name: &str, depth: usize) -> Element<'_, Message> {
    button(text(name).size(META_SIZE).wrapping(Wrapping::None))
        .padding(CRUMB_PADDING)
        .style(theme::crumb)
        .on_press(Message::Crumb(depth))
        .into()
}

fn current_crumb(name: &str) -> Element<'_, Message> {
    text(name)
        .size(META_SIZE)
        .font(theme::SEMIBOLD)
        .style(theme::strong_text)
        .wrapping(Wrapping::None)
        .into()
}

fn crumb_text(label: &str) -> Element<'_, Message> {
    text(label).size(META_SIZE).style(theme::muted_text).into()
}

fn picker_note<'a>(note: &'a str) -> Element<'a, Message> {
    container(text(note).style(theme::muted_text))
        .padding(PICKER_NOTE_PADDING)
        .into()
}

fn picker_entry<'a>(
    icon: Icon,
    tint: Tint,
    label: String,
    goes_in: bool,
    message: Message,
) -> Element<'a, Message> {
    let mut line = row![
        icon.view(ICON_SIZE, tint),
        ellipsized(label, parts::ROW_SIZE, Alignment::Start).width(Length::Fill),
    ]
    .spacing(PICKER_ENTRY_GAP)
    .height(Length::Fill)
    .align_y(Alignment::Center);
    if goes_in {
        line = line.push(Icon::Chevron.view(PICKER_CHEVRON, Tint::Muted));
    }
    button(line)
        .padding(PICKER_ENTRY_PADDING)
        .width(Length::Fill)
        .height(PICKER_ENTRY_HEIGHT)
        .style(theme::tool)
        .on_press(message)
        .into()
}

fn titled(verb: &str, nodes: &[Node]) -> String {
    match nodes {
        [node] => format!("{verb} “{}”", node.name),
        _ => format!("{verb} {} items", nodes.len()),
    }
}

fn ids_of(nodes: &[Node]) -> Vec<NodeId> {
    nodes.iter().map(|node| node.id).collect()
}

/// The folders of `listing` to offer as destinations: not what is `moving`, nor Lost & Found,
/// which only receives restored files.
fn pickable(listing: Vec<Node>, moving: &[Node], lost_and_found: NodeId) -> Vec<Node> {
    let mut folders: Vec<Node> = listing
        .into_iter()
        .filter(|entry| {
            entry.kind == NodeKind::Folder
                && entry.id != lost_and_found
                && !moving.iter().any(|node| node.id == entry.id)
        })
        .collect();
    sort_nodes(&mut folders, Sort::default());
    folders
}

fn list_folders(client: &Client, target: Option<NodeId>) -> Task<Message> {
    let client = client.clone();
    Task::perform(async move { client.list(target).await }, move |result| {
        Message::Folders(target, result)
    })
}

fn perform(
    operation: impl Future<Output = Result<(), ApiError>> + Send + 'static,
) -> Task<Message> {
    Task::perform(operation, Message::Finished)
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    fn node(name: &str, raw: u128) -> Node {
        Node {
            id: NodeId(Uuid::from_u128(raw)),
            parent_id: None,
            kind: NodeKind::File,
            name: name.to_owned(),
            size: 2048,
            sha256: None,
            mime: String::new(),
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    fn clash(response_target: Option<Node>) -> MoveResponse {
        MoveResponse {
            conflicts: vec![Conflict {
                source: node("a.txt", 1),
                path: "dir/a.txt".to_owned(),
                existing: Some(node("a.txt", 2)),
            }],
            target: response_target,
            ..MoveResponse::default()
        }
    }

    fn client() -> Client {
        Client::new("http://localhost")
    }

    fn moving() -> Dialog {
        Dialog::with(Kind::Move {
            nodes: vec![node("a.txt", 1)],
            lost_and_found: NodeId(Uuid::from_u128(99)),
            trail: Vec::new(),
            folders: Some(Vec::new()),
        })
    }

    #[test]
    fn a_move_held_back_by_clashes_shows_them_with_the_target_name() {
        let mut dialog = moving();
        dialog.saving = true;

        let outcome = dialog.update(
            &client(),
            None,
            Message::Placed(Ok(clash(Some(node("Photos", 3))))),
        );

        assert!(matches!(outcome, Outcome::Task(_)));
        assert!(!dialog.saving);
        let Kind::Conflicts {
            title, conflicts, ..
        } = &dialog.kind
        else {
            panic!("expected the conflicts dialog");
        };
        assert_eq!(title, "1 file already exists in “Photos”");
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn clashes_at_the_root_are_titled_with_home() {
        let mut dialog = moving();

        let _ = dialog.update(&client(), None, Message::Placed(Ok(clash(None))));

        let Kind::Conflicts { title, .. } = &dialog.kind else {
            panic!("expected the conflicts dialog");
        };
        assert_eq!(title, "1 file already exists in “Home”");
    }

    #[test]
    fn a_move_without_clashes_is_done() {
        let mut dialog = moving();

        let outcome = dialog.update(
            &client(),
            None,
            Message::Placed(Ok(MoveResponse::default())),
        );

        assert!(matches!(outcome, Outcome::Done));
    }

    #[test]
    fn cancelling_the_conflicts_returns_to_the_move_dialog() {
        let mut dialog = moving();
        let _ = dialog.update(&client(), None, Message::Placed(Ok(clash(None))));

        let outcome = dialog.update(&client(), None, Message::Cancel);

        assert!(matches!(outcome, Outcome::Task(_)));
        assert!(matches!(dialog.kind, Kind::Move { .. }));
        let outcome = dialog.update(&client(), None, Message::Cancel);
        assert!(matches!(outcome, Outcome::Closed));
    }

    #[test]
    fn choosing_a_mode_sends_the_move_again() {
        let mut dialog = moving();
        let _ = dialog.update(&client(), None, Message::Placed(Ok(clash(None))));

        let outcome = dialog.update(&client(), None, Message::Resolve(OnConflict::Skip));

        assert!(matches!(outcome, Outcome::Task(_)));
        assert!(dialog.saving);
        assert!(matches!(dialog.kind, Kind::Conflicts { .. }));
    }

    #[test]
    fn enter_does_not_resolve_clashes() {
        let mut dialog = moving();
        let _ = dialog.update(&client(), None, Message::Placed(Ok(clash(None))));

        let outcome = dialog.update(&client(), None, Message::Submit);

        assert!(matches!(outcome, Outcome::Task(_)));
        assert!(!dialog.saving);
        assert!(matches!(dialog.kind, Kind::Conflicts { .. }));
    }

    #[test]
    fn a_failed_move_keeps_the_dialog_open_with_the_error() {
        let mut dialog = moving();
        dialog.saving = true;

        let outcome = dialog.update(
            &client(),
            None,
            Message::Placed(Err(ApiError::Unauthorized)),
        );

        assert!(matches!(outcome, Outcome::Failed));
        assert!(!dialog.saving);
        assert!(dialog.error.is_some());
    }

    fn restoring() -> Dialog {
        Dialog::restore_conflicts(vec![NodeId(Uuid::from_u128(1))], clash(None))
    }

    #[test]
    fn lost_and_found_is_not_offered_as_a_destination() {
        let folder = |name, raw| Node {
            kind: NodeKind::Folder,
            ..node(name, raw)
        };
        let found = folder("Lost & Found", 9);
        let moving = folder("moving", 1);
        let other = folder("other", 2);

        let folders = pickable(
            vec![found.clone(), moving.clone(), other.clone()],
            std::slice::from_ref(&moving),
            found.id,
        );

        let ids: Vec<_> = folders.iter().map(|folder| folder.id).collect();
        assert!(ids.contains(&other.id));
        assert!(!ids.contains(&found.id));
        assert!(!ids.contains(&moving.id));
    }

    #[test]
    fn restore_clashes_are_titled_with_lost_and_found() {
        let dialog = restoring();

        let Kind::Conflicts { title, back, .. } = &dialog.kind else {
            panic!("expected the conflicts dialog");
        };
        assert_eq!(title, "1 file already exists in Lost & Found");
        assert!(back.is_none());
    }

    #[test]
    fn cancelling_restore_clashes_closes_the_dialog() {
        let mut dialog = restoring();

        let outcome = dialog.update(&client(), None, Message::Cancel);

        assert!(matches!(outcome, Outcome::Closed));
    }

    #[test]
    fn choosing_a_mode_sends_the_restore_again() {
        let mut dialog = restoring();

        let outcome = dialog.update(&client(), None, Message::Resolve(OnConflict::Override));

        assert!(matches!(outcome, Outcome::Task(_)));
        assert!(dialog.saving);
    }

    #[test]
    fn a_restore_without_clashes_reports_what_came_back() {
        let mut dialog = restoring();
        dialog.saving = true;

        let outcome = dialog.update(
            &client(),
            None,
            Message::Placed(Ok(MoveResponse::default())),
        );

        assert!(matches!(outcome, Outcome::Restored(_)));
    }

    #[test]
    fn a_crumb_goes_back_to_its_folder() {
        let mut dialog = Dialog::with(Kind::Move {
            nodes: vec![node("a.txt", 1)],
            lost_and_found: NodeId(Uuid::from_u128(99)),
            trail: vec![node("Documents", 2), node("Invoices", 3)],
            folders: Some(Vec::new()),
        });

        let _ = dialog.update(&client(), None, Message::Crumb(1));

        let Kind::Move { trail, folders, .. } = &dialog.kind else {
            panic!("expected the move dialog");
        };
        assert_eq!(trail.len(), 1);
        assert_eq!(trail[0].name, "Documents");
        assert!(folders.is_none());
    }

    #[test]
    fn the_move_button_names_the_destination_and_the_way_up_names_the_parent() {
        let dialog = Dialog::with(Kind::Move {
            nodes: vec![node("a.txt", 1), node("b.txt", 4)],
            lost_and_found: NodeId(Uuid::from_u128(99)),
            trail: vec![node("Documents", 2), node("Invoices", 3)],
            folders: Some(Vec::new()),
        });
        let mut ui = iced_test::simulator(dialog.view());

        assert!(ui.find("Move 2 items").is_ok());
        assert!(ui.find("Move to Invoices").is_ok());
        assert!(ui.find("Up to Documents").is_ok());
    }

    #[test]
    fn a_new_folder_says_where_it_goes() {
        let (dialog, _task) = Dialog::new_folder("Documents");
        let mut ui = iced_test::simulator(dialog.view());

        assert!(ui.find("In Documents").is_ok());
    }

    #[test]
    fn deleting_offers_the_recycle_bin() {
        let dialog = Dialog::delete(vec![node("a.txt", 1)]);
        let mut ui = iced_test::simulator(dialog.view());

        assert!(ui.find("Move “a.txt” to recycle bin?").is_ok());
        assert!(ui.find("Move to bin").is_ok());
        assert!(ui.find("Delete").is_err());
    }

    #[test]
    fn deleting_lost_and_found_shows_the_servers_message_in_the_dialog() {
        let mut dialog = Dialog::delete(vec![node("Lost & Found", 1)]);
        dialog.saving = true;
        let err = ApiError::Api {
            code: protocol::ErrorCode::Conflict,
            message: "Lost & Found cannot be deleted".to_owned(),
        };

        let outcome = dialog.update(&client(), None, Message::Finished(Err(err)));

        assert!(matches!(outcome, Outcome::Failed));
        assert_eq!(
            dialog.error.as_deref(),
            Some("Lost & Found cannot be deleted")
        );
    }

    #[test]
    fn emptying_the_bin_asks_for_confirmation_with_the_count() {
        let mut dialog = Dialog::empty_trash(3);
        {
            let mut ui = iced_test::simulator(dialog.view());
            assert!(
                ui.find("Permanently delete 3 files? This cannot be undone.")
                    .is_ok()
            );
        }

        let outcome = dialog.update(&client(), None, Message::Submit);

        assert!(matches!(outcome, Outcome::Task(_)));
        assert!(dialog.saving);
    }
}
