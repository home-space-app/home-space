use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use iced::Task;

use api_client::Client;
use protocol::{Node, NodeId, NodeKind};

use client_core::format::sanitize_component;
use sync_core::{Base, hash_file};

use super::Message;
use crate::transfers::{self, DownloadFile, UploadPlan};

const TEMP_FOLDER: &str = "home-space";

/// Asks for files to upload into `parent` and plans their transfer.
pub fn pick_upload(parent: Option<NodeId>) -> Task<Message> {
    Task::perform(
        async move {
            match rfd::AsyncFileDialog::new().pick_files().await {
                Some(handles) => {
                    let paths = handles
                        .iter()
                        .map(|handle| handle.path().to_path_buf())
                        .collect();
                    transfers::plan_upload(paths).await
                }
                None => Ok(UploadPlan::default()),
            }
        },
        move |result| Message::UploadPlanned(parent, result),
    )
}

/// Plans the upload of dropped `paths` into `parent`.
pub fn plan_upload(parent: Option<NodeId>, paths: Vec<PathBuf>) -> Task<Message> {
    Task::perform(transfers::plan_upload(paths), move |result| {
        Message::UploadPlanned(parent, result)
    })
}

/// Asks where to save `nodes` and plans their download.
pub fn download(client: &Client, nodes: Vec<Node>) -> Task<Message> {
    match nodes.as_slice() {
        [node] if node.kind == NodeKind::File => {
            let node = node.clone();
            Task::perform(
                async move {
                    let handle = rfd::AsyncFileDialog::new()
                        .set_file_name(&node.name)
                        .save_file()
                        .await;
                    Ok(handle
                        .map(|handle| {
                            vec![DownloadFile::replacing(&node, handle.path().to_path_buf())]
                        })
                        .unwrap_or_default())
                },
                Message::DownloadPlanned,
            )
        }
        _ => {
            let client = client.clone();
            Task::perform(
                async move {
                    match rfd::AsyncFileDialog::new().pick_folder().await {
                        Some(folder) => {
                            transfers::plan_download(client, nodes, folder.path().to_path_buf())
                                .await
                        }
                        None => Ok(Vec::new()),
                    }
                },
                Message::DownloadPlanned,
            )
        }
    }
}

/// Where `node` is downloaded to be opened: a folder of its own in the system's temp folder, so
/// the file keeps its name and the system picks the app by its extension.
pub fn temp_target(node: &Node) -> PathBuf {
    std::env::temp_dir()
        .join(TEMP_FOLDER)
        .join(node.id.0.to_string())
        .join(sanitize_component(&node.name))
}

/// The copy of a file in a synced folder, on disk.
#[derive(Debug, Clone)]
pub struct SyncedCopy {
    pub path: PathBuf,
    /// What was last agreed with the server about the file, if anything.
    pub base: Option<Base>,
}

impl SyncedCopy {
    /// Whether the server is known to have moved on from this copy. A copy with edits, or one
    /// nothing is recorded about, is left for the sync to sort out and still counts as current.
    fn is_behind(&self, node: &Node) -> bool {
        self.base
            .as_ref()
            .filter(|base| base.path == self.path && !base.is_edited())
            .is_some_and(|base| match (&base.sha256, &node.sha256) {
                (Some(ours), Some(theirs)) => ours != theirs,
                _ => false,
            })
    }
}

/// How a file gets opened.
#[derive(Debug)]
enum Opening {
    Local(PathBuf),
    Download(DownloadFile),
}

/// Opens the file `node` with the default app: from a local copy that is the server's version,
/// or else from a download to the temp folder.
pub fn open(client: &Client, node: Node, synced: Option<SyncedCopy>) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move {
            match plan_open(&client, node, synced).await? {
                Opening::Local(path) => transfers::open_file(&path).map(|()| Vec::new()),
                Opening::Download(file) => Ok(vec![file]),
            }
        },
        Message::DownloadPlanned,
    )
}

/// Asks the server for the current version of `node` and picks what to open. While the server
/// can't be reached, any copy will do. The synced folder belongs to the sync, so a newer version
/// goes to the temp folder and the sync updates the synced copy itself.
async fn plan_open(
    client: &Client,
    node: Node,
    synced: Option<SyncedCopy>,
) -> Result<Opening, String> {
    let current = match client.get(node.id).await {
        Ok(current) => current,
        Err(err) if err.is_unreachable() => {
            let temp = temp_target(&node);
            return match synced {
                Some(copy) => Ok(Opening::Local(copy.path)),
                None if temp.is_file() => Ok(Opening::Local(temp)),
                None => Err(err.to_string()),
            };
        }
        Err(err) => return Err(err.to_string()),
    };
    if let Some(copy) = synced.filter(|copy| !copy.is_behind(&current)) {
        return Ok(Opening::Local(copy.path));
    }
    let target = temp_target(&current);
    if holds_version(&target, &current).await {
        return Ok(Opening::Local(target));
    }
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|err| format!("{}: {err}", parent.display()))?;
    }
    Ok(Opening::Download(DownloadFile::opening(&current, target)))
}

/// Whether the file at `path` has the content of `node`.
async fn holds_version(path: &Path, node: &Node) -> bool {
    let Some(sha256) = node.sha256.clone() else {
        return false;
    };
    if !path.is_file() {
        return false;
    }
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || hash_file(&path, &AtomicBool::new(false)))
        .await
        .is_ok_and(|hash| hash.is_ok_and(|hash| hash == sha256))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use protocol::paths;
    use serde_json::json;
    use sync_core::nanos;
    use time::OffsetDateTime;
    use uuid::Uuid;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path as url},
    };

    use super::*;

    /// A file node of its own, so that its temp folder is too.
    fn file(sha256: Option<&str>) -> Node {
        Node {
            id: NodeId(Uuid::new_v4()),
            parent_id: None,
            kind: NodeKind::File,
            name: "photo.jpg".to_owned(),
            size: 5,
            sha256: sha256.map(str::to_owned),
            mime: String::new(),
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    fn folder() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("home-space-open-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("folder");
        dir
    }

    fn sha(path: &Path) -> String {
        hash_file(path, &AtomicBool::new(false)).expect("hash")
    }

    /// A file with `content`, and the base saying it is what was agreed with the server.
    fn synced(node: &Node, content: &str) -> SyncedCopy {
        let path = folder().join(&node.name);
        fs::write(&path, content).expect("file");
        let meta = fs::metadata(&path).expect("meta");
        let base = Base {
            node: node.id,
            path: path.clone(),
            kind: NodeKind::File,
            sha256: Some(sha(&path)),
            size: meta.len(),
            mtime: nanos(meta.modified().expect("mtime")),
        };
        SyncedCopy {
            path,
            base: Some(base),
        }
    }

    fn put_in_temp(node: &Node, content: &str) -> PathBuf {
        let path = temp_target(node);
        fs::create_dir_all(path.parent().expect("parent")).expect("folder");
        fs::write(&path, content).expect("file");
        path
    }

    /// A server the client is signed in to, which runs until the returned handle is dropped.
    async fn signed_in() -> (MockServer, Client) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(url(paths::LOGIN))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "token", "refresh_token": "refresh", "expires_in": 900
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(url(paths::ME))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "00000000-0000-0000-0000-000000000001",
                "username": "alice",
                "lost_and_found_id": "00000000-0000-0000-0000-0000000000f0"
            })))
            .mount(&server)
            .await;
        let client = Client::new(server.uri());
        client.login("alice", "pw").await.expect("login");
        (server, client)
    }

    async fn server_with(node: &Node) -> (MockServer, Client) {
        let (server, client) = signed_in().await;
        Mock::given(method("GET"))
            .and(url(paths::node(node.id)))
            .respond_with(ResponseTemplate::new(200).set_body_json(node))
            .mount(&server)
            .await;
        (server, client)
    }

    /// A server behind a gateway that can't reach it.
    async fn unreachable_server(node: &Node) -> (MockServer, Client) {
        let (server, client) = signed_in().await;
        Mock::given(method("GET"))
            .and(url(paths::node(node.id)))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;
        (server, client)
    }

    fn assert_local(opening: Result<Opening, String>, path: &Path) {
        match opening {
            Ok(Opening::Local(opened)) => assert_eq!(opened, path),
            other => panic!("expected {}, got {other:?}", path.display()),
        }
    }

    fn assert_download(opening: Result<Opening, String>) {
        assert!(
            matches!(opening, Ok(Opening::Download(_))),
            "expected a download, got {opening:?}"
        );
    }

    #[tokio::test]
    async fn a_synced_copy_of_the_servers_version_opens_directly() {
        let mut node = file(None);
        let copy = synced(&node, "hello");
        node.sha256 = copy.base.as_ref().and_then(|base| base.sha256.clone());
        let (_server, client) = server_with(&node).await;

        let path = copy.path.clone();
        assert_local(plan_open(&client, node, Some(copy)).await, &path);
    }

    #[tokio::test]
    async fn a_synced_copy_behind_the_server_is_downloaded_to_temp() {
        let node = file(Some("newer"));
        let copy = synced(&node, "hello");
        let (_server, client) = server_with(&node).await;

        let before = fs::read(&copy.path).expect("copy");
        let path = copy.path.clone();
        assert_download(plan_open(&client, node, Some(copy)).await);
        assert_eq!(fs::read(&path).expect("copy"), before);
    }

    #[tokio::test]
    async fn a_synced_copy_with_edits_opens_directly() {
        let node = file(Some("newer"));
        let copy = synced(&node, "hello");
        fs::write(&copy.path, "edited here").expect("edit");
        let (_server, client) = server_with(&node).await;

        let path = copy.path.clone();
        assert_local(plan_open(&client, node, Some(copy)).await, &path);
    }

    #[tokio::test]
    async fn a_synced_copy_with_no_base_opens_directly() {
        let node = file(Some("newer"));
        let mut copy = synced(&node, "hello");
        copy.base = None;
        let (_server, client) = server_with(&node).await;

        let path = copy.path.clone();
        assert_local(plan_open(&client, node, Some(copy)).await, &path);
    }

    #[tokio::test]
    async fn a_current_temp_copy_opens_without_a_download() {
        let mut node = file(None);
        let temp = put_in_temp(&node, "hello");
        node.sha256 = Some(sha(&temp));
        let (_server, client) = server_with(&node).await;

        assert_local(plan_open(&client, node, None).await, &temp);
    }

    #[tokio::test]
    async fn a_stale_temp_copy_is_downloaded_again() {
        let node = file(Some("newer"));
        put_in_temp(&node, "hello");
        let (_server, client) = server_with(&node).await;

        assert_download(plan_open(&client, node, None).await);
    }

    #[tokio::test]
    async fn a_file_with_no_copy_is_downloaded() {
        let node = file(Some("newer"));
        let (_server, client) = server_with(&node).await;

        assert_download(plan_open(&client, node, None).await);
    }

    #[tokio::test]
    async fn any_copy_opens_while_the_server_cannot_be_reached() {
        let node = file(Some("newer"));
        let (_server, client) = unreachable_server(&node).await;

        let copy = synced(&node, "hello");
        let path = copy.path.clone();
        assert_local(plan_open(&client, node.clone(), Some(copy)).await, &path);

        let temp = put_in_temp(&node, "hello");
        assert_local(plan_open(&client, node, None).await, &temp);
    }

    #[tokio::test]
    async fn no_copy_and_no_server_is_an_error() {
        let node = file(Some("newer"));
        let (_server, client) = unreachable_server(&node).await;

        let opening = plan_open(&client, node, None).await;
        assert!(opening.is_err(), "got {opening:?}");
    }
}
