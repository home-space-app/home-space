use sqlx::PgPool;

use super::*;
use crate::{NewNode, NodeKind, TenantId};

async fn user(db: &Db, name: &str) -> UserId {
    db.create_user(TenantId::DEFAULT, name, "hash")
        .await
        .expect("user")
        .id
}

async fn file(db: &Db, owner: UserId, name: &str, size: i64, sha256: &[u8]) -> Node {
    db.create_node(
        owner,
        NewNode {
            id: NodeId::new(),
            parent_id: None,
            kind: NodeKind::File,
            name: name.to_owned(),
            size,
            sha256: Some(sha256.to_vec()),
            mime: "text/plain".to_owned(),
        },
    )
    .await
    .expect("file")
}

#[sqlx::test(migrations = "../../migrations")]
async fn lists_files_sharing_content_oldest_first(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let bob = user(&db, "bob").await;
    let first = file(&db, alice, "a.txt", 3, b"x").await;
    let second = file(&db, alice, "b.txt", 3, b"x").await;
    file(&db, alice, "unique.txt", 3, b"y").await;
    file(&db, alice, "empty-1.txt", 0, b"e").await;
    file(&db, alice, "empty-2.txt", 0, b"e").await;
    file(&db, bob, "a.txt", 3, b"x").await;

    let ids = db
        .duplicate_nodes(alice)
        .await
        .expect("duplicates")
        .into_iter()
        .map(|node| node.id)
        .collect::<Vec<_>>();

    assert_eq!(ids, vec![first.id, second.id]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn nodes_by_ids_stays_within_the_owner(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let bob = user(&db, "bob").await;
    let mine = file(&db, alice, "a.txt", 1, b"x").await;
    let theirs = file(&db, bob, "a.txt", 1, b"x").await;

    let found = db
        .nodes_by_ids(alice, &[mine.id, theirs.id])
        .await
        .expect("nodes");

    assert_eq!(found, vec![mine]);
}
