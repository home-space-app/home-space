//! One pass over the sources: list, plan and upload, one file at a time.
//!
//! A source that can tell what changed, such as a phone's photo library, isn't listed every time. It
//! gets a token once a pass has dealt with everything, and later passes ask only for what changed
//! since. The token stays put while anything is unfinished, so a file that failed or was held back
//! comes back in the next answer, with no separate queue of work. A listing every so often catches
//! anything the platform failed to report.
//!
//! A file that fails waits longer after each failure. When the server can't be reached at all,
//! the failures are not the files' fault: after a few in a row the pass stops, and the next one
//! waits with a growing delay, as in the desktop client's sync. Stopping (the network or the
//! settings no longer allow it, the OS wants the app to finish) takes effect between chunks. It
//! keeps the upload on the server to continue from.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use api_client::{ApiError, Client};
use protocol::{CreateUpload, ErrorCode, Node, NodeId, UploadSession};
use time::OffsetDateTime;
use uuid::Uuid;

use super::plan::{self, Item, Plan, Record, Step};
use super::store::{SavedUpload, SourceRow, Store};
use super::{CurrentUpload, LocalItem, LocalSources, PassSummary, SourceKind};
use crate::CoreError;

/// Largest chunk sent, whatever the server allows: a chunk is held in memory, and it is what is
/// sent again after a dropped connection.
const MAX_CHUNK_BYTES: u64 = 8 * 1024 * 1024;
const RETRY: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(10 * 60);
/// Unreachable-server failures in a row that make a pass stop as an outage.
const OUTAGE_FAILURES: u32 = 3;
const MAX_OUTAGE_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// How long a source that reports its changes goes between listings, in nanoseconds.
const FULL_SCAN_NANOS: i64 = 7 * 24 * 60 * 60 * 1_000_000_000;
/// Why a source is paused when its server folder is gone.
pub(super) const TARGET_GONE: &str = "The folder on the server no longer exists. Choose another.";

/// When a file or the whole pass may be tried again.
#[derive(Default)]
pub(super) struct Backoff {
    files: HashMap<(String, String), (u32, Instant)>,
    outage: Option<(u32, Instant)>,
}

fn delay(failures: u32, first: Duration, max: Duration) -> Duration {
    first
        .saturating_mul(2u32.saturating_pow(failures.saturating_sub(1)))
        .min(max)
}

impl Backoff {
    fn outage_active(&self, now: Instant) -> bool {
        self.outage.is_some_and(|(_, until)| now < until)
    }

    fn file_waiting(&self, source: &str, key: &str, now: Instant) -> bool {
        self.files
            .get(&(source.to_owned(), key.to_owned()))
            .is_some_and(|(_, until)| now < *until)
    }

    fn file_failed(&mut self, source: &str, key: &str, now: Instant) {
        let entry = self
            .files
            .entry((source.to_owned(), key.to_owned()))
            .or_insert((0, now));
        entry.0 += 1;
        entry.1 = now + delay(entry.0, RETRY, MAX_BACKOFF);
    }

    fn file_sent(&mut self, source: &str, key: &str) {
        self.files.remove(&(source.to_owned(), key.to_owned()));
    }

    fn outage_began(&mut self, now: Instant) {
        let failures = self.outage.map_or(0, |(failures, _)| failures) + 1;
        self.outage = Some((failures, now + delay(failures, RETRY, MAX_OUTAGE_BACKOFF)));
    }

    /// Everything may be tried at once: the user asked for it.
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// How a file's upload ended, other than with success.
#[derive(Debug)]
enum Failure {
    /// Told to stop; the file is simply still pending.
    Stopped,
    /// Changed while it was going up, or can't be read for now. It stays pending.
    Local(String),
    /// A folder on the way to the file's place is gone.
    FolderGone,
    Api(ApiError),
}

impl From<ApiError> for Failure {
    fn from(err: ApiError) -> Self {
        Self::Api(err)
    }
}

impl From<rusqlite::Error> for Failure {
    fn from(err: rusqlite::Error) -> Self {
        Self::Local(err.to_string())
    }
}

/// What a source's token becomes once everything in its plan is done.
enum NextToken {
    /// Asked what changed, and got this for next time.
    Changes(String),
    /// Listed it. `None` if this source can't tell what changed.
    Listing(Option<String>),
}

/// The work on a source, and where that leaves it.
struct Prepared {
    plan: Plan,
    next: NextToken,
}

/// What a source holds now, or what changed in it.
struct Found {
    items: Vec<LocalItem>,
    next: NextToken,
}

/// Asks what changed if the token allows it, and lists the source otherwise.
fn look_at(
    local: &dyn LocalSources,
    kind: SourceKind,
    handle: &str,
    token: Option<String>,
) -> Result<Found, CoreError> {
    if let Some(token) = token
        && let Some(changes) = local.changes_since(kind, handle.to_owned(), token)?
    {
        return Ok(Found {
            items: changes.items,
            next: NextToken::Changes(changes.token),
        });
    }
    // First, so that a change made during the listing shows up next time. Without a token the
    // source is only listed again, so a failure here costs nothing.
    let token = local.change_token(kind, handle.to_owned()).ok().flatten();
    Ok(Found {
        items: local.list(kind, handle.to_owned())?,
        next: NextToken::Listing(token),
    })
}

/// Everything a pass works with.
pub(super) struct Pass<'a> {
    pub client: &'a Client,
    pub store: &'a Store,
    pub local: Arc<dyn LocalSources>,
    pub stop: Arc<AtomicBool>,
    pub backoff: &'a Mutex<Backoff>,
    /// Why the last file of each source failed.
    pub errors: &'a Mutex<HashMap<String, String>>,
    /// Told after each thing worth showing; takes the files still to go per source.
    pub report: &'a (dyn Fn(&str, u64) + Send + Sync),
    pub current: &'a Mutex<Option<CurrentUpload>>,
    pub changed: &'a (dyn Fn() + Send + Sync),
}

pub(super) fn nanos(time: SystemTime) -> i64 {
    let nanos = OffsetDateTime::from(time).unix_timestamp_nanos();
    i64::try_from(nanos).unwrap_or(if nanos < 0 { i64::MIN } else { i64::MAX })
}

fn to_item(item: LocalItem) -> Item {
    Item {
        mtime: nanos(item.modified_at),
        key: item.key,
        dir: item.relative_dir,
        name: item.name,
        size: item.size,
        mime: item.mime,
    }
}

fn is_not_found(err: &ApiError) -> bool {
    matches!(
        err,
        ApiError::Api {
            code: ErrorCode::NotFound,
            ..
        }
    )
}

fn kind_of(row: &SourceRow) -> SourceKind {
    if row.kind == super::KIND_PHOTOS {
        SourceKind::Photos
    } else {
        SourceKind::Folder
    }
}

impl Pass<'_> {
    pub(super) async fn run(&self) -> Result<PassSummary, CoreError> {
        let mut summary = PassSummary::default();
        let backoff_active = self
            .backoff
            .lock()
            .expect("backoff lock")
            .outage_active(Instant::now());
        if backoff_active {
            summary.outage = true;
            return Ok(summary);
        }
        let mut unreachable = 0;
        let sources = self.store.sources().map_err(storage_error)?;
        'sources: for source in sources {
            if self.stopped() {
                summary.stopped = true;
                break;
            }
            // A deleted target stays paused until the user picks another. Others try again:
            // a permission can come back.
            if source.problem.as_deref() == Some(TARGET_GONE) {
                continue;
            }
            let Some(Prepared { plan, next }) = self.prepare(&source, &mut summary).await? else {
                if summary.outage {
                    break;
                }
                continue;
            };
            let mut remaining = plan.steps.len() as u64 + plan.settling as u64;
            summary.pending += remaining;
            (self.report)(&source.id, remaining);
            let mut clean = plan.settling == 0;
            for step in plan.steps {
                let key = match &step {
                    Step::First(item) | Step::Again { item, .. } => item.key.clone(),
                };
                let waiting = self.backoff.lock().expect("backoff lock").file_waiting(
                    &source.id,
                    &key,
                    Instant::now(),
                );
                if waiting {
                    clean = false;
                    continue;
                }
                if self.stopped() {
                    summary.stopped = true;
                    break 'sources;
                }
                match self.step(&source, step).await {
                    Ok(sent) => {
                        unreachable = 0;
                        summary.uploaded += u64::from(sent);
                        summary.pending = summary.pending.saturating_sub(1);
                        remaining = remaining.saturating_sub(1);
                        (self.report)(&source.id, remaining);
                        self.backoff
                            .lock()
                            .expect("backoff lock")
                            .file_sent(&source.id, &key);
                    }
                    Err(Failure::Stopped) => {
                        summary.stopped = true;
                        break 'sources;
                    }
                    Err(Failure::FolderGone) => {
                        self.store
                            .set_problem(&source.id, Some(TARGET_GONE))
                            .map_err(storage_error)?;
                        continue 'sources;
                    }
                    Err(Failure::Local(detail)) => {
                        clean = false;
                        summary.failed += 1;
                        self.note_error(&source.id, &detail);
                        summary.last_error = Some(detail);
                        self.file_failed(&source.id, &key);
                    }
                    Err(Failure::Api(ApiError::Unauthorized)) => {
                        return Err(CoreError::Unauthorized);
                    }
                    Err(Failure::Api(err)) => {
                        clean = false;
                        if err.is_unreachable() {
                            unreachable += 1;
                            if unreachable >= OUTAGE_FAILURES {
                                summary.outage = true;
                                self.backoff
                                    .lock()
                                    .expect("backoff lock")
                                    .outage_began(Instant::now());
                                break 'sources;
                            }
                        } else {
                            unreachable = 0;
                            summary.failed += 1;
                            self.note_error(&source.id, &err.to_string());
                            summary.last_error = Some(err.to_string());
                        }
                        self.file_failed(&source.id, &key);
                    }
                }
            }
            if clean {
                self.errors.lock().expect("errors lock").remove(&source.id);
                self.store
                    .set_last_synced(&source.id, nanos(SystemTime::now()))
                    .map_err(storage_error)?;
                self.keep_token(&source.id, next).map_err(storage_error)?;
            }
        }
        Ok(summary)
    }

    fn note_error(&self, source: &str, detail: &str) {
        self.errors
            .lock()
            .expect("errors lock")
            .insert(source.to_owned(), detail.to_owned());
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn file_failed(&self, source: &str, key: &str) {
        self.backoff
            .lock()
            .expect("backoff lock")
            .file_failed(source, key, Instant::now());
    }

    /// Lists a source and plans its work. `None` when it can't be worked on now, which it records
    /// as a problem or, for an unreachable server, in the summary.
    async fn prepare(
        &self,
        source: &SourceRow,
        summary: &mut PassSummary,
    ) -> Result<Option<Prepared>, CoreError> {
        if let Some(target) = &source.target {
            let id = crate::parse_id(target)?;
            match self.client.get(id).await {
                Ok(_) => {}
                Err(err) if is_not_found(&err) => {
                    self.store
                        .set_problem(&source.id, Some(TARGET_GONE))
                        .map_err(storage_error)?;
                    return Ok(None);
                }
                Err(ApiError::Unauthorized) => return Err(CoreError::Unauthorized),
                Err(err) if err.is_unreachable() => {
                    summary.outage = true;
                    self.backoff
                        .lock()
                        .expect("backoff lock")
                        .outage_began(Instant::now());
                    return Ok(None);
                }
                Err(err) => return Err(err.into()),
            }
        }
        let token = self
            .store
            .change_token(&source.id)
            .map_err(storage_error)?
            .filter(|(_, full_scan_at)| {
                nanos(SystemTime::now()).saturating_sub(*full_scan_at) < FULL_SCAN_NANOS
            })
            .map(|(token, _)| token);
        let found = {
            let local = self.local.clone();
            let handle = source.handle.clone();
            let kind = kind_of(source);
            tokio::task::spawn_blocking(move || look_at(local.as_ref(), kind, &handle, token))
                .await
                .map_err(|err| CoreError::Failed {
                    code: "local".to_owned(),
                    detail: err.to_string(),
                })?
        };
        let (items, next) = match found {
            Ok(found) => (
                found.items.into_iter().map(to_item).collect::<Vec<Item>>(),
                found.next,
            ),
            Err(err) => {
                // Such as a permission the user took back. Nothing is lost by waiting.
                self.store
                    .set_problem(&source.id, Some(&err.to_string()))
                    .map_err(storage_error)?;
                return Ok(None);
            }
        };
        self.store
            .set_problem(&source.id, None)
            .map_err(storage_error)?;
        let records = match next {
            NextToken::Changes(_) => self
                .store
                .records_of(&source.id, items.iter().map(|item| item.key.as_str())),
            NextToken::Listing(_) => self.store.records(&source.id),
        }
        .map_err(storage_error)?;
        let plan = plan::plan(
            &items,
            &records,
            source.leave_out_existing,
            nanos(SystemTime::now()),
        );
        if source.leave_out_existing {
            // All at once: a pass that ends early must not find some of them unrecorded next time.
            self.store
                .leave_out(&source.id, &plan.left_out)
                .map_err(storage_error)?;
        }
        self.discard_stale(source, &plan).await?;
        Ok(Some(Prepared { plan, next }))
    }

    /// Everything the source held up to `next` is dealt with.
    fn keep_token(&self, source: &str, next: NextToken) -> rusqlite::Result<()> {
        match next {
            NextToken::Changes(token) => {
                // Only a listing that went through counts as one.
                let full_scan_at = self
                    .store
                    .change_token(source)?
                    .map_or(0, |(_, full_scan_at)| full_scan_at);
                self.store.save_change_token(source, &token, full_scan_at)
            }
            NextToken::Listing(Some(token)) => {
                self.store
                    .save_change_token(source, &token, nanos(SystemTime::now()))
            }
            NextToken::Listing(None) => self.store.forget_change_token(source),
        }
    }

    /// Cancels the saved uploads of files that no longer need sending, or that changed.
    async fn discard_stale(&self, source: &SourceRow, plan: &Plan) -> Result<(), CoreError> {
        let wanted: HashSet<&str> = plan
            .steps
            .iter()
            .map(|step| match step {
                Step::First(item) | Step::Again { item, .. } => item.key.as_str(),
            })
            .collect();
        for saved in self
            .store
            .saved_uploads(&source.id)
            .map_err(storage_error)?
        {
            if !wanted.contains(saved.key.as_str()) {
                let _ = self.client.cancel_upload(saved.id).await;
                self.store
                    .forget_upload(&source.id, &saved.key)
                    .map_err(storage_error)?;
            }
        }
        Ok(())
    }

    /// Carries out one step; `true` if something was sent.
    async fn step(&self, source: &SourceRow, step: Step) -> Result<bool, Failure> {
        let (item, name, dir, version_of) = match step {
            Step::First(item) => {
                let (name, dir) = (item.name.clone(), item.dir.clone());
                (item, name, dir, None)
            }
            Step::Again {
                item,
                name,
                dir,
                node,
            } => (item, name, dir, node),
        };
        let node = match self.send(source, &item, &name, &dir, version_of).await {
            Err(Failure::Api(err)) if is_not_found(&err) => {
                // A folder below the target went away, or the upload did. Once more from the top.
                self.store.forget_folders(&source.id)?;
                self.send(source, &item, &name, &dir, version_of).await?
            }
            other => other?,
        };
        let record = Record {
            key: item.key,
            size: item.size,
            mtime: item.mtime,
            remote_name: Some(node.name),
            remote_dir: dir,
            remote_node: Some(node.id),
        };
        self.store.save_record(&source.id, &record)?;
        Ok(true)
    }

    async fn send(
        &self,
        source: &SourceRow,
        item: &Item,
        name: &str,
        dir: &str,
        version_of: Option<NodeId>,
    ) -> Result<Node, Failure> {
        let parent = self.folder(source, dir).await?;
        let wanted = SavedUpload {
            source: source.id.clone(),
            key: item.key.clone(),
            id: Uuid::nil(),
            parent: parent.map(|id| id.to_string()),
            name: name.to_owned(),
            size: item.size,
            mtime: item.mtime,
        };
        let session = match self.resumable(&wanted).await? {
            Some(session) => session,
            None => {
                let request = CreateUpload {
                    parent_id: parent,
                    name: name.to_owned(),
                    size: item.size,
                    modified_at: OffsetDateTime::from_unix_timestamp_nanos(i128::from(item.mtime))
                        .ok(),
                    mime: item.mime.clone(),
                    node_id: None,
                    keep_both: true,
                    version_of,
                    auto_upload: true,
                    transfer_item_id: None,
                };
                let session = self.client.create_upload(&request).await?;
                self.store.save_upload(&SavedUpload {
                    id: session.id,
                    ..wanted
                })?;
                session
            }
        };
        self.push(source, item, session).await
    }

    /// The saved upload of this file, if it still matches and the server still has it.
    async fn resumable(&self, wanted: &SavedUpload) -> Result<Option<UploadSession>, Failure> {
        let saved = self
            .store
            .saved_uploads(&wanted.source)?
            .into_iter()
            .find(|saved| saved.key == wanted.key);
        let Some(saved) = saved else {
            return Ok(None);
        };
        if saved.parent == wanted.parent
            && saved.name == wanted.name
            && saved.size == wanted.size
            && saved.mtime == wanted.mtime
        {
            match self.client.upload_status(saved.id).await {
                Ok(session) => return Ok(Some(session)),
                Err(err) if is_not_found(&err) => {}
                Err(err) => return Err(err.into()),
            }
        } else {
            let _ = self.client.cancel_upload(saved.id).await;
        }
        self.store.forget_upload(&wanted.source, &wanted.key)?;
        Ok(None)
    }

    /// Sends the rest of the file, keeping the upload on the server unless retrying can't help.
    async fn push(
        &self,
        source: &SourceRow,
        item: &Item,
        mut session: UploadSession,
    ) -> Result<Node, Failure> {
        session.chunk_size = session.chunk_size.min(MAX_CHUNK_BYTES);
        let reading = Reading::default();
        self.show_current(Some(CurrentUpload {
            source_id: source.id.clone(),
            name: item.name.clone(),
            done: session.offset,
            total: item.size,
        }));
        let sent = self
            .client
            .continue_upload(
                session,
                item.size,
                |offset, len| self.read(source, item, offset, len, &reading),
                |done| {
                    if let Some(current) =
                        self.current.lock().expect("current upload lock").as_mut()
                    {
                        current.done = done;
                    }
                    (self.changed)();
                },
            )
            .await;
        self.show_current(None);
        match sent {
            Ok(node) => {
                self.store.forget_upload(&source.id, &item.key)?;
                Ok(node)
            }
            Err(_) if reading.stopped.load(Ordering::Relaxed) => Err(Failure::Stopped),
            Err(err) if reading.unreadable.load(Ordering::Relaxed) => {
                Err(Failure::Local(err.to_string()))
            }
            Err(err) if reading.changed.load(Ordering::Relaxed) => {
                let _ = self.client.cancel_upload(session.id).await;
                self.store.forget_upload(&source.id, &item.key)?;
                Err(Failure::Local(err.to_string()))
            }
            Err(err) if err.is_transient() || err == ApiError::Unauthorized => Err(err.into()),
            Err(err) => {
                // The server will not take the rest; the next try starts over.
                let _ = self.client.cancel_upload(session.id).await;
                self.store.forget_upload(&source.id, &item.key)?;
                Err(err.into())
            }
        }
    }

    fn show_current(&self, current: Option<CurrentUpload>) {
        *self.current.lock().expect("current upload lock") = current;
        (self.changed)();
    }

    /// Reads `len` bytes of the file. Fails on being told to stop, or if the file is no longer the
    /// one being sent, which is checked before the last chunk stores it on the server.
    async fn read(
        &self,
        source: &SourceRow,
        item: &Item,
        offset: u64,
        len: usize,
        reading: &Reading,
    ) -> std::io::Result<Vec<u8>> {
        if self.stopped() {
            reading.stopped.store(true, Ordering::Relaxed);
            return Err(std::io::Error::other("stopped"));
        }
        let local = self.local.clone();
        let handle = source.handle.clone();
        let kind = kind_of(source);
        let key = item.key.clone();
        let (size, mtime, last) = (item.size, item.mtime, offset + len as u64 >= item.size);
        let read = tokio::task::spawn_blocking(move || {
            let read = local.read(kind, handle.clone(), key.clone(), offset, len as u64);
            // After reading: a change just before the read is seen too.
            if last {
                let now = local.stat(kind, handle, key)?;
                let same =
                    now.is_some_and(|now| now.size == size && nanos(now.modified_at) == mtime);
                if !same {
                    return Ok(None);
                }
            }
            read.map(Some)
        })
        .await
        .map_err(std::io::Error::other)?
        .map_err(|err: CoreError| {
            reading.unreadable.store(true, Ordering::Relaxed);
            std::io::Error::other(err.to_string())
        })?;
        match read {
            Some(bytes) if bytes.len() == len => Ok(bytes),
            _ => {
                reading.changed.store(true, Ordering::Relaxed);
                Err(std::io::Error::other("the file changed while uploading"))
            }
        }
    }

    /// The server folder at `dir` below the source's target, made if missing.
    async fn folder(&self, source: &SourceRow, dir: &str) -> Result<Option<NodeId>, Failure> {
        let target = match &source.target {
            Some(target) => Some(target.parse::<NodeId>().map_err(|_| Failure::FolderGone)?),
            None => None,
        };
        let mut parent = target;
        let mut path = String::new();
        for part in dir.split('/').filter(|part| !part.is_empty()) {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(part);
            if let Some(known) = self.store.folder(&source.id, &path)? {
                parent = known.parse().ok();
                continue;
            }
            let node = match self.client.create_folder(parent, part).await {
                Ok(node) => node,
                Err(ApiError::Api {
                    code: ErrorCode::Conflict,
                    ..
                }) => {
                    let siblings = self.client.list(parent).await?;
                    siblings
                        .into_iter()
                        .find(|node| node.kind == protocol::NodeKind::Folder && node.name == part)
                        .ok_or_else(|| {
                            Failure::Local(format!("the folder {part} could not be made"))
                        })?
                }
                Err(err) if is_not_found(&err) && parent == target => {
                    return Err(Failure::FolderGone);
                }
                Err(err) => return Err(err.into()),
            };
            self.store
                .save_folder(&source.id, &path, &node.id.to_string())?;
            parent = Some(node.id);
        }
        Ok(parent)
    }
}

/// What went wrong while reading a file for its upload, which the upload only sees as an error.
#[derive(Default)]
struct Reading {
    stopped: AtomicBool,
    changed: AtomicBool,
    unreadable: AtomicBool,
}

pub(super) fn storage_error(err: rusqlite::Error) -> CoreError {
    CoreError::Failed {
        code: "storage".to_owned(),
        detail: err.to_string(),
    }
}
