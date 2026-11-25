use sqlx::PgPool;
use time::Duration;
use uuid::Uuid;

use super::*;

async fn user(db: &Db, tenant: TenantId, name: &str) -> UserId {
    db.create_user(tenant, name, "hash").await.expect("user").id
}

fn codes(names: &[&str]) -> Vec<Vec<u8>> {
    names.iter().map(|name| name.as_bytes().to_vec()).collect()
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_app_counts_once_it_is_confirmed_and_then_cannot_be_replaced(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, TenantId::DEFAULT, "alice").await;
    db.start_totp(TenantId::DEFAULT, alice, b"first")
        .await
        .expect("start");
    assert!(!db.second_factors(alice).await.expect("db").totp);
    db.start_totp(TenantId::DEFAULT, alice, b"second")
        .await
        .expect("an unconfirmed app is replaced");
    let stored = db.totp(alice).await.expect("db").expect("stored");
    assert_eq!(stored.secret, b"second");
    assert!(!db.accept_totp_step(alice, 5).await.expect("db"));

    db.confirm_totp(TenantId::DEFAULT, alice, 10, &codes(&["a", "b"]))
        .await
        .expect("confirm");
    let state = db.second_factors(alice).await.expect("db");
    assert!(state.totp);
    assert_eq!(state.recovery_codes_left, 2);
    assert!(matches!(
        db.start_totp(TenantId::DEFAULT, alice, b"third").await,
        Err(DbError::Conflict)
    ));
    assert!(matches!(
        db.confirm_totp(TenantId::DEFAULT, alice, 11, &[]).await,
        Err(DbError::NotFound)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_time_step_is_accepted_once_and_never_an_earlier_one(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, TenantId::DEFAULT, "alice").await;
    db.start_totp(TenantId::DEFAULT, alice, b"secret")
        .await
        .expect("start");
    db.confirm_totp(TenantId::DEFAULT, alice, 10, &[])
        .await
        .expect("confirm");
    assert!(!db.accept_totp_step(alice, 10).await.expect("db"));
    assert!(db.accept_totp_step(alice, 11).await.expect("db"));
    assert!(!db.accept_totp_step(alice, 11).await.expect("db"));
    assert!(!db.accept_totp_step(alice, 9).await.expect("db"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_recovery_code_works_once_and_a_new_set_voids_the_old(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, TenantId::DEFAULT, "alice").await;
    let bob = user(&db, TenantId::DEFAULT, "bob").await;
    db.replace_recovery_codes(TenantId::DEFAULT, alice, &codes(&["a", "b"]))
        .await
        .expect("codes");
    assert!(!db.use_recovery_code(bob, b"a").await.expect("db"));
    assert!(db.use_recovery_code(alice, b"a").await.expect("db"));
    assert!(!db.use_recovery_code(alice, b"a").await.expect("db"));
    assert_eq!(
        db.second_factors(alice)
            .await
            .expect("db")
            .recovery_codes_left,
        1
    );
    db.replace_recovery_codes(TenantId::DEFAULT, alice, &codes(&["c"]))
        .await
        .expect("new set");
    assert!(!db.use_recovery_code(alice, b"b").await.expect("db"));
    assert!(db.use_recovery_code(alice, b"c").await.expect("db"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_waiting_sign_in_is_found_only_by_its_tenant_and_finishes_once(pool: PgPool) {
    let db = Db::from_pool(pool.clone());
    let other = crate::tenants::add_test_tenant(&db).await;
    let alice = user(&db, TenantId::DEFAULT, "alice").await;
    let later = OffsetDateTime::now_utc() + Duration::minutes(5);
    db.insert_pending_login(TenantId::DEFAULT, b"token", alice, later)
        .await
        .expect("insert");
    db.insert_pending_login(
        TenantId::DEFAULT,
        b"old",
        alice,
        OffsetDateTime::now_utc() - Duration::seconds(1),
    )
    .await
    .expect("insert");
    assert!(
        db.pending_login(other, b"token")
            .await
            .expect("db")
            .is_none()
    );
    assert!(
        db.pending_login(TenantId::DEFAULT, b"old")
            .await
            .expect("db")
            .is_none()
    );
    let pending = db
        .pending_login(TenantId::DEFAULT, b"token")
        .await
        .expect("db")
        .expect("waiting");
    assert_eq!(
        pending,
        PendingLogin {
            user_id: alice,
            attempts: 0,
            webauthn_state: None,
            webauthn_origin: None,
        }
    );
    assert_eq!(
        db.count_pending_login_failure(b"token").await.expect("db"),
        Some(1)
    );
    assert!(db.take_pending_login(b"token").await.expect("db"));
    assert!(!db.take_pending_login(b"token").await.expect("db"));

    db.prune_sign_ins().await.expect("prune");
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pending_logins")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(left, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_administrators_reset_removes_the_factors_and_ends_the_sessions(pool: PgPool) {
    let db = Db::from_pool(pool);
    let other = crate::tenants::add_test_tenant(&db).await;
    let alice = user(&db, TenantId::DEFAULT, "alice").await;
    db.start_totp(TenantId::DEFAULT, alice, b"secret")
        .await
        .expect("start");
    db.confirm_totp(TenantId::DEFAULT, alice, 10, &codes(&["a"]))
        .await
        .expect("confirm");
    let session = Uuid::now_v7();
    db.start_session(
        session,
        alice,
        None,
        &crate::sessions::signed_in_now(),
        session.as_bytes(),
        OffsetDateTime::now_utc() + Duration::days(1),
    )
    .await
    .expect("session");
    let accounts = db.accounts(TenantId::DEFAULT).await.expect("accounts");
    assert!(accounts[0].authenticator_app);
    assert!(!accounts[0].webauthn);

    assert!(matches!(
        db.reset_second_factors(other, alice).await,
        Err(DbError::NotFound)
    ));
    db.reset_second_factors(TenantId::DEFAULT, alice)
        .await
        .expect("reset");
    assert_eq!(
        db.second_factors(alice).await.expect("db"),
        SecondFactorState::default()
    );
    assert!(db.totp(alice).await.expect("db").is_none());
    let live = db
        .session_authentication(TenantId::DEFAULT, session, alice)
        .await
        .expect("db");
    assert!(live.is_none());
}
