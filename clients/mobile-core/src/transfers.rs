//! Sending files to a device, and what was sent to the user's devices. See
//! `docs/send-to-device.md`.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use api_client::Client;
use client_core::format::sanitize_component;
use client_core::{send, transfers as words};
use protocol::{
    CreateTransfer, CreateUpload, NewTransferItem, NodeKind, TransferState, TransferSummary,
    TransferTarget, UserId,
};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use uuid::Uuid;

use crate::files::{DownloadListener, storage};
use crate::{CoreClient, CoreError, Entry, EntryKind, off_ui_thread, parse_id};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Largest chunk sent, so that progress keeps moving on a slow connection.
const MAX_CHUNK_BYTES: u64 = 4 * 1024 * 1024;
/// How often one chunk is retried before a send gives up.
const CHUNK_RETRIES: u32 = 5;

/// Where a transfer stands; see `protocol::TransferState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TransferStatus {
    /// The sender is still uploading its files.
    Uploading,
    /// Waiting for the recipient to save, download or decline it.
    Ready,
    Declined,
    Downloaded,
    Saved,
    Cancelled,
    Expired,
}

impl From<TransferState> for TransferStatus {
    fn from(state: TransferState) -> Self {
        match state {
            TransferState::Uploading => Self::Uploading,
            TransferState::Ready => Self::Ready,
            TransferState::Declined => Self::Declined,
            TransferState::Downloaded => Self::Downloaded,
            TransferState::Saved => Self::Saved,
            TransferState::Cancelled => Self::Cancelled,
            TransferState::Expired => Self::Expired,
        }
    }
}

/// A transfer in a list, with the words the desktop and web clients show for it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TransferRow {
    pub id: String,
    /// What it holds: `report.pdf`, `Photos — 124 files` or `3 files`.
    pub title: String,
    /// E.g. `12.0 MB`.
    pub size_label: String,
    /// `From dana · MacBook` for a received one, `To Laptop` for a sent one.
    pub party: String,
    pub status: TransferStatus,
    /// The status in words, e.g. `Waiting · 23 h left` or `Downloaded`.
    pub status_label: String,
    /// E.g. `5 h left`, while it can still be acted on.
    pub time_left: Option<String>,
    /// Whether it is one file, whose name is then `title`.
    pub single_file: bool,
    /// Whether it was sent to this device, rather than to another one of the user.
    pub this_device: bool,
    /// The sender's note, if any.
    pub note: Option<String>,
}

/// What was sent to the user and what the user sent, the newest first.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TransferLists {
    pub received: Vec<TransferRow>,
    pub sent: Vec<TransferRow>,
}

/// A device files can be sent to.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SendTarget {
    pub user_id: String,
    pub username: String,
    /// Whether it is one of the user's own devices.
    pub own: bool,
    pub device_id: String,
    pub name: String,
    /// One of `protocol`'s `PLATFORM_*` values, or another short name.
    pub platform: String,
    /// E.g. `Linux · online`; with the owner in front for another user's device.
    pub status: String,
    /// Who gets it, as the send button says it: `Laptop`, or `dana’s Phone`.
    pub recipient: String,
    /// What happens after sending.
    pub note: String,
    /// The one to offer first.
    pub preferred: bool,
}

/// A file on this phone to send, by a path the app can read.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SendFile {
    pub name: String,
    pub path: String,
    pub mime: Option<String>,
}

/// The sends and downloads in progress, so that each can be stopped.
#[derive(Default)]
pub(crate) struct Running {
    active: Mutex<HashMap<Uuid, watch::Sender<bool>>>,
}

impl Running {
    pub(crate) fn cancel_all(&self) {
        for cancel in self.active.lock().expect("transfers lock").values() {
            cancel.send_replace(true);
        }
    }

    fn cancel(&self, id: Uuid) {
        if let Some(cancel) = self.active.lock().expect("transfers lock").get(&id) {
            cancel.send_replace(true);
        }
    }
}

/// One transfer being sent or downloaded; a second run of it is refused until this is dropped.
struct Active {
    running: Arc<Running>,
    id: Uuid,
    cancelled: watch::Receiver<bool>,
}

impl Active {
    fn start(running: &Arc<Running>, id: Uuid) -> Result<Self, CoreError> {
        let mut active = running.active.lock().expect("transfers lock");
        if active.contains_key(&id) {
            return Err(CoreError::Failed {
                code: "already_running".to_owned(),
                detail: "this transfer is already under way".to_owned(),
            });
        }
        let (cancel, cancelled) = watch::channel(false);
        active.insert(id, cancel);
        Ok(Self {
            running: running.clone(),
            id,
            cancelled,
        })
    }

    fn check(&self) -> Result<(), CoreError> {
        if *self.cancelled.borrow() {
            Err(CoreError::Cancelled)
        } else {
            Ok(())
        }
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        self.running
            .active
            .lock()
            .expect("transfers lock")
            .remove(&self.id);
    }
}

/// Progress of one transfer, reported at most every [`PROGRESS_INTERVAL`].
struct Reporter<'a> {
    listener: &'a dyn DownloadListener,
    id: String,
    total: i64,
    last: Mutex<Option<Instant>>,
}

impl<'a> Reporter<'a> {
    fn new(listener: &'a dyn DownloadListener, id: Uuid, total: u64) -> Self {
        Self {
            listener,
            id: id.to_string(),
            total: i64::try_from(total).unwrap_or(i64::MAX),
            last: Mutex::new(None),
        }
    }

    fn report(&self, done: u64, force: bool) {
        let mut last = self.last.lock().expect("progress lock");
        if force || last.is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL) {
            *last = Some(Instant::now());
            let done = i64::try_from(done).unwrap_or(i64::MAX);
            self.listener.progress(self.id.clone(), done, self.total);
        }
    }
}

#[uniffi::export]
impl CoreClient {
    /// The transfers sent to the user, and the ones the user sent.
    pub async fn transfers(&self) -> Result<TransferLists, CoreError> {
        let inner = self.inner.clone();
        let me = self.me();
        let device = self.device;
        off_ui_thread(async move {
            let received = inner.incoming_transfers(false).await?;
            let sent = inner.outgoing_transfers().await?;
            let now = OffsetDateTime::now_utc();
            let rows = |list: Vec<TransferSummary>, incoming: bool| {
                list.iter()
                    .map(|transfer| row(transfer, me, device, incoming, now))
                    .collect()
            };
            Ok(TransferLists {
                received: rows(received, true),
                sent: rows(sent, false),
            })
        })
        .await
    }

    /// The devices files can be sent to: the user's own first, then other users'. This device
    /// is left out.
    pub async fn send_targets(&self) -> Result<Vec<SendTarget>, CoreError> {
        let inner = self.inner.clone();
        let targets = off_ui_thread(async move { inner.transfer_targets().await }).await?;
        Ok(send_targets(&targets))
    }

    /// Sends files and folders of the user's tree to a device. The server copies them, so this
    /// returns once the transfer is on its way.
    pub async fn send_entries(
        &self,
        entries: Vec<Entry>,
        user_id: String,
        device_id: String,
    ) -> Result<(), CoreError> {
        let (recipient_id, recipient_device) = recipient(&user_id, &device_id)?;
        let mut taken = BTreeSet::new();
        let mut items = Vec::with_capacity(entries.len());
        for entry in entries {
            // Search results can hold the same name twice.
            let path = send::free_name(&entry.name, &taken);
            taken.insert(path.clone());
            items.push(NewTransferItem {
                path,
                kind: match entry.kind {
                    EntryKind::Folder => NodeKind::Folder,
                    EntryKind::File => NodeKind::File,
                },
                size: 0,
                node_id: Some(parse_id(&entry.id)?),
                mime: None,
                modified_at: None,
            });
        }
        let request = CreateTransfer {
            recipient_id,
            recipient_device,
            items,
            message: None,
        };
        let inner = self.inner.clone();
        off_ui_thread(async move { inner.create_transfer(&request).await }).await?;
        Ok(())
    }

    /// Sends files of this phone to a device, uploading them one after another. `listener`
    /// hears of the new transfer's id first, with nothing done. A failure or
    /// [`CoreClient::cancel_transfer`] takes the transfer off the server again.
    pub async fn send_files(
        &self,
        files: Vec<SendFile>,
        user_id: String,
        device_id: String,
        listener: Arc<dyn DownloadListener>,
    ) -> Result<(), CoreError> {
        let (recipient_id, recipient_device) = recipient(&user_id, &device_id)?;
        let inner = self.inner.clone();
        let running = self.transfers.clone();
        off_ui_thread(async move {
            let mut taken = BTreeSet::new();
            let mut sources: HashMap<String, (PathBuf, u64)> = HashMap::new();
            let mut items = Vec::with_capacity(files.len());
            for file in files {
                let path = PathBuf::from(&file.path);
                let size = fs::metadata(&path).await.map_err(storage)?.len();
                let name = send::free_name(&sanitize_component(file.name.trim()), &taken);
                taken.insert(name.clone());
                sources.insert(name.clone(), (path, size));
                items.push(NewTransferItem {
                    path: name,
                    kind: NodeKind::File,
                    size,
                    node_id: None,
                    mime: file.mime.filter(|mime| !mime.is_empty()),
                    modified_at: None,
                });
            }
            let total = sources.values().map(|(_, size)| size).sum();
            let request = CreateTransfer {
                recipient_id,
                recipient_device,
                items,
                message: None,
            };
            let created = inner.create_transfer(&request).await?;
            let active = Active::start(&running, created.id)?;
            let reporter = Reporter::new(listener.as_ref(), created.id, total);
            reporter.report(0, true);
            let mut before = 0;
            for item in created.items.iter().filter(|item| item.upload_needed) {
                let Some((path, size)) = sources.get(&item.path) else {
                    continue;
                };
                let sent = upload_item(&inner, item.id, &item.path, path, *size, &active, |done| {
                    reporter.report(before + done, false);
                })
                .await;
                if let Err(err) = sent {
                    let _ = inner.cancel_transfer(created.id).await;
                    return Err(err);
                }
                before += size;
            }
            reporter.report(total, true);
            Ok(())
        })
        .await
    }

    /// Downloads every file of a transfer into `dest_dir`, a folder of its own that is emptied
    /// first, and checks each against its hash. Returns the files' paths relative to it. Call
    /// [`CoreClient::confirm_transfer`] once they are where the user wants them.
    pub async fn download_transfer(
        &self,
        id: String,
        dest_dir: String,
        listener: Arc<dyn DownloadListener>,
    ) -> Result<Vec<String>, CoreError> {
        let id = transfer_id(&id)?;
        let inner = self.inner.clone();
        let running = self.transfers.clone();
        off_ui_thread(async move {
            let active = Active::start(&running, id)?;
            let dest = PathBuf::from(dest_dir);
            let _ = fs::remove_dir_all(&dest).await;
            let fetched = fetch(&inner, id, &dest, &active, listener.as_ref()).await;
            if fetched.is_err() {
                let _ = fs::remove_dir_all(&dest).await;
            }
            fetched
        })
        .await
    }

    /// The download is where the user wants it: the server removes its copy.
    pub async fn confirm_transfer(&self, id: String) -> Result<(), CoreError> {
        let id = transfer_id(&id)?;
        let inner = self.inner.clone();
        Ok(off_ui_thread(async move { inner.confirm_transfer(id).await }).await?)
    }

    /// Moves a received transfer into the user's tree under `parent_id`, or Home when `None`.
    pub async fn save_transfer(
        &self,
        id: String,
        parent_id: Option<String>,
    ) -> Result<(), CoreError> {
        let id = transfer_id(&id)?;
        let parent = parent_id.as_deref().map(parse_id).transpose()?;
        let inner = self.inner.clone();
        off_ui_thread(async move { inner.save_transfer(id, parent).await }).await?;
        Ok(())
    }

    /// The recipient declines a transfer.
    pub async fn decline_transfer(&self, id: String) -> Result<(), CoreError> {
        let id = transfer_id(&id)?;
        let inner = self.inner.clone();
        Ok(off_ui_thread(async move { inner.decline_transfer(id).await }).await?)
    }

    /// The sender takes a transfer back, stopping its upload from this phone if one runs.
    pub async fn cancel_transfer(&self, id: String) -> Result<(), CoreError> {
        let id = transfer_id(&id)?;
        self.transfers.cancel(id);
        let inner = self.inner.clone();
        Ok(off_ui_thread(async move { inner.cancel_transfer(id).await }).await?)
    }

    /// Stops a running download of a transfer, whose call then fails as `Cancelled`. The
    /// transfer stays on the server.
    pub fn stop_transfer_download(&self, id: String) -> Result<(), CoreError> {
        self.transfers.cancel(transfer_id(&id)?);
        Ok(())
    }
}

impl CoreClient {
    fn me(&self) -> UserId {
        let id = self.files.user().and_then(|id| id.parse().ok());
        UserId(id.unwrap_or_else(Uuid::nil))
    }
}

fn transfer_id(id: &str) -> Result<Uuid, CoreError> {
    id.parse()
        .map_err(|_| CoreError::InvalidId { id: id.to_owned() })
}

fn recipient(user_id: &str, device_id: &str) -> Result<(UserId, Uuid), CoreError> {
    Ok((UserId(transfer_id(user_id)?), transfer_id(device_id)?))
}

fn row(
    transfer: &TransferSummary,
    me: UserId,
    device: Option<Uuid>,
    incoming: bool,
    now: OffsetDateTime,
) -> TransferRow {
    let time_left = (!transfer.state.is_finished())
        .then(|| words::time_left(transfer.expires_at, now))
        .and_then(|left| left.strip_prefix("in ").map(|left| format!("{left} left")));
    TransferRow {
        id: transfer.id.to_string(),
        title: words::transfer_name(transfer),
        size_label: words::transfer_size(transfer),
        party: if incoming {
            words::transfer_from(transfer, me)
        } else {
            words::transfer_to(transfer, me)
        },
        status: transfer.state.into(),
        status_label: words::transfer_state(transfer.state, transfer.expires_at, now),
        time_left,
        single_file: transfer.item_count == 1 && transfer.file_count == 1,
        this_device: device == Some(transfer.recipient_device),
        note: transfer.message.clone().filter(|note| !note.is_empty()),
    }
}

fn send_targets(targets: &[TransferTarget]) -> Vec<SendTarget> {
    let first = send::first_choice(targets).map(|(_, device)| device.id);
    [true, false]
        .into_iter()
        .flat_map(|own| send::pickable(targets, own))
        .flat_map(|(target, devices)| {
            devices.into_iter().map(move |device| {
                let status = send::device_status(device);
                SendTarget {
                    user_id: target.user.id.to_string(),
                    username: target.user.username.clone(),
                    own: target.own,
                    device_id: device.id.to_string(),
                    name: device.name.clone(),
                    platform: device.platform.clone(),
                    status: if target.own {
                        status
                    } else {
                        format!("{} · {status}", target.user.username)
                    },
                    recipient: send::recipient_name(target, device),
                    note: send::send_note(target, device),
                    preferred: target.own && first == Some(device.id),
                }
            })
        })
        .collect()
}

/// What is being sent from this phone, counted: `3 files · 9.4 MB`.
#[uniffi::export]
pub fn send_summary(files: u32, bytes: u64) -> String {
    send::items_summary(0, files as usize, bytes)
}

/// Uploads the file at `path` as the item `item` of a transfer. `progress` hears how many of
/// its bytes the server has.
async fn upload_item(
    client: &Client,
    item: Uuid,
    name: &str,
    path: &Path,
    size: u64,
    active: &Active,
    progress: impl Fn(u64),
) -> Result<(), CoreError> {
    active.check()?;
    let session = client
        .create_upload(&CreateUpload {
            parent_id: None,
            name: name.to_owned(),
            size,
            modified_at: None,
            mime: None,
            node_id: None,
            keep_both: false,
            version_of: None,
            auto_upload: false,
            transfer_item_id: Some(item),
        })
        .await?;
    let chunk_size = session
        .chunk_size
        .min(client.upload_chunk_bytes())
        .clamp(1, MAX_CHUNK_BYTES);
    let mut offset = session.offset;
    let mut failures = 0;
    loop {
        if let Err(cancelled) = active.check() {
            let _ = client.cancel_upload(session.id).await;
            return Err(cancelled);
        }
        let len =
            usize::try_from(chunk_size.min(size.saturating_sub(offset))).unwrap_or(usize::MAX);
        let chunk = sync_core::read_range(path, offset, len)
            .await
            .map_err(storage)?;
        match client.upload_chunk(session.id, offset, chunk).await {
            Ok(result) => {
                failures = 0;
                progress(result.offset);
                // A transfer item is stored once all its bytes are in; it never becomes a node.
                if result.offset >= size {
                    return Ok(());
                }
                offset = result.offset;
            }
            Err(err) if err.is_transient() && failures < CHUNK_RETRIES => {
                failures += 1;
                let wait = Duration::from_millis(500 * 2u64.pow(failures - 1));
                tokio::time::sleep(wait).await;
                if let Ok(status) = client.upload_status(session.id).await {
                    offset = status.offset;
                    progress(offset);
                }
            }
            Err(err) => {
                let _ = client.cancel_upload(session.id).await;
                return Err(err.into());
            }
        }
    }
}

/// Downloads the files of transfer `id` under `dest` and answers their paths relative to it.
async fn fetch(
    client: &Client,
    id: Uuid,
    dest: &Path,
    active: &Active,
    listener: &dyn DownloadListener,
) -> Result<Vec<String>, CoreError> {
    let transfer = client.transfer(id).await?;
    fs::create_dir_all(dest).await.map_err(storage)?;
    let reporter = Reporter::new(listener, id, transfer.summary.total_bytes);
    reporter.report(0, true);
    let mut cancelled = active.cancelled.clone();
    let mut done = 0;
    let mut files = Vec::new();
    for item in &transfer.items {
        // The server checks paths too, but what is written here is this phone's to guard.
        let components: Vec<String> = item
            .path
            .split('/')
            .filter(|component| !component.is_empty())
            .map(sanitize_component)
            .collect();
        if components.is_empty() {
            continue;
        }
        let path = components
            .iter()
            .fold(dest.to_path_buf(), |path, component| path.join(component));
        if item.kind == NodeKind::Folder {
            fs::create_dir_all(&path).await.map_err(storage)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.map_err(storage)?;
        }
        active.check()?;
        let mut response = client.download_transfer_item(id, item.id).await?;
        let mut out = fs::File::create(&path).await.map_err(storage)?;
        let mut hasher = Sha256::new();
        loop {
            let chunk = tokio::select! {
                biased;
                _ = cancelled.wait_for(|cancelled| *cancelled) => return Err(CoreError::Cancelled),
                chunk = response.chunk() => chunk,
            };
            let Some(chunk) = chunk.map_err(|err| CoreError::Unreachable {
                detail: crate::last_cause(&err.to_string()).to_owned(),
            })?
            else {
                break;
            };
            hasher.update(&chunk);
            out.write_all(&chunk).await.map_err(storage)?;
            done += chunk.len() as u64;
            reporter.report(done, false);
        }
        out.flush().await.map_err(storage)?;
        let sha256 = hex::encode(hasher.finalize());
        if let Some(expected) = &item.sha256
            && !expected.eq_ignore_ascii_case(&sha256)
        {
            return Err(CoreError::Failed {
                code: "damaged".to_owned(),
                detail: format!("{} arrived damaged; download it again", item.path),
            });
        }
        files.push(components.join("/"));
    }
    reporter.report(done, true);
    Ok(files)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;
    use crate::tests::signed_in;

    const TRANSFER: &str = "00000000-0000-0000-0000-0000000000f1";
    const ITEM: &str = "00000000-0000-0000-0000-0000000000f2";
    const UPLOAD: &str = "00000000-0000-0000-0000-0000000000f3";
    const USER: &str = "00000000-0000-0000-0000-0000000000a1";
    const DEVICE: &str = "00000000-0000-0000-0000-0000000000b1";

    #[derive(Default)]
    struct Progress(Mutex<Vec<(String, i64, i64)>>);

    impl DownloadListener for Progress {
        fn progress(&self, id: String, done: i64, total: i64) {
            self.0.lock().unwrap().push((id, done, total));
        }
    }

    fn summary(state: &str, name: Option<&str>, files: u32) -> serde_json::Value {
        json!({
            "id": TRANSFER,
            "sender": { "id": USER, "username": "dana" },
            "sender_device_name": "MacBook",
            "recipient": { "id": "00000000-0000-0000-0000-0000000000a2", "username": "me" },
            "recipient_device": DEVICE,
            "recipient_device_name": "Pixel",
            "state": state,
            "total_bytes": 5,
            "item_count": files,
            "file_count": files,
            "name": name,
            "created_at": "2026-11-10T10:00:00Z",
            "expires_at": "2999-01-01T00:00:00Z",
        })
    }

    #[tokio::test]
    async fn lists_say_who_sent_what_and_how_it_stands() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(protocol::paths::TRANSFERS_INCOMING))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!([summary("ready", None, 3)])),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(protocol::paths::TRANSFERS_OUTGOING))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([summary(
                "downloaded",
                Some("a.pdf"),
                1
            )])))
            .mount(&server)
            .await;
        let client = signed_in(&server).await;

        let lists = client.transfers().await.unwrap();

        let received = &lists.received[0];
        assert_eq!(received.title, "3 files");
        assert_eq!(received.party, "From dana · MacBook");
        assert_eq!(received.status, TransferStatus::Ready);
        assert!(received.time_left.as_deref().unwrap().ends_with(" left"));
        assert!(!received.single_file);
        let sent = &lists.sent[0];
        assert_eq!(sent.title, "a.pdf");
        assert_eq!(sent.status_label, "Downloaded");
        assert_eq!(sent.time_left, None);
        assert!(sent.single_file);
    }

    #[tokio::test]
    async fn targets_list_own_devices_first_and_leave_this_one_out() {
        let device = |id: &str, name: &str, online: bool, current: bool| {
            json!({
                "id": id, "name": name, "platform": "desktop-linux",
                "signed_in": true, "online": online, "current": current,
            })
        };
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(protocol::paths::DEVICE_TARGETS))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {
                    "user": { "id": USER, "username": "dana" },
                    "own": false,
                    "devices": [device(DEVICE, "Phone", true, false)],
                },
                {
                    "user": { "id": "00000000-0000-0000-0000-0000000000a2", "username": "me" },
                    "own": true,
                    "devices": [
                        device("00000000-0000-0000-0000-0000000000b2", "Pixel", true, true),
                        device("00000000-0000-0000-0000-0000000000b3", "iPad", false, false),
                        device("00000000-0000-0000-0000-0000000000b4", "Laptop", true, false),
                    ],
                },
            ])))
            .mount(&server)
            .await;
        let client = signed_in(&server).await;

        let targets = client.send_targets().await.unwrap();

        let shown: Vec<_> = targets
            .iter()
            .map(|target| (target.recipient.as_str(), target.own, target.preferred))
            .collect();
        assert_eq!(
            shown,
            [
                ("iPad", true, false),
                ("Laptop", true, true),
                ("dana’s Phone", false, false),
            ]
        );
        assert!(targets[2].status.starts_with("dana · "));
    }

    #[tokio::test]
    async fn files_of_this_phone_go_up_as_the_items_of_a_new_transfer() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::TRANSFERS))
            .and(body_partial_json(json!({
                "recipient_id": USER,
                "recipient_device": DEVICE,
                "items": [{ "path": "a_b.txt", "kind": "file", "size": 5 }],
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": TRANSFER,
                "state": "uploading",
                "items": [{ "id": ITEM, "path": "a_b.txt", "upload_needed": true }],
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::UPLOADS))
            .and(body_partial_json(
                json!({ "transfer_item_id": ITEM, "size": 5 }),
            ))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(
                    json!({ "id": UPLOAD, "offset": 0, "size": 5, "chunk_size": 3 }),
                ),
            )
            .expect(1)
            .mount(&server)
            .await;
        let received = Arc::new(Mutex::new(Vec::new()));
        let seen = received.clone();
        Mock::given(method("PATCH"))
            .and(path(format!("{}/{UPLOAD}", protocol::paths::UPLOADS)))
            .respond_with(move |request: &Request| {
                let mut received = seen.lock().unwrap();
                received.extend_from_slice(&request.body);
                // The last chunk of a transfer item answers no node.
                ResponseTemplate::new(200).set_body_json(json!({ "offset": received.len() }))
            })
            .mount(&server)
            .await;
        let client = signed_in(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("picked");
        std::fs::write(&file, b"hello").unwrap();
        let progress = Arc::new(Progress::default());

        client
            .send_files(
                vec![SendFile {
                    name: "a/b.txt".to_owned(),
                    path: file.to_string_lossy().into_owned(),
                    mime: None,
                }],
                USER.to_owned(),
                DEVICE.to_owned(),
                progress.clone(),
            )
            .await
            .unwrap();

        assert_eq!(*received.lock().unwrap(), b"hello");
        let reports = progress.0.lock().unwrap();
        assert_eq!(reports.first().unwrap(), &(TRANSFER.to_owned(), 0, 5));
        assert_eq!(reports.last().unwrap(), &(TRANSFER.to_owned(), 5, 5));
    }

    #[tokio::test]
    async fn a_download_keeps_folders_and_refuses_damaged_files() {
        let server = MockServer::start().await;
        let mut transfer = summary("ready", Some("Trip"), 1);
        transfer["items"] = json!([
            { "id": "00000000-0000-0000-0000-0000000000f4", "path": "Trip", "kind": "folder", "size": 0 },
            {
                "id": ITEM, "path": "Trip/a.txt", "kind": "file", "size": 5,
                "sha256": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
            },
        ]);
        Mock::given(method("GET"))
            .and(path(protocol::paths::transfer(TRANSFER.parse().unwrap())))
            .respond_with(ResponseTemplate::new(200).set_body_json(transfer))
            .mount(&server)
            .await;
        let content = protocol::paths::transfer_item_content(
            TRANSFER.parse().unwrap(),
            ITEM.parse().unwrap(),
        );
        Mock::given(method("GET"))
            .and(path(content.clone()))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello".to_vec()))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(content))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hellp".to_vec()))
            .mount(&server)
            .await;
        let client = signed_in(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("in");
        let dest_text = dest.to_string_lossy().into_owned();
        let progress = Arc::new(Progress::default());

        let files = client
            .download_transfer(TRANSFER.to_owned(), dest_text.clone(), progress.clone())
            .await
            .unwrap();

        assert_eq!(files, ["Trip/a.txt"]);
        assert_eq!(std::fs::read(dest.join("Trip/a.txt")).unwrap(), b"hello");
        assert_eq!(progress.0.lock().unwrap().last().unwrap().1, 5);

        let damaged = client
            .download_transfer(TRANSFER.to_owned(), dest_text, progress)
            .await;

        assert!(matches!(damaged, Err(CoreError::Failed { code, .. }) if code == "damaged"));
        assert!(!dest.exists());
    }
}
