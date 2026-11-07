mod anchor;
mod context_menu;
mod details;
mod devices;
mod dialogs;
mod files;
mod fit_column;
mod folder;
mod grid;
mod input;
mod layout;
mod lost_and_found;
pub(crate) mod parts;
mod search;
mod selection;
mod sidebar;
mod sync_queue;
mod syncing;
mod toolbar;
mod trash;

use std::path::{Path, PathBuf};

use iced::{
    Size, Subscription, Task, event,
    keyboard::Modifiers,
    widget::{
        operation,
        scrollable::{AbsoluteOffset, RelativeOffset, Viewport},
    },
    window,
};

use api_client::{ApiError, Client};
use client_core::{labels::HOME, listing::SortKey};
use protocol::{Me, Node, NodeId, NodeKind, paths};
use uuid::Uuid;

use self::{
    context_menu::Placement,
    dialogs::{Dialog, Outcome},
    folder::{Folder, Listing},
    input::{Shortcut, input},
    lost_and_found::LostAndFound,
    search::Search,
    syncing::Syncing,
    toolbar::Action,
};
use crate::{
    events, sync,
    transfers::{self, Batch, Direction, DownloadFile, Transfers, UploadPlan},
};

const DEFAULT_WINDOW: Size = Size::new(1280.0, 800.0);

#[derive(Debug, Clone)]
pub enum Message {
    /// Opens a folder from the breadcrumbs, which may be any folder; going back returns to the
    /// folder it was left from.
    Jump(Option<NodeId>),
    /// Shows the files again from another page or, already there, goes to Home.
    OpenFiles,
    OpenLostAndFound,
    /// The contents of Lost & Found, which decide whether the sidebar offers it.
    LostAndFoundLoaded(Result<Vec<Node>, ApiError>),
    Refresh,
    Loaded(Option<NodeId>, Result<Listing, ApiError>),
    Click(NodeId),
    /// A click on the icon or name of a folder's row.
    OpenFolderLink(NodeId),
    /// Where the rows of the listing are scrolled.
    GridScrolled(Viewport),
    Toggle(NodeId),
    ToggleAll,
    ClearSelection,
    SortBy(SortKey),
    ContextMenu(NodeId, Placement),
    CrumbsMenu(Placement),
    AccountMenu(Placement),
    CloseMenu,
    Act(Action),
    NewFolder,
    Upload,
    UploadPlanned(Option<NodeId>, Result<UploadPlan, String>),
    DownloadPlanned(Result<Vec<DownloadFile>, String>),
    Dialog(dialogs::Message),
    Details(details::Message),
    OpenTrash,
    Trash(trash::Message),
    OpenSyncQueue,
    OpenSyncSettings,
    OpenDevices,
    Devices(devices::Message),
    Transfer(transfers::Message),
    Shortcut(Shortcut),
    Modifiers(Modifiers),
    Resized(Size),
    FileHovered,
    FileDropped(PathBuf),
    FilesHoveredLeft,
    Server(protocol::Event),
    Search(search::Message),
    SignOut,
    SignedOut,
    Sync(syncing::Message),
    OpenInWeb(Option<NodeId>),
}

pub enum Update {
    Task(Task<Message>),
    /// Signed out of `server`, which the login screen offers again.
    SignedOut {
        server: String,
    },
}

/// A popup menu and what it hangs from.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Menu {
    Actions(Placement),
    Crumbs(Placement),
    Account(Placement),
    Device(Uuid, Placement),
}

/// What the browser shows below the header, other than search results.
enum Page {
    Folder,
    /// The recycle bin.
    Trash(Box<trash::State>),
    /// What the sync has left to do.
    SyncQueue,
    /// What syncs where.
    SyncSettings,
    /// The user's devices.
    Devices(Box<devices::State>),
}

pub struct State {
    client: Client,
    me: Me,
    folder: Folder,
    /// The folder a breadcrumb jump left, which going back returns to.
    previous: Option<Option<NodeId>>,
    modifiers: Modifiers,
    window: Size,
    menu: Option<Menu>,
    dialog: Option<Dialog>,
    details: Option<details::State>,
    page: Page,
    transfers: Transfers,
    dropping: bool,
    search: Search,
    syncing: Syncing,
    lost_and_found: LostAndFound,
}

impl State {
    pub fn open(client: Client, me: Me) -> (Self, Task<Message>) {
        let state = Self {
            client,
            folder: Folder::new(me.lost_and_found_id),
            lost_and_found: LostAndFound::new(me.lost_and_found_id),
            me,
            previous: None,
            modifiers: Modifiers::default(),
            window: DEFAULT_WINDOW,
            menu: None,
            dialog: None,
            details: None,
            page: Page::Folder,
            transfers: Transfers::default(),
            dropping: false,
            search: Search::default(),
            syncing: Syncing::default(),
        };
        // The window may have been resized before the browser opened.
        let size = window::latest()
            .and_then(window::size)
            .map(Message::Resized);
        let task = Task::batch([
            state.folder.load(&state.client),
            state.load_lost_and_found(),
            size,
        ]);
        (state, task)
    }

    pub fn update(&mut self, message: Message) -> Update {
        match message {
            Message::SignedOut => Update::SignedOut {
                server: self.client.base_url().to_owned(),
            },
            message => {
                let task = self.handle(message);
                Update::Task(Task::batch([task, self.follow_selection()]))
            }
        }
    }

    fn follow_selection(&mut self) -> Task<Message> {
        let Some(open) = self.details.as_ref().map(details::State::node_id) else {
            return Task::none();
        };
        match self.folder.selected().as_slice() {
            [node] if node.id != open => {
                let (details, task) = details::State::open(&self.client, node.clone());
                self.details = Some(details);
                task.map(Message::Details)
            }
            _ => Task::none(),
        }
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Jump(folder) => {
                let from = self.folder.id;
                let task = self.open_folder(folder);
                self.previous = (from != folder).then_some(from);
                task
            }
            Message::OpenFiles => {
                // Lost & Found has its own place, so the files never return to it.
                if (matches!(self.page, Page::Folder) && !self.search.is_active())
                    || self.within_lost_and_found()
                {
                    self.handle(Message::Jump(None))
                } else {
                    self.open_folder(self.folder.id)
                }
            }
            Message::OpenLostAndFound => self.handle(Message::Jump(Some(self.lost_and_found.id()))),
            Message::LostAndFoundLoaded(result) => {
                // On failure the place stays as it was until the next refresh or resync.
                if let Ok(children) = result {
                    self.lost_and_found.loaded(&children);
                }
                Task::none()
            }
            Message::Refresh => {
                Task::batch([self.folder.reload(&self.client), self.load_lost_and_found()])
            }
            Message::Loaded(folder, result) => {
                self.folder.loaded(folder, result);
                Task::none()
            }
            Message::Click(id) => self.click(id),
            Message::OpenFolderLink(id) => self.open_folder_link(id),
            Message::GridScrolled(viewport) => {
                self.folder.viewport = Some(viewport);
                Task::none()
            }
            Message::Toggle(id) => {
                self.folder.selection.toggle(id);
                Task::none()
            }
            Message::ToggleAll => {
                self.folder.selection.toggle_all(&self.folder.listing.nodes);
                Task::none()
            }
            Message::ClearSelection => {
                self.folder.selection.clear();
                Task::none()
            }
            Message::SortBy(key) => {
                self.folder.sort_by(key);
                Task::none()
            }
            Message::ContextMenu(id, placement) => {
                self.folder.selection.ensure(id);
                self.menu = Some(Menu::Actions(placement));
                Task::none()
            }
            Message::CrumbsMenu(placement) => {
                self.menu = Some(Menu::Crumbs(placement));
                Task::none()
            }
            Message::AccountMenu(placement) => {
                self.menu = Some(Menu::Account(placement));
                Task::none()
            }
            Message::CloseMenu => {
                self.menu = None;
                Task::none()
            }
            Message::Act(action) => {
                self.menu = None;
                self.act(action)
            }
            Message::NewFolder if self.in_lost_and_found() => Task::none(),
            Message::NewFolder => {
                let (dialog, task) = Dialog::new_folder(self.folder_name());
                self.dialog = Some(dialog);
                task.map(Message::Dialog)
            }
            Message::Upload => self.pick_upload(),
            Message::UploadPlanned(parent, Ok(plan)) if !plan.is_empty() => self
                .transfers
                .enqueue(&self.client, Batch::Upload { parent, plan })
                .map(Message::Transfer),
            Message::DownloadPlanned(Ok(files)) if !files.is_empty() => self
                .transfers
                .enqueue(&self.client, Batch::Download(files))
                .map(Message::Transfer),
            Message::UploadPlanned(_, Err(err)) | Message::DownloadPlanned(Err(err)) => {
                self.folder.error = Some(err);
                Task::none()
            }
            Message::UploadPlanned(..) | Message::DownloadPlanned(_) => Task::none(),
            Message::Dialog(message) => self.dialog_update(message),
            Message::Details(message) => self.details_update(message),
            Message::OpenTrash => self.open_trash(),
            Message::Trash(message) => self.trash_update(message),
            Message::OpenSyncQueue => self.open_sync_page(Page::SyncQueue),
            Message::OpenSyncSettings => self.open_sync_page(Page::SyncSettings),
            Message::OpenDevices => self.open_devices(),
            Message::Devices(devices::Message::Menu(id, placement)) => {
                self.menu = Some(Menu::Device(id, placement));
                Task::none()
            }
            Message::Devices(message) => {
                self.menu = None;
                self.devices_update(message)
            }
            Message::Transfer(message) => {
                let reload = matches!(message, transfers::Message::BatchDone(Direction::Upload));
                let task = self
                    .transfers
                    .update(&self.client, message)
                    .map(Message::Transfer);
                if reload {
                    Task::batch([task, self.folder.load(&self.client)])
                } else {
                    task
                }
            }
            Message::Shortcut(shortcut) => self.shortcut(shortcut),
            Message::Modifiers(modifiers) => {
                self.modifiers = modifiers;
                Task::none()
            }
            Message::Resized(size) => {
                self.window = size;
                Task::none()
            }
            Message::FileHovered => {
                self.dropping = self.dialog.is_none() && !self.in_lost_and_found();
                Task::none()
            }
            Message::FilesHoveredLeft => {
                self.dropping = false;
                Task::none()
            }
            Message::FileDropped(path) => {
                self.dropping = false;
                if self.dialog.is_none() && !self.in_lost_and_found() {
                    files::plan_upload(self.folder.id, vec![path])
                } else {
                    Task::none()
                }
            }
            Message::Server(event) => {
                self.syncing
                    .send(sync::Command::RemoteChanged(event.clone()));
                let details = self.sync_details(&event);
                let found = if self.lost_and_found.apply(&event) {
                    self.load_lost_and_found()
                } else {
                    Task::none()
                };
                let listing = if self.folder.apply(&event) {
                    self.folder.load(&self.client)
                } else {
                    Task::none()
                };
                let trash = match (&mut self.page, &event) {
                    (
                        Page::Trash(trash),
                        protocol::Event::TrashChanged | protocol::Event::Resync,
                    ) => trash.load(&self.client).map(Message::Trash),
                    (
                        Page::Devices(devices),
                        protocol::Event::DevicesChanged | protocol::Event::Resync,
                    ) => devices.load(&self.client).map(Message::Devices),
                    _ => Task::none(),
                };
                Task::batch([details, found, listing, trash])
            }
            Message::Sync(message) => {
                if matches!(
                    message,
                    syncing::Message::Pick(_)
                        | syncing::Message::Set(_)
                        | syncing::Message::Unmap(_)
                        | syncing::Message::OpenLocal(_)
                ) {
                    self.menu = None;
                }
                match self
                    .syncing
                    .update(message, |id| self.folder.listing.name_of(id))
                {
                    syncing::Outcome::Task(task) => task.map(Message::Sync),
                    syncing::Outcome::Error(err) => {
                        self.folder.error = Some(err);
                        Task::none()
                    }
                }
            }
            Message::OpenInWeb(folder) => {
                self.menu = None;
                let url = self.client.url(&paths::web_folder(folder));
                if let Err(err) = syncing::open_url(&url) {
                    self.folder.error = Some(err);
                }
                Task::none()
            }
            Message::Search(message) => match self.search.update(&self.client, message) {
                search::Outcome::Task(task) => task.map(Message::Search),
                search::Outcome::OpenFolder(folder) => self.open_folder(folder),
            },
            Message::SignOut => {
                self.menu = None;
                let client = self.client.clone();
                Task::perform(
                    async move {
                        let _ = client.logout().await;
                    },
                    |()| Message::SignedOut,
                )
            }
            Message::SignedOut => Task::none(),
        }
    }

    fn open_folder(&mut self, folder: Option<NodeId>) -> Task<Message> {
        self.previous = None;
        self.folder.open(folder);
        self.page = Page::Folder;
        self.search.clear();
        self.details = None;
        self.menu = None;
        Task::batch([
            self.folder.load(&self.client),
            operation::snap_to(grid::ROWS, RelativeOffset::START),
        ])
    }

    /// Leaves another page, such as the recycle bin; in a folder, goes to the one a breadcrumb
    /// jump left or else to the parent.
    fn back(&mut self) -> Task<Message> {
        if !matches!(self.page, Page::Folder) {
            self.open_folder(self.folder.id)
        } else if let Some(previous) = self.previous {
            self.open_folder(previous)
        } else if let Some(parent) = self.folder.parent() {
            self.open_folder(parent)
        } else {
            Task::none()
        }
    }

    fn open_trash(&mut self) -> Task<Message> {
        self.search.clear();
        self.folder.error = None;
        self.details = None;
        self.menu = None;
        self.folder.selection.clear();
        let mut trash = trash::State::open();
        let task = trash.load(&self.client);
        self.page = Page::Trash(Box::new(trash));
        task.map(Message::Trash)
    }

    fn open_sync_page(&mut self, page: Page) -> Task<Message> {
        self.search.clear();
        self.folder.error = None;
        self.details = None;
        self.menu = None;
        self.folder.selection.clear();
        self.page = page;
        Task::none()
    }

    fn open_devices(&mut self) -> Task<Message> {
        self.search.clear();
        self.folder.error = None;
        self.details = None;
        self.menu = None;
        self.folder.selection.clear();
        let mut devices = devices::State::open();
        let task = devices.load(&self.client);
        self.page = Page::Devices(Box::new(devices));
        task.map(Message::Devices)
    }

    fn devices_update(&mut self, message: devices::Message) -> Task<Message> {
        match &mut self.page {
            Page::Devices(devices) => devices.update(&self.client, message).map(Message::Devices),
            _ => Task::none(),
        }
    }

    fn trash_update(&mut self, message: trash::Message) -> Task<Message> {
        let Page::Trash(trash) = &mut self.page else {
            return Task::none();
        };
        match trash.update(&self.client, self.modifiers, message) {
            trash::Outcome::Task(task) => task.map(Message::Trash),
            trash::Outcome::Conflicts { ids, response } => {
                self.dialog = Some(Dialog::restore_conflicts(ids, *response));
                Task::none()
            }
            trash::Outcome::ConfirmEmpty(count) => {
                self.dialog = Some(Dialog::empty_trash(count));
                Task::none()
            }
            trash::Outcome::OpenFolder(id) => self.open_folder(Some(id)),
        }
    }

    /// Fetches the listing the browser currently shows.
    fn reload(&mut self) -> Task<Message> {
        match &mut self.page {
            Page::Trash(trash) => trash.load(&self.client).map(Message::Trash),
            Page::Devices(devices) => devices.load(&self.client).map(Message::Devices),
            Page::Folder | Page::SyncQueue | Page::SyncSettings => self.folder.reload(&self.client),
        }
    }

    fn click(&mut self, id: NodeId) -> Task<Message> {
        match self.folder.click(id, self.modifiers) {
            Some(id) => self.activate(id),
            None => Task::none(),
        }
    }

    /// Opens the folder, unless a modifier asks for selecting it like a click on the rest of the
    /// row does.
    fn open_folder_link(&mut self, id: NodeId) -> Task<Message> {
        if self.modifiers.command() || self.modifiers.shift() {
            self.click(id)
        } else if self.folder.follow_link() {
            self.activate(id)
        } else {
            Task::none()
        }
    }

    /// Moves a single selection one row, keeping it in view.
    fn step_selection(&mut self, forward: bool) -> Task<Message> {
        let offset = self.folder.step(forward);
        scroll_rows_to(offset)
    }

    /// Grows or shrinks the selection one row, keeping the moved end in view.
    fn extend_selection(&mut self, forward: bool) -> Task<Message> {
        let offset = self.folder.extend(forward);
        scroll_rows_to(offset)
    }

    fn activate(&mut self, id: NodeId) -> Task<Message> {
        let node = self.folder.node(id).cloned();
        match node {
            Some(node) if node.kind == NodeKind::Folder => self.open_folder(Some(node.id)),
            Some(node) => self.open_file(&node),
            None => Task::none(),
        }
    }

    /// Opens the file `node` with the default app, from a local copy that is the server's version
    /// or else from a fresh download.
    fn open_file(&self, node: &Node) -> Task<Message> {
        let synced = self.local_copy(node).map(|path| files::SyncedCopy {
            path,
            base: self.syncing.status().base(node.id).cloned(),
        });
        files::open(&self.client, node.clone(), synced)
    }

    /// Where the synced file `node` is on disk, once it is there.
    fn local_copy(&self, node: &Node) -> Option<PathBuf> {
        self.syncing
            .status()
            .file_path(node.id)
            .filter(|path| path.is_file())
            .map(Path::to_path_buf)
    }

    fn act(&mut self, action: Action) -> Task<Message> {
        let selected = self.folder.selected();
        match (action, selected.as_slice()) {
            (_, []) => Task::none(),
            (Action::Open, [node]) => self.activate(node.id),
            (Action::OpenFile, [node]) => self.open_file(node),
            (Action::Rename, [node]) => {
                let (dialog, task) = Dialog::rename(node.clone());
                self.dialog = Some(dialog);
                task.map(Message::Dialog)
            }
            (Action::Details, [node]) => {
                let (details, task) = details::State::open(&self.client, node.clone());
                self.details = Some(details);
                task.map(Message::Details)
            }
            (Action::Open | Action::OpenFile | Action::Rename | Action::Details, _) => Task::none(),
            (Action::Download, _) => files::download(&self.client, selected),
            (Action::Move, _) => {
                let (dialog, task) =
                    Dialog::move_nodes(&self.client, selected, self.me.lost_and_found_id);
                self.dialog = Some(dialog);
                task.map(Message::Dialog)
            }
            (Action::Delete, _)
                if selected
                    .iter()
                    .any(|node| node.id == self.me.lost_and_found_id) =>
            {
                Task::none()
            }
            (Action::Delete, _) => {
                self.dialog = Some(Dialog::delete(selected));
                Task::none()
            }
        }
    }

    /// The name of the folder shown.
    fn folder_name(&self) -> &str {
        self.folder
            .listing
            .path
            .last()
            .map_or(HOME, |folder| folder.name.as_str())
    }

    fn details_update(&mut self, message: details::Message) -> Task<Message> {
        let Some(details) = &mut self.details else {
            return Task::none();
        };
        match message {
            details::Message::Open => {
                let node = details.node().clone();
                return match node.kind {
                    NodeKind::Folder => self.open_folder(Some(node.id)),
                    NodeKind::File => self.open_file(&node),
                };
            }
            details::Message::DownloadNode => {
                return files::download(&self.client, vec![details.node().clone()]);
            }
            _ => {}
        }
        match details.update(&self.client, message) {
            Some(task) => task.map(Message::Details),
            None => {
                self.details = None;
                Task::none()
            }
        }
    }

    fn sync_details(&mut self, event: &protocol::Event) -> Task<Message> {
        let Some(details) = &self.details else {
            return Task::none();
        };
        let id = details.node_id();
        match event {
            protocol::Event::NodeDeleted { id: deleted, .. } if *deleted == id => {
                self.details = None;
                Task::none()
            }
            protocol::Event::NodeUpdated { node } if node.id == id => {
                details.refresh(&self.client).map(Message::Details)
            }
            protocol::Event::Resync => details.refresh(&self.client).map(Message::Details),
            _ => Task::none(),
        }
    }

    fn dialog_update(&mut self, message: dialogs::Message) -> Task<Message> {
        let Some(dialog) = &mut self.dialog else {
            return Task::none();
        };
        match dialog.update(&self.client, self.folder.id, message) {
            Outcome::Task(task) => task.map(Message::Dialog),
            Outcome::Closed => {
                self.dialog = None;
                Task::none()
            }
            Outcome::Done => {
                self.dialog = None;
                self.folder.selection.clear();
                if let Page::Trash(trash) = &mut self.page {
                    trash.clear_selection();
                }
                self.reload()
            }
            Outcome::Restored(response) => {
                self.dialog = None;
                if let Page::Trash(trash) = &mut self.page {
                    trash.restored_with(&response);
                }
                self.reload()
            }
            Outcome::Failed => self.reload(),
        }
    }

    fn shortcut(&mut self, shortcut: Shortcut) -> Task<Message> {
        if self.dialog.is_some() {
            match shortcut {
                Shortcut::Escape => self.dialog_update(dialogs::Message::Cancel),
                Shortcut::Activate => self.dialog_update(dialogs::Message::Submit),
                _ => Task::none(),
            }
        } else if shortcut == Shortcut::Search {
            // The search box is on the files.
            let files = if matches!(self.page, Page::Folder) {
                self.menu = None;
                Task::none()
            } else {
                self.open_folder(self.folder.id)
            };
            Task::batch([files, operation::focus(layout::SEARCH_INPUT)])
        } else if self.menu.take().is_some() && shortcut == Shortcut::Escape {
            Task::none()
        } else if self.search.is_active() {
            if matches!(shortcut, Shortcut::Escape | Shortcut::Back) {
                self.search.clear();
            }
            Task::none()
        } else if let Page::Trash(trash) = &mut self.page {
            match shortcut {
                Shortcut::Escape => trash.clear_selection(),
                Shortcut::SelectAll => trash.select_all(),
                Shortcut::Refresh => return self.reload(),
                Shortcut::Back => return self.back(),
                Shortcut::Parent => return self.open_folder(self.folder.id),
                _ => {}
            }
            Task::none()
        } else if let Page::Devices(devices) = &mut self.page {
            match shortcut {
                Shortcut::Escape if devices.is_asking() => {
                    self.devices_update(devices::Message::Cancel)
                }
                Shortcut::Refresh => self.reload(),
                Shortcut::Escape | Shortcut::Back | Shortcut::Parent => {
                    self.open_folder(self.folder.id)
                }
                _ => Task::none(),
            }
        } else if matches!(self.page, Page::SyncQueue | Page::SyncSettings) {
            match shortcut {
                Shortcut::Escape | Shortcut::Back | Shortcut::Parent => {
                    self.open_folder(self.folder.id)
                }
                _ => Task::none(),
            }
        } else {
            match shortcut {
                Shortcut::Escape => {
                    self.folder.selection.clear();
                    Task::none()
                }
                Shortcut::SelectAll => {
                    self.folder.selection.select_all(&self.folder.listing.nodes);
                    Task::none()
                }
                Shortcut::Delete => self.act(Action::Delete),
                Shortcut::Rename => self.act(Action::Rename),
                Shortcut::Activate | Shortcut::Open => match self.folder.selected().as_slice() {
                    [node] => self.activate(node.id),
                    _ => Task::none(),
                },
                Shortcut::Back => self.back(),
                Shortcut::Parent => match self.folder.parent() {
                    Some(parent) => self.open_folder(parent),
                    None => Task::none(),
                },
                Shortcut::NewFolder => self.handle(Message::NewFolder),
                Shortcut::Upload => self.pick_upload(),
                Shortcut::Refresh => self.handle(Message::Refresh),
                Shortcut::Search => Task::none(),
                Shortcut::SelectPrevious => self.step_selection(false),
                Shortcut::SelectNext => self.step_selection(true),
                Shortcut::ExtendPrevious => self.extend_selection(false),
                Shortcut::ExtendNext => self.extend_selection(true),
            }
        }
    }

    /// Lost & Found only receives restored files, so nothing is created or uploaded into it.
    fn in_lost_and_found(&self) -> bool {
        self.folder.id == Some(self.me.lost_and_found_id)
    }

    /// Whether the folder shown is Lost & Found or inside it.
    fn within_lost_and_found(&self) -> bool {
        let found = self.me.lost_and_found_id;
        self.in_lost_and_found()
            || (self.folder.id.is_some()
                && self
                    .folder
                    .listing
                    .path
                    .iter()
                    .any(|folder| folder.id == found))
    }

    fn load_lost_and_found(&self) -> Task<Message> {
        let client = self.client.clone();
        let found = self.lost_and_found.id();
        Task::perform(
            async move { client.list(Some(found)).await },
            Message::LostAndFoundLoaded,
        )
    }

    fn pick_upload(&self) -> Task<Message> {
        if self.in_lost_and_found() {
            return Task::none();
        }
        files::pick_upload(self.folder.id)
    }

    /// Input events and live server events while the browser is shown.
    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            event::listen_with(input),
            events::subscription(&self.client).map(Message::Server),
            sync::subscription(&self.client, self.me.id)
                .map(|update| Message::Sync(syncing::Message::Engine(update))),
        ])
    }
}

/// Scrolls the rows to `offset`, when there is one.
fn scroll_rows_to(offset: Option<f32>) -> Task<Message> {
    match offset {
        Some(y) => operation::scroll_to(
            grid::ROWS,
            AbsoluteOffset {
                x: None,
                y: Some(y),
            },
        ),
        None => Task::none(),
    }
}
#[cfg(test)]
mod tests;
