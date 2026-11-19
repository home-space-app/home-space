use futures_util::stream;
use sqlx::PgPool;
use tokio::io::AsyncReadExt;

use bytes::Bytes;
use futures_util::Stream;
use time::OffsetDateTime;

use db::{FOLDER_MIME, Node, NodeId, OnConflict};

use super::*;
use crate::{
    Actor, BodyError, MoveOutcome, NodeUpdate, StoreOutcome,
    content::{FALLBACK_MIME, resolve_mime},
};

struct Fixture {
    files: FileServices,
    alice: UserId,
    bob: UserId,
    _dir: tempfile::TempDir,
}

fn body(text: &'static str) -> impl Stream<Item = Result<Bytes, BodyError>> + use<> {
    stream::iter([Ok(Bytes::from_static(text.as_bytes()))])
}

async fn fixture(pool: PgPool, max_upload_bytes: u64) -> Fixture {
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
            FilesConfig::new(max_upload_bytes),
            Events::none(),
        ),
        alice,
        bob,
        _dir: dir,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn upload_then_download_returns_same_content(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let node = fx
        .files
        .nodes
        .upload(
            Actor::user(fx.alice),
            None,
            "note.txt",
            None,
            None,
            body("hello"),
        )
        .await
        .map(StoreOutcome::into_node)
        .expect("upload");
    let (meta, mut file) = fx
        .files
        .nodes
        .download(fx.alice, node.id)
        .await
        .expect("download");
    let mut content = String::new();
    file.read_to_string(&mut content).await.expect("read");
    assert_eq!((content.as_str(), meta.size), ("hello", 5));
    assert_eq!(meta.mime, "text/plain");
}

#[sqlx::test(migrations = "../../migrations")]
async fn ancestors_run_from_root_to_parent(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let a = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), None, "a")
        .await
        .expect("a");
    let b = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), Some(a.id), "b")
        .await
        .expect("b");
    let file = fx
        .files
        .nodes
        .upload(
            Actor::user(fx.alice),
            Some(b.id),
            "f.txt",
            None,
            None,
            body("x"),
        )
        .await
        .map(StoreOutcome::into_node)
        .expect("file");
    let names = fx
        .files
        .nodes
        .ancestors(fx.alice, file.id)
        .await
        .expect("ancestors")
        .into_iter()
        .map(|node| node.name)
        .collect::<Vec<_>>();
    assert_eq!(names, ["a", "b"]);
    assert!(
        fx.files
            .nodes
            .ancestors(fx.alice, a.id)
            .await
            .expect("root level")
            .is_empty()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn ancestors_of_another_users_node_are_not_found(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let a = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), None, "a")
        .await
        .expect("a");
    assert!(matches!(
        fx.files.nodes.ancestors(fx.bob, a.id).await,
        Err(FilesError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn other_users_cannot_access_a_node(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let node = fx
        .files
        .nodes
        .upload(
            Actor::user(fx.alice),
            None,
            "note.txt",
            None,
            None,
            body("hello"),
        )
        .await
        .map(StoreOutcome::into_node)
        .expect("upload");
    assert!(matches!(
        fx.files.nodes.get(fx.bob, node.id).await,
        Err(FilesError::NotFound)
    ));
    assert!(matches!(
        fx.files.nodes.download(fx.bob, node.id).await,
        Err(FilesError::NotFound)
    ));
    assert!(matches!(
        fx.files.trash.delete(Actor::user(fx.bob), node.id).await,
        Err(FilesError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn cannot_upload_into_another_users_folder(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let folder = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), None, "docs")
        .await
        .expect("folder");
    let result = fx
        .files
        .nodes
        .upload(
            Actor::user(fx.bob),
            Some(folder.id),
            "x.txt",
            None,
            None,
            body("hi"),
        )
        .await;
    assert!(matches!(result, Err(FilesError::NotFound)));
}

async fn put(fx: &Fixture, parent: Option<NodeId>, name: &str, text: &'static str) -> Node {
    fx.files
        .nodes
        .upload(Actor::user(fx.alice), parent, name, None, None, body(text))
        .await
        .map(StoreOutcome::into_node)
        .expect("upload")
}

/// Now shifted by `offset`, at the microsecond precision the database keeps.
fn now_plus(offset: time::Duration) -> OffsetDateTime {
    let at = OffsetDateTime::now_utc() + offset;
    at.replace_nanosecond(at.nanosecond() / 1000 * 1000)
        .expect("nanosecond")
}

async fn put_at(fx: &Fixture, name: &str, text: &'static str, at: OffsetDateTime) -> StoreOutcome {
    fx.files
        .nodes
        .upload(
            Actor::user(fx.alice),
            None,
            name,
            None,
            Some(at),
            body(text),
        )
        .await
        .expect("upload")
}

async fn move_to(fx: &Fixture, id: NodeId, parent: Option<NodeId>) -> MoveOutcome {
    let update = NodeUpdate {
        parent_id: db::Patch::to(parent),
        ..NodeUpdate::default()
    };
    fx.files
        .nodes
        .update(Actor::user(fx.alice), id, update)
        .await
        .expect("move")
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

async fn version_content(fx: &Fixture, id: NodeId, version: i32) -> String {
    let (_, _, mut file) = fx
        .files
        .versions
        .download_version(fx.alice, id, version)
        .await
        .expect("version");
    let mut content = String::new();
    file.read_to_string(&mut content).await.expect("read");
    content
}

#[sqlx::test(migrations = "../../migrations")]
async fn uploading_identical_content_changes_nothing(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let first = put(&fx, None, "a.txt", "same").await;
    let second = fx
        .files
        .nodes
        .upload(
            Actor::user(fx.alice),
            None,
            "a.txt",
            None,
            None,
            body("same"),
        )
        .await
        .expect("upload");
    assert!(matches!(second, StoreOutcome::Unchanged(ref node) if node.id == first.id));
    assert_eq!(second.into_node().version, 1);
    assert!(
        fx.files
            .versions
            .versions(fx.alice, first.id)
            .await
            .expect("versions")
            .is_empty()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn uploading_different_content_keeps_the_old_one_as_a_version(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let first = put(&fx, None, "a.txt", "one").await;
    let second = put(&fx, None, "a.txt", "two").await;
    assert_eq!((second.id, second.version), (first.id, 2));
    assert_eq!(content_of(&fx, first.id).await, "two");
    assert_eq!(version_content(&fx, first.id, 1).await, "one");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_new_file_takes_the_client_modification_time(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let at = now_plus(-time::Duration::hours(1));
    let node = put_at(&fx, "a.txt", "one", at).await.into_node();
    assert_eq!(node.updated_at, at);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_future_modification_time_counts_as_now(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let at = now_plus(time::Duration::days(1));
    let node = put_at(&fx, "a.txt", "one", at).await.into_node();
    assert!(node.updated_at < at);
}

#[sqlx::test(migrations = "../../migrations")]
async fn newer_content_becomes_current(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let first = put_at(&fx, "a.txt", "old", now_plus(-time::Duration::hours(2)))
        .await
        .into_node();
    let newer = now_plus(-time::Duration::hours(1));
    let outcome = put_at(&fx, "a.txt", "new", newer).await;
    assert!(matches!(outcome, StoreOutcome::Versioned { .. }));
    let node = outcome.into_node();
    assert_eq!((node.version, node.updated_at), (2, newer));
    assert_eq!(content_of(&fx, first.id).await, "new");
    assert_eq!(version_content(&fx, first.id, 1).await, "old");
}

#[sqlx::test(migrations = "../../migrations")]
async fn older_content_is_kept_as_a_version_and_the_current_one_stays(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let current_at = now_plus(-time::Duration::hours(1));
    let first = put_at(&fx, "a.txt", "current", current_at)
        .await
        .into_node();
    let outcome = put_at(&fx, "a.txt", "older", now_plus(-time::Duration::hours(2))).await;
    assert!(matches!(outcome, StoreOutcome::Archived { .. }));
    let node = outcome.into_node();
    assert_eq!(
        (node.id, node.version, node.updated_at),
        (first.id, 2, current_at)
    );
    assert_eq!(content_of(&fx, first.id).await, "current");
    assert_eq!(version_content(&fx, first.id, 1).await, "older");
}

#[sqlx::test(migrations = "../../migrations")]
async fn replacing_with_older_content_keeps_it_as_a_version(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let file = put(&fx, None, "a.txt", "current").await;
    let older = now_plus(-time::Duration::hours(1));
    let outcome = fx
        .files
        .nodes
        .replace_content(
            Actor::user(fx.alice),
            file.id,
            None,
            Some(older),
            body("older"),
        )
        .await
        .expect("replace");
    assert!(matches!(outcome, StoreOutcome::Archived { .. }));
    assert_eq!(content_of(&fx, file.id).await, "current");
    assert_eq!(version_content(&fx, file.id, 1).await, "older");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_file_and_a_folder_can_share_a_name(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    put(&fx, None, "same", "x").await;
    assert!(
        fx.files
            .nodes
            .create_folder(Actor::user(fx.alice), None, "same")
            .await
            .is_ok()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn creating_a_folder_with_a_taken_name_conflicts(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    fx.files
        .nodes
        .create_folder(Actor::user(fx.alice), None, "a")
        .await
        .expect("a");
    let result = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), None, "a")
        .await;
    assert!(matches!(result, Err(FilesError::Conflict)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn folders_are_stored_with_the_directory_mime(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let folder = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), None, "a")
        .await
        .expect("a");
    assert_eq!(folder.mime, FOLDER_MIME);
}

#[sqlx::test(migrations = "../../migrations")]
async fn renaming_onto_a_taken_name_conflicts(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    put(&fx, None, "a.txt", "1").await;
    let other = put(&fx, None, "b.txt", "2").await;
    let update = NodeUpdate {
        name: Some("a.txt".to_owned()),
        ..NodeUpdate::default()
    };
    let result = fx
        .files
        .nodes
        .update(Actor::user(fx.alice), other.id, update)
        .await;
    assert!(matches!(result, Err(FilesError::Conflict)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn moving_an_identical_file_drops_the_source_and_merges_dates(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let folder = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), None, "dir")
        .await
        .expect("dir");
    let dest = put(&fx, Some(folder.id), "a.txt", "same").await;
    let source = put(&fx, None, "a.txt", "same").await;
    let outcome = move_to(&fx, source.id, Some(folder.id)).await;
    assert_eq!(outcome.node.id, dest.id);
    assert_eq!(outcome.node.version, 1);
    assert_eq!(
        outcome.node.created_at,
        dest.created_at.min(source.created_at)
    );
    assert_eq!(
        outcome.node.updated_at,
        dest.updated_at.max(source.updated_at)
    );
    assert_eq!(outcome.deleted.len(), 1);
    assert!(matches!(
        fx.files.nodes.get(fx.alice, source.id).await,
        Err(FilesError::NotFound)
    ));
    assert!(
        fx.files
            .content
            .store
            .open_blob(fx.alice, source.id)
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn moving_a_different_file_versions_the_destination_and_appends_history(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let folder = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), None, "dir")
        .await
        .expect("dir");
    let dest = put(&fx, Some(folder.id), "a.txt", "dest1").await;
    put(&fx, Some(folder.id), "a.txt", "dest2").await;
    let source = put(&fx, None, "a.txt", "src1").await;
    put(&fx, None, "a.txt", "src2").await;
    let outcome = move_to(&fx, source.id, Some(folder.id)).await;
    assert_eq!(outcome.node.id, dest.id);
    assert_eq!(outcome.node.version, 4);
    assert_eq!(content_of(&fx, dest.id).await, "src2");
    assert_eq!(version_content(&fx, dest.id, 1).await, "dest1");
    assert_eq!(version_content(&fx, dest.id, 2).await, "dest2");
    assert_eq!(version_content(&fx, dest.id, 3).await, "src1");
    assert_eq!(outcome.content_changed, [dest.id]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn moving_a_folder_merges_into_the_same_named_destination(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let target = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), None, "target")
        .await
        .expect("target");
    let dest = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), Some(target.id), "docs")
        .await
        .expect("dest");
    let dest_sub = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), Some(dest.id), "sub")
        .await
        .expect("dest sub");
    put(&fx, Some(dest_sub.id), "x.txt", "old").await;
    let source = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), None, "docs")
        .await
        .expect("src");
    let source_sub = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), Some(source.id), "sub")
        .await
        .expect("src sub");
    let new_x = put(&fx, Some(source_sub.id), "x.txt", "new").await;
    let only = put(&fx, Some(source.id), "only.txt", "o").await;

    let outcome = move_to(&fx, source.id, Some(target.id)).await;

    assert_eq!(outcome.node.id, dest.id);
    let sub = fx
        .files
        .nodes
        .list(fx.alice, Some(dest.id))
        .await
        .expect("list");
    assert_eq!(
        sub.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
        ["only.txt", "sub"]
    );
    let files = fx
        .files
        .nodes
        .list(fx.alice, Some(dest_sub.id))
        .await
        .expect("list");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].version, 2);
    assert_eq!(content_of(&fx, files[0].id).await, "new");
    assert_eq!(version_content(&fx, files[0].id, 1).await, "old");
    assert_eq!(content_of(&fx, only.id).await, "o");
    for gone in [source.id, source_sub.id, new_x.id] {
        assert!(matches!(
            fx.files.nodes.get(fx.alice, gone).await,
            Err(FilesError::NotFound)
        ));
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn purging_a_file_removes_its_content_and_version_blobs(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let file = put(&fx, None, "a.txt", "one").await;
    put(&fx, None, "a.txt", "two").await;
    let version_id = fx
        .files
        .content
        .db
        .get_version(fx.alice, file.id, 1)
        .await
        .expect("v")
        .id;
    fx.files
        .trash
        .delete(Actor::user(fx.alice), file.id)
        .await
        .expect("delete");
    fx.files
        .trash
        .purge_trashed(Actor::user(fx.alice), file.id)
        .await
        .expect("purge");
    assert!(
        fx.files
            .content
            .store
            .open_blob(fx.alice, file.id)
            .await
            .is_err()
    );
    assert!(
        fx.files
            .content
            .store
            .open_version(fx.alice, version_id)
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_version_keeps_the_current_content(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let file = put(&fx, None, "a.txt", "one").await;
    put(&fx, None, "a.txt", "two").await;
    fx.files
        .versions
        .delete_version(fx.alice, file.id, 1)
        .await
        .expect("delete version");
    assert!(matches!(
        fx.files
            .versions
            .download_version(fx.alice, file.id, 1)
            .await,
        Err(FilesError::NotFound)
    ));
    assert_eq!(content_of(&fx, file.id).await, "two");
}

#[sqlx::test(migrations = "../../migrations")]
async fn oversized_upload_is_rejected(pool: PgPool) {
    let fx = fixture(pool, 3).await;
    let result = fx
        .files
        .nodes
        .upload(
            Actor::user(fx.alice),
            None,
            "a.txt",
            None,
            None,
            body("hello"),
        )
        .await;
    assert!(matches!(result, Err(FilesError::TooLarge { limit: 3 })));
}

#[sqlx::test(migrations = "../../migrations")]
async fn invalid_name_is_rejected(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let result = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), None, "a/b")
        .await;
    assert!(matches!(result, Err(FilesError::InvalidName)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn folder_cannot_move_into_its_descendant(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let outer = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), None, "outer")
        .await
        .expect("outer");
    let inner = fx
        .files
        .nodes
        .create_folder(Actor::user(fx.alice), Some(outer.id), "inner")
        .await
        .expect("inner");
    let update = NodeUpdate {
        parent_id: db::Patch::Set(inner.id),
        ..NodeUpdate::default()
    };
    let result = fx
        .files
        .nodes
        .update(Actor::user(fx.alice), outer.id, update)
        .await;
    assert!(matches!(result, Err(FilesError::Cycle)));
}

async fn lost_and_found(fx: &Fixture) -> NodeId {
    fx.files
        .content
        .db
        .special_folders(fx.alice)
        .await
        .expect("folders")
        .lost_and_found_id
}

async fn folder(fx: &Fixture, parent: Option<NodeId>, name: &str) -> Node {
    fx.files
        .nodes
        .create_folder(Actor::user(fx.alice), parent, name)
        .await
        .expect("folder")
}

async fn bin_paths(fx: &Fixture) -> Vec<String> {
    let mut paths = fx
        .files
        .trash
        .trash(fx.alice)
        .await
        .expect("trash")
        .into_iter()
        .map(|item| item.original_path)
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

async fn blob_exists(fx: &Fixture, id: NodeId) -> bool {
    fx.files.content.store.open_blob(fx.alice, id).await.is_ok()
}

/// Bins two files named `a.txt` ("old", then "new") that lived in different folders.
async fn bin_same_named_pair(fx: &Fixture) -> (Node, Node) {
    let first = folder(fx, None, "first").await;
    let second = folder(fx, None, "second").await;
    let old = put(fx, Some(first.id), "a.txt", "old").await;
    let new = put(fx, Some(second.id), "a.txt", "new").await;
    fx.files
        .trash
        .delete(Actor::user(fx.alice), old.id)
        .await
        .expect("delete old");
    fx.files
        .trash
        .delete(Actor::user(fx.alice), new.id)
        .await
        .expect("delete new");
    (old, new)
}

#[sqlx::test(migrations = "../../migrations")]
async fn new_users_get_hidden_recycle_bin_and_lost_and_found(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let folders = fx
        .files
        .content
        .db
        .special_folders(fx.alice)
        .await
        .expect("folders");
    let root = fx.files.nodes.list(fx.alice, None).await.expect("root");
    assert_eq!(
        root.iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>(),
        ["Lost & Found"]
    );
    assert_eq!(root[0].id, folders.lost_and_found_id);
    assert_ne!(
        fx.files
            .content
            .db
            .special_folders(fx.bob)
            .await
            .expect("bob"),
        folders
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_recycle_bin_folder_is_not_reachable(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let trash = fx
        .files
        .content
        .db
        .special_folders(fx.alice)
        .await
        .expect("f")
        .trash_id;
    let file = put(&fx, None, "a.txt", "x").await;
    let not_found = |result: Result<(), FilesError>| {
        assert!(matches!(result, Err(FilesError::NotFound)));
    };
    not_found(fx.files.nodes.get(fx.alice, trash).await.map(drop));
    not_found(fx.files.nodes.list(fx.alice, Some(trash)).await.map(drop));
    not_found(fx.files.nodes.ancestors(fx.alice, trash).await.map(drop));
    not_found(
        fx.files
            .trash
            .delete(Actor::user(fx.alice), trash)
            .await
            .map(drop),
    );
    not_found(
        fx.files
            .nodes
            .update(
                Actor::user(fx.alice),
                trash,
                NodeUpdate {
                    name: Some("x".to_owned()),
                    ..NodeUpdate::default()
                },
            )
            .await
            .map(drop),
    );
    not_found(
        fx.files
            .nodes
            .update(
                Actor::user(fx.alice),
                file.id,
                NodeUpdate {
                    parent_id: db::Patch::Set(trash),
                    ..NodeUpdate::default()
                },
            )
            .await
            .map(drop),
    );
    not_found(
        fx.files
            .nodes
            .move_nodes(
                Actor::user(fx.alice),
                &[file.id],
                Some(trash),
                OnConflict::Override,
            )
            .await
            .map(drop),
    );
    not_found(
        fx.files
            .nodes
            .upload(
                Actor::user(fx.alice),
                Some(trash),
                "b.txt",
                None,
                None,
                body("x"),
            )
            .await
            .map(drop),
    );
    not_found(
        fx.files
            .nodes
            .create_folder(Actor::user(fx.alice), Some(trash), "f")
            .await
            .map(drop),
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_folder_bins_its_files_with_their_paths(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let p = folder(&fx, None, "p").await;
    let a = folder(&fx, Some(p.id), "a").await;
    let b = folder(&fx, Some(a.id), "b").await;
    let empty = folder(&fx, Some(a.id), "empty").await;
    let x = put(&fx, Some(b.id), "x.txt", "one").await;
    put(&fx, Some(b.id), "x.txt", "two").await;
    let y = put(&fx, Some(a.id), "y.txt", "y").await;
    let version_id = fx
        .files
        .content
        .db
        .get_version(fx.alice, x.id, 1)
        .await
        .expect("v")
        .id;

    let outcome = fx
        .files
        .trash
        .delete(Actor::user(fx.alice), a.id)
        .await
        .expect("delete");

    assert_eq!(outcome.node.id, a.id);
    assert_eq!(outcome.trashed.len(), 2);
    for gone in [a.id, b.id, empty.id] {
        assert!(matches!(
            fx.files.nodes.get(fx.alice, gone).await,
            Err(FilesError::NotFound)
        ));
    }
    assert!(fx.files.nodes.get(fx.alice, p.id).await.is_ok());
    assert_eq!(bin_paths(&fx).await, ["/p/a/b/x.txt", "/p/a/y.txt"]);
    assert!(blob_exists(&fx, x.id).await);
    assert!(blob_exists(&fx, y.id).await);
    assert!(
        fx.files
            .content
            .store
            .open_version(fx.alice, version_id)
            .await
            .is_ok()
    );
    assert!(matches!(
        fx.files.nodes.get(fx.alice, x.id).await,
        Err(FilesError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn same_named_files_coexist_in_the_bin_and_their_name_can_be_reused(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let first = put(&fx, None, "a.txt", "one").await;
    fx.files
        .trash
        .delete(Actor::user(fx.alice), first.id)
        .await
        .expect("delete");
    let second = put(&fx, None, "a.txt", "two").await;
    assert_ne!(second.id, first.id);
    assert_eq!(second.version, 1);
    fx.files
        .trash
        .delete(Actor::user(fx.alice), second.id)
        .await
        .expect("delete");
    let third = put(&fx, None, "a.txt", "three").await;
    assert_eq!(bin_paths(&fx).await, ["/a.txt", "/a.txt"]);
    assert_eq!(content_of(&fx, third.id).await, "three");
    assert!(blob_exists(&fx, first.id).await);
    assert!(blob_exists(&fx, second.id).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn binned_files_cannot_be_used(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let file = put(&fx, None, "a.txt", "one").await;
    put(&fx, None, "a.txt", "two").await;
    fx.files
        .trash
        .delete(Actor::user(fx.alice), file.id)
        .await
        .expect("delete");
    assert!(matches!(
        fx.files.nodes.get(fx.alice, file.id).await,
        Err(FilesError::NotFound)
    ));
    assert!(matches!(
        fx.files.nodes.download(fx.alice, file.id).await,
        Err(FilesError::NotFound)
    ));
    assert!(matches!(
        fx.files.versions.versions(fx.alice, file.id).await,
        Err(FilesError::NotFound)
    ));
    assert!(matches!(
        fx.files
            .versions
            .download_version(fx.alice, file.id, 1)
            .await,
        Err(FilesError::NotFound)
    ));
    assert!(matches!(
        fx.files.versions.delete_version(fx.alice, file.id, 1).await,
        Err(FilesError::NotFound)
    ));
    assert!(matches!(
        fx.files
            .nodes
            .replace_content(Actor::user(fx.alice), file.id, None, None, body("x"))
            .await,
        Err(FilesError::NotFound)
    ));
    assert!(matches!(
        fx.files.trash.delete(Actor::user(fx.alice), file.id).await,
        Err(FilesError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn restoring_lands_in_lost_and_found_with_all_versions(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let dir = folder(&fx, None, "dir").await;
    let file = put(&fx, Some(dir.id), "a.txt", "one").await;
    put(&fx, Some(dir.id), "a.txt", "two").await;
    fx.files
        .trash
        .delete(Actor::user(fx.alice), dir.id)
        .await
        .expect("delete");

    let batch = fx
        .files
        .trash
        .restore(Actor::user(fx.alice), &[file.id], OnConflict::Abort)
        .await
        .expect("restore");

    let found = lost_and_found(&fx).await;
    assert_eq!(batch.moved.len(), 1);
    assert_eq!(batch.target.as_ref().map(|node| node.id), Some(found));
    let restored = fx
        .files
        .nodes
        .get(fx.alice, file.id)
        .await
        .expect("restored");
    assert_eq!((restored.parent_id, restored.version), (Some(found), 2));
    assert_eq!(content_of(&fx, file.id).await, "two");
    assert_eq!(version_content(&fx, file.id, 1).await, "one");
    assert!(bin_paths(&fx).await.is_empty());
    let listing = fx
        .files
        .nodes
        .list(fx.alice, Some(found))
        .await
        .expect("list");
    assert_eq!(listing.len(), 1);
}

/// Bins "binned" as `/dir/a.txt` while Lost & Found holds a live "live".
async fn binned_clash(fx: &Fixture) -> (Node, Node) {
    let found = lost_and_found(fx).await;
    let live = put(fx, Some(found), "a.txt", "live").await;
    let dir = folder(fx, None, "dir").await;
    let binned = put(fx, Some(dir.id), "a.txt", "binned").await;
    fx.files
        .trash
        .delete(Actor::user(fx.alice), dir.id)
        .await
        .expect("delete");
    (live, binned)
}

#[sqlx::test(migrations = "../../migrations")]
async fn aborting_a_restore_reports_the_conflicts_and_changes_nothing(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let (live, binned) = binned_clash(&fx).await;

    let batch = fx
        .files
        .trash
        .restore(Actor::user(fx.alice), &[binned.id], OnConflict::Abort)
        .await
        .expect("restore");

    assert!(batch.moved.is_empty() && batch.skipped.is_empty());
    assert_eq!(batch.conflicts.len(), 1);
    let conflict = &batch.conflicts[0];
    assert_eq!(conflict.source.id, binned.id);
    assert_eq!(conflict.path, "/dir/a.txt");
    assert_eq!(
        conflict.existing.as_ref().map(|node| node.id),
        Some(live.id)
    );
    assert_eq!(bin_paths(&fx).await, ["/dir/a.txt"]);
    assert_eq!(content_of(&fx, live.id).await, "live");
}

#[sqlx::test(migrations = "../../migrations")]
async fn overriding_a_restore_merges_into_the_live_file(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let (live, binned) = binned_clash(&fx).await;

    let batch = fx
        .files
        .trash
        .restore(Actor::user(fx.alice), &[binned.id], OnConflict::Override)
        .await
        .expect("restore");

    assert_eq!(batch.moved.len(), 1);
    assert_eq!(batch.moved[0].id, live.id);
    assert_eq!(content_of(&fx, live.id).await, "binned");
    assert_eq!(version_content(&fx, live.id, 1).await, "live");
    assert!(bin_paths(&fx).await.is_empty());
    assert!(!blob_exists(&fx, binned.id).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn skipping_a_restore_leaves_the_file_in_the_bin(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let (live, binned) = binned_clash(&fx).await;
    let other = {
        let dir = folder(&fx, None, "other").await;
        let file = put(&fx, Some(dir.id), "b.txt", "b").await;
        fx.files
            .trash
            .delete(Actor::user(fx.alice), dir.id)
            .await
            .expect("delete");
        file
    };

    let batch = fx
        .files
        .trash
        .restore(
            Actor::user(fx.alice),
            &[binned.id, other.id],
            OnConflict::Skip,
        )
        .await
        .expect("restore");

    assert_eq!(
        batch.skipped.iter().map(|node| node.id).collect::<Vec<_>>(),
        [binned.id]
    );
    assert_eq!(
        batch.moved.iter().map(|node| node.id).collect::<Vec<_>>(),
        [other.id]
    );
    assert_eq!(bin_paths(&fx).await, ["/dir/a.txt"]);
    let unchanged = fx.files.nodes.get(fx.alice, live.id).await.expect("live");
    assert_eq!((unchanged.version, unchanged.sha256), (1, live.sha256));
    assert_eq!(content_of(&fx, live.id).await, "live");
    assert!(blob_exists(&fx, binned.id).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn restoring_same_named_files_aborts_and_skips_them_all(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let (old, new) = bin_same_named_pair(&fx).await;
    let ids = [old.id, new.id];

    let aborted = fx
        .files
        .trash
        .restore(Actor::user(fx.alice), &ids, OnConflict::Abort)
        .await
        .expect("abort");
    assert_eq!(aborted.conflicts.len(), 2);
    assert!(aborted.conflicts.iter().all(|c| c.existing.is_none()));
    let mut paths = aborted
        .conflicts
        .iter()
        .map(|c| c.path.as_str())
        .collect::<Vec<_>>();
    paths.sort_unstable();
    assert_eq!(paths, ["/first/a.txt", "/second/a.txt"]);
    assert_eq!(bin_paths(&fx).await.len(), 2);

    let skipped = fx
        .files
        .trash
        .restore(Actor::user(fx.alice), &ids, OnConflict::Skip)
        .await
        .expect("skip");
    assert!(skipped.moved.is_empty());
    assert_eq!(skipped.skipped.len(), 2);
    assert_eq!(bin_paths(&fx).await.len(), 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn overriding_same_named_files_restores_the_newest_as_current(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let (old, new) = bin_same_named_pair(&fx).await;

    let batch = fx
        .files
        .trash
        .restore(
            Actor::user(fx.alice),
            &[new.id, old.id],
            OnConflict::Override,
        )
        .await
        .expect("restore");

    assert_eq!(batch.moved.len(), 2);
    let found = fx
        .files
        .nodes
        .list(fx.alice, Some(lost_and_found(&fx).await))
        .await
        .expect("list");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].version, 2);
    assert_eq!(content_of(&fx, found[0].id).await, "new");
    assert_eq!(version_content(&fx, found[0].id, 1).await, "old");
    assert!(bin_paths(&fx).await.is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn restoring_ignores_live_files_and_rejects_unknown_ids(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let live = put(&fx, None, "live.txt", "x").await;
    let binned = put(&fx, None, "gone.txt", "y").await;
    fx.files
        .trash
        .delete(Actor::user(fx.alice), binned.id)
        .await
        .expect("delete");

    let batch = fx
        .files
        .trash
        .restore(
            Actor::user(fx.alice),
            &[live.id, binned.id],
            OnConflict::Abort,
        )
        .await
        .expect("restore");
    assert_eq!(batch.moved.len(), 1);
    assert_eq!(batch.moved[0].id, binned.id);
    assert_eq!(
        fx.files
            .nodes
            .get(fx.alice, live.id)
            .await
            .expect("live")
            .parent_id,
        None
    );

    assert!(matches!(
        fx.files
            .trash
            .restore(Actor::user(fx.alice), &[NodeId::new()], OnConflict::Abort)
            .await,
        Err(FilesError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn lost_and_found_and_its_ancestors_cannot_be_deleted(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let found = lost_and_found(&fx).await;
    assert!(matches!(
        fx.files.trash.delete(Actor::user(fx.alice), found).await,
        Err(FilesError::LostAndFound)
    ));

    let outer = folder(&fx, None, "outer").await;
    let inner = folder(&fx, Some(outer.id), "inner").await;
    put(&fx, Some(outer.id), "keep.txt", "x").await;
    move_to(&fx, found, Some(inner.id)).await;
    for id in [outer.id, inner.id, found] {
        assert!(matches!(
            fx.files.trash.delete(Actor::user(fx.alice), id).await,
            Err(FilesError::LostAndFound)
        ));
    }
    assert!(bin_paths(&fx).await.is_empty());
    assert!(fx.files.nodes.get(fx.alice, outer.id).await.is_ok());
}

#[sqlx::test(migrations = "../../migrations")]
async fn lost_and_found_can_be_renamed_and_stays_protected(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let found = lost_and_found(&fx).await;
    let update = NodeUpdate {
        name: Some("Found".to_owned()),
        ..NodeUpdate::default()
    };
    let renamed = fx
        .files
        .nodes
        .update(Actor::user(fx.alice), found, update)
        .await
        .expect("rename");
    assert_eq!(renamed.node.name, "Found");
    assert!(matches!(
        fx.files.trash.delete(Actor::user(fx.alice), found).await,
        Err(FilesError::LostAndFound)
    ));
    let file = put(&fx, None, "a.txt", "x").await;
    fx.files
        .trash
        .delete(Actor::user(fx.alice), file.id)
        .await
        .expect("delete");
    let batch = fx
        .files
        .trash
        .restore(Actor::user(fx.alice), &[file.id], OnConflict::Abort)
        .await
        .expect("restore");
    assert_eq!(batch.moved[0].parent_id, Some(found));
}

#[sqlx::test(migrations = "../../migrations")]
async fn purging_removes_one_binned_file_and_its_blobs(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let kept = put(&fx, None, "kept.txt", "k").await;
    let purged = put(&fx, None, "purged.txt", "one").await;
    put(&fx, None, "purged.txt", "two").await;
    let version_id = fx
        .files
        .content
        .db
        .get_version(fx.alice, purged.id, 1)
        .await
        .expect("v")
        .id;
    fx.files
        .trash
        .delete(Actor::user(fx.alice), kept.id)
        .await
        .expect("delete");
    fx.files
        .trash
        .delete(Actor::user(fx.alice), purged.id)
        .await
        .expect("delete");

    fx.files
        .trash
        .purge_trashed(Actor::user(fx.alice), purged.id)
        .await
        .expect("purge");

    assert_eq!(bin_paths(&fx).await, ["/kept.txt"]);
    assert!(!blob_exists(&fx, purged.id).await);
    assert!(
        fx.files
            .content
            .store
            .open_version(fx.alice, version_id)
            .await
            .is_err()
    );
    assert!(blob_exists(&fx, kept.id).await);
    assert!(matches!(
        fx.files
            .trash
            .restore(Actor::user(fx.alice), &[purged.id], OnConflict::Abort)
            .await,
        Err(FilesError::NotFound)
    ));
    assert!(matches!(
        fx.files
            .trash
            .purge_trashed(Actor::user(fx.alice), purged.id)
            .await,
        Err(FilesError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn purging_only_works_on_binned_files(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let live = put(&fx, None, "live.txt", "x").await;
    let dir = folder(&fx, None, "dir").await;
    for id in [live.id, dir.id] {
        assert!(matches!(
            fx.files
                .trash
                .purge_trashed(Actor::user(fx.alice), id)
                .await,
            Err(FilesError::NotFound)
        ));
    }
    assert_eq!(content_of(&fx, live.id).await, "x");
}

#[sqlx::test(migrations = "../../migrations")]
async fn emptying_the_bin_removes_every_binned_file_and_blob(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let dir = folder(&fx, None, "dir").await;
    let a = put(&fx, Some(dir.id), "a.txt", "one").await;
    put(&fx, Some(dir.id), "a.txt", "two").await;
    let b = put(&fx, None, "b.txt", "b").await;
    let version_id = fx
        .files
        .content
        .db
        .get_version(fx.alice, a.id, 1)
        .await
        .expect("v")
        .id;
    let live = put(&fx, None, "live.txt", "l").await;
    let bobs = fx
        .files
        .nodes
        .upload(
            Actor::user(fx.bob),
            None,
            "bob.txt",
            None,
            None,
            body("bob"),
        )
        .await
        .map(StoreOutcome::into_node)
        .expect("bob");
    fx.files
        .trash
        .delete(Actor::user(fx.bob), bobs.id)
        .await
        .expect("bob delete");
    fx.files
        .trash
        .delete(Actor::user(fx.alice), dir.id)
        .await
        .expect("delete");
    fx.files
        .trash
        .delete(Actor::user(fx.alice), b.id)
        .await
        .expect("delete");

    fx.files
        .trash
        .empty_trash(Actor::user(fx.alice))
        .await
        .expect("empty");

    assert!(bin_paths(&fx).await.is_empty());
    for id in [a.id, b.id] {
        assert!(!blob_exists(&fx, id).await);
        assert!(matches!(
            fx.files
                .trash
                .restore(Actor::user(fx.alice), &[id], OnConflict::Abort)
                .await,
            Err(FilesError::NotFound)
        ));
    }
    assert!(
        fx.files
            .content
            .store
            .open_version(fx.alice, version_id)
            .await
            .is_err()
    );
    assert_eq!(content_of(&fx, live.id).await, "l");
    assert_eq!(
        fx.files.trash.trash(fx.bob).await.expect("bob trash").len(),
        1
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn other_users_cannot_restore_or_purge_binned_files(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let file = put(&fx, None, "a.txt", "x").await;
    fx.files
        .trash
        .delete(Actor::user(fx.alice), file.id)
        .await
        .expect("delete");
    assert!(matches!(
        fx.files
            .trash
            .restore(Actor::user(fx.bob), &[file.id], OnConflict::Override)
            .await,
        Err(FilesError::NotFound)
    ));
    assert!(matches!(
        fx.files
            .trash
            .purge_trashed(Actor::user(fx.bob), file.id)
            .await,
        Err(FilesError::NotFound)
    ));
    assert!(
        fx.files
            .trash
            .trash(fx.bob)
            .await
            .expect("trash")
            .is_empty()
    );
    assert_eq!(bin_paths(&fx).await, ["/a.txt"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn replace_content_updates_size_and_hash(pool: PgPool) {
    let fx = fixture(pool, 1024).await;
    let file = fx
        .files
        .nodes
        .upload(Actor::user(fx.alice), None, "a.txt", None, None, body("hi"))
        .await
        .map(StoreOutcome::into_node)
        .expect("upload");
    let updated = fx
        .files
        .nodes
        .replace_content(Actor::user(fx.alice), file.id, None, None, body("longer"))
        .await
        .map(StoreOutcome::into_node)
        .expect("replace");
    assert_eq!(updated.size, 6);
    assert_ne!(updated.sha256, file.sha256);
}

#[test]
fn declared_type_wins_over_the_file_name() {
    assert_eq!(resolve_mime(Some("image/png"), "note.txt"), "image/png");
}

#[test]
fn declared_parameters_are_dropped() {
    assert_eq!(
        resolve_mime(Some("text/plain; charset=utf-8"), "a"),
        "text/plain"
    );
}

#[test]
fn octet_stream_falls_back_to_the_file_name() {
    assert_eq!(
        resolve_mime(Some("application/octet-stream"), "note.txt"),
        "text/plain"
    );
}

#[test]
fn missing_type_is_guessed_from_the_file_name() {
    assert_eq!(resolve_mime(None, "note.txt"), "text/plain");
}

#[test]
fn unknown_extension_stays_octet_stream() {
    assert_eq!(
        resolve_mime(Some("application/octet-stream"), "blob.zzzz"),
        FALLBACK_MIME
    );
}

#[test]
fn invalid_declared_type_is_ignored() {
    assert_eq!(resolve_mime(Some("not a mime"), "note.txt"), "text/plain");
}
