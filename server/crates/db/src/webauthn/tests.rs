use sqlx::PgPool;
use time::Duration;

use super::*;

async fn user(db: &Db, tenant: TenantId, name: &str) -> UserId {
    db.create_user(tenant, name, "hash").await.expect("user").id
}

fn key<'a>(id: Uuid, credential_id: &'a [u8], kind: &'a str) -> NewCredential<'a> {
    NewCredential {
        id,
        credential_id,
        credential: r#"{"counter":0}"#,
        kind,
        rp_id: "files.example.com",
        name: "YubiKey",
    }
}

fn codes() -> Vec<Vec<u8>> {
    vec![b"a".to_vec(), b"b".to_vec()]
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_first_key_brings_recovery_codes_and_the_last_takes_them(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, TenantId::DEFAULT, "alice").await;
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    assert!(
        db.add_webauthn_credential(
            TenantId::DEFAULT,
            alice,
            key(first, b"one", "passkey"),
            &codes()
        )
        .await
        .expect("add")
    );
    assert!(
        !db.add_webauthn_credential(
            TenantId::DEFAULT,
            alice,
            key(second, b"two", "security_key"),
            &[b"c".to_vec()]
        )
        .await
        .expect("add")
    );
    let state = db.second_factors(alice).await.expect("db");
    assert_eq!((state.webauthn, state.recovery_codes_left), (2, 2));
    assert!(state.any());
    let accounts = db.accounts(TenantId::DEFAULT).await.expect("accounts");
    assert!(accounts[0].webauthn && !accounts[0].authenticator_app);

    db.rename_webauthn_credential(alice, first, "Phone")
        .await
        .expect("rename");
    let stored = db.webauthn_credentials(alice).await.expect("db");
    assert_eq!(stored[0].name, "Phone");
    assert_eq!(stored[0].credential, r#"{"counter": 0}"#);

    db.remove_webauthn_credential(alice, first)
        .await
        .expect("remove");
    assert_eq!(
        db.second_factors(alice)
            .await
            .expect("db")
            .recovery_codes_left,
        2
    );
    db.remove_webauthn_credential(alice, second)
        .await
        .expect("remove");
    assert_eq!(
        db.second_factors(alice).await.expect("db"),
        SecondFactorState::default()
    );
    assert!(matches!(
        db.remove_webauthn_credential(alice, second).await,
        Err(DbError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn removing_the_app_keeps_the_codes_while_a_key_remains(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, TenantId::DEFAULT, "alice").await;
    db.add_webauthn_credential(
        TenantId::DEFAULT,
        alice,
        key(Uuid::now_v7(), b"one", "passkey"),
        &codes(),
    )
    .await
    .expect("add");
    db.start_totp(TenantId::DEFAULT, alice, b"secret")
        .await
        .expect("start");
    assert!(
        !db.confirm_totp(TenantId::DEFAULT, alice, 10, &[b"x".to_vec()])
            .await
            .expect("confirm"),
        "a second factor brings no new codes"
    );
    db.remove_totp(alice).await.expect("remove");
    let state = db.second_factors(alice).await.expect("db");
    assert_eq!(
        (state.totp, state.webauthn, state.recovery_codes_left),
        (false, 1, 2)
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_user_has_at_most_twenty_keys_each_registered_once_per_tenant(pool: PgPool) {
    let db = Db::from_pool(pool);
    let other = crate::tenants::add_test_tenant(&db).await;
    let alice = user(&db, TenantId::DEFAULT, "alice").await;
    let bob = user(&db, other, "bob").await;
    for n in 0..MAX_WEBAUTHN_CREDENTIALS {
        let id = n.to_be_bytes();
        db.add_webauthn_credential(
            TenantId::DEFAULT,
            alice,
            key(Uuid::now_v7(), &id, "passkey"),
            &[],
        )
        .await
        .expect("add");
    }
    assert!(matches!(
        db.add_webauthn_credential(
            TenantId::DEFAULT,
            alice,
            key(Uuid::now_v7(), b"more", "passkey"),
            &[]
        )
        .await,
        Err(DbError::Conflict)
    ));
    let taken = 0_i64.to_be_bytes();
    db.add_webauthn_credential(other, bob, key(Uuid::now_v7(), &taken, "passkey"), &[])
        .await
        .expect("another tenant may hold the same credential id");
    let found = db
        .webauthn_credential_by_id(other, &taken)
        .await
        .expect("db")
        .expect("found");
    assert_eq!(found.user_id, bob);
    assert!(
        db.webauthn_credential_by_id(other, b"none")
            .await
            .expect("db")
            .is_none()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_handle_is_made_once_and_finds_the_user_only_in_their_tenant(pool: PgPool) {
    let db = Db::from_pool(pool);
    let other = crate::tenants::add_test_tenant(&db).await;
    let alice = user(&db, TenantId::DEFAULT, "alice").await;
    let handle = db
        .webauthn_handle(alice, Uuid::now_v7())
        .await
        .expect("handle");
    assert_eq!(
        db.webauthn_handle(alice, Uuid::now_v7())
            .await
            .expect("handle"),
        handle
    );
    assert_eq!(
        db.user_by_webauthn_handle(TenantId::DEFAULT, handle)
            .await
            .expect("db"),
        Some(alice)
    );
    assert_eq!(
        db.user_by_webauthn_handle(other, handle).await.expect("db"),
        None
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_ceremony_is_answered_once_by_its_kind_and_tenant_before_it_expires(pool: PgPool) {
    let db = Db::from_pool(pool.clone());
    let other = crate::tenants::add_test_tenant(&db).await;
    let alice = user(&db, TenantId::DEFAULT, "alice").await;
    let id = Uuid::now_v7();
    let later = OffsetDateTime::now_utc() + Duration::minutes(5);
    let ceremony = |id, expires_at| NewCeremony {
        id,
        user: Some(alice),
        kind: "add_passkey",
        origin: "https://files.example.com",
        state: r#"{"challenge":"x"}"#,
        expires_at,
    };
    db.insert_webauthn_ceremony(TenantId::DEFAULT, ceremony(id, later))
        .await
        .expect("insert");
    let old = Uuid::now_v7();
    db.insert_webauthn_ceremony(
        TenantId::DEFAULT,
        ceremony(old, OffsetDateTime::now_utc() - Duration::seconds(1)),
    )
    .await
    .expect("insert");

    assert!(
        db.take_webauthn_ceremony(other, id, &["add_passkey"])
            .await
            .expect("db")
            .is_none()
    );
    assert!(
        db.take_webauthn_ceremony(TenantId::DEFAULT, id, &["sign_in"])
            .await
            .expect("db")
            .is_none()
    );
    assert!(
        db.take_webauthn_ceremony(TenantId::DEFAULT, old, &["add_passkey"])
            .await
            .expect("db")
            .is_none()
    );
    let taken = db
        .take_webauthn_ceremony(TenantId::DEFAULT, id, &["add_passkey"])
        .await
        .expect("db")
        .expect("waiting");
    assert_eq!(taken.user_id, Some(alice));
    assert_eq!(taken.origin, "https://files.example.com");
    assert!(
        db.take_webauthn_ceremony(TenantId::DEFAULT, id, &["add_passkey"])
            .await
            .expect("db")
            .is_none()
    );

    db.prune_sign_ins().await.expect("prune");
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM webauthn_ceremonies")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(left, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_administrators_reset_removes_the_keys(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, TenantId::DEFAULT, "alice").await;
    db.add_webauthn_credential(
        TenantId::DEFAULT,
        alice,
        key(Uuid::now_v7(), b"one", "passkey"),
        &codes(),
    )
    .await
    .expect("add");
    db.reset_second_factors(TenantId::DEFAULT, alice)
        .await
        .expect("reset");
    assert!(db.webauthn_credentials(alice).await.expect("db").is_empty());
    assert_eq!(
        db.second_factors(alice).await.expect("db"),
        SecondFactorState::default()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_tenant_has_passkeys_only_for_the_address_they_were_made_on(pool: PgPool) {
    let db = Db::from_pool(pool);
    let other = crate::tenants::add_test_tenant(&db).await;
    let alice = user(&db, TenantId::DEFAULT, "alice").await;
    let has = |tenant, rp_id| {
        let db = db.clone();
        async move { db.tenant_has_passkeys(tenant, rp_id).await.expect("db") }
    };
    db.add_webauthn_credential(
        TenantId::DEFAULT,
        alice,
        key(Uuid::now_v7(), b"key", "security_key"),
        &codes(),
    )
    .await
    .expect("add");
    assert!(!has(TenantId::DEFAULT, "files.example.com").await);

    db.add_webauthn_credential(
        TenantId::DEFAULT,
        alice,
        key(Uuid::now_v7(), b"passkey", "passkey"),
        &codes(),
    )
    .await
    .expect("add");
    assert!(has(TenantId::DEFAULT, "files.example.com").await);
    assert!(!has(TenantId::DEFAULT, "other.example.com").await);
    assert!(!has(other, "files.example.com").await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_use_is_recorded_only_while_the_counter_goes_up(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, TenantId::DEFAULT, "alice").await;
    let id = Uuid::now_v7();
    db.add_webauthn_credential(
        TenantId::DEFAULT,
        alice,
        key(id, b"key", "passkey"),
        &codes(),
    )
    .await
    .expect("add");
    let touch = |credential: Option<&'static str>, counter| {
        let db = db.clone();
        async move {
            db.touch_webauthn_credential(id, credential, counter)
                .await
                .expect("db")
        }
    };

    // An authenticator that keeps no counter answers 0 every time.
    assert!(touch(None, 0).await);
    assert!(touch(None, 0).await);
    assert!(touch(Some(r#"{"counter":5}"#), 5).await);
    assert!(!touch(Some(r#"{"counter":5}"#), 5).await);
    assert!(!touch(Some(r#"{"counter":4}"#), 4).await);
    assert!(!touch(None, 0).await);
    assert!(touch(Some(r#"{"counter":6}"#), 6).await);
    let stored = db.webauthn_credentials(alice).await.expect("db");
    assert_eq!(stored[0].credential, r#"{"counter": 6}"#);
    assert!(stored[0].last_used_at.is_some());
}
