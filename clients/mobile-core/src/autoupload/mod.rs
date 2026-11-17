//! Auto-upload: local folders the user marks go up to a folder on the server. New and changed files
//! are sent. Nothing on the server ever changes the local folder.
//!
//! The apps own what Rust can't reach: a folder on Android is a `content://` tree and a photo on
//! iOS is a PhotoKit asset, neither of them with a path. So they list and read files through
//! [`LocalSources`], decide when a pass runs and whether the network allows it, and pass the
//! outcome to the user. See `docs/specs/mobile-sync.feature`.

mod plan;
mod run;
mod store;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use api_client::Client;
use uuid::Uuid;

use self::run::{Backoff, Pass, storage_error};
use self::store::{SourceRow, Store};
use crate::{CoreClient, CoreError, off_ui_thread, parse_id};

const KIND_FOLDER: &str = "folder";
const KIND_PHOTOS: &str = "photos";

/// What a source is, which tells the app how to read its `handle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SourceKind {
    /// A folder the user picked: a persisted tree URI on Android, a bookmark on iOS.
    Folder,
    /// The camera photos: MediaStore on Android, the photo library on iOS.
    Photos,
}

/// A file of a source, as the app sees it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct LocalItem {
    /// Identifies the file within its source, stable while the file stays where it is.
    pub key: String,
    /// Its folder below the source, `/` separated; empty for the source itself.
    pub relative_dir: String,
    pub name: String,
    pub size: u64,
    pub modified_at: SystemTime,
    pub mime: Option<String>,
}

/// What changed in a source since a token, from [`LocalSources::changes_since`].
#[derive(Debug, Clone, uniffi::Record)]
pub struct LocalChanges {
    /// Files added or changed since the token, leaving out any still being written. Files that went
    /// away need no mention, as nothing on the server follows them.
    pub items: Vec<LocalItem>,
    /// For next time. Taken before looking, so that a change made meanwhile shows up again rather
    /// than not at all.
    pub token: String,
}

/// How Rust gets at the files of a source. Called on a background thread and may block.
#[uniffi::export(with_foreign)]
pub trait LocalSources: Send + Sync {
    /// Every file of the source, leaving out any still being written. Throws if the source can't
    /// be read at all, such as when the user took back the permission.
    fn list(&self, kind: SourceKind, handle: String) -> Result<Vec<LocalItem>, CoreError>;
    /// A token for how the source stands now, to ask later what changed. `None` if changes can't
    /// be told for this source, which is then listed on every pass. Rust asks before it lists.
    fn change_token(&self, kind: SourceKind, handle: String) -> Result<Option<String>, CoreError>;
    /// What changed since `token`. `None` if that can't be told, such as when the token has expired
    /// or access has narrowed, and Rust then lists the source. Throws like [`LocalSources::list`].
    fn changes_since(
        &self,
        kind: SourceKind,
        handle: String,
        token: String,
    ) -> Result<Option<LocalChanges>, CoreError>;
    /// The file as it is now, `None` if it is gone.
    fn stat(
        &self,
        kind: SourceKind,
        handle: String,
        key: String,
    ) -> Result<Option<LocalItem>, CoreError>;
    /// `len` bytes from `offset`.
    fn read(
        &self,
        kind: SourceKind,
        handle: String,
        key: String,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>, CoreError>;
}

/// Told when what [`AutoUpload::sources`] returns may have changed. Called on a background thread,
/// so hand it to the UI thread. It must not throw.
#[uniffi::export(with_foreign)]
pub trait AutoUploadListener: Send + Sync {
    fn changed(&self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct SyncSettings {
    /// Transfers start only on Wi-Fi. The app enforces it, by what it passes to a pass.
    pub wifi_only: bool,
    /// Sync while the app isn't open. The app schedules that.
    pub background: bool,
}

/// A source with where it stands.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct UploadSource {
    pub id: String,
    pub kind: SourceKind,
    pub handle: String,
    pub label: String,
    /// The server folder; `None` for Home.
    pub target_id: Option<String>,
    pub target_label: String,
    /// Why it isn't syncing, for the user.
    pub problem: Option<String>,
    /// When everything in it was last on the server.
    pub last_synced_at: Option<SystemTime>,
    /// Files still to go, as of the last pass.
    pub pending: u64,
    /// Why the last file that failed did, for the user, until a pass of this source goes well.
    pub last_error: Option<String>,
}

/// The file a pass is sending now.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CurrentUpload {
    pub source_id: String,
    pub name: String,
    /// What the server has.
    pub done: u64,
    pub total: u64,
}

/// How a pass went.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct PassSummary {
    pub uploaded: u64,
    /// Files left for a later pass.
    pub pending: u64,
    /// Files that failed in this pass; each is tried again later.
    pub failed: u64,
    /// Why the last of those failed, for the user.
    pub last_error: Option<String>,
    /// The server can't be reached. Passes wait for it, unless started with `retry_now`.
    pub outage: bool,
    /// The pass was told to stop.
    pub stopped: bool,
    /// Another pass was already running, so this one did nothing.
    pub busy: bool,
}

/// Auto-upload for the signed-in user. Keep one per client, and call [`AutoUpload::wipe`] on sign-out.
#[derive(uniffi::Object)]
pub struct AutoUpload {
    client: Client,
    store: Arc<Store>,
    local: Arc<dyn LocalSources>,
    listener: Arc<dyn AutoUploadListener>,
    stop: Arc<AtomicBool>,
    /// Held by the running pass.
    running: Arc<tokio::sync::Mutex<()>>,
    backoff: Arc<Mutex<Backoff>>,
    pending: Arc<Mutex<HashMap<String, u64>>>,
    errors: Arc<Mutex<HashMap<String, String>>>,
    current: Arc<Mutex<Option<CurrentUpload>>>,
}

#[uniffi::export]
impl AutoUpload {
    /// Opens what is kept for `client`'s signed-in user under `data_dir`, a folder that outlives
    /// the app's runs and isn't wiped with downloaded copies.
    #[uniffi::constructor]
    pub fn open(
        client: Arc<CoreClient>,
        data_dir: String,
        local: Arc<dyn LocalSources>,
        listener: Arc<dyn AutoUploadListener>,
    ) -> Result<Arc<Self>, CoreError> {
        let user = client.files.user().ok_or(CoreError::Unauthorized)?;
        let path = PathBuf::from(data_dir).join(format!("autoupload-{user}.db"));
        let store = Store::open(&path).map_err(storage_error)?;
        Ok(Arc::new(Self {
            client: client.inner.clone(),
            store: Arc::new(store),
            local,
            listener,
            stop: Arc::default(),
            running: Arc::default(),
            backoff: Arc::default(),
            pending: Arc::default(),
            errors: Arc::default(),
            current: Arc::default(),
        }))
    }

    /// The file being sent now, if any. The listener hears when this changes.
    pub fn current(&self) -> Option<CurrentUpload> {
        self.current.lock().expect("current upload lock").clone()
    }

    pub fn settings(&self) -> Result<SyncSettings, CoreError> {
        Ok(SyncSettings {
            wifi_only: self.store.wifi_only().map_err(storage_error)?,
            background: self.store.background().map_err(storage_error)?,
        })
    }

    pub fn set_settings(&self, settings: SyncSettings) -> Result<(), CoreError> {
        self.store
            .set_settings(settings.wifi_only, settings.background)
            .map_err(storage_error)
    }

    pub fn sources(&self) -> Result<Vec<UploadSource>, CoreError> {
        let pending = self.pending.lock().expect("pending lock");
        let errors = self.errors.lock().expect("errors lock");
        let sources = self.store.sources().map_err(storage_error)?;
        Ok(sources
            .into_iter()
            .map(|row| {
                let pending = pending.get(&row.id).copied().unwrap_or_default();
                let error = errors.get(&row.id).cloned();
                to_source(row, pending, error)
            })
            .collect())
    }

    /// Adds a source that goes up to `target_id`, or Home. With `upload_existing` off, only files
    /// created or changed from now on are sent.
    pub fn add_source(
        &self,
        kind: SourceKind,
        handle: String,
        label: String,
        target_id: Option<String>,
        target_label: String,
        upload_existing: bool,
    ) -> Result<UploadSource, CoreError> {
        if let Some(target) = &target_id {
            parse_id(target)?;
        }
        let row = SourceRow {
            id: Uuid::new_v4().to_string(),
            kind: match kind {
                SourceKind::Folder => KIND_FOLDER,
                SourceKind::Photos => KIND_PHOTOS,
            }
            .to_owned(),
            handle,
            label,
            target: target_id,
            target_label,
            leave_out_existing: !upload_existing,
            problem: None,
            last_synced_at: None,
        };
        self.store.add_source(&row).map_err(storage_error)?;
        Ok(to_source(row, 0, None))
    }

    /// Stops uploading a source. What was uploaded stays on the server, and the local files are
    /// not touched.
    pub async fn remove_source(&self, id: String) -> Result<(), CoreError> {
        let client = self.client.clone();
        let store = self.store.clone();
        let running = self.running.clone();
        let stop = self.stop.clone();
        let pending = self.pending.clone();
        off_ui_thread(async move {
            stop.store(true, Ordering::Relaxed);
            let _pass = running.lock().await;
            stop.store(false, Ordering::Relaxed);
            let saved = store.saved_uploads(&id).map_err(storage_error)?;
            for upload in saved {
                let _ = client.cancel_upload(upload.id).await;
            }
            pending.lock().expect("pending lock").remove(&id);
            store.remove_source(&id).map_err(storage_error)
        })
        .await
    }

    /// Sends a source to another server folder from now on, and clears its problem.
    pub fn set_source_target(
        &self,
        id: String,
        target_id: Option<String>,
        target_label: String,
    ) -> Result<(), CoreError> {
        if let Some(target) = &target_id {
            parse_id(target)?;
        }
        self.store
            .set_target(&id, target_id.as_deref(), &target_label)
            .map_err(storage_error)
    }

    /// Changes what a source covers, such as which albums a photos source uploads, and its label.
    /// Files already sent don't go again; files it now covers go on the next pass.
    pub fn set_source_handle(
        &self,
        id: String,
        handle: String,
        label: String,
    ) -> Result<(), CoreError> {
        self.store
            .set_handle(&id, &handle, &label)
            .map_err(storage_error)
    }

    /// Runs one pass, unless one is running. The app calls this only when the network and the
    /// settings allow it, and [`AutoUpload::stop`] once they no longer do. With `retry_now`,
    /// files and an outage waiting to be tried again go at once, for when the user asked.
    pub async fn run_pass(&self, retry_now: bool) -> Result<PassSummary, CoreError> {
        let client = self.client.clone();
        let store = self.store.clone();
        let local = self.local.clone();
        let listener = self.listener.clone();
        let stop = self.stop.clone();
        let running = self.running.clone();
        let backoff = self.backoff.clone();
        let pending = self.pending.clone();
        let errors = self.errors.clone();
        let current = self.current.clone();
        off_ui_thread(async move {
            let Ok(_pass) = running.try_lock() else {
                return Ok(PassSummary {
                    busy: true,
                    ..PassSummary::default()
                });
            };
            stop.store(false, Ordering::Relaxed);
            if retry_now {
                backoff.lock().expect("backoff lock").clear();
            }
            let report = |source: &str, left: u64| {
                pending
                    .lock()
                    .expect("pending lock")
                    .insert(source.to_owned(), left);
                listener.changed();
            };
            let pass = Pass {
                client: &client,
                store: &store,
                local,
                stop,
                backoff: &backoff,
                errors: &errors,
                report: &report,
                current: &current,
                changed: &|| listener.changed(),
            };
            let summary = pass.run().await;
            listener.changed();
            summary
        })
        .await
    }

    /// Makes a running pass stop before its next chunk. What was sent stays on the server, for the
    /// next pass to continue from.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Forgets everything kept for this user, for signing out. Call it before signing out, because
    /// it cancels the uploads under way on the server. The local files are not touched.
    pub async fn wipe(&self) -> Result<(), CoreError> {
        let client = self.client.clone();
        let store = self.store.clone();
        let running = self.running.clone();
        let stop = self.stop.clone();
        off_ui_thread(async move {
            stop.store(true, Ordering::Relaxed);
            let _pass = running.lock().await;
            for source in store.sources().map_err(storage_error)? {
                for upload in store.saved_uploads(&source.id).map_err(storage_error)? {
                    let _ = client.cancel_upload(upload.id).await;
                }
                store.remove_source(&source.id).map_err(storage_error)?;
            }
            store.set_settings(true, false).map_err(storage_error)?;
            Ok(())
        })
        .await
    }
}

fn to_source(row: SourceRow, pending: u64, last_error: Option<String>) -> UploadSource {
    UploadSource {
        kind: if row.kind == KIND_PHOTOS {
            SourceKind::Photos
        } else {
            SourceKind::Folder
        },
        id: row.id,
        handle: row.handle,
        label: row.label,
        target_id: row.target,
        target_label: row.target_label,
        problem: row.problem,
        last_synced_at: row.last_synced_at.map(|nanos| {
            SystemTime::UNIX_EPOCH + Duration::from_nanos(u64::try_from(nanos).unwrap_or_default())
        }),
        pending,
        last_error,
    }
}
