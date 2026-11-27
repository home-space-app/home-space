use protocol::ProtectionState;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use super::*;
use crate::{
    Db, DbError, FOLDER_MIME, NewContent, NewCredential, NewNode, NodeId, NodeKind, TenantId,
    UserId,
};

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
        kind: protocol::WebAuthnKind::Passkey,
        rp_id: "files.example.com",
        name: "YubiKey",
    };
    db.add_webauthn_credential(TenantId::DEFAULT, user, new, &[b"a".to_vec()])
        .await
        .expect("key");
    id
}

/// A folder key whose credential has the id `credential_id`.
fn security_key(credential_id: &'static [u8], epoch: i32) -> NewUnlocker<'static> {
    NewUnlocker {
        id: Uuid::now_v7(),
        source: UnlockerSource::SecurityKey(FolderKey {
            name: "YubiKey",
            credential_id,
            rp_id: "files.example.com",
            prf_salt: &[3; 32],
        }),
        wrapped_vault_key: &WRAPPED,
        epoch,
    }
}

fn recovery(epoch: i32) -> NewUnlocker<'static> {
    NewUnlocker {
        id: Uuid::now_v7(),
        source: UnlockerSource::Recovery,
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

fn with_key(epoch: i32) -> NewProtection<'static> {
    NewProtection::SecurityKey(KeyLock {
        wrapped_secret: &WRAPPED,
        epoch,
        recipient: RECIPIENT,
    })
}

fn with_password() -> NewProtection<'static> {
    NewProtection::Password(PasswordLock {
        password_hash: "$argon2id$...",
        recovery_hash: "$argon2id$...",
        sealed_key: &[1, 2, 3],
        recipient: RECIPIENT,
    })
}

/// A folder locked with a password.
async fn password_folder(db: &Db, owner: UserId, node: NodeId) {
    db.protect_folder(owner, node, with_password())
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
    let yubikey: &[u8] = b"one";
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
    assert_eq!(vault.unlockers[0].name.as_deref(), Some("YubiKey"));
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
async fn folder_keys_are_apart_from_sign_in_credentials(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let bob = user(&db, "bob").await;
    let sign_in = key(&db, alice, b"one").await;
    db.add_vault_unlocker(alice, security_key(b"one", 1))
        .await
        .expect("alice");
    // Credential ids are unique per user only: the server cannot tell whose key made one.
    db.add_vault_unlocker(bob, security_key(b"one", 1))
        .await
        .expect("bob");
    db.remove_webauthn_credential(alice, sign_in)
        .await
        .expect("removed");
    db.reset_second_factors(TenantId::DEFAULT, alice)
        .await
        .expect("reset");
    let vault = db.vault(alice).await.expect("vault").expect("some");
    assert_eq!(vault.unlockers.len(), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_last_unlocker_of_locked_folders_stays(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let first = security_key(b"one", 1);
    db.add_vault_unlocker(alice, first).await.expect("first");
    let papers = folder(&db, alice, None, "Papers").await;
    db.protect_folder(alice, papers, with_key(1))
        .await
        .expect("protected");

    assert!(matches!(
        db.remove_vault_unlocker(alice, first.id).await,
        Err(DbError::Conflict)
    ));
    let second = security_key(b"two", 1);
    db.add_vault_unlocker(alice, second).await.expect("second");
    db.remove_vault_unlocker(alice, first.id)
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
    let only = security_key(b"one", 1);
    db.add_vault_unlocker(alice, only).await.expect("first");
    db.remove_vault_unlocker(alice, only.id)
        .await
        .expect("removed");
    assert!(db.vault(alice).await.expect("vault").is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn rotation_rewraps_everything_or_nothing(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let kept = security_key(b"one", 1);
    let lost = security_key(b"two", 1);
    db.add_vault_unlocker(alice, kept).await.expect("kept");
    db.add_vault_unlocker(alice, lost).await.expect("lost");
    let papers = folder(&db, alice, None, "Papers").await;
    db.protect_folder(alice, papers, with_key(1))
        .await
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

    let refused = |result: Result<StoredProtection, ProtectError>| match result {
        Err(ProtectError::Refused(refusal)) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
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
        .expect("inner");
    assert_eq!(stored.state, ProtectionState::Protected);
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
        .expect("other");
    assert_eq!(stored.state, ProtectionState::Protecting);
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
        .expect("protecting");
    assert_eq!(stored.state, ProtectionState::Protecting);
    let finished = db
        .set_protection_state(
            alice,
            papers,
            &[ProtectionState::Protecting],
            ProtectionState::Protected,
        )
        .await
        .expect("finished");
    assert_eq!(finished.state, ProtectionState::Protected);
    assert!(matches!(
        db.set_protection_state(
            alice,
            papers,
            &[ProtectionState::Protecting],
            ProtectionState::Protected
        )
        .await,
        Err(DbError::Stale)
    ));
    // A protected folder is never just forgotten: its secret would be gone with it.
    assert!(matches!(
        db.remove_protection(
            alice,
            papers,
            &[ProtectionState::Protecting, ProtectionState::Unprotecting]
        )
        .await,
        Err(DbError::Stale)
    ));
    db.set_protection_state(
        alice,
        papers,
        &[ProtectionState::Protected],
        ProtectionState::Unprotecting,
    )
    .await
    .expect("unprotecting");
    db.remove_protection(
        alice,
        papers,
        &[ProtectionState::Protecting, ProtectionState::Unprotecting],
    )
    .await
    .expect("removed");
    assert!(matches!(
        db.remove_protection(alice, papers, &[ProtectionState::Unprotecting])
            .await,
        Err(DbError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_password_folder_is_found_from_inside_and_lists_its_files(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let notes = folder(&db, alice, None, "Notes").await;
    let inner = folder(&db, alice, Some(notes), "2026").await;
    let content = |byte: u8| NewContent {
        size: 1,
        sha256: vec![byte],
        mime: "text/plain".to_owned(),
        modified_at: None,
    };
    let mut tx = db.transaction(alice).await.expect("tx");
    let file = tx
        .insert_file(Some(inner), "a.txt", &content(1))
        .await
        .expect("file");
    tx.commit().await.expect("commit");
    assert!(
        db.enclosing_password_folder(alice, file.id)
            .await
            .expect("db")
            .is_none()
    );

    password_folder(&db, alice, notes).await;
    let found = db
        .enclosing_password_folder(alice, file.id)
        .await
        .expect("db")
        .expect("inside");
    assert_eq!(found.node_id, notes);
    assert_eq!(found.state, ProtectionState::Protecting);
    assert_eq!(found.recipient, RECIPIENT);
    let files = db.folder_files(alice, notes).await.expect("files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].node_id, file.id);
    assert!(files[0].versions.is_empty());
    let converting = db
        .converting_password_folders(TenantId::DEFAULT)
        .await
        .expect("converting");
    assert_eq!(
        converting,
        vec![(alice, notes, ProtectionState::Protecting)]
    );
    let secrets = db
        .password_secrets(alice, notes)
        .await
        .expect("db")
        .expect("secrets");
    assert_eq!(secrets.sealed_key, vec![1, 2, 3]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_unlock_belongs_to_one_session_and_expires(pool: PgPool) {
    let db = Db::from_pool(pool.clone());
    let alice = user(&db, "alice").await;
    let notes = folder(&db, alice, None, "Notes").await;
    password_folder(&db, alice, notes).await;
    let session = |id: Uuid| {
        sqlx::query(
            "INSERT INTO sessions (id, user_id, expires_at, auth_methods, authenticated_at) \
             VALUES ($1, $2, now() + interval '1 day', '{pwd}', now())",
        )
        .bind(id)
        .bind(alice)
        .execute(&pool)
    };
    let laptop = Uuid::now_v7();
    let phone = Uuid::now_v7();
    session(laptop).await.expect("laptop");
    session(phone).await.expect("phone");
    let later = OffsetDateTime::now_utc() + time::Duration::minutes(15);
    assert!(
        !db.use_unlocked_folder(laptop, notes, later)
            .await
            .expect("db")
    );
    db.unlock_folder(laptop, notes, later)
        .await
        .expect("unlock");
    assert!(
        db.use_unlocked_folder(laptop, notes, later)
            .await
            .expect("db")
    );
    assert!(
        !db.use_unlocked_folder(phone, notes, later)
            .await
            .expect("db")
    );
    db.lock_folder(laptop, notes).await.expect("lock");
    assert!(
        !db.use_unlocked_folder(laptop, notes, later)
            .await
            .expect("db")
    );

    let past = OffsetDateTime::now_utc() - time::Duration::minutes(1);
    db.unlock_folder(phone, notes, past).await.expect("unlock");
    assert!(
        !db.use_unlocked_folder(phone, notes, later)
            .await
            .expect("db")
    );
    assert_eq!(db.expire_folder_unlocks().await.expect("expire"), 1);
}

/// Security: protecting a folder waits for a store into it that is under way, and then sees
/// what it stored, so the converter seals it; the store cannot land after the folder went
/// straight to `protected` as an empty one.
#[sqlx::test(migrations = "../../migrations")]
async fn protecting_waits_for_a_store_in_progress_and_sees_what_it_stored(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let docs = folder(&db, alice, None, "docs").await;
    let mut store = db.transaction(alice).await.expect("tx");
    store.hold_protections().await.expect("hold");
    assert!(
        store
            .password_folder_of(Some(docs))
            .await
            .expect("read")
            .is_none()
    );
    let content = NewContent {
        size: 1,
        sha256: vec![1; 32],
        mime: "text/plain".to_owned(),
        modified_at: None,
    };
    store
        .insert_file(Some(docs), "plain.txt", &content)
        .await
        .expect("file");

    let protecting = tokio::spawn({
        let db = db.clone();
        async move { db.protect_folder(alice, docs, with_password()).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!protecting.is_finished(), "protecting waits for the store");
    store.commit().await.expect("commit");

    let protected = protecting.await.expect("task").expect("protected");
    assert_eq!(
        protected.state,
        ProtectionState::Protecting,
        "the folder is no longer empty"
    );
}

/// A store that holds the protections reads the protection as it is, and keeps it so.
#[sqlx::test(migrations = "../../migrations")]
async fn a_store_after_protecting_sees_the_password_folder(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let docs = folder(&db, alice, None, "docs").await;
    let inner = folder(&db, alice, Some(docs), "inner").await;
    password_folder(&db, alice, docs).await;
    let mut store = db.transaction(alice).await.expect("tx");
    store.hold_protections().await.expect("hold");
    let holder = store
        .password_folder_of(Some(inner))
        .await
        .expect("read")
        .expect("inside");
    assert_eq!(holder.node_id, docs);
    assert!(
        store
            .password_folder_of(None)
            .await
            .expect("root")
            .is_none()
    );
}
