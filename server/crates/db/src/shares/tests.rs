use sqlx::PgPool;

use super::*;
use crate::SharePermission;
use crate::{NewNode, SharedAccess};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789a";

async fn user(db: &Db, name: &str) -> UserId {
    db.create_user(TenantId::DEFAULT, name, "hash")
        .await
        .expect("user")
        .id
}

async fn node(db: &Db, owner: UserId, parent: Option<NodeId>, kind: NodeKind) -> Node {
    db.create_node(
        owner,
        NewNode {
            id: NodeId::new(),
            parent_id: parent,
            kind,
            name: Uuid::now_v7().to_string(),
            size: 0,
            sha256: None,
            mime: String::new(),
        },
    )
    .await
    .expect("node")
}

fn new_share(token: &str) -> NewShareLink<'_> {
    NewShareLink {
        id: Uuid::now_v7(),
        token,
        password_hash: None,
        permission: SharePermission::Read,
        expires_at: None,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_node_inside_a_shared_folder_is_reachable(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let folder = node(&db, alice, None, NodeKind::Folder).await;
    let inner = node(&db, alice, Some(folder.id), NodeKind::Folder).await;
    let file = node(&db, alice, Some(inner.id), NodeKind::File).await;
    let share = db
        .create_share_link(alice, new_share(TOKEN), &[folder.id])
        .await
        .expect("share");
    assert_eq!(
        db.shared_node(share.root(), file.id)
            .await
            .expect("reachable")
            .id,
        file.id
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_sibling_of_a_shared_folder_is_not_reachable(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let parent = node(&db, alice, None, NodeKind::Folder).await;
    let shared = node(&db, alice, Some(parent.id), NodeKind::Folder).await;
    let sibling = node(&db, alice, Some(parent.id), NodeKind::File).await;
    let share = db
        .create_share_link(alice, new_share(TOKEN), &[shared.id])
        .await
        .expect("share");
    assert!(matches!(
        db.shared_node(share.root(), sibling.id).await,
        Err(DbError::NotFound)
    ));
    assert!(matches!(
        db.shared_node(share.root(), parent.id).await,
        Err(DbError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn another_users_node_cannot_be_shared(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let bob = user(&db, "bob").await;
    let theirs = node(&db, bob, None, NodeKind::File).await;
    assert!(matches!(
        db.create_share_link(alice, new_share(TOKEN), &[theirs.id])
            .await,
        Err(DbError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn changing_the_password_raises_its_version(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let file = node(&db, alice, None, NodeKind::File).await;
    let share = db
        .create_share_link(alice, new_share(TOKEN), &[file.id])
        .await
        .expect("share");
    db.update_share(
        alice,
        share.id,
        ShareUpdate {
            password_hash: crate::Patch::Set("phc"),
            ..ShareUpdate::default()
        },
    )
    .await
    .expect("update");
    let updated = db
        .share_by_token(TenantId::DEFAULT, TOKEN)
        .await
        .expect("by token");
    assert_eq!(updated.password_hash.as_deref(), Some("phc"));
    assert_eq!(updated.password_version, share.password_version + 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_new_link_only_reads(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let file = node(&db, alice, None, NodeKind::File).await;
    let share = db
        .create_share_link(alice, new_share(TOKEN), &[file.id])
        .await
        .expect("share");
    assert_eq!(share.permission, SharePermission::Read);
    assert_eq!(
        db.share_by_token(TenantId::DEFAULT, TOKEN)
            .await
            .expect("by token")
            .id,
        share.id
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn changing_the_expiry_keeps_the_password(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let file = node(&db, alice, None, NodeKind::File).await;
    let new = NewShareLink {
        password_hash: Some("phc"),
        ..new_share(TOKEN)
    };
    let share = db
        .create_share_link(alice, new, &[file.id])
        .await
        .expect("share");
    let expires_at = OffsetDateTime::now_utc() + time::Duration::days(1);
    db.update_share(
        alice,
        share.id,
        ShareUpdate {
            expires_at: crate::Patch::Set(expires_at),
            ..ShareUpdate::default()
        },
    )
    .await
    .expect("update");
    let updated = db
        .share_by_token(TenantId::DEFAULT, TOKEN)
        .await
        .expect("by token");
    assert_eq!(updated.password_hash.as_deref(), Some("phc"));
    assert_eq!(updated.password_version, share.password_version);
    assert!(updated.expires_at.is_some());
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_share_removes_its_link(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let file = node(&db, alice, None, NodeKind::File).await;
    let share = db
        .create_share_link(alice, new_share(TOKEN), &[file.id])
        .await
        .expect("share");
    db.delete_share(alice, share.id).await.expect("delete");
    assert!(matches!(
        db.share_by_token(TenantId::DEFAULT, TOKEN).await,
        Err(DbError::NotFound)
    ));
}

fn new_user_share() -> NewUserShare {
    NewUserShare {
        id: Uuid::now_v7(),
        expires_at: None,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_share_with_users_only_reads_and_has_no_link(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let bob = user(&db, "bob").await;
    let file = node(&db, alice, None, NodeKind::File).await;
    let share = db
        .create_user_share(alice, new_user_share(), &[file.id], &[bob])
        .await
        .expect("share");
    assert_eq!(share.token, None);
    let recipients = db.share_recipients(&[share.id]).await.expect("recipients");
    let only = &recipients[&share.id];
    assert_eq!(only.len(), 1);
    assert_eq!(only[0].user_id, bob);
    assert_eq!(only[0].username, "bob");
    assert_eq!(only[0].permission, SharePermission::Read);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_share_cannot_be_for_its_owner(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let file = node(&db, alice, None, NodeKind::File).await;
    assert!(matches!(
        db.create_user_share(alice, new_user_share(), &[file.id], &[alice])
            .await,
        Err(DbError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_share_cannot_be_for_an_unknown_user(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let file = node(&db, alice, None, NodeKind::File).await;
    assert!(matches!(
        db.create_user_share(alice, new_user_share(), &[file.id], &[UserId::new()])
            .await,
        Err(DbError::NotFound)
    ));
    assert!(db.owned_shares(alice, None).await.expect("list").is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn shares_stay_within_their_tenant(pool: PgPool) {
    let db = Db::from_pool(pool);
    let other = crate::tenants::add_test_tenant(&db).await;
    let alice = user(&db, "alice").await;
    let bob = user(&db, "bob").await;
    let mallory = db
        .create_user(other, "mallory", "hash")
        .await
        .expect("mallory")
        .id;
    let file = node(&db, alice, None, NodeKind::File).await;

    assert!(matches!(
        db.create_user_share(alice, new_user_share(), &[file.id], &[bob, mallory])
            .await,
        Err(DbError::NotFound)
    ));
    let share = db
        .create_user_share(alice, new_user_share(), &[file.id], &[bob])
        .await
        .expect("share");
    assert!(matches!(
        db.update_share(
            alice,
            share.id,
            ShareUpdate {
                recipients: Some(&[mallory]),
                ..ShareUpdate::default()
            },
        )
        .await,
        Err(DbError::NotFound)
    ));
    let recipients = db.share_recipients(&[share.id]).await.expect("recipients");
    assert_eq!(recipients[&share.id].len(), 1);
    assert_eq!(recipients[&share.id][0].user_id, bob);

    let link = db
        .create_share_link(alice, new_share(TOKEN), &[file.id])
        .await
        .expect("link");
    assert_eq!(
        db.share_by_token(TenantId::DEFAULT, TOKEN)
            .await
            .expect("by token")
            .id,
        link.id
    );
    assert!(matches!(
        db.share_by_token(other, TOKEN).await,
        Err(DbError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn changing_recipients_keeps_when_the_others_were_added(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let bob = user(&db, "bob").await;
    let carol = user(&db, "carol").await;
    let dave = user(&db, "dave").await;
    let file = node(&db, alice, None, NodeKind::File).await;
    let share = db
        .create_user_share(alice, new_user_share(), &[file.id], &[bob, carol])
        .await
        .expect("share");
    let before = db.share_recipients(&[share.id]).await.expect("before");
    db.update_share(
        alice,
        share.id,
        ShareUpdate {
            recipients: Some(&[carol, dave]),
            ..ShareUpdate::default()
        },
    )
    .await
    .expect("update");
    let after = db.share_recipients(&[share.id]).await.expect("after");
    let names: Vec<&str> = after[&share.id]
        .iter()
        .map(|recipient| recipient.username.as_str())
        .collect();
    assert_eq!(names, ["carol", "dave"]);
    assert_eq!(
        after[&share.id][0].created_at,
        before[&share.id][1].created_at
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_owner_lists_links_and_shares_with_users(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let bob = user(&db, "bob").await;
    let file = node(&db, alice, None, NodeKind::File).await;
    db.create_share_link(alice, new_share(TOKEN), &[file.id])
        .await
        .expect("link");
    db.create_user_share(alice, new_user_share(), &[file.id], &[bob])
        .await
        .expect("users");
    let shares = db.owned_shares(alice, Some(file.id)).await.expect("list");
    assert_eq!(shares.len(), 2);
    assert_eq!(
        shares.iter().filter(|share| share.token.is_some()).count(),
        1
    );
}
