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
mod people_picker;
mod protect;
mod search;
mod selection;
mod send;
mod share;
mod shared;
mod sidebar;
mod sync_queue;
mod syncing;
mod toolbar;
mod transfer_list;
mod transfer_save;
mod trash;
mod version;
mod viewer;
mod visibility;

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
use client_core::{
    gallery::Gallery,
    labels::HOME,
    listing::{Sort, SortKey},
    send::files_to_upload,
    viewers,
};
use protocol::{Me, Node, NodeId, NodeKind, ProtectionState, TrashItem, paths};
use uuid::Uuid;

use self::{
    context_menu::Placement,
    dialogs::{Dialog, Outcome, Within},
    folder::{Folder, Listing},
    input::{Shortcut, input},
    lost_and_found::LostAndFound,
    search::Search,
    syncing::Syncing,
    toolbar::{Action, Limits},
    transfer_list::TransferList,
};
use crate::{
    app_link::AppLink,
    events,
    protected::{
        self, Inside, LOCK_CHECK, LOCKED_MEANWHILE, Protected, Protection,
        convert::{self, Stopped},
    },
    sync,
    transfers::{self, Batch, Direction, DownloadFile, Transfers, UploadPlan},
    viewers::viewers,
};

const DEFAULT_WINDOW: Size = Size::new(1280.0, 800.0);
/// How often the Transfers page says the times left again.
const CLOCK_TICK: std::time::Duration = std::time::Duration::from_secs(60);
const LOCKED: &str = "This folder is locked. Unlock it first.";
const FROZEN: &str =
    "This folder is being unprotected; nothing can change in it until that is done.";
const ENCRYPTING: &str = "Encrypting the files of this folder…";
const DECRYPTING: &str = "Decrypting the files of this folder…";
const PROTECT_STOPPED: &str =
    "Encrypting the files of this folder stopped; choose Finish protecting to go on.";
const UNPROTECT_STOPPED: &str =
    "Decrypting the files of this folder stopped; choose Finish unprotecting to go on.";

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
    /// Opens the actions on the selection from the count in the selection bar.
    SelectionMenu(Placement),
    AccountMenu(Placement),
    CloseMenu,
    Act(Action),
    NewFolder,
    Upload,
    /// Opens the dialog sending files of this computer to a device.
    SendLocal,
    Send(send::Message),
    Share(share::Message),
    UploadPlanned(Option<NodeId>, Result<UploadPlan, String>),
    DownloadPlanned(Result<Vec<DownloadFile>, String>),
    Dialog(dialogs::Message),
    Details(details::Message),
    OpenTrash,
    Trash(trash::Message),
    OpenShared,
    Shared(shared::Message),
    OpenSyncQueue,
    OpenSyncSettings,
    OpenDevices,
    Devices(devices::Message),
    OpenTransfers,
    Transfers(transfer_list::Message),
    /// The version chip, banner and "New version available" toast.
    Version(version::Message),
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
    Viewer(viewer::Message),
    /// The protected folders the server lists.
    ProtectedLoaded(Result<protected::Listed, ApiError>),
    /// Opens the dialog protecting this folder with a password.
    ProtectFolder(Node),
    /// Asks for the password of this protected folder.
    UnlockFolder(NodeId),
    LockFolder(NodeId),
    LockAll,
    ChangeFolderPassword(NodeId),
    UnprotectFolder(NodeId),
    /// Goes on with protecting or unprotecting this folder where it stopped.
    FinishConversion(NodeId),
    Converting(NodeId, convert::Progress),
    /// Locks the folders past their time without activity.
    LockIdle,
    Protect(protect::Message),
    /// Opens what a `home-space://` link names, such as a folder handed over by the browser.
    OpenLink(AppLink),
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
    /// The people picker of the device visibility dialog.
    Users(Placement),
    /// The people picker of the share dialog.
    SharePeople(Placement),
}

impl Menu {
    /// Whether it opens from a dialog, so it goes over that dialog rather than under it.
    fn over_dialog(self) -> bool {
        matches!(self, Self::Users(_) | Self::SharePeople(_))
    }
}

/// What the browser shows below the header, other than search results.
enum Page {
    Folder,
    /// The recycle bin.
    Trash(Box<trash::State>),
    /// What other users shared with the user.
    Shared(Box<shared::State>),
    /// What the sync has left to do.
    SyncQueue,
    /// What syncs where.
    SyncSettings,
    /// The user's devices.
    Devices(Box<devices::State>),
    /// What was sent to this device and what the user sent.
    Transfers,
    /// A file in a built-in viewer.
    Viewer(Box<viewer::State>),
}

pub struct State {
    client: Client,
    had_session: bool,
    me: Me,
    folder: Folder,
    /// The folder a breadcrumb jump left, which going back returns to.
    previous: Option<Option<NodeId>>,
    modifiers: Modifiers,
    window: Size,
    menu: Option<Menu>,
    dialog: Option<Dialog>,
    /// The dialog sending files to a device, while it is open.
    send: Option<send::SendDialog>,
    /// The dialog sharing files through a link, while it is open.
    share: Option<share::ShareDialog>,
    details: Option<details::State>,
    page: Page,
    transfers: Transfers,
    dropping: bool,
    search: Search,
    syncing: Syncing,
    lost_and_found: LostAndFound,
    /// The Transfers page, kept while elsewhere so downloads go on and the sidebar counts.
    transfer_list: TransferList,
    /// Where the user went from unsaved edits in the viewer, while asked whether to drop them.
    leave_to: Option<Box<Message>>,
    versions: version::Versions,
    /// The protected folders and the keys of the unlocked ones.
    protected: Protected,
    /// The dialog protecting, unlocking or unprotecting a folder, while it is open.
    protect: Option<protect::Dialog>,
    /// A folder whose content is being encrypted or decrypted, or whose conversion stopped.
    converting: Option<Conversion>,
    /// Plain files of a newly protected folder found in the recycle bin, to offer deleting once
    /// the dialog showing its recovery key closes.
    binned: Option<(String, Vec<TrashItem>)>,
    /// A folder opened from a link, which asks to be unlocked once it is listed locked.
    unlock_when_listed: Option<NodeId>,
    /// Whether the server's list of protected folders came, so a listing says for sure whether
    /// it is protected.
    protected_listed: bool,
}

/// A protected folder whose content is being converted.
#[derive(Debug, Clone)]
struct Conversion {
    folder: NodeId,
    direction: convert::Direction,
    message: String,
    failed: bool,
}

impl State {
    pub fn open(client: Client, me: Me) -> (Self, Task<Message>) {
        let (versions, check) = version::Versions::new(&client);
        let mut state = Self {
            versions,
            transfer_list: TransferList::new(me.id, me.lost_and_found_id),
            had_session: client.access_token().is_some(),
            client,
            folder: Folder::new(me.lost_and_found_id),
            lost_and_found: LostAndFound::new(me.lost_and_found_id),
            me,
            previous: None,
            modifiers: Modifiers::default(),
            window: DEFAULT_WINDOW,
            menu: None,
            dialog: None,
            send: None,
            share: None,
            details: None,
            page: Page::Folder,
            transfers: Transfers::default(),
            dropping: false,
            search: Search::default(),
            syncing: Syncing::default(),
            leave_to: None,
            protected: Protected::default(),
            protect: None,
            converting: None,
            binned: None,
            unlock_when_listed: None,
            protected_listed: false,
        };
        // The window may have been resized before the browser opened.
        let size = window::latest()
            .and_then(window::size)
            .map(Message::Resized);
        let transfers = state.transfer_list.load(&state.client);
        let task = Task::batch([
            state.folder.load(&state.client),
            state.load_protected(),
            state.load_lost_and_found(),
            transfers.map(Message::Transfers),
            check.map(Message::Version),
            size,
        ]);
        (state, task)
    }

    pub fn update(&mut self, message: Message) -> Update {
        match message {
            Message::SignedOut => {
                self.forget_protected();
                Update::SignedOut {
                    server: self.client.base_url().to_owned(),
                }
            }
            message => {
                let task = self.handle(message);
                if self.session_ended() {
                    self.forget_protected();
                    return Update::SignedOut {
                        server: self.client.base_url().to_owned(),
                    };
                }
                Update::Task(Task::batch([task, self.follow_selection()]))
            }
        }
    }

    fn session_ended(&self) -> bool {
        self.had_session && self.client.access_token().is_none()
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
        if let Page::Viewer(viewer) = &self.page
            && viewer.has_unsaved_edits()
            && leaves_page(&message)
        {
            self.menu = None;
            self.dialog = Some(Dialog::discard_edits(viewer.name().to_owned()));
            self.leave_to = Some(Box::new(message));
            return Task::none();
        }
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
                let result = result.map(|listing| self.open_listing(folder, listing));
                self.folder.loaded(folder, result);
                // Only the protected folder itself shows that it is locked.
                match &self.folder.protection {
                    Protection::Locked(locked)
                        if folder == self.folder.id && folder != Some(locked.node_id) =>
                    {
                        let root = locked.node_id;
                        if self.unlock_when_listed == folder {
                            self.unlock_when_listed = Some(root);
                        }
                        self.open_folder(Some(root))
                    }
                    Protection::Locked(locked)
                        if folder == self.folder.id
                            && self.unlock_when_listed == Some(locked.node_id)
                            && self.protect.is_none() =>
                    {
                        self.unlock_when_listed = None;
                        let root = locked.node_id;
                        self.handle(Message::UnlockFolder(root))
                    }
                    _ => {
                        if folder == self.folder.id && self.protected_listed {
                            self.unlock_when_listed = None;
                        }
                        Task::none()
                    }
                }
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
            Message::SelectionMenu(placement) => {
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
            Message::NewFolder => match self.adding_inside() {
                Ok(inside) => {
                    let (dialog, task) = Dialog::new_folder(self.folder_name(), inside);
                    self.dialog = Some(dialog);
                    task.map(Message::Dialog)
                }
                Err(err) => {
                    self.folder.error = Some(err.to_owned());
                    Task::none()
                }
            },
            Message::Upload => self.pick_upload(),
            Message::SendLocal => self.open_send(send::Source::Local),
            Message::Send(message) => self.send_update(message),
            Message::Share(message) => self.share_update(message),
            Message::UploadPlanned(parent, Ok(plan)) if !plan.is_empty() => {
                match self.upload_inside(parent) {
                    Ok(inside) => self
                        .transfers
                        .enqueue(
                            &self.client,
                            Batch::Upload {
                                parent,
                                plan,
                                inside,
                            },
                        )
                        .map(Message::Transfer),
                    Err(err) => {
                        self.folder.error = Some(err.to_owned());
                        Task::none()
                    }
                }
            }
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
            Message::OpenShared => self.open_shared(),
            Message::Shared(message) => match &mut self.page {
                Page::Shared(shared) => shared.update(&self.client, message).map(Message::Shared),
                _ => Task::none(),
            },
            Message::OpenSyncQueue => self.open_sync_page(Page::SyncQueue),
            Message::OpenSyncSettings => self.open_sync_page(Page::SyncSettings),
            Message::OpenDevices => self.open_devices(),
            Message::Devices(devices::Message::Menu(id, placement)) => {
                self.menu = Some(Menu::Device(id, placement));
                Task::none()
            }
            Message::Devices(devices::Message::Visibility(visibility::Message::Pick(
                placement,
            ))) => {
                self.menu = Some(Menu::Users(placement));
                self.devices_update(devices::Message::Visibility(visibility::Message::Pick(
                    placement,
                )))
            }
            Message::Devices(devices::Message::Visibility(visibility::Message::Moved(
                placement,
            ))) => {
                if let Some(Menu::Users(_)) = self.menu {
                    self.menu = Some(Menu::Users(placement));
                }
                Task::none()
            }
            Message::Devices(message) => {
                if !message.keeps_picker_open() {
                    self.menu = None;
                }
                self.devices_update(message)
            }
            Message::OpenTransfers => self.open_transfers(),
            // Saving from the toast happens on the Transfers page, where the dialog opens.
            Message::Transfers(transfer_list::Message::Save(id))
                if !matches!(self.page, Page::Transfers) =>
            {
                let open = self.open_transfers();
                let save = self
                    .transfer_list
                    .update(&self.client, transfer_list::Message::Save(id))
                    .map(Message::Transfers);
                Task::batch([open, save])
            }
            Message::Version(message) => {
                self.versions.update(message);
                Task::none()
            }
            Message::Transfers(message) => self
                .transfer_list
                .update(&self.client, message)
                .map(Message::Transfers),
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
                self.dropping = self.dialog.is_none()
                    && self.send.is_none()
                    && self.share.is_none()
                    && self.protect.is_none()
                    && !self.in_lost_and_found()
                    && self.adding_inside().is_ok();
                Task::none()
            }
            Message::FilesHoveredLeft => {
                self.dropping = false;
                Task::none()
            }
            Message::FileDropped(path) => {
                self.dropping = false;
                if let Some(send) = &self.send {
                    send.add_paths(vec![path]).map(Message::Send)
                } else if self.dialog.is_none()
                    && self.protect.is_none()
                    && !self.in_lost_and_found()
                    && self.adding_inside().is_ok()
                {
                    files::plan_upload(self.folder.id, vec![path])
                } else {
                    Task::none()
                }
            }
            Message::Server(event) => {
                self.syncing
                    .send(sync::Command::RemoteChanged(event.clone()));
                let details = self.sync_details(&event);
                let viewer = self.sync_viewer(&event);
                let found = if self.lost_and_found.apply(&event) {
                    self.load_lost_and_found()
                } else {
                    Task::none()
                };
                // Names in a protected folder are sealed in the event: the listing is opened again.
                let listing = if self.folder.protection.folder().is_some() {
                    if self.touches_listing(&event) {
                        self.folder.load(&self.client)
                    } else {
                        Task::none()
                    }
                } else if self.folder.apply(&event) {
                    self.folder.load(&self.client)
                } else {
                    Task::none()
                };
                let protected = if matches!(event, protocol::Event::Resync) {
                    self.load_protected()
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
                let transfers = match event {
                    // One for this computer is announced, unless the Transfers page shows it.
                    protocol::Event::TransferReceived { transfer }
                        if Some(transfer.recipient_device) == self.client.device_id()
                            && !matches!(self.page, Page::Transfers) =>
                    {
                        self.transfer_list
                            .update(&self.client, transfer_list::Message::Received(transfer))
                            .map(Message::Transfers)
                    }
                    protocol::Event::TransferReceived { .. }
                    | protocol::Event::TransferChanged { .. }
                    | protocol::Event::Resync => self
                        .transfer_list
                        .load(&self.client)
                        .map(Message::Transfers),
                    _ => Task::none(),
                };
                Task::batch([details, viewer, found, listing, protected, trash, transfers])
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
                let status = matches!(message, syncing::Message::Engine(sync::Update::Status(_)));
                let task = match self
                    .syncing
                    .update(message, |id| self.folder.listing.name_of(id))
                {
                    syncing::Outcome::Task(task) => task.map(Message::Sync),
                    syncing::Outcome::Error(err) => {
                        self.folder.error = Some(err);
                        Task::none()
                    }
                };
                if status {
                    self.keep_out_of_sync();
                }
                task
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
                self.forget_protected();
                let client = self.client.clone();
                Task::perform(
                    async move {
                        let _ = client.logout().await;
                    },
                    |()| Message::SignedOut,
                )
            }
            Message::SignedOut => Task::none(),
            Message::Viewer(message) => self.viewer_update(message),
            Message::ProtectedLoaded(Ok(listed)) => {
                self.protected_listed = true;
                let gone = self.protected.loaded(listed);
                self.keep_out_of_sync();
                let locked: Vec<Task<Message>> =
                    gone.into_iter().map(|id| self.locked(id)).collect();
                Task::batch(locked.into_iter().chain([self.folder.load(&self.client)]))
            }
            // Until the list comes, folders show as they are stored.
            Message::ProtectedLoaded(Err(_)) => {
                self.protected_listed = true;
                Task::none()
            }
            Message::ProtectFolder(node) => {
                self.menu = None;
                let (dialog, task) = protect::Dialog::protect(node);
                self.protect = Some(dialog);
                task.map(Message::Protect)
            }
            Message::UnlockFolder(id) => self.open_protect(id, protect::Dialog::unlock),
            Message::ChangeFolderPassword(id) => {
                self.open_protect(id, protect::Dialog::change_password)
            }
            Message::UnprotectFolder(id) => self.open_protect(id, |folder, name| {
                (protect::Dialog::unprotect(folder, name), Task::none())
            }),
            Message::LockFolder(id) => {
                self.menu = None;
                self.protected.lock(id);
                self.locked(id)
            }
            Message::LockAll => {
                self.menu = None;
                let locked = self.protected.lock_all();
                Task::batch(
                    locked
                        .into_iter()
                        .map(|id| self.locked(id))
                        .collect::<Vec<_>>(),
                )
            }
            Message::LockIdle => {
                let locked = self.protected.lock_idle();
                Task::batch(
                    locked
                        .into_iter()
                        .map(|id| self.locked(id))
                        .collect::<Vec<_>>(),
                )
            }
            Message::FinishConversion(id) => {
                self.menu = None;
                let direction = match self.protected.get(id).map(|folder| folder.state) {
                    Some(ProtectionState::Protecting) => convert::Direction::Protect,
                    Some(ProtectionState::Unprotecting) => convert::Direction::Unprotect,
                    _ => return Task::none(),
                };
                self.start_conversion(id, direction)
            }
            Message::Converting(id, progress) => self.conversion_update(id, progress),
            Message::Protect(message) => self.protect_update(message),
            Message::OpenLink(AppLink::Folder { server, folder }) => {
                if !same_server(server.as_str(), self.client.base_url()) {
                    self.folder.error = Some(format!(
                        "The link is for the folder of another server, {server}. This app is \
                         signed in to {}.",
                        self.client.base_url()
                    ));
                    return Task::none();
                }
                self.unlock_when_listed = Some(folder);
                self.handle(Message::Jump(Some(folder)))
            }
        }
    }

    /// Lists the protected folders.
    fn load_protected(&self) -> Task<Message> {
        Task::perform(
            protected::fetch(self.client.clone()),
            Message::ProtectedLoaded,
        )
    }

    /// Keeps the sync out of the protected folders: one that a synced folder holds, or that is
    /// synced itself, is set to not sync, which stops following it and leaves its local copy
    /// as it is. Its names and files on the server are sealed, and mean nothing on disk.
    fn keep_out_of_sync(&self) {
        let status = self.syncing.status();
        for id in self.protected.ids() {
            let reached = status.location(Some(id)).is_some();
            if reached && status.badge(Some(id)) != Some(sync::Badge::Disabled) {
                self.syncing.send(sync::Command::Map(sync::Mapping {
                    remote: Some(id),
                    local: None,
                }));
            }
        }
    }

    /// Locks every folder and deletes the decrypted copies, as signing out does.
    fn forget_protected(&mut self) {
        self.protected.lock_all();
        protected::files::sweep();
    }

    /// The listing of `folder`, with the names opened when it is in an unlocked protected
    /// folder, and nothing of it listed when that is locked. Notes which it is when it is the
    /// folder shown.
    fn open_listing(&mut self, folder: Option<NodeId>, listing: Listing) -> Listing {
        if folder != self.folder.id {
            return listing;
        }
        let Some(protection) = self.protected.enclosing(&listing.path).cloned() else {
            self.folder.protection = Protection::None;
            return listing;
        };
        match self.protected.inside(protection.node_id) {
            Some(inside) => {
                self.folder.protection = Protection::Unlocked(protection);
                Listing {
                    path: inside.open_path(listing.path),
                    nodes: inside.open_listing(listing.nodes),
                }
            }
            None => {
                self.folder.protection = Protection::Locked(protection);
                Listing {
                    path: listing.path,
                    nodes: Vec::new(),
                }
            }
        }
    }

    /// What work in the folder shown needs when it is in an unlocked protected folder; counts
    /// as activity in it.
    fn current_inside(&mut self) -> Option<Inside> {
        let root = match &self.folder.protection {
            Protection::Unlocked(folder) => folder.node_id,
            Protection::None | Protection::Locked(_) => return None,
        };
        self.protected.inside(root)
    }

    /// Whether files and folders may be added to the folder shown, and the keys that seal them
    /// when it is in a protected folder.
    fn adding_inside(&mut self) -> Result<Option<Inside>, &'static str> {
        match &self.folder.protection {
            Protection::None => Ok(None),
            Protection::Locked(_) => Err(LOCKED),
            protection if protection.is_frozen() => Err(FROZEN),
            Protection::Unlocked(_) => self.current_inside().map(Some).ok_or(LOCKED_MEANWHILE),
        }
    }

    /// The keys an upload into `parent` needs. One planned for another folder than the one
    /// shown goes as it is, unless that is a protected folder: the server refuses plain files
    /// inside one anyway.
    fn upload_inside(&mut self, parent: Option<NodeId>) -> Result<Option<Inside>, &'static str> {
        if parent == self.folder.id {
            return self.adding_inside();
        }
        match parent.filter(|id| self.protected.get(*id).is_some()) {
            Some(root) => self.protected.inside(root).map(Some).ok_or(LOCKED),
            None => Ok(None),
        }
    }

    /// What protected folders keep the selection from.
    fn limits(&self, selected: &[&Node]) -> Limits {
        Limits {
            protected: self.folder.protection.folder().is_some()
                || selected
                    .iter()
                    .any(|node| self.protected.is_or_holds(node.id)),
            frozen: self.folder.protection.is_frozen(),
        }
    }

    /// Whether `event` changes what the folder shown lists.
    fn touches_listing(&self, event: &protocol::Event) -> bool {
        let listed = |id: NodeId| self.folder.node(id).is_some();
        match event {
            protocol::Event::NodeCreated { node } => node.parent_id == self.folder.id,
            protocol::Event::NodeUpdated { node } => {
                node.parent_id == self.folder.id || listed(node.id)
            }
            protocol::Event::NodeDeleted { id, parent_id } => {
                *parent_id == self.folder.id || listed(*id)
            }
            protocol::Event::Resync => true,
            _ => false,
        }
    }

    /// Opens a dialog about the protected folder `id`, which `open` makes from it and its name.
    fn open_protect(
        &mut self,
        id: NodeId,
        open: impl FnOnce(
            protocol::ProtectedFolder,
            String,
        ) -> (protect::Dialog, Task<protect::Message>),
    ) -> Task<Message> {
        self.menu = None;
        let Some(folder) = self.protected.get(id).cloned() else {
            return Task::none();
        };
        let name = self
            .folder
            .listing
            .name_of(id)
            .unwrap_or_else(|| HOME.to_owned());
        let (dialog, task) = open(folder, name);
        self.protect = Some(dialog);
        task.map(Message::Protect)
    }

    /// Takes in that the protected folder `id` locked: what showed it goes back to the folder,
    /// which shows that it is locked.
    fn locked(&mut self, id: NodeId) -> Task<Message> {
        if self.dialog.as_ref().and_then(Dialog::protected_root) == Some(id) {
            self.dialog = None;
        }
        let viewing =
            matches!(&self.page, Page::Viewer(viewer) if viewer.protected_root() == Some(id));
        if self.folder.protection.root() != Some(id) && !viewing {
            return Task::none();
        }
        if viewing || matches!(self.page, Page::Folder) {
            self.open_folder(Some(id))
        } else {
            // Elsewhere, such as in the recycle bin: the folder is listed again on the way back.
            self.folder.listing.nodes.clear();
            self.folder.selection.clear();
            Task::none()
        }
    }

    /// Starts converting the content of the unlocked protected folder `id`.
    fn start_conversion(&mut self, id: NodeId, direction: convert::Direction) -> Task<Message> {
        if self
            .converting
            .as_ref()
            .is_some_and(|conversion| !conversion.failed)
        {
            return Task::none();
        }
        let Some(inside) = self.protected.inside(id) else {
            self.folder.error = Some(LOCKED.to_owned());
            return Task::none();
        };
        let message = match direction {
            convert::Direction::Protect => ENCRYPTING,
            convert::Direction::Unprotect => DECRYPTING,
        };
        self.converting = Some(Conversion {
            folder: id,
            direction,
            message: message.to_owned(),
            failed: false,
        });
        Task::run(
            convert::run(self.client.clone(), inside, direction),
            move |progress| Message::Converting(id, progress),
        )
    }

    fn conversion_update(&mut self, id: NodeId, progress: convert::Progress) -> Task<Message> {
        let Some(conversion) = self.converting.as_mut().filter(|known| known.folder == id) else {
            return Task::none();
        };
        let direction = conversion.direction;
        match progress {
            convert::Progress::File(name) => {
                conversion.message = match direction {
                    convert::Direction::Protect => format!("Encrypting “{name}”…"),
                    convert::Direction::Unprotect => format!("Decrypting “{name}”…"),
                };
                Task::none()
            }
            convert::Progress::Done(Ok(finished)) => {
                self.converting = None;
                match finished.folder {
                    Some(folder) => self.protected.remember(folder),
                    None => self.protected.forget(id),
                }
                if !finished.binned.is_empty() {
                    let name = self
                        .folder
                        .listing
                        .name_of(id)
                        .unwrap_or_else(|| HOME.to_owned());
                    self.binned = Some((name, finished.binned));
                    self.offer_binned();
                }
                Task::batch([self.folder.load(&self.client), self.load_protected()])
            }
            convert::Progress::Done(Err(stopped)) => {
                let stopped_text = match direction {
                    convert::Direction::Protect => PROTECT_STOPPED,
                    convert::Direction::Unprotect => UNPROTECT_STOPPED,
                };
                conversion.failed = true;
                conversion.message = format!("{stopped_text} {stopped}");
                if let Stopped::Api(ApiError::ReauthenticationRequired { message, methods }) =
                    stopped
                    && self.protect.is_none()
                {
                    let (dialog, task) = protect::Dialog::confirm_removal(id, message, methods);
                    self.protect = Some(dialog);
                    return task.map(Message::Protect);
                }
                self.folder.load(&self.client)
            }
        }
    }

    /// Offers to delete for good the plain files of a newly protected folder that are in the
    /// recycle bin, once no other dialog about protection is open.
    fn offer_binned(&mut self) {
        if self.protect.is_none()
            && let Some((name, items)) = self.binned.take()
        {
            self.protect = Some(protect::Dialog::binned(name, items));
        }
    }

    fn protect_update(&mut self, message: protect::Message) -> Task<Message> {
        let Some(dialog) = &mut self.protect else {
            return Task::none();
        };
        match dialog.update(&self.client, message) {
            protect::Outcome::Task(task) => task.map(Message::Protect),
            protect::Outcome::Closed => {
                self.protect = None;
                self.offer_binned();
                Task::none()
            }
            protect::Outcome::Opened {
                folder,
                keys,
                close,
            } => {
                if close {
                    self.protect = None;
                }
                self.protected.remember(folder.clone());
                if let Err(err) = self.protected.unlock(&folder, keys) {
                    self.protect = None;
                    self.folder.error = Some(err.to_string());
                    return Task::none();
                }
                let convert = if folder.state == ProtectionState::Protecting {
                    self.start_conversion(folder.node_id, convert::Direction::Protect)
                } else {
                    Task::none()
                };
                Task::batch([convert, self.folder.load(&self.client)])
            }
            protect::Outcome::Unprotecting(folder) => {
                self.protect = None;
                let id = folder.node_id;
                self.protected.remember(folder);
                let convert = self.start_conversion(id, convert::Direction::Unprotect);
                Task::batch([convert, self.folder.load(&self.client)])
            }
            protect::Outcome::Removed(id) => {
                self.protect = None;
                self.converting = None;
                self.protected.forget(id);
                Task::batch([self.folder.load(&self.client), self.load_protected()])
            }
        }
    }

    fn viewer_update(&mut self, message: viewer::Message) -> Task<Message> {
        let Page::Viewer(viewer) = &mut self.page else {
            return Task::none();
        };
        // Reading and editing count as activity in the protected folder.
        if let Some(root) = viewer.protected_root() {
            self.protected.inside(root);
        }
        match viewer.update(&self.client, message) {
            viewer::Outcome::Task(task) => task.map(Message::Viewer),
            viewer::Outcome::Open(node) => {
                let gallery = viewer.take_gallery().map(|mut gallery| {
                    gallery.move_to(node.id);
                    gallery
                });
                match viewers().viewer_for_node(&node) {
                    Some(found) => self.show_in_viewer(&node, found, gallery),
                    None => self.open_externally(&node),
                }
            }
            viewer::Outcome::Closed => self.open_folder(self.folder.id),
        }
    }

    /// Follows changes to the file the viewer shows; it closes when the file goes.
    fn sync_viewer(&mut self, event: &protocol::Event) -> Task<Message> {
        let Page::Viewer(viewer) = &mut self.page else {
            return Task::none();
        };
        viewer.gallery_changed(event);
        match event {
            protocol::Event::NodeUpdated { node } if node.id == viewer.node_id() => {
                viewer.changed(&self.client, node).map(Message::Viewer)
            }
            protocol::Event::NodeDeleted { id, .. } if *id == viewer.node_id() => {
                self.open_folder(self.folder.id)
            }
            _ => Task::none(),
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

    fn open_shared(&mut self) -> Task<Message> {
        self.search.clear();
        self.folder.error = None;
        self.details = None;
        self.menu = None;
        self.folder.selection.clear();
        let mut shared = shared::State::open(self.me.lost_and_found_id);
        let task = shared.load(&self.client);
        self.page = Page::Shared(Box::new(shared));
        task.map(Message::Shared)
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

    fn open_transfers(&mut self) -> Task<Message> {
        self.search.clear();
        self.folder.error = None;
        self.details = None;
        self.menu = None;
        self.folder.selection.clear();
        self.page = Page::Transfers;
        self.transfer_list.clear_toast();
        self.transfer_list
            .load(&self.client)
            .map(Message::Transfers)
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
            Page::Shared(shared) => shared
                .update(&self.client, shared::Message::Refresh)
                .map(Message::Shared),
            Page::Devices(devices) => devices.load(&self.client).map(Message::Devices),
            Page::Transfers => self
                .transfer_list
                .load(&self.client)
                .map(Message::Transfers),
            Page::Folder | Page::SyncQueue | Page::SyncSettings | Page::Viewer(_) => {
                self.folder.reload(&self.client)
            }
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

    /// Opens the file `node` in its built-in viewer, or with the default app when it has none.
    fn open_file(&mut self, node: &Node) -> Task<Message> {
        match viewers().viewer_for_node(node) {
            Some(viewer) => self.open_viewer(node, viewer),
            None => self.open_externally(node),
        }
    }

    /// Opens `node` in `viewer`. A viewer that steps through its folder gets the folder's files
    /// from the listing shown when it is that folder, and lists the folder otherwise.
    fn open_viewer(&mut self, node: &Node, viewer: viewers::Viewer) -> Task<Message> {
        if !viewer.kind.navigates() {
            return self.show_in_viewer(node, viewer, None);
        }
        let listed = !self.folder.loading && node.parent_id == self.folder.id;
        let inside = self.current_inside();
        if listed {
            let gallery = Gallery::new(
                viewers(),
                &self.folder.listing.nodes,
                node,
                self.folder.sort,
            );
            self.show_in_viewer(node, viewer, gallery)
        } else {
            let show = self.show_in_viewer(node, viewer, None);
            let list = list_gallery(&self.client, node.clone(), self.folder.sort, inside);
            Task::batch([show, list])
        }
    }

    fn show_in_viewer(
        &mut self,
        node: &Node,
        viewer: viewers::Viewer,
        gallery: Option<Gallery>,
    ) -> Task<Message> {
        self.search.clear();
        self.folder.error = None;
        self.details = None;
        self.menu = None;
        let synced = self.synced_copy(node);
        let inside = self.current_inside();
        let (state, task) =
            viewer::State::open(&self.client, node.clone(), viewer, synced, gallery, inside);
        self.page = Page::Viewer(Box::new(state));
        task.map(Message::Viewer)
    }

    /// Opens the file `node` with the default app, from a local copy that is the server's version
    /// or else from a fresh download; in a protected folder, from a copy decrypted into the
    /// private folder.
    fn open_externally(&mut self, node: &Node) -> Task<Message> {
        match &self.folder.protection {
            Protection::None => files::open(&self.client, node.clone(), self.synced_copy(node)),
            Protection::Locked(_) => Task::none(),
            Protection::Unlocked(_) => match self.current_inside() {
                Some(inside) => files::open_protected(node, inside),
                None => Task::none(),
            },
        }
    }

    fn synced_copy(&self, node: &Node) -> Option<files::SyncedCopy> {
        self.local_copy(node).map(|path| files::SyncedCopy {
            path,
            base: self.syncing.status().base(node.id).cloned(),
        })
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
        let limits = self.limits(&selected.iter().collect::<Vec<_>>());
        let refused = match action {
            Action::Send | Action::Share | Action::Details => limits.protected,
            Action::Rename | Action::Move | Action::Delete => limits.frozen,
            Action::Open | Action::OpenFile | Action::OpenWith | Action::Download => false,
        };
        if refused {
            return Task::none();
        }
        match (action, selected.as_slice()) {
            (_, []) => Task::none(),
            (Action::Open, [node]) => self.activate(node.id),
            (Action::OpenFile, [node]) => {
                let node = node.clone();
                self.open_file(&node)
            }
            (Action::OpenWith, [node]) => {
                let node = node.clone();
                self.open_externally(&node)
            }
            (Action::Rename, [node]) => {
                let inside = self.current_inside();
                let (dialog, task) = Dialog::rename(node.clone(), inside);
                self.dialog = Some(dialog);
                task.map(Message::Dialog)
            }
            (Action::Details, [node]) => {
                let (details, task) = details::State::open(&self.client, node.clone());
                self.details = Some(details);
                task.map(Message::Details)
            }
            (
                Action::Open
                | Action::OpenFile
                | Action::OpenWith
                | Action::Rename
                | Action::Details,
                _,
            ) => Task::none(),
            (Action::Download, _) => {
                let inside = self.current_inside();
                if self.folder.protection.folder().is_some() && inside.is_none() {
                    return Task::none();
                }
                files::download(&self.client, selected, inside)
            }
            (Action::Send, _) => self.open_send(send::Source::Nodes(selected)),
            (Action::Share, _) => {
                let (dialog, task) =
                    share::ShareDialog::open(&self.client, selected, self.me.username.clone());
                self.share = Some(dialog);
                task.map(Message::Share)
            }
            (Action::Move, _) => {
                let within = self.current_inside().map(|inside| Within {
                    inside,
                    trail: self.folder.listing.path.clone(),
                });
                if self.folder.protection.folder().is_some() && within.is_none() {
                    return Task::none();
                }
                let (dialog, task) = Dialog::move_nodes(
                    &self.client,
                    selected,
                    self.me.lost_and_found_id,
                    self.protected.ids(),
                    within,
                );
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
                let for_good = self.folder.protection.folder().is_some()
                    || selected
                        .iter()
                        .any(|node| self.protected.is_or_holds(node.id));
                self.dialog = Some(Dialog::delete(selected, for_good));
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
                return files::download(&self.client, vec![details.node().clone()], None);
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
                self.leave_to = None;
                Task::none()
            }
            Outcome::Discarded => {
                self.dialog = None;
                let Some(leave_to) = self.leave_to.take() else {
                    return Task::none();
                };
                if let Page::Viewer(viewer) = &mut self.page {
                    viewer.discard();
                }
                self.handle(*leave_to)
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

    fn open_send(&mut self, source: send::Source) -> Task<Message> {
        self.menu = None;
        let (dialog, task) = send::SendDialog::open(&self.client, source);
        self.send = Some(dialog);
        task.map(Message::Send)
    }

    fn share_update(&mut self, message: share::Message) -> Task<Message> {
        let Some(dialog) = &mut self.share else {
            return Task::none();
        };
        self.menu = match message {
            share::Message::Pick(placement) => Some(Menu::SharePeople(placement)),
            share::Message::Moved(placement) => self.menu.map(|menu| match menu {
                Menu::SharePeople(_) => Menu::SharePeople(placement),
                other => other,
            }),
            _ if message.keeps_picker_open() => self.menu,
            _ => None,
        };
        match dialog.update(&self.client, message) {
            share::Outcome::Task(task) => task.map(Message::Share),
            share::Outcome::Closed => {
                self.share = None;
                Task::none()
            }
        }
    }

    fn send_update(&mut self, message: send::Message) -> Task<Message> {
        let Some(dialog) = &mut self.send else {
            return Task::none();
        };
        match dialog.update(&self.client, message) {
            send::Outcome::Task(task) => task.map(Message::Send),
            send::Outcome::Closed => {
                self.send = None;
                Task::none()
            }
            send::Outcome::Sent {
                created,
                files,
                label,
            } => {
                self.send = None;
                let uploads = files_to_upload(
                    &created,
                    files.into_iter().map(|file| (file.path.clone(), file)),
                );
                let upload = if uploads.is_empty() {
                    self.transfers.note_sent(label);
                    Task::none()
                } else {
                    self.transfers
                        .enqueue(&self.client, Batch::Send(uploads))
                        .map(Message::Transfer)
                };
                let listed = self
                    .transfer_list
                    .load(&self.client)
                    .map(Message::Transfers);
                Task::batch([upload, listed])
            }
        }
    }

    fn shortcut(&mut self, shortcut: Shortcut) -> Task<Message> {
        // Escape refuses a link; nothing opens it but the button, so a stray Enter can't.
        if self.transfer_list.is_confirming_link() {
            return match shortcut {
                Shortcut::Escape => {
                    self.handle(Message::Transfers(transfer_list::Message::CancelOpen))
                }
                _ => Task::none(),
            };
        }
        if self.share.is_some() {
            match shortcut {
                Shortcut::Escape if matches!(self.menu, Some(Menu::SharePeople(_))) => {
                    self.menu = None;
                    Task::none()
                }
                Shortcut::Escape => self.share_update(share::Message::Cancel),
                Shortcut::Activate => self.share_update(share::Message::Submit),
                _ => Task::none(),
            }
        } else if self.send.is_some() {
            match shortcut {
                Shortcut::Escape => self.send_update(send::Message::Cancel),
                Shortcut::Activate => self.send_update(send::Message::Submit),
                _ => Task::none(),
            }
        } else if self.protect.is_some() {
            match shortcut {
                Shortcut::Escape => self.protect_update(protect::Message::Cancel),
                Shortcut::Activate => self.protect_update(protect::Message::Submit),
                _ => Task::none(),
            }
        } else if self.dialog.is_some() {
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
        } else if let Page::Shared(shared) = &mut self.page {
            match shortcut {
                Shortcut::Escape if shared.is_asking() => self.handle(Message::Shared(
                    shared::Message::SaveDialog(transfer_save::Message::Cancel),
                )),
                Shortcut::Activate if shared.is_asking() => self.handle(Message::Shared(
                    shared::Message::SaveDialog(transfer_save::Message::Submit),
                )),
                Shortcut::Escape | Shortcut::Back | Shortcut::Parent
                    if shared.viewing().is_some() =>
                {
                    self.handle(Message::Shared(shared::Message::CloseView))
                }
                Shortcut::Open | Shortcut::Next if shared.viewing().is_some() => {
                    self.handle(Message::Shared(shared::Message::Next))
                }
                Shortcut::Previous if shared.viewing().is_some() => {
                    self.handle(Message::Shared(shared::Message::Previous))
                }
                Shortcut::Refresh => self.reload(),
                Shortcut::Escape | Shortcut::Back | Shortcut::Parent => {
                    self.open_folder(self.folder.id)
                }
                _ => Task::none(),
            }
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
        } else if matches!(self.page, Page::Transfers) {
            match shortcut {
                Shortcut::Escape if self.transfer_list.is_asking() => {
                    self.handle(Message::Transfers(transfer_list::Message::SaveDialog(
                        transfer_save::Message::Cancel,
                    )))
                }
                Shortcut::Activate if self.transfer_list.is_asking() => {
                    self.handle(Message::Transfers(transfer_list::Message::SaveDialog(
                        transfer_save::Message::Submit,
                    )))
                }
                Shortcut::Refresh => self.reload(),
                Shortcut::Escape | Shortcut::Back | Shortcut::Parent => {
                    self.open_folder(self.folder.id)
                }
                _ => Task::none(),
            }
        } else if matches!(self.page, Page::Viewer(_)) {
            match shortcut {
                Shortcut::Escape | Shortcut::Back | Shortcut::Parent => {
                    self.handle(Message::Viewer(viewer::Message::Close))
                }
                Shortcut::Open | Shortcut::Next => {
                    self.handle(Message::Viewer(viewer::Message::Next))
                }
                Shortcut::Previous => self.handle(Message::Viewer(viewer::Message::Previous)),
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
                Shortcut::Search | Shortcut::Next | Shortcut::Previous => Task::none(),
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
        let clock = if matches!(self.page, Page::Transfers) {
            iced::time::every(CLOCK_TICK).map(|_| Message::Transfers(transfer_list::Message::Tick))
        } else {
            Subscription::none()
        };
        // Folders lock after their time without activity even when nothing else happens.
        let lock = if self.protected.any_unlocked() {
            iced::time::every(LOCK_CHECK).map(|_| Message::LockIdle)
        } else {
            Subscription::none()
        };
        Subscription::batch([
            clock,
            lock,
            event::listen_with(input),
            events::subscription(&self.client).map(|live| match live {
                events::Live::Event(event) => Message::Server(*event),
                events::Live::SignedOut => Message::SignedOut,
            }),
            sync::subscription(&self.client, self.me.id)
                .map(|update| Message::Sync(syncing::Message::Engine(update))),
        ])
    }
}

/// Whether `link` and `signed_in`, two server addresses, are the same server: the same scheme,
/// host and port, whatever the path or a trailing slash.
fn same_server(link: &str, signed_in: &str) -> bool {
    match (url::Url::parse(link), url::Url::parse(signed_in)) {
        (Ok(link), Ok(signed_in)) => link.origin() == signed_in.origin(),
        _ => false,
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

/// Whether `message` takes the browser away from the page it shows.
fn leaves_page(message: &Message) -> bool {
    matches!(
        message,
        Message::Jump(_)
            | Message::OpenFiles
            | Message::OpenLostAndFound
            | Message::OpenTrash
            | Message::OpenShared
            | Message::OpenSyncQueue
            | Message::OpenSyncSettings
            | Message::OpenDevices
            | Message::OpenTransfers
            | Message::Transfers(transfer_list::Message::Save(_))
            | Message::SignOut
            | Message::Viewer(viewer::Message::Close)
    )
}

/// Lists the folder of `node` for the viewer to step through, in `sort` order; with the names
/// opened in a protected folder.
fn list_gallery(client: &Client, node: Node, sort: Sort, inside: Option<Inside>) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move {
            let listing = client
                .list(node.parent_id)
                .await
                .map_err(|err| err.to_string())?;
            let listing = match &inside {
                Some(inside) => inside.open_listing(listing),
                None => listing,
            };
            Gallery::new(viewers(), &listing, &node, sort)
                .ok_or_else(|| format!("{} has no viewer to step through", node.name))
        },
        |gallery| Message::Viewer(viewer::Message::Gallery(gallery)),
    )
}

#[cfg(test)]
mod tests;
