//! Files the user picks to upload. They wait in a queue kept on disk and go up one at a time, so
//! they carry on when the app next runs, whatever ended this run. Each goes up from a copy of its
//! own, because a picked document has no lasting path on either platform.
//!
//! The queue runs on the Rust runtime, not in a caller's coroutine: a screen going away doesn't stop
//! it. See `docs/specs/mobile-files.feature`.

mod store;
#[cfg(test)]
mod tests;

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use api_client::{ApiError, Client};
use protocol::{CreateUpload, ErrorCode, UploadSession};
use tokio::sync::watch;
use uuid::Uuid;

use self::store::{Row, Store};
use crate::autoupload::AutoUploadListener;
use crate::{CoreClient, CoreError, RUNTIME, off_ui_thread, parse_id};

/// Shown for a file whose copy was still being made when the app ended, on a platform that can't
/// get back to the original.
/// Largest chunk sent, whatever the server allows. Progress moves once per chunk, so this keeps it
/// moving on a slow connection, and it is the most that is sent again after a dropped one.
const MAX_CHUNK_BYTES: u64 = 4 * 1024 * 1024;

const PICK_AGAIN: &str = "The app closed before this file was ready. Pick it again.";
const COPY_GONE: &str = "The copy of this file has gone. Pick it again.";
const FOLDER_GONE: &str = "The folder this file goes to no longer exists.";

/// How the app reaches a picked file that it hasn't copied yet. Called on a background thread, and
/// may block.
#[uniffi::export(with_foreign)]
pub trait PickedFiles: Send + Sync {
    /// Copies the original that `source` stands for to the file `dest`. A failure's message is
    /// shown to the user.
    fn copy_file(&self, source: String, dest: String) -> Result<(), CoreError>;
    /// `source` is no longer needed, so any lasting access to it can go.
    fn release(&self, source: String);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum QueuedUploadState {
    /// Its turn hasn't come.
    Waiting,
    /// The copy to send is being made.
    Preparing,
    Uploading,
    /// The server can't be reached. It goes on the next [`UploadQueue::run`].
    Offline,
    /// Waits for the user to continue or cancel. See [`UploadQueue::pause`].
    Paused,
    /// Stopped for the reason in `problem`, until the user retries or cancels.
    Failed,
}

/// A file in the queue, until it is on the server or the user gives up on it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct QueuedUpload {
    pub key: String,
    pub name: String,
    /// The server folder it goes to; `None` for Home.
    pub parent_id: Option<String>,
    pub folder_name: String,
    pub state: QueuedUploadState,
    /// What the server has, as far as known.
    pub done: u64,
    /// The size of the file; 0 until its copy is made.
    pub total: u64,
    pub problem: Option<String>,
    /// Whether [`UploadQueue::retry`] can help. It also continues a paused file.
    pub can_retry: bool,
}

/// How a run went.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct QueueSummary {
    /// Files that will go on a later run.
    pub remaining: u64,
    /// Files waiting for the user after a problem.
    pub failed: u64,
    /// Files waiting for the user to continue.
    pub paused: u64,
    /// The server can't be reached.
    pub offline: bool,
    /// [`UploadQueue::stop`] ended the run.
    pub stopped: bool,
}

/// The file on its way up, for the rows and for cancelling it.
struct Active {
    key: String,
    copying: bool,
    cancel: Arc<AtomicBool>,
    /// Keeps it stopped if a new run clears [`Inner::stop`] before its next chunk.
    pause: Arc<AtomicBool>,
}

#[derive(Default)]
struct Runner {
    /// Asked to run again while running, because something may have been added too late to see.
    again: bool,
    last: QueueSummary,
    session_ended: bool,
}

struct Inner {
    client: Client,
    store: Store,
    dir: PathBuf,
    files: Arc<dyn PickedFiles>,
    listener: Arc<dyn AutoUploadListener>,
    stop: AtomicBool,
    offline: AtomicBool,
    wiped: AtomicBool,
    active: Mutex<Option<Active>>,
    runner: Mutex<Runner>,
    running: watch::Sender<bool>,
}

/// The picked files of the signed-in user. Keep one per client, and call [`UploadQueue::wipe`] on
/// sign-out.
#[derive(uniffi::Object)]
pub struct UploadQueue {
    inner: Arc<Inner>,
}

#[uniffi::export]
impl UploadQueue {
    /// Opens what is kept for `client`'s signed-in user under `data_dir`, a folder that outlives
    /// the app's runs and isn't wiped with downloaded copies. The copies stay in there too, so it
    /// shouldn't be backed up.
    #[uniffi::constructor]
    pub fn open(
        client: Arc<CoreClient>,
        data_dir: String,
        files: Arc<dyn PickedFiles>,
        listener: Arc<dyn AutoUploadListener>,
    ) -> Result<Arc<Self>, CoreError> {
        let user = client.files.user().ok_or(CoreError::Unauthorized)?;
        let dir = PathBuf::from(data_dir).join(format!("uploads-{user}"));
        std::fs::create_dir_all(&dir).map_err(file_error)?;
        let store = Store::open(&dir.join("queue.db")).map_err(storage_error)?;
        store.lose_unprepared(PICK_AGAIN).map_err(storage_error)?;
        Ok(Arc::new(Self {
            inner: Arc::new(Inner {
                client: client.inner.clone(),
                store,
                dir,
                files,
                listener,
                stop: AtomicBool::new(false),
                offline: AtomicBool::new(false),
                wiped: AtomicBool::new(false),
                active: Mutex::default(),
                runner: Mutex::default(),
                running: watch::Sender::new(false),
            }),
        }))
    }

    /// Adds a file going into `parent_id`, or Home when `None`, as a new version if a file already
    /// has this name. With a `source`, [`PickedFiles::copy_file`] copies it when its turn comes.
    /// Otherwise the app puts the file at [`UploadQueue::copy_path`] and calls
    /// [`UploadQueue::ready`]; if the app ends before that, the user is asked to pick it again.
    /// Returns its key. Nothing goes up until [`UploadQueue::run`]. `mime` is the file's type when
    /// the app knows it better than its name says, such as shared text saved without an extension.
    pub fn add(
        &self,
        parent_id: Option<String>,
        folder_name: String,
        name: String,
        source: Option<String>,
        mime: Option<String>,
    ) -> Result<String, CoreError> {
        if let Some(parent) = &parent_id {
            parse_id(parent)?;
        }
        let key = Uuid::new_v4().to_string();
        self.inner
            .store
            .add(
                &key,
                parent_id.as_deref(),
                &folder_name,
                &name,
                source.as_deref(),
                mime.as_deref().filter(|mime| !mime.is_empty()),
            )
            .map_err(storage_error)?;
        self.inner.listener.changed();
        Ok(key)
    }

    /// Where the app puts the file of `key`.
    pub fn copy_path(&self, key: String) -> String {
        self.inner.copy_of(&key).to_string_lossy().into_owned()
    }

    /// The file of `key` is at [`UploadQueue::copy_path`]. `name` replaces its name, if given.
    pub fn ready(&self, key: String, name: Option<String>) -> Result<(), CoreError> {
        let copy = self.inner.copy_of(&key);
        let size = std::fs::metadata(&copy).map_err(file_error)?.len();
        let kept = self
            .inner
            .store
            .mark_ready(&key, name.as_deref(), size)
            .map_err(storage_error)?;
        if !kept {
            let _ = std::fs::remove_file(copy);
        }
        self.inner.listener.changed();
        Ok(())
    }

    /// The app couldn't make the file of `key`. `problem` is shown to the user.
    pub fn fail(&self, key: String, problem: String) -> Result<(), CoreError> {
        self.inner
            .store
            .set_problem(&key, Some(&problem))
            .map_err(storage_error)?;
        self.inner.listener.changed();
        Ok(())
    }

    /// The queue, in the order it goes up.
    pub fn items(&self) -> Result<Vec<QueuedUpload>, CoreError> {
        let rows = self.inner.store.rows().map_err(storage_error)?;
        let active = self.inner.active.lock().expect("active upload lock");
        let offline = self.inner.offline.load(Ordering::Relaxed);
        Ok(rows
            .into_iter()
            .map(|row| {
                let state = match active.as_ref().filter(|active| active.key == row.key) {
                    Some(active) if active.copying => QueuedUploadState::Preparing,
                    Some(_) => QueuedUploadState::Uploading,
                    None if row.problem.is_some() => QueuedUploadState::Failed,
                    None if !row.ready && row.source.is_none() => QueuedUploadState::Preparing,
                    None if row.paused => QueuedUploadState::Paused,
                    None if offline => QueuedUploadState::Offline,
                    None => QueuedUploadState::Waiting,
                };
                QueuedUpload {
                    can_retry: row.ready || row.source.is_some(),
                    key: row.key,
                    name: row.name,
                    parent_id: row.parent,
                    folder_name: row.folder,
                    state,
                    done: row.done,
                    total: row.size,
                    problem: row.problem,
                }
            })
            .collect())
    }

    /// Sends what can go, one file at a time, and returns once nothing more can. Starts a run
    /// unless one is going, in which case this waits for that one. The run carries on if this call
    /// is abandoned. Throws `Unauthorized` if the session has ended.
    pub async fn run(&self) -> Result<QueueSummary, CoreError> {
        let inner = self.inner.clone();
        off_ui_thread(async move {
            let mut running = inner.running.subscribe();
            Inner::start(&inner);
            let _ = running.wait_for(|running| !running).await;
            let runner = inner.runner.lock().expect("upload runner lock");
            if runner.session_ended {
                return Err(CoreError::Unauthorized);
            }
            Ok(runner.last.clone())
        })
        .await
    }

    /// Like [`UploadQueue::run`] without waiting. A run in progress looks again before it ends, so
    /// it sees whatever was just added.
    pub fn start(&self) {
        Inner::start(&self.inner);
    }

    /// Ends the run at the next chunk. What is left waits for the next [`UploadQueue::run`].
    pub fn stop(&self) {
        self.inner.runner.lock().expect("upload runner lock").again = false;
        self.inner.stop.store(true, Ordering::Relaxed);
    }

    /// Ends the run at the next chunk, and nothing in the queue goes until the user continues it
    /// with [`UploadQueue::retry`]. This lasts when the app next opens, unlike
    /// [`UploadQueue::stop`]. Files added later go as usual.
    pub fn pause(&self) -> Result<(), CoreError> {
        let paused = self.inner.store.pause_all().map_err(storage_error);
        if let Some(active) = self
            .inner
            .active
            .lock()
            .expect("active upload lock")
            .as_ref()
        {
            active.pause.store(true, Ordering::Relaxed);
        }
        self.stop();
        self.inner.listener.changed();
        paused
    }

    /// Lets a file that was paused or stopped on a problem go again on the next
    /// [`UploadQueue::run`].
    pub fn retry(&self, key: String) -> Result<(), CoreError> {
        self.inner.store.resume(&key).map_err(storage_error)?;
        self.inner.listener.changed();
        Ok(())
    }

    /// Gives up on a file: the row goes now, a running upload stops at its next chunk, and the
    /// server lets go of what it received.
    pub fn cancel(&self, key: String) -> Result<(), CoreError> {
        let removed = self.inner.store.remove(&key).map_err(storage_error)?;
        self.inner.listener.changed();
        let Some(row) = removed else { return Ok(()) };
        if let Some(source) = &row.source {
            self.inner.files.release(source.clone());
        }
        let active = self.inner.active.lock().expect("active upload lock");
        if let Some(active) = active.as_ref().filter(|active| active.key == key) {
            // The run removes the copy and tells the server, since it knows the latest session.
            active.cancel.store(true, Ordering::Relaxed);
            return Ok(());
        }
        drop(active);
        let _ = std::fs::remove_file(self.inner.copy_of(&key));
        if let Some(session) = row.session {
            let client = self.inner.client.clone();
            RUNTIME.spawn(async move {
                let _ = client.cancel_upload(session).await;
            });
        }
        Ok(())
    }

    /// Stops everything, ends the uploads on the server and deletes the queue and its copies. Call
    /// it before the session goes.
    pub async fn wipe(&self) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        off_ui_thread(async move {
            inner.wiped.store(true, Ordering::Relaxed);
            inner.stop.store(true, Ordering::Relaxed);
            let mut running = inner.running.subscribe();
            let _ = running.wait_for(|running| !running).await;
            for row in inner.store.rows().map_err(storage_error)? {
                if let Some(session) = row.session {
                    let _ = inner.client.cancel_upload(session).await;
                }
                if let Some(source) = row.source {
                    inner.files.release(source);
                }
                inner.store.remove(&row.key).map_err(storage_error)?;
            }
            let _ = tokio::fs::remove_dir_all(&inner.dir).await;
            Ok(())
        })
        .await
    }
}

/// How one file went.
enum Sent {
    /// It is on the server, or the user gave up on it. Next.
    Done,
    /// It waits for the user. Next.
    Problem(String),
    Stopped,
    Offline,
    SessionEnded,
}

impl Inner {
    fn copy_of(&self, key: &str) -> PathBuf {
        self.dir.join(key)
    }

    fn start(inner: &Arc<Self>) {
        let mut runner = inner.runner.lock().expect("upload runner lock");
        if inner.wiped.load(Ordering::Relaxed) {
            return;
        }
        inner.stop.store(false, Ordering::Relaxed);
        if *inner.running.borrow() {
            runner.again = true;
            return;
        }
        inner.running.send_replace(true);
        drop(runner);
        let inner = inner.clone();
        RUNTIME.spawn(async move { inner.drain().await });
    }

    async fn drain(self: Arc<Self>) {
        loop {
            let (mut offline, mut stopped, mut ended) = (false, false, false);
            while let Ok(Some(row)) = self.store.next() {
                if self.stop.load(Ordering::Relaxed) {
                    stopped = true;
                    break;
                }
                let key = row.key.clone();
                let cancel = Arc::new(AtomicBool::new(false));
                let pause = Arc::new(AtomicBool::new(false));
                *self.active.lock().expect("active upload lock") = Some(Active {
                    key: key.clone(),
                    copying: !row.ready,
                    cancel: cancel.clone(),
                    pause: pause.clone(),
                });
                self.listener.changed();
                let sent = self.send(row, &cancel, &pause).await;
                *self.active.lock().expect("active upload lock") = None;
                match sent {
                    Sent::Done => {}
                    Sent::Problem(problem) => {
                        let _ = self.store.set_problem(&key, Some(&problem));
                    }
                    Sent::Stopped => stopped = true,
                    Sent::Offline => offline = true,
                    Sent::SessionEnded => ended = true,
                }
                self.listener.changed();
                if stopped || offline || ended {
                    break;
                }
            }
            self.offline.store(offline, Ordering::Relaxed);

            let rows = self.store.rows().unwrap_or_default();
            let failed = rows.iter().filter(|row| row.problem.is_some()).count() as u64;
            let paused = rows
                .iter()
                .filter(|row| row.paused && row.problem.is_none())
                .count() as u64;
            let mut runner = self.runner.lock().expect("upload runner lock");
            if std::mem::take(&mut runner.again) && !self.wiped.load(Ordering::Relaxed) {
                continue;
            }
            runner.last = QueueSummary {
                remaining: rows.len() as u64 - failed - paused,
                failed,
                paused,
                offline,
                stopped,
            };
            runner.session_ended = ended;
            self.running.send_replace(false);
            drop(runner);
            self.listener.changed();
            return;
        }
    }

    async fn send(&self, mut row: Row, cancel: &AtomicBool, pause: &AtomicBool) -> Sent {
        let copy = self.copy_of(&row.key);
        if !row.ready {
            let Some(source) = row.source.clone() else {
                return Sent::Done;
            };
            let size = match self.make_copy(&source, &copy).await {
                Ok(size) => size,
                Err(err) => return Sent::Problem(message_of(&err)),
            };
            match self.store.mark_ready(&row.key, None, size) {
                Ok(true) => {}
                // Cancelled meanwhile.
                Ok(false) | Err(_) => {
                    let _ = tokio::fs::remove_file(&copy).await;
                    return Sent::Done;
                }
            }
            self.files.release(source);
            row.ready = true;
            row.size = size;
            if let Some(active) = self.active.lock().expect("active upload lock").as_mut() {
                active.copying = false;
            }
            self.listener.changed();
        }
        if !copy.exists() {
            let _ = self.store.lost(&row.key, COPY_GONE);
            return Sent::Done;
        }

        let mut session = match self.session(&row).await {
            Ok(session) => session,
            Err(err) => return self.failed(&err),
        };
        session.chunk_size = session.chunk_size.min(MAX_CHUNK_BYTES);
        match self.store.set_session(&row.key, session.id, session.offset) {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                let _ = self.client.cancel_upload(session.id).await;
                let _ = tokio::fs::remove_file(&copy).await;
                return Sent::Done;
            }
        }
        self.listener.changed();

        let read_failed = AtomicBool::new(false);
        let stored = self
            .client
            .continue_upload(
                session,
                row.size,
                |offset, len| {
                    let halted = cancel.load(Ordering::Relaxed)
                        || pause.load(Ordering::Relaxed)
                        || self.stop.load(Ordering::Relaxed);
                    let (copy, read_failed) = (&copy, &read_failed);
                    async move {
                        if halted {
                            return Err(io::Error::other("the upload was stopped"));
                        }
                        let chunk = sync_core::read_range(copy, offset, len).await;
                        if chunk.is_err() {
                            read_failed.store(true, Ordering::Relaxed);
                        }
                        chunk
                    }
                },
                |done| {
                    self.offline.store(false, Ordering::Relaxed);
                    let _ = self.store.set_done(&row.key, done);
                    self.listener.changed();
                },
            )
            .await;

        if cancel.load(Ordering::Relaxed) {
            if stored.is_err() {
                let _ = self.client.cancel_upload(session.id).await;
            }
            let _ = tokio::fs::remove_file(&copy).await;
            return Sent::Done;
        }
        match stored {
            Ok(_) => {
                let _ = self.store.remove(&row.key);
                let _ = tokio::fs::remove_file(&copy).await;
                Sent::Done
            }
            Err(_) if read_failed.load(Ordering::Relaxed) => {
                let _ = self.store.lost(&row.key, COPY_GONE);
                let _ = self.client.cancel_upload(session.id).await;
                Sent::Done
            }
            Err(_) if pause.load(Ordering::Relaxed) || self.stop.load(Ordering::Relaxed) => {
                Sent::Stopped
            }
            Err(err) => self.failed(&err),
        }
    }

    fn failed(&self, err: &ApiError) -> Sent {
        if *err == ApiError::Unauthorized {
            Sent::SessionEnded
        } else if err.is_unreachable() {
            Sent::Offline
        } else if matches!(
            err,
            ApiError::Api {
                code: ErrorCode::NotFound,
                ..
            }
        ) {
            Sent::Problem(FOLDER_GONE.to_owned())
        } else {
            Sent::Problem(message_of(&CoreError::from(err.clone())))
        }
    }

    /// Picks up where the last try stopped, or starts anew if the server let it go.
    async fn session(&self, row: &Row) -> Result<UploadSession, ApiError> {
        if let Some(saved) = row.session {
            match self.client.upload_status(saved).await {
                Ok(status) => return Ok(status),
                Err(err) if err.is_transient() || err == ApiError::Unauthorized => return Err(err),
                Err(_) => {}
            }
        }
        let parent = row.parent.as_deref().and_then(|id| parse_id(id).ok());
        self.client
            .create_upload(&CreateUpload {
                parent_id: parent,
                name: row.name.clone(),
                size: row.size,
                modified_at: None,
                mime: row.mime.clone(),
                node_id: None,
                keep_both: false,
                version_of: None,
                auto_upload: false,
                transfer_item_id: None,
            })
            .await
    }

    /// Copies by way of a temporary name, so that a file with the final name is always whole.
    async fn make_copy(&self, source: &str, copy: &std::path::Path) -> Result<u64, CoreError> {
        let part = copy.with_extension("part");
        let files = self.files.clone();
        let (source, dest) = (source.to_owned(), part.to_string_lossy().into_owned());
        let copied = tokio::task::spawn_blocking(move || files.copy_file(source, dest))
            .await
            .unwrap_or_else(|err| std::panic::resume_unwind(err.into_panic()));
        let finished = match copied {
            Ok(()) => tokio::fs::rename(&part, copy).await.map_err(file_error),
            Err(err) => Err(err),
        };
        if let Err(err) = finished {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(err);
        }
        Ok(tokio::fs::metadata(copy).await.map_err(file_error)?.len())
    }
}

fn storage_error(err: rusqlite::Error) -> CoreError {
    CoreError::Failed {
        code: "storage".to_owned(),
        detail: err.to_string(),
    }
}

fn file_error(err: io::Error) -> CoreError {
    CoreError::Failed {
        code: "file".to_owned(),
        detail: format!("the file can't be read: {err}"),
    }
}

/// The sentence to show for a failure.
fn message_of(err: &CoreError) -> String {
    let message = match err {
        CoreError::Unreachable { .. } => "could not reach the server".to_owned(),
        CoreError::Failed { detail, .. } => detail.clone(),
        other => other.to_string(),
    };
    let mut letters = message.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => message,
    }
}
