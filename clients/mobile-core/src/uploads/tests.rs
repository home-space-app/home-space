//! The queue against a fake of the server's upload API. How the server itself treats a name clash
//! is tested in `server/crates/api/tests/uploads.rs`.

use std::path::Path;
use std::sync::Mutex;

use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::tests::{node, signed_in};

const UPLOAD_ID: &str = "00000000-0000-0000-0000-0000000000bb";

/// Originals by source name; one not listed can't be read. Records what was released.
#[derive(Default)]
struct Originals {
    files: Mutex<Vec<(String, Vec<u8>)>>,
    released: Mutex<Vec<String>>,
}

impl PickedFiles for Originals {
    fn copy_file(&self, source: String, dest: String) -> Result<(), CoreError> {
        let files = self.files.lock().unwrap();
        let Some((_, content)) = files.iter().find(|(name, _)| *name == source) else {
            return Err(CoreError::Failed {
                code: "file".to_owned(),
                detail: "no longer allowed".to_owned(),
            });
        };
        std::fs::write(dest, content).unwrap();
        Ok(())
    }

    fn release(&self, source: String) {
        self.released.lock().unwrap().push(source);
    }
}

/// Runs something once each time the queue reports a change, until it says it is done.
#[derive(Default)]
struct OnChange(Mutex<Option<Box<dyn FnMut() -> bool + Send>>>);

impl AutoUploadListener for OnChange {
    fn changed(&self) {
        // Not held while it runs, since what it does on the queue reports changes too.
        let Some(mut run) = self.0.lock().unwrap().take() else {
            return;
        };
        if !run() {
            *self.0.lock().unwrap() = Some(run);
        }
    }
}

struct Setup {
    server: MockServer,
    client: Arc<CoreClient>,
    dir: tempfile::TempDir,
    originals: Arc<Originals>,
    changes: Arc<OnChange>,
}

impl Setup {
    async fn new() -> Self {
        let server = MockServer::start().await;
        let client = Arc::new(signed_in(&server).await);
        Self {
            server,
            client,
            dir: tempfile::tempdir().unwrap(),
            originals: Arc::default(),
            changes: Arc::default(),
        }
    }

    fn open(&self) -> Arc<UploadQueue> {
        UploadQueue::open(
            self.client.clone(),
            self.dir.path().to_string_lossy().into_owned(),
            self.originals.clone(),
            self.changes.clone(),
        )
        .unwrap()
    }

    fn original(&self, source: &str, content: &[u8]) {
        self.originals
            .files
            .lock()
            .unwrap()
            .push((source.to_owned(), content.to_vec()));
    }

    fn add(&self, queue: &UploadQueue, name: &str, source: &str, content: &[u8]) -> String {
        self.original(source, content);
        queue
            .add(None, "Home".into(), name.into(), Some(source.into()), None)
            .unwrap()
    }

    async fn mount_start(&self, size: u64, chunk_size: u64) {
        Mock::given(method("POST"))
            .and(path(protocol::paths::UPLOADS))
            .and(body_partial_json(
                serde_json::json!({ "name": "a.txt", "size": size }),
            ))
            // The user chose this file, so a name clash makes a new version.
            .and(|request: &wiremock::Request| {
                let body: serde_json::Value = request.body_json().expect("json body");
                body.get("keep_both").is_none() && body.get("auto_upload").is_none()
            })
            .respond_with(upload_session(0, size, chunk_size))
            .expect(1)
            .mount(&self.server)
            .await;
    }

    /// The files left in the queue's folder, leaving out its database.
    fn copies(&self) -> Vec<String> {
        let mut found = Vec::new();
        for folder in std::fs::read_dir(self.dir.path()).unwrap() {
            let folder = folder.unwrap().path();
            for file in std::fs::read_dir(&folder).unwrap() {
                let name = file.unwrap().file_name().to_string_lossy().into_owned();
                if !name.starts_with("queue.db") {
                    found.push(name);
                }
            }
        }
        found
    }
}

fn upload_session(offset: u64, size: u64, chunk_size: u64) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "id": UPLOAD_ID, "offset": offset, "size": size, "chunk_size": chunk_size,
    }))
}

fn chunk_at(offset: &str, reply: ResponseTemplate) -> Mock {
    Mock::given(method("PATCH"))
        .and(path(format!("{}/{UPLOAD_ID}", protocol::paths::UPLOADS)))
        .and(header(protocol::paths::UPLOAD_OFFSET, offset))
        .respond_with(reply)
}

fn chunk_stored(offset: u64) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_json(serde_json::json!({ "offset": offset, "node": node("a.txt", "file", 6) }))
}

fn chunk_taken(offset: u64) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({ "offset": offset }))
}

fn refused() -> ResponseTemplate {
    ResponseTemplate::new(400)
        .set_body_json(serde_json::json!({ "error": "bad_request", "message": "no" }))
}

fn only(queue: &UploadQueue) -> QueuedUpload {
    let mut items = queue.items().unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    items.remove(0)
}

#[tokio::test]
async fn a_picked_file_is_copied_goes_up_in_chunks_and_leaves_the_queue() {
    let setup = Setup::new().await;
    setup.mount_start(6, 4).await;
    chunk_at("0", chunk_taken(4))
        .expect(1)
        .mount(&setup.server)
        .await;
    chunk_at("4", chunk_stored(6))
        .expect(1)
        .mount(&setup.server)
        .await;
    let queue = setup.open();
    setup.add(&queue, "a.txt", "picked", b"abcdef");
    assert_eq!(only(&queue).state, QueuedUploadState::Waiting);

    let summary = queue.run().await.unwrap();

    assert_eq!(summary, QueueSummary::default());
    assert!(queue.items().unwrap().is_empty());
    assert!(setup.copies().is_empty(), "{:?}", setup.copies());
    // Access to the original goes as soon as there is a copy.
    assert_eq!(*setup.originals.released.lock().unwrap(), ["picked"]);
}

#[tokio::test]
async fn chunks_stay_small_whatever_the_server_allows() {
    const SERVER_CHUNK: u64 = 32 * 1024 * 1024;
    let size = MAX_CHUNK_BYTES + 1;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(protocol::paths::CAPABILITIES))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "upload_chunk_bytes": SERVER_CHUNK })),
        )
        .mount(&server)
        .await;
    let client = Arc::new(signed_in(&server).await);
    let setup = Setup {
        server,
        client,
        dir: tempfile::tempdir().unwrap(),
        originals: Arc::default(),
        changes: Arc::default(),
    };
    setup.mount_start(size, SERVER_CHUNK).await;
    chunk_at("0", chunk_taken(MAX_CHUNK_BYTES))
        .expect(1)
        .mount(&setup.server)
        .await;
    chunk_at(&MAX_CHUNK_BYTES.to_string(), chunk_stored(size))
        .expect(1)
        .mount(&setup.server)
        .await;
    let queue = setup.open();
    let content = vec![0; usize::try_from(size).unwrap()];
    setup.add(&queue, "a.txt", "picked", &content);

    queue.run().await.unwrap();

    let requests = setup.server.received_requests().await.unwrap();
    let chunks: Vec<usize> = requests
        .iter()
        .filter(|request| request.method.as_str() == "PATCH")
        .map(|request| request.body.len())
        .collect();
    assert_eq!(chunks, [usize::try_from(MAX_CHUNK_BYTES).unwrap(), 1]);
}

#[tokio::test]
async fn pausing_lasts_until_the_user_continues() {
    let setup = Setup::new().await;
    setup.mount_start(6, 4).await;
    chunk_at("0", chunk_taken(4))
        .expect(1)
        .mount(&setup.server)
        .await;
    let queue = setup.open();
    let key = setup.add(&queue, "a.txt", "picked", b"abcdef");
    let pausing = queue.clone();
    *setup.changes.0.lock().unwrap() = Some(Box::new(move || {
        let progressed = pausing
            .items()
            .unwrap()
            .first()
            .is_some_and(|item| item.done == 4);
        if progressed {
            pausing.pause().unwrap();
        }
        progressed
    }));

    let summary = queue.run().await.unwrap();

    assert!(summary.stopped);
    assert_eq!((summary.remaining, summary.paused), (0, 1));
    assert_eq!(only(&queue).state, QueuedUploadState::Paused);
    assert_eq!(only(&queue).done, 4);

    // Opening the app again sends nothing. A chunk now would fail, with no reply set up for it.
    queue.run().await.unwrap();
    let reopened = setup.open();
    let summary = reopened.run().await.unwrap();
    assert_eq!((summary.remaining, summary.paused), (0, 1));
    assert_eq!(only(&reopened).state, QueuedUploadState::Paused);

    Mock::given(method("GET"))
        .and(path(format!("{}/{UPLOAD_ID}", protocol::paths::UPLOADS)))
        .respond_with(upload_session(4, 6, 4))
        .expect(1)
        .mount(&setup.server)
        .await;
    chunk_at("4", chunk_stored(6))
        .expect(1)
        .mount(&setup.server)
        .await;
    reopened.retry(key).unwrap();
    assert_eq!(reopened.run().await.unwrap(), QueueSummary::default());
    assert!(reopened.items().unwrap().is_empty());
}

#[tokio::test]
async fn a_file_added_after_pausing_goes_and_the_paused_one_waits() {
    let setup = Setup::new().await;
    setup.mount_start(6, 6).await;
    chunk_at("0", chunk_stored(6))
        .expect(1)
        .mount(&setup.server)
        .await;
    let queue = setup.open();
    setup.add(&queue, "paused.txt", "first", b"abc");
    queue.pause().unwrap();
    setup.add(&queue, "a.txt", "second", b"abcdef");

    let summary = queue.run().await.unwrap();

    assert_eq!((summary.remaining, summary.paused), (0, 1));
    assert_eq!(only(&queue).name, "paused.txt");
    assert_eq!(only(&queue).state, QueuedUploadState::Paused);
}

#[test]
fn a_queue_from_before_pausing_keeps_its_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("queue.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE queue (
                position INTEGER PRIMARY KEY AUTOINCREMENT,
                key      TEXT NOT NULL UNIQUE,
                parent   TEXT,
                folder   TEXT NOT NULL,
                name     TEXT NOT NULL,
                source   TEXT,
                ready    INTEGER NOT NULL,
                size     INTEGER NOT NULL,
                done     INTEGER NOT NULL,
                session  TEXT,
                problem  TEXT
            );
            INSERT INTO queue (key, folder, name, ready, size, done)
            VALUES ('old', 'Home', 'a.txt', 1, 6, 4);",
        )
        .unwrap();

    let rows = Store::open(&path).unwrap().rows().unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].key, "old");
    assert!(!rows[0].paused);
    assert_eq!(rows[0].mime, None);
}

#[tokio::test]
async fn the_queue_continues_where_it_stopped_after_the_app_ends() {
    let setup = Setup::new().await;
    setup.mount_start(6, 4).await;
    chunk_at("0", chunk_taken(4))
        .expect(1)
        .mount(&setup.server)
        .await;
    chunk_at("4", chunk_stored(6))
        .expect(1)
        .mount(&setup.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{}/{UPLOAD_ID}", protocol::paths::UPLOADS)))
        .respond_with(upload_session(4, 6, 4))
        .expect(1)
        .mount(&setup.server)
        .await;
    let queue = setup.open();
    setup.add(&queue, "a.txt", "picked", b"abcdef");
    let stopping = queue.clone();
    *setup.changes.0.lock().unwrap() = Some(Box::new(move || {
        let progressed = stopping.items().unwrap()[0].done == 4;
        if progressed {
            stopping.stop();
        }
        progressed
    }));

    let summary = queue.run().await.unwrap();
    assert!(summary.stopped, "{summary:?}");
    assert_eq!(summary.remaining, 1);
    // What the phone knows shows at once in the next run of the app.
    drop(queue);
    let reopened = setup.open();
    let waiting = only(&reopened);
    assert_eq!(
        (waiting.state, waiting.done, waiting.total),
        (QueuedUploadState::Waiting, 4, 6)
    );

    let summary = reopened.run().await.unwrap();
    assert_eq!(summary, QueueSummary::default());
    assert!(reopened.items().unwrap().is_empty());
}

#[tokio::test]
async fn a_server_that_cant_be_reached_leaves_everything_for_the_next_run() {
    let setup = Setup::new().await;
    Mock::given(method("POST"))
        .and(path(protocol::paths::UPLOADS))
        .respond_with(ResponseTemplate::new(502))
        .up_to_n_times(1)
        .mount(&setup.server)
        .await;
    setup.mount_start(6, 6).await;
    chunk_at("0", chunk_stored(6))
        .expect(1)
        .mount(&setup.server)
        .await;
    let queue = setup.open();
    setup.add(&queue, "a.txt", "picked", b"abcdef");

    let summary = queue.run().await.unwrap();
    assert!(summary.offline, "{summary:?}");
    let item = only(&queue);
    assert_eq!(item.state, QueuedUploadState::Offline);
    assert_eq!(item.problem, None);

    assert_eq!(queue.run().await.unwrap(), QueueSummary::default());
    assert!(queue.items().unwrap().is_empty());
}

#[tokio::test]
async fn a_refused_file_waits_for_the_user_and_the_next_one_goes() {
    let setup = Setup::new().await;
    Mock::given(method("POST"))
        .and(path(protocol::paths::UPLOADS))
        .and(body_partial_json(serde_json::json!({ "name": "bad.txt" })))
        .respond_with(refused())
        .up_to_n_times(1)
        .mount(&setup.server)
        .await;
    Mock::given(method("POST"))
        .and(path(protocol::paths::UPLOADS))
        .respond_with(upload_session(0, 6, 6))
        .expect(2)
        .mount(&setup.server)
        .await;
    chunk_at("0", chunk_stored(6))
        .expect(2)
        .mount(&setup.server)
        .await;
    let queue = setup.open();
    let bad = setup.add(&queue, "bad.txt", "one", b"abcdef");
    setup.add(&queue, "a.txt", "two", b"abcdef");

    let summary = queue.run().await.unwrap();

    assert_eq!((summary.failed, summary.remaining), (1, 0));
    let item = only(&queue);
    assert_eq!(item.key, bad);
    assert_eq!(item.state, QueuedUploadState::Failed);
    assert_eq!(item.problem.as_deref(), Some("No"));
    assert!(item.can_retry);

    // It also survives the app ending, without being sent by itself.
    drop(queue);
    let reopened = setup.open();
    assert_eq!(reopened.run().await.unwrap().failed, 1);

    reopened.retry(bad).unwrap();
    assert_eq!(reopened.run().await.unwrap(), QueueSummary::default());
    assert!(reopened.items().unwrap().is_empty());
}

#[tokio::test]
async fn an_original_that_cant_be_read_can_be_tried_again() {
    let setup = Setup::new().await;
    Mock::given(method("POST"))
        .and(path(protocol::paths::UPLOADS))
        .respond_with(upload_session(0, 6, 6))
        .expect(1)
        .mount(&setup.server)
        .await;
    chunk_at("0", chunk_stored(6))
        .expect(1)
        .mount(&setup.server)
        .await;
    let queue = setup.open();
    let key = queue
        .add(None, "Home".into(), "a.txt".into(), Some("picked".into()), None)
        .unwrap();

    assert_eq!(queue.run().await.unwrap().failed, 1);
    let item = only(&queue);
    assert_eq!(item.problem.as_deref(), Some("No longer allowed"));
    assert!(item.can_retry);
    assert!(setup.originals.released.lock().unwrap().is_empty());

    setup.original("picked", b"abcdef");
    queue.retry(key).unwrap();
    assert_eq!(queue.run().await.unwrap(), QueueSummary::default());
}

#[tokio::test]
async fn cancelling_stops_at_the_next_chunk_and_ends_the_upload_on_the_server() {
    let setup = Setup::new().await;
    setup.mount_start(6, 4).await;
    chunk_at("0", chunk_taken(4))
        .expect(1)
        .mount(&setup.server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{}/{UPLOAD_ID}", protocol::paths::UPLOADS)))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&setup.server)
        .await;
    let queue = setup.open();
    let key = setup.add(&queue, "a.txt", "picked", b"abcdef");
    let cancelling = queue.clone();
    *setup.changes.0.lock().unwrap() = Some(Box::new(move || {
        let progressed = cancelling
            .items()
            .unwrap()
            .first()
            .is_some_and(|item| item.done == 4);
        if progressed {
            cancelling.cancel(key.clone()).unwrap();
        }
        progressed
    }));

    assert_eq!(queue.run().await.unwrap(), QueueSummary::default());

    assert!(queue.items().unwrap().is_empty());
    assert!(setup.copies().is_empty(), "{:?}", setup.copies());
}

#[tokio::test]
async fn cancelling_one_that_failed_ends_it_on_the_server_and_removes_the_copy() {
    let setup = Setup::new().await;
    setup.mount_start(6, 4).await;
    chunk_at("0", refused()).mount(&setup.server).await;
    let deleted = Mock::given(method("DELETE"))
        .and(path(format!("{}/{UPLOAD_ID}", protocol::paths::UPLOADS)))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount_as_scoped(&setup.server)
        .await;
    let queue = setup.open();
    let key = setup.add(&queue, "a.txt", "picked", b"abcdef");
    assert_eq!(queue.run().await.unwrap().failed, 1);
    assert_eq!(setup.copies().len(), 1);

    queue.cancel(key).unwrap();

    assert!(queue.items().unwrap().is_empty());
    assert!(setup.copies().is_empty());
    deleted.wait_until_satisfied().await;
}

#[tokio::test]
async fn a_file_the_app_was_still_preparing_when_it_ended_must_be_picked_again() {
    let setup = Setup::new().await;
    let queue = setup.open();
    let key = queue
        .add(None, "Home".into(), "Photo".into(), None, None)
        .unwrap();
    assert_eq!(only(&queue).state, QueuedUploadState::Preparing);
    // Nothing can go while the app makes the copy.
    assert_eq!(queue.run().await.unwrap().remaining, 1);
    drop(queue);

    let reopened = setup.open();
    let item = only(&reopened);
    assert_eq!(item.state, QueuedUploadState::Failed);
    assert_eq!(item.problem.as_deref(), Some(PICK_AGAIN));
    assert!(!item.can_retry);
    reopened.cancel(key).unwrap();
    assert!(reopened.items().unwrap().is_empty());
}

#[tokio::test]
async fn a_file_the_app_prepared_goes_under_the_name_it_gives() {
    let setup = Setup::new().await;
    setup.mount_start(6, 6).await;
    chunk_at("0", chunk_stored(6))
        .expect(1)
        .mount(&setup.server)
        .await;
    let queue = setup.open();
    let key = queue
        .add(None, "Home".into(), "Photo".into(), None, None)
        .unwrap();
    std::fs::write(queue.copy_path(key.clone()), b"abcdef").unwrap();
    queue.ready(key, Some("a.txt".into())).unwrap();
    assert_eq!(only(&queue).name, "a.txt");

    assert_eq!(queue.run().await.unwrap(), QueueSummary::default());
}

#[tokio::test]
async fn a_type_the_app_gives_goes_to_the_server() {
    let setup = Setup::new().await;
    Mock::given(method("POST"))
        .and(path(protocol::paths::UPLOADS))
        .and(body_partial_json(
            serde_json::json!({ "name": "Pixel_2026-11-11_10-00-00", "mime": "text/plain" }),
        ))
        .respond_with(upload_session(0, 6, 6))
        .expect(1)
        .mount(&setup.server)
        .await;
    chunk_at("0", chunk_stored(6))
        .expect(1)
        .mount(&setup.server)
        .await;
    let queue = setup.open();
    let key = queue
        .add(
            None,
            "Home".into(),
            "Pixel_2026-11-11_10-00-00".into(),
            None,
            Some("text/plain".into()),
        )
        .unwrap();
    std::fs::write(queue.copy_path(key.clone()), b"abcdef").unwrap();
    queue.ready(key, None).unwrap();

    assert_eq!(queue.run().await.unwrap(), QueueSummary::default());
}

#[tokio::test]
async fn a_copy_that_has_gone_asks_for_the_file_again() {
    let setup = Setup::new().await;
    let queue = setup.open();
    let key = queue
        .add(None, "Home".into(), "a.txt".into(), None, None)
        .unwrap();
    let copy = queue.copy_path(key.clone());
    std::fs::write(&copy, b"abcdef").unwrap();
    queue.ready(key, None).unwrap();
    std::fs::remove_file(Path::new(&copy)).unwrap();

    assert_eq!(queue.run().await.unwrap().failed, 1);
    let item = only(&queue);
    assert_eq!(item.problem.as_deref(), Some(COPY_GONE));
    assert!(!item.can_retry);
}

#[tokio::test]
async fn adding_during_a_run_is_picked_up_by_that_run() {
    let setup = Setup::new().await;
    Mock::given(method("POST"))
        .and(path(protocol::paths::UPLOADS))
        .respond_with(upload_session(0, 6, 6))
        .expect(2)
        .mount(&setup.server)
        .await;
    chunk_at("0", chunk_stored(6))
        .expect(2)
        .mount(&setup.server)
        .await;
    let queue = setup.open();
    setup.add(&queue, "a.txt", "one", b"abcdef");
    setup.original("two", b"abcdef");
    let adding = queue.clone();
    *setup.changes.0.lock().unwrap() = Some(Box::new(move || {
        adding
            .add(None, "Home".into(), "b.txt".into(), Some("two".into()), None)
            .unwrap();
        true
    }));

    assert_eq!(queue.run().await.unwrap(), QueueSummary::default());
    assert!(queue.items().unwrap().is_empty());
}

#[tokio::test]
async fn wiping_ends_uploads_on_the_server_and_deletes_the_queue() {
    let setup = Setup::new().await;
    setup.mount_start(6, 4).await;
    chunk_at("0", refused()).mount(&setup.server).await;
    Mock::given(method("DELETE"))
        .and(path(format!("{}/{UPLOAD_ID}", protocol::paths::UPLOADS)))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&setup.server)
        .await;
    let queue = setup.open();
    setup.add(&queue, "a.txt", "picked", b"abcdef");
    setup.original("other", b"x");
    queue.run().await.unwrap();
    let waiting = queue
        .add(None, "Home".into(), "b.txt".into(), Some("other".into()), None)
        .unwrap();
    queue.stop();

    queue.wipe().await.unwrap();

    assert_eq!(std::fs::read_dir(setup.dir.path()).unwrap().count(), 0);
    assert_eq!(
        *setup.originals.released.lock().unwrap(),
        ["picked", "other"]
    );
    drop(waiting);
    // Whatever still holds the queue can't start it again.
    assert_eq!(queue.run().await.unwrap().remaining, 0);
}

#[tokio::test]
async fn wiping_during_an_upload_stops_it_at_the_next_chunk() {
    let setup = Setup::new().await;
    setup.mount_start(6, 4).await;
    chunk_at("0", chunk_taken(4))
        .expect(1)
        .mount(&setup.server)
        .await;
    // The rest never goes.
    chunk_at("4", chunk_stored(6))
        .expect(0)
        .mount(&setup.server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{}/{UPLOAD_ID}", protocol::paths::UPLOADS)))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&setup.server)
        .await;
    let queue = setup.open();
    setup.add(&queue, "a.txt", "picked", b"abcdef");
    let (wiped, wipe_now) = tokio::sync::oneshot::channel();
    let mut wiped = Some(wiped);
    let wiping = queue.clone();
    *setup.changes.0.lock().unwrap() = Some(Box::new(move || {
        // As signing out does: from outside the run, and without waiting for it.
        let progressed = wiping
            .items()
            .unwrap()
            .first()
            .is_some_and(|item| item.done == 4);
        if progressed {
            let wiping = wiping.clone();
            let done = wiped.take().expect("once");
            wiping.inner.wiped.store(true, Ordering::Relaxed);
            wiping.inner.stop.store(true, Ordering::Relaxed);
            RUNTIME.spawn(async move {
                wiping.wipe().await.unwrap();
                let _ = done.send(());
            });
        }
        progressed
    }));

    queue.run().await.unwrap();
    wipe_now.await.unwrap();

    assert_eq!(std::fs::read_dir(setup.dir.path()).unwrap().count(), 0);
}
