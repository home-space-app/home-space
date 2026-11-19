use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use db::{Db, NewContent, NewNode, Node, NodeId, NodeKind, OnConflict, Patch, TenantId, UserId};

use super::{NodeUpdate, StoreOptions, move_nodes, store_file, update_node};
use crate::FilesError;

async fn store(db: &Db, owner: UserId, name: &str, content: NewContent) -> Node {
    store_file(db, owner, None, name, content, StoreOptions::default())
        .await
        .expect("store")
        .into_node()
}

async fn add(db: &Db, owner: UserId, parent: Option<NodeId>, kind: NodeKind, name: &str) -> Node {
    let new = NewNode {
        id: NodeId::new(),
        parent_id: parent,
        kind,
        name: name.to_owned(),
        size: 0,
        sha256: None,
        mime: "text/plain".to_owned(),
    };
    db.create_node(owner, new).await.expect("node")
}

async fn file(db: &Db, owner: UserId, parent: NodeId, name: &str) -> Node {
    add(db, owner, Some(parent), NodeKind::File, name).await
}

async fn folder(db: &Db, owner: UserId, parent: Option<NodeId>, name: &str) -> Node {
    add(db, owner, parent, NodeKind::Folder, name).await
}

struct Tree {
    alice: UserId,
    /// Destination with `Photos/2024/a.jpg` and `Photos/old.jpg`.
    target: Node,
    existing: Node,
    /// Source with `Photos/2024/a.jpg` (clashing), `Photos/2024/b.jpg` and `Photos/new.jpg`.
    source_photos: Node,
    clashing: Node,
    fresh: Node,
    nested_fresh: Node,
}

async fn tree(db: &Db) -> Tree {
    let alice = db
        .create_user(TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let target = folder(db, alice, None, "target").await;
    let photos = folder(db, alice, Some(target.id), "Photos").await;
    let year = folder(db, alice, Some(photos.id), "2024").await;
    let existing = file(db, alice, year.id, "a.jpg").await;
    file(db, alice, photos.id, "old.jpg").await;
    let source_photos = folder(db, alice, None, "Photos").await;
    let source_year = folder(db, alice, Some(source_photos.id), "2024").await;
    let clashing = file(db, alice, source_year.id, "a.jpg").await;
    let nested_fresh = file(db, alice, source_year.id, "b.jpg").await;
    let fresh = file(db, alice, source_photos.id, "new.jpg").await;
    Tree {
        alice,
        target,
        existing,
        source_photos,
        clashing,
        fresh,
        nested_fresh,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn aborting_a_move_lists_nested_conflicts_and_changes_nothing(pool: PgPool) {
    let db = Db::from_pool(pool);
    let tree = tree(&db).await;

    let batch = move_nodes(
        &db,
        tree.alice,
        &[tree.source_photos.id],
        Some(tree.target.id),
        OnConflict::Abort,
    )
    .await
    .expect("move");

    assert!(batch.moved.is_empty());
    assert_eq!(batch.conflicts.len(), 1);
    let conflict = &batch.conflicts[0];
    assert_eq!(conflict.path, "Photos/2024/a.jpg");
    assert_eq!(conflict.source.id, tree.clashing.id);
    assert_eq!(
        conflict.existing.as_ref().map(|node| node.id),
        Some(tree.existing.id)
    );
    assert_eq!(
        batch.target.as_ref().map(|node| node.id),
        Some(tree.target.id)
    );
    for id in [tree.source_photos.id, tree.clashing.id, tree.fresh.id] {
        assert!(db.get_node(tree.alice, id).await.is_ok());
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn moving_without_conflicts_merges_folders_even_when_aborting(pool: PgPool) {
    let db = Db::from_pool(pool);
    let tree = tree(&db).await;
    db.delete_node(tree.alice, tree.clashing.id)
        .await
        .expect("remove clash");

    let batch = move_nodes(
        &db,
        tree.alice,
        &[tree.source_photos.id],
        Some(tree.target.id),
        OnConflict::Abort,
    )
    .await
    .expect("move");

    assert!(batch.conflicts.is_empty());
    assert_eq!(batch.moved.len(), 1);
    assert_ne!(batch.moved[0].id, tree.source_photos.id);
    let fresh = db.get_node(tree.alice, tree.fresh.id).await.expect("fresh");
    assert_eq!(fresh.parent_id, Some(batch.moved[0].id));
    assert!(
        db.get_node(tree.alice, tree.source_photos.id)
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn skipping_a_move_bins_clashing_files_and_removes_emptied_folders(pool: PgPool) {
    let db = Db::from_pool(pool);
    let tree = tree(&db).await;

    let batch = move_nodes(
        &db,
        tree.alice,
        &[tree.source_photos.id],
        Some(tree.target.id),
        OnConflict::Skip,
    )
    .await
    .expect("move");

    assert!(batch.conflicts.is_empty());
    assert!(batch.binned);
    assert_eq!(
        batch.skipped.iter().map(|node| node.id).collect::<Vec<_>>(),
        [tree.clashing.id]
    );
    // The destination is untouched.
    let existing = db
        .get_node(tree.alice, tree.existing.id)
        .await
        .expect("existing");
    assert_eq!(existing.version, 1);
    // The clashing file is in the bin with the path it had.
    assert!(db.get_node(tree.alice, tree.clashing.id).await.is_err());
    let binned = db.list_trash(tree.alice).await.expect("trash");
    assert_eq!(binned.len(), 1);
    assert_eq!(binned[0].node.id, tree.clashing.id);
    assert_eq!(binned[0].original_path, "/Photos/2024/a.jpg");
    // The other files moved and the emptied source folders are gone.
    for moved in [tree.fresh.id, tree.nested_fresh.id] {
        let node = db.get_node(tree.alice, moved).await.expect("moved");
        assert_ne!(node.parent_id, Some(tree.source_photos.id));
    }
    assert!(
        db.get_node(tree.alice, tree.source_photos.id)
            .await
            .is_err()
    );
    assert!(
        batch
            .deleted
            .iter()
            .any(|node| node.id == tree.source_photos.id)
    );
    assert!(
        batch
            .deleted
            .iter()
            .any(|node| node.id == tree.clashing.id && node.parent_id.is_some())
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn overriding_a_move_merges_clashing_files_without_binning(pool: PgPool) {
    let db = Db::from_pool(pool);
    let tree = tree(&db).await;

    let batch = move_nodes(
        &db,
        tree.alice,
        &[tree.source_photos.id],
        Some(tree.target.id),
        OnConflict::Override,
    )
    .await
    .expect("move");

    assert!(!batch.binned);
    assert!(batch.skipped.is_empty());
    assert!(db.list_trash(tree.alice).await.expect("trash").is_empty());
    assert!(db.get_node(tree.alice, tree.existing.id).await.is_ok());
    assert!(db.get_node(tree.alice, tree.clashing.id).await.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn selected_files_with_the_same_name_clash_with_each_other(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let one = folder(&db, alice, None, "one").await;
    let two = folder(&db, alice, None, "two").await;
    let target = folder(&db, alice, None, "target").await;
    let a = file(&db, alice, one.id, "same.txt").await;
    let b = file(&db, alice, two.id, "same.txt").await;
    let other = file(&db, alice, two.id, "other.txt").await;
    let ids = [a.id, b.id, other.id];

    let aborted = move_nodes(&db, alice, &ids, Some(target.id), OnConflict::Abort)
        .await
        .expect("abort");
    assert_eq!(aborted.conflicts.len(), 2);
    assert!(aborted.conflicts.iter().all(|c| c.existing.is_none()));
    assert!(aborted.conflicts.iter().all(|c| c.path == "same.txt"));

    let skipped = move_nodes(&db, alice, &ids, Some(target.id), OnConflict::Skip)
        .await
        .expect("skip");
    assert_eq!(skipped.skipped.len(), 2);
    let binned = db.list_trash(alice).await.expect("trash");
    let mut paths = binned
        .iter()
        .map(|item| item.original_path.as_str())
        .collect::<Vec<_>>();
    paths.sort_unstable();
    assert_eq!(paths, ["/one/same.txt", "/two/same.txt"]);
    assert_eq!(
        skipped.moved.iter().map(|node| node.id).collect::<Vec<_>>(),
        [other.id]
    );
    let listing = db
        .list_children(alice, Some(target.id))
        .await
        .expect("list");
    assert_eq!(listing.len(), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn nodes_inside_a_selected_folder_move_along_with_it(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let dir = folder(&db, alice, None, "dir").await;
    let inner = file(&db, alice, dir.id, "a.txt").await;
    let target = folder(&db, alice, None, "target").await;

    let batch = move_nodes(
        &db,
        alice,
        &[inner.id, dir.id, dir.id],
        Some(target.id),
        OnConflict::Abort,
    )
    .await
    .expect("move");

    assert_eq!(batch.moved.len(), 1);
    assert_eq!(batch.moved[0].id, dir.id);
    let inner = db.get_node(alice, inner.id).await.expect("inner");
    assert_eq!(inner.parent_id, Some(dir.id));
}

#[sqlx::test(migrations = "../../migrations")]
async fn moving_checks_cycles_ownership_and_the_target(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let bob = db
        .create_user(TenantId::DEFAULT, "bob", "x")
        .await
        .expect("bob")
        .id;
    let outer = folder(&db, alice, None, "outer").await;
    let inner = folder(&db, alice, Some(outer.id), "inner").await;
    let plain = file(&db, alice, outer.id, "a.txt").await;
    let move_to = |ids: Vec<NodeId>, parent: Option<NodeId>, owner| {
        let db = db.clone();
        async move { move_nodes(&db, owner, &ids, parent, OnConflict::Override).await }
    };
    assert!(matches!(
        move_to(vec![outer.id], Some(inner.id), alice).await,
        Err(FilesError::Cycle)
    ));
    assert!(matches!(
        move_to(vec![outer.id], Some(outer.id), alice).await,
        Err(FilesError::Cycle)
    ));
    assert!(matches!(
        move_to(vec![plain.id], Some(plain.id), alice).await,
        Err(FilesError::NotFound)
    ));
    assert!(matches!(
        move_to(vec![plain.id], Some(inner.id), bob).await,
        Err(FilesError::NotFound)
    ));
    assert!(matches!(
        move_to(vec![plain.id, NodeId::new()], Some(inner.id), alice).await,
        Err(FilesError::NotFound)
    ));
    let root = move_to(vec![plain.id], None, alice).await.expect("to root");
    assert!(root.target.is_none());
    assert_eq!(root.moved[0].parent_id, None);
}

fn content(byte: u8) -> NewContent {
    NewContent {
        size: 1,
        sha256: vec![byte; 32],
        mime: "text/plain".to_owned(),
        modified_at: None,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_file_names_the_device_that_wrote_it_and_a_rename_keeps_it(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let laptop = Uuid::now_v7();
    let written = store(&db.on_device(Some(laptop)), alice, "a.txt", content(1)).await;
    let update = NodeUpdate {
        name: Some("b.txt".to_owned()),
        parent_id: Patch::Keep,
    };

    let renamed = update_node(&db, alice, written.id, update)
        .await
        .expect("rename");

    assert_eq!(written.edited_on, Some(laptop));
    assert_eq!(renamed.node.edited_on, Some(laptop));
}

#[sqlx::test(migrations = "../../migrations")]
async fn new_content_names_its_device_and_the_version_keeps_the_previous_one(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let (laptop, phone) = (Uuid::now_v7(), Uuid::now_v7());
    store(&db.on_device(Some(laptop)), alice, "a.txt", content(1)).await;

    let node = store(&db.on_device(Some(phone)), alice, "a.txt", content(2)).await;

    let versions = db.list_versions(alice, node.id).await.expect("versions");
    assert_eq!(node.edited_on, Some(phone));
    assert_eq!(
        versions.iter().map(|v| v.edited_on).collect::<Vec<_>>(),
        [Some(laptop)]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn older_content_is_kept_as_a_version_of_the_device_that_sent_it(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let (laptop, phone) = (Uuid::now_v7(), Uuid::now_v7());
    store(&db.on_device(Some(laptop)), alice, "a.txt", content(1)).await;
    let older = NewContent {
        modified_at: Some(OffsetDateTime::UNIX_EPOCH),
        ..content(2)
    };

    let node = store(&db.on_device(Some(phone)), alice, "a.txt", older).await;

    let versions = db.list_versions(alice, node.id).await.expect("versions");
    assert_eq!(node.edited_on, Some(laptop));
    assert_eq!(
        versions.iter().map(|v| v.edited_on).collect::<Vec<_>>(),
        [Some(phone)]
    );
}
