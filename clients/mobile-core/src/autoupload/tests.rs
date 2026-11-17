//! A pass against a fake of the server's upload API. How the server itself treats the flags is
//! tested in `server/crates/api/tests/uploads.rs`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

use super::run::TARGET_GONE;
use super::*;
use crate::{CoreClient, DeviceIdentity, SessionListener};

const TARGET: &str = "0193a4a0-0000-7000-8000-00000000000a";
const UPLOAD: &str = "0193a4a0-0000-7000-8000-0000000000bb";
const CHUNK: usize = 4;

/// One upload the fake was asked to start.
#[derive(Clone, Debug)]
struct Started {
    body: Value,
    received: Vec<u8>,
    cancelled: bool,
}

#[derive(Default)]
struct Fake {
    uploads: Vec<Started>,
    /// Folders made, as (parent, name).
    made: Vec<(Option<String>, String)>,
    /// The name to give the next stored file, as `keep_both` might.
    rename_to: Option<String>,
    target_gone: bool,
    /// Answer everything with this, like a gateway whose server is down.
    down: bool,
    requests: usize,
}

fn node(id: &str, name: &str, kind: &str) -> Value {
    json!({
        "id": id, "parent_id": null, "kind": kind, "name": name, "size": 1, "sha256": null,
        "mime": "application/octet-stream", "version": 1,
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
    })
}

fn not_found() -> ResponseTemplate {
    ResponseTemplate::new(404).set_body_json(json!({ "error": "not_found", "message": "gone" }))
}

impl Fake {
    fn respond(&mut self, request: &Request) -> ResponseTemplate {
        let path = request.url.path().to_owned();
        let method = request.method.as_str().to_owned();
        if path.ends_with(protocol::paths::LOGIN) {
            return ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "access", "refresh_token": "refresh", "expires_in": 900,
            }));
        }
        if path.ends_with(protocol::paths::ME) {
            return ResponseTemplate::new(200).set_body_json(json!({
                "id": "00000000-0000-0000-0000-000000000001", "username": "me",
                "lost_and_found_id": "00000000-0000-0000-0000-000000000002",
            }));
        }
        self.requests += 1;
        if self.down {
            return ResponseTemplate::new(502).set_body_string("<html>bad gateway</html>");
        }
        if path.ends_with(&format!("/nodes/{TARGET}")) {
            return if self.target_gone {
                not_found()
            } else {
                ResponseTemplate::new(200).set_body_json(node(TARGET, "Photos", "folder"))
            };
        }
        if path.ends_with("/folders") {
            let body: Value = serde_json::from_slice(&request.body).expect("body");
            let name = body["name"].as_str().expect("name").to_owned();
            let parent = body["parent_id"].as_str().map(str::to_owned);
            self.made.push((parent, name.clone()));
            let id = format!("0193a4a0-0000-7000-8000-{:012x}", 0x100 + self.made.len());
            return ResponseTemplate::new(201).set_body_json(node(&id, &name, "folder"));
        }
        if path.ends_with("/uploads") && method == "POST" {
            let body: Value = serde_json::from_slice(&request.body).expect("body");
            let size = body["size"].as_u64().expect("size");
            self.uploads.push(Started {
                body,
                received: Vec::new(),
                cancelled: false,
            });
            return ResponseTemplate::new(201).set_body_json(json!({
                "id": self.upload_id(self.uploads.len() - 1), "offset": 0, "size": size,
                "chunk_size": CHUNK,
            }));
        }
        if let Some(index) = self.upload_index(&path) {
            return self.on_upload(index, &method, request);
        }
        not_found()
    }

    fn upload_id(&self, index: usize) -> String {
        format!("{}{:x}", &UPLOAD[..UPLOAD.len() - 1], index)
    }

    fn upload_index(&self, path: &str) -> Option<usize> {
        (0..self.uploads.len()).find(|index| path.ends_with(&self.upload_id(*index)))
    }

    fn on_upload(&mut self, index: usize, method: &str, request: &Request) -> ResponseTemplate {
        let id = self.upload_id(index);
        let rename = self.rename_to.clone();
        let upload = &mut self.uploads[index];
        let size = upload.body["size"].as_u64().expect("size");
        match method {
            "GET" => ResponseTemplate::new(200).set_body_json(json!({
                "id": id, "offset": upload.received.len(), "size": size, "chunk_size": CHUNK,
            })),
            "DELETE" => {
                upload.cancelled = true;
                ResponseTemplate::new(204)
            }
            _ => {
                upload.received.extend_from_slice(&request.body);
                let offset = upload.received.len() as u64;
                let name = rename
                    .unwrap_or_else(|| upload.body["name"].as_str().expect("name").to_owned());
                let stored = (offset == size)
                    .then(|| node("0193a4a0-0000-7000-8000-0000000000cc", &name, "file"));
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "offset": offset, "node": stored }))
            }
        }
    }
}

type ReadHook = Box<dyn Fn(usize, &Sources) + Send + Sync>;

#[derive(Default)]
struct Sources {
    files: Mutex<HashMap<String, (LocalItem, Vec<u8>)>>,
    reads: AtomicUsize,
    /// Called before each read with how many came before it.
    on_read: Mutex<Option<ReadHook>>,
    list_error: Mutex<Option<String>>,
    /// Full listings asked for.
    listings: AtomicUsize,
    /// Called while listing.
    on_list: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    changes: Mutex<ChangeFeed>,
}

impl Sources {
    fn put(&self, key: &str, dir: &str, content: &[u8], age: Duration) {
        let item = LocalItem {
            key: key.to_owned(),
            relative_dir: dir.to_owned(),
            name: format!("{key}.jpg"),
            size: content.len() as u64,
            modified_at: SystemTime::now() - age,
            mime: Some("image/jpeg".to_owned()),
        };
        self.files
            .lock()
            .unwrap()
            .insert(key.to_owned(), (item, content.to_vec()));
        self.changes.lock().unwrap().touch(key);
    }

    /// Changes a file behind the platform's back: its change feed doesn't mention it.
    fn put_unreported(&self, key: &str, content: &[u8]) {
        let before = self.changes.lock().unwrap().touched.clone();
        self.put(key, "", content, OLD);
        self.changes.lock().unwrap().touched = before;
    }
}

/// What the fake platform reports about changes.
#[derive(Default)]
struct ChangeFeed {
    enabled: bool,
    expired: bool,
    generation: u64,
    touched: HashMap<String, u64>,
}

impl ChangeFeed {
    fn touch(&mut self, key: &str) {
        self.generation += 1;
        self.touched.insert(key.to_owned(), self.generation);
    }
}

const OLD: Duration = Duration::from_secs(60);

impl LocalSources for Sources {
    fn change_token(&self, _: SourceKind, _: String) -> Result<Option<String>, CoreError> {
        let reports = self.changes.lock().unwrap();
        Ok(reports.enabled.then(|| reports.generation.to_string()))
    }

    fn changes_since(
        &self,
        _: SourceKind,
        _: String,
        token: String,
    ) -> Result<Option<LocalChanges>, CoreError> {
        let reports = self.changes.lock().unwrap();
        if !reports.enabled || reports.expired {
            return Ok(None);
        }
        let since: u64 = token.parse().expect("token");
        let files = self.files.lock().unwrap();
        let mut items: Vec<LocalItem> = reports
            .touched
            .iter()
            .filter(|(_, generation)| **generation > since)
            .filter_map(|(key, _)| files.get(key).map(|(item, _)| item.clone()))
            .collect();
        items.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(Some(LocalChanges {
            items,
            token: reports.generation.to_string(),
        }))
    }

    fn list(&self, _: SourceKind, _: String) -> Result<Vec<LocalItem>, CoreError> {
        self.listings.fetch_add(1, Ordering::SeqCst);
        if let Some(on_list) = self.on_list.lock().unwrap().take() {
            on_list();
        }
        if let Some(detail) = self.list_error.lock().unwrap().clone() {
            return Err(CoreError::Failed {
                code: "permission".to_owned(),
                detail,
            });
        }
        let mut items: Vec<LocalItem> = self
            .files
            .lock()
            .unwrap()
            .values()
            .map(|(item, _)| item.clone())
            .collect();
        items.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(items)
    }

    fn stat(&self, _: SourceKind, _: String, key: String) -> Result<Option<LocalItem>, CoreError> {
        Ok(self
            .files
            .lock()
            .unwrap()
            .get(&key)
            .map(|(item, _)| item.clone()))
    }

    fn read(
        &self,
        _: SourceKind,
        _: String,
        key: String,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>, CoreError> {
        let count = self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some(hook) = self.on_read.lock().unwrap().as_ref() {
            hook(count, self);
        }
        let files = self.files.lock().unwrap();
        let (_, content) = files.get(&key).expect("file");
        let start = (offset as usize).min(content.len());
        let end = (start + len as usize).min(content.len());
        Ok(content[start..end].to_vec())
    }
}

struct Quiet;

impl AutoUploadListener for Quiet {
    fn changed(&self) {}
}

impl SessionListener for Quiet {
    fn refresh_token_changed(&self, _: Option<String>) {}
}

struct World {
    _server: MockServer,
    fake: Arc<Mutex<Fake>>,
    sources: Arc<Sources>,
    auto: Arc<AutoUpload>,
    _client: Arc<CoreClient>,
    seen: Arc<Seen>,
}

async fn world() -> World {
    let server = MockServer::start().await;
    let fake = Arc::new(Mutex::new(Fake::default()));
    let responder = fake.clone();
    Mock::given(any())
        .respond_with(move |request: &Request| responder.lock().unwrap().respond(request))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().expect("dir").keep();
    let device = DeviceIdentity {
        id: "0193a4a0-0000-7000-8000-0000000000dd".to_owned(),
        name: "Pixel".to_owned(),
    };
    let client = Arc::new(
        CoreClient::with_session(
            server.uri(),
            device,
            Arc::new(Quiet),
            dir.join("files").to_string_lossy().into_owned(),
        )
        .expect("client"),
    );
    client
        .login("me".to_owned(), "pw".to_owned())
        .await
        .expect("login");
    let sources = Arc::new(Sources::default());
    let seen = Arc::new(Seen::default());
    let auto = AutoUpload::open(
        client.clone(),
        dir.join("sync").to_string_lossy().into_owned(),
        sources.clone(),
        seen.clone(),
    )
    .expect("open");
    seen.auto.set(Arc::downgrade(&auto)).expect("once");
    fake.lock().unwrap().requests = 0;
    World {
        _server: server,
        fake,
        sources,
        auto,
        seen,
        _client: client,
    }
}

/// What [`AutoUpload::current`] said each time the listener heard of a change.
#[derive(Default)]
struct Seen {
    auto: std::sync::OnceLock<std::sync::Weak<AutoUpload>>,
    current: Mutex<Vec<CurrentUpload>>,
}

impl AutoUploadListener for Seen {
    fn changed(&self) {
        let auto = self.auto.get().and_then(std::sync::Weak::upgrade);
        if let Some(current) = auto.and_then(|auto| auto.current()) {
            self.current.lock().unwrap().push(current);
        }
    }
}

impl World {
    fn add(&self, upload_existing: bool) -> UploadSource {
        self.auto
            .add_source(
                SourceKind::Folder,
                "handle".to_owned(),
                "Camera".to_owned(),
                Some(TARGET.to_owned()),
                "Photos".to_owned(),
                upload_existing,
            )
            .expect("add")
    }

    fn uploads(&self) -> Vec<Started> {
        self.fake.lock().unwrap().uploads.clone()
    }

    fn source(&self) -> UploadSource {
        self.auto.sources().expect("sources").remove(0)
    }
}

#[tokio::test]
async fn settings_default_to_wifi_only_and_no_background() {
    let w = world().await;
    assert_eq!(
        w.auto.settings().unwrap(),
        SyncSettings {
            wifi_only: true,
            background: false,
        }
    );
    let changed = SyncSettings {
        wifi_only: false,
        background: true,
    };
    w.auto.set_settings(changed).unwrap();
    assert_eq!(w.auto.settings().unwrap(), changed);
}

#[tokio::test]
async fn a_new_file_goes_up_as_an_auto_upload_that_keeps_both() {
    let w = world().await;
    w.add(true);
    w.sources.put("a", "", b"hello world", OLD);

    let summary = w.auto.run_pass(false).await.expect("pass");

    assert_eq!(summary.uploaded, 1);
    let uploads = w.uploads();
    assert_eq!(uploads[0].received, b"hello world");
    let body = &uploads[0].body;
    assert_eq!(body["name"], "a.jpg");
    assert_eq!(body["parent_id"], TARGET);
    assert_eq!(body["keep_both"], true);
    assert_eq!(body["auto_upload"], true);
    assert_eq!(body["mime"], "image/jpeg");
    assert!(body["modified_at"].is_string());
    assert!(w.source().last_synced_at.is_some());
}

#[tokio::test]
async fn an_unchanged_file_is_not_sent_again() {
    let w = world().await;
    w.add(true);
    w.sources.put("a", "", b"hello", OLD);
    w.auto.run_pass(false).await.expect("first");

    let again = w.auto.run_pass(false).await.expect("second");

    assert_eq!(again.uploaded, 0);
    assert_eq!(w.uploads().len(), 1);
}

#[tokio::test]
async fn an_edit_goes_to_the_name_the_server_gave_and_may_become_a_version() {
    let w = world().await;
    w.add(true);
    w.sources.put("a", "", b"first", OLD);
    w.fake.lock().unwrap().rename_to = Some("a (1).jpg".to_owned());
    w.auto.run_pass(false).await.expect("first");

    w.sources
        .put("a", "", b"edited", OLD - Duration::from_secs(1));
    let summary = w.auto.run_pass(false).await.expect("second");

    assert_eq!(summary.uploaded, 1);
    let edit = &w.uploads()[1];
    assert_eq!(edit.body["name"], "a (1).jpg");
    assert_eq!(edit.received, b"edited");
    // The server decides: a new version only if that file still has the name.
    assert_eq!(edit.body["keep_both"], true);
    assert_eq!(
        edit.body["version_of"],
        "0193a4a0-0000-7000-8000-0000000000cc"
    );
    assert_eq!(edit.body["auto_upload"], true);
}

#[tokio::test]
async fn existing_files_can_be_left_out_until_they_change() {
    let w = world().await;
    w.add(false);
    w.sources.put("old", "", b"old", OLD);

    let first = w.auto.run_pass(false).await.expect("first");
    assert_eq!(first.uploaded, 0);
    assert!(w.uploads().is_empty());

    w.sources.put("new", "", b"new", OLD);
    w.sources
        .put("old", "", b"edited", OLD - Duration::from_secs(1));
    let second = w.auto.run_pass(false).await.expect("second");
    assert_eq!(second.uploaded, 2);
    let names: Vec<_> = w.uploads().iter().map(|u| u.body["name"].clone()).collect();
    assert!(names.contains(&json!("new.jpg")));
    assert!(names.contains(&json!("old.jpg")));
}

#[tokio::test]
async fn a_first_pass_that_stops_early_still_leaves_existing_files_out() {
    let w = world().await;
    w.add(false);
    w.sources.put("a", "", b"aa", OLD);
    w.sources.put("b", "", b"bb", OLD);
    let auto = w.auto.clone();
    *w.sources.on_list.lock().unwrap() = Some(Box::new(move || auto.stop()));

    w.auto.run_pass(false).await.expect("stopped");

    let next = w.auto.run_pass(false).await.expect("next");
    assert_eq!(next.uploaded, 0);
    assert!(w.uploads().is_empty());
}

#[tokio::test]
async fn subfolders_are_made_once_below_the_target() {
    let w = world().await;
    w.add(true);
    w.sources.put("a", "2026/05", b"aa", OLD);
    w.sources.put("b", "2026/05", b"bb", OLD);

    w.auto.run_pass(false).await.expect("pass");

    let made = w.fake.lock().unwrap().made.clone();
    assert_eq!(made.len(), 2, "{made:?}");
    assert_eq!(made[0], (Some(TARGET.to_owned()), "2026".to_owned()));
    assert_eq!(made[1].1, "05");
    assert!(w.uploads()[0].body["parent_id"] != TARGET);
}

#[tokio::test]
async fn stopping_takes_effect_between_chunks_and_the_next_pass_continues() {
    let w = world().await;
    w.add(true);
    w.sources.put("a", "", b"0123456789", OLD);
    let auto = w.auto.clone();
    *w.sources.on_read.lock().unwrap() = Some(Box::new(move |count, _| {
        if count == 0 {
            auto.stop();
        }
    }));

    // The read that was under way when told to stop still goes; the next one doesn't.
    let stopped = w.auto.run_pass(false).await.expect("stopped");
    assert!(stopped.stopped);
    assert_eq!(stopped.uploaded, 0);
    assert_eq!(w.uploads()[0].received, b"0123");

    *w.sources.on_read.lock().unwrap() = None;
    let resumed = w.auto.run_pass(false).await.expect("resumed");
    assert_eq!(resumed.uploaded, 1);
    let uploads = w.uploads();
    assert_eq!(uploads.len(), 1, "it continued instead of starting over");
    assert_eq!(uploads[0].received, b"0123456789");
}

#[tokio::test]
async fn a_file_changed_during_its_upload_is_not_stored() {
    let w = world().await;
    w.add(true);
    w.sources.put("a", "", b"0123456789", OLD);
    *w.sources.on_read.lock().unwrap() = Some(Box::new(|count, sources| {
        if count == 0 {
            sources.put("a", "", b"different!", OLD - Duration::from_secs(1));
        }
    }));

    let summary = w.auto.run_pass(false).await.expect("pass");

    assert_eq!(summary.uploaded, 0);
    assert_eq!(summary.failed, 1);
    assert!(w.uploads()[0].cancelled);
    assert_eq!(w.source().pending, 1);
    assert_eq!(w.source().last_synced_at, None);
}

#[tokio::test]
async fn a_file_changed_right_before_its_last_chunk_is_read_is_not_stored() {
    let w = world().await;
    w.add(true);
    w.sources.put("a", "", b"0123", OLD);
    *w.sources.on_read.lock().unwrap() = Some(Box::new(|count, sources| {
        if count == 0 {
            sources.put("a", "", b"4567", OLD - Duration::from_secs(1));
        }
    }));

    let summary = w.auto.run_pass(false).await.expect("pass");

    assert_eq!((summary.uploaded, summary.failed), (0, 1));
    assert!(w.uploads()[0].cancelled);
}

#[test]
fn records_from_before_the_id_was_kept_stay_and_have_no_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auto.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE records (
                source      TEXT NOT NULL,
                key         TEXT NOT NULL,
                size        INTEGER NOT NULL,
                mtime       INTEGER NOT NULL,
                remote_name TEXT,
                remote_dir  TEXT NOT NULL,
                PRIMARY KEY (source, key)
            );
            INSERT INTO records VALUES ('s', 'a', 1, 2, 'a.jpg', '2026');",
        )
        .unwrap();

    let records = super::store::Store::open(&path)
        .unwrap()
        .records("s")
        .unwrap();

    assert_eq!(records["a"].remote_name.as_deref(), Some("a.jpg"));
    assert_eq!(records["a"].remote_node, None);
}

#[tokio::test]
async fn a_deleted_target_pauses_the_source_until_another_is_chosen() {
    let w = world().await;
    let source = w.add(true);
    w.sources.put("a", "", b"data", OLD);
    w.fake.lock().unwrap().target_gone = true;

    let summary = w.auto.run_pass(false).await.expect("pass");

    assert_eq!(summary.uploaded, 0);
    assert!(w.uploads().is_empty());
    assert_eq!(w.source().problem.as_deref(), Some(TARGET_GONE));
    assert!(
        w.fake.lock().unwrap().made.is_empty(),
        "not silently made again"
    );

    w.auto
        .set_source_target(source.id, None, "Home".to_owned())
        .expect("target");
    assert_eq!(w.source().problem, None);
    let after = w.auto.run_pass(false).await.expect("after");
    assert_eq!(after.uploaded, 1);
    assert_eq!(w.uploads()[0].body.get("parent_id"), None);
}

#[tokio::test]
async fn changing_what_a_source_covers_keeps_what_was_sent() {
    let w = world().await;
    let source = w.add(true);
    w.sources.put("a", "", b"data", OLD);
    w.auto.run_pass(false).await.expect("first");

    w.auto
        .set_source_handle(source.id, "albums".to_owned(), "Albums".to_owned())
        .expect("handle");
    w.sources.put("b", "WhatsApp Images", b"more", OLD);
    let after = w.auto.run_pass(false).await.expect("after");

    let changed = w.source();
    assert_eq!(changed.handle, "albums");
    assert_eq!(changed.label, "Albums");
    assert_eq!(after.uploaded, 1, "only the file it didn't cover before");
    assert_eq!(w.uploads().len(), 2);
}

#[tokio::test]
async fn a_source_that_cant_be_read_reports_it_and_recovers() {
    let w = world().await;
    w.add(true);
    w.sources.put("a", "", b"data", OLD);
    *w.sources.list_error.lock().unwrap() = Some("permission revoked".to_owned());

    w.auto.run_pass(false).await.expect("pass");
    assert_eq!(w.source().problem.as_deref(), Some("permission revoked"));

    *w.sources.list_error.lock().unwrap() = None;
    let after = w.auto.run_pass(false).await.expect("after");
    assert_eq!(after.uploaded, 1);
    assert_eq!(w.source().problem, None);
}

#[tokio::test]
async fn an_unreachable_server_is_one_outage_that_waits() {
    let w = world().await;
    w.add(true);
    w.sources.put("a", "", b"data", OLD);
    w.fake.lock().unwrap().down = true;

    let summary = w.auto.run_pass(false).await.expect("pass");
    assert!(summary.outage);
    assert_eq!(summary.failed, 0, "not reported per file");

    let before = w.fake.lock().unwrap().requests;
    let waiting = w.auto.run_pass(false).await.expect("waiting");
    assert!(waiting.outage);
    assert_eq!(w.fake.lock().unwrap().requests, before, "no request sent");

    w.fake.lock().unwrap().down = false;
    let retried = w.auto.run_pass(true).await.expect("retried");
    assert_eq!(retried.uploaded, 1);
}

#[tokio::test]
async fn removing_a_source_cancels_its_upload_and_wipe_forgets_everything() {
    let w = world().await;
    let source = w.add(true);
    w.sources.put("a", "", b"0123456789", OLD);
    let auto = w.auto.clone();
    *w.sources.on_read.lock().unwrap() = Some(Box::new(move |count, _| {
        if count == 0 {
            auto.stop();
        }
    }));
    w.auto.run_pass(false).await.expect("stopped");

    w.auto.remove_source(source.id).await.expect("remove");
    assert!(w.uploads()[0].cancelled);
    assert!(w.auto.sources().unwrap().is_empty());

    w.add(true);
    w.auto.wipe().await.expect("wipe");
    assert!(w.auto.sources().unwrap().is_empty());
    assert!(w.auto.settings().unwrap().wifi_only);
}

impl World {
    fn report_changes(&self) {
        self.sources.changes.lock().unwrap().enabled = true;
    }

    fn listings(&self) -> usize {
        self.sources.listings.load(Ordering::SeqCst)
    }

    fn names(&self) -> Vec<String> {
        self.uploads()
            .iter()
            .map(|upload| upload.body["name"].as_str().unwrap().to_owned())
            .collect()
    }

    async fn pass(&self) -> PassSummary {
        self.auto.run_pass(true).await.unwrap()
    }
}

#[tokio::test]
async fn a_source_that_reports_changes_is_listed_once_and_then_asked() {
    let w = world().await;
    w.report_changes();
    w.sources.put("a", "", b"aaa", OLD);
    w.add(true);

    w.pass().await;
    assert_eq!((w.listings(), w.names()), (1, vec!["a.jpg".to_owned()]));

    w.sources.put("b", "", b"bbb", OLD);
    w.pass().await;
    w.pass().await;
    assert_eq!(w.listings(), 1);
    assert_eq!(w.names(), ["a.jpg", "b.jpg"]);
}

#[tokio::test]
async fn a_changed_file_in_the_answer_goes_to_its_recorded_place() {
    let w = world().await;
    w.report_changes();
    w.sources.put("a", "", b"aaa", OLD);
    w.add(true);
    w.pass().await;

    w.sources.put("a", "", b"changed", OLD);
    w.pass().await;

    let uploads = w.uploads();
    assert_eq!((uploads.len(), w.listings()), (2, 1));
    assert_eq!(uploads[1].received, b"changed");
    assert!(uploads[1].body["version_of"].is_string());
}

#[tokio::test]
async fn a_source_that_cant_report_changes_is_listed_every_time() {
    let w = world().await;
    w.sources.put("a", "", b"aaa", OLD);
    w.add(true);
    w.pass().await;
    w.pass().await;
    assert_eq!(w.listings(), 2);
    assert!(w.auto.store.change_token(&w.source().id).unwrap().is_none());
}

#[tokio::test]
async fn a_file_held_back_comes_again_without_a_new_change() {
    let w = world().await;
    w.report_changes();
    w.add(true);
    w.pass().await;

    // Still being written, so it waits. Nothing changes after that, and it must not be forgotten.
    w.sources.put("a", "", b"aaa", Duration::ZERO);
    assert_eq!(w.pass().await.uploaded, 0);
    let token = w
        .auto
        .store
        .change_token(&w.source().id)
        .unwrap()
        .unwrap()
        .0;
    w.sources.put_unreported("a", b"aaa");
    w.sources.changes.lock().unwrap().generation += 1;

    assert_eq!(w.pass().await.uploaded, 1);
    assert_eq!(w.listings(), 1);
    assert_ne!(
        w.auto
            .store
            .change_token(&w.source().id)
            .unwrap()
            .unwrap()
            .0,
        token
    );
}

#[tokio::test]
async fn a_first_listing_that_doesnt_finish_is_listed_again() {
    let w = world().await;
    w.report_changes();
    w.sources.put("a", "", b"aaa", OLD);
    w.sources.put("b", "", b"bbb", Duration::ZERO);
    w.add(true);

    w.pass().await;
    assert!(w.auto.store.change_token(&w.source().id).unwrap().is_none());

    w.sources.put_unreported("b", b"bbb");
    w.pass().await;
    assert_eq!(w.listings(), 2);
    assert_eq!(w.names(), ["a.jpg", "b.jpg"]);
    assert!(w.auto.store.change_token(&w.source().id).unwrap().is_some());
}

#[tokio::test]
async fn an_expired_token_falls_back_to_a_listing() {
    let w = world().await;
    w.report_changes();
    w.add(true);
    w.pass().await;

    w.sources.put_unreported("a", b"aaa");
    w.sources.changes.lock().unwrap().expired = true;
    w.pass().await;

    assert_eq!(w.listings(), 2);
    assert_eq!(w.names(), ["a.jpg"]);
}

#[tokio::test]
async fn a_listing_now_and_then_finds_what_the_platform_didnt_report() {
    let w = world().await;
    w.report_changes();
    w.add(true);
    w.pass().await;

    w.sources.put_unreported("a", b"aaa");
    w.pass().await;
    assert_eq!(w.names(), Vec::<String>::new());

    let id = w.source().id;
    let (token, _) = w.auto.store.change_token(&id).unwrap().unwrap();
    let long_ago = run::nanos(SystemTime::now()) - 8 * 24 * 60 * 60 * 1_000_000_000;
    w.auto
        .store
        .save_change_token(&id, &token, long_ago)
        .unwrap();
    w.pass().await;

    assert_eq!(w.listings(), 2);
    assert_eq!(w.names(), ["a.jpg"]);
    let (_, scanned) = w.auto.store.change_token(&id).unwrap().unwrap();
    assert!(scanned > long_ago);
}

#[tokio::test]
async fn an_answer_doesnt_count_as_a_listing() {
    let w = world().await;
    w.report_changes();
    w.add(true);
    w.pass().await;
    let id = w.source().id;
    let (_, scanned) = w.auto.store.change_token(&id).unwrap().unwrap();

    w.sources.put("a", "", b"aaa", OLD);
    w.pass().await;

    assert_eq!(w.auto.store.change_token(&id).unwrap().unwrap().1, scanned);
}

#[tokio::test]
async fn removing_a_source_forgets_its_token() {
    let w = world().await;
    w.report_changes();
    let source = w.add(true);
    w.pass().await;
    assert!(w.auto.store.change_token(&source.id).unwrap().is_some());

    w.auto.remove_source(source.id.clone()).await.unwrap();

    assert!(w.auto.store.change_token(&source.id).unwrap().is_none());
}

#[tokio::test]
async fn a_pass_says_which_file_it_is_sending_and_how_far_it_has_come() {
    let w = world().await;
    w.add(true);
    w.sources
        .put("a", "", b"0123456789", Duration::from_secs(60));

    w.auto.run_pass(false).await.expect("pass");

    let seen = w.seen.current.lock().unwrap();
    let done: Vec<u64> = seen.iter().map(|current| current.done).collect();
    assert_eq!(done.first(), Some(&0), "{seen:?}");
    assert!(done.contains(&(CHUNK as u64)), "{seen:?}");
    assert!(
        seen.iter().all(|c| c.name == "a.jpg" && c.total == 10),
        "{seen:?}"
    );
    drop(seen);
    assert_eq!(w.auto.current(), None);
}
