use futures_util::stream;
use sha2::Sha256;
use sqlx::PgPool;
use tokio::io::AsyncReadExt;

use db::Db;

use super::*;
use crate::{Actor, BlobStore, FileServices, FilesConfig, Names, StoreOptions};
use db::NodeId;
use events::Events;

struct Fixture {
    files: FileServices,
    alice: UserId,
    bob: UserId,
    _dir: tempfile::TempDir,
}

async fn fixture(pool: PgPool) -> Fixture {
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
    Fixture {
        files: FileServices::new(
            db,
            store,
            FilesConfig {
                chunk_bytes: 4,
                ..FilesConfig::new(1024)
            },
            Events::none(),
        ),
        alice,
        bob,
        _dir: dir,
    }
}

fn chunk(text: &'static str) -> impl Stream<Item = Result<Bytes, BodyError>> + use<> {
    stream::iter([Ok(Bytes::from_static(text.as_bytes()))])
}

/// Yields `text` and then fails, like a connection that drops mid-chunk.
fn broken(text: &'static str) -> impl Stream<Item = Result<Bytes, BodyError>> + use<> {
    stream::iter([
        Ok(Bytes::from_static(text.as_bytes())),
        Err(BodyError::Interrupted),
    ])
}

fn new_file(name: &str) -> UploadTarget {
    UploadTarget::New {
        parent: None,
        name: name.to_owned(),
        options: StoreOptions::default(),
    }
}

async fn start(fx: &Fixture, size: u64) -> Uuid {
    fx.files
        .uploads
        .create_upload(
            fx.alice,
            None,
            crate::UploadRequest {
                target: new_file("a.txt"),
                names: &Names::Plain,
                size,
                content_type: None,
                modified_at: None,
            },
        )
        .await
        .expect("create")
        .id
}

async fn append(fx: &Fixture, id: Uuid, offset: u64, text: &'static str) -> ChunkOutcome {
    fx.files
        .uploads
        .append_chunk(
            Actor::user(fx.alice),
            id,
            offset,
            &Names::Plain,
            chunk(text),
        )
        .await
        .expect("append")
}

fn completed(outcome: ChunkOutcome) -> db::Node {
    match outcome {
        ChunkOutcome::Complete(stored) => stored.into_node(),
        ChunkOutcome::Partial { offset } => panic!("still partial at {offset}"),
        ChunkOutcome::Transferred { .. } => panic!("stored into a transfer"),
    }
}

async fn content_of(fx: &Fixture, id: NodeId) -> String {
    let (_, mut file) = fx
        .files
        .nodes
        .download(fx.alice, id)
        .await
        .expect("download");
    let mut content = String::new();
    file.read_to_string(&mut content).await.expect("read");
    content
}

#[sqlx::test(migrations = "../../migrations")]
async fn chunks_store_the_same_file_as_a_single_upload(pool: PgPool) {
    let fx = fixture(pool).await;
    let id = start(&fx, 11).await;
    assert!(matches!(
        append(&fx, id, 0, "hell").await,
        ChunkOutcome::Partial { offset: 4 }
    ));
    assert!(matches!(
        append(&fx, id, 4, "o wo").await,
        ChunkOutcome::Partial { offset: 8 }
    ));
    let node = completed(append(&fx, id, 8, "rld").await);
    assert_eq!(content_of(&fx, node.id).await, "hello world");
    assert_eq!(node.sha256, Some(Sha256::digest(b"hello world").to_vec()));
    assert_eq!((node.size, node.mime.as_str()), (11, "text/plain"));
    assert!(matches!(
        fx.files.uploads.upload_status(fx.alice, id).await,
        Err(FilesError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_wrong_offset_is_rejected_with_the_current_one(pool: PgPool) {
    let fx = fixture(pool).await;
    let id = start(&fx, 8).await;
    append(&fx, id, 0, "abc").await;
    let result = fx
        .files
        .uploads
        .append_chunk(Actor::user(fx.alice), id, 1, &Names::Plain, chunk("x"))
        .await;
    assert!(matches!(
        result,
        Err(FilesError::OffsetMismatch { current: 3 })
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_interrupted_chunk_resumes_from_the_received_bytes(pool: PgPool) {
    let fx = fixture(pool).await;
    let id = start(&fx, 6).await;
    let result = fx
        .files
        .uploads
        .append_chunk(Actor::user(fx.alice), id, 0, &Names::Plain, broken("abc"))
        .await;
    assert!(result.is_err());
    let status = fx
        .files
        .uploads
        .upload_status(fx.alice, id)
        .await
        .expect("status");
    assert_eq!(status.offset, 3);
    let node = completed(append(&fx, id, 3, "def").await);
    assert_eq!(node.sha256, Some(Sha256::digest(b"abcdef").to_vec()));
}

#[sqlx::test(migrations = "../../migrations")]
async fn completing_after_a_restart_rehashes_the_file(pool: PgPool) {
    let fx = fixture(pool).await;
    let id = start(&fx, 6).await;
    append(&fx, id, 0, "abc").await;
    fx.files.uploads.uploads.forget_all();
    let node = completed(append(&fx, id, 3, "def").await);
    assert_eq!(node.sha256, Some(Sha256::digest(b"abcdef").to_vec()));
    assert_eq!(content_of(&fx, node.id).await, "abcdef");
}

#[sqlx::test(migrations = "../../migrations")]
async fn sizes_over_the_limit_are_rejected(pool: PgPool) {
    let fx = fixture(pool).await;
    let result = fx
        .files
        .uploads
        .create_upload(
            fx.alice,
            None,
            crate::UploadRequest {
                target: new_file("a.txt"),
                names: &Names::Plain,
                size: 1025,
                content_type: None,
                modified_at: None,
            },
        )
        .await;
    assert!(matches!(result, Err(FilesError::TooLarge { limit: 1024 })));
    let id = start(&fx, 2).await;
    let result = fx
        .files
        .uploads
        .append_chunk(Actor::user(fx.alice), id, 0, &Names::Plain, chunk("abc"))
        .await;
    assert!(matches!(result, Err(FilesError::TooLarge { limit: 2 })));
}

#[sqlx::test(migrations = "../../migrations")]
async fn other_users_cannot_see_or_feed_an_upload(pool: PgPool) {
    let fx = fixture(pool).await;
    let id = start(&fx, 4).await;
    assert!(matches!(
        fx.files.uploads.upload_status(fx.bob, id).await,
        Err(FilesError::NotFound)
    ));
    assert!(matches!(
        fx.files
            .uploads
            .append_chunk(Actor::user(fx.bob), id, 0, &Names::Plain, chunk("abcd"))
            .await,
        Err(FilesError::NotFound)
    ));
    assert!(matches!(
        fx.files.uploads.cancel_upload(fx.bob, id).await,
        Err(FilesError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_replace_upload_versions_the_existing_file(pool: PgPool) {
    let fx = fixture(pool).await;
    let file = fx
        .files
        .nodes
        .upload(
            Actor::user(fx.alice),
            crate::NewFile {
                parent: None,
                name: "a.txt",
                names: &Names::Plain,
                content_type: None,
                modified_at: None,
            },
            chunk("old"),
        )
        .await
        .expect("upload")
        .into_node();
    let id = fx
        .files
        .uploads
        .create_upload(
            fx.alice,
            None,
            crate::UploadRequest {
                target: UploadTarget::Replace(file.id),
                names: &Names::Plain,
                size: 5,
                content_type: None,
                modified_at: None,
            },
        )
        .await
        .expect("create")
        .id;
    append(&fx, id, 0, "new").await;
    let node = completed(append(&fx, id, 3, "er").await);
    assert_eq!((node.id, node.version), (file.id, 2));
    assert_eq!(content_of(&fx, file.id).await, "newer");
}

#[sqlx::test(migrations = "../../migrations")]
async fn invalid_names_are_rejected_up_front(pool: PgPool) {
    let fx = fixture(pool).await;
    let result = fx
        .files
        .uploads
        .create_upload(
            fx.alice,
            None,
            crate::UploadRequest {
                target: new_file("a/b"),
                names: &Names::Plain,
                size: 1,
                content_type: None,
                modified_at: None,
            },
        )
        .await;
    assert!(matches!(result, Err(FilesError::InvalidName)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn cancelling_a_devices_uploads_leaves_the_others(pool: PgPool) {
    let fx = fixture(pool).await;
    let laptop = Uuid::now_v7();
    let from_laptop = fx
        .files
        .uploads
        .create_upload(
            fx.alice,
            Some(laptop),
            crate::UploadRequest {
                target: new_file("a.txt"),
                names: &Names::Plain,
                size: 4,
                content_type: None,
                modified_at: None,
            },
        )
        .await
        .expect("laptop")
        .id;
    let unnamed = start(&fx, 4).await;

    let cancelled = fx
        .files
        .uploads
        .cancel_device_uploads(fx.alice, laptop)
        .await
        .expect("cancel");

    assert_eq!(cancelled, 1);
    assert!(matches!(
        fx.files.uploads.upload_status(fx.alice, from_laptop).await,
        Err(FilesError::NotFound)
    ));
    assert!(
        fx.files
            .uploads
            .upload_status(fx.alice, unnamed)
            .await
            .is_ok()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn cancelling_and_expiring_remove_the_upload(pool: PgPool) {
    let fx = fixture(pool).await;
    let cancelled = start(&fx, 4).await;
    fx.files
        .uploads
        .cancel_upload(fx.alice, cancelled)
        .await
        .expect("cancel");
    assert!(matches!(
        fx.files.uploads.upload_status(fx.alice, cancelled).await,
        Err(FilesError::NotFound)
    ));

    let stale = start(&fx, 4).await;
    append(&fx, stale, 0, "ab").await;
    let fresh = start(&fx, 4).await;
    assert_eq!(
        fx.files
            .uploads
            .expire_uploads(Duration::from_secs(3600))
            .await
            .expect("keep"),
        0
    );
    assert!(
        fx.files
            .uploads
            .expire_uploads(Duration::ZERO)
            .await
            .expect("expire")
            >= 2
    );
    for id in [stale, fresh] {
        assert!(matches!(
            fx.files.uploads.upload_status(fx.alice, id).await,
            Err(FilesError::NotFound)
        ));
    }
}
