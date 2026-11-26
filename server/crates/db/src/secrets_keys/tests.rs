use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::*;
use crate::{FOLDER_MIME, NewNode, NewProtection, NodeKind, PasswordLock};

const RECIPIENT: &str = "age1fauk43xvke3myu0vg0syaflnwhlslwxd2504jfdjwejkjc50pcysgcf5gf";

async fn user(db: &Db, tenant: TenantId, name: &str) -> UserId {
    db.create_user(tenant, name, "hash").await.expect("user").id
}

async fn app(db: &Db, tenant: TenantId, user: UserId, sealed: &[u8]) {
    db.start_totp(tenant, user, sealed).await.expect("totp");
}

async fn password_folder(db: &Db, owner: UserId, sealed: &[u8]) -> NodeId {
    let new = NewNode {
        id: NodeId::new(),
        parent_id: None,
        kind: NodeKind::Folder,
        name: format!("papers-{}", Uuid::now_v7()),
        size: 0,
        sha256: None,
        mime: FOLDER_MIME.to_owned(),
    };
    let node = db.create_node(owner, new).await.expect("folder").id;
    let lock = NewProtection::Password(PasswordLock {
        password_hash: "$argon2id$...",
        recovery_hash: "$argon2id$...",
        sealed_key: sealed,
        recipient: RECIPIENT,
    });
    db.protect_folder(owner, node, lock)
        .await
        .expect("db")
        .expect("protected");
    node
}

async fn session(db: &Db, user: UserId) -> Uuid {
    let id = Uuid::now_v7();
    db.start_session(
        id,
        user,
        None,
        &crate::sessions::signed_in_now(),
        id.as_bytes(),
        OffsetDateTime::now_utc() + Duration::days(1),
    )
    .await
    .expect("session");
    id
}

async fn sessions_of(db: &Db, user: UserId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM sessions WHERE user_id = $1")
        .bind(user)
        .fetch_one(db.pool())
        .await
        .expect("count")
}

/// `sealed` with every value replaced by `with`.
fn resealed(sealed: &SealedSecrets, with: &[u8]) -> SealedSecrets {
    SealedSecrets {
        totp: sealed
            .totp
            .iter()
            .map(|row| SealedTotp {
                sealed: with.to_vec(),
                ..row.clone()
            })
            .collect(),
        folders: sealed
            .folders
            .iter()
            .map(|row| SealedFolderKey {
                sealed: with.to_vec(),
                ..row.clone()
            })
            .collect(),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_check_is_recorded_once(pool: PgPool) {
    let db = Db::from_pool(pool);
    assert_eq!(
        db.secrets_key_check(TenantId::DEFAULT).await.expect("db"),
        None
    );
    assert!(
        db.record_secrets_key_check(TenantId::DEFAULT, b"one")
            .await
            .expect("db")
    );
    assert!(
        !db.record_secrets_key_check(TenantId::DEFAULT, b"two")
            .await
            .expect("db")
    );
    assert_eq!(
        db.secrets_key_check(TenantId::DEFAULT).await.expect("db"),
        Some(b"one".to_vec())
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn rotation_reseals_the_tenants_secrets_and_signs_its_users_out(pool: PgPool) {
    let db = Db::from_pool(pool);
    let other_tenant = crate::tenants::add_test_tenant(&db).await;
    let alice = user(&db, TenantId::DEFAULT, "alice").await;
    let bob = user(&db, TenantId::DEFAULT, "bob").await;
    let carol = user(&db, other_tenant, "carol").await;
    app(&db, TenantId::DEFAULT, alice, b"old").await;
    app(&db, other_tenant, carol, b"old").await;
    let papers = password_folder(&db, bob, b"old").await;
    let carols = password_folder(&db, carol, b"old").await;
    for who in [alice, bob, carol] {
        session(&db, who).await;
    }

    let rotation = db
        .begin_secrets_rotation(TenantId::DEFAULT)
        .await
        .expect("begin");
    assert_eq!(rotation.check, None);
    assert_eq!(rotation.sealed.totp.len(), 1);
    assert_eq!(rotation.sealed.folders.len(), 1);
    let new = resealed(&rotation.sealed, b"new");
    assert_eq!(rotation.finish(b"check", &new).await.expect("finish"), 2);

    let sealed = db.sealed_secrets(TenantId::DEFAULT).await.expect("db");
    assert_eq!(
        sealed.totp,
        vec![SealedTotp {
            user: alice,
            sealed: b"new".to_vec()
        }]
    );
    assert_eq!(
        sealed.folders,
        vec![SealedFolderKey {
            node: papers,
            owner: bob,
            sealed: b"new".to_vec()
        }]
    );
    assert_eq!(
        db.secrets_key_check(TenantId::DEFAULT).await.expect("db"),
        Some(b"check".to_vec())
    );
    assert_eq!(sessions_of(&db, alice).await, 0);
    assert_eq!(sessions_of(&db, bob).await, 0);

    // The other tenant keeps its key, its secrets and its sessions.
    let others = db.sealed_secrets(other_tenant).await.expect("db");
    assert_eq!(others.totp[0].sealed, b"old");
    assert_eq!(others.folders[0].node, carols);
    assert_eq!(others.folders[0].sealed, b"old");
    assert_eq!(db.secrets_key_check(other_tenant).await.expect("db"), None);
    assert_eq!(sessions_of(&db, carol).await, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_rotation_that_leaves_something_out_keeps_nothing(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, TenantId::DEFAULT, "alice").await;
    app(&db, TenantId::DEFAULT, alice, b"old").await;
    password_folder(&db, alice, b"old").await;
    session(&db, alice).await;

    let rotation = db
        .begin_secrets_rotation(TenantId::DEFAULT)
        .await
        .expect("begin");
    let mut partial = resealed(&rotation.sealed, b"new");
    partial.folders.clear();
    assert!(matches!(
        rotation.finish(b"check", &partial).await,
        Err(DbError::Stale)
    ));

    let sealed = db.sealed_secrets(TenantId::DEFAULT).await.expect("db");
    assert_eq!(sealed.totp[0].sealed, b"old");
    assert_eq!(sealed.folders[0].sealed, b"old");
    assert_eq!(
        db.secrets_key_check(TenantId::DEFAULT).await.expect("db"),
        None
    );
    assert_eq!(sessions_of(&db, alice).await, 1);
}
