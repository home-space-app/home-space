//! Two-way sync of mapped remote folders with local folders, shared by the desktop and mobile
//! clients. What differs between them is behind [`Platform`].
//!
//! Every pass compares three states per path: the remote tree (kept current from the
//! server's change journal), a scan of the local folders and the bases recorded when both
//! last agreed (see [`plan`]). Passes run on start, after local file changes, after server
//! events and every few minutes.
//!
//! A path that fails is tried again later, waiting longer after each failure. When the
//! server cannot be reached at all, such as behind a gateway answering 502, the failures
//! are not the paths' fault: after a few in a row the pass stops starting transfers and the
//! sync reports one [`Outage`], trying again with a growing delay.
//!
//! A chunked upload's session is saved in the [`store`], so that after a restart it
//! continues from what the server already has, as long as the file is unchanged. Sessions of
//! files changed, deleted or no longer to be uploaded are cancelled on the server.

mod apply;
#[cfg(test)]
mod e2e;
mod layout;
mod model;
mod plan;
mod queue;
mod remote;
mod scan;
mod status;
mod store;

use std::{
    any::Any,
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use time::OffsetDateTime;

use futures::{SinkExt, StreamExt, channel::mpsc::Sender};
use tokio::{sync::mpsc, time::Instant};

use api_client::{ApiError, Client};
use protocol::{Event, NodeId};

use self::{
    apply::{Effect, Failure, Folders},
    layout::Layout,
    plan::{Action, Phase},
    queue::{Progress, QueueItem},
    remote::{Delta, Mirror},
    scan::Scan,
    status::Badges,
};
pub use self::{
    apply::{part_path, read_range},
    layout::local_name,
    model::{Base, Mapping, RemoteNode, nanos},
    queue::{Fault, Gate, ItemState, Operation, QueueItem as Item, Summary, Transfers},
    scan::{hash_file, is_ignored},
    status::Badge,
    store::Store,
};

const TRANSFERS: usize = 4;
/// Local changes come in bursts; a pass waits for the burst to settle.
const SETTLE: Duration = Duration::from_secs(1);
/// Catches what neither the file watcher nor the server events reported.
const RESCAN: Duration = Duration::from_secs(300);
const RETRY: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(10 * 60);
/// Unreachable-server failures in a row that make a pass stop as an outage.
const OUTAGE_FAILURES: u32 = 3;
const MAX_OUTAGE_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// Code of a file the scan could not read.
const UNREADABLE_CODE: &str = "unreadable";
/// How often badges are refreshed while a pass is transferring.
const PROGRESS: Duration = Duration::from_millis(500);

/// What the engine needs from the platform it runs on. The defaults suit an app that keeps its
/// synced files to itself: the user can't reach them to delete or move them, and there is no
/// system trash to use.
pub trait Platform: Send + Sync + 'static {
    /// Whether the local copies belong to the app, not the user. A path that goes missing was
    /// then not deleted by anyone, so it is brought back rather than deleted on the server. A
    /// mapping whose node is gone from the server goes too, along with its local folder.
    fn app_owned_copies(&self) -> bool {
        true
    }

    /// Where edits go whose file is gone from the server, along with the mapping that held them,
    /// so that they aren't lost. Without it the mapping stays.
    fn rescue_folder(&self) -> Option<NodeId> {
        None
    }

    /// The app's own data, which no local folder of a mapping may overlap.
    fn protected_dir(&self) -> Option<PathBuf> {
        None
    }

    /// Takes away a local file whose node is gone from the server. May block.
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    /// Starts reporting changes under `roots` as `()` on `changed`, for as long as the returned
    /// value is kept.
    fn watch(
        &self,
        _roots: &[&Path],
        _changed: mpsc::UnboundedSender<()>,
    ) -> Option<Box<dyn Any + Send>> {
        None
    }
}

/// A [`Platform`] that relies on all the defaults.
#[derive(Debug, Clone, Copy, Default)]
pub struct PrivateFiles {
    pub rescue: Option<NodeId>,
}

impl Platform for PrivateFiles {
    fn rescue_folder(&self) -> Option<NodeId> {
        self.rescue
    }
}

/// What the sync reports to the app.
#[derive(Debug, Clone)]
pub enum Update {
    /// The engine started; send it commands through this.
    Ready(Commands),
    Status(Arc<Status>),
    /// A requested mapping was not made.
    Rejected(String),
    /// Work is done for now, for a pass that started after [`Command::Wake`] with this ticket:
    /// everything is in sync, waiting to be tried again, or held back by the [`Gate`].
    Settled(u64),
}

/// What the engine is busy with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Activity {
    #[default]
    Idle,
    /// Reading what changed on the server.
    Checking,
    /// Looking through the local folders, hashing new and changed files.
    Scanning,
    /// Moving, transferring and deleting.
    Syncing,
}

/// A snapshot of the sync for the interface.
#[derive(Debug, Clone, Default)]
pub struct Status {
    /// Badge of every synced node; Home is under `None`.
    pub badges: Badges,
    pub mappings: Vec<Mapping>,
    /// Names of the mapped remote folders, as far as they are known.
    pub names: HashMap<NodeId, String>,
    /// Where each synced folder is on disk, Home under `None`.
    pub locations: HashMap<Option<NodeId>, PathBuf>,
    /// Where each synced file is on disk.
    pub files: HashMap<NodeId, PathBuf>,
    /// What was last agreed with the server about each synced node.
    pub bases: HashMap<NodeId, Base>,
    /// Paths with work ahead.
    pub pending: usize,
    /// Paths that failed, with why.
    pub problems: Vec<(PathBuf, Fault)>,
    /// Why the last pass could not run.
    pub error: Option<String>,
    /// Set while the server cannot be reached.
    pub outage: Option<Outage>,
    pub activity: Activity,
    /// Everything with work ahead, then what failed.
    pub queue: Vec<Item>,
    /// Synced folders whose next pass reads everything again, trusting no cache.
    pub verifying: HashSet<Option<NodeId>>,
}

impl Status {
    pub fn badge(&self, node: Option<NodeId>) -> Option<Badge> {
        self.badges.get(&node).copied()
    }

    /// Where the folder `node` syncs to, if it does.
    pub fn location(&self, node: Option<NodeId>) -> Option<&Path> {
        self.locations.get(&node).map(PathBuf::as_path)
    }

    /// Where the file `node` syncs to, if it does; it may not be on disk yet.
    pub fn file_path(&self, node: NodeId) -> Option<&Path> {
        self.files.get(&node).map(PathBuf::as_path)
    }

    /// What was last agreed with the server about `node`, if anything.
    pub fn base(&self, node: NodeId) -> Option<&Base> {
        self.bases.get(&node)
    }

    /// The new files and folders waiting to be uploaded into `folder` (`None` for Home),
    /// which the server's listing of it cannot show yet.
    pub fn new_uploads_in(&self, folder: Option<NodeId>) -> impl Iterator<Item = &Item> {
        self.queue.iter().filter(move |item| item.is_new_in(folder))
    }

    /// The queue in numbers.
    pub fn summary(&self) -> Summary {
        queue::summarize(&self.queue)
    }

    /// Whether the folder `node` waits for, or is in, a verifying pass.
    pub fn is_verifying(&self, node: Option<NodeId>) -> bool {
        self.verifying.contains(&node)
    }

    /// The mapping set on `node` itself, if any.
    pub fn mapping(&self, node: Option<NodeId>) -> Option<&Mapping> {
        self.mappings.iter().find(|mapping| mapping.remote == node)
    }

    /// The state of the sync as a whole: the worst of all synced nodes, `None` while
    /// nothing is synced.
    pub fn overall(&self) -> Option<Badge> {
        if self.error.is_some() || self.outage.is_some() || !self.problems.is_empty() {
            return Some(Badge::Problem);
        }
        if self.pending > 0 {
            return Some(Badge::Pending);
        }
        self.mappings
            .iter()
            .any(|mapping| mapping.local.is_some())
            .then_some(Badge::Synced)
    }
}

/// The server cannot be reached; the sync waits and tries again on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outage {
    pub reason: String,
    pub since: OffsetDateTime,
    /// When the sync tries again.
    pub retry_at: OffsetDateTime,
    attempts: u32,
}

#[derive(Debug, Clone)]
pub enum Command {
    /// Syncs a remote folder to a local folder, or sets it to not sync.
    Map(Mapping),
    /// Removes the mapping of a remote folder; its local files stay.
    Unmap(Option<NodeId>),
    /// The server reported a change.
    RemoteChanged(Event),
    /// Tries the failed paths again now: these ones, or all of them with `None`.
    Retry(Option<Vec<PathBuf>>),
    /// Like [`Command::Map`], without reading everything mapped again: `known` holds what is
    /// already known of the node, and only a folder not told about here is read.
    Pin {
        mapping: Mapping,
        known: Option<RemoteNode>,
    },
    /// Removes a mapping along with its local folder and what was recorded about it.
    Purge(NodeId),
    /// Runs a pass now, unless the [`Gate`] is closed.
    Wake(u64),
    /// Ends the engine.
    Shutdown,
    /// Reads the synced folder `node` (`None` for Home) from scratch on both sides: the
    /// server's tree again, and every local file hashed again, catching what the caches
    /// missed.
    Verify(Option<NodeId>),
}

/// Sends commands to the running engine.
#[derive(Debug, Clone)]
pub struct Commands(mpsc::UnboundedSender<Command>);

impl Commands {
    pub fn send(&self, command: Command) {
        let _ = self.0.send(command);
    }
}

/// Runs the sync until the session ends or the app stops listening. Reports to `output`.
pub async fn run(
    client: Client,
    store: impl Into<Arc<Store>>,
    platform: Arc<dyn Platform>,
    gate: Gate,
    mut output: Sender<Update>,
) {
    let mut engine = match Engine::load(client, store, platform) {
        Ok(engine) => engine,
        Err(error) => {
            let _ = output
                .send(Update::Status(Arc::new(unavailable(&error))))
                .await;
            return;
        }
    };
    let (commands, mut received) = mpsc::unbounded_channel();
    if output
        .send(Update::Ready(Commands(commands)))
        .await
        .is_err()
    {
        return;
    }
    engine.set_gate(gate.clone());
    engine.report_now(&mut output).await;
    let (changed, mut local_changes) = mpsc::unbounded_channel();
    let mut watcher = engine.watch(&changed);
    let mut due = Some(Instant::now());
    let mut interrupted: Option<Command> = None;
    let mut ticket = 0;
    loop {
        let wake = due.unwrap_or_else(|| Instant::now() + RESCAN);
        tokio::select! {
            command = async {
                match interrupted.take() {
                    Some(command) => Some(command),
                    None => received.recv().await,
                }
            } => {
                let Some(command) = command else { break };
                if engine.is_echo(&command) {
                    continue;
                }
                match command {
                    Command::Shutdown => break,
                    Command::Wake(asked) => ticket = asked,
                    _ => {}
                }
                if engine.handle(command, &mut output).await {
                    watcher = engine.watch(&changed);
                    engine.activity = Activity::Checking;
                    engine.report_now(&mut output).await;
                }
                due = Some(due.map_or(Instant::now(), |at| at.min(Instant::now())));
            }
            Some(()) = local_changes.recv() => {
                due.get_or_insert_with(|| Instant::now() + SETTLE);
            }
            () = tokio::time::sleep_until(wake) => {
                if gate.is_closed() {
                    // [`Command::Wake`] brings it back once the gate opens.
                    due = None;
                    let _ = output.send(Update::Settled(ticket)).await;
                    continue;
                }
                let started = ticket;
                match engine.pass(&mut output, &mut received).await {
                    Ok(next) => {
                        due = next.map(|delay| Instant::now() + delay);
                        if next.is_none_or(|delay| delay > SETTLE) {
                            let _ = output.send(Update::Settled(started)).await;
                        }
                    }
                    Err(SignedOut) => break,
                }
                interrupted = engine.interrupted.take();
                // Our own writes during the pass were reported too; they need no pass.
                while local_changes.try_recv().is_ok() {}
            }
        }
    }
    drop(watcher);
}

/// The state to report when the sync cannot start.
pub fn unavailable(error: &impl std::fmt::Display) -> Status {
    Status {
        error: Some(format!("sync is unavailable: {error}")),
        ..Status::default()
    }
}

/// The session ended; the engine stops.
struct SignedOut;

/// A path that failed and when it may be tried again.
#[derive(Debug, Clone)]
struct Problem {
    fault: Fault,
    attempts: u32,
    retry_at: Instant,
    /// What was being done, when it is known.
    item: Option<QueueItem>,
}

struct Engine {
    client: Client,
    platform: Arc<dyn Platform>,
    store: Arc<Store>,
    mirror: Mirror,
    mappings: Vec<Mapping>,
    /// Set when the mapped folders must be read from scratch.
    reload: bool,
    /// Mapped nodes still to be read, while the rest of what is mapped is known.
    unread: HashSet<NodeId>,
    gate: Gate,
    /// Synced folders whose local files are hashed again on the next scan.
    verify: HashSet<Option<NodeId>>,
    problems: HashMap<PathBuf, Problem>,
    /// Set while the server cannot be reached.
    outage: Option<Outage>,
    /// Local paths the last scan could not read, with why.
    unreadable: Vec<(PathBuf, Fault)>,
    activity: Activity,
    /// The work ahead as of the last plan, less what has finished since.
    queue: Vec<QueueItem>,
    /// Bytes moved so far by the transfers running.
    progress: Progress,
    /// Set when the engine goes away, so a scan on a blocking thread stops with it; the
    /// app cannot quit while one runs.
    stop: Arc<AtomicBool>,
    /// A command that arrived while a scan ran and cut it short, to be handled next.
    interrupted: Option<Command>,
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Engine {
    fn load(
        client: Client,
        store: impl Into<Arc<Store>>,
        platform: Arc<dyn Platform>,
    ) -> rusqlite::Result<Self> {
        let store: Arc<Store> = store.into();
        let mirror = Mirror {
            nodes: store
                .remote_nodes()?
                .into_iter()
                .map(|node| (node.id, node))
                .collect(),
            cursor: store.cursor()?,
        };
        let mappings = store.mappings()?;
        // Mapped just before the app last stopped, so never read.
        let unread = if platform.app_owned_copies() {
            mappings
                .iter()
                .filter(|m| m.local.is_some())
                .filter_map(|m| m.remote)
                .filter(|id| !mirror.nodes.contains_key(id))
                .collect()
        } else {
            HashSet::new()
        };
        Ok(Self {
            client,
            platform,
            mappings,
            reload: mirror.cursor.is_none(),
            unread,
            gate: Gate::default(),
            verify: HashSet::new(),
            mirror,
            store,
            problems: HashMap::new(),
            outage: None,
            unreadable: Vec::new(),
            activity: Activity::Idle,
            queue: Vec::new(),
            progress: Progress::default(),
            stop: Arc::new(AtomicBool::new(false)),
            interrupted: None,
        })
    }

    fn set_gate(&mut self, gate: Gate) {
        self.progress = Arc::new(queue::Transfers::new(gate.clone()));
        self.gate = gate;
    }

    /// Watches the mapped local folders; changes are reported as `()` on `changed`.
    fn watch(&self, changed: &mpsc::UnboundedSender<()>) -> Option<Box<dyn Any + Send>> {
        let roots: Vec<&Path> = self
            .mappings
            .iter()
            .filter_map(|m| m.local.as_deref())
            .collect();
        self.platform.watch(&roots, changed.clone())
    }

    /// Applies a command; returns whether the watched folders changed.
    async fn handle(&mut self, command: Command, output: &mut Sender<Update>) -> bool {
        match command {
            Command::RemoteChanged(_) | Command::Wake(_) | Command::Shutdown => false,
            Command::Pin { mapping, known } => {
                if let Err(err) = self.pin(&mapping, known) {
                    let _ = output.send(Update::Rejected(err.to_string())).await;
                    return false;
                }
                self.mappings.retain(|m| m.remote != mapping.remote);
                self.mappings.push(mapping);
                true
            }
            Command::Purge(remote) => {
                if let Err(err) = self.purge(remote).await {
                    let _ = output.send(Update::Rejected(err.to_string())).await;
                }
                true
            }
            Command::Retry(paths) => {
                let now = Instant::now();
                self.problems
                    .iter_mut()
                    .filter(|(path, _)| paths.as_ref().is_none_or(|paths| paths.contains(path)))
                    .for_each(|(_, problem)| {
                        problem.retry_at = now;
                        problem.attempts = 0;
                    });
                self.outage = None;
                false
            }
            Command::Verify(remote) => {
                if self.synced_root(remote).is_some() {
                    self.verify.insert(remote);
                    self.reload = true;
                }
                false
            }
            Command::Map(mapping) => {
                if let Some(local) = &mapping.local {
                    let others: Vec<&Path> = self
                        .mappings
                        .iter()
                        .filter(|m| m.remote != mapping.remote)
                        .filter_map(|m| m.local.as_deref())
                        .collect();
                    let data = self.platform.protected_dir();
                    if let Some(reason) = layout::conflict(local, &others, data.as_deref()) {
                        let _ = output.send(Update::Rejected(reason)).await;
                        return false;
                    }
                }
                if let Err(err) = self.store.set_mapping(&mapping) {
                    let _ = output.send(Update::Rejected(err.to_string())).await;
                    return false;
                }
                self.mappings.retain(|m| m.remote != mapping.remote);
                self.mappings.push(mapping);
                self.reload = true;
                true
            }
            Command::Unmap(remote) => {
                if let Err(err) = self.store.remove_mapping(remote) {
                    let _ = output.send(Update::Rejected(err.to_string())).await;
                    return false;
                }
                self.mappings.retain(|m| m.remote != remote);
                self.verify.remove(&remote);
                true
            }
        }
    }

    fn pin(&mut self, mapping: &Mapping, known: Option<RemoteNode>) -> rusqlite::Result<()> {
        match known {
            Some(node) => {
                self.store
                    .update_remote(false, std::slice::from_ref(&node), &[])?;
                self.mirror.nodes.insert(node.id, node);
            }
            None => self.unread.extend(mapping.remote),
        }
        self.store.set_mapping(mapping)
    }

    /// Forgets the mapping of `remote` and removes its local folder.
    async fn purge(&mut self, remote: NodeId) -> Result<(), String> {
        let Some(root) = self
            .mappings
            .iter()
            .find(|m| m.remote == Some(remote))
            .and_then(|m| m.local.clone())
        else {
            return Ok(());
        };
        self.store
            .remove_mapping(Some(remote))
            .map_err(|err| err.to_string())?;
        self.mappings.retain(|m| m.remote != Some(remote));
        self.unread.remove(&remote);
        self.forget_folder(&root).await;
        Ok(())
    }

    /// Removes a local folder no mapping uses any more, and what was recorded under it.
    async fn forget_folder(&mut self, root: &Path) {
        for upload in self.store.saved_uploads().unwrap_or_default() {
            if upload.path.starts_with(root) {
                let _ = apply::discard(&self.client, &self.store, &upload).await;
            }
        }
        for base in self.store.bases().unwrap_or_default() {
            if base.path.starts_with(root)
                && let Err(err) = self.store.forget_base(base.node, Some(&base.path))
            {
                eprintln!("could not record sync state: {err}");
            }
        }
        self.problems.retain(|path, _| !path.starts_with(root));
        self.unreadable.retain(|(path, _)| !path.starts_with(root));
        let target = root.to_path_buf();
        let removed = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(target)).await;
        if let Ok(Err(err)) = removed
            && err.kind() != io::ErrorKind::NotFound
        {
            eprintln!("could not remove {}: {err}", root.display());
        }
    }

    /// Reads the mapped nodes that are still unread.
    async fn read_unread(&mut self) -> Result<(), ApiError> {
        let mapped: HashSet<Option<NodeId>> = self.mappings.iter().map(|m| m.remote).collect();
        for id in self.unread.clone() {
            let delta = self.mirror.read_mapped(&self.client, id, &mapped).await?;
            self.record_remote(&delta);
            self.unread.remove(&id);
        }
        Ok(())
    }

    /// Lets go of the mappings whose node is gone from the server, sending edits still in them
    /// to the rescue folder first. One whose edits can't be sent now stays for a later pass.
    async fn drop_gone_mappings(&mut self) -> Result<(), SignedOut> {
        let gone: Vec<(NodeId, PathBuf)> = self
            .mappings
            .iter()
            .filter_map(|m| Some((m.remote?, m.local.clone()?)))
            .filter(|(id, _)| !self.mirror.nodes.contains_key(id) && !self.unread.contains(id))
            .collect();
        for (id, root) in gone {
            match self.rescue_edits(&root).await {
                Ok(true) => {
                    if let Err(err) = self.purge(id).await {
                        eprintln!("could not remove a mapping: {err}");
                    }
                }
                Ok(false) => {}
                Err(ApiError::Unauthorized) => return Err(SignedOut),
                Err(err) => eprintln!("could not rescue edits: {err}"),
            }
        }
        Ok(())
    }

    /// Sends the files under `root` that differ from their base to the rescue folder. Returns
    /// whether nothing is left to lose.
    async fn rescue_edits(&self, root: &Path) -> Result<bool, ApiError> {
        let bases = self.store.bases().unwrap_or_default();
        let edited: Vec<Base> = bases
            .into_iter()
            .filter(|base| base.path.starts_with(root) && base.is_edited())
            .collect();
        if edited.is_empty() {
            return Ok(true);
        }
        let Some(folder) = self.platform.rescue_folder() else {
            return Ok(false);
        };
        for base in edited {
            let Some(name) = base.path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let modified = std::fs::metadata(&base.path)
                .and_then(|meta| meta.modified())
                .ok()
                .map(OffsetDateTime::from);
            let body = || apply::file_body(&base.path, &self.progress);
            self.client
                .upload_modified(Some(folder), name, modified, body)
                .await?;
        }
        Ok(true)
    }

    /// Where the folder `remote` syncs to, if it does.
    fn synced_root(&self, remote: Option<NodeId>) -> Option<&Path> {
        self.mappings
            .iter()
            .find(|m| m.remote == remote)
            .and_then(|m| m.local.as_deref())
    }

    /// Brings everything mapped in sync once; returns when the next pass is due unless
    /// local or remote changes call for one earlier.
    async fn pass(
        &mut self,
        output: &mut Sender<Update>,
        commands: &mut mpsc::UnboundedReceiver<Command>,
    ) -> Result<Option<Duration>, SignedOut> {
        if self.mappings.iter().all(|m| m.local.is_none()) {
            self.activity = Activity::Idle;
            self.unreadable.clear();
            self.queue.clear();
            self.report(output, &Layout::default(), &[], None).await;
            return Ok(None);
        }
        self.activity = Activity::Checking;
        self.report_now(output).await;
        let refreshed = if self.reload {
            self.mirror.reload(&self.client, &self.mappings).await
        } else {
            self.mirror.refresh(&self.client, &self.mappings).await
        };
        let refreshed = match refreshed {
            Ok(delta) => {
                self.reload = false;
                self.record_remote(&delta);
                self.read_unread().await
            }
            Err(err) => Err(err),
        };
        match refreshed {
            Ok(()) => {}
            Err(ApiError::Unauthorized) => return Err(SignedOut),
            Err(err) => {
                self.activity = Activity::Idle;
                let layout = Layout::new(&self.mirror.nodes, &self.mappings);
                let (delay, error) = if err.is_unreachable() {
                    (self.unreachable(err.to_string()), None)
                } else {
                    (
                        RETRY,
                        Some(format!("cannot read changes from the server: {err}")),
                    )
                };
                self.report(output, &layout, &[], error).await;
                return Ok(Some(delay));
            }
        }
        let private = self.platform.app_owned_copies();
        if private {
            self.drop_gone_mappings().await?;
            for root in self.mappings.iter().filter_map(|m| m.local.as_deref()) {
                if let Err(err) = tokio::fs::create_dir_all(root).await {
                    eprintln!("could not create {}: {err}", root.display());
                }
            }
        }
        let layout = Layout::new(&self.mirror.nodes, &self.mappings);
        let bases = self.store.bases().unwrap_or_default();
        self.activity = Activity::Scanning;
        self.report(output, &layout, &[], None).await;
        let Some(scan) = self.scan(&layout, &bases, commands).await else {
            // A command cut the scan short; it is handled first, then the pass starts over.
            return Ok(Some(Duration::ZERO));
        };
        self.verify.clear();
        self.unreadable = self.faults_of(&scan.unreadable);
        let now = Instant::now();
        let planned = plan::plan_with(&layout, &scan, &bases, private);
        self.discard_uploads(&planned).await;
        // A path with nothing left to do was resolved meanwhile, perhaps by the user.
        self.problems.retain(|path, _| {
            planned
                .iter()
                .any(|action| action.path() == Some(path.as_path()))
        });
        let actions: Vec<Action> = planned
            .into_iter()
            .filter(|action| {
                action.path().is_none_or(|path| {
                    self.problems
                        .get(path)
                        .is_none_or(|problem| problem.retry_at <= now)
                })
            })
            .collect();
        self.queue = queue::items(&actions, &layout.folders);
        if actions.is_empty() {
            // The server answered and nothing is left to send it.
            self.outage = None;
            self.activity = Activity::Idle;
            self.report(output, &layout, &[], None).await;
            return Ok(self.next_retry());
        }
        self.activity = Activity::Syncing;
        self.report(output, &layout, &actions, None).await;
        let outage = self.execute(output, &layout, actions).await?;
        self.activity = Activity::Idle;
        self.queue.clear();
        self.report_now(output).await;
        Ok(Some(outage.unwrap_or(SETTLE)))
    }

    /// Scans the local folders, or gives up with `None` when a command arrives meanwhile
    /// (kept in `interrupted`), since the scan of a big folder takes long.
    async fn scan(
        &mut self,
        layout: &Layout,
        bases: &[model::Base],
        commands: &mut mpsc::UnboundedReceiver<Command>,
    ) -> Option<Scan> {
        let roots = layout.roots.clone();
        let skipped = layout.skipped.clone();
        let rehashed: Vec<PathBuf> = self
            .verify
            .iter()
            .filter_map(|&remote| self.synced_root(remote))
            .map(Path::to_path_buf)
            .collect();
        let known = bases
            .iter()
            .map(|base| (base.path.clone(), base.clone()))
            .collect();
        let remote = layout.entries.keys().cloned().collect();
        let stop = Arc::clone(&self.stop);
        let mut task = tokio::task::spawn_blocking(move || {
            scan::scan(&roots, &skipped, &known, &remote, &rehashed, &stop)
        });
        let blind = || Scan {
            blind: layout.roots.clone(),
            ..Scan::default()
        };
        let command = loop {
            tokio::select! {
                scan = &mut task => return Some(scan.unwrap_or_else(|_| blind())),
                command = commands.recv() => {
                    if command.as_ref().is_some_and(|command| self.is_echo(command)) {
                        continue;
                    }
                    break command;
                }
            }
        };
        // The scan gives up at its next folder or file chunk; wait for it to let go.
        self.stop.store(true, Ordering::Relaxed);
        let _ = task.await;
        self.stop.store(false, Ordering::Relaxed);
        // Without a command the app is going away, which ends the loop.
        self.interrupted = command;
        None
    }

    /// Whether `command` reports a server change the mirror already holds, mostly the
    /// engine's own changes coming back, which need no pass.
    fn is_echo(&self, command: &Command) -> bool {
        matches!(command, Command::RemoteChanged(event) if self.mirror.already_has(event))
    }

    /// Cancels the saved chunked uploads of paths with no upload planned any more, such as
    /// files deleted, or synced some other way, since the upload began.
    async fn discard_uploads(&self, planned: &[Action]) {
        let uploading: HashSet<&Path> = planned
            .iter()
            .filter_map(|action| match action {
                Action::Upload { path, .. } => Some(path.as_path()),
                _ => None,
            })
            .collect();
        let saved = self.store.saved_uploads().unwrap_or_default();
        for upload in saved
            .iter()
            .filter(|upload| !uploading.contains(upload.path.as_path()))
        {
            // One the server cannot be asked about now stays saved for a later pass.
            let _ = apply::discard(&self.client, &self.store, upload).await;
        }
    }

    /// The faults of the paths the scan could not read, keeping when each was first seen.
    fn faults_of(&self, unreadable: &[(PathBuf, String)]) -> Vec<(PathBuf, Fault)> {
        let seen: HashMap<&Path, &Fault> = self
            .unreadable
            .iter()
            .map(|(path, fault)| (path.as_path(), fault))
            .collect();
        unreadable
            .iter()
            .map(|(path, reason)| {
                let fault = seen
                    .get(path.as_path())
                    .filter(|fault| fault.reason == *reason)
                    .map_or_else(
                        || Fault::now(UNREADABLE_CODE, reason.clone()),
                        |fault| (*fault).clone(),
                    );
                (path.clone(), fault)
            })
            .collect()
    }

    /// Runs the actions phase by phase; transfers run several at a time. Returns when to
    /// try again if the server turned out to be unreachable.
    async fn execute(
        &mut self,
        output: &mut Sender<Update>,
        layout: &Layout,
        actions: Vec<Action>,
    ) -> Result<Option<Duration>, SignedOut> {
        let mut folders: Folders = layout.folders.clone();
        let mut breaker = Breaker::default();
        // Everything not finished yet, which the badges show as pending.
        let mut unfinished = actions.clone();
        let mut reported = Instant::now();
        let finish = |unfinished: &mut Vec<Action>, action: &Action| {
            if let Some(at) = unfinished.iter().position(|other| other == action) {
                unfinished.swap_remove(at);
            }
        };
        for phase in [Phase::Move, Phase::Create, Phase::Transfer, Phase::Delete] {
            if breaker.is_tripped() || self.gate.is_closed() {
                break;
            }
            let now: Vec<Action> = actions
                .iter()
                .filter(|action| action.phase() == phase)
                .cloned()
                .collect();
            if phase == Phase::Transfer {
                let snapshot = folders.clone();
                let client = self.client.clone();
                let store = Arc::clone(&self.store);
                let platform = Arc::clone(&self.platform);
                let progress = Arc::clone(&self.progress);
                let halt = Arc::clone(&breaker.halt);
                let shared = apply::Shared {
                    client: &client,
                    store: &store,
                    platform: &platform,
                    folders: &snapshot,
                    progress: &progress,
                };
                let mut outcomes = apply::execute_all(shared, &halt, now, TRANSFERS);
                let mut tick = tokio::time::interval(PROGRESS);
                loop {
                    // A large file reports on the tick, between the ends of transfers.
                    let due = tokio::select! {
                        next = outcomes.next() => {
                            let Some((action, outcome)) = next else { break };
                            finish(&mut unfinished, &action);
                            self.settle(&mut folders, &mut breaker, action, outcome)?;
                            reported.elapsed() >= PROGRESS
                        }
                        _ = tick.tick() => true,
                    };
                    if due {
                        reported = Instant::now();
                        self.queue = queue::items(&unfinished, &folders);
                        self.report_current(output, &unfinished).await;
                    }
                }
            } else {
                // One at a time, so each action sees the folders the previous ones created.
                for action in now {
                    if breaker.is_tripped() || self.gate.is_closed() {
                        break;
                    }
                    let outcome = apply::execute(
                        &self.client,
                        &self.store,
                        &self.platform,
                        &folders,
                        &self.progress,
                        action.clone(),
                    )
                    .await;
                    finish(&mut unfinished, &action);
                    self.settle(&mut folders, &mut breaker, action, outcome)?;
                }
            }
            if reported.elapsed() >= PROGRESS || phase == Phase::Delete {
                reported = Instant::now();
                self.queue = queue::items(&unfinished, &folders);
                self.report_current(output, &unfinished).await;
            }
        }
        Ok(self.conclude(&folders, breaker))
    }

    /// Records how the pass ended: an outage, returning when to try again, or the paths that
    /// failed on their own.
    fn conclude(&mut self, folders: &Folders, breaker: Breaker) -> Option<Duration> {
        match breaker.tripped {
            Some(reason) => Some(self.unreachable(reason)),
            None => {
                // The server answered meanwhile, so these paths failed on their own.
                for (action, fault) in breaker.held {
                    self.fail(folders, &action, fault);
                }
                self.outage = None;
                None
            }
        }
    }

    fn settle(
        &mut self,
        folders: &mut Folders,
        breaker: &mut Breaker,
        action: Action,
        outcome: Result<Vec<Effect>, Failure>,
    ) -> Result<(), SignedOut> {
        match outcome {
            Ok(effects) => {
                breaker.in_row = 0;
                if let Some(path) = action.path() {
                    self.problems.remove(path);
                }
                for effect in effects {
                    self.record(folders, effect);
                }
            }
            Err(Failure::SignedOut) => return Err(SignedOut),
            Err(Failure::Stale | Failure::Halted) => {}
            Err(Failure::Unreachable(fault)) => breaker.failed(action, fault),
            Err(Failure::Error(fault)) => self.fail(folders, &action, fault),
        }
        Ok(())
    }

    /// Records that `action` failed with `fault`, to be tried again after a while.
    fn fail(&mut self, folders: &Folders, action: &Action, fault: Fault) {
        let Some(path) = action.path() else {
            return;
        };
        let attempts = self.problems.get(path).map_or(1, |p| p.attempts + 1);
        self.problems.insert(
            path.to_path_buf(),
            Problem {
                fault,
                attempts,
                retry_at: Instant::now() + backoff(attempts, MAX_BACKOFF),
                item: QueueItem::of(action, folders),
            },
        );
    }

    /// Records that the server could not be reached; returns when to try again.
    fn unreachable(&mut self, reason: String) -> Duration {
        let attempts = self.outage.as_ref().map_or(1, |outage| outage.attempts + 1);
        let delay = backoff(attempts, MAX_OUTAGE_BACKOFF);
        let now = OffsetDateTime::now_utc();
        self.outage = Some(Outage {
            reason,
            since: self.outage.as_ref().map_or(now, |outage| outage.since),
            retry_at: now + delay,
            attempts,
        });
        delay
    }

    fn record(&mut self, folders: &mut Folders, effect: Effect) {
        let stored = match effect {
            Effect::Base(base) => self.store.put_base(&base),
            Effect::Forget(node, at) => self.store.forget_base(node, at.as_deref()),
            Effect::Remote(node) => {
                self.mirror.nodes.insert(node.id, node.clone());
                self.store.update_remote(false, &[node], &[])
            }
            Effect::RemoteGone(id) => {
                self.mirror.nodes.remove(&id);
                self.store.update_remote(false, &[], &[id])
            }
            Effect::Rebase { from, to } => {
                *folders = std::mem::take(folders)
                    .into_iter()
                    .map(|(path, id)| (model::rebased(&path, &from, &to).unwrap_or(path), id))
                    .collect();
                self.store.rebase(&from, &to)
            }
            Effect::Folder(path, id) => {
                folders.insert(path, Some(id));
                Ok(())
            }
        };
        if let Err(err) = stored {
            eprintln!("could not record sync state: {err}");
        }
    }

    fn record_remote(&mut self, delta: &Delta) {
        let stored = self
            .store
            .update_remote(delta.replace, &delta.upserted, &delta.removed)
            .and_then(|()| self.store.set_cursor(self.mirror.cursor));
        if let Err(err) = stored {
            eprintln!("could not record the remote tree: {err}");
        }
    }

    /// When the earliest failed path may be tried again.
    fn next_retry(&self) -> Option<Duration> {
        self.problems
            .values()
            .map(|problem| problem.retry_at.saturating_duration_since(Instant::now()))
            .min()
    }

    /// Reports with the layout of what is known now, between passes.
    async fn report_now(&self, output: &mut Sender<Update>) {
        self.report_current(output, &[]).await;
    }

    /// Reports with the layout of what is known now, so that what a running pass has finished
    /// already counts as synced.
    async fn report_current(&self, output: &mut Sender<Update>, pending: &[Action]) {
        let layout = Layout::new(&self.mirror.nodes, &self.mappings);
        self.report(output, &layout, pending, None).await;
    }

    async fn report(
        &self,
        output: &mut Sender<Update>,
        layout: &Layout,
        pending: &[Action],
        error: Option<String>,
    ) {
        let bases = self.store.bases().unwrap_or_default();
        let pending_paths: Vec<PathBuf> = pending
            .iter()
            .filter_map(|action| action.path().map(Path::to_path_buf))
            .collect();
        let problem_paths: Vec<PathBuf> = self
            .problems
            .keys()
            .chain(self.unreadable.iter().map(|(path, _)| path))
            .cloned()
            .collect();
        let badges = status::badges(
            layout,
            &self.mirror.nodes,
            &status::based(&bases),
            &pending_paths,
            &problem_paths,
        );
        let mut problems: Vec<(PathBuf, Fault)> = self
            .problems
            .iter()
            .map(|(path, problem)| (path.clone(), problem.fault.clone()))
            .chain(self.unreadable.iter().cloned())
            .collect();
        problems.sort_by(|a, b| a.0.cmp(&b.0));
        let names = self
            .mappings
            .iter()
            .filter_map(|mapping| mapping.remote)
            .filter_map(|id| Some((id, self.mirror.nodes.get(&id)?.name.clone())))
            .collect();
        let locations = layout
            .folders
            .iter()
            .map(|(path, id)| (*id, path.clone()))
            .collect();
        let moving = queue::snapshot(&self.progress);
        let mut queue: Vec<QueueItem> = self
            .queue
            .iter()
            .cloned()
            .map(|item| item.with_progress(&moving))
            .collect();
        // What is moving now first; the sort keeps the order of the rest.
        queue.sort_by_key(|item| item.state != ItemState::Active);
        let mut failed: Vec<QueueItem> = self
            .problems
            .iter()
            .map(|(path, problem)| match &problem.item {
                Some(item) => item.clone().failed(problem.fault.clone()),
                None => QueueItem::blocked(path, problem.fault.clone()),
            })
            .chain(
                self.unreadable
                    .iter()
                    .map(|(path, fault)| QueueItem::blocked(path, fault.clone())),
            )
            .collect();
        failed.sort_by(|a, b| a.path.cmp(&b.path));
        queue.extend(failed);
        let status = Status {
            badges,
            mappings: self.mappings.clone(),
            names,
            locations,
            files: layout.files(),
            bases: bases.into_iter().map(|base| (base.node, base)).collect(),
            pending: pending_paths.len(),
            problems,
            error,
            outage: self.outage.clone(),
            activity: self.activity,
            queue,
            verifying: self.verify.clone(),
        };
        let _ = output.send(Update::Status(Arc::new(status))).await;
    }
}

/// Tells a server that cannot be reached from paths that fail on their own, during one pass.
#[derive(Default)]
struct Breaker {
    /// Unreachable-server failures since the last action that worked.
    in_row: u32,
    /// Actions that failed as unreachable, held until it is clear whether the server was down.
    held: Vec<(Action, Fault)>,
    /// Why the pass stopped as an outage.
    tripped: Option<String>,
    /// Set once tripped, so no further transfer starts.
    halt: Arc<AtomicBool>,
}

impl Breaker {
    fn failed(&mut self, action: Action, fault: Fault) {
        self.in_row += 1;
        if self.in_row >= OUTAGE_FAILURES && self.tripped.is_none() {
            self.tripped = Some(fault.reason.clone());
            self.halt.store(true, Ordering::Relaxed);
        }
        self.held.push((action, fault));
    }

    fn is_tripped(&self) -> bool {
        self.tripped.is_some()
    }
}

/// The wait after `attempts` failures: [`RETRY`], doubling each time up to `max`.
fn backoff(attempts: u32, max: Duration) -> Duration {
    RETRY
        .saturating_mul(2u32.saturating_pow(attempts.saturating_sub(1)))
        .min(max)
}

#[cfg(test)]
mod tests {
    use super::{model::LocalEntry, *};
    use protocol::NodeKind;

    struct Desktoplike;

    impl Platform for Desktoplike {
        fn app_owned_copies(&self) -> bool {
            false
        }
    }

    const UNREACHABLE: &str = "server unreachable (502 Bad Gateway)";

    fn engine() -> Engine {
        Engine::load(
            Client::new("http://127.0.0.1:9"),
            Store::in_memory().expect("store"),
            Arc::new(Desktoplike),
        )
        .expect("engine")
    }

    fn upload(name: &str) -> Action {
        Action::Upload {
            node: None,
            path: PathBuf::from("/sync").join(name),
            local: LocalEntry {
                kind: NodeKind::File,
                size: 1,
                mtime: 0,
                sha256: None,
                unhashed: false,
            },
        }
    }

    fn folders() -> Folders {
        Folders::from([(PathBuf::from("/sync"), None)])
    }

    fn unreachable() -> Result<Vec<Effect>, Failure> {
        Err(Failure::Unreachable(Fault::now("502", UNREACHABLE)))
    }

    fn settle_all(
        engine: &mut Engine,
        breaker: &mut Breaker,
        outcomes: Vec<(&str, Result<Vec<Effect>, Failure>)>,
    ) {
        let mut folders = folders();
        for (name, outcome) in outcomes {
            assert!(
                engine
                    .settle(&mut folders, breaker, upload(name), outcome)
                    .is_ok()
            );
        }
    }

    #[tokio::test]
    async fn an_upload_counts_as_synced_before_the_pass_ends() {
        let dir = std::env::temp_dir().join(format!("home-space-report-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("folder");
        let path = dir.join("photo.jpg");
        std::fs::write(&path, "photo").expect("file");
        let meta = std::fs::metadata(&path).expect("meta");
        let sha256 = hash_file(&path, &std::sync::atomic::AtomicBool::new(false)).expect("hash");
        let id = "0193a4a0-0000-7000-8000-0000000000aa";
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(protocol::paths::FILES))
            .respond_with(
                wiremock::ResponseTemplate::new(201).set_body_json(serde_json::json!({
                    "id": id, "parent_id": null, "kind": "file", "name": "photo.jpg",
                    "size": meta.len(), "sha256": sha256,
                    "mime": "image/jpeg", "version": 1,
                    "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(protocol::paths::LOGIN))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "access_token": "token", "refresh_token": "refresh", "expires_in": 900
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(protocol::paths::ME))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "00000000-0000-0000-0000-000000000001",
                    "username": "alice",
                    "lost_and_found_id": "00000000-0000-0000-0000-0000000000f0"
                })),
            )
            .mount(&server)
            .await;
        let client = Client::new(server.uri());
        client.login("alice", "pw").await.expect("login");
        let mut engine = Engine::load(
            client,
            Store::in_memory().expect("store"),
            Arc::new(Desktoplike),
        )
        .expect("engine");
        engine.mappings = vec![Mapping {
            remote: None,
            local: Some(dir.clone()),
        }];
        let layout = Layout::new(&engine.mirror.nodes, &engine.mappings);
        let action = Action::Upload {
            node: None,
            path: path.clone(),
            local: LocalEntry {
                kind: NodeKind::File,
                size: meta.len(),
                mtime: nanos(meta.modified().expect("mtime")),
                sha256: Some(sha256),
                unhashed: false,
            },
        };
        let (mut output, mut updates) = futures::channel::mpsc::channel(16);

        assert!(
            engine
                .execute(&mut output, &layout, vec![action])
                .await
                .is_ok()
        );

        drop(output);
        let mut last = None;
        while let Ok(update) = updates.try_recv() {
            if let Update::Status(status) = update {
                last = Some(status);
            }
        }
        let status = last.expect("a status during the pass");
        let node = NodeId(id.parse().expect("id"));
        assert_eq!(status.file_path(node), Some(path.as_path()));
        assert_eq!(status.badge(Some(node)), Some(Badge::Synced));
        assert!(status.base(node).is_some());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn unreachable_failures_in_a_row_are_one_outage_and_no_file_problems() {
        let mut engine = engine();
        let mut breaker = Breaker::default();
        settle_all(
            &mut engine,
            &mut breaker,
            vec![
                ("a", unreachable()),
                ("b", unreachable()),
                ("c", unreachable()),
            ],
        );
        assert!(breaker.halt.load(Ordering::Relaxed));
        let delay = engine.conclude(&folders(), breaker);
        assert_eq!(delay, Some(RETRY));
        assert!(engine.problems.is_empty());
        let outage = engine.outage.as_ref().expect("outage");
        assert_eq!(outage.reason, UNREACHABLE);
    }

    #[test]
    fn an_outage_waits_longer_each_time() {
        let mut engine = engine();
        assert_eq!(engine.unreachable(UNREACHABLE.to_owned()), RETRY);
        assert_eq!(engine.unreachable(UNREACHABLE.to_owned()), RETRY * 2);
        let since = engine.outage.as_ref().map(|outage| outage.since);
        engine.unreachable(UNREACHABLE.to_owned());
        assert_eq!(engine.outage.as_ref().map(|outage| outage.since), since);
    }

    #[test]
    fn unreachable_failures_between_successes_are_file_problems() {
        let mut engine = engine();
        engine.unreachable(UNREACHABLE.to_owned());
        let mut breaker = Breaker::default();
        settle_all(
            &mut engine,
            &mut breaker,
            vec![
                ("a", unreachable()),
                ("b", unreachable()),
                ("c", Ok(Vec::new())),
                ("d", unreachable()),
            ],
        );
        assert!(!breaker.halt.load(Ordering::Relaxed));
        assert_eq!(engine.conclude(&folders(), breaker), None);
        assert_eq!(engine.problems.len(), 3);
        assert!(engine.outage.is_none());
    }

    #[test]
    fn a_file_error_is_a_problem_with_its_code() {
        let mut engine = engine();
        let mut breaker = Breaker::default();
        let too_large = Failure::from(ApiError::Status {
            status: 413,
            reason: "Payload Too Large".to_owned(),
        });
        settle_all(&mut engine, &mut breaker, vec![("big", Err(too_large))]);
        let problem = engine
            .problems
            .get(Path::new("/sync/big"))
            .expect("problem");
        assert_eq!(problem.fault.code, "413");
        assert_eq!(problem.attempts, 1);
    }

    #[tokio::test]
    async fn retry_makes_the_chosen_problems_due_now() {
        let mut engine = engine();
        let mut breaker = Breaker::default();
        let failed = || Err(Failure::Error(Fault::now("local", "busy")));
        settle_all(
            &mut engine,
            &mut breaker,
            vec![("a", failed()), ("b", failed())],
        );
        engine.unreachable(UNREACHABLE.to_owned());
        let (mut output, _updates) = futures::channel::mpsc::channel(4);
        let chosen = PathBuf::from("/sync/a");
        engine
            .handle(Command::Retry(Some(vec![chosen.clone()])), &mut output)
            .await;
        let now = Instant::now();
        let due = |path: &Path| engine.problems.get(path).expect("problem").retry_at <= now;
        assert!(due(&chosen));
        assert!(!due(Path::new("/sync/b")));
        assert!(engine.outage.is_none());
    }

    async fn mapped_engine() -> (Engine, Sender<Update>) {
        let mut engine = engine();
        let (mut output, _updates) = futures::channel::mpsc::channel(4);
        engine
            .handle(
                Command::Map(Mapping {
                    remote: None,
                    local: Some(PathBuf::from("/sync")),
                }),
                &mut output,
            )
            .await;
        engine.reload = false;
        (engine, output)
    }

    #[tokio::test]
    async fn verify_reads_the_server_again_and_rehashes_the_folder() {
        let (mut engine, mut output) = mapped_engine().await;

        engine.handle(Command::Verify(None), &mut output).await;

        assert!(engine.reload);
        assert!(engine.verify.contains(&None));
    }

    #[tokio::test]
    async fn verify_of_a_folder_not_synced_does_nothing() {
        let (mut engine, mut output) = mapped_engine().await;
        let other = NodeId::from(uuid::Uuid::from_u128(7));

        engine
            .handle(Command::Verify(Some(other)), &mut output)
            .await;

        assert!(!engine.reload);
        assert!(engine.verify.is_empty());
    }

    #[tokio::test]
    async fn unmapping_drops_a_pending_verify() {
        let (mut engine, mut output) = mapped_engine().await;
        engine.handle(Command::Verify(None), &mut output).await;

        engine.handle(Command::Unmap(None), &mut output).await;

        assert!(engine.verify.is_empty());
    }
}
