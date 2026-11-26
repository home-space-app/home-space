use sqlx::PgPool;

use super::*;
use crate::{FOLDER_MIME, NewCredential, NewNode, NodeKind, TenantId};

const RECIPIENT: &str = "age1fauk43xvke3myu0vg0syaflnwhlslwxd2504jfdjwejkjc50pcysgcf5gf";
const WRAPPED: [u8; 73] = [7; 73];

async fn user(db: &Db, name: &str) -> UserId {
    db.create_user(TenantId::DEFAULT, name, "hash")
        .await
        .expect("user")
        .id
}

async fn key(db: &Db, user: UserId, credential_id: &[u8]) -> Uuid {
    let id = Uuid::now_v7();
    let new = NewCredential {
        id,
        credential_id,
        credential: r#"{"counter":0}"#,
        kind: "passkey",
        rp_id: "files.example.com",
        name: "YubiKey",
    };
    db.add_webauthn_credential(TenantId::DEFAULT, user, new, &[b"a".to_vec()])
        .await
        .expect("key");
    id
}

fn security_key(credential: Uuid, epoch: i32) -> NewUnlocker<'static> {
    NewUnlocker {
        id: Uuid::now_v7(),
        kind: "security_key",
        credential: Some(credential),
        prf_salt: Some(&[3; 32]),
        wrapped_vault_key: &WRAPPED,
        epoch,
    }
}

fn recovery(epoch: i32) -> NewUnlocker<'static> {
    NewUnlocker {
        id: Uuid::now_v7(),
        kind: "recovery",
        credential: None,
        prf_salt: None,
        wrapped_vault_key: &WRAPPED,
        epoch,
    }
}

async fn folder(db: &Db, owner: UserId, parent: Option<NodeId>, name: &str) -> NodeId {
    let new = NewNode {
        id: NodeId::new(),
        parent_id: parent,
        kind: NodeKind::Folder,
        name: name.to_owned(),
        size: 0,
        sha256: None,
        mime: FOLDER_MIME.to_owned(),
    };
    db.create_node(owner, new).await.expect("folder").id
}

fn with_key(epoch: i32) -> KeyLock<'static> {
    KeyLock {
        wrapped_secret: &WRAPPED,
        epoch,
        recipient: RECIPIENT,
    }
}

/// A folder locked with a password, as the server will store one.
async fn password_folder(db: &Db, owner: UserId, node: NodeId) {
    sqlx::query(
        "INSERT INTO protected_folders (node_id, owner_id, lock, password_hash, sealed_key, state) \
         VALUES ($1, $2, 'password', '$argon2id$...', '\\x00', 'protected')",
    )
    .bind(node)
    .bind(owner)
    .execute(db.pool())
    .await
    .expect("password folder");
}

/// Gives `user` a vault at epoch 1.
async fn vault(db: &Db, user: UserId) {
    db.add_vault_unlocker(user, recovery(1))
        .await
        .expect("vault");
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_first_unlocker_makes_the_vault_and_the_rest_need_its_epoch(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let yubikey = key(&db, alice, b"one").await;
    assert!(db.vault(alice).await.expect("vault").is_none());
    assert!(matches!(
        db.add_vault_unlocker(alice, security_key(yubikey, 2)).await,
        Err(DbError::Stale)
    ));
    db.add_vault_unlocker(alice, security_key(yubikey, 1))
        .await
        .expect("first");
    assert!(matches!(
        db.add_vault_unlocker(alice, security_key(yubikey, 1)).await,
        Err(DbError::Conflict)
    ));
    db.add_vault_unlocker(alice, recovery(1))
        .await
        .expect("recovery");
    // A new recovery key replaces the old one.
    let replacing = recovery(1);
    db.add_vault_unlocker(alice, replacing)
        .await
        .expect("again");
    let vault = db.vault(alice).await.expect("vault").expect("some");
    assert_eq!(vault.epoch, 1);
    assert_eq!(vault.unlockers.len(), 2);
    assert_eq!(
        vault.unlockers[0].credential_id.as_deref(),
        Some(&b"one"[..])
    );
    assert_eq!(
        vault.unlockers[0].rp_id.as_deref(),
        Some("files.example.com")
    );
    assert_eq!(vault.unlockers[1].id, replacing.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_the_users_own_credentials_unlock_their_vault(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let bob = user(&db, "bob").await;
    let bobs = key(&db, bob, b"bob").await;
    assert!(matches!(
        db.add_vault_unlocker(alice, security_key(bobs, 1)).await,
        Err(DbError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_last_unlocker_of_locked_folders_stays(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let yubikey = key(&db, alice, b"one").await;
    let spare = key(&db, alice, b"two").await;
    let first = security_key(yubikey, 1);
    db.add_vault_unlocker(alice, first).await.expect("first");
    let papers = folder(&db, alice, None, "Papers").await;
    db.protect_folder(alice, papers, with_key(1))
        .await
        .expect("db")
        .expect("protected");

    assert!(matches!(
        db.remove_vault_unlocker(alice, first.id).await,
        Err(DbError::Conflict)
    ));
    assert!(matches!(
        db.remove_webauthn_credential(alice, yubikey).await,
        Err(DbError::Conflict)
    ));
    let second = security_key(spare, 1);
    db.add_vault_unlocker(alice, second).await.expect("second");
    // With another key, removing the sign-in credential removes its unlocker too.
    db.remove_webauthn_credential(alice, yubikey)
        .await
        .expect("removed");
    let vault = db.vault(alice).await.expect("vault").expect("some");
    assert_eq!(vault.unlockers.len(), 1);
    assert_eq!(vault.unlockers[0].id, second.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_vault_without_folders_goes_with_its_last_unlocker(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let yubikey = key(&db, alice, b"one").await;
    db.add_vault_unlocker(alice, security_key(yubikey, 1))
        .await
        .expect("first");
    db.remove_webauthn_credential(alice, yubikey)
        .await
        .expect("removed");
    assert!(db.vault(alice).await.expect("vault").is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn rotation_rewraps_everything_or_nothing(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let one = key(&db, alice, b"one").await;
    let two = key(&db, alice, b"two").await;
    let kept = security_key(one, 1);
    let lost = security_key(two, 1);
    db.add_vault_unlocker(alice, kept).await.expect("kept");
    db.add_vault_unlocker(alice, lost).await.expect("lost");
    let papers = folder(&db, alice, None, "Papers").await;
    db.protect_folder(alice, papers, with_key(1))
        .await
        .expect("db")
        .expect("protected");
    let notes = folder(&db, alice, None, "Notes").await;
    password_folder(&db, alice, notes).await;

    let rewrapped = |id: Uuid| Rewrapped {
        id,
        prf_salt: None,
        wrapped: vec![9; 73],
    };
    // A folder left out, or an unlocker that is neither kept nor removed, is refused.
    assert!(matches!(
        db.rotate_vault(alice, 1, &[rewrapped(kept.id)], &[], &[lost.id])
            .await,
        Err(DbError::Invalid)
    ));
    assert!(matches!(
        db.rotate_vault(alice, 1, &[rewrapped(kept.id)], &[rewrapped(papers.0)], &[])
            .await,
        Err(DbError::Invalid)
    ));
    // The password folder is not the vault's.
    assert!(matches!(
        db.rotate_vault(
            alice,
            1,
            &[rewrapped(kept.id)],
            &[rewrapped(papers.0), rewrapped(notes.0)],
            &[lost.id]
        )
        .await,
        Err(DbError::Invalid)
    ));
    assert!(matches!(
        db.rotate_vault(alice, 1, &[], &[rewrapped(papers.0)], &[kept.id, lost.id])
            .await,
        Err(DbError::Conflict)
    ));
    let epoch = db
        .rotate_vault(
            alice,
            1,
            &[rewrapped(kept.id)],
            &[rewrapped(papers.0)],
            &[lost.id],
        )
        .await
        .expect("rotated");
    assert_eq!(epoch, 2);
    assert!(matches!(
        db.rotate_vault(alice, 1, &[rewrapped(kept.id)], &[rewrapped(papers.0)], &[])
            .await,
        Err(DbError::Stale)
    ));
    let vault = db.vault(alice).await.expect("vault").expect("some");
    assert_eq!(vault.epoch, 2);
    assert_eq!(vault.unlockers.len(), 1);
    assert_eq!(vault.unlockers[0].wrapped_vault_key, vec![9; 73]);
    assert_eq!(vault.unlockers[0].prf_salt, Some(vec![3; 32]));
    let protected = db
        .protected_folder(alice, papers)
        .await
        .expect("db")
        .expect("some");
    assert_eq!(protected.epoch, Some(2));
    assert_eq!(protected.wrapped_secret, Some(vec![9; 73]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn protection_never_overlaps_or_takes_special_folders(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let outer = folder(&db, alice, None, "Outer").await;
    let inner = folder(&db, alice, Some(outer), "Inner").await;

    let refused = |result: Result<Result<StoredProtection, ProtectRefusal>, DbError>| {
        result.expect("db").expect_err("refused")
    };
    // No vault yet.
    assert_eq!(
        refused(db.protect_folder(alice, inner, with_key(1)).await),
        ProtectRefusal::VaultChanged
    );
    vault(&db, alice).await;
    assert_eq!(
        refused(db.protect_folder(alice, inner, with_key(2)).await),
        ProtectRefusal::VaultChanged
    );
    let stored = db
        .protect_folder(alice, inner, with_key(1))
        .await
        .expect("db")
        .expect("inner");
    assert_eq!(stored.state, "protected");
    assert_eq!(
        refused(db.protect_folder(alice, inner, with_key(1)).await),
        ProtectRefusal::Overlaps
    );
    assert_eq!(
        refused(db.protect_folder(alice, outer, with_key(1)).await),
        ProtectRefusal::Overlaps
    );
    let special = db.special_folders(alice).await.expect("special");
    assert_eq!(
        refused(
            db.protect_folder(alice, special.lost_and_found_id, with_key(1))
                .await
        ),
        ProtectRefusal::SpecialFolder
    );
    let bob = user(&db, "bob").await;
    assert_eq!(
        refused(db.protect_folder(bob, outer, with_key(1)).await),
        ProtectRefusal::NotFound
    );

    // A folder with content waits for its conversion.
    let other = folder(&db, alice, None, "Other").await;
    folder(&db, alice, Some(other), "Child").await;
    let stored = db
        .protect_folder(alice, other, with_key(1))
        .await
        .expect("db")
        .expect("other");
    assert_eq!(stored.state, "protecting");
    assert_eq!(db.protected_folders(alice).await.expect("list").len(), 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_conversion_finishes_or_is_cancelled_but_a_protected_folder_stays(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    vault(&db, alice).await;
    let papers = folder(&db, alice, None, "Papers").await;
    folder(&db, alice, Some(papers), "Old").await;
    let stored = db
        .protect_folder(alice, papers, with_key(1))
        .await
        .expect("db")
        .expect("protecting");
    assert_eq!(stored.state, "protecting");
    let finished = db
        .set_protection_state(alice, papers, &["protecting"], "protected")
        .await
        .expect("finished");
    assert_eq!(finished.state, "protected");
    assert!(matches!(
        db.set_protection_state(alice, papers, &["protecting"], "protected")
            .await,
        Err(DbError::Stale)
    ));
    // A protected folder is never just forgotten: its secret would be gone with it.
    assert!(matches!(
        db.remove_protection(alice, papers, &["protecting", "unprotecting"])
            .await,
        Err(DbError::Stale)
    ));
    db.set_protection_state(alice, papers, &["protected"], "unprotecting")
        .await
        .expect("unprotecting");
    db.remove_protection(alice, papers, &["protecting", "unprotecting"])
        .await
        .expect("removed");
    assert!(matches!(
        db.remove_protection(alice, papers, &["unprotecting"]).await,
        Err(DbError::NotFound)
    ));
}
