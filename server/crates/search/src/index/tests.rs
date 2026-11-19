use std::{io::Read, time::Duration};

use bytes::Bytes;
use futures_util::stream;
use sqlx::PgPool;

use files::{Actor, FileServices, FilesConfig, NodeUpdate, StoreOutcome};

use super::*;
use crate::provider::ExtractError;

struct Plain;

impl ContentProvider for Plain {
    fn id(&self) -> &'static str {
        "plain"
    }

    fn version(&self) -> u32 {
        1
    }

    fn accepts(&self, mime: &str, _name: &str) -> bool {
        mime == "text/plain"
    }

    fn extract(&self, reader: &mut dyn Read, limit: usize) -> Result<String, ExtractError> {
        let mut text = String::new();
        reader
            .take(u64::try_from(limit).unwrap_or(u64::MAX))
            .read_to_string(&mut text)?;
        Ok(text)
    }
}

const POLL_ATTEMPTS: u32 = 100;
const POLL_INTERVAL: Duration = Duration::from_millis(50);

struct Fixture {
    index: SearchIndex,
    _worker: Worker,
    files: FileServices,
    alice: UserId,
    bob: UserId,
    _dir: tempfile::TempDir,
}

async fn fixture(pool: PgPool, providers: Vec<Arc<dyn ContentProvider>>) -> Fixture {
    let db = Db::from_pool(pool);
    let dir = tempfile::tempdir().expect("tempdir");
    let store = BlobStore::open(dir.path()).await.expect("store");
    let alice = db
        .create_user(db::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let bob = db
        .create_user(db::TenantId::DEFAULT, "bob", "x")
        .await
        .expect("bob")
        .id;
    let (index, worker) = SearchIndex::start(db.clone(), store.clone(), providers);
    // Lost & Found folders are created without an index entry like any other node.
    index.catch_up().await.expect("initial catch up");
    Fixture {
        index,
        _worker: worker,
        files: FileServices::new(
            db,
            store,
            FilesConfig::new(1024 * 1024),
            events::Events::none(),
        ),
        alice,
        bob,
        _dir: dir,
    }
}

fn body(text: &'static str) -> impl futures_util::Stream<Item = Result<Bytes, files::BodyError>> {
    stream::iter([Ok(Bytes::from_static(text.as_bytes()))])
}

async fn upload(fx: &Fixture, parent: Option<NodeId>, name: &str, text: &'static str) -> Node {
    let node = fx
        .files
        .nodes
        .upload(Actor::user(fx.alice), parent, name, None, None, body(text))
        .await
        .expect("upload")
        .into_node();
    fx.index.index_name(&node).await.expect("index name");
    node
}

async fn names(index: &SearchIndex, owner: UserId, query: &str) -> Vec<String> {
    index
        .search(owner, query, None)
        .await
        .expect("search")
        .into_iter()
        .map(|hit| hit.node.name)
        .collect()
}

#[sqlx::test(migrations = "../../migrations")]
async fn finds_a_file_by_a_prefix_of_a_word_in_its_name(pool: PgPool) {
    let fx = fixture(pool, Vec::new()).await;
    upload(&fx, None, "IMG_2024-holidayPhoto.jpg", "x").await;
    assert_eq!(
        names(&fx.index, fx.alice, "holi").await,
        ["IMG_2024-holidayPhoto.jpg"]
    );
    assert_eq!(
        names(&fx.index, fx.alice, "photo 2024").await,
        ["IMG_2024-holidayPhoto.jpg"]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_word_must_match(pool: PgPool) {
    let fx = fixture(pool, Vec::new()).await;
    upload(&fx, None, "holiday.jpg", "x").await;
    assert!(names(&fx.index, fx.alice, "holiday work").await.is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn blank_query_returns_nothing(pool: PgPool) {
    let fx = fixture(pool, Vec::new()).await;
    upload(&fx, None, "holiday.jpg", "x").await;
    assert!(names(&fx.index, fx.alice, "  - ").await.is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn hits_report_their_location(pool: PgPool) {
    let fx = fixture(pool, Vec::new()).await;
    let folder = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), None, "trips")
        .await
        .expect("folder");
    upload(&fx, Some(folder.id), "paris.txt", "x").await;
    let hits = fx
        .index
        .search(fx.alice, "paris", None)
        .await
        .expect("search");
    assert_eq!(hits[0].location, ["trips"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn rename_reindexes_the_name(pool: PgPool) {
    let fx = fixture(pool, Vec::new()).await;
    let node = upload(&fx, None, "old.txt", "x").await;
    let renamed = fx
        .files
        .nodes
        .update(
            Actor::user(fx.alice),
            node.id,
            NodeUpdate {
                name: Some("fresh.txt".to_owned()),
                ..NodeUpdate::default()
            },
        )
        .await
        .expect("rename")
        .node;
    fx.index.index_name(&renamed).await.expect("reindex");
    assert!(names(&fx.index, fx.alice, "old").await.is_empty());
    assert_eq!(names(&fx.index, fx.alice, "fresh").await, ["fresh.txt"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn users_only_find_their_own_nodes(pool: PgPool) {
    let fx = fixture(pool, Vec::new()).await;
    upload(&fx, None, "secret.txt", "x").await;
    assert!(names(&fx.index, fx.bob, "secret").await.is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn catch_up_indexes_nodes_created_without_the_index(pool: PgPool) {
    let fx = fixture(pool, Vec::new()).await;
    fx.files
        .nodes
        .upload(
            Actor::user(fx.alice),
            None,
            "legacy.txt",
            None,
            None,
            body("x"),
        )
        .await
        .expect("upload");
    assert!(names(&fx.index, fx.alice, "legacy").await.is_empty());
    let report = fx.index.catch_up().await.expect("catch up");
    assert_eq!(report.names_indexed, 1);
    assert_eq!(names(&fx.index, fx.alice, "legacy").await, ["legacy.txt"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn catch_up_does_nothing_for_an_up_to_date_index(pool: PgPool) {
    let fx = fixture(pool, Vec::new()).await;
    upload(&fx, None, "a.txt", "x").await;
    assert_eq!(
        fx.index.catch_up().await.expect("catch up"),
        CatchUp::default()
    );
}

async fn eventually_finds(fx: &Fixture, query: &str, expected: &[&str]) {
    for _ in 0..POLL_ATTEMPTS {
        if names(&fx.index, fx.alice, query).await == expected {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert_eq!(names(&fx.index, fx.alice, query).await, expected);
}

#[sqlx::test(migrations = "../../migrations")]
async fn content_is_searchable_once_a_provider_extracted_it(pool: PgPool) {
    let fx = fixture(pool, vec![Arc::new(Plain)]).await;
    let node = upload(&fx, None, "notes.txt", "remember the zebra crossing").await;
    assert!(names(&fx.index, fx.alice, "zebra").await.is_empty());
    fx.index.enqueue_content(fx.alice, node.id);
    eventually_finds(&fx, "zebra", &["notes.txt"]).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn files_without_a_matching_provider_keep_name_only_entries(pool: PgPool) {
    let fx = fixture(pool, vec![Arc::new(Plain)]).await;
    let node = fx
        .files
        .nodes
        .upload(
            Actor::user(fx.alice),
            None,
            "photo.jpg",
            Some("image/jpeg"),
            None,
            body("zebra"),
        )
        .await
        .expect("upload")
        .into_node();
    fx.index.index_name(&node).await.expect("index name");
    fx.index.enqueue_content(fx.alice, node.id);
    fx.index.catch_up().await.expect("catch up");
    tokio::time::sleep(POLL_INTERVAL * 5).await;
    assert!(names(&fx.index, fx.alice, "zebra").await.is_empty());
    assert_eq!(names(&fx.index, fx.alice, "photo").await, ["photo.jpg"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn catch_up_re_extracts_content_that_changed(pool: PgPool) {
    let fx = fixture(pool, vec![Arc::new(Plain)]).await;
    let node = upload(&fx, None, "notes.txt", "alpha").await;
    fx.index.enqueue_content(fx.alice, node.id);
    eventually_finds(&fx, "alpha", &["notes.txt"]).await;
    fx.files
        .nodes
        .replace_content(Actor::user(fx.alice), node.id, None, None, body("bravo"))
        .await
        .expect("replace");
    let report = fx.index.catch_up().await.expect("catch up");
    assert_eq!(report.content_queued, 1);
    eventually_finds(&fx, "bravo", &["notes.txt"]).await;
    assert!(names(&fx.index, fx.alice, "alpha").await.is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_new_version_replaces_the_indexed_content(pool: PgPool) {
    let fx = fixture(pool, vec![Arc::new(Plain)]).await;
    let node = upload(&fx, None, "notes.txt", "alpha").await;
    fx.index.enqueue_content(fx.alice, node.id);
    eventually_finds(&fx, "alpha", &["notes.txt"]).await;
    let outcome = fx
        .files
        .nodes
        .upload(
            Actor::user(fx.alice),
            None,
            "notes.txt",
            None,
            None,
            body("bravo"),
        )
        .await
        .expect("upload");
    assert!(matches!(outcome, StoreOutcome::Versioned { .. }));
    fx.index.enqueue_content(fx.alice, node.id);
    eventually_finds(&fx, "bravo", &["notes.txt"]).await;
    assert!(names(&fx.index, fx.alice, "alpha").await.is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn catch_up_leaves_extracted_content_alone(pool: PgPool) {
    let fx = fixture(pool, vec![Arc::new(Plain)]).await;
    let node = upload(&fx, None, "notes.txt", "alpha").await;
    fx.index.enqueue_content(fx.alice, node.id);
    eventually_finds(&fx, "alpha", &["notes.txt"]).await;
    assert_eq!(
        fx.index.catch_up().await.expect("catch up"),
        CatchUp::default()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn rebuild_restores_names_and_content(pool: PgPool) {
    let fx = fixture(pool, vec![Arc::new(Plain)]).await;
    upload(&fx, None, "notes.txt", "alpha").await;
    let report = fx.index.rebuild().await.expect("rebuild");
    // The file and the two Lost & Found folders.
    assert_eq!(report.names_indexed, 3);
    eventually_finds(&fx, "alpha", &["notes.txt"]).await;
}
