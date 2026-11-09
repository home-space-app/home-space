//! The Rust client exported to the mobile apps through UniFFI.
//!
//! The types here are the FFI surface: ids are strings and times are `SystemTime`, so the
//! generated Kotlin and Swift need no custom converters. Everything behind them (routes,
//! JSON, sessions, errors, sorting and formatting) is the same code the desktop and web
//! clients run.

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use api_client::{ApiError, Pkce, VERIFIER_BYTES};
use client_core::{format, icon, labels, listing};
use protocol::{DeviceInfo, NodeId, NodeKind, OnConflict, paths};

#[cfg(target_os = "android")]
mod android;
mod autoupload;
mod events;
mod files;
mod offline;
mod uploads;

pub use autoupload::{
    AutoUpload, AutoUploadListener, LocalItem, LocalSources, PassSummary, SourceKind, SyncSettings,
    UploadSource,
};
pub use events::{EventStream, EventsListener, LiveEvent};
pub use files::DownloadListener;
pub use offline::{OfflineProblem, OfflineState, OfflineStatus};

uniffi::setup_scaffolding!();

/// Where every async call runs. UniFFI polls a future on the caller's thread, which in the apps
/// is the UI thread, so without this the TLS handshake would run there too. On Android that
/// handshake blocks on network calls for revocation checks, which throws
/// `NetworkOnMainThreadException`.
static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("start the async runtime")
});

/// Runs `work` on [`RUNTIME`]. It finishes even if the app stops waiting, so cancelling a call
/// can't lose a refresh token the server has already rotated.
async fn off_ui_thread<T: Send + 'static>(work: impl Future<Output = T> + Send + 'static) -> T {
    match RUNTIME.spawn(work).await {
        Ok(value) => value,
        Err(err) => std::panic::resume_unwind(err.into_panic()),
    }
}

/// The server's limit on a device name.
const MAX_DEVICE_NAME_CHARS: usize = 100;
#[cfg(target_os = "ios")]
const PLATFORM: &str = protocol::PLATFORM_IOS;
#[cfg(not(target_os = "ios"))]
const PLATFORM: &str = protocol::PLATFORM_ANDROID;

/// The end of an error's chain, which names what went wrong: from `request failed: error sending
/// request for url (...): client error (Connect): invalid peer certificate: Expired`, `Expired`.
fn last_cause(message: &str) -> &str {
    message.rsplit(": ").next().unwrap_or(message)
}

/// Failure of a call, as the apps see it.
///
/// No field may be called `message`: Kotlin exceptions already have one, and the generated
/// class would not compile.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("not signed in")]
    Unauthorized,
    /// The server could not be reached, or a gateway answered that it is down.
    #[error("{detail}")]
    Unreachable { detail: String },
    /// Any other failure; `code` is short and stable, such as `conflict` or `502`.
    #[error("{detail}")]
    Failed { code: String, detail: String },
    #[error("invalid id: {id}")]
    InvalidId { id: String },
    /// A password sign-in was refused.
    #[error("invalid username or password")]
    InvalidCredentials,
    /// The server refused the login code of a single sign-on; it may have timed out.
    #[error("the sign-in expired")]
    SsoExpired,
    /// The identity provider refused the user, or the user is not in the required group.
    #[error("not allowed to sign in with this account")]
    SsoDenied,
    /// A single sign-on ended in any other failure.
    #[error("signing in failed")]
    SsoFailed,
    /// What was typed as a server address is not an http or https URL.
    #[error("the server address must start with http:// or https://")]
    InvalidServerAddress,
    /// What was typed is an `http://` address outside the home network, where the password
    /// would cross the internet unencrypted.
    #[error("use https:// for a server outside your home network")]
    UnprotectedServerAddress,
    /// The download was stopped by [`CoreClient::cancel_download`] or by signing out.
    #[error("the download was cancelled")]
    Cancelled,
}

/// A foreign implementation threw something other than a `CoreError`.
impl From<uniffi::UnexpectedUniFFICallbackError> for CoreError {
    fn from(err: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Failed {
            code: "callback".to_owned(),
            detail: err.reason,
        }
    }
}

impl From<api_client::ApiError> for CoreError {
    fn from(err: api_client::ApiError) -> Self {
        if err == api_client::ApiError::Unauthorized {
            Self::Unauthorized
        } else if err.is_unreachable() {
            Self::Unreachable {
                detail: last_cause(&err.to_string()).to_owned(),
            }
        } else {
            Self::Failed {
                code: err.code(),
                detail: err.to_string(),
            }
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ServerHealth {
    pub status: String,
    pub is_ok: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SignedInUser {
    pub id: String,
    pub username: String,
    pub lost_and_found_id: String,
}

/// How a server lets users sign in; at least one of the two is on.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SignInOptions {
    pub password: bool,
    /// Name of the identity provider for the sign-in button, when single sign-on is on.
    pub sso_name: Option<String>,
}

/// A started single sign-on: open `url` in a browser and keep `verifier` for
/// [`CoreClient::finish_sso`].
#[derive(Debug, Clone, uniffi::Record)]
pub struct SsoRequest {
    pub url: String,
    pub verifier: String,
}

/// This install of the app, as the server's device list shows it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct DeviceIdentity {
    /// A UUID the app generates once and keeps.
    pub id: String,
    /// Such as the phone's model.
    pub name: String,
}

/// Told about the refresh token, which the app keeps to resume the session on its next start.
///
/// Called on the thread that made the request and awaited by it. Refresh tokens are single use
/// and replaying an old one ends the session, so save the token before returning. It must not
/// throw.
#[uniffi::export(with_foreign)]
pub trait SessionListener: Send + Sync {
    /// `token` is the one to save now, or `None` when the session ended.
    fn refresh_token_changed(&self, token: Option<String>);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EntryKind {
    Folder,
    File,
}

/// Which icon to show for an entry; the apps bundle one asset for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EntryIcon {
    Folder,
    Image,
    Document,
    Other,
}

/// A file or folder, with the labels the desktop and web clients show for it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Entry {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: EntryKind,
    pub icon: EntryIcon,
    pub name: String,
    pub size: i64,
    pub mime: String,
    pub version: i32,
    pub updated_at: SystemTime,
    /// Lowercase hex SHA-256 of a file's content.
    pub sha256: Option<String>,
    /// The copy of the current content on this phone, for files that have one.
    pub local_path: Option<String>,
    /// How its copy here stands; `None` when it isn't kept on this phone.
    pub offline_state: Option<offline::OfflineState>,
    /// Kept for its own sake, rather than for being in a folder that is, so its copy can be removed.
    pub offline_kept: bool,
    /// Empty for folders, otherwise e.g. `1.5 MB`.
    pub size_label: String,
}

/// A file or folder that matched a search.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SearchResult {
    pub entry: Entry,
    /// Where it is, such as `Home / Photos / 2026`.
    pub location: String,
}

impl From<protocol::Node> for Entry {
    fn from(node: protocol::Node) -> Self {
        let kind = match node.kind {
            NodeKind::Folder => EntryKind::Folder,
            NodeKind::File => EntryKind::File,
        };
        let entry_icon = match icon::node_icon(&node) {
            icon::NodeIcon::Folder => EntryIcon::Folder,
            icon::NodeIcon::Image => EntryIcon::Image,
            icon::NodeIcon::Document => EntryIcon::Document,
            icon::NodeIcon::Other => EntryIcon::Other,
        };
        Self {
            id: node.id.to_string(),
            parent_id: node.parent_id.map(|id| id.to_string()),
            size_label: match kind {
                EntryKind::Folder => String::new(),
                EntryKind::File => format::human_size(node.size),
            },
            updated_at: node.updated_at.into(),
            kind,
            icon: entry_icon,
            name: node.name,
            size: node.size,
            mime: node.mime,
            version: node.version,
            sha256: node.sha256,
            local_path: None,
            offline_state: None,
            offline_kept: false,
        }
    }
}

/// What the details panel shows for a file or folder, like the desktop and web clients.
#[derive(Debug, Clone, uniffi::Record)]
pub struct EntryDetails {
    pub entry: Entry,
    /// The MIME type, or `Folder`.
    pub type_label: String,
    /// When it was created; the apps format this for the device's locale and time zone.
    pub created_at: SystemTime,
    /// `Edited on` for files and `Created on` for folders.
    pub device_title: String,
    /// The device, when the server knows it.
    pub device_label: Option<String>,
    /// The device that auto-uploaded the file, if one did.
    pub source_device: Option<String>,
}

/// A device of the signed-in user.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct UserDevice {
    pub id: String,
    pub name: String,
    /// One of `protocol`'s `PLATFORM_*` values, or another short name.
    pub platform: String,
    /// When the server last heard from it.
    pub last_seen_at: SystemTime,
    pub signed_in: bool,
    /// Whether this is the device that asked.
    pub current: bool,
}

/// What to do about files that already exist where others are moved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MoveMode {
    /// Change nothing and report the conflicts.
    Abort,
    /// The moved file becomes current and the existing content a version.
    Override,
    /// Keep the existing file and put the moved one in the recycle bin.
    Skip,
}

/// A file that clashes with another at the destination.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MoveConflict {
    pub name: String,
    /// Path relative to what is moved.
    pub path: String,
    pub incoming: ConflictSide,
    /// The file in the way; `None` when two selected files share a name.
    pub existing: Option<ConflictSide>,
}

/// One of the files in a conflict.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ConflictSide {
    /// When it last changed; the apps format this for the device's locale and time zone.
    pub updated_at: SystemTime,
    /// E.g. `1.5 KB`.
    pub size_label: String,
}

/// How a move ended. With conflicts nothing changed, and the move can be retried with a mode.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MoveResult {
    /// E.g. `1 file already exists in “Photos”`.
    pub conflicts_title: String,
    pub conflicts: Vec<MoveConflict>,
}

/// Client for one server; the apps keep one per signed-in server.
#[derive(uniffi::Object)]
pub struct CoreClient {
    inner: api_client::Client,
    /// The signed-in user's Lost & Found, which Home leaves out while it is empty.
    lost_and_found: Mutex<Option<NodeId>>,
    files: Arc<files::LocalFiles>,
}

#[uniffi::export]
impl CoreClient {
    /// A signed-out client for the server at `base_url`, e.g. `http://10.0.2.2:8080`.
    #[uniffi::constructor]
    pub fn new(base_url: String) -> Self {
        Self::wrapping(api_client::Client::new(base_url.trim_end_matches('/')))
    }

    /// A signed-out client for `base_url` that signs in as `device`, reports its refresh token
    /// to `listener` and keeps local copies of files under `files_dir`, a folder only it uses.
    #[uniffi::constructor]
    pub fn with_session(
        base_url: String,
        device: DeviceIdentity,
        listener: Arc<dyn SessionListener>,
        files_dir: String,
    ) -> Result<Self, CoreError> {
        let device = device_info(device)?;
        let inner = api_client::Client::with_listener(
            base_url.trim_end_matches('/'),
            Arc::new(move |token: Option<&str>| {
                listener.refresh_token_changed(token.map(str::to_owned));
            }),
        )
        .with_device(device);
        let mut client = Self::wrapping(inner);
        client.files = Arc::new(files::LocalFiles::new(PathBuf::from(files_dir)));
        Ok(client)
    }

    pub fn base_url(&self) -> String {
        self.inner.base_url().to_owned()
    }

    /// How this server lets users sign in; needs no session.
    pub async fn providers(&self) -> Result<SignInOptions, CoreError> {
        let inner = self.inner.clone();
        let providers = off_ui_thread(async move { inner.providers().await }).await?;
        Ok(SignInOptions {
            password: providers.password,
            sso_name: providers.oidc.map(|oidc| oidc.name),
        })
    }

    /// Whether the server and its database are up.
    pub async fn health(&self) -> Result<ServerHealth, CoreError> {
        let inner = self.inner.clone();
        let health = off_ui_thread(async move { inner.health().await }).await?;
        Ok(ServerHealth {
            is_ok: health.is_ok(),
            status: health.status,
        })
    }

    /// Signs in with a password; the session stays in this client.
    pub async fn login(
        &self,
        username: String,
        password: String,
    ) -> Result<SignedInUser, CoreError> {
        let inner = self.inner.clone();
        let me = off_ui_thread(async move { inner.login(&username, &password).await })
            .await
            .map_err(|err| match err {
                ApiError::Unauthorized => CoreError::InvalidCredentials,
                other => other.into(),
            })?;
        Ok(self.signed_in(me))
    }

    /// Starts a single sign-on. The browser comes back to [`paths::MOBILE_OIDC_REDIRECT`].
    pub fn begin_sso(&self) -> Result<SsoRequest, CoreError> {
        let mut bytes = [0u8; VERIFIER_BYTES];
        rand::fill(&mut bytes);
        let pkce = Pkce::from_random(bytes);
        let url = self
            .inner
            .oidc_start_url(paths::MOBILE_OIDC_REDIRECT, &pkce)?;
        Ok(SsoRequest {
            url,
            verifier: pkce.verifier().to_owned(),
        })
    }

    /// Completes a single sign-on from the URL the browser came back to.
    pub async fn finish_sso(
        &self,
        callback_url: String,
        verifier: String,
    ) -> Result<SignedInUser, CoreError> {
        let code = login_code(&callback_url)?;
        let inner = self.inner.clone();
        let pkce = Pkce::from_verifier(verifier);
        let me = off_ui_thread(async move { inner.oidc_exchange(&code, &pkce).await })
            .await
            .map_err(|err| match err {
                ApiError::Unauthorized => CoreError::SsoExpired,
                other => other.into(),
            })?;
        Ok(self.signed_in(me))
    }

    /// Resumes a session from a saved refresh token. `Unauthorized` means that session is gone;
    /// after any other failure the token is still good to retry with. Call it once per token:
    /// it rotates the token, and replaying the old one ends the session.
    pub async fn restore(&self, refresh_token: String) -> Result<SignedInUser, CoreError> {
        let inner = self.inner.clone();
        let me = off_ui_thread(async move { inner.restore(&refresh_token).await }).await?;
        Ok(self.signed_in(me))
    }

    /// Lists a folder, or Home when `parent_id` is `None`, folders first then by name. Home
    /// leaves out Lost & Found while it is empty.
    pub async fn list(&self, parent_id: Option<String>) -> Result<Vec<Entry>, CoreError> {
        let parent = parent_id.as_deref().map(parse_id).transpose()?;
        let inner = self.inner.clone();
        let lost_and_found = *self.lost_and_found.lock().expect("lost and found lock");
        let files = self.files.clone();
        off_ui_thread(async move {
            let mut nodes = match (parent, lost_and_found) {
                (None, Some(found)) => inner.list_home(found).await,
                _ => inner.list(parent).await,
            }?;
            listing::sort_nodes(&mut nodes, listing::Sort::default());
            let mut entries: Vec<Entry> = nodes.into_iter().map(Entry::from).collect();
            files.mark_copies(&inner, &mut entries).await;
            Ok(entries)
        })
        .await
    }

    /// Files and folders whose name matches `query`, best match first.
    pub async fn search(&self, query: String) -> Result<Vec<SearchResult>, CoreError> {
        let inner = self.inner.clone();
        let files = self.files.clone();
        off_ui_thread(async move {
            let hits = inner.search(&query).await?;
            let locations: Vec<String> = hits
                .iter()
                .map(|hit| labels::location_label(&hit.location))
                .collect();
            let mut entries: Vec<Entry> =
                hits.into_iter().map(|hit| Entry::from(hit.node)).collect();
            files.mark_copies(&inner, &mut entries).await;
            Ok(entries
                .into_iter()
                .zip(locations)
                .map(|(entry, location)| SearchResult { entry, location })
                .collect())
        })
        .await
    }

    /// The folders above a file or folder, from the one under Home down to its parent.
    pub async fn ancestors(&self, id: String) -> Result<Vec<Entry>, CoreError> {
        let id = parse_id(&id)?;
        let inner = self.inner.clone();
        let nodes = off_ui_thread(async move { inner.ancestors(id).await }).await?;
        Ok(nodes.into_iter().map(Entry::from).collect())
    }

    /// Creates a folder in `parent_id`, or in Home when `None`.
    pub async fn create_folder(
        &self,
        parent_id: Option<String>,
        name: String,
    ) -> Result<Entry, CoreError> {
        let parent = parent_id.as_deref().map(parse_id).transpose()?;
        let inner = self.inner.clone();
        let node = off_ui_thread(async move { inner.create_folder(parent, &name).await }).await?;
        Ok(node.into())
    }

    /// The user's devices, the most recently seen first.
    pub async fn devices(&self) -> Result<Vec<UserDevice>, CoreError> {
        let inner = self.inner.clone();
        let devices = off_ui_thread(async move { inner.devices().await }).await?;
        Ok(devices
            .into_iter()
            .map(|device| UserDevice {
                id: device.id.to_string(),
                name: device.name,
                platform: device.platform,
                last_seen_at: device.last_seen_at.into(),
                signed_in: device.signed_in,
                current: device.current,
            })
            .collect())
    }

    /// The fields of the details panel. The device is left out if the device list won't load.
    pub async fn details(&self, id: String) -> Result<EntryDetails, CoreError> {
        let id = parse_id(&id)?;
        let inner = self.inner.clone();
        let files = self.files.clone();
        off_ui_thread(async move {
            let node = inner.get(id).await?;
            let devices = inner.devices().await.unwrap_or_default();
            let is_folder = node.kind == NodeKind::Folder;
            let device_label = labels::edited_on(node.edited_on, &devices);
            // Only a label, and an older server has none to give.
            let properties = inner.node_properties(id).await.unwrap_or_default();
            let type_label = if is_folder {
                "Folder".to_owned()
            } else {
                node.mime.clone()
            };
            let created_at = node.created_at.into();
            let mut entries = [Entry::from(node)];
            files.mark_copies(&inner, &mut entries).await;
            let [entry] = entries;
            Ok(EntryDetails {
                entry,
                type_label,
                created_at,
                device_title: if is_folder { "Created on" } else { "Edited on" }.to_owned(),
                device_label,
                source_device: properties.source_device,
            })
        })
        .await
    }

    /// Renames within the same folder; a name that is taken fails as `conflict`.
    pub async fn rename(&self, id: String, name: String) -> Result<Entry, CoreError> {
        let id = parse_id(&id)?;
        let inner = self.inner.clone();
        let node = off_ui_thread(async move { inner.rename(id, &name).await }).await?;
        Ok(node.into())
    }

    /// Moves into `parent_id`, or to Home when `None`. Not a rename's `PATCH`: that silently
    /// merges with a file of the same name, whereas here the user gets to choose.
    pub async fn move_nodes(
        &self,
        ids: Vec<String>,
        parent_id: Option<String>,
        mode: MoveMode,
    ) -> Result<MoveResult, CoreError> {
        let ids = ids
            .iter()
            .map(|id| parse_id(id))
            .collect::<Result<Vec<_>, _>>()?;
        let parent = parent_id.as_deref().map(parse_id).transpose()?;
        let on_conflict = match mode {
            MoveMode::Abort => OnConflict::Abort,
            MoveMode::Override => OnConflict::Override,
            MoveMode::Skip => OnConflict::Skip,
        };
        let inner = self.inner.clone();
        let files = self.files.clone();
        off_ui_thread(async move {
            let moved = inner.move_nodes(&ids, parent, on_conflict).await?;
            for skipped in &moved.skipped {
                files.remove(&inner, *skipped).await;
            }
            let location = moved.target.as_ref().map_or_else(
                || format!("“{}”", labels::HOME),
                |target| format!("“{}”", target.name),
            );
            Ok(MoveResult {
                conflicts_title: format::conflicts_title(moved.conflicts.len(), &location),
                conflicts: moved.conflicts.iter().map(MoveConflict::from).collect(),
            })
        })
        .await
    }

    /// Moves to the recycle bin, one at a time, along with any local copy. Stops at the first
    /// failure, with the ones before it already deleted.
    pub async fn delete(&self, ids: Vec<String>) -> Result<(), CoreError> {
        let ids = ids
            .iter()
            .map(|id| parse_id(id))
            .collect::<Result<Vec<_>, _>>()?;
        let inner = self.inner.clone();
        let files = self.files.clone();
        off_ui_thread(async move {
            for id in ids {
                inner.delete(id).await?;
                files.remove(&inner, id).await;
            }
            Ok(())
        })
        .await
    }

    /// Ends the session here and on the server, and removes the files kept on this phone. They
    /// go first: this device is already signed out, however long the server takes to answer.
    pub async fn logout(&self) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        let files = self.files.clone();
        off_ui_thread(async move {
            files.cancel_all();
            let unfinished = files.wipe().await;
            for upload in unfinished {
                let _ = inner.cancel_upload(upload).await;
            }
            inner.logout().await
        })
        .await
        .map_err(Into::into)
    }
}

impl CoreClient {
    fn wrapping(inner: api_client::Client) -> Self {
        Self {
            inner,
            lost_and_found: Mutex::new(None),
            files: Arc::default(),
        }
    }

    fn signed_in(&self, me: protocol::Me) -> SignedInUser {
        *self.lost_and_found.lock().expect("lost and found lock") = Some(me.lost_and_found_id);
        self.files.set_user(me.id.to_string(), me.lost_and_found_id);
        me.into()
    }
}

impl From<protocol::Me> for SignedInUser {
    fn from(me: protocol::Me) -> Self {
        Self {
            id: me.id.to_string(),
            username: me.username,
            lost_and_found_id: me.lost_and_found_id.to_string(),
        }
    }
}

impl From<&protocol::Node> for ConflictSide {
    fn from(node: &protocol::Node) -> Self {
        Self {
            updated_at: node.updated_at.into(),
            size_label: format::human_size(node.size),
        }
    }
}

impl From<&protocol::Conflict> for MoveConflict {
    fn from(conflict: &protocol::Conflict) -> Self {
        Self {
            name: conflict.source.name.clone(),
            path: conflict.path.clone(),
            incoming: ConflictSide::from(&conflict.source),
            existing: conflict.existing.as_ref().map(ConflictSide::from),
        }
    }
}

fn device_info(device: DeviceIdentity) -> Result<DeviceInfo, CoreError> {
    let id = device.id.parse().map_err(|_| CoreError::InvalidId {
        id: device.id.clone(),
    })?;
    let name: String = device
        .name
        .trim()
        .chars()
        .take(MAX_DEVICE_NAME_CHARS)
        .collect();
    Ok(DeviceInfo {
        id,
        name: if name.is_empty() {
            PLATFORM.to_owned()
        } else {
            name
        },
        platform: PLATFORM.to_owned(),
    })
}

/// The login code in the URL the browser came back to, or why there is none.
fn login_code(callback_url: &str) -> Result<String, CoreError> {
    let mut url = url::Url::parse(callback_url).map_err(|_| CoreError::SsoFailed)?;
    let (mut code, mut error) = (None, None);
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            _ => {}
        }
    }
    url.set_query(None);
    if url.as_str() != paths::MOBILE_OIDC_REDIRECT {
        return Err(CoreError::SsoFailed);
    }
    match (code, error) {
        (Some(code), _) if !code.is_empty() => Ok(code),
        (_, Some(error)) if error == "access_denied" => Err(CoreError::SsoDenied),
        _ => Err(CoreError::SsoFailed),
    }
}

fn parse_id(id: &str) -> Result<NodeId, CoreError> {
    id.parse()
        .map_err(|_| CoreError::InvalidId { id: id.to_owned() })
}

/// Whether moving `entries` into `destination` (`None` for Home) would leave them where they are.
#[uniffi::export]
pub fn already_in(entries: Vec<Entry>, destination: Option<String>) -> bool {
    entries.iter().all(|entry| entry.parent_id == destination)
}

/// Formats a byte count the way the desktop and web clients do, e.g. `1.5 MB`.
#[uniffi::export]
pub fn human_size(bytes: i64) -> String {
    format::human_size(bytes)
}

/// Tidies what was typed as a server address: no surrounding space or trailing slash. Plain
/// `http://` is accepted only for a server on the home network.
#[uniffi::export]
pub fn normalize_server_url(input: String) -> Result<String, CoreError> {
    let trimmed = input.trim().trim_end_matches('/');
    let url = url::Url::parse(trimmed).map_err(|_| CoreError::InvalidServerAddress)?;
    let is_web = matches!(url.scheme(), "http" | "https");
    if !is_web || url.host_str().is_none() {
        Err(CoreError::InvalidServerAddress)
    } else if api_client::is_unprotected(trimmed) {
        Err(CoreError::UnprotectedServerAddress)
    } else {
        Ok(trimmed.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    pub(crate) fn node(name: &str, kind: &str, size: i64) -> serde_json::Value {
        serde_json::json!({
            "id": "00000000-0000-0000-0000-00000000000".to_owned() + &name.len().to_string(),
            "parent_id": null,
            "kind": kind,
            "name": name,
            "size": size,
            "sha256": null,
            "mime": "application/octet-stream",
            "version": 1,
            "created_at": "2026-11-01T10:00:00Z",
            "updated_at": "2026-11-02T11:30:00Z",
        })
    }

    const DEVICE_ID: &str = "00000000-0000-0000-0000-0000000000aa";

    #[test]
    fn entries_carry_the_icon_for_their_kind_and_mime() {
        let icon = |kind: &str, mime: &str| {
            let mut json = node("a", kind, 0);
            json["mime"] = mime.into();
            let node: protocol::Node = serde_json::from_value(json).expect("node");
            Entry::from(node).icon
        };
        assert_eq!(icon("folder", ""), EntryIcon::Folder);
        assert_eq!(icon("file", "image/png"), EntryIcon::Image);
        assert_eq!(icon("file", "application/pdf"), EntryIcon::Document);
        assert_eq!(icon("file", "application/zip"), EntryIcon::Other);
    }

    /// Every token a client reported, in order.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<Option<String>>>);

    impl SessionListener for Recorder {
        fn refresh_token_changed(&self, token: Option<String>) {
            self.0.lock().unwrap().push(token);
        }
    }

    fn device() -> DeviceIdentity {
        DeviceIdentity {
            id: DEVICE_ID.to_owned(),
            name: "Pixel".to_owned(),
        }
    }

    /// A folder for a test's files that lasts until the test process ends.
    fn test_dir() -> String {
        let dir = tempfile::tempdir().expect("temp dir");
        dir.keep().to_string_lossy().into_owned()
    }

    fn session_client(server: &MockServer) -> (CoreClient, Arc<Recorder>) {
        let recorder = Arc::new(Recorder::default());
        let client = CoreClient::with_session(server.uri(), device(), recorder.clone(), test_dir())
            .expect("client");
        (client, recorder)
    }

    fn tokens(refresh_token: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access", "refresh_token": refresh_token, "expires_in": 900,
        }))
    }

    fn unauthorized() -> ResponseTemplate {
        ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": "unauthorized", "message": "invalid credentials",
        }))
    }

    async fn mount_me(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path(protocol::paths::ME))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "00000000-0000-0000-0000-000000000001",
                "username": "me",
                "lost_and_found_id": "00000000-0000-0000-0000-000000000002",
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn providers_reports_both_modes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(protocol::paths::PROVIDERS))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "password": false, "oidc": {"name": "Keycloak"},
            })))
            .mount(&server)
            .await;

        let options = CoreClient::new(server.uri()).providers().await.unwrap();
        assert_eq!(
            options,
            SignInOptions {
                password: false,
                sso_name: Some("Keycloak".to_owned()),
            }
        );
    }

    #[tokio::test]
    async fn a_refused_password_is_invalid_credentials() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::LOGIN))
            .respond_with(unauthorized())
            .mount(&server)
            .await;

        let (client, recorder) = session_client(&server);
        let err = client.login("me".into(), "no".into()).await.unwrap_err();
        assert!(matches!(err, CoreError::InvalidCredentials), "{err:?}");
        assert!(recorder.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn login_names_the_device_and_reports_the_token_then_logout_clears_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::LOGIN))
            .and(body_partial_json(serde_json::json!({
                "device": {"id": DEVICE_ID, "name": "Pixel"},
            })))
            .respond_with(tokens("first"))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::LOGOUT))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        mount_me(&server).await;

        let (client, recorder) = session_client(&server);
        let user = client.login("me".into(), "secret".into()).await.unwrap();
        assert_eq!(user.username, "me");
        client.logout().await.unwrap();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            [Some("first".to_owned()), None]
        );
    }

    /// The thread each token was reported on, which is where the request ran.
    #[derive(Default)]
    struct ThreadRecorder(Mutex<Vec<std::thread::ThreadId>>);

    impl SessionListener for ThreadRecorder {
        fn refresh_token_changed(&self, _token: Option<String>) {
            self.0.lock().unwrap().push(std::thread::current().id());
        }
    }

    #[tokio::test]
    async fn requests_do_not_run_on_the_callers_thread() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::LOGIN))
            .respond_with(tokens("first"))
            .mount(&server)
            .await;
        mount_me(&server).await;

        let recorder = Arc::new(ThreadRecorder::default());
        let client = CoreClient::with_session(server.uri(), device(), recorder.clone(), test_dir())
            .expect("client");
        client.login("me".into(), "secret".into()).await.unwrap();

        let threads = recorder.0.lock().unwrap();
        assert_eq!(threads.len(), 1);
        assert_ne!(threads[0], std::thread::current().id());
    }

    #[test]
    fn unreachable_errors_keep_only_the_cause() {
        assert_eq!(
            last_cause(
                "request failed: error sending request for url (https://x/y): \
                 client error (Connect): invalid peer certificate: Expired"
            ),
            "Expired"
        );
        assert_eq!(last_cause("HTTP 502 Bad Gateway"), "HTTP 502 Bad Gateway");
    }

    #[tokio::test]
    async fn restore_reports_the_rotated_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::REFRESH))
            .and(body_partial_json(
                serde_json::json!({"refresh_token": "saved"}),
            ))
            .respond_with(tokens("rotated"))
            .mount(&server)
            .await;
        mount_me(&server).await;

        let (client, recorder) = session_client(&server);
        let user = client.restore("saved".into()).await.unwrap();
        assert_eq!(user.username, "me");
        assert_eq!(*recorder.0.lock().unwrap(), [Some("rotated".to_owned())]);
    }

    #[tokio::test]
    async fn restoring_an_ended_session_is_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::REFRESH))
            .respond_with(unauthorized())
            .mount(&server)
            .await;

        let (client, _) = session_client(&server);
        let err = client.restore("saved".into()).await.unwrap_err();
        assert!(matches!(err, CoreError::Unauthorized), "{err:?}");
    }

    #[tokio::test]
    async fn restoring_while_offline_keeps_the_saved_token() {
        let recorder = Arc::new(Recorder::default());
        let client = CoreClient::with_session(
            "http://127.0.0.1:9".into(),
            device(),
            recorder.clone(),
            test_dir(),
        )
        .unwrap();
        let err = client.restore("saved".into()).await.unwrap_err();
        assert!(matches!(err, CoreError::Unreachable { .. }), "{err:?}");
        assert!(recorder.0.lock().unwrap().is_empty());
    }

    #[test]
    fn begin_sso_asks_for_the_mobile_redirect_with_a_fresh_verifier() {
        let client = CoreClient::new("https://files.example.com".into());
        let first = client.begin_sso().unwrap();
        let second = client.begin_sso().unwrap();
        assert_ne!(first.verifier, second.verifier);

        let url = url::Url::parse(&first.url).unwrap();
        assert_eq!(url.path(), protocol::paths::OIDC_START);
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query["redirect_uri"], protocol::paths::MOBILE_OIDC_REDIRECT);
        assert_eq!(
            query["code_challenge"],
            Pkce::from_verifier(first.verifier).challenge()
        );
    }

    #[tokio::test]
    async fn finish_sso_exchanges_the_code_with_the_verifier() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::OIDC_EXCHANGE))
            .and(body_partial_json(serde_json::json!({
                "code": "login code",
                "code_verifier": "the-verifier",
                "device": {"id": DEVICE_ID},
            })))
            .respond_with(tokens("first"))
            .mount(&server)
            .await;
        mount_me(&server).await;

        let (client, recorder) = session_client(&server);
        let user = client
            .finish_sso(
                "app.homespace://oidc/callback?code=login%20code".into(),
                "the-verifier".into(),
            )
            .await
            .unwrap();
        assert_eq!(user.username, "me");
        assert_eq!(*recorder.0.lock().unwrap(), [Some("first".to_owned())]);
    }

    #[tokio::test]
    async fn a_refused_login_code_is_expired() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::OIDC_EXCHANGE))
            .respond_with(unauthorized())
            .mount(&server)
            .await;

        let (client, _) = session_client(&server);
        let err = client
            .finish_sso(
                "app.homespace://oidc/callback?code=old".into(),
                "the-verifier".into(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::SsoExpired), "{err:?}");
    }

    #[test]
    fn reads_the_callback_url() {
        assert_eq!(
            login_code("app.homespace://oidc/callback?code=a%20b").unwrap(),
            "a b"
        );
        assert!(matches!(
            login_code("app.homespace://oidc/callback?error=access_denied"),
            Err(CoreError::SsoDenied)
        ));
        for url in [
            "app.homespace://oidc/callback?error=login_failed",
            "app.homespace://oidc/callback?code=",
            "app.homespace://oidc/callback",
            "app.homespace://oidc/other?code=x",
            "app.homespace://oidc/callback?code=x#frag",
            "https://evil.example.com/?code=x",
            "not a url",
        ] {
            assert!(
                matches!(login_code(url), Err(CoreError::SsoFailed)),
                "{url}"
            );
        }
    }

    #[test]
    fn device_names_fit_the_server_limit() {
        let long = DeviceIdentity {
            id: DEVICE_ID.to_owned(),
            name: "é".repeat(150),
        };
        assert_eq!(device_info(long).unwrap().name.chars().count(), 100);

        let blank = DeviceIdentity {
            id: DEVICE_ID.to_owned(),
            name: "  ".to_owned(),
        };
        assert_eq!(device_info(blank).unwrap().name, PLATFORM);

        let bad_id = DeviceIdentity {
            id: "nope".to_owned(),
            name: "Pixel".to_owned(),
        };
        assert!(matches!(
            device_info(bad_id),
            Err(CoreError::InvalidId { .. })
        ));
    }

    #[test]
    fn normalizes_server_addresses() {
        assert_eq!(
            normalize_server_url("  https://files.example.com/ \n".into()).unwrap(),
            "https://files.example.com"
        );
        assert_eq!(
            normalize_server_url("http://10.0.2.2:8080".into()).unwrap(),
            "http://10.0.2.2:8080"
        );
        assert_eq!(
            normalize_server_url("http://nas.local:8080/".into()).unwrap(),
            "http://nas.local:8080"
        );
        assert!(matches!(
            normalize_server_url("http://files.example.com".into()),
            Err(CoreError::UnprotectedServerAddress)
        ));
        for input in [
            "",
            "files.example.com",
            "ftp://files.example.com",
            "http://",
        ] {
            assert!(
                matches!(
                    normalize_server_url(input.into()),
                    Err(CoreError::InvalidServerAddress)
                ),
                "{input}"
            );
        }
    }

    #[tokio::test]
    async fn health_reports_ok() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(protocol::paths::HEALTH))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"status": "ok"})),
            )
            .mount(&server)
            .await;

        let health = CoreClient::new(server.uri()).health().await.unwrap();
        assert!(health.is_ok);
    }

    #[tokio::test]
    async fn list_sorts_folders_first_and_labels_sizes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(protocol::paths::NODES))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                node("b.txt", "file", 1536),
                node("Photos", "folder", 0),
                node("a.txt", "file", 10),
            ])))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path(protocol::paths::LOGIN))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "access", "refresh_token": "refresh", "expires_in": 900,
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(protocol::paths::ME))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "00000000-0000-0000-0000-000000000001",
                "username": "me",
                "lost_and_found_id": "00000000-0000-0000-0000-000000000002",
            })))
            .mount(&server)
            .await;

        let client = CoreClient::new(server.uri());
        let user = client.login("me".into(), "secret".into()).await.unwrap();
        assert_eq!(user.username, "me");
        let entries = client.list(None).await.unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Photos", "a.txt", "b.txt"]);
        assert_eq!(entries[0].size_label, "");
        assert_eq!(entries[2].size_label, "1.5 KB");
        assert_eq!(
            entries[2].updated_at,
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_793_619_000)
        );
    }

    /// The names Home lists for a signed-in user whose Lost & Found holds `contents`.
    async fn home_names(contents: serde_json::Value) -> Vec<String> {
        const LOST_AND_FOUND: &str = "00000000-0000-0000-0000-000000000002";
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::LOGIN))
            .respond_with(tokens("refresh"))
            .mount(&server)
            .await;
        mount_me(&server).await;
        Mock::given(method("GET"))
            .and(path(protocol::paths::NODES))
            .and(query_param("parent_id", LOST_AND_FOUND))
            .respond_with(ResponseTemplate::new(200).set_body_json(contents))
            .mount(&server)
            .await;
        let mut found = node("Lost & Found", "folder", 0);
        found["id"] = LOST_AND_FOUND.into();
        Mock::given(method("GET"))
            .and(path(protocol::paths::NODES))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([found, node("Photos", "folder", 0)])),
            )
            .mount(&server)
            .await;

        let client = CoreClient::new(server.uri());
        client.login("me".into(), "secret".into()).await.unwrap();
        let entries = client.list(None).await.unwrap();
        entries.into_iter().map(|entry| entry.name).collect()
    }

    #[tokio::test]
    async fn home_leaves_out_an_empty_lost_and_found() {
        assert_eq!(home_names(serde_json::json!([])).await, ["Photos"]);
    }

    #[tokio::test]
    async fn home_shows_a_lost_and_found_with_items() {
        let contents = serde_json::json!([node("a.txt", "file", 10)]);
        assert_eq!(home_names(contents).await, ["Lost & Found", "Photos"]);
    }

    #[tokio::test]
    async fn unreachable_server_is_its_own_error() {
        let err = CoreClient::new("http://127.0.0.1:9".into())
            .health()
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Unreachable { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn malformed_id_is_rejected_before_any_request() {
        let err = CoreClient::new("http://127.0.0.1:9".into())
            .list(Some("nope".into()))
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::InvalidId { .. }));
    }

    const FILE_ID: &str = "00000000-0000-0000-0000-0000000000f1";
    const CONTENT: &[u8] = b"hello world";
    const CONTENT_SHA: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
    const OTHER: &[u8] = b"changed";
    /// Not the hash of anything the mock server sends.
    const OTHER_SHA: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    fn file_json(name: &str, sha: &str) -> serde_json::Value {
        let mut json = node(name, "file", CONTENT.len() as i64);
        json["id"] = FILE_ID.into();
        json["sha256"] = sha.into();
        json
    }

    fn file_route() -> String {
        protocol::paths::node(FILE_ID.parse().unwrap())
    }

    fn content_route() -> String {
        protocol::paths::file_content(FILE_ID.parse().unwrap())
    }

    async fn mount_file(server: &MockServer, json: serde_json::Value) {
        server.reset().await;
        mount_login(server).await;
        Mock::given(method("GET"))
            .and(path(file_route()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json))
            .mount(server)
            .await;
    }

    async fn mount_content(server: &MockServer, body: &[u8]) {
        Mock::given(method("GET"))
            .and(path(content_route()))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
            .mount(server)
            .await;
    }

    async fn mount_login(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path(protocol::paths::LOGIN))
            .respond_with(tokens("first"))
            .mount(server)
            .await;
        mount_me(server).await;
    }

    pub(crate) async fn signed_in(server: &MockServer) -> CoreClient {
        mount_login(server).await;
        let (client, _) = session_client(server);
        client.login("me".into(), "secret".into()).await.unwrap();
        client
    }

    /// Every progress report of a download.
    #[derive(Default)]
    struct Progress(Mutex<Vec<(i64, i64)>>);

    impl DownloadListener for Progress {
        fn progress(&self, _id: String, done: i64, total: i64) {
            self.0.lock().unwrap().push((done, total));
        }
    }

    fn listed(server_file: serde_json::Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!([server_file]))
    }

    async fn listed_path(
        client: &CoreClient,
        server: &MockServer,
        json: serde_json::Value,
    ) -> Option<String> {
        listed_state(client, server, json).await.0
    }

    async fn listed_state(
        client: &CoreClient,
        server: &MockServer,
        json: serde_json::Value,
    ) -> (Option<String>, Option<offline::OfflineState>) {
        let folder = "00000000-0000-0000-0000-0000000000d1";
        server.reset().await;
        Mock::given(method("GET"))
            .and(path(protocol::paths::NODES))
            .and(query_param("parent_id", folder))
            .respond_with(listed(json))
            .mount(server)
            .await;
        let entries = client.list(Some(folder.to_owned())).await.unwrap();
        (entries[0].local_path.clone(), entries[0].offline_state)
    }

    #[tokio::test]
    async fn opening_a_file_downloads_it_once_and_reports_progress() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        mount_file(&server, file_json("a.txt", CONTENT_SHA)).await;
        mount_content(&server, CONTENT).await;

        let progress = Arc::new(Progress::default());
        let opened = client
            .open_copy(FILE_ID.into(), progress.clone())
            .await
            .unwrap();
        assert_eq!(std::fs::read(&opened).unwrap(), CONTENT);
        assert!(opened.ends_with("a.txt"), "{opened}");
        let total = CONTENT.len() as i64;
        assert_eq!(progress.0.lock().unwrap().last(), Some(&(total, total)));

        // The content is no longer served, so the second open can only have used the copy.
        mount_file(&server, file_json("a.txt", CONTENT_SHA)).await;
        let again = client.open_copy(FILE_ID.into(), progress).await.unwrap();
        assert_eq!(again, opened);
    }

    #[tokio::test]
    async fn the_listing_marks_the_copy_as_behind_once_the_content_changes() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        let same = file_json("a.txt", CONTENT_SHA);
        assert_eq!(listed_path(&client, &server, same.clone()).await, None);

        mount_file(&server, same.clone()).await;
        mount_content(&server, CONTENT).await;
        let opened = client
            .open_copy(FILE_ID.into(), Arc::new(Progress::default()))
            .await
            .unwrap();

        let (path_of, state_of) = (Some(opened), Some(offline::OfflineState::Synced));
        assert_eq!(
            listed_state(&client, &server, same).await,
            (path_of.clone(), state_of)
        );
        // Still there until the sync or the next open replaces it, and shown as behind.
        let replaced = file_json("a.txt", OTHER_SHA);
        assert_eq!(
            listed_state(&client, &server, replaced).await,
            (path_of, Some(offline::OfflineState::Updating))
        );
    }

    #[tokio::test]
    async fn new_content_replaces_the_old_copy() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        mount_file(&server, file_json("a.txt", CONTENT_SHA)).await;
        mount_content(&server, CONTENT).await;
        let listener = Arc::new(Progress::default());
        let old = client
            .open_copy(FILE_ID.into(), listener.clone())
            .await
            .unwrap();

        let sha = hex_sha(OTHER);
        mount_file(&server, file_json("a.txt", &sha)).await;
        mount_content(&server, OTHER).await;
        let new = client.open_copy(FILE_ID.into(), listener).await.unwrap();

        assert_eq!(new, old, "it stays where it is");
        assert_eq!(std::fs::read(&new).unwrap(), OTHER);
    }

    fn hex_sha(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(bytes))
    }

    #[tokio::test]
    async fn a_rename_on_the_server_keeps_the_copy_under_the_new_name() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        mount_file(&server, file_json("a.txt", CONTENT_SHA)).await;
        mount_content(&server, CONTENT).await;
        let listener = Arc::new(Progress::default());
        client
            .open_copy(FILE_ID.into(), listener.clone())
            .await
            .unwrap();

        mount_file(&server, file_json("b.txt", CONTENT_SHA)).await;
        let renamed = client.open_copy(FILE_ID.into(), listener).await.unwrap();
        assert!(renamed.ends_with("b.txt"), "{renamed}");
        assert_eq!(std::fs::read(&renamed).unwrap(), CONTENT);
    }

    #[tokio::test]
    async fn a_file_replaced_during_the_download_starts_over_then_gives_up() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        mount_file(&server, file_json("a.txt", OTHER_SHA)).await;
        mount_content(&server, CONTENT).await;

        let err = client
            .open_copy(FILE_ID.into(), Arc::new(Progress::default()))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, CoreError::Failed { code, .. } if code == "content_changed"),
            "{err:?}"
        );
        let requests = server.received_requests().await.unwrap();
        let downloads = requests
            .iter()
            .filter(|request| request.url.path() == content_route())
            .count();
        assert_eq!(downloads, 2);
    }

    /// Cancels the download from inside its first progress report.
    struct CancelOnProgress(Mutex<Option<Arc<CoreClient>>>);

    impl DownloadListener for CancelOnProgress {
        fn progress(&self, id: String, _done: i64, _total: i64) {
            if let Some(client) = self.0.lock().unwrap().take() {
                client.cancel_download(id).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn cancelling_leaves_nothing_behind_and_a_new_download_can_start() {
        let server = MockServer::start().await;
        let client = Arc::new(signed_in(&server).await);
        mount_file(&server, file_json("a.txt", CONTENT_SHA)).await;
        mount_content(&server, CONTENT).await;

        let listener = Arc::new(CancelOnProgress(Mutex::new(Some(client.clone()))));
        let err = client
            .open_copy(FILE_ID.into(), listener)
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Cancelled), "{err:?}");
        let same = file_json("a.txt", CONTENT_SHA);

        let offline = client.files.offline(&client.inner).await.unwrap();
        assert!(
            !offline
                .part_path(FILE_ID.parse().unwrap())
                .unwrap()
                .exists()
        );
        assert_eq!(listed_path(&client, &server, same.clone()).await, None);

        mount_file(&server, same).await;
        mount_content(&server, CONTENT).await;
        client
            .open_copy(FILE_ID.into(), Arc::new(Progress::default()))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn the_copy_opens_while_the_server_is_down() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        mount_file(&server, file_json("a.txt", CONTENT_SHA)).await;
        mount_content(&server, CONTENT).await;
        let listener = Arc::new(Progress::default());
        let opened = client
            .open_copy(FILE_ID.into(), listener.clone())
            .await
            .unwrap();

        server.reset().await;
        Mock::given(method("GET"))
            .and(path(file_route()))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        assert_eq!(
            client.open_copy(FILE_ID.into(), listener).await.unwrap(),
            opened
        );
    }

    #[tokio::test]
    async fn saving_writes_the_destination_and_leaves_no_copy() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        let same = file_json("a.txt", CONTENT_SHA);
        mount_file(&server, same.clone()).await;
        mount_content(&server, CONTENT).await;

        let dest = std::path::Path::new(&test_dir())
            .join("out")
            .join("saved.txt");
        client
            .save_copy(
                FILE_ID.into(),
                dest.to_string_lossy().into_owned(),
                Arc::new(Progress::default()),
            )
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), CONTENT);
        assert_eq!(listed_path(&client, &server, same).await, None);
    }

    #[tokio::test]
    async fn signing_out_removes_the_copies_even_if_the_server_fails() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        mount_file(&server, file_json("a.txt", CONTENT_SHA)).await;
        mount_content(&server, CONTENT).await;
        let opened = client
            .open_copy(FILE_ID.into(), Arc::new(Progress::default()))
            .await
            .unwrap();

        server.reset().await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::LOGOUT))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let _ = client.logout().await;
        assert!(!std::path::Path::new(&opened).exists());
    }

    #[tokio::test]
    async fn deleting_a_file_removes_its_copy() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        mount_file(&server, file_json("a.txt", CONTENT_SHA)).await;
        mount_content(&server, CONTENT).await;
        let opened = client
            .open_copy(FILE_ID.into(), Arc::new(Progress::default()))
            .await
            .unwrap();

        server.reset().await;
        Mock::given(method("DELETE"))
            .and(path(file_route()))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        client.delete(vec![FILE_ID.into()]).await.unwrap();
        assert!(!std::path::Path::new(&opened).exists());
    }

    #[tokio::test]
    async fn a_taken_name_is_a_conflict() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(file_route()))
            .and(body_partial_json(serde_json::json!({"name": "b.txt"})))
            .respond_with(ResponseTemplate::new(409).set_body_json(serde_json::json!({
                "error": "conflict", "message": "name already exists",
            })))
            .mount(&server)
            .await;
        let err = signed_in(&server)
            .await
            .rename(FILE_ID.into(), "b.txt".into())
            .await
            .unwrap_err();
        assert!(
            matches!(&err, CoreError::Failed { code, .. } if code == "conflict"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn a_move_reports_its_conflicts_with_labels() {
        let server = MockServer::start().await;
        let mut target = node("Photos", "folder", 0);
        target["id"] = "00000000-0000-0000-0000-0000000000d1".into();
        Mock::given(method("POST"))
            .and(path(protocol::paths::NODES_MOVE))
            .and(body_partial_json(
                serde_json::json!({"on_conflict": "abort"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "moved": [], "skipped": [], "target": target,
                "conflicts": [{
                    "source": file_json("a.txt", CONTENT_SHA),
                    "path": "a.txt",
                    "existing": null,
                }],
            })))
            .mount(&server)
            .await;
        let result = signed_in(&server)
            .await
            .move_nodes(vec![FILE_ID.into()], None, MoveMode::Abort)
            .await
            .unwrap();
        assert_eq!(result.conflicts_title, "1 file already exists in “Photos”");
        assert_eq!(result.conflicts[0].name, "a.txt");
        assert_eq!(result.conflicts[0].incoming.size_label, "11 B");
        assert_eq!(
            result.conflicts[0].incoming.updated_at,
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_793_619_000)
        );
        assert!(result.conflicts[0].existing.is_none());
    }

    #[test]
    fn already_in_compares_every_parent() {
        let entry = |parent: Option<&str>| {
            let mut json = node("a", "file", 0);
            json["parent_id"] = parent.map_or(serde_json::Value::Null, Into::into);
            Entry::from(serde_json::from_value::<protocol::Node>(json).unwrap())
        };
        let parent = "00000000-0000-0000-0000-0000000000d1";
        assert!(already_in(vec![entry(None)], None));
        assert!(!already_in(vec![entry(None)], Some(parent.to_owned())));
        assert!(!already_in(vec![entry(None), entry(Some(parent))], None));
    }

    #[tokio::test]
    async fn an_edit_is_kept_when_the_file_opens_again_and_is_reported_as_unsent() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        mount_file(&server, file_json("a.txt", CONTENT_SHA)).await;
        mount_content(&server, CONTENT).await;
        let listener = Arc::new(Progress::default());
        let opened = client
            .open_copy(FILE_ID.into(), listener.clone())
            .await
            .unwrap();
        assert!(client.unsent_edits(None).await.unwrap().is_empty());

        std::fs::write(&opened, "edited").unwrap();
        // Even with newer content on the server, the edit isn't overwritten here.
        mount_file(&server, file_json("a.txt", OTHER_SHA)).await;
        let again = client.open_copy(FILE_ID.into(), listener).await.unwrap();

        assert_eq!(again, opened);
        assert_eq!(std::fs::read(&again).unwrap(), b"edited");
        assert_eq!(client.unsent_edits(None).await.unwrap(), ["a.txt"]);
        assert_eq!(
            client.unsent_edits(Some(FILE_ID.into())).await.unwrap(),
            ["a.txt"]
        );
    }

    #[tokio::test]
    async fn an_opened_file_is_kept_and_its_copy_can_be_removed() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        let same = file_json("a.txt", CONTENT_SHA);
        mount_file(&server, same.clone()).await;
        mount_content(&server, CONTENT).await;
        let opened = client
            .open_copy(FILE_ID.into(), Arc::new(Progress::default()))
            .await
            .unwrap();
        let offline = client.files.offline(&client.inner).await.unwrap();
        let mut status = offline.status();
        for _ in 0..200 {
            if status.mapping(Some(FILE_ID.parse().unwrap())).is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            status = offline.status();
        }
        assert_eq!(client.offline_status().await.unwrap().kept, 1);

        client.remove_offline_copy(FILE_ID.into()).await.unwrap();
        assert!(!std::path::Path::new(&opened).exists());
        assert_eq!(listed_path(&client, &server, same).await, None);
        assert_eq!(client.offline_status().await.unwrap().kept, 0);
    }

    #[tokio::test]
    async fn search_results_say_where_they_are() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        Mock::given(method("GET"))
            .and(path(protocol::paths::SEARCH))
            .and(query_param("q", "hol"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "node": node("beach.jpg", "file", 3), "location": ["Photos", "2026"] },
                { "node": node("hol", "folder", 0), "location": [] },
            ])))
            .mount(&server)
            .await;

        let results = client.search("hol".into()).await.unwrap();

        let found: Vec<_> = results
            .iter()
            .map(|result| (result.entry.name.as_str(), result.location.as_str()))
            .collect();
        assert_eq!(
            found,
            [("beach.jpg", "Home / Photos / 2026"), ("hol", "Home")]
        );
    }

    #[tokio::test]
    async fn devices_keep_the_servers_order_and_say_which_is_this_one() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(protocol::paths::DEVICES))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": "00000000-0000-0000-0000-00000000000a",
                    "name": "Pixel",
                    "platform": "android",
                    "created_at": "2026-11-01T00:00:00Z",
                    "last_seen_at": "2026-11-07T08:00:00Z",
                    "signed_in": true,
                    "current": true,
                    "uploads": 0,
                },
                {
                    "id": "00000000-0000-0000-0000-00000000000b",
                    "name": "Laptop",
                    "platform": "desktop-linux",
                    "created_at": "2026-10-01T00:00:00Z",
                    "last_seen_at": "2026-11-03T22:10:00Z",
                    "signed_in": false,
                    "current": false,
                    "uploads": 0,
                },
            ])))
            .mount(&server)
            .await;
        let client = signed_in(&server).await;

        let devices = client.devices().await.unwrap();

        let summary: Vec<_> = devices
            .iter()
            .map(|device| (device.name.as_str(), device.signed_in, device.current))
            .collect();
        assert_eq!(summary, [("Pixel", true, true), ("Laptop", false, false)]);
    }

    #[tokio::test]
    async fn a_new_folder_goes_in_the_folder_asked_for() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::FOLDERS))
            .and(body_partial_json(serde_json::json!({
                "name": "New", "parent_id": FILE_ID,
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(node("New", "folder", 0)))
            .expect(1)
            .mount(&server)
            .await;

        let entry = client
            .create_folder(Some(FILE_ID.into()), "New".into())
            .await
            .unwrap();

        assert_eq!(entry.kind, EntryKind::Folder);
    }
}
