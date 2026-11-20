//! Opening and saving files, with progress. A file that is opened stays on the phone as an offline
//! copy, which the sync then looks after: see [`crate::offline`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use protocol::{Node, NodeId, NodeKind};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;

use crate::autoupload::AutoUploadListener;
use crate::cancel::{next_chunk, not_cancelled};
use crate::offline::Offline;
use crate::{CoreClient, CoreError, Entry, off_ui_thread, parse_id};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// How often a download starts over because the file was replaced while it ran.
const ATTEMPTS: usize = 2;

/// Told how far a download has come.
///
/// Called on a background thread, so hand the numbers to the UI thread. It must not throw.
#[uniffi::export(with_foreign)]
pub trait DownloadListener: Send + Sync {
    /// `done` of `total` bytes of the file `id` have arrived.
    fn progress(&self, id: String, done: i64, total: i64);
}

/// The downloads in progress, where files go, and the sync that keeps them.
#[derive(Default)]
pub(crate) struct LocalFiles {
    /// `None` for a client that never signs in, which keeps no files.
    root: Option<PathBuf>,
    user: Mutex<Option<String>>,
    /// Where edits go whose file is gone from the server.
    rescue: Mutex<Option<NodeId>>,
    active: Mutex<HashMap<NodeId, watch::Sender<bool>>>,
    offline: tokio::sync::Mutex<Option<Arc<Offline>>>,
    listener: Arc<Mutex<Option<Arc<dyn AutoUploadListener>>>>,
}

impl LocalFiles {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root: Some(root),
            ..Self::default()
        }
    }

    pub(crate) fn set_user(&self, user: String, rescue: NodeId) {
        *self.user.lock().expect("files user lock") = Some(user);
        *self.rescue.lock().expect("files rescue lock") = Some(rescue);
    }

    pub(crate) fn user(&self) -> Option<String> {
        self.user.lock().expect("files user lock").clone()
    }

    pub(crate) fn set_offline_listener(&self, listener: Arc<dyn AutoUploadListener>) {
        *self.listener.lock().expect("offline listener lock") = Some(listener);
    }

    /// The sync of the signed-in user, started if it wasn't.
    pub(crate) async fn offline(
        &self,
        client: &api_client::Client,
    ) -> Result<Arc<Offline>, CoreError> {
        let mut offline = self.offline.lock().await;
        if let Some(running) = offline.as_ref() {
            return Ok(running.clone());
        }
        let dir = self
            .root
            .as_ref()
            .zip(self.user())
            .map(|(root, user)| root.join(user))
            .ok_or_else(|| CoreError::Failed {
                code: "no_files_dir".to_owned(),
                detail: "this client keeps no files".to_owned(),
            })?;
        let rescue = *self.rescue.lock().expect("files rescue lock");
        let started = Offline::start(client.clone(), dir, rescue, self.listener.clone()).await?;
        *offline = Some(started.clone());
        Ok(started)
    }

    /// Fills in the local state of `entries`.
    pub(crate) async fn mark_copies(&self, client: &api_client::Client, entries: &mut [Entry]) {
        if let Ok(offline) = self.offline(client).await {
            offline.mark(entries, true);
        }
    }

    /// Stops keeping `id` for its own sake, if it was, along with its copy.
    pub(crate) async fn remove(&self, client: &api_client::Client, id: NodeId) {
        if let Ok(offline) = self.offline(client).await {
            offline.remove(id).await;
        }
    }

    pub(crate) fn cancel_all(&self) {
        for cancel in self.active.lock().expect("downloads lock").values() {
            cancel.send_replace(true);
        }
    }

    /// Ends the sync and removes every user's copies. Returns the uploads under way on the server,
    /// to cancel while the session lasts.
    pub(crate) async fn wipe(&self) -> Vec<uuid::Uuid> {
        let running = self.offline.lock().await.take();
        let uploads = match running {
            Some(running) => running.shut_down().await,
            None => Vec::new(),
        };
        if let Some(root) = &self.root {
            let _ = fs::remove_dir_all(root).await;
        }
        uploads
    }
}

/// What opening a file does with the copy that is there.
pub(crate) enum MarkedFile {
    /// There is none, or it is an unedited older version: download.
    Fetch,
    /// It matches the server.
    Current,
    /// It holds what the server does, though nothing recorded that yet.
    SameContent(String),
    /// It differs and may hold edits, so it opens as it is and the sync sorts it out.
    Edited,
}

/// One running download of a file; a second is refused until this is dropped.
struct Active {
    files: Arc<LocalFiles>,
    id: NodeId,
    cancelled: watch::Receiver<bool>,
}

impl Active {
    fn start(files: &Arc<LocalFiles>, id: NodeId) -> Result<Self, CoreError> {
        let mut active = files.active.lock().expect("downloads lock");
        if active.contains_key(&id) {
            return Err(CoreError::Failed {
                code: "already_downloading".to_owned(),
                detail: "this file is already downloading".to_owned(),
            });
        }
        let (cancel, cancelled) = watch::channel(false);
        active.insert(id, cancel);
        Ok(Self {
            files: files.clone(),
            id,
            cancelled,
        })
    }

    fn check(&self) -> Result<(), CoreError> {
        not_cancelled(&self.cancelled)
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        self.files
            .active
            .lock()
            .expect("downloads lock")
            .remove(&self.id);
    }
}

#[uniffi::export]
impl CoreClient {
    /// A local copy of the file to show in a viewer: the one already here if it is current, else
    /// downloaded first. While the server can't be reached, any copy will do.
    pub async fn open_copy(
        &self,
        id: String,
        listener: Arc<dyn DownloadListener>,
    ) -> Result<String, CoreError> {
        let id = parse_id(&id)?;
        let client = self.inner.clone();
        let files = self.files.clone();
        off_ui_thread(async move {
            let active = Active::start(&files, id)?;
            let offline = files.offline(&client).await?;
            let mut node = match client.get(id).await {
                Ok(node) => node,
                Err(err) if err.is_unreachable() => {
                    return match offline.existing_copy(id) {
                        Some(copy) => Ok(path_text(&copy)),
                        None => Err(err.into()),
                    };
                }
                Err(err) => return Err(err.into()),
            };
            require_file(&node)?;
            let target = offline.place_for(&node)?;
            // The engine leaves such a name alone, so it stays a plain copy of what the server has.
            let synced = !sync_core::is_ignored(&sync_core::local_name(&node.name));
            match crate::offline::decide(&offline, &node, &target) {
                MarkedFile::Current => {}
                MarkedFile::SameContent(sha256) => {
                    crate::offline::record_copy(&offline, id, &target, sha256)?;
                }
                MarkedFile::Edited if synced => {}
                MarkedFile::Edited | MarkedFile::Fetch => {
                    let before = fs::metadata(&target).await.ok();
                    let part = offline.part_path(id)?;
                    let (fetched, sha256) =
                        fetch(&client, node.clone(), &part, &active, listener.as_ref()).await?;
                    node = fetched;
                    set_modified(&part, &node).map_err(storage)?;
                    // Edits made during the download stay; the sync sorts them out.
                    let after = fs::metadata(&target).await.ok();
                    if same_file(before.as_ref(), after.as_ref()) {
                        if let Some(parent) = target.parent() {
                            fs::create_dir_all(parent).await.map_err(storage)?;
                        }
                        fs::rename(&part, &target).await.map_err(storage)?;
                        if synced {
                            crate::offline::record_copy(&offline, id, &target, sha256)?;
                        }
                    } else {
                        let _ = fs::remove_file(&part).await;
                    }
                }
            }
            if synced {
                offline.keep(&node, &target);
            }
            Ok(path_text(&target))
        })
        .await
    }

    /// Saves the file at `dest_path`, which ends up complete or not at all. It doesn't count as a
    /// copy: this file gets no badge from it.
    pub async fn save_copy(
        &self,
        id: String,
        dest_path: String,
        listener: Arc<dyn DownloadListener>,
    ) -> Result<(), CoreError> {
        let id = parse_id(&id)?;
        let client = self.inner.clone();
        let files = self.files.clone();
        off_ui_thread(async move {
            let active = Active::start(&files, id)?;
            let node = client.get(id).await?;
            require_file(&node)?;
            let dest = PathBuf::from(dest_path);
            let parent = dest.parent().unwrap_or(Path::new("")).to_path_buf();
            fs::create_dir_all(&parent).await.map_err(storage)?;
            if let Ok(offline) = files.offline(&client).await
                && let Ok(Some(base)) = offline.store.base(id)
                && base.sha256 == node.sha256
                && !base.is_edited()
                && base.path.is_file()
            {
                fs::copy(&base.path, &dest).await.map_err(storage)?;
                return Ok(());
            }
            let part = parent.join(format!("{id}.part"));
            fetch(&client, node, &part, &active, listener.as_ref()).await?;
            fs::rename(&part, &dest).await.map_err(storage)
        })
        .await
    }

    /// Stops a running download of `id`, whose call then fails as `Cancelled`.
    pub fn cancel_download(&self, id: String) -> Result<(), CoreError> {
        let id = parse_id(&id)?;
        if let Some(cancel) = self.files.active.lock().expect("downloads lock").get(&id) {
            cancel.send_replace(true);
        }
        Ok(())
    }
}

/// Downloads to `part`, starting over once if the file was replaced meanwhile, and returns the
/// node the content belongs to with the hash of what arrived. No partial file is left behind on failure.
async fn fetch(
    client: &api_client::Client,
    mut node: Node,
    part: &Path,
    active: &Active,
    listener: &dyn DownloadListener,
) -> Result<(Node, String), CoreError> {
    for _ in 0..ATTEMPTS {
        let written = stream(client, &node, part, active, listener).await;
        let sha256 = match written {
            Ok(sha256) => sha256,
            Err(err) => {
                let _ = fs::remove_file(part).await;
                return Err(err);
            }
        };
        if node.sha256.as_ref().is_none_or(|expect| *expect == sha256) {
            return Ok((node, sha256));
        }
        node = match client.get(node.id).await {
            Ok(node) => node,
            Err(err) => {
                let _ = fs::remove_file(part).await;
                return Err(err.into());
            }
        };
    }
    let _ = fs::remove_file(part).await;
    Err(CoreError::Failed {
        code: "content_changed".to_owned(),
        detail: "the file changed while downloading".to_owned(),
    })
}

/// Writes the content of `node` to `part`, returning the hash of what arrived.
///
/// Cancelling stops only the transfer of the body. Requests before it run to their end, since one
/// of them may be refreshing the session and a rotated token must not be lost.
async fn stream(
    client: &api_client::Client,
    node: &Node,
    part: &Path,
    active: &Active,
    listener: &dyn DownloadListener,
) -> Result<String, CoreError> {
    active.check()?;
    let mut response = client.download(node.id).await?;
    active.check()?;
    let mut out = fs::File::create(part).await.map_err(storage)?;
    let mut hasher = Sha256::new();
    let mut cancelled = active.cancelled.clone();
    let id = node.id.to_string();
    let mut done: i64 = 0;
    let mut reported: Option<Instant> = None;
    loop {
        let Some(chunk) = next_chunk(&mut response, &mut cancelled).await? else {
            break;
        };
        hasher.update(&chunk);
        out.write_all(&chunk).await.map_err(storage)?;
        done = done.saturating_add(i64::try_from(chunk.len()).unwrap_or(i64::MAX));
        if reported.is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL) {
            listener.progress(id.clone(), done, node.size);
            reported = Some(Instant::now());
        }
    }
    out.flush().await.map_err(storage)?;
    active.check()?;
    listener.progress(id, done, node.size);
    Ok(hex::encode(hasher.finalize()))
}

fn require_file(node: &Node) -> Result<(), CoreError> {
    if node.kind == NodeKind::File {
        Ok(())
    } else {
        Err(CoreError::Failed {
            code: "not_a_file".to_owned(),
            detail: format!("“{}” is a folder", node.name),
        })
    }
}

pub(crate) fn storage(err: std::io::Error) -> CoreError {
    CoreError::Failed {
        code: "local_storage".to_owned(),
        detail: err.to_string(),
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The copy gets the time of the content on the server, as with a file the sync downloads.
fn set_modified(part: &Path, node: &Node) -> std::io::Result<()> {
    std::fs::File::options()
        .write(true)
        .open(part)?
        .set_modified(SystemTime::from(node.updated_at))
}

fn same_file(before: Option<&std::fs::Metadata>, after: Option<&std::fs::Metadata>) -> bool {
    match (before, after) {
        (None, None) => true,
        (Some(before), Some(after)) => {
            before.len() == after.len() && before.modified().ok() == after.modified().ok()
        }
        _ => false,
    }
}
