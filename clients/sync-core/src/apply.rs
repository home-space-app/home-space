use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

use futures::{StreamExt, stream};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::{fs, io::AsyncWriteExt};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use api_client::{ApiError, Client};
use protocol::{CreateUpload, ErrorCode, NodeId, NodeKind, UploadSession};

use super::{
    model::{Base, LocalEntry, RemoteNode, SavedUpload, datetime, nanos},
    plan::Action,
    queue::{self, Active, Fault, Progress},
    scan::{hash_file, is_ignored},
    store::Store,
};
use crate::{Platform, layout::MAX_NAME_BYTES};

const PART_SUFFIX: &str = ".part";
/// Hex digits of the full name kept in a part file's name when the name itself doesn't fit.
const PART_HASH_LEN: usize = 16;

/// Where a download into `target` is written until it is complete.
pub fn part_path(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    // A name at the file system's limit leaves no room for the suffix. The hash keeps two long
    // names that start alike apart, as downloads run at the same time. It is the same on each try,
    // so what an interrupted download left is overwritten rather than piling up.
    if name.len() + PART_SUFFIX.len() > MAX_NAME_BYTES {
        let hash = hex::encode(Sha256::digest(name.as_bytes()));
        name.truncate(
            name.floor_char_boundary(MAX_NAME_BYTES - PART_SUFFIX.len() - PART_HASH_LEN - 1),
        );
        name.push('-');
        name.push_str(&hash[..PART_HASH_LEN]);
    }
    name.push_str(PART_SUFFIX);
    target.with_file_name(name)
}

/// `len` bytes of the file at `path` from `offset`, for one chunk of an upload.
pub async fn read_range(path: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let mut file = fs::File::open(path).await?;
    file.seek(io::SeekFrom::Start(offset)).await?;
    let mut chunk = Vec::with_capacity(len);
    file.take(u64::try_from(len).unwrap_or(u64::MAX))
        .read_to_end(&mut chunk)
        .await?;
    if chunk.len() == len {
        Ok(chunk)
    } else {
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the file got shorter during the upload",
        ))
    }
}

/// What a finished action changed, for the engine to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Base(Base),
    /// Drops the base of a node; with a path, only while it is still recorded there.
    Forget(NodeId, Option<PathBuf>),
    Remote(RemoteNode),
    RemoteGone(NodeId),
    /// Everything recorded at and under `from` is now under `to`.
    Rebase {
        from: PathBuf,
        to: PathBuf,
    },
    /// A remote folder now stands for the local folder at the path.
    Folder(PathBuf, NodeId),
}

/// Code of a failure on this computer, such as a file that could not be read.
pub const LOCAL_CODE: &str = "local";
/// Code of a path that cannot be synced as it is.
pub const BLOCKED_CODE: &str = "blocked";

/// Why an action did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The local file changed after the scan; the next pass looks again.
    Stale,
    /// Not started, since the server became unreachable during the pass.
    Halted,
    /// The server could not be reached, which is no fault of the path.
    Unreachable(Fault),
    Error(Fault),
    SignedOut,
}

impl Failure {
    fn local(reason: impl Into<String>) -> Self {
        Self::Error(Fault::now(LOCAL_CODE, reason))
    }

    fn blocked(reason: impl Into<String>) -> Self {
        Self::Error(Fault::now(BLOCKED_CODE, reason))
    }
}

impl From<ApiError> for Failure {
    fn from(err: ApiError) -> Self {
        match err {
            ApiError::Unauthorized => Self::SignedOut,
            other if other.is_unreachable() => {
                Self::Unreachable(Fault::now(other.code(), other.to_string()))
            }
            other => Self::Error(Fault::now(other.code(), other.to_string())),
        }
    }
}

impl From<io::Error> for Failure {
    fn from(err: io::Error) -> Self {
        Self::local(err.to_string())
    }
}

impl From<rusqlite::Error> for Failure {
    fn from(err: rusqlite::Error) -> Self {
        Self::local(format!("the sync database: {err}"))
    }
}

/// The remote folder for each local folder that has one, `None` standing for Home.
pub type Folders = HashMap<PathBuf, Option<NodeId>>;

/// Carries out `action`, reporting the bytes of a transfer to `progress`.
pub async fn execute(
    client: &Client,
    store: &Store,
    platform: &Arc<dyn Platform>,
    folders: &Folders,
    progress: &Progress,
    action: Action,
) -> Result<Vec<Effect>, Failure> {
    match action {
        Action::MoveLocal { from, to, .. } => {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).await?;
            }
            if fs::try_exists(&to).await? {
                return Err(Failure::Stale);
            }
            fs::rename(&from, &to).await?;
            Ok(vec![Effect::Rebase { from, to }])
        }
        Action::MoveRemote { node, from, to } => {
            let (parent, name) = place(folders, &to)?;
            let moved = client.relocate(node, parent, &name).await?;
            Ok(vec![
                Effect::Remote(moved.into()),
                Effect::Rebase { from, to },
            ])
        }
        Action::CreateLocalDir { node, path } => {
            fs::create_dir_all(&path).await?;
            Ok(vec![Effect::Base(Base::new(
                node,
                path,
                &LocalEntry::folder(),
            ))])
        }
        Action::CreateRemoteDir { path } => {
            let (parent, name) = place(folders, &path)?;
            let folder = match client.create_folder(parent, &name).await {
                Err(ApiError::Api {
                    code: ErrorCode::Conflict,
                    ..
                }) => client
                    .list(parent)
                    .await?
                    .into_iter()
                    .find(|node| node.kind == NodeKind::Folder && node.name == name)
                    .ok_or_else(|| Failure::blocked("a file of this name is on the server"))?,
                other => other?,
            };
            let id = folder.id;
            Ok(vec![
                Effect::Remote(folder.into()),
                Effect::Folder(path.clone(), id),
                Effect::Base(Base::new(id, path, &LocalEntry::folder())),
            ])
        }
        Action::Download { node, path, expect } => {
            let base = download(client, progress, &node, &path, expect.as_ref()).await?;
            Ok(vec![Effect::Base(base)])
        }
        Action::Upload { node, path, local } => {
            upload(client, store, folders, progress, node, path, local).await
        }
        Action::Adopt { node, path, local } => {
            Ok(vec![Effect::Base(Base::new(node, path, &local))])
        }
        Action::DeleteRemote { node, .. } => {
            match client.delete(node).await {
                Err(ApiError::Api {
                    code: ErrorCode::NotFound,
                    ..
                }) => {}
                other => other?,
            }
            Ok(vec![Effect::RemoteGone(node), Effect::Forget(node, None)])
        }
        Action::TrashLocal { node, path, expect } => {
            unchanged(&path, Some(&expect)).await?;
            let (platform, target) = (Arc::clone(platform), path.clone());
            tokio::task::spawn_blocking(move || platform.remove_file(&target))
                .await
                .map_err(|err| Failure::local(err.to_string()))?
                .map_err(|err| Failure::local(err.to_string()))?;
            Ok(vec![Effect::Forget(node, Some(path))])
        }
        Action::RemoveLocalDir { node, path } => {
            let target = path.clone();
            let removed = tokio::task::spawn_blocking(move || remove_if_empty(&target))
                .await
                .map_err(|err| Failure::local(err.to_string()))??;
            // A folder still holding something unsynced is left; the next pass tries again.
            Ok(if removed {
                vec![Effect::Forget(node, Some(path))]
            } else {
                Vec::new()
            })
        }
        Action::Forget { node } => Ok(vec![Effect::Forget(node, None)]),
        Action::Blocked { reason, .. } => Err(Failure::blocked(reason)),
    }
}

/// What the transfers running at the same time share.
#[derive(Clone, Copy)]
pub struct Shared<'a> {
    pub client: &'a Client,
    pub store: &'a Store,
    pub platform: &'a Arc<dyn Platform>,
    pub folders: &'a Folders,
    pub progress: &'a Progress,
}

/// Carries out `actions` at most `limit` at a time, yielding each with its outcome. Once
/// `halt` is set, the actions not started yet fail as [`Failure::Halted`]; those running
/// finish.
pub fn execute_all<'a>(
    with: Shared<'a>,
    halt: &'a AtomicBool,
    actions: Vec<Action>,
    limit: usize,
) -> impl futures::Stream<Item = (Action, Result<Vec<Effect>, Failure>)> + 'a {
    stream::iter(actions)
        .map(move |action| async move {
            let outcome = if halt.load(Ordering::Relaxed) || queue::halted(with.progress) {
                Err(Failure::Halted)
            } else {
                execute(
                    with.client,
                    with.store,
                    with.platform,
                    with.folders,
                    with.progress,
                    action.clone(),
                )
                .await
            };
            (action, outcome)
        })
        .buffer_unordered(limit)
}

/// The remote parent and name of the local `path`.
fn place(folders: &Folders, path: &Path) -> Result<(Option<NodeId>, String), Failure> {
    let parent = path
        .parent()
        .and_then(|parent| folders.get(parent))
        .ok_or_else(|| Failure::blocked("its folder is not on the server yet"))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Failure::blocked("the name is not valid Unicode"))?;
    Ok((*parent, name.to_owned()))
}

async fn upload(
    client: &Client,
    store: &Store,
    folders: &Folders,
    progress: &Progress,
    node: Option<NodeId>,
    path: PathBuf,
    mut local: LocalEntry,
) -> Result<Vec<Effect>, Failure> {
    let _active = Active::start(progress, &path);
    unchanged(&path, Some(&local)).await?;
    let sha256 = match local.sha256.clone() {
        Some(sha256) => sha256,
        None => {
            // Left to the upload by the scan. The file is read again just after, from the cache.
            let target = path.clone();
            let stop = AtomicBool::new(false);
            let sha256 = tokio::task::spawn_blocking(move || hash_file(&target, &stop))
                .await
                .map_err(|err| Failure::local(err.to_string()))??;
            unchanged(&path, Some(&local)).await?;
            local.sha256 = Some(sha256.clone());
            local.unhashed = false;
            sha256
        }
    };
    let modified = Some(upload_time(store, node, &local)?);
    let saved = store.saved_upload(&path)?;
    let stored = if local.size > client.upload_chunk_bytes() {
        let (parent, name) = match node {
            Some(_) => (None, String::new()),
            None => place(folders, &path)?,
        };
        let wanted = SavedUpload {
            path: path.clone(),
            id: Uuid::nil(),
            node,
            parent,
            name,
            size: local.size,
            mtime: local.mtime,
            sha256,
        };
        queue::set(progress, &path, 0);
        let session = match resumable(client, store, saved, &wanted).await? {
            Some(session) => session,
            None => {
                let request = CreateUpload {
                    parent_id: wanted.parent,
                    name: wanted.name.clone(),
                    size: wanted.size,
                    modified_at: modified,
                    mime: None,
                    node_id: wanted.node,
                    keep_both: false,
                    version_of: None,
                    auto_upload: false,
                    transfer_item_id: None,
                };
                let session = client.create_upload(&request).await?;
                store.save_upload(&SavedUpload {
                    id: session.id,
                    ..wanted
                })?;
                session
            }
        };
        queue::set(progress, &path, session.offset);
        let sent = client
            .continue_upload(
                session,
                local.size,
                |offset, len| {
                    let path = &path;
                    async move {
                        if queue::halted(progress) {
                            return Err(io::ErrorKind::Interrupted.into());
                        }
                        read_range(path, offset, len).await
                    }
                },
                |done| queue::set(progress, &path, done),
            )
            .await;
        match sent {
            Ok(stored) => {
                store.forget_upload(&path)?;
                stored
            }
            // Told to stop, not refused: the next pass continues from what the server has.
            Err(_) if queue::halted(progress) => return Err(Failure::Halted),
            Err(err) if is_passing(&err) => return Err(err.into()),
            Err(err) => {
                // The server will not take the rest; the next pass starts over.
                let _ = client.cancel_upload(session.id).await;
                store.forget_upload(&path)?;
                return Err(err.into());
            }
        }
    } else {
        if let Some(saved) = saved {
            discard(client, store, &saved).await?;
        }
        let body = || file_body(&path, progress);
        match node {
            Some(id) => client.replace_content(id, modified, body).await?,
            None => {
                let (parent, name) = place(folders, &path)?;
                client
                    .upload_modified(parent, &name, modified, body)
                    .await?
            }
        }
    };
    let stored = RemoteNode::from(stored);
    let base = if stored.sha256 == local.sha256 {
        Base::new(stored.id, path, &local)
    } else {
        // The server's content changed later and stays current; ours is now a version.
        download(client, progress, &stored, &path, Some(&local)).await?
    };
    Ok(vec![Effect::Remote(stored), Effect::Base(base)])
}

/// The modification time to send a file with. The server keeps the later content as current, and
/// a rename or move there counts as later too. So when the server's content is still what this
/// edit started from, the edit goes as later than that: it is the only change to the content.
fn upload_time(
    store: &Store,
    node: Option<NodeId>,
    local: &LocalEntry,
) -> Result<OffsetDateTime, Failure> {
    let edited = datetime(local.mtime);
    let Some(id) = node else {
        return Ok(edited);
    };
    let (Some(base), Some(remote)) = (store.base(id)?, store.remote_node(id)?) else {
        return Ok(edited);
    };
    Ok(
        if base.sha256 == remote.sha256 && remote.updated_at >= edited {
            remote.updated_at + time::Duration::seconds(1)
        } else {
            edited
        },
    )
}

/// The session of `saved` to continue, when it still sends `wanted` and the server still has
/// it. A saved upload of other content or to another place is cancelled.
async fn resumable(
    client: &Client,
    store: &Store,
    saved: Option<SavedUpload>,
    wanted: &SavedUpload,
) -> Result<Option<UploadSession>, Failure> {
    let Some(saved) = saved else {
        return Ok(None);
    };
    if !saved.is_same(wanted) {
        discard(client, store, &saved).await?;
        return Ok(None);
    }
    match client.upload_status(saved.id).await {
        Ok(session) => Ok(Some(session)),
        Err(err) if is_passing(&err) => Err(err.into()),
        Err(_) => {
            // Expired or cancelled on the server.
            store.forget_upload(&saved.path)?;
            Ok(None)
        }
    }
}

/// Cancels `saved` on the server and forgets it. It stays saved while the server cannot be
/// asked, so that a later pass cancels it.
pub async fn discard(client: &Client, store: &Store, saved: &SavedUpload) -> Result<(), Failure> {
    match client.cancel_upload(saved.id).await {
        Err(err) if is_passing(&err) => Err(err.into()),
        _ => Ok(store.forget_upload(&saved.path)?),
    }
}

/// Whether `err` may pass, so that an upload is worth keeping for later.
fn is_passing(err: &ApiError) -> bool {
    err.is_transient() || *err == ApiError::Unauthorized
}

/// Writes the content of `node` to `path` through a partial file, unless the local file is
/// no longer `expect`. The file takes the node's modification time.
async fn download(
    client: &Client,
    progress: &Progress,
    node: &RemoteNode,
    path: &Path,
    expect: Option<&LocalEntry>,
) -> Result<Base, Failure> {
    let _active = Active::start(progress, path);
    unchanged(path, expect).await?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let response = client.download(node.id).await?;
    let part = part_path(path);
    let written = write_part(response, &part, node, progress, path).await;
    let sha256 = match written {
        Ok(sha256) => sha256,
        Err(err) => {
            let _ = fs::remove_file(&part).await;
            return Err(if err.kind() == io::ErrorKind::Interrupted {
                Failure::Halted
            } else {
                err.into()
            });
        }
    };
    if let Err(failure) = unchanged(path, expect).await {
        let _ = fs::remove_file(&part).await;
        return Err(failure);
    }
    fs::rename(&part, path).await?;
    let meta = fs::metadata(path).await?;
    Ok(Base {
        node: node.id,
        path: path.to_path_buf(),
        kind: NodeKind::File,
        sha256: Some(sha256),
        size: meta.len(),
        mtime: meta.modified().map(nanos).unwrap_or(0),
    })
}

/// Streams the response into `part`, returning the hash of what was written.
async fn write_part(
    response: reqwest::Response,
    part: &Path,
    node: &RemoteNode,
    progress: &Progress,
    path: &Path,
) -> io::Result<String> {
    let mut out = fs::File::create(part).await?;
    let mut hasher = Sha256::new();
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        if queue::halted(progress) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let chunk = chunk.map_err(io::Error::other)?;
        hasher.update(&chunk);
        out.write_all(&chunk).await?;
        queue::advance(progress, path, chunk.len() as u64);
    }
    out.flush().await?;
    let out = out.into_std().await;
    let modified = SystemTime::from(node.updated_at);
    tokio::task::spawn_blocking(move || out.set_modified(modified))
        .await
        .map_err(io::Error::other)??;
    Ok(hex::encode(hasher.finalize()))
}

/// Fails as [`Failure::Stale`] unless the file at `path` still has the size and time of
/// `expect`, or is still absent when `expect` is `None`.
async fn unchanged(path: &Path, expect: Option<&LocalEntry>) -> Result<(), Failure> {
    let meta = match fs::symlink_metadata(path).await {
        Ok(meta) => Some(meta),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err.into()),
    };
    let same = match (meta, expect) {
        (None, None) => true,
        (Some(meta), Some(expect)) => {
            meta.is_file()
                && meta.len() == expect.size
                && meta.modified().map(nanos).unwrap_or(0) == expect.mtime
        }
        _ => false,
    };
    if same { Ok(()) } else { Err(Failure::Stale) }
}

/// The body that sends the file at `path`, counting what has gone out in `progress`.
pub fn file_body(path: &Path, progress: &Progress) -> reqwest::Body {
    match std::fs::File::open(path) {
        Ok(file) => {
            // A retried request sends the file again from the start.
            queue::set(progress, path, 0);
            let (progress, path) = (Arc::clone(progress), path.to_path_buf());
            let sent = ReaderStream::new(fs::File::from_std(file)).inspect(move |chunk| {
                if let Ok(chunk) = chunk {
                    queue::advance(&progress, &path, chunk.len() as u64);
                }
            });
            reqwest::Body::wrap_stream(sent)
        }
        Err(err) => {
            reqwest::Body::wrap_stream(stream::once(async move { Err::<Vec<u8>, io::Error>(err) }))
        }
    }
}

/// Removes the folder at `path` when it holds nothing but files the sync ignores.
fn remove_if_empty(path: &Path) -> Result<bool, Failure> {
    let mut clutter = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let ignored =
            entry.file_type()?.is_file() && entry.file_name().to_str().is_some_and(is_ignored);
        if !ignored {
            return Ok(false);
        }
        clutter.push(entry.path());
    }
    for file in clutter {
        std::fs::remove_file(file)?;
    }
    std::fs::remove_dir(path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path as url},
    };

    use super::*;

    const SESSION: &str = "0193a4a0-0000-7000-8000-000000000001";
    const NEWER: &str = "0193a4a0-0000-7000-8000-000000000002";
    const SIZE: u64 = api_client::FALLBACK_UPLOAD_CHUNK_BYTES + 10;

    fn temp(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("home-space-apply-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        path
    }

    #[test]
    fn a_part_path_fits_beside_a_name_at_the_limit() {
        assert_eq!(
            part_path(Path::new("/tmp/a.pdf")),
            PathBuf::from("/tmp/a.pdf.part")
        );
        let long = "é".repeat(120) + "xxxxxxxxxxxxxxx";
        let part = part_path(&Path::new("/tmp").join(&long));
        assert!(part.file_name().expect("name").len() <= MAX_NAME_BYTES);
        assert!(is_ignored(
            part.file_name().expect("name").to_str().expect("text")
        ));
    }

    #[test]
    fn long_names_that_start_alike_get_their_own_part_files() {
        let long = |end: &str| format!("{}{end}", "x".repeat(250));
        let dir = Path::new("/tmp");
        let (a, b) = (
            part_path(&dir.join(long("aaaaa"))),
            part_path(&dir.join(long("bbbbb"))),
        );
        assert_ne!(a, b);
        assert_eq!(a, part_path(&dir.join(long("aaaaa"))));
        assert!(a.file_name().expect("name").len() <= MAX_NAME_BYTES);
    }

    #[test]
    fn a_folder_with_only_clutter_is_removed() {
        let dir = temp("clutter");
        std::fs::write(dir.join(".DS_Store"), "x").expect("clutter");
        assert_eq!(remove_if_empty(&dir), Ok(true));
        assert!(!dir.exists());
    }

    #[test]
    fn a_folder_with_a_real_file_stays() {
        let dir = temp("real");
        std::fs::write(dir.join("a.txt"), "x").expect("file");
        assert_eq!(remove_if_empty(&dir), Ok(false));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[tokio::test]
    async fn a_file_that_changed_since_the_scan_is_stale() {
        let dir = temp("stale");
        let path = dir.join("a.txt");
        std::fs::write(&path, "x").expect("file");
        let meta = std::fs::metadata(&path).expect("meta");
        let scanned = LocalEntry {
            kind: NodeKind::File,
            size: meta.len(),
            mtime: nanos(meta.modified().expect("mtime")),
            sha256: None,
            unhashed: false,
        };
        assert_eq!(unchanged(&path, Some(&scanned)).await, Ok(()));
        assert_eq!(unchanged(&path, None).await, Err(Failure::Stale));
        std::fs::write(&path, "longer").expect("change");
        assert_eq!(unchanged(&path, Some(&scanned)).await, Err(Failure::Stale));
        assert_eq!(unchanged(&dir.join("missing"), None).await, Ok(()));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    async fn signed_in(server: &MockServer) -> Client {
        Mock::given(method("POST"))
            .and(url(protocol::paths::LOGIN))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "token", "refresh_token": "refresh", "expires_in": 900
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(url(protocol::paths::ME))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "00000000-0000-0000-0000-000000000001",
                "username": "alice",
                "lost_and_found_id": "00000000-0000-0000-0000-0000000000f0"
            })))
            .mount(server)
            .await;
        let client = Client::new(server.uri());
        client.login("alice", "pw").await.expect("login");
        client
    }

    fn session(id: &str, offset: u64) -> Value {
        json!({ "id": id, "offset": offset, "size": SIZE, "chunk_size": SIZE })
    }

    /// Answers the chunk at `offset` of upload `id` with the stored file of `sha256`.
    async fn completes(server: &MockServer, id: &str, offset: u64, sha256: &str) {
        Mock::given(method("PATCH"))
            .and(url(protocol::paths::upload(id.parse().expect("id"))))
            .and(header(protocol::paths::UPLOAD_OFFSET, offset.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "offset": SIZE,
                "node": {
                    "id": "0193a4a0-0000-7000-8000-0000000000aa", "parent_id": null,
                    "kind": "file", "name": "big.bin", "size": SIZE, "sha256": sha256,
                    "mime": "application/octet-stream", "version": 1,
                    "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
                }
            })))
            .expect(1)
            .mount(server)
            .await;
    }

    /// A big file in `dir` as the scan finds it, with its hash.
    fn big_file(dir: &Path) -> (PathBuf, LocalEntry) {
        let path = dir.join("big.bin");
        std::fs::write(&path, vec![7u8; usize::try_from(SIZE).expect("size")]).expect("file");
        let meta = std::fs::metadata(&path).expect("meta");
        let local = LocalEntry {
            kind: NodeKind::File,
            size: meta.len(),
            mtime: nanos(meta.modified().expect("mtime")),
            sha256: Some(hash_file(&path, &AtomicBool::new(false)).expect("hash")),
            unhashed: false,
        };
        (path, local)
    }

    fn saved(path: &Path, id: &str, local: &LocalEntry) -> SavedUpload {
        SavedUpload {
            path: path.to_path_buf(),
            id: id.parse().expect("id"),
            node: None,
            parent: None,
            name: "big.bin".to_owned(),
            size: local.size,
            mtime: local.mtime,
            sha256: local.sha256.clone().expect("hash"),
        }
    }

    #[tokio::test]
    async fn a_saved_upload_continues_from_the_servers_offset() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        let dir = temp("resume");
        let (path, local) = big_file(&dir);
        let store = Store::in_memory().expect("store");
        store
            .save_upload(&saved(&path, SESSION, &local))
            .expect("save");
        Mock::given(method("GET"))
            .and(url(protocol::paths::upload(SESSION.parse().expect("id"))))
            .respond_with(ResponseTemplate::new(200).set_body_json(session(SESSION, 1000)))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(url(protocol::paths::UPLOADS))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        completes(
            &server,
            SESSION,
            1000,
            local.sha256.as_deref().expect("hash"),
        )
        .await;
        let folders = Folders::from([(dir.clone(), None)]);

        let effects = upload(
            &client,
            &store,
            &folders,
            &Progress::default(),
            None,
            path.clone(),
            local,
        )
        .await
        .expect("upload");

        assert_eq!(effects.len(), 2);
        assert_eq!(store.saved_upload(&path).expect("saved"), None);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[tokio::test]
    async fn a_saved_upload_of_other_content_is_cancelled_and_started_again() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        let dir = temp("restart");
        let (path, local) = big_file(&dir);
        let store = Store::in_memory().expect("store");
        let old = SavedUpload {
            sha256: "edited since".to_owned(),
            ..saved(&path, SESSION, &local)
        };
        store.save_upload(&old).expect("save");
        Mock::given(method("DELETE"))
            .and(url(protocol::paths::upload(SESSION.parse().expect("id"))))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(url(protocol::paths::UPLOADS))
            .respond_with(ResponseTemplate::new(201).set_body_json(session(NEWER, 0)))
            .expect(1)
            .mount(&server)
            .await;
        completes(&server, NEWER, 0, local.sha256.as_deref().expect("hash")).await;
        let folders = Folders::from([(dir.clone(), None)]);

        upload(
            &client,
            &store,
            &folders,
            &Progress::default(),
            None,
            path.clone(),
            local,
        )
        .await
        .expect("upload");

        assert_eq!(store.saved_upload(&path).expect("saved"), None);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[tokio::test]
    async fn a_saved_upload_stays_while_the_server_cannot_be_reached() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        let dir = temp("unreachable");
        let (path, local) = big_file(&dir);
        let store = Store::in_memory().expect("store");
        let kept = saved(&path, SESSION, &local);
        store.save_upload(&kept).expect("save");
        Mock::given(method("GET"))
            .and(url(protocol::paths::upload(SESSION.parse().expect("id"))))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;
        let folders = Folders::from([(dir.clone(), None)]);

        let outcome = upload(
            &client,
            &store,
            &folders,
            &Progress::default(),
            None,
            path.clone(),
            local,
        )
        .await;
        let discarded = discard(&client, &store, &kept).await;

        assert!(matches!(outcome, Err(Failure::Unreachable(_))));
        assert!(matches!(discarded, Err(Failure::Unreachable(_))));
        assert_eq!(store.saved_upload(&path).expect("saved"), Some(kept));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[tokio::test]
    async fn a_discarded_upload_is_cancelled_and_forgotten() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        let store = Store::in_memory().expect("store");
        let local = LocalEntry {
            sha256: Some("x".to_owned()),
            ..LocalEntry::folder()
        };
        let gone = saved(Path::new("/sync/deleted.bin"), SESSION, &local);
        store.save_upload(&gone).expect("save");
        Mock::given(method("DELETE"))
            .and(url(protocol::paths::upload(SESSION.parse().expect("id"))))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        discard(&client, &store, &gone).await.expect("discard");

        assert_eq!(store.saved_uploads().expect("saved"), []);
    }

    #[test]
    fn place_finds_the_remote_parent_and_name() {
        let folders = Folders::from([(PathBuf::from("/sync"), None)]);
        assert_eq!(
            place(&folders, Path::new("/sync/a.txt")),
            Ok((None, "a.txt".to_owned()))
        );
        assert!(place(&folders, Path::new("/sync/new/a.txt")).is_err());
    }
}
